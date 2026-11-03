//! Normal-Windows preparation implemented in Rust.
//!
//! The desktop program does not delegate task creation or system preparation
//! to PowerShell.  This module owns the public `prepare`, `list-volumes`,
//! `inspect-environment` and `wim-info` commands; it uses only Rust file I/O
//! plus the Windows inbox command-line tools that perform the OS operations.

use backuprestore_core::{
    BootMode, DestinationSpec, ImageSpec, Operation, TargetRole,
    TargetSpec, Task, TaskError, TaskStore, VolumeIdentity, VolumeRoles,
    sha256_file, validate_volume_roles, write_json_atomic,
};
use chrono::Utc;
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error_code::ErrCode;
#[allow(unused_imports)]
pub(crate) use crate::environment_admin::*;
pub(crate) use crate::image_admin::wim_full_windows_probe;
pub(crate) use crate::prepare_bcd::*;
pub(crate) use crate::prepare_env::*;
pub(crate) use crate::prepare_payload::*;
pub(crate) use crate::prepare_plan::*;
pub(crate) use crate::prepare_mount::*;
pub(crate) use crate::prepare_validation::*;
pub(crate) use crate::volume_device::*;
use crate::{append_log, err, run_logged};

pub(crate) const RESERVED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub(crate) fn prepare(arguments: Vec<String>) -> Result<(), TaskError> {
    let options = parse_prepare_options(arguments)?;
    let executable_dir = executable_dir()?;
    let workspace_drive = drive_from_path(&executable_dir)?;
    validate_prepare_options_safety(&options, Some(workspace_drive))?;
    let workspace = volume_identity(workspace_drive)?;
    let target = if matches!(
        options.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) {
        volume_identity(
            options
                .target_drive
                .ok_or_else(|| err("--target-drive is required for restore operations"))?,
        )?
    } else {
        workspace.clone()
    };
    if matches!(
        options.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) && workspace.same_partition(&target)
    {
        return Err(restore_workspace_target_error(
            workspace.drive_letter.unwrap_or(workspace_drive),
        ));
    }
    // Perform the no-overwrite guard before elevation. A direct CLI call must
    // fail locally without triggering UAC or touching any boot configuration.
    // 2026-10-18：launcher-errors.log 与 GUI 读取方同走 `log_root`——这条日志
    // 记录的恰是「连任务都没建成就失败了」的最早期故障，不能放在可能正是
    // 还原目标的程序目录里。
    require_administrator(&crate::log_root::logs_dir().join("launcher-errors.log"))?;
    prepare_task(&executable_dir, &workspace, target, options)
}


fn prepare_task(
    executable_dir: &Path,
    workspace: &VolumeIdentity,
    target: VolumeIdentity,
    options: PrepareOptions,
) -> Result<(), TaskError> {
    // ★ 盘符时间线的起点必须在这里，早于任何身份探测：恢复卷的盘符是
    //   `recovery_identity()`（下面第 979 行那段之后）跑 `diskpart assign letter=`
    //   现挂的，晚一步开就正好漏掉要观测的那一次。设 `BACKUPRESTORE_LETTER_TRACE_MS`
    //   （如 200）才启采样线程；不设时只记「谁挂的、谁撤的」。
    crate::letter_trace::start(crate::log_root::logs_dir().join("drive-letter-trace.log"));
    assert_environment(options.source_drive)?;
    let source = volume_identity(options.source_drive)?;
    require_windows_volume(&source, "source")?;
    assert_bitlocker_off(options.source_drive)?;
    if workspace.is_reserved_partition() {
        return Err(err("program directory cannot be on EFI, MSR or Recovery"));
    }
    let (image_volume, image_path, image_relative) = if let Some(path) = &options.image_path {
        let (path, volume, relative) =
            image_path_info(path, options.operation != Operation::Backup)?;
        (volume, Some(path), relative)
    } else {
        (workspace.clone(), None, String::new())
    };
    if image_volume.is_reserved_partition() {
        return Err(err("image volume cannot be EFI, MSR or Recovery"));
    }
    assert_bitlocker_off(
        image_volume
            .drive_letter
            .ok_or_else(|| err("image volume has no drive letter"))?,
    )?;
    if matches!(
        options.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) {
        // Reject reserved restore targets explicitly before the BitLocker
        // probe. `manage-bde` cannot open EFI/MSR/Recovery partitions, so
        // without this early check a user selecting the EFI or Recovery
        // partition would see a confusing BitLocker error instead of the real
        // reason the target is ineligible.
        if target.is_reserved_partition() {
            return Err(restore_reserved_target_error());
        }
        assert_bitlocker_off(
            options
                .target_drive
                .ok_or_else(|| err("restore target drive is missing"))?,
        )?;
    }
    let recovery = recovery_identity()?;
    let efi = efi_identity(options.efi_drive)?;
    if workspace.same_partition(&efi) {
        return Err(err("program directory cannot be on EFI volume"));
    }
    if image_volume.same_partition(&efi) {
        return Err(err("image volume cannot be on EFI volume"));
    }
    // v1.7.11 新启动通道：注册 WinRE **全程只读**，因此
    //   - F1（还原目标 == 注册 WinRE 宿主）：格式化不再威胁「续跑启动源」（它在镜像卷上）；
    //   - F3（备份源 == 注册 WinRE 宿主）：载荷从不写注册位，镜像天然干净。
    // 两道冲突门与迁出闸门随之删除；仅保留 workspace/镜像卷不能落在注册 WinRE 分区
    // 这条基础规则（那是程序自身资产，与任务启动源无关）。
    let roles = VolumeRoles {
        workspace,
        image: &image_volume,
        source: Some(&source),
        target: matches!(
            options.operation,
            Operation::RestoreExisting | Operation::CreateSecondary
        )
        .then_some(&target),
    };
    if let Some(conflict) = validate_volume_roles(&roles, options.operation, &recovery).err() {
        return Err(err(conflict.message()));
    }
    // 准备日志先建好再跑输入校验：`validate_operation_inputs` 里的 DISM 预检
    // （镜像完整性探测）要往这里写 stdout/stderr。早先日志目录在校验**之后**
    // 才创建，那段失败就无处落盘——正是 2026-10-16 审查点名的日志缺口 1。
    // 2026-10-18：落点改走 `log_root`（用户要求：不放 PE、不放目标分区）。
    let bootstrap_log = crate::log_root::logs_dir().join("prepare-bootstrap.log");
    if let Some(parent) = bootstrap_log.parent() {
        fs::create_dir_all(parent)?;
    }
    let validation = validate_operation_inputs(
        &source,
        &target,
        &image_volume,
        image_path.as_deref(),
        &options,
        &bootstrap_log,
    )?;
    let image_acceptance = validation.acceptance;
    let store = TaskStore::new(executable_dir);
    // S5 新启动项撞名消解（docs/20261015-0725）：拟用的菜单名与现有任一启动项
    // 重名时追加 -YYYYMMDDHHMM。GUI 侧已按用户可见的方式消解过一次（幂等，
    // 已加后缀的名字不会二次加），但 CLI 直跑不过 GUI，必须在这里兜底——
    // 否则 validate 的「启动菜单出现同名项」会直接拒掉任务，而用户看到的
    // 名字是他自己起的，完全无法归因。读不到 BCD 不阻断：退化成不消解，
    // 由 validate 的同名守卫以明确错误拦住，也好过 silently 建出两个同名项。
    let options = {
        let mut options = options;
        if options.operation == Operation::CreateSecondary {
            let existing = crate::boot_menu::existing_loader_descriptions(&bootstrap_log);
            let resolved = crate::boot_menu::resolve_menu_name_collision(
                &options.boot_menu_name,
                &existing,
                chrono::Local::now(),
            );
            if resolved != options.boot_menu_name.trim() {
                append_log(
                    &bootstrap_log,
                    &crate::i18n::msg_tpl(
                        "wp_boot_menu_dup",
                        &[&options.boot_menu_name, &resolved],
                    ),
                )?;
            }
            options.boot_menu_name = resolved;
        }
        options
    };
    let cleanup =
        store.cleanup_terminal_tasks(backuprestore_core::DEFAULT_TERMINAL_TASK_RETENTION)?;
    if !cleanup.removed_task_ids.is_empty()
        || cleanup.skipped_nonterminal > 0
        || !cleanup.skipped_mounted.is_empty()
    {
        append_log(
            &bootstrap_log,
            &format!(
                "terminal task cleanup: removed={}, skipped_nonterminal={}, skipped_mounted={}",
                cleanup.removed_task_ids.len(),
                cleanup.skipped_nonterminal,
                cleanup.skipped_mounted.len()
            ),
        )?;
    }
    // 一个工作区共用同一份 EFI BCD 的 bootsequence，无法安全并发两个未终结的
    // 启动事务。载荷虽按 task ID 隔离，但那只防止文件互相覆盖，不解决 bootsequence
    // 争用；因此新任务只要发现任何未终结任务，都先停下交给用户选择继续或清理。
    // 这一步必须位于创建新 task.json、导出 BCD 和修改任何启动对象之前。
    let pending = store.incomplete_tasks()?;
    if !pending.is_empty() {
        let details = pending
            .iter()
            .map(|task| {
                let target = task
                    .target
                    .as_ref()
                    .map(|spec| spec.volume.partition_guid.as_str())
                    .unwrap_or("(无目标卷)");
                format!(
                    "id={} operation={:?} stage={:?} target={} created={}",
                    task.task_id, task.operation, task.status, target, task.created
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        append_log(
            &bootstrap_log,
            &format!("new preparation blocked by incomplete tasks: {details}"),
        )?;
        return Err(crate::errc_with(ErrCode::UnterminatedTaskExists, &[&details]));
    }
    ensure_workspace_capacity(workspace, &recovery)?;

    let boot_plan = match options.operation {
        Operation::CreateSecondary => BootMode::AddSecondary,
        _ => BootMode::ReturnExisting,
    };
    let mut task = Task::new(
        options.operation,
        backuprestore_core::BootPlan {
            mode: boot_plan,
            previous_bcd_sha256: None,
            menu_name: if options.operation == Operation::CreateSecondary {
                Some(options.boot_menu_name.clone())
            } else {
                None
            },
            // The one-time boot request is enabled only after the BCD
            // snapshot has been exported and hashed below. This lets us run
            // a complete side-effect-free task validation before any BCD or
            // WinRE mutation while still persisting a fully protected plan.
            boot_sequence_requested: false,
        },
    );
    task.source = Some(source.clone());
    task.workspace_volume = Some(workspace.clone());
    task.compress = options.compress.clone();
    match options.operation {
        Operation::Backup => {
            // 备份前镜像卷空间预检（2026-10-26）：在「决定重启去做」之前就挡掉
            // 明显装不下的目标，省得进恢复环境写了几十 GiB 才失败。★ 硬闸比例
            // 故意很低（源已用量的 5%），宁可放过也不误拒——WIM 压缩比随内容
            // 变化极大，按固定比例预估会拦掉真实可行的备份。
            if let Some(path) = image_path.as_ref() {
                ensure_backup_image_capacity(
                    options.source_drive,
                    path,
                    options.compress.as_deref().unwrap_or("fast"),
                )?;
            }
            task.destination = Some(DestinationSpec {
                volume: image_volume.clone(),
                absolute_path: image_path.clone(),
                relative_path: image_relative.clone(),
            });
            // 备份索引名与保留最近 N 个索引：写入任务供恢复执行时使用。
            task.image_name = options.image_name.clone();
            task.keep_indexes = options.keep_indexes;
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let Some(path) = image_path.as_ref() else {
                return Err(crate::errc(ErrCode::ImageFileMissing));
            };
            let file_size = fs::metadata(path)?.len();
            // 优先复用 sidecar metadata 里的权威 sha256；无 sidecar 镜像且未勾选 verify_hash 时填充合规占位，彻底避免重复读盘。
            // 例外：用户已显式接受"与副档不符的当前镜像"时，权威值改成**接受时读到的真实哈希**。
            // 否则任务里留着副档旧值 + verifyHash=true，重启后离线侧必然再次拒绝，
            // 用户的强制继续被静默撤销（历史缺陷）。
            let accepted_sha256 = image_acceptance
                .as_ref()
                .filter(|accepted| !accepted.accepted_sha256.is_empty())
                .map(|accepted| accepted.accepted_sha256.clone());
            let sha256 = if let Some(accepted) = accepted_sha256 {
                accepted
            } else if let Ok(meta) = crate::read_index_metadata(Path::new(path), options.wim_index)
            {
                meta.image_sha256
            } else if options.verify_hash {
                sha256_file(path)?
            } else {
                "0".repeat(64)
            };
            task.verify_hash = Some(options.verify_hash);
            task.image_acceptance = image_acceptance.clone();
            task.image = Some(ImageSpec {
                volume: image_volume.clone(),
                absolute_path: Some(path.clone()),
                relative_path: image_relative.clone(),
                sha256,
                size_bytes: file_size,
                index: options.wim_index,
                name: None,
            });
            task.target = Some(TargetSpec {
                volume: target.clone(),
                role: if options.operation == Operation::CreateSecondary {
                    TargetRole::NewWindows
                } else {
                    TargetRole::ExistingWindows
                },
                boot_menu_name: (options.operation == Operation::CreateSecondary)
                    .then(|| options.boot_menu_name.clone()),
                minimum_size_bytes: target.partition_size,
                // 目标性质在这里定案并落盘：RE 内 `%SystemDrive%` 是恢复环境的 RAM 盘 X:，
                // 探测不到用户的系统卷，所以离线侧只能读这个结论（见字段文档）。
                // 取不到活动系统身份时直接失败，不要退化成 false——那会让「重建唯一系统」
                // 误走第二系统收尾。
                is_active_system: target.same_partition(&active_system_volume_identity()?),
                // 格式化选项落盘（GUI 复选框 → prepare 参数 → RE 恢复链读取）
                format_target: options.format_target,
                // 「覆盖式应用到已含 Windows 卷」的路由标记（任务 #110）：
                // 由 validate 判定，RE 恢复链读取后走 wimlib 通道。
                overlay_no_acl: validation.overlay_no_acl,
            });
        }
        Operation::Probe => {}
    }

    // Validate the complete task before exporting BCD or touching WinRE. This
    // keeps malformed/reserved/undersized requests side-effect free even when
    // they arrived through the CLI instead of the GUI.
    task.validate()?;

    let task_dir = store.task_dir(&task.task_id)?;
    let prepare_log = task_dir.join("prepare.log");
    let bootstrap_bcd = executable_dir.join(format!(".backuprestore-{}.bcd", task.task_id));
    let bootstrap_arg = bootstrap_bcd.to_string_lossy().into_owned();
    run_logged("bcdedit.exe", &["/export", &bootstrap_arg], &bootstrap_log)?;
    task.boot_plan.previous_bcd_sha256 = Some(sha256_file(&bootstrap_bcd)?);
    task.boot_plan.boot_sequence_requested = true;
    task.validate()?;
    // 新启动通道：载荷放到**镜像卷**（re_staging），注册位只读，不污染系统卷。
    //
    // 2026-10-13 修正（docs/20261013-1936 对照实验 + 单变量矩阵 A/B/C）：
    // 1935 的「bootmgr 只解析系统卷盘符」结论不完整——真正的情况是
    // **盘符形式**的 ramdisk 规格只对系统卷生效，数据卷必须写
    // \Device\HarddiskVolumeN 设备路径形式（bcdedit 显示层两种都规范化成 [X:]，
    // 存储层不同，所以从回读看不出差别）。设备路径形式在数据卷上实测可引导
    // （GUID 目录 + 隐藏/系统属性 + 设备路径 = 成功；同结构换成盘符 = 回落）。
    // 修复落在 boot_entry（写规格时解析设备路径），载荷因此可以留在镜像卷：
    // 不污染 C:，单系统还原格式化目标卷也不会抹掉断电续跑的载荷。
    // 2.4.5 曾把载荷改到系统卷作权宜——那会污染 C: 且 restore-existing 断电后
    // 无法续跑，本轮回退。
    // ★ 字母会**逸出**：`boot_sdi` 把盘符拼进路径、存在结构体里，后面 prepare_payload
    //   还要按它读注册的 WinRE。显式延到进程结束（有边界：收尾统一撤销 + 崩溃兜底）。
    let (recovery_letter, lease_recovery) =
        ensure_volume_mounted_leased(&recovery, 'R', &prepare_log)?;
    // ★★ 复核结论（2026-10-28 深夜 + 2026-10-29 实机观测，见
    //    docs/20261029-0215-…-盘符观测-… 与 `letter_trace` 模块注释）：
    //    下面这行 `hold_for_process()` 在本机是**空转**，原因是：恢复卷本身就有盘符，
    //    `ensure_volume_mounted_leased` 一定走「复用既有盘符」分支 → `created=false`
    //    → 租约 `letter=None`。
    //
    //    2026-10-29 的实机时间线把「那个盘符哪来的」钉死了，答案和当初的推断不同：
    //      - 本机 WinRE 注册在 `harddisk0\partition4`，也就是 **OS 分区（C:）**，
    //        所以 `recovery_identity()` 解析出的恢复卷就是 C:，`identity_from_diskpart`
    //        在「复用已挂载分区」那一轮就命中 C:，**从不执行 `diskpart assign letter=`**；
    //      - 时间线实证：`REUSE C: disk 0 partition 4 (already mounted; no assignment)`，
    //        全程零挂载、零撤销。
    //    也就是说「准备期临时分配了一个 R:」在**本机这个布局下不存在**。它只在
    //    「WinRE 落在无盘符的专用恢复分区」时才会发生（本机分区 5 只有 840 MB，
    //    Win11 26100 的 reagentc 不接受，实测三次都坚持放回 partition4）。
    //
    //    ⚠️ 这**不是类型级恒真**，只是「:979 到 :1226 之间恢复卷一直有盘符」这个
    //       环境前提。所以这里只能放**调试期**断言——release 下编译掉，生产行为一字
    //       不变；一旦报警，说明前提变了、要去查 `identity_from_diskpart`，而不是来
    //       删这条断言。
    //    另外：`identity_from_diskpart` 里那条 `diskpart assign letter=` 确实**没有
    //    租约**（它写的是持久的 MountedDevices），在 OEM 布局上是真实的泄漏风险，
    //    只是本机走不到。要不要给它补租约，等有 OEM 布局的机器再说。
    debug_assert!(
        lease_recovery.owns_no_letter(),
        "recovery lease unexpectedly owns {recovery_letter}: the recovery volume lost its \
         drive letter between the workspace capacity check and the lease point"
    );
    lease_recovery.hold_for_process();
    let re_staging = ReStaging {
        volume: image_volume.clone(),
        boot_sdi: PathBuf::from(format!(r"{recovery_letter}:\Recovery\WindowsRE\boot.sdi")),
    };
    let result = prepare_payload(
        executable_dir,
        &store,
        &mut task,
        &recovery,
        &efi,
        &options,
        &prepare_log,
        &bootstrap_bcd,
        &re_staging,
    );
    let _ = fs::remove_file(&bootstrap_bcd);
    if let Err(error) = result {
        if task_dir.exists() {
            // 把稳定错误码一起落盘：Io/Json/InvalidTransition 的 Display 故意不带码，
            // 只有 error.code() 拿得到（界面「刷新任务状态」据此显示）。
            let _ = store.write_failure_coded(
                &mut task,
                error.code(),
                error.to_string(),
                Some(1),
            );
            let _ = append_log(&prepare_log, &format!("Preparation failed: {error}"));
        }
        return Err(error);
    }
    // recoveryLog 指向镜像同目录（如 E:\Recovery.log），与 WIM 并排便于查看；
    // GUI「刷新任务状态」据此显示。备份/还原都会把日志写到镜像同目录。
    let recovery_log_path = match options.image_path.as_ref() {
        Some(path) => Path::new(path)
            .parent()
            .map(|parent| parent.join("Recovery.log"))
            .unwrap_or_else(|| PathBuf::from("Recovery.log")),
        None => store.log_path(&task.task_id)?,
    };
    write_json_atomic(
        executable_dir.join("last-task.json"),
        &json!({
            "taskId": task.task_id,
            "operation": task.operation,
            "taskRoot": task_dir,
            "statusJson": store.status_path(&task.task_id)?,
            "recoveryLog": recovery_log_path,
            "prepareLog": prepare_log,
            "imagePath": image_path,
            "created": Utc::now(),
        }),
    )?;
    // `--json` 下用单行 JSON 取代两行 `KEY=VALUE`：判据只认字段，不再解析
    // 自然语言，也不必再区分「哪些命令有 TASK_ID 哪些没有」。
    // 非 json 模式保持旧的两行输出（既有脚本依赖它，不动）。
    // 详情上报给 `main` 统一输出（operation 由 main 从 `--operation` 参数推断，
    // 口径与 online 一致）。非 json 模式保持旧的两行输出。
    crate::cli_json::set_detail(
        Some(task.task_id.clone()),
        Some(prepare_log.display().to_string()),
    );
    if !crate::cli_json::enabled() {
        println!("TASK_ID={}", task.task_id);
        println!("TASK_ROOT={}", task_dir.display());
    }
    Ok(())
}

/// 新启动通道（v1.7.11）需要的「载荷 RE 暂存位置」信息。
///
/// * `volume` — 承载载荷 WIM 的卷（默认 = 镜像卷；本来就可写、且不被还原格式化）。
/// * `boot_sdi` — RAM 盘模板文件来源（注册卷上的 `boot.sdi`，可能不存在）。
fn executable_dir() -> Result<PathBuf, TaskError> {
    env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| err("cannot determine executable directory"))
}

fn drive_from_path(path: &Path) -> Result<char, TaskError> {
    let raw = path.to_string_lossy();
    let bytes = raw.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return Err(err("program directory must use a drive-root path"));
    }
    Ok((bytes[0] as char).to_ascii_uppercase())
}

pub(crate) fn workspace_relative(path: &Path) -> Result<String, TaskError> {
    let raw = path.to_string_lossy().replace('/', "\\");
    if raw.len() < 4 {
        return Err(err("program directory is invalid"));
    }
    let relative = raw[3..].trim_matches('\\');
    if relative.is_empty() {
        return Err(err("program directory cannot be a volume root"));
    }
    backuprestore_core::validate_relative_path(relative)?;
    Ok(relative.to_string())
}

/// 挂载一个卷并把盘符交给调用方，随租约返回。
pub(crate) fn ensure_volume_mounted_leased(
    identity: &VolumeIdentity,
    preferred: char,
    log: &Path,
) -> Result<(char, MountLease), TaskError> {
    crate::prepare_mount::ensure_volume_mounted_leased(identity, preferred, log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_options_reject_missing_image_or_invalid_drive() {
        assert!(
            parse_prepare_options(vec![
                "--operation".into(),
                "probe".into(),
                "--source-drive".into(),
                "C".into(),
            ])
            .is_ok()
        );
        assert!(
            parse_prepare_options(vec![
                "--operation".into(),
                "restore-existing".into(),
                "--source-drive".into(),
                "C".into(),
            ])
            .is_err()
        );
        assert!(
            parse_prepare_options(vec![
                "--operation".into(),
                "probe".into(),
                "--source-drive".into(),
                "CC".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn prepare_fault_options_require_efi_only_for_efi_faults() {
        let base = vec![
            "--operation".into(),
            "probe".into(),
            "--source-drive".into(),
            "P".into(),
        ];
        let mut power_loss = base.clone();
        power_loss.extend(["--test-fault".into(), "power-loss-window".into()]);
        assert!(
            parse_prepare_options(power_loss).is_ok(),
            "power-loss-window uses the real system EFI by default"
        );

        for fault in ["identity-env-mismatch", "bcdboot-failure"] {
            let mut missing_efi = base.clone();
            missing_efi.extend(["--test-fault".into(), fault.into()]);
            assert!(
                parse_prepare_options(missing_efi).is_err(),
                "{fault} must require an explicit development EFI"
            );
        }
        for fault in [
            "power-loss-target-erased",
            "power-loss-image-applied",
            "power-loss-boot-repaired",
        ] {
            let mut system_efi_fault = base.clone();
            system_efi_fault.extend(["--test-fault".into(), fault.into()]);
            assert!(
                parse_prepare_options(system_efi_fault).is_ok(),
                "{fault} must use the current system EFI by default"
            );
        }
    }
}

