//! 在线入口的执行事务：只在捕获、保留策略、校验和副档落盘全部成功后返回成功。
use crate::windows_command::{self, INFINITE};
use backuprestore_core::{BackupMetadata, TaskError, VolumeIdentity, image_metadata as im};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) struct OnlineOpParams {
    pub operation: String,
    pub source_drive: String,
    pub target_drive: String,
    pub image_path: String,
    pub index: String,
    pub compress: String,
    pub image_name: String,
    pub keep_indexes: Option<u32>,
    /// B（还原数据到非 C 盘）在线通道的「格式化目标分区」选项（2026-10-14 用户
    /// 拍板：B 的正确语义是格式化后还原，GUI 默认勾选）。勾选时先 diskpart
    /// 快速格式化并校验成功，再应用镜像；离线通道（RE 内）恒格式化不受此开关影响。
    pub format_target: bool,
}
pub(crate) struct OnlineResult {
    pub success: bool,
    pub detail: String,
}
struct ProgressGuard(std::sync::Arc<crate::recovery_progress::ProgressShared>);
impl Drop for ProgressGuard {
    fn drop(&mut self) {
        crate::recovery_progress::request_close(&self.0);
    }
}
fn log(path: &Path, message: &str) -> Result<(), TaskError> {
    crate::append_log(path, message)
}

/// STEP 1/4 的开场白按 `operation` 分叉（2026-10-15 用户裁定 A3）。
///
/// 备份与还原共用 `online_operation` 模块，但两者实质动作相反：备份只读源卷写镜像，
/// 还原则会格式化目标卷并改写引导。此前共用一句还原措辞，备份用户会以为自己的盘
/// 要被格式化；且旧还原措辞里「不改动主系统默认启动项」在 v2.6.1 default 归属翻转后
/// 已是**事实错误**（还原后 default 指向本次还原的新系统），一并订正。
fn step1_notice(operation: &str) -> String {
    if operation == "backup" {
        "STEP 1/4 校验参数和卷身份；备份只读取源卷并写入镜像，不格式化任何卷、不改写启动引导"
            .to_string()
    } else {
        "STEP 1/4 校验参数和卷身份；还原到另一 Windows 卷时会格式化目标卷并写入引导，\
         还原完成后默认启动项指向本次还原的系统（原系统仍保留在菜单中）"
            .to_string()
    }
}

/// 在线任务本轮的工作目录（日志 + 引导方案 session 都放这里）。
///
/// 【2026-10-16 审查修订，日志缺口 9】此前固定写 `%TEMP%\BackupRestore-online-*`：
/// 磁盘清理、甚至「把含 TEMP 的卷还原掉」这类操作都会把完整现场连带删掉，
/// 事后只剩 `gui.log` 里一行含路径的摘要，而现场恰恰是排查还原失败唯一的材料。
/// 【2026-10-18 用户要求】落点再搬到 `log_root::logs_dir()`（不放 PE、
/// 不放目标分区）。选址规则见 `online_log`（纯逻辑 + 本机单测），这里只消费结果。
pub(crate) fn execute(params: &OnlineOpParams) -> OnlineResult {
    let (dir, placement_note) = match crate::online_log::resolve_dir() {
        Ok(found) => found,
        Err(error) => {
            return OnlineResult {
                success: false,
                detail: format!("无法创建日志目录：{error}"),
            };
        }
    };
    let path = dir.join("Online.log");
    // 位置说明写进本轮日志第一行：日志自己说明自己住在哪，现场才找得到。
    // 写失败不阻断（真的写不进盘会由随后的 STEP 1 日志如实暴露）。
    let _ = log(
        &path,
        &if placement_note.is_empty() {
            format!("日志位置：{}（统一日志根，不随 %TEMP% 清理丢失）", path.display())
        } else {
            format!("日志位置：{}；{}", path.display(), placement_note)
        },
    );
    let progress = ProgressGuard(crate::recovery_progress::spawn(
        path.clone(),
        Some(if params.operation == "backup" {
            "ONLINE_BACKUP"
        } else {
            "ONLINE_RESTORE"
        }),
    ));
    let result = log(&path, &step1_notice(&params.operation))
        .and_then(|()| execute_inner(params, &path, &progress.0));
    let mut success = result.is_ok();
    let final_message = match result {
        Ok(()) => "操作完成：命令、回读和副档簿记均已通过".to_string(),
        Err(error) => format!("[ERROR] 在线操作未完成：{error}；不要将部分产物视为有效备份"),
    };
    let log_error = log(&path, &final_message).err();
    drop(progress);
    if log_error.is_some() {
        success = false;
    }
    // 弹窗只显示摘要，完整输出始终保留在本轮独立日志，避免共享临时文件互相覆盖。
    OnlineResult {
        success,
        detail: format!(
            "{final_message}\n日志：{}{}",
            path.display(),
            log_error
                .map(|e| format!("\n写最终日志失败：{e}"))
                .unwrap_or_default()
        ),
    }
}
fn drive(value: &str) -> Result<char, TaskError> {
    let raw = value.trim_end_matches(':');
    if raw.len() != 1 || !raw.as_bytes()[0].is_ascii_alphabetic() {
        return Err(crate::err("invalid drive letter"));
    }
    Ok(raw.chars().next().unwrap().to_ascii_uppercase())
}
fn dism(args: &[String], log_path: &Path) -> Result<(), TaskError> {
    // 【2026-10-16 审查修订，卡死 6】等待上限可配置（`dism_timeout`），
    // 默认仍是 INFINITE、与旧版行为一致。此前是硬编码 INFINITE：DISM 挂死则
    // 工作线程永久等待，「创建任务」按钮与状态永远停在「执行中」。
    // 在线路径前后各有一条日志，所以比 prepare 路径强；缺的就是这个超时。
    let wait_ms = crate::dism_timeout::wait_ms();
    log(
        log_path,
        &format!(
            "DISM 开始（等待上限 {}）：{args:?}",
            if wait_ms == crate::windows_command::INFINITE {
                "不限制".to_string()
            } else {
                format!("{wait_ms} 毫秒")
            }
        ),
    )?;
    let outcome = windows_command::run_program("dism.exe", args, Some(log_path), wait_ms);
    log(log_path, &format!("DISM 结果：{outcome}"))?;
    if !outcome.is_success() {
        return Err(crate::err(&format!("DISM failed: {outcome}")));
    }
    Ok(())
}
fn indexes(image: &Path, log_path: &Path) -> Result<Vec<u32>, TaskError> {
    crate::wim_indexes(image, log_path)
}

/// 跑一次 `dism /Get-WimInfo` 并取回原始输出。
///
/// 与其它 DISM 调用**共用同一次执行**是硬要求：DISM 读 WIM 需要独占访问，
/// 前一个 DISM 主进程退出后句柄未必立刻释放，紧接着再对同一 WIM 跑一次会报
/// "另一个程序正在使用此文件"（2026-10-12 实机还原在 STEP 1 之后 2 毫秒踩到）。
/// `index` 为 Some 时带 `/Index:N`，DISM 才会输出 Edition/Version 等完整字段。
fn run_dism_wim_info(
    image: &Path,
    index: Option<u32>,
    log_path: &Path,
) -> Result<String, TaskError> {
    let out_file = cmd_out_file(
        log_path
            .parent()
            .ok_or_else(|| crate::err("日志目录缺失"))?,
    );
    let mut args = vec![
        "/English".to_string(),
        "/Get-WimInfo".to_string(),
        format!("/WimFile:{}", image.display()),
    ];
    if let Some(index) = index {
        args.push(format!("/Index:{index}"));
    }
    run_capture("dism.exe", &args, &out_file, false)
}

/// 从 DISM 输出里取出全部索引号，拒绝空列表或含 0 的结果。
fn parse_wim_index_list(output: &str) -> Result<Vec<u32>, TaskError> {
    let mut list = crate::windows_prepare::parse_dism_images(output)?
        .iter()
        .filter_map(|image| image.get("ImageIndex").and_then(serde_json::Value::as_u64))
        .filter_map(|index| u32::try_from(index).ok())
        .collect::<Vec<_>>();
    list.sort_unstable();
    list.dedup();
    if list.is_empty() || list.contains(&0) {
        return Err(crate::err("DISM returned invalid WIM indexes"));
    }
    Ok(list)
}

/// 列出全部索引，同时取回**带 `/Index:N` 的完整输出**（含 Edition/Version 行）。
///
/// 必须带 `/Index:N`：不带它 DISM 只输出摘要（Index/Name/Description/Size），
/// 没有版本字段，版本名就无从解析。2026-10-12 实机因此拿到
/// `versioned_name = None`，白丢一次拿到 "Professional 10.0.26200" 的机会。
fn indexes_with_info(
    image: &Path,
    log_path: &Path,
    index: u32,
) -> Result<(Vec<u32>, String), TaskError> {
    let output = run_dism_wim_info(image, Some(index), log_path)?;
    Ok((parse_wim_index_list(&output)?, output))
}

/// 从 WIM 本身解析版本化菜单名，如 `Professional 10.0.26200`。
///
/// 数据源按优先级：先镜像副档（备份时记录），副档是占位值时退到 WIM 自身的
/// Edition/Version（映像内部带的，不依赖我们备份时写了什么，对第三方 WIM 同样有效）。
///
/// `wim_info` 是 `indexes_with_info()` 已经抓到的 DISM 输出，传进来只为解析，
/// **不再自己跑 DISM**：DISM 读 WIM 需要独占访问，紧邻的第二次调用会报
/// "另一个程序正在使用此文件"（2026-10-12 实机还原打完版本名后 2 毫秒即失败）。
///
/// **必须过滤占位值 `unknown`**：本程序自己写副档时，`windows_edition` /
/// `windows_build` 目前全是硬编码的 `"unknown"`（capture、image_metadata.rs 的
/// 默认值，以及 main.rs 那个没有任何生产者的 `context_value("WINDOWS_EDITION", ...)`）。
/// 不过滤的话这里会拼出字面的 "unknown unknown" 并写进启动菜单——比不起名更糟。
fn read_image_version_name(image: &Path, index: u32, wim_info: &str) -> Option<String> {
    /// 判断字段是否是可用的真实值（非空、非占位）。
    fn usable(value: &str) -> bool {
        let value = value.trim().trim_matches(|c| c == '<' || c == '>');
        !value.is_empty()
            && !value.eq_ignore_ascii_case("unknown")
            && !value.eq_ignore_ascii_case("n/a")
            && !value.eq_ignore_ascii_case("undefined")
    }
    /// 把 edition/build 两段拼成菜单名，两段都不可用时返回 None。
    fn combine(edition: &str, build: &str) -> Option<String> {
        let name = match (usable(edition), usable(build)) {
            (true, true) => format!("{edition} {build}"),
            (true, false) => edition.trim().to_string(),
            (false, true) => format!("Windows {}", build.trim()),
            (false, false) => return None,
        };
        let trimmed = name.trim();
        (!trimmed.is_empty()).then(|| trimmed.chars().take(60).collect())
    }
    if let Ok(metadata) = im::read_index_metadata(image, index)
        && let Some(name) = combine(
            metadata.windows_edition.trim(),
            metadata.windows_build.trim(),
        )
    {
        return Some(name);
    }
    // 副档是占位值，退到 WIM 自身。
    //
    // **必须跳过输出最前面 DISM 自己的版本行**（"Version: 10.0.26100.9549" 出现在
    // "Details for image" 之前）：那一行是 dism.exe 这个程序的版本，不是镜像版本。
    // 实机测试曾因此把菜单名写成 "Professional 10.0.26100.9549"。
    let details = wim_info
        .split_once("Details for image")
        .map(|(_, rest)| rest)
        .unwrap_or(wim_info);
    let text_field = |label: &str| -> String {
        details
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.trim().eq_ignore_ascii_case(label))
            .map(|(_, value)| value.trim().to_string())
            .unwrap_or_default()
    };
    // DISM 也可能输出 JSON（本项目 parse_dism_images 消费的形态）。
    let json_field = |label: &str| -> String {
        serde_json::from_str::<serde_json::Value>(wim_info)
            .ok()
            .and_then(|value| {
                value
                    .get("images")
                    .and_then(|images| images.as_array())
                    .and_then(|images| images.first().cloned())
                    .and_then(|image| image.get(label).cloned())
            })
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default()
    };
    let edition = [text_field("Edition"), json_field("EditionId")]
        .into_iter()
        .find(|value| usable(value))
        .unwrap_or_default();
    let build = [text_field("Version"), json_field("ImageVersion")]
        .into_iter()
        .find(|value| usable(value))
        .unwrap_or_default();
    combine(&edition, &build)
}

/// `progress` 用于在第二阶段发布引导修复会话（还原到另一个 Windows 卷时）。
fn execute_inner(
    params: &OnlineOpParams,
    log_path: &Path,
    progress: &std::sync::Arc<crate::recovery_progress::ProgressShared>,
) -> Result<(), TaskError> {
    backuprestore_core::validate_absolute_path(&params.image_path)?;
    let image = Path::new(&params.image_path);
    let affected = drive(if params.operation == "backup" {
        &params.source_drive
    } else {
        &params.target_drive
    })?;
    let active = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    if affected == drive(&active)? {
        return Err(crate::err(
            "active Windows volume must be processed offline",
        ));
    }
    let identity = crate::windows_prepare::volume_identity(affected)?;
    if !identity.is_complete() {
        return Err(crate::err("online volume identity is incomplete"));
    }
    let image_drive = image
        .to_string_lossy()
        .chars()
        .next()
        .ok_or_else(|| crate::err("missing image volume"))?;
    let image_volume = crate::windows_prepare::volume_identity(image_drive)?;
    if identity.same_partition(&image_volume) {
        return Err(crate::err(
            "image and affected volume must be on different partitions",
        ));
    }
    let root = PathBuf::from(format!("{affected}:\\"));
    log(log_path, &format!("卷身份：{identity:?}"))?;
    match params.operation.as_str() {
        "backup" => {
            let (total, free) = crate::windows_prepare::disk_free_space(affected)?;
            capture(
                params,
                &root,
                identity,
                total.saturating_sub(free),
                log_path,
            )
        }
        "restore-existing" => {
            let index: u32 = params
                .index
                .parse()
                .map_err(|_| crate::err("invalid restore index"))?;
            let (wim_indexes, wim_info) = indexes_with_info(image, log_path, index)?;
            if !wim_indexes.contains(&index) {
                return Err(crate::err("restore index missing from WIM"));
            }
            // 目标若本身是另一个 Windows 卷：完整走 格式化 → 应用 → 修复引导，
            // 与 PE 入口对齐（目标都是非当前运行的第二系统）。断电风险与 PE 相同——
            // 第二系统即使中途失败也只是不可启动，主系统仍在，重新还原即可，故不引入状态机。
            if crate::pe_safety::target_is_windows_volume(&root)? {
                return restore_to_windows_volume(
                    params, &root, identity, index, log_path, progress, &wim_info,
                );
            }
            // 「格式化目标分区」选项（默认勾选）：先 diskpart 快速格式化并**校验成功**
            // （同分区 GUID/几何不变 + 卷序列号已改变 + 根目录可读；2026-10-14 起不再强制 NTFS），失败即中止
            // ——格式化没成功绝不能往旧数据上叠镜像。格式化会换卷序列号，因此后续
            // 应用后的身份比对基准切换为格式化后的新身份（分区身份不变，仍锁同一分区）。
            let baseline = if params.format_target {
                let dir = log_path
                    .parent()
                    .ok_or_else(|| crate::err("日志目录缺失"))?;
                log(
                    log_path,
                    "STEP 2/4 格式化目标卷（diskpart quick）并校验格式化成功",
                )?;
                let script = crate::diskpart_format_script(
                    identity.disk_number,
                    identity.partition_number,
                )?;
                let script_path = dir.join("format.txt");
                fs::write(&script_path, script)?;
                run_capture(
                    "diskpart.exe",
                    &["/s".into(), script_path.to_string_lossy().into_owned()],
                    &cmd_out_file(dir),
                    true,
                )?;
                let formatted = crate::windows_prepare::refresh_identity(&identity)?;
                backuprestore_core::operation_safety::verify_formatted_volume(
                    &identity, &formatted,
                )?;
                // 根目录可读性检查移到下面，改用**卷 GUID 路径**做（见 M6 注释）：
                // 盘符在格式化后可能被重排到别的卷，用它做检查等于没检查。
                log(
                    log_path,
                    &format!(
                        "格式化校验通过：序列号 {} -> {}，分区身份不变",
                        identity.volume_serial, formatted.volume_serial
                    ),
                )?;
                formatted
            } else {
                identity
            };
            // 【2026-10-18 缺陷修复 M6】应用目标改用**卷 GUID 路径**（`\\?\Volume{…}\`）。
            // 此前 `/ApplyDir` 用的是格式化**之前**取定的盘符路径（`{affected}:\`）：
            // 格式化会换卷序列号、也可能让 mountmgr 把这个盘符重排到别的卷，
            // 那时 DISM 就把镜像写到了错误的卷上，而事后校验（下面的
            // `volume_identity(affected)`）只能「写完之后才发现」——破坏性写入
            // 已经完成。系统卷分支（`restore_to_windows_volume`）早已改用
            // `volume_path()`，这里与之对齐。
            let apply_root = PathBuf::from(
                baseline
                    .volume_path()
                    .ok_or_else(|| crate::err("目标卷缺少稳定 GUID 路径，拒绝按盘符应用镜像"))?,
            );
            fs::read_dir(&apply_root)?;
            log(
                log_path,
                if params.format_target {
                    "STEP 2.5/4 应用镜像（数据卷已按选项格式化后写入）"
                } else {
                    "STEP 2/4 还原镜像（数据卷在线覆盖，不格式化、不修复引导）"
                },
            )?;
            dism(
                &[
                    "/English".into(),
                    "/Apply-Image".into(),
                    format!("/ImageFile:{}", image.display()),
                    format!("/Index:{index}"),
                    // M6：按卷 GUID 路径应用，不按盘符（盘符可能在格式化后被重排）。
                    format!("/ApplyDir:{}", apply_root.display()),
                    "/CheckIntegrity".into(),
                    "/Verify".into(),
                ],
                log_path,
            )?;
            log(
                log_path,
                "STEP 3/4 回读目标卷身份；DISM 已执行完整性和应用校验",
            )?;
            let after = crate::windows_prepare::volume_identity(affected)?;
            if !baseline.same_partition(&after)
                || baseline.partition_offset != after.partition_offset
                || baseline.partition_size != after.partition_size
                || baseline.volume_serial != after.volume_serial
            {
                return Err(crate::err("restore target identity changed during apply"));
            }
            // 数据卷并不包含 Windows 内核，不能用 ntoskrnl.exe 存在与否判断数据还原成功。
            log(
                log_path,
                if params.format_target {
                    "STEP 4/4 完成在线还原（已格式化后写入）；未执行系统引导修复"
                } else {
                    "STEP 4/4 完成在线还原；未执行格式化或系统引导修复"
                },
            )
        }
        // C（他盘系统还原）的在线通道：无论目标当前是不是 Windows 卷，都走
        // 「格式化 → 应用 → 盘符重绑 → bcdboot → 菜单命名」全链路——这条入口的
        // 语义就是把完整 Windows 装到另一个分区。与 restore-existing 在线分支的
        // 差异：那条按目标现状分叉（数据卷走覆盖），这里语义就是装系统。
        // execute_inner 顶部的 affected==活动系统 拒绝已保证目标不是当前系统盘。
        "create-secondary" => {
            let index: u32 = params
                .index
                .parse()
                .map_err(|_| crate::err("invalid restore index"))?;
            let (wim_indexes, wim_info) = indexes_with_info(image, log_path, index)?;
            if !wim_indexes.contains(&index) {
                return Err(crate::err("restore index missing from WIM"));
            }
            log(
                log_path,
                "STEP 2/4 在线部署第二系统（格式化 → 应用 → 修复引导，不重启）",
            )?;
            restore_to_windows_volume(
                params, &root, identity, index, log_path, progress, &wim_info,
            )
        }
        _ => Err(crate::err("unsupported online operation")),
    }
}

/// 在线还原到一个「本身就是 Windows 卷」的目标：完整执行 格式化 → 应用镜像 → 修复引导，
/// 与 PE 入口对齐（目标都是非当前运行的第二系统）。断电风险与 PE 相同——
/// 第二系统即使中途失败也只是不可启动，主系统仍在，重新还原即可，故不引入状态机。
fn restore_to_windows_volume(
    params: &OnlineOpParams,
    root: &Path,
    target: VolumeIdentity,
    index: u32,
    log_path: &Path,
    progress: &std::sync::Arc<crate::recovery_progress::ProgressShared>,
    wim_info: &str,
) -> Result<(), TaskError> {
    let image: &Path = Path::new(&params.image_path);
    let dir = log_path
        .parent()
        .ok_or_else(|| crate::err("日志目录缺失"))?;
    // 【准备期判定 · 2026-10-22 任务 #110：原「硬拒」已改「路由」】
    // 本函数只在目标卷**已含 Windows** 时进入（execute_inner 按
    // `target_is_windows_volume` 分流）。此时「取消格式化（覆盖式应用）」对 DISM
    // 是确定性失败组合：DISM 要把镜像里 `Program Files\WindowsApps\...`（MSIX
    // 包目录，owner=TrustedInstaller、带 MSIX 专用 ACL）的目录元数据还原到卷上，
    // 而该目录受 TrustedInstaller 保护、拒绝改写 owner/ACL →
    // `RestoreDirMetadata` 被拒 `0x80070005` → exit 5。精确根因与离线侧同源：
    // 见 docs/20261016-1413 §2（PE 内 F: 成功 / C: 秒退对照实验）与
    // docs/20261016-1836。原实现在此处 `return Err`（在线态硬拒）；2026-10-22
    // 起改为路由到 wimlib --no-acls，判定逻辑见下方 #110 注释。
    // 判据与离线侧共用同一纯函数（`pe_safety`），避免两处文案分叉；
    // 第一个参数传**真实探测结果**（2026-10-18 设计观察 7：C 入口目标常态是
    // 空分区，硬编码 true 会用事实错误的文案拒绝合法组合）。
    //
    // 【2026-10-22 任务 #110：硬拒改路由】命中「已含 Windows + 取消格式化」时
    // 不再 `return Err`：改走 wimlib --no-acls 通道（DISM 该组合必然
    // RestoreDirMetadata 0x80070005，PE 内对照实验钉死，docs/20261016-1413 §2；
    // wimlib 跳过 SD 还原且自行获取 SeRestore/SeBackup 提权，POC 见
    // docs/20261022-1427）。代价（镜像目录权限不还原、MSIX 应用权限可能异常）
    // 由 GUI 确认框明示；CLI 直跑 --skip-format 视为高级用户显式意图。
    let target_contains_windows = crate::pe_safety::target_is_windows_volume(root)?;
    let overlay_no_acl = crate::pe_safety::overlay_restore_on_windows_volume(
        target_contains_windows,
        params.format_target,
    );
    if overlay_no_acl {
        log(
            log_path,
            "覆盖式应用（取消格式化）命中已含 Windows 的目标卷：不再硬拒，\
             本次将走 wimlib --no-acls 通道（镜像目录权限不还原）",
        )?;
    }
    // 擦除前预检（2.4.2）：在线 create-secondary/restore 拿纯 PE WIM 一样会
    // "格式化之后才失败"（STEP 3.5 盘符绑定需要完整 Windows 注册表）。
    // 与 prepare 链共用同一判据：缺 reagentc.exe 或 SYSTEM hive 直接拒绝。
    match crate::windows_prepare::wim_full_windows_probe(&params.image_path, index, log_path)? {
        crate::windows_prepare::ProbeOutcome::FullWindows => {
            log(log_path, "镜像完整性预检：只读挂载确认完整 Windows")?;
        }
        crate::windows_prepare::ProbeOutcome::QuickPass(name) => {
            log(
                log_path,
                &format!(
                    "镜像完整性预检：镜像名「{name}」符合完整 Windows 特征，\
                     走快速通道放行（跳过挂载，省一次大镜像挂载）"
                ),
            )?;
        }
        crate::windows_prepare::ProbeOutcome::HeuristicPass => {
            log(
                log_path,
                "镜像完整性预检：挂载探测不可用（如本机有附加 VHD），按镜像名启发式放行；恢复期注册检查兜底",
            )?;
        }
    }
    let esp = crate::windows_prepare::esp_identity_without_drive_letter()?;
    let esp_letter = crate::windows_prepare::ensure_volume_mounted(&esp, 'S', log_path)?;
    crate::pe_safety::verify_binding(
        &esp,
        &crate::windows_prepare::volume_identity(esp_letter)?,
        true,
    )?;
    let store = format!("{esp_letter}:\\EFI\\Microsoft\\Boot\\BCD");
    // 给"本次还原的目标"起名的首选来源是镜像副档里的 windows_edition +
    // windows_build（备份时记录的真实版本，如 "Windows 11 Pro" + "26100.9549"）。
    // 这比读盘符/日期信息量大得多，且此刻目标卷还没格式化，读不到 ntoskrnl。
    let versioned_name = read_image_version_name(image, index, wim_info);
    if let Some(name) = &versioned_name {
        log(log_path, &format!("引导菜单将使用镜像版本名：{name}"))?;
    }
    // 引导修复方案必须在格式化前准备好：格式化后目标卷是空卷，探测不到任何系统。
    // 这里只落盘候选、不写 BCD；实际写入延后到 STEP 4 映像应用完成之后。
    // task_id 用日志目录名：同一次在线操作只有一个目录，足够唯一定位 boot-menu-plan.json。
    let boot_menu_session = crate::boot_menu::BootMenuSession::prepare(
        &target,
        &dir.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "online-restore".to_string()),
        dir,
        log_path,
        versioned_name.as_deref(),
    )?;
    // 在线通道只服务第二系统（活动系统盘必须在离线环境处理，execute_inner 顶部的
    // affected==活动系统 拒绝已保证目标不是当前系统盘）。
    // default 归属=指向本次还原出来的系统（2026-10-15 用户规则，替代原先的
    // "保持原主系统"；docs/20261015-0725 S1，单一权威：apply 只执行不推断）。
    // 该规则已固化在 `boot_menu::decide_default` 里，无需在此显式设定。
    // 调用方显式指定了镜像名时以它为准（用户自己的命名优先于自动版本名）。
    // 目标下标不硬编码：默认顺序里它在最后，用户也可能上移。
    //
    // S5 撞名消解：在线路径没有离线 prepare 的那道 CLI 兜底，而 rename 发生
    // 在 BootMenuSession::prepare 之后——不在这里消解，同名项要等到 STEP 4
    // 的 validate 才报错，那时目标卷已格式化、镜像已应用，失败代价极高。
    // 两级对照：先 live BCD（用户定义的「现有启动菜单」），闭包内再对照
    // 方案其它条目（覆盖"有 Windows 但 BCD 无条目"的候选卷）。
    if !params.image_name.is_empty() {
        let existing = crate::boot_menu::existing_loader_descriptions(log_path);
        let pre_resolved = crate::boot_menu::resolve_menu_name_collision(
            &params.image_name,
            &existing,
            chrono::Local::now(),
        );
        if pre_resolved != params.image_name.trim() {
            log(
                log_path,
                &format!(
                    "启动菜单名与现有启动项重名，已消解：{} -> {}",
                    params.image_name, pre_resolved
                ),
            )?;
        }
        boot_menu_session.edit(move |plan| {
            if let Some(position) = plan.restore_target_index() {
                let others: Vec<String> = plan
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != position)
                    .map(|(_, entry)| entry.display_name.clone())
                    .collect();
                let name = crate::boot_menu::resolve_menu_name_collision(
                    &pre_resolved,
                    &others,
                    chrono::Local::now(),
                );
                plan.rename(position, &name);
            }
        });
    }
    progress.set_boot_menu(std::sync::Arc::clone(&boot_menu_session));
    log(
        log_path,
        if params.format_target {
            "STEP 2/4 还原镜像前先快照引导状态并格式化目标卷"
        } else {
            "STEP 2/4 还原镜像前先快照引导状态（按选项跳过格式化，覆盖式应用）"
        },
    )?;
    let before = run_bcd_edit(
        &[
            "/store".into(),
            store.clone(),
            "/enum".into(),
            "{bootmgr}".into(),
            "/v".into(),
        ],
        dir,
    )?;
    let old = crate::parse_boot_manager_state(&before)
        .ok_or_else(|| crate::err("引导管理器状态未知，拒绝继续"))?;
    fs::copy(&store, dir.join("bcd-before"))?;
    fs::write(dir.join("bootmgr-before.txt"), &before)?;
    // 与 WinRE 执行链对齐的第二个名字：进度窗「BCD 回退」按钮（与 B 快捷键）
    // 复用 windows_prepare::rollback_bcd_snapshot，它认的是 bcd-before-raw
    // （与离线 prepare 链同一命名）。同一字节写两份，磁盘代价约百 KB。
    fs::copy(&store, dir.join("bcd-before-raw"))?;
    // 【2026-10-18 补线】装载 BCD 回退闭包。
    // 在此之前 set_bcd_rollback 全仓没有调用方：进度窗的「BCD 回退(B)」按钮
    // 从加上那天起每次点击都只能记一行「尚不可用」——功能等于不存在。
    // 快照已落盘、ESP 身份在手，此刻装载可覆盖后续全部引导改写
    // （bcdboot 预建项 / 菜单命名 / default 归属），用户在恢复环境里改坏
    // 引导后能一键退回准备期状态，不必再从宿主侧挂 PE 光盘手改。
    {
        let rollback_dir = dir.to_path_buf();
        let rollback_esp = esp.clone();
        let rollback_log = log_path.to_path_buf();
        progress.set_bcd_rollback(Arc::new(move || {
            crate::windows_prepare::rollback_bcd_snapshot(
                &rollback_dir,
                &rollback_esp,
                &rollback_log,
            )
            .map_err(|error| error.to_string())
        }));
    }

    // 格式化选项（GUI「格式化目标分区」复选框，默认勾选）：取消勾选=跳过格式化
    // 与格式化后校验，直接覆盖应用（盘符重绑/bcdboot/菜单命名照常——它们要求
    // 的是「应用后的 Windows」，与是否先清空无关）。BCD 快照在上面照做，
    // 回滚能力不受影响。
    let after = if params.format_target {
        let script = crate::diskpart_format_script(target.disk_number, target.partition_number)?;
        let script_path = dir.join("format.txt");
        fs::write(&script_path, script)?;
        run_capture(
            "diskpart.exe",
            &["/s".into(), script_path.to_string_lossy().into_owned()],
            &cmd_out_file(dir),
            true,
        )?;
        let formatted = crate::windows_prepare::refresh_identity(&target)?;
        backuprestore_core::operation_safety::verify_formatted_volume(&target, &formatted)?;
        formatted
    } else {
        crate::windows_prepare::refresh_identity(&target)?
    };
    let target_root = PathBuf::from(
        after
            .volume_path()
            .ok_or_else(|| crate::err("目标卷缺少稳定 GUID 路径"))?,
    );
    fs::read_dir(&target_root)?;

    log(
        log_path,
        if overlay_no_acl {
            "STEP 3/4 应用镜像（wimlib --no-acls 覆盖式通道）并校验系统文件"
        } else {
            "STEP 3/4 应用镜像（DISM /Apply-Image）并校验系统文件"
        },
    )?;
    if overlay_no_acl {
        // 任务 #110：在线态命中「已含 Windows + 取消格式化」→ wimlib --no-acls。
        // 在线态 current_exe 在程序同目录（包目录 / H:\brwork），wimlib 二进制
        // 由 build-win.sh --deploy 随包部署到那里。
        let executable_dir = std::env::current_exe()
            .map_err(|error| {
                crate::err(&format!("无法定位程序目录（wimlib 通道）：{error}"))
            })?
            .parent()
            .ok_or_else(|| crate::err("程序目录解析失败（wimlib 通道）"))?
            .to_path_buf();
        crate::wimlib_apply::apply_no_acl(
            &executable_dir,
            image,
            index,
            &target_root,
            log_path,
        )?;
    } else {
        dism(
            &[
                "/English".into(),
                "/Apply-Image".into(),
                format!("/ImageFile:{}", image.display()),
                format!("/Index:{index}"),
                format!("/ApplyDir:{}", target_root.display()),
                "/CheckIntegrity".into(),
                "/Verify".into(),
            ],
            log_path,
        )?;
    }
    for components in [
        vec!["Windows", "System32", "config", "SYSTEM"],
        vec!["Windows", "System32", "winload.efi"],
        vec!["Windows", "System32", "ntoskrnl.exe"],
    ] {
        if !crate::pe_safety::probe_file(&target_root, &components)? {
            return Err(crate::err("系统文件验证失败，不能降级为数据还原成功"));
        }
    }

    log(
        log_path,
        "STEP 3.5/4 绑定第二系统盘符（离线 SYSTEM/SOFTWARE，主系统不受影响）",
    )?;
    let target_letter = crate::windows_prepare::ensure_volume_mounted(&after, 'T', log_path)?;
    crate::pe_safety::verify_binding(
        &after,
        &crate::windows_prepare::volume_identity(target_letter)?,
        true,
    )?;
    // 镜像里的 MountedDevices 仍把原系统盘符绑在主系统分区上。不重绑则新系统启动后
    // 注册表与服务里的绝对路径全部指向别的卷，服务初始化失败（LogonUI/dwm 崩溃循环）。
    // 离线路径在 bcdboot 前做同一件事（main.rs 的 CreateSecondary 分支），此处对齐。
    crate::secondary_registry::repair(&target_root, target_letter, &after, log_path)?;

    // 用户要求：还原进行中就要能在真实启动菜单里看到这一项（占位），
    // 最后 STEP 4 再按用户在弹窗里的选择补成正式项。
    // 时机选在 DISM 应用完成 + 盘符绑定之后：此刻目标卷已是完整 Windows
    // （winload.efi / SYSTEM hive 都在），用 bcdboot 建的 loader 属性完整、
    // 真的能启动，不是坏项。
    // 若不提前建，用户等待几十分钟期间重启，菜单里看不到正在还原的系统。
    //
    // 2026-10-13 更正：原注释写「/addlast 只追加到菜单末尾，不动默认启动项」
    // 是**错的**——/addlast 管的是固件条目位置，与 default 无关；bcdboot 默认
    // 就会把新 loader 设成 {bootmgr}.default。
    // 2026-10-15 用户规则修订：**不再加 /d**。此前加 /d 是为了让 default 留在
    // 原主系统；现在要求还原完成后 default 就指向新还原出来的系统，bcdboot 的
    // 默认行为正好是我们要的。旧顾虑"中途重启会进半成品系统"在此不成立——
    // 这一步在 DISM 应用完成之后才跑，目标卷此刻已是完整 Windows。
    log(
        log_path,
        "STEP 3.6/4 预建还原系统启动项（等待期间即可在启动菜单看到并选择）",
    )?;
    run_capture(
        "bcdboot.exe",
        &[
            format!("{target_letter}:\\Windows"),
            "/s".into(),
            format!("{esp_letter}:"),
            "/f".into(),
            "UEFI".into(),
            "/addlast".into(),
        ],
        &cmd_out_file(dir),
        true,
    )?;
    // 立刻把休眠引用恢复成快照值：bcdboot 会把 {bootmgr}.resumeobject 改写成新
    // loader 的恢复对象，若用户此刻重启，休眠恢复会指向错误系统。
    // **default 不再改回**：2026-10-15 用户规则要求它指向本次还原的系统，
    // bcdboot 不带 /d 后正是如此；最终值仍由 STEP 4 的 boot_menu::apply 写定。
    // 菜单顺序也留给 STEP 4 按用户选择写入。
    if let Some(resume) = old.resume_object.clone() {
        run_bcd_edit(
            &[
                "/store".into(),
                store.clone(),
                "/set".into(),
                "{bootmgr}".into(),
                "resumeobject".into(),
                resume,
            ],
            dir,
        )?;
    } else {
        // 快照里本来就没有 resumeobject（未开休眠/快速启动，或 ESP 是本工具新建的）。
        // bcdboot 刚刚给 {bootmgr} 写了一个指向新系统的值，不删掉的话：
        // STEP 4 的预检与终检都会判定 resume_object 变了而报失败，而且休眠恢复
        // 会指向错误的系统。删除失败是良性的（本来就没有这个值）。
        let _ = run_bcd_edit(
            &[
                "/store".into(),
                store.clone(),
                "/deletevalue".into(),
                "{bootmgr}".into(),
                "resumeobject".into(),
            ],
            dir,
        );
    }

    log(
        log_path,
        "STEP 4/4 修复引导（菜单命名/顺序/默认项，默认启动项=本次还原的系统）",
    )?;
    let live = run_bcd_edit(
        &[
            "/store".into(),
            store.clone(),
            "/enum".into(),
            "{bootmgr}".into(),
            "/v".into(),
        ],
        dir,
    )?;
    if crate::parse_boot_manager_state(&live).as_ref() != Some(&old) {
        // STEP 3.6 已有意把新 loader 追加进 displayorder，且按 2026-10-15 用户
        // 规则 default 已改指新系统，所以这里不能再要求与快照完全一致；真正要
        // 守住的是「休眠引用未被改写」——它指错会让休眠恢复走进错误的系统。
        let parsed = crate::parse_boot_manager_state(&live);
        let unchanged = parsed
            .as_ref()
            .is_some_and(|now| now.resume_object == old.resume_object);
        if !unchanged {
            return Err(crate::err("预检后引导状态发生变化，停止引导写入"));
        }
    }
    // 不再无条件 bcdboot：STEP 3.6 已经建好 loader 并追加进菜单。此处再跑一次
    // 会为同一分区建出第二个 loader，而 boot_menu::apply 对「同卷多个加载器」
    // 一律拒绝猜测（按枚举顺序猜会把用户引导到错误系统）。apply 内部只在
    // 确实找不到 loader 时才兜底 bcdboot。
    let all = run_bcd_edit(
        &[
            "/store".into(),
            store.clone(),
            "/enum".into(),
            "all".into(),
            "/v".into(),
        ],
        dir,
    )?;
    // 用 boot_menu 的解析器而不是 pe_safety::target_loader：后者只认英文
    // device/osdevice/path，在中文 Windows 上会直接判定"目标引导加载器不唯一或
    // 设备字段不完整"而失败，连 apply 都走不到。
    let loader = crate::boot_menu::loader_identifier_for_partition(&all, target_letter)?
        .ok_or_else(|| crate::err("bcdboot 后找不到第二系统的启动加载器"))?;
    // 上面的 bcdboot 已把第二系统 loader 建好；引导方案此刻冻结，由 boot_menu::apply
    // 统一写入菜单名称、顺序、默认项、休眠引用与显示时间，并逐项回读校验。
    // 原先这里手写一套 description/displayorder/default/resumeobject，与 apply 职责
    // 完全重叠且不给用户选择权，故整体替换（原四项断言保留在下方作最终验收）。
    let boot_plan = boot_menu_session.freeze()?;
    crate::boot_menu::apply(
        &boot_plan,
        &PathBuf::from(format!("{esp_letter}:\\")),
        log_path,
        None,
    )?;
    let current = run_bcd_edit(
        &[
            "/store".into(),
            store.clone(),
            "/enum".into(),
            "{bootmgr}".into(),
            "/v".into(),
        ],
        dir,
    )?;
    let cur = match crate::parse_boot_manager_state(&current) {
        Some(c) => c,
        None => return Err(crate::err("无法解析还原后的引导管理器状态，拒绝报成功")),
    };
    // 验收（2026-10-15 用户规则）：默认启动项必须指向**本次还原出来的系统**
    // （目标卷分区），而不是仍在运行的原主系统；bootmgr 的 resume_object 恢复为
    // 快照值；原有 loader 的恢复对象映射不得被破坏；原菜单里已有的项必须一个
    // 不少地保留（引导修复只允许追加/改名/改序，不允许弄丢既有入口）。
    //
    // **不能直接比对 GUID 文本**：bcdedit 会把默认项显示成 `{current}`、工具项显示成
    // `{memdiag}` 这类别名。2026-10-12 实机还原就在这里误报——默认项其实没变
    // （{current} 展开就是原主系统 loader，device 都是 partition=C:），
    // 但字符串比对认为 {12e3701c...} != {current}，于是把一次成功的还原判成失败。
    // 默认项改用「指向哪个分区」判断：本次还原的系统在目标卷。
    let target_is_default = match cur.default.to_ascii_lowercase().as_str() {
        // 别名 {current} 就是"当前正在运行的 Windows"——在线路径下它就是原主系统
        // （活动系统盘作为在线目标在入口处已被拒），绝不可能是本次还原的目标。
        "{current}" | "{default}" => false,
        identifier => {
            let object = run_bcd_edit(
                &[
                    "/store".into(),
                    store.clone(),
                    "/enum".into(),
                    identifier.to_string(),
                    "/v".into(),
                ],
                dir,
            )?;
            let device = object
                .lines()
                .filter_map(|line| line.split_once(char::is_whitespace))
                .find(|(key, _)| key.trim().eq_ignore_ascii_case("device"))
                .map(|(_, value)| value.trim().to_ascii_lowercase())
                .unwrap_or_default();
            // device 已转小写，target_letter 也必须转小写后再比，
            // 否则 "partition=f:".starts_with("partition=F:") 永远为 false。
            device.starts_with(&format!(
                "partition={}:",
                target_letter.to_ascii_lowercase()
            ))
        }
    };
    if !target_is_default {
        return Err(crate::err(
            "默认启动项未指向本次还原的系统，拒绝报成功",
        ));
    }
    // 原有菜单项一个都不能少。同样不能直接比 GUID 文本：bcdedit 可能把其中某项
    // 显示成 {current}/{memdiag}/{default} 之类的别名。把两边的常见别名都展开成
    // 真实 GUID 再比（{current}/{default} 展开为默认项）。
    //
    // **例外：目标卷原来的 loader 允许被本次新建的取代。** 还原到 E: 时，
    // bcdboot 会把 E: 的旧 loader 换成指向新镜像的新 loader——这正是"还原"该有的
    // 行为。2026-10-12 实机在这里误报：旧的 {556b9391}（E:）被新的 {2f72105a}（E:）
    // 取代，断言认为"原有菜单项丢失"，可那恰恰是成功的还原。
    // 所以只要求**非目标卷**的原有项一个不少。
    // 把别名解析成真实 GUID。
    //
    // **不能图省事用 `cur.default` 顶替**：`{bootmgr}` 的 `default` 字段自身就可能是
    // `{default}` 这个自引用别名（2026-10-12 实机正是如此），那样替换完还是别名，
    // 与快照里的完整 GUID 永远不相等，于是一次成功的还原被误判成"菜单项丢失"。
    // 正解是问 bcdedit 要这个别名的真实标识符。
    let resolve_alias = |alias: &str| -> Option<String> {
        let text = run_bcd_edit(
            &[
                "/store".into(),
                store.clone(),
                "/enum".into(),
                alias.to_string(),
                "/v".into(),
            ],
            dir,
        )
        .ok()?;
        text.lines().find_map(|line| {
            let trimmed = line.trim();
            let (key, value) = trimmed.split_once(char::is_whitespace)?;
            (key.eq_ignore_ascii_case("identifier") || key == "标识符")
                .then(|| crate::bcd_identifier_from_line(value.trim()).map(str::to_owned))
                .flatten()
        })
    };
    let expand = |ids: &[String]| -> Vec<String> {
        ids.iter()
            .map(|id| match id.to_ascii_lowercase().as_str() {
                "{current}" | "{default}" => resolve_alias(id).unwrap_or_else(|| id.clone()),
                _ => id.clone(),
            })
            .collect()
    };
    let expected = expand(&old.display_order);
    let actual = expand(&cur.display_order);
    // BCDEdit 可能把显示顺序中的合法条目写成同一行的多个值；状态解析器
    // 已按字段边界收集续行。这里统一大小写，避免 GUID 大小写差异造成误报。
    let expected: Vec<String> = expected
        .into_iter()
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let actual: Vec<String> = actual
        .into_iter()
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let missing: Vec<String> = expected
        .iter()
        .filter(|id| !actual.iter().any(|cur_id| cur_id == *id))
        .cloned()
        .collect();
    // 丢掉的项如果本来就指向目标卷（被新 loader 取代），属正常；
    // 丢的是别的卷（例如主系统）才是真问题。
    //
    // 2026-10-13 实机根因（三轮同错）：连续第二次还原时，上一轮建的 E: 旧 loader
    // （{cdf9efe4}）会被本轮的新 loader（{d146fb01}）取代并**删除**——这正是还原
    // 该有的行为。可旧代码把"bcdedit 查不到该项"（对象已删）直接当成"丢失外来项"
    // 报错，注释写的意图恰好相反。正解：对象被删=被取代（apply 只删目标卷的旧
    // loader，新 loader 在菜单里另有断言）；对象仍存在时看 device 指向，指向目标卷
    // 之外的卷才算真丢失。
    // 门禁判定全程落日志：连续四轮同错后不再接受"黑盒判定"——缺失清单、
    // 每个对象的枚举结果、device 指向与最终判定值都必须可事后核对。
    log(
        log_path,
        &format!(
            "门禁对账：快照菜单 {:?}；现菜单 {:?}；缺失 {:?}",
            expected, actual, missing
        ),
    )?;
    let lost_foreign = missing.iter().any(|id| {
        let object = run_bcd_edit(
            &[
                "/store".into(),
                store.clone(),
                "/enum".into(),
                (*id).to_string(),
                "/v".into(),
            ],
            dir,
        );
        match object {
            // 进程失败（bcdedit 未跑起来）：与对象被删同样按被取代处理，
            // 不让探测通道的偶发故障拦下一次成功的还原。
            Err(_) => {
                let _ = log(log_path, &format!("门禁对象 {id} 枚举失败（按被取代处理）"));
                false
            }
            Ok(text) => {
                let verdict = missing_entry_is_foreign(&text, target_letter);
                if object_exists_in_enum(&text) {
                    let device = text
                        .lines()
                        .filter_map(|line| line.split_once(char::is_whitespace))
                        .find(|(key, _)| {
                            let key = key.trim();
                            key.eq_ignore_ascii_case("device") || key == "设备"
                        })
                        .map(|(_, value)| value.trim().to_string())
                        .unwrap_or_default();
                    let _ = log(
                        log_path,
                        &format!("门禁对象 {id} 仍存在 device={device:?} 判定丢失={verdict}"),
                    );
                } else {
                    // 退出码 0 + 空文案（"没有匹配的对象"）= 对象已被删。
                    let _ = log(
                        log_path,
                        &format!("门禁对象 {id} 已被删（枚举无标识符，被取代），不算丢失"),
                    );
                }
                verdict
            }
        }
    });
    if lost_foreign {
        return Err(crate::err("原有启动菜单项丢失，拒绝报成功"));
    }
    if !cur.display_order.iter().any(|id| id == &loader) {
        return Err(crate::err("第二系统启动项未进入菜单，拒绝报成功"));
    }
    if cur.resume_object != old.resume_object {
        return Err(crate::err(
            "bootmgr resume_object 未恢复为主系统，拒绝报成功",
        ));
    }
    for (l, r) in &old.loader_resume_objects {
        if !cur
            .loader_resume_objects
            .iter()
            .any(|(cl, cr)| cl == l && cr == r)
        {
            return Err(crate::err("原有系统恢复对象被破坏，拒绝报成功"));
        }
    }
    // 注：在线（非 PE）入口不调用 winre_registration::repair——该修复拒绝非 PE 环境，
    // 目标系统的 WinRE 注册应在其自身首次启动后用 reagentc 完成（W09 收敛项）。
    log(
        log_path,
        "引导与启动菜单已写入；系统实际可启动性需另行启动验收",
    )
}

/// 判定一个从菜单里消失的既有项是否属于"外来卷被误删"。
///
/// 输入是该对象当前的 bcdedit /enum 输出与目标盘符。调用方已保证对象仍存在
/// （被删的情形由调用方按"被取代"处理，不走这里）。
/// - device 指向目标卷：旧 loader 被新 loader 取代，正常，返回 false。
/// - device 指向其它任何卷（含主系统活动卷、ramdisk PE 项）或读不到 device：
///   视为真丢失，返回 true。
/// 中文系统里 device 字段名显示为"设备"，必须与英文一同识别
/// （2026-10-12 已在 boot_menu 踩过同样的本地化坑）。
/// 枚举输出里是否存在 BCD 标识符（{guid}）。
///
/// bcdedit 查一个**已删除**的对象时退出码为 0（2026-10-13 实测），只输出
/// 本地化文案（中文"没有匹配的对象或存储为空"、英文"The element data store
/// is not present"等）。文案不可枚举，结构性判据是唯一稳的：全文没有任何
/// BCD 标识符即视为对象不存在。判定与日志措辞共用这一判据，避免再出现
/// "对象已被删却打印'仍存在'"的歧义（2.4.1 验收遗留小项）。
#[cfg(any(windows, test))]
fn object_exists_in_enum(object_text: &str) -> bool {
    object_text
        .lines()
        .any(|line| crate::bcd_identifier_from_line(line).is_some())
}

#[cfg(any(windows, test))]
fn missing_entry_is_foreign(object_text: &str, target_letter: char) -> bool {
    // 对象不存在 = 被新 loader 取代，不算丢失（判据见 object_exists_in_enum）。
    if !object_exists_in_enum(object_text) {
        return false;
    }
    let device = object_text
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find(|(key, _)| {
            let key = key.trim();
            key.eq_ignore_ascii_case("device") || key == "设备"
        })
        .map(|(_, value)| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    // 对象存在但读不到 device：按真丢失处理（保守，交人工排查）。
    if device.is_empty() {
        return true;
    }
    !device.starts_with(&format!(
        "partition={}:",
        target_letter.to_ascii_lowercase()
    ))
}

/// 单调序号，保证每次外部命令的输出文件互不覆盖，便于事后排查。
static CMD_SEQ: AtomicU64 = AtomicU64::new(0);
fn cmd_out_file(dir: &Path) -> PathBuf {
    let n = CMD_SEQ.fetch_add(1, Ordering::SeqCst);
    dir.join(format!("cmd-{n}.out"))
}

/// 执行外部程序并读取其解码后的输出；失败（非零退出）时返回带输出的错误。
fn run_capture(
    program: &str,
    args: &[String],
    out: &Path,
    long: bool,
) -> Result<String, TaskError> {
    let outcome = crate::windows_command::run_program(
        program,
        args,
        Some(out),
        if long {
            // DISM 用可配置上限（默认 INFINITE）；其余长命令保持无限等待。
            if program.to_ascii_lowercase().ends_with("dism.exe") {
                crate::dism_timeout::wait_ms()
            } else {
                INFINITE
            }
        } else {
            60_000
        },
    );
    // run_program 用作业对象约束子进程树，返回时子进程的 stdout 句柄未必已经释放，
    // 立刻 read 会撞上"另一个程序正在使用此文件"。DISM 尤其明显：它退出后仍可能
    // 有 worker 在收尾。2026-10-12 实机还原在 STEP 1 之后的第一次 DISM 就撞上，
    // 间隔只有 50 毫秒。短暂重试是 Windows 上处理这种竞态的标准做法。
    let mut attempt = 0;
    let raw = loop {
        attempt += 1;
        match fs::read(out) {
            Ok(raw) => break raw,
            Err(error) if attempt < 20 => {
                let _ = error;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(error) => return Err(error.into()),
        }
    };
    let text = crate::pe_operation::decode_output(&raw)?;
    if !outcome.is_success() {
        return Err(crate::err(&format!("{program} 失败：{outcome}\n{text}")));
    }
    Ok(text)
}

/// 便捷封装：直接执行 bcdedit 并取回其文本输出。
fn run_bcd_edit(args: &[String], dir: &Path) -> Result<String, TaskError> {
    run_capture("bcdedit.exe", args, &cmd_out_file(dir), false)
}

/// 候选目录保留到事务完全成功；失败留下日志指定的副本，原镜像不受追加/清理失败影响。
pub(crate) fn capture(
    params: &OnlineOpParams,
    source: &Path,
    identity: VolumeIdentity,
    used: u64,
    log_path: &Path,
) -> Result<(), TaskError> {
    let destination = Path::new(&params.image_path);
    let parent = destination
        .parent()
        .ok_or_else(|| crate::err("missing image parent"))?;
    let workspace = parent.join(format!(
        ".br-online-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::create_dir(&workspace)?;
    log(
        log_path,
        &format!(
            "候选目录：{}；失败时保留，原文件不覆盖",
            workspace.display()
        ),
    )?;
    let candidate = workspace.join("candidate.wim");
    let existing = match fs::metadata(destination) {
        Ok(m) if m.is_file() => true,
        Ok(_) => return Err(crate::err("backup destination is not a regular file")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    let before = if existing {
        indexes(destination, log_path)?
    } else {
        vec![]
    };
    let mut metadata = im::load_sidecars(destination, &before)?;
    let original_hash = if existing {
        Some(backuprestore_core::sha256_file(destination)?)
    } else {
        None
    };
    // 目录级旧副档只有哈希匹配时才属于本镜像；绝不覆盖同目录另一个 WIM 的记录。
    let legacy_path = im::legacy_metadata_path(destination)?;
    let legacy: Option<BackupMetadata> = match backuprestore_core::read_json(&legacy_path) {
        Ok(value) => Some(value),
        Err(TaskError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            log(
                log_path,
                &format!("[WARN] 旧版目录级副档无法读取，不作为本次依据：{error}"),
            )?;
            None
        }
    };
    let own_legacy = legacy.as_ref().is_some_and(|m| {
        original_hash
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case(&m.image_sha256))
            && before.contains(&m.wim_index)
    });
    if own_legacy {
        let value = legacy.as_ref().unwrap();
        metadata
            .entry(value.wim_index)
            .or_insert_with(|| value.clone());
    }
    for &index in &before {
        if !metadata.contains_key(&index) {
            log(
                log_path,
                &format!("[WARN] 旧索引 {index} 无副档，保留未知来源状态，不伪造源卷信息"),
            )?;
        }
    }
    // 备份原副档；发布过程中若写入失败，使用这些文件与 previous.wim 回滚。
    let mut saved_sidecars = Vec::new();
    for index in 1..=before.len() as u32 + 1 {
        let path = im::index_metadata_path(destination, index)?;
        match fs::read(&path) {
            Ok(bytes) => {
                fs::write(workspace.join(format!("original-{index}.json")), &bytes)?;
                saved_sidecars.push((path, Some(bytes)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => saved_sidecars.push((path, None)),
            Err(e) => return Err(e.into()),
        }
    }
    if own_legacy {
        saved_sidecars.push((legacy_path.clone(), Some(fs::read(&legacy_path)?)));
    }
    if existing {
        fs::copy(destination, &candidate)?;
    }
    let config = backuprestore_core::write_capture_exclusion_config(source)?;
    let started = Utc::now();
    let name = if params.image_name.is_empty() {
        "Windows Backup"
    } else {
        &params.image_name
    };
    let mut args = vec![
        "/English".into(),
        if existing {
            "/Append-Image".into()
        } else {
            "/Capture-Image".into()
        },
        format!("/ImageFile:{}", candidate.display()),
        format!("/CaptureDir:{}", source.display()),
        format!("/Name:{name}"),
        "/CheckIntegrity".into(),
        format!("/ConfigFile:{}", config.display()),
    ];
    if !existing {
        args.push(format!(
            "/Compress:{}",
            backuprestore_core::canonical_compression(&params.compress)?
        ));
    }
    log(log_path, "STEP 2/4 捕获镜像（候选副本，无固定超时）")?;
    dism(&args, log_path)?;
    let after = indexes(&candidate, log_path)?;
    let new_index = im::verify_appended_index(&before, &after)?;
    let duration = (Utc::now() - started).num_seconds().max(1) as u64;
    let size = fs::metadata(&candidate)?.len();
    let source_size = identity.partition_size;
    let serial = identity.volume_serial.clone();
    metadata.insert(
        new_index,
        BackupMetadata {
            version: 3,
            image_type: "wim".into(),
            created: Utc::now(),
            started: Some(started),
            duration_secs: Some(duration),
            bytes_per_sec: Some(size / duration),
            computer: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into()),
            windows_edition: "unknown".into(),
            architecture: crate::native_windows_architecture(),
            windows_build: "unknown".into(),
            wim_index: new_index,
            image_sha256: String::new(),
            image_size: size,
            source: identity,
            captured_used_bytes: used,
            reserved_bytes: 0,
            minimum_target_size: source_size,
            volume_serial: serial,
            program_version: backuprestore_core::PROGRAM_VERSION.into(),
            // 记录 DISM /Name 的有效索引名：sidecar 快速路径直接用它呈现，
            // 不再合成 "Windows Backup (index N)" 占位名冒充镜像真名。
            image_name: Some(name.to_string()),
        },
    );
    log(log_path, "STEP 3/4 保留策略、索引回读和最终哈希校验")?;
    let keep = params
        .keep_indexes
        .unwrap_or(new_index)
        .max(1)
        .min(new_index);
    let removed = new_index - keep;
    for remaining in (keep + 1..=new_index).rev() {
        dism(
            &[
                "/English".into(),
                "/Delete-Image".into(),
                format!("/ImageFile:{}", candidate.display()),
                "/Index:1".into(),
            ],
            log_path,
        )?;
        let actual = indexes(&candidate, log_path)?;
        if actual != (1..remaining).collect::<Vec<_>>() {
            return Err(crate::err(
                "retention index read-back mismatch; original image not replaced",
            ));
        }
    }
    let metadata = im::remap_sidecars(&metadata, removed);
    let hash = backuprestore_core::sha256_file(&candidate)?;
    let final_size = fs::metadata(&candidate)?.len();
    im::write_sidecars(&candidate, &metadata, &hash, final_size, 0)?;
    log(
        log_path,
        &format!("校验完成：sha256={hash} bytes={final_size} 保留={keep} 删除={removed}"),
    )?;
    // 防止另一入口在候选构建期间修改了原镜像；发布前必须重验。
    if let Some(expected) = original_hash {
        if backuprestore_core::sha256_file(destination)? != expected {
            return Err(crate::err(
                "original WIM changed concurrently; candidate retained, publication refused",
            ));
        }
    } else if destination.try_exists()? {
        return Err(crate::err(
            "destination appeared concurrently; publication refused",
        ));
    }
    log(
        log_path,
        "STEP 4/4 发布镜像并回读副档；旧文件在事务成功前保留",
    )?;
    let previous = workspace.join("previous.wim");
    if existing {
        fs::rename(destination, &previous)?;
    }
    let publish = (|| -> Result<(), TaskError> {
        fs::rename(&candidate, destination)?;
        im::write_sidecars(destination, &metadata, &hash, final_size, new_index)?;
        if own_legacy {
            if let Some(first) = metadata.get(&1) {
                let mut first = first.clone();
                first.image_sha256 = hash.clone();
                first.image_size = final_size;
                backuprestore_core::write_json_atomic(&legacy_path, &first)?;
                if backuprestore_core::read_json::<BackupMetadata>(&legacy_path)? != first {
                    return Err(crate::err("legacy metadata read-back mismatch"));
                }
            } else {
                fs::remove_file(&legacy_path)?;
            }
        }
        if fs::metadata(destination)?.len() != final_size {
            return Err(crate::err("published WIM size mismatch"));
        }
        Ok(())
    })();
    if let Err(error) = publish {
        // 即使日志盘已满，也必须尝试恢复；不能让记录错误短路回滚。
        let rollback_log_error = log(
            log_path,
            &format!("[ERROR] 发布失败：{error}；开始恢复原镜像和副档"),
        )
        .err();
        let rollback = (|| -> Result<(), TaskError> {
            if destination.try_exists()? {
                fs::rename(destination, workspace.join("failed-publication.wim"))?;
            }
            if existing {
                fs::rename(&previous, destination)?;
            }
            for (path, bytes) in saved_sidecars {
                if let Some(bytes) = bytes {
                    fs::write(&path, &bytes)?;
                    if fs::read(&path)? != bytes {
                        return Err(crate::err("sidecar rollback read-back mismatch"));
                    }
                } else {
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            Ok(())
        })();
        return Err(crate::err(&format!(
            "publication failed: {error}; rollback={rollback:?}; log_error={rollback_log_error:?}; artifacts={}",
            workspace.display()
        )));
    }
    // 只清理本事务创建的目录；不触及用户的其他文件、旧镜像或工具链。
    fs::remove_dir_all(&workspace)?;
    log(log_path, "镜像及副档已发布并回读；候选目录已清理")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_name_parses_real_dism_text_output() {
        // 夹具是 2026-10-12 实机 `dism /Get-WimInfo /WimFile:H:\c-backup\c-system.wim /Index:1`
        // 的真实输出（副本，只留相关行）。副档在这台机器上是 "unknown"，
        // 所以这条后备路是实际生效的那一条。
        let output = "\
Deployment Image Servicing and Management tool
Version: 10.0.26100.9549

Details for image : H:\\c-backup\\c-system.wim

Index : 1
Name : Windows Backup
Description : <undefined>
Size : 92,995,766,369 bytes
WIM Bootable : No
Architecture : arm64
Hal : COMPUTER\\Generic
Version : 10.0.26200
ServicePack Build : 9550
ServicePack Level : 0
Edition : Professional

The operation completed successfully.";
        assert_eq!(
            read_image_version_name(Path::new("H:\\c-backup\\c-system.wim"), 1, output).as_deref(),
            Some("Professional 10.0.26200"),
            "必须用映像内部的 Edition + Version，不能是副档里的 unknown"
        );
    }

    #[test]
    fn version_name_requires_the_index_qualified_output() {
        // 不带 /Index:N 时 DISM 只输出摘要，没有 Edition/Version 行。
        // 产品代码必须带 /Index: 调用，否则版本名白丢。实机 2026-10-12 正是如此：
        // 夹具带 /Index: 所以单测通过，而代码跑的是不带 /Index: 的命令，
        // 于是 versioned_name = None。
        let summary = "\
Deployment Image Servicing and Management tool
Version: 10.0.26100.9549

Details for image : H:\\c-backup\\c-system.wim

Index : 1
Name : Windows Backup
Description : <undefined>
Size : 92,995,766,369 bytes

The operation completed successfully.";
        assert_eq!(
            read_image_version_name(Path::new("x.wim"), 1, summary),
            None,
            "摘要形态里没有版本信息，必须返回 None 让调用方降级"
        );
    }

    #[test]
    fn version_name_ignores_dism_own_version_line() {
        // 输出最前面的 "Version: 10.0.26100.9549" 是 dism.exe 程序自己的版本，
        // 不是镜像版本。误拿它会让菜单名写成 dism 的版本号。
        let output = "\
Deployment Image Servicing and Management tool
Version: 10.0.26100.9549

Details for image : H:\\c-backup\\c-system.wim

Index : 1
Edition : Professional
Version : 10.0.26200";
        assert_eq!(
            read_image_version_name(Path::new("x.wim"), 1, output).as_deref(),
            Some("Professional 10.0.26200"),
            "必须取 Details for image 之后的镜像版本"
        );
    }

    #[test]
    fn version_name_falls_back_to_json_shape() {
        // DISM 也可能输出 JSON（本项目 parse_dism_images 消费的形态）。
        let output = r#"{"images":[{"ImageIndex":1,"ImageName":"Windows Backup","Architecture":"arm64","EditionId":"Professional","ImageVersion":"10.0.26200"}]}"#;
        assert_eq!(
            read_image_version_name(Path::new("x.wim"), 1, output).as_deref(),
            Some("Professional 10.0.26200")
        );
    }

    #[test]
    fn version_name_rejects_placeholders() {
        // 副档/映像里的占位值必须被拒，否则菜单名会变成字面的 "unknown unknown"。
        for output in [
            "Index : 1\nEdition : unknown\nVersion : unknown",
            "Index : 1\nEdition : <undefined>\nVersion : <undefined>",
            "Index : 1\n",
            "",
        ] {
            assert_eq!(
                read_image_version_name(Path::new("x.wim"), 1, output),
                None,
                "占位值或缺失字段必须返回 None，让调用方继续降级"
            );
        }
    }

    #[test]
    fn deleted_old_target_loader_is_replaced_not_lost() {
        // 2026-10-13 实机（连续第二轮还原）：上一轮建的 E: 旧 loader 被本轮新
        // loader 取代并删除。关键实测：bcdedit /enum {已删对象} **退出码为 0**，
        // 只输出本地化文案"没有匹配的对象或存储为空。"——Err 分支根本不会触发，
        // 空文案会走 Ok 分支。必须靠"全文无 BCD 标识符"识别对象不存在。
        for text in [
            "没有匹配的对象或存储为空。\r\n",
            "The element data store is not present.\r\n",
        ] {
            assert!(
                !missing_entry_is_foreign(text, 'E'),
                "对象不存在（退出码 0 + 空文案）必须按被取代处理：{text:?}"
            );
        }
    }

    #[test]
    fn enum_error_also_counts_as_replaced() {
        // 进程失败（Err）分支同样按被取代处理——调用方 Err(_) => false。
        // 这里只确认正常对象文本不会误进该路径。
        let object = "标识符                  {8b55181a-c666-11f1-895d-d2749b7d5c85}\r\ndevice                  partition=E:";
        assert!(!missing_entry_is_foreign(object, 'E'));
    }

    #[test]
    fn missing_entry_pointing_at_target_volume_is_replaced() {
        // 仍存在且指向目标卷 E: 的缺项=被新 loader 取代，正常。
        let object = "Windows 启动加载器
-------------------
标识符                  {cdf9efe4-c657-11f1-895d-d2749b7d5c85}
device                  partition=E:
path                    \\Windows\\system32\\winload.efi
description             Professional 10.0.26200
osdevice                partition=E:";
        assert!(!missing_entry_is_foreign(object, 'E'));
    }

    #[test]
    fn missing_entry_pointing_at_primary_is_really_lost() {
        // 指向主系统 C: 的项从菜单消失=真问题，必须拦下。
        let object = "Windows 启动加载器
-------------------
标识符                  {12e3701c-c361-11f1-90a2-c222b5348ef7}
device                  partition=C:
path                    \\Windows\\system32\\winload.efi";
        assert!(missing_entry_is_foreign(object, 'E'));
    }

    #[test]
    fn missing_entry_accepts_chinese_device_label() {
        // 中文系统 device 显示为"设备"，大小写与本地化都不能漏判。
        let object = "Windows 启动加载器
-------------------
标识符                  {d0a5dda3-c644-11f1-895d-d2749b7d5c85}
设备                    partition=E:
路径                    \\Windows\\system32\\winload.efi";
        assert!(!missing_entry_is_foreign(object, 'E'));
        // 同样是中文标签但指向别的卷，仍算丢失。
        let foreign = object.replace("partition=E:", "partition=F:");
        assert!(missing_entry_is_foreign(&foreign, 'E'));
    }

    #[test]
    fn missing_entry_without_device_line_is_lost() {
        // 读不到 device 的对象按保守处理：算丢失，交人工排查。
        let object = "Windows 启动加载器
-------------------
标识符                  {deadbeef-c644-11f1-895d-d2749b7d5c85}
description             something";
        assert!(missing_entry_is_foreign(object, 'E'));
    }

    #[test]
    fn self_referential_default_alias_must_not_expand_to_itself() {
        // `{bootmgr}` 的 default 字段自身就可能是 `{default}` 这个自引用别名
        // （2026-10-12 实机正是如此）。若展开逻辑写成
        // `"{default}" => cur.default.clone()`，替换完还是 `{default}`，
        // 与快照里的完整 GUID 永远不相等，会把成功的还原误判成"菜单项丢失"。
        // 正解是问 bcdedit 要该别名的真实标识符（实测展开为 {12e3701c-...}）。
        let snapshot = "{12e3701c-c361-11f1-90a2-c222b5348ef7}".to_string();
        let after = "{default}".to_string();
        // 错误的展开：自指，替换后毫无变化。
        let broken = after.clone();
        assert_ne!(
            broken, snapshot,
            "若展开结果仍等于别名本身，就永远匹配不上快照 —— 这就是 bug 的形态"
        );
        // 正确的展开：别名 -> 真实 GUID（由 bcdedit /enum {别名} 提供）。
        let resolved = "{12e3701c-c361-11f1-90a2-c222b5348ef7}".to_string();
        assert_eq!(resolved, snapshot, "解析后的 GUID 必须与快照一致");
    }

    #[test]
    fn bcd_aliases_expand_to_the_real_default_identifier() {
        // bcdedit 的 displayorder 里会出现 {current}/{default} 这类别名。
        // 2026-10-12 实机：还原后第二项显示成 {default}，而快照里是完整 GUID，
        // 不做别名展开就会误判"原有启动菜单项丢失"。
        let cur_default = "{12e3701c-c361-11f1-90a2-c222b5348ef7}".to_string();
        let expand = |ids: &[String]| -> Vec<String> {
            ids.iter()
                .map(|id| match id.to_ascii_lowercase().as_str() {
                    "{current}" | "{default}" => cur_default.clone(),
                    _ => id.clone(),
                })
                .collect()
        };
        let snapshot = vec!["{12e3701c-c361-11f1-90a2-c222b5348ef7}".to_string()];
        let after = vec![
            "{2f72105a-c642-11f1-895d-d2749b7d5c85}".to_string(),
            "{default}".to_string(),
        ];
        let expected = expand(&snapshot);
        let actual = expand(&after);
        assert!(
            expected.iter().all(|id| actual.iter().any(|cur| cur == id)),
            "别名展开后，快照里的项必须能在还原后的菜单里找到"
        );
    }

    #[test]
    fn primary_partition_match_is_case_insensitive() {
        // bcdedit 报的 device 会被转成小写（partition=c:），而 SystemDrive 给的是
        // 大写 C:。直接 starts_with("partition=C:") 永远为 false，会把一次成功的
        // 还原判成"默认启动项不符合约定"。2026-10-12 实机正是这么误报的。
        let device = "partition=c:".to_string();
        let active_letter = 'C';
        assert!(
            device.starts_with(&format!(
                "partition={}:",
                active_letter.to_ascii_lowercase()
            )),
            "小写化的 device 必须能和大写盘符匹配上"
        );
        // 反向确认：若不做小写化，这个断言会失败——即bug的形态。
        assert!(!device.starts_with(&format!("partition={active_letter}:")));
    }

    #[test]
    fn version_name_uses_whichever_field_is_present() {
        // 只有一个字段可用时也要给出可区分的名字，而不是放弃。
        assert_eq!(
            read_image_version_name(Path::new("x.wim"), 1, "Index : 1\nEdition : Professional")
                .as_deref(),
            Some("Professional")
        );
        assert_eq!(
            read_image_version_name(Path::new("x.wim"), 1, "Index : 1\nVersion : 10.0.26200")
                .as_deref(),
            Some("Windows 10.0.26200")
        );
    }

    #[test]
    fn drive_validation_rejects_command_text() {
        assert_eq!(drive("d:").unwrap(), 'D');
        for value in ["", "CC", "C & echo x", "1", "C:\\"] {
            assert!(drive(value).is_err());
        }
    }
    // 在 Windows 上使用临时目录而不是用户卷，测试真正的 DISM 跨入口簿记。
    #[test]
    #[ignore = "需要 Windows 管理员和 DISM；只在显式验收时运行"]
    fn online_capture_append_keep_and_data_apply() {
        let root = std::env::temp_dir().join(format!(
            "br-online-test-{}",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        let image = root.join("backup.wim");
        let log_path = root.join("test.log");
        let mut identity = VolumeIdentity::new("test-disk", "test-partition");
        identity.partition_size = 1024 * 1024;
        let mut params = OnlineOpParams {
            operation: "backup".into(),
            source_drive: "Z".into(),
            target_drive: "Y".into(),
            image_path: image.to_string_lossy().into(),
            index: "1".into(),
            compress: "fast".into(),
            image_name: "中文 & % ! ^ 备份".into(),
            keep_indexes: None,
            format_target: false,
        };
        fs::write(source.join("data.txt"), "first").unwrap();
        capture(&params, &source, identity.clone(), 5, &log_path).unwrap();
        let first = crate::read_index_metadata(&image, 1).unwrap();
        fs::write(source.join("data.txt"), "second").unwrap();
        capture(&params, &source, identity.clone(), 6, &log_path).unwrap();
        assert_eq!(indexes(&image, &log_path).unwrap(), vec![1, 2]);
        let refreshed = crate::read_index_metadata(&image, 1).unwrap();
        assert_eq!(first.source, refreshed.source);
        assert_ne!(first.image_sha256, refreshed.image_sha256);
        params.keep_indexes = Some(2);
        fs::write(source.join("data.txt"), "third").unwrap();
        capture(&params, &source, identity, 5, &log_path).unwrap();
        assert_eq!(indexes(&image, &log_path).unwrap(), vec![1, 2]);
        let hash = backuprestore_core::sha256_file(&image).unwrap();
        for index in [1, 2] {
            assert_eq!(
                crate::read_index_metadata(&image, index)
                    .unwrap()
                    .image_sha256,
                hash
            );
        }
        assert!(!im::index_metadata_path(&image, 3).unwrap().exists());
        let target = root.join("restored");
        fs::create_dir(&target).unwrap();
        dism(
            &[
                "/English".into(),
                "/Apply-Image".into(),
                format!("/ImageFile:{}", image.display()),
                "/Index:2".into(),
                format!("/ApplyDir:{}", target.display()),
                "/CheckIntegrity".into(),
                "/Verify".into(),
            ],
            &log_path,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(target.join("data.txt")).unwrap(),
            "third"
        );
        let original_sidecars =
            [1, 2].map(|i| fs::read(im::index_metadata_path(&image, i).unwrap()).unwrap());
        assert!(
            capture(
                &params,
                &root.join("missing-source"),
                VolumeIdentity::new("x", "y"),
                1,
                &log_path
            )
            .is_err()
        );
        assert_eq!(backuprestore_core::sha256_file(&image).unwrap(), hash);
        for (i, bytes) in original_sidecars.iter().enumerate() {
            assert_eq!(
                &fs::read(im::index_metadata_path(&image, i as u32 + 1).unwrap()).unwrap(),
                bytes
            );
        }
        let bad_sidecar = im::index_metadata_path(&image, 1).unwrap();
        fs::write(&bad_sidecar, "invalid json").unwrap();
        assert!(
            capture(
                &params,
                &source,
                VolumeIdentity::new("x", "y"),
                1,
                &log_path
            )
            .is_err()
        );
        assert_eq!(backuprestore_core::sha256_file(&image).unwrap(), hash);
        fs::write(bad_sidecar, &original_sidecars[0]).unwrap();
        println!("ONLINE_E2E_SUCCESS log={} hash={hash}", log_path.display());
        // 保留小体积验收日志和产物，不删除失败证据。
    }

    #[test]
    fn step1_notice_splits_by_operation() {
        // 2026-10-15 用户裁定 A3：备份与还原共用本模块，开场白必须分叉——
        // 备份用户看到「会格式化并写入引导」会以为自己的盘要被动手。
        let backup = step1_notice("backup");
        assert!(
            backup.contains("只读取源卷") && backup.contains("不格式化"),
            "备份措辞必须强调只读源卷：{backup}"
        );
        assert!(
            !backup.contains("格式化目标卷"),
            "备份措辞不得出现格式化目标卷：{backup}"
        );
        let restore = step1_notice("restore");
        assert!(
            restore.contains("格式化目标卷") && restore.contains("写入引导"),
            "还原措辞要说明格式化与引导：{restore}"
        );
        assert!(
            restore.contains("默认启动项指向本次还原的系统"),
            "还原措辞必须反映 v2.6.1 翻转后的 default 归属：{restore}"
        );
        assert!(
            !restore.contains("不改动主系统默认启动项"),
            "旧措辞事实错误（翻转后 default 会改指），必须剔除：{restore}"
        );
    }
}
