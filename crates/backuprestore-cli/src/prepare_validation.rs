//! 准备阶段前置校验、环境门禁与容量断言子系统 (prepare_validation.rs)
//!
//! 收拢 `windows_prepare.rs` 中所有前置安全检查：
//! 1. `validate_operation_inputs`: 备份/还原/创建第二系统操作前置输入校验、覆盖式还原模式标记与 WIM 完整性核查；
//! 2. `assert_environment`: 验证当前宿主环境（系统卷必须配备可用的 WinRE/BCD 恢复序列）；
//! 3. `require_administrator`: 验证进程已取得管理员提权（fltmc 退出码探测）；
//! 4. `require_windows_volume`: 断言卷为非保留分区的合法 NTFS 卷；
//! 5. `assert_bitlocker_off`: 断言卷未启用 BitLocker 保护；
//! 6. `ensure_workspace_capacity`: 校验工作空间卷拥有足够的空间承载 WinRE staging；
//! 7. `ensure_backup_image_capacity`: 备份前校验镜像目标卷可用容量。

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};

use backuprestore_core::{ImageAcceptance, Operation, TaskError, VolumeIdentity};
use chrono::Utc;

use crate::error_code::ErrCode;
use crate::image_admin::{wim_full_windows_probe, ProbeOutcome};
use crate::prepare_plan::PrepareOptions;
use crate::process_runner::capture_output;
use crate::volume_device::{
    active_system_volume_identity, disk_free_space, volume_free_bytes,
};
use crate::windows_prepare::RESERVED_BYTES;
use crate::{append_log, err, sha256_file};

pub(crate) struct ValidationOutcome {
    /// 用户显式接受了与副档不符的当前镜像（原返回值）。
    pub acceptance: Option<ImageAcceptance>,
    /// 命中「覆盖式应用（取消格式化）打到已含 Windows 的目标卷」组合。
    ///
    /// 2026-10-22 起该组合**不再硬拒**（用户裁定「换 wimlib」）：
    /// 准备期在此落盘 `overlay_no_acl=true`，恢复期（WinRE 链）据此走 wimlib
    /// 通道——DISM 该组合必然 `RestoreDirMetadata 0x80070005`（PE 内对照实验
    /// 钉死，见 docs/20261016-1413 §2），wimlib 自行获取 SeRestore/SeBackup
    /// 提权且该提权覆盖 SD 还原（2026-10-23 概念验证，docs/20261022-2308：
    /// tiny + 全量两级，镜像 SD 逐字节还原），故镜像目录权限完整还原、
    /// 个别受保护目录可能保留卷上现有权限（GUI 确认框明示）。
    /// GUI 侧有显式确认框明示该语义；CLI 直跑视为高级用户
    /// 显式选择 `--skip-format` 的意图。
    /// **名字沿袭历史**：`overlay_no_acl` 里的 `no_acl` 是 v2.6.25–v2.6.26
    /// 「跳过 SD 还原」时代的残留，当前语义 = 覆盖式应用（安全描述符随镜像
    /// 还原）；不改名是为了 task.json 序列化兼容。
    pub overlay_no_acl: bool,
}

pub(crate) fn validate_operation_inputs(
    source: &VolumeIdentity,
    target: &VolumeIdentity,
    image: &VolumeIdentity,
    image_path: Option<&str>,
    options: &PrepareOptions,
    log: &Path,
) -> Result<ValidationOutcome, TaskError> {
    match options.operation {
        Operation::Probe => Ok(ValidationOutcome {
            acceptance: None,
            overlay_no_acl: false,
        }),
        Operation::Backup => {
            if image.same_partition(source) {
                return Err(err("image volume must differ from backup source"));
            }
            // 盘符在这里**显式**取，不靠 400 行外那条为 BitLocker 写的
            // `assert_bitlocker_off(...ok_or_else...)` 顺带保证：那条守卫
            // 一旦被重构掉，这里就会退化成 panic。
            let source_letter = source
                .drive_letter
                .ok_or_else(|| err("backup source volume has no drive letter"))?;
            let image_letter = image
                .drive_letter
                .ok_or_else(|| err("image volume has no drive letter"))?;
            let required = source
                .partition_size
                .saturating_sub(volume_free_bytes(source_letter)?)
                .saturating_add(RESERVED_BYTES);
            // Appending is transactional: Recovery copies the current WIM to
            // a same-volume candidate before capture. Reserve that extra copy
            // here so a full destination cannot strand the original half-way
            // through an append attempt.
            let append_candidate_bytes = image_path
                .filter(|path| Path::new(path).is_file())
                .map(|path| fs::metadata(path).map(|metadata| metadata.len()))
                .transpose()?
                .unwrap_or(0);
            let required = required.saturating_add(append_candidate_bytes);
            if volume_free_bytes(image_letter)? < required {
                return Err(err("backup destination free space is insufficient"));
            }
            Ok(ValidationOutcome {
                acceptance: None,
                overlay_no_acl: false,
            })
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            // 「覆盖式应用到已含 Windows 卷」的路由标记：命中组合时置 true，
            // 由调用方写进 `TargetSpec.overlay_no_acl` 落盘（见
            // [`ValidationOutcome`] 文档）。未命中（含 Backup）恒 false。
            let mut overlay_no_acl = false;
            if !options.allow_destructive {
                return Err(err("restore requires --allow-destructive"));
            }
            if image.same_partition(target) {
                return Err(err("image volume must differ from restore target"));
            }
            if options.operation == Operation::RestoreExisting && !source.same_partition(target) {
                return Err(err("single-system restore target must equal source"));
            }
            if options.operation == Operation::CreateSecondary && source.same_partition(target) {
                return Err(err("second-system target must differ from source"));
            }
            // 目标是不是当前正在运行的那套系统。这个结论下面要用三次：P0 安全闸、
            // 是否需要完整 Windows 镜像（P2）、以及落进任务给 RE 读的 is_active_system。
            let target_is_active = target.same_partition(&active_system_volume_identity()?);
            // 【P0 安全闸】create-secondary 绝不能把目标指向当前正在运行的系统卷。
            //
            // 在这之前唯一挡着的是上面那条 `source != target`，而它可以用「源选别的卷」
            // 绕开：实测 `--operation create-secondary --source-drive T --target-drive C`
            // 会被接受，重启进 RE 后 diskpart 直接格式化正在运行的系统盘。GUI 同样能
            // 构造（源卷选 T:、目标选 C:），因为 GUI 的离线分流只覆盖
            // backup / restore-existing，create-secondary 不走 `%SystemDrive%` 检查。
            //
            // restore-existing **不加**这条：它的目标就是当前系统卷，属于合法场景
            // （重建唯一系统，GUI 会弹窗走 PE/RE 离线链）。
            if options.operation == Operation::CreateSecondary && target_is_active {
                return Err(err(
                    "create-secondary target must not be the active Windows volume",
                ));
            }
            // 【2026-10-18 缺陷修复 H1 第二层：prepare 侧同判据】
            // restore-existing **反过来**：它的目标必须是当前正在运行的系统卷
            // （重建唯一系统）。此前这里只断言 `source == target`，而 A 入口的
            // GUI 目标自动选中会静默失效（下拉停在上一个入口选中的盘），于是
            // 「源＝目标＝某个数据盘」也能一路通到「格式化该盘 + 灌镜像」。
            // GUI 已加同判据拦截，这里是**不依赖 GUI 的最终防线**（CLI 直通、
            // 测试钩子、GUI 绕过都挡在这里）。
            //
            // 前提：`prepare` 只在线上 Windows 会话里跑（GUI/CLI 建任务阶段），
            // 故 `%SystemDrive%` 就是那套正在运行的 Windows；RE 侧执行的是
            // recover/resume，不走本函数。
            if options.operation == Operation::RestoreExisting && !target_is_active {
                return Err(err(
                    "restore-existing target must be the active Windows volume",
                ));
            }
            let path = image_path.ok_or_else(|| err("restore image is missing"))?;
            if !Path::new(path).is_file() {
                return Err(err("restore image does not exist"));
            }
            let _ = capture_output(
                "dism.exe",
                &[
                    "/Get-WimInfo",
                    &format!("/WimFile:{path}"),
                    &format!("/Index:{}", options.wim_index),
                ],
            )?;
            // 擦除前确认镜像是完整 Windows（纯 PE 走这里会在格式化后才失败）。
            //
            // 【P2】但数据卷还原必须放行：数据卷镜像本来就没有 reagentc.exe / SYSTEM
            // hive，拿完整 Windows 的标准去卡它等于禁掉整个数据卷离线通道。判据用
            // **目标卷**当前有无 SYSTEM hive，与恢复期分叉 BCDBoot 的判据完全一致
            // （`main.rs` 的 data-volume 分支）：目标不是 Windows 卷 → 这次只铺数据、
            // 不碰引导，镜像缺 Windows 标记属正常。
            //
            // 本机此前是靠「挂载报 87 → 镜像名启发式放行」才通过的，换一台 DISM 挂载
            // 可用的机器，数据卷的离线还原会被硬拒（实测路径见 docs 待改清单 P2）。
            //
            // ⚠️ create-secondary 不在放行范围内：装第二系统必须是完整 Windows，
            // 否则产出一个进不了桌面的引导项。
            //
            // ⚠️ 活动系统卷**不做文件探测**：它的 SYSTEM hive 被系统独占，`probe_file`
            // 会 `File::open` 它并撞上共享违例（os error 32），那个错误不是 NotFound、
            // 会直接把准备阶段弄失败。而活动系统卷必然是 Windows 卷，无需探测。
            let target_is_windows = target_is_active
                || crate::pe_safety::target_is_windows_volume(&PathBuf::from(format!(
                    "{}:\\",
                    target.drive_letter.ok_or_else(|| crate::errc(ErrCode::TargetLetterMissing))?
                )))?;
            // 【准备期判定 · 2026-10-22 任务 #110：原「硬拒」已改「路由」】
            // 「系统卷 + 取消格式化（覆盖式应用）」对 DISM 是确定性失败组合
            // （根因见下）；2026-10-22 起不再挡在准备期，改为在此判定并落盘
            // `overlay_no_acl`，恢复期走 wimlib 通道。判定仍必须在
            // 准备期完成：RE 内探测不到宿主卷的 Windows 属性（`%SystemDrive%`
            // 是恢复环境自己的 RAM 盘 X:），与 `is_active_system` 同一理由。
            //
            // 精确根因（2026-10-16 PE 内对照实验钉死，见 docs/20261016-1413 §2 与
            // docs/20261016-1836）：DISM 覆盖式应用要把镜像里
            // `C:\Program Files\WindowsApps\...`（MSIX 应用包目录，owner =
            // NT SERVICE\TrustedInstaller，带 MSIX 专用 ACL）的目录元数据 /
            // Security Descriptor / owner 还原到卷上；该目录受 TrustedInstaller 保护，
            // 操作系统拒绝改写其 owner/ACL → `RestoreDirMetadata:(2418): Access is
            // denied` → `0x80070005` → DISM 立即整体放弃（exit 5）。
            // 实测旁证：`md` 在该目录下能建子目录、`rd` 能删（普通写入允许），
            // **只有"改写既有受保护目录元数据"这一步被拒**；格式化能过是因为受保护
            // 目录被清空，DISM 只需"新建"（允许）而非"改写"。
            //
            // 该约束与 RE/在线/PE 环境无关、与 DISM 版本无关：PE 里同镜像同样失败。
            // `--skip-format` 只对**数据卷**（无 Windows 的卷）有效（数据卷里没有
            // WindowsApps，2026-10-14 已在 T: 验证），对**已含 Windows 的系统卷**无效。
            //
            // 判据用「目标卷当前是否含 Windows」而非「是否活动系统卷」：任何既有
            // Windows 卷都带 TrustedInstaller 保护的目录，都会命中同一根因。
            //
            // 【2026-10-22 任务 #110：硬拒改路由】用户裁定「换 wimlib」
            // 后，本组合不再 `return Err`：置 `overlay_no_acl=true` 落进任务，
            // 恢复期走 wimlib 通道（wimlib 自行获取 SeRestore/SeBackup 提权，
            // RE 内可用；2026-10-23 概念验证 docs/20261022-2308 证明该提权
            // 同时覆盖 SD 还原——镜像目录权限完整还原，个别受保护目录可能
            // 保留卷上现有权限，GUI 确认框明示该语义）。
            // CLI 直跑 --skip-format 视为高级用户显式意图。
            if crate::pe_safety::overlay_restore_on_windows_volume(
                target_is_windows,
                options.format_target,
            ) {
                overlay_no_acl = true;
                append_log(
                    log,
                    crate::i18n::msg("wp_overlay_marked"),
                )?;
            }
            if options.operation == Operation::CreateSecondary || target_is_windows {
                // 降级放行必须留痕：否则事后无法判断「门禁到底跑没跑过」。
                // 2026-10-22 教训——参数错导致权威探测从未生效，而日志里一个字都没有。
                match wim_full_windows_probe(path, options.wim_index, log)? {
                    ProbeOutcome::FullWindows => append_log(
                        log,
                        crate::i18n::msg("wp_precheck_mounted_ok"),
                    )?,
                    ProbeOutcome::QuickPass(name) => append_log(
                        log,
                        &crate::i18n::msg_tpl("wp_precheck_fast_path", &[name.as_str()]),
                    )?,
                    ProbeOutcome::HeuristicPass => append_log(
                        log,
                        crate::i18n::msg("wp_precheck_heuristic"),
                    )?,
                }
            }
            // 配套备份 metadata（.index-N.metadata.json）存在时校验 WIM 完整性；
            // 优先比对物理文件大小，避免在提权准备阶段对 58GB 镜像再次耗时 2 分钟重复读盘计算哈希。
            // 缺失时视为第三方/PE WIM（如安装 WinRE/PE 为第二系统），跳过
            // 哈希校验与目标大小预检——DISM /Get-WimInfo 已确认 WIM 可读。
            let metadata = crate::read_index_metadata(Path::new(path), options.wim_index).ok();
            // 用户显式接受当前镜像时在这里产生例外记录；只有真正越过了副档判据才记，
            // 正常通过校验的镜像不写例外（避免例外退化成默认状态）。
            let mut acceptance: Option<ImageAcceptance> = None;
            if let Some(metadata) = &metadata {
                let file_size = fs::metadata(path)?.len();
                let size_conflict = metadata.image_size > 0 && file_size != metadata.image_size;
                if size_conflict && !options.force_restore_hash {
                    return Err(err("restore image size does not match metadata"));
                }
                // 用户要求：增加一个校验选项框，不勾选时仅对比大小，勾选时对比哈希
                if options.verify_hash {
                    let expected = &metadata.image_sha256;
                    // 这一次读盘是严格哈希模式本来就要付的代价；下面复用同一个值，
                    // 不额外再读一遍 58GB 镜像。
                    let actual = sha256_file(path)?;
                    let hash_conflict = !actual.eq_ignore_ascii_case(expected);
                    if hash_conflict && !options.force_restore_hash {
                        return Err(err("restore image hash does not match metadata"));
                    }
                    if hash_conflict || size_conflict {
                        // 记录"用户看过并接受的就是这份内容"。离线侧据此核验镜像
                        // 未再变化，旧副档只留痕。
                        acceptance = Some(ImageAcceptance {
                            accepted_sha256: actual,
                            accepted_size_bytes: file_size,
                            index: options.wim_index,
                            superseded_sidecar_sha256: Some(expected.clone()),
                            accepted_at: Utc::now(),
                        });
                    }
                } else if size_conflict {
                    // 仅对比大小模式：用户接受的是"当前这个大小"，不为此多读一次全盘哈希。
                    // 留空哈希表示本次例外不覆盖哈希判据，离线侧也只走大小校验。
                    acceptance = Some(ImageAcceptance {
                        accepted_sha256: String::new(),
                        accepted_size_bytes: file_size,
                        index: options.wim_index,
                        superseded_sidecar_sha256: Some(metadata.image_sha256.clone()),
                        accepted_at: Utc::now(),
                    });
                }
            }
            let minimum = metadata
                .as_ref()
                .map(|metadata| {
                    metadata
                        .required_target_size()
                        .max(metadata.source.partition_size)
                })
                .unwrap_or(0);
            if target.partition_size < minimum {
                return Err(err("restore target is too small"));
            }
            Ok(ValidationOutcome {
                acceptance,
                overlay_no_acl,
            })
        }
    }
}

pub(crate) fn assert_environment(source_drive: char) -> Result<(), TaskError> {
    // 数据卷（无 SYSTEM hive）可在当前 Windows 在线备份/还原，无需恢复环境；
    // 仅系统卷（存在 SYSTEM hive）要求 WinRE 可用（备份/还原在 PE/RE 中执行）。
    if Path::new(&format!(
        r"{}:\Windows\System32\config\SYSTEM",
        source_drive
    ))
    .is_file()
    {
        // `reagentc /info` 有一个上下文级失败模式（退出码 9，DISM 写不了
        // `C:\Windows\Logs\DISM` 日志），**不代表 WinRE 真不可用**。所以：
        // /info 成功 → 按原判据（须报 GLOBALROOT 或 Recovery\WindowsRE）；
        // /info 失败 → 退回 BCD 能否推出 WinRE 卷来判"是否已注册"，判不出才拒绝。
        let winre_available = match capture_output("reagentc.exe", &["/info"]) {
            Ok(reagent) => reagent.contains("GLOBALROOT") || reagent.contains("Recovery\\WindowsRE"),
            Err(_) => crate::windows_prepare::recovery_identity_from_bcd().is_ok(),
        };
        if !winre_available {
            return Err(err("Windows RE is disabled or unavailable"));
        }
    }
    Ok(())
}

/// 【2026-10-17 第二轮核查，日志缺口 6】`fltmc` 的输出落盘。
///
/// 此前三个流全是 `Stdio::null()`，只按退出码判断 —— 提权失败时用户看到
/// 「需要管理员权限」，而日志里**看不出到底是没提权、还是 fltmc 本身不可用**
/// （后者在某些精简环境里会出现，此时真正的修法完全不同）。
pub(crate) fn require_administrator(log: &Path) -> Result<(), TaskError> {
    // 【2026-10-18 缺陷修复 M10】先跑真正的权限判据（`fltmc` 的退出码），
    // 日志写入单独容错。此前用 `capture_logged`：它第一行就是
    // `append_log(log, …)?`，一旦日志不可写（目录满 / ACL 限制 / `logs_dir`
    // 回退到不可写目录），错误会在 `fltmc` **之前**就返回，被这里一律翻成
    // 「administrator elevation is required」——用户反复提权仍然失败，
    // 真正的原因（日志写不了）被彻底带偏。
    // 用不写日志的 `capture_output`（非零退出即 Err），把「提权不够」与「日志写不了」
    // 两种失败分开：只有前者才报「需要管理员」。
    match capture_output("fltmc.exe", &[]) {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = append_log(log, &format!("administrator check failed: {error}"));
            Err(err("administrator elevation is required"))
        }
    }
}

pub(crate) fn require_windows_volume(identity: &VolumeIdentity, name: &str) -> Result<(), TaskError> {
    if identity.is_reserved_partition() || !identity.filesystem.eq_ignore_ascii_case("NTFS") {
        return Err(err(&format!("{name} must be a non-reserved NTFS volume")));
    }
    Ok(())
}

pub(crate) fn assert_bitlocker_off(letter: char) -> Result<(), TaskError> {
    // `-protectionaserrorlevel` has inconsistent exit-code semantics across
    // Windows releases. Parse the inbox status report conservatively instead:
    // only a clearly unencrypted (0.0%) volume is allowed. Any other result,
    // including an unreadable or partially encrypted report, is rejected.
    let output = capture_output("manage-bde.exe", &["-status", &format!("{letter}:")])?;
    if output.contains("0.0%") && !output.contains("100.0%") {
        Ok(())
    } else {
        Err(err(&format!(
            "BitLocker protection is enabled or volume state is unknown on {letter}:"
        )))
    }
}

pub(crate) fn ensure_workspace_capacity(
    workspace: &VolumeIdentity,
    recovery: &VolumeIdentity,
) -> Result<(), TaskError> {
    let workspace_letter = workspace
        .drive_letter
        .ok_or_else(|| err("workspace volume has no drive letter"))?;
    let recovery_letter = recovery
        .drive_letter
        .ok_or_else(|| err("Recovery volume has no drive letter"))?;
    let registered_wim = PathBuf::from(format!(
        r"{}:\Recovery\WindowsRE\Winre.wim",
        recovery_letter
    ));
    let wim_size = fs::metadata(&registered_wim)
        .map_err(|_| err("registered WinRE image is unavailable for workspace capacity check"))?
        .len();
    let required = wim_size.saturating_mul(2).saturating_add(256 * 1024 * 1024);
    let available = volume_free_bytes(workspace_letter)?;
    if available < required {
        return Err(err(&format!(
            "program directory volume has insufficient free space for WinRE staging: {available} < {required}"
        )));
    }
    Ok(())
}

/// 备份前镜像卷空间预检：镜像所在卷可用空间低于硬闸 → 带码拒绝。
///
/// 与在线通道（`online_operation::capture`）用的是同一对 core 纯函数，保证两条
/// 链阈值一致。读不到卷空间（卷不存在/权限）时**静默放过**：预检只是提前止损，
/// 不是权威判据，真正的权威是捕获过程本身与它的回滚。
pub(crate) fn ensure_backup_image_capacity(
    source_letter: char,
    image_path: &str,
    compress: &str,
) -> Result<(), TaskError> {
    let image_letter = match Path::new(image_path)
        .parent()
        .and_then(|parent| parent.to_string_lossy().chars().next())
        .filter(|c| c.is_ascii_uppercase())
    {
        Some(letter) => letter,
        // 非盘符路径（UNC 等）：无法判定卷，放过。
        None => return Ok(()),
    };
    let (source_total, source_free, available) = match (
        disk_free_space(source_letter),
        volume_free_bytes(image_letter),
    ) {
        (Ok((total, free)), Ok(available)) => (total, free, available),
        _ => return Ok(()),
    };
    let used = source_total.saturating_sub(source_free);
    // 判定与阈值都来自 core 的同一对纯函数（与在线通道共用），这里不写不等式——
    // 抄两遍就没法给「拦下」写判据（见 backup_space_shortfall 的注释）。
    if let Some(floor) = backuprestore_core::backup_space_shortfall(available, used) {
        return Err(crate::errc_with(
            ErrCode::BackupSpaceInsufficient,
            &[
                &crate::text_parsing::format_bytes(Some(available)),
                &crate::text_parsing::format_bytes(Some(floor)),
                &crate::text_parsing::format_bytes(Some(used)),
            ],
        ));
    }
    // 低于保守估计只做记录不做拦截（估计值本身对含大量媒体的数据卷可能偏低）。
    let estimated = backuprestore_core::estimate_backup_bytes(used, compress);
    if available < estimated {
        let dir = crate::log_root::logs_dir();
        {
            let _ = append_log(
                &dir.join("prepare.log"),
                &crate::i18n::msg_tpl(
                    "ol_backup_space_warn",
                    &[
                        &crate::text_parsing::format_bytes(Some(available)),
                        &crate::text_parsing::format_bytes(Some(estimated)),
                        &crate::text_parsing::format_bytes(Some(used)),
                    ],
                ),
            );
        }
    }
    Ok(())
}
