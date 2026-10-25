//! 全局语言层：让「界面之外」的地方也能按语言输出。
//!
//! 【背景】本项目原本只有**界面**是多语言的：
//!   · GUI：`native_gui.rs` 的 `Language` 枚举 + `ui_text` / `tooltip_text` 双语表；
//!   · 恢复环境提示：`MessageLanguage`（经 `prepare --ui-language` → `UI_LANGUAGE`
//!     → `RecoveryTask.env` → `ui_language()` 传递）。
//! 但**日志和 CLI stdout 没有语言维度**——`append_log(path, message)` 不带
//! language 参数，258 个调用点全是中文硬编码。英文用户排障、以及自动化测试
//! 想让输出避开中文时，都无能为力。
//!
//! 【本模块的定位：进程级语言状态 + 关键文案表】
//!   ① 语言来源有优先级（见 `init_from_env` / `set`），一处设置全进程可见；
//!   ② 文案集中在 `MESSAGES` 表，按 key 取，缺失时**返回 key 本身**
//!      （宁可看到 `some_key`，也不要静默输出空串或错误语言）；
//!   ③ 「适当」多语言化——先覆盖**人最常看、判据最关心**的结论性文案
//!      （成功/失败/阶段/日志位置），细节调试日志暂保持中文，逐步补。
//!
//! 【为什么不改 `append_log` 的签名】258 个调用点逐个加参数是灾难级改动，
//! 且绝大多数细节日志不值得翻译。改为：需要双语的点主动查这张表。

use std::sync::atomic::{AtomicU8, Ordering};

/// 中英双语文案的语言维度。
///
/// ★ 2026-10-25 搬家：原来定义在 `pe_safety.rs`，而那个模块是
/// `#[cfg(any(windows, test))]`。问题在于 `text_parsing.rs` 是**无条件编译**的，
/// 它要用本模块的判据文案（终态识别要双语都认），于是 macOS 上不带 `test` 的
/// `cargo check` 直接 E0433——**`cargo test` 因为带 `test` cfg 反而能过**，
/// 所以 v2.6.41 只跑单测时没暴露。语言枚举本就属于语言层，搬到这里、让 `i18n`
/// 无条件编译即可；`pe_safety` 仍然 `pub(crate) use` 重导出，
/// 既有的 `crate::pe_safety::MessageLanguage` 引用一行都不用改。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageLanguage {
    Chinese,
    English,
}

/// 进程级语言。0 = 中文（默认，与既有行为一致），1 = 英文。
static LANG: AtomicU8 = AtomicU8::new(0);

/// 语言来源优先级（高 → 低）：
///   1. 显式 `set()`（GUI 切语言、RE 侧读到 `UI_LANGUAGE` 后调用）
///   2. 环境变量 `BACKUPRESTORE_LANG`（`en*` 判为英文，其余中文）
///   3. 默认中文
pub(crate) fn set(language: MessageLanguage) {
    LANG.store(
        match language {
            MessageLanguage::Chinese => 0,
            MessageLanguage::English => 1,
        },
        Ordering::SeqCst,
    );
}

pub(crate) fn get() -> MessageLanguage {
    match LANG.load(Ordering::SeqCst) {
        1 => MessageLanguage::English,
        _ => MessageLanguage::Chinese,
    }
}

/// 进程启动时调用一次：环境变量可以覆盖默认语言，便于无人值守/自动化场景。
/// 例：`BACKUPRESTORE_LANG=en` → 日志与 CLI 结论文案走英文。
///
/// 优先级：`BACKUPRESTORE_LANG` > 界面里选过的语言（`BackupRestore.settings.ini`
/// 的 `ui_language`）> 中文。
///
/// ★ 2026-10-25 补第二级：此前 CLI 完全不理界面偏好，用户在 GUI 里选了 English，
/// 命令行日志照样中文，两套口径对不上。**环境变量仍然最高**——自动化脚本要的是
/// 确定性，不能被某台机器上界面选过什么影响。
pub(crate) fn init_from_env() {
    if let Ok(value) = std::env::var("BACKUPRESTORE_LANG") {
        let value = value.trim().to_ascii_lowercase();
        if value.starts_with("en") {
            set(MessageLanguage::English);
            return;
        }
        if value.starts_with("zh") {
            set(MessageLanguage::Chinese);
            return;
        }
        // 值不认识 → 不当作显式指定，继续往下看界面偏好
    }
    if let Some(language) = settings_language() {
        set(language);
    }
}

/// `BackupRestore.settings.ini` 里语言偏好的键名（与 `native_gui` 共用，
/// 单一来源——两处各写一份字面量迟早会不同步）。
pub(crate) const SETTINGS_UI_LANGUAGE_KEY: &str = "ui_language";

/// 读界面语言偏好。文件缺失 / 读失败 / 值不认识 → `None`（保持默认中文）。
///
/// 只读不写：CLI 不该修改用户界面设置。
fn settings_language() -> Option<MessageLanguage> {
    let path = std::env::current_exe()
        .ok()?
        .parent()?
        .join(crate::settings_file::SETTINGS_FILE_NAME);
    let text = std::fs::read_to_string(path).ok()?;
    parse_settings_language(&text)
}

/// 纯函数：从设置文件文本里取语言偏好（本机单测可直接覆盖）。
fn parse_settings_language(text: &str) -> Option<MessageLanguage> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != SETTINGS_UI_LANGUAGE_KEY {
            continue;
        }
        return match value.trim().to_ascii_lowercase().as_str() {
            "en" | "english" => Some(MessageLanguage::English),
            "zh" | "chinese" => Some(MessageLanguage::Chinese),
            _ => None,
        };
    }
    None
}

/// 文案缺失时的哨兵。
const MISSING: &str = "<i18n:missing>";

/// 关键文案表：`(key, 中文, 英文)`。新增文案请按字母序插入并补单测。
const MESSAGES: &[(&str, &str, &str)] = &[
    // 在线通道（备份 / 还原数据卷 / 第二系统）主链：STEP 行、门禁对账、副档发布。
    // ★ 这批用的是模块自己的 `log()` 而不是 `append_log`，2026-10-25 第一遍统计
    //   漏了它们（按 `append_log(` 数的），实测日志里还剩中文才补上。
    (
        "ol_backup_space_warn",
        "镜像卷可用空间 {0} 低于新索引的保守估计 {1}（源卷已用 {2}）。不阻止本次备份，\
但若空间耗尽，捕获将失败并回滚（原镜像不受影响）。",
        "Free space on the image volume ({0}) is below the conservative estimate for the new index \
({1}); the source volume uses {2}. This run is not blocked, but if space runs out the capture \
fails and rolls back (the original image is untouched).",
    ),
    (
        "ol_backup_space_unknown",
        "无法读取镜像卷 {0}: 的可用空间（{1}），跳过备份前空间预检。",
        "Cannot read the free space of image volume {0}: ({1}); skipping the pre-backup space check.",
    ),
    (
        "ol_boot_written",
        "引导与启动菜单已写入；系统实际可启动性需另行启动验收",
        "Boot entries and the boot menu are written; actual bootability still needs a separate \
boot test",
    ),
    (
        "ol_candidate_dir",
        "候选目录：{0}；失败时保留，原文件不覆盖",
        "Candidate directory: {0}; kept on failure, the original files are not overwritten",
    ),
    (
        "ol_format_verified",
        "格式化校验通过：序列号 {0} -> {1}，分区身份不变",
        "Format verified: serial {0} -> {1}, partition identity unchanged",
    ),
    (
        "ol_gate_enum_failed",
        "门禁对象 {0} 枚举失败（按被取代处理）",
        "Gate object {0} could not be enumerated (treated as superseded)",
    ),
    (
        "ol_gate_lost",
        "门禁对象 {0} 仍存在 device={1} 判定丢失={2}",
        "Gate object {0} still present device={1} lost={2}",
    ),
    (
        "ol_gate_reconcile",
        "门禁对账：快照菜单 {0}；现菜单 {1}；缺失 {2}",
        "Gate reconciliation: snapshot menu {0}; current menu {1}; missing {2}",
    ),
    (
        "ol_gate_superseded",
        "门禁对象 {0} 已被删（枚举无标识符，被取代），不算丢失",
        "Gate object {0} was deleted (no identifier when enumerated; superseded) and is not \
counted as lost",
    ),
    (
        "ol_legacy_sidecar",
        "[WARN] 旧版目录级副档无法读取，不作为本次依据：{0}",
        "[WARN] The legacy directory-level sidecar could not be read and is not used as evidence: {0}",
    ),
    (
        "ol_menu_dup",
        "启动菜单名与现有启动项重名，已消解：{0} -> {1}",
        "Boot menu name collided with an existing entry and was resolved: {0} -> {1}",
    ),
    (
        "ol_menu_version_name",
        "引导菜单将使用镜像版本名：{0}",
        "The boot menu will use the image version name: {0}",
    ),
    (
        "ol_overlay_marked",
        "覆盖式应用（取消格式化）命中已含 Windows 的目标卷：不再硬拒，本次将走 wimlib 通道（镜像目录权限完整还原）",
        "Overlay apply (format unchecked) hit a target volume that already contains Windows: it is \
no longer rejected and this run uses the wimlib channel (image directory permissions restored \
in full)",
    ),
    (
        "ol_publish_failed",
        "[ERROR] 发布失败：{0}；开始恢复原镜像和副档",
        "[ERROR] Publish failed: {0}; restoring the original image and sidecar",
    ),
    (
        "ol_published",
        "镜像及副档已发布并回读；候选目录已清理",
        "Image and sidecar published and read back; the candidate directory was cleaned up",
    ),
    (
        "ol_sidecar_missing",
        "[WARN] 旧索引 {0} 无副档，保留未知来源状态，不伪造源卷信息",
        "[WARN] Legacy index {0} has no sidecar; its provenance stays unknown and no source \
information is fabricated",
    ),
    (
        "ol_step_apply_data_fmt",
        "STEP 2.5/4 应用镜像（数据卷已按选项格式化后写入）",
        "STEP 2.5/4 Apply the image (the data volume was formatted per options before writing)",
    ),
    (
        "ol_step_apply_data_overlay",
        "STEP 2/4 还原镜像（数据卷在线覆盖，不格式化、不修复引导）",
        "STEP 2/4 Restore the image (online overlay on a data volume; no format, no boot repair)",
    ),
    (
        "ol_step_apply_dism",
        "STEP 3/4 应用镜像（DISM /Apply-Image）并校验系统文件",
        "STEP 3/4 Apply the image (DISM /Apply-Image) and verify system files",
    ),
    (
        "ol_step_apply_wimlib",
        "STEP 3/4 应用镜像（wimlib 覆盖式通道）并校验系统文件",
        "STEP 3/4 Apply the image (wimlib overlay channel) and verify system files",
    ),
    (
        "ol_step_boot",
        "STEP 4/4 修复引导（菜单命名/顺序/默认项，默认启动项=本次还原的系统）",
        "STEP 4/4 Repair boot (menu name/order/default; the default entry is the system restored \
in this run)",
    ),
    (
        "ol_step_capture",
        "STEP 2/4 捕获镜像（候选副本，无固定超时）",
        "STEP 2/4 Capture the image (candidate copy, no fixed timeout)",
    ),
    (
        "ol_step_done_fmt",
        "STEP 4/4 完成在线还原（已格式化后写入）；未执行系统引导修复",
        "STEP 4/4 Online restore complete (written after formatting); no system boot repair was \
performed",
    ),
    (
        "ol_step_done_overlay",
        "STEP 4/4 完成在线还原；未执行格式化或系统引导修复",
        "STEP 4/4 Online restore complete; neither formatting nor system boot repair was performed",
    ),
    (
        "ol_step_format",
        "STEP 2/4 格式化目标卷（diskpart quick）并校验格式化成功",
        "STEP 2/4 Format the target volume (diskpart quick) and verify it succeeded",
    ),
    (
        "ol_step_keep_verify",
        "STEP 3/4 保留策略、索引回读和最终哈希校验",
        "STEP 3/4 Keep policy, index read-back and final hash verification",
    ),
    (
        "ol_step_letter",
        "STEP 3.5/4 绑定第二系统盘符（离线 SYSTEM/SOFTWARE，主系统不受影响）",
        "STEP 3.5/4 Bind the secondary system's drive letter (offline SYSTEM/SOFTWARE; the primary \
system is unaffected)",
    ),
    (
        "ol_step_preboot",
        "STEP 3.6/4 预建还原系统启动项（等待期间即可在启动菜单看到并选择）",
        "STEP 3.6/4 Pre-create the restored system's boot entry (visible and selectable in the boot \
menu while waiting)",
    ),
    (
        "ol_step_publish",
        "STEP 4/4 发布镜像并回读副档；旧文件在事务成功前保留",
        "STEP 4/4 Publish the image and read back the sidecar; the old files are kept until the \
transaction succeeds",
    ),
    (
        "ol_step_secondary",
        "STEP 2/4 在线部署第二系统（格式化 → 应用 → 修复引导，不重启）",
        "STEP 2/4 Deploy the secondary system online (format → apply → repair boot; no reboot)",
    ),
    (
        "ol_step_snapshot_fmt",
        "STEP 2/4 还原镜像前先快照引导状态并格式化目标卷",
        "STEP 2/4 Snapshot the boot state and format the target volume before restoring",
    ),
    (
        "ol_step_snapshot_overlay",
        "STEP 2/4 还原镜像前先快照引导状态（按选项跳过格式化，覆盖式应用）",
        "STEP 2/4 Snapshot the boot state before restoring (format skipped per options; overlay apply)",
    ),
    (
        "ol_step_verify",
        "STEP 3/4 回读目标卷身份；DISM 已执行完整性和应用校验",
        "STEP 3/4 Read back the target volume identity; DISM already ran integrity and apply \
verification",
    ),
    (
        "ol_verify_done",
        "校验完成：sha256={0} bytes={1} 保留={2} 删除={3}",
        "Verification complete: sha256={0} bytes={1} keep={2} remove={3}",
    ),
    (
        "ol_volume_identity",
        "卷身份：{0}",
        "Volume identity: {0}",
    ),
    (
        "pe_boot_verified",
        "引导与恢复环境关联已回读；系统实际可启动性需另行启动验收",
        "Boot entries and the recovery-environment link were read back; actual bootability still \
needs a separate boot test",
    ),
    (
        "pe_cmd_exit",
        "命令退出：{0}；原始输出：{1}",
        "Command exit: {0}; raw output: {1}",
    ),
    (
        "pe_cmd_start",
        "命令开始：{0} {1}",
        "Command start: {0} {1}",
    ),
    (
        "pe_precheck_passed",
        "预检通过：目标={0}；镜像摘要={1}；索引={2}；意图={3}",
        "Pre-check passed: target={0}; image digest={1}; index={2}; intent={3}",
    ),
    (
        "pe_status",
        "状态：{0}",
        "Status: {0}",
    ),
    // 恢复环境主链：续跑 / 恢复失败 / 盘符解析 / ESP 提前挂载 / default 归属。
    // 这些是排障时最先翻的行——2026-10-25 之前全是中文硬编码，英文用户只能猜。
    (
        "assign_letter_failed",
        "按 GUID 给 {0}: 分配盘符失败（{1}）：{2}",
        "Failed to assign a drive letter to {0}: by GUID ({1}): {2}",
    ),
    (
        "assign_letter_mismatch",
        "{0}: 分配后身份不符（现场 {1}），继续尝试下一个字母",
        "{0}: identity mismatch after assignment (live {1}); trying the next letter",
    ),
    (
        "assign_letter_ok",
        "卷原本没有盘符，已按 GUID 分配到 {0}: 并复核身份通过",
        "The volume had no drive letter; assigned {0}: by GUID and re-verified the identity",
    ),
    (
        "default_switched",
        "默认启动项已指向本次还原的系统（{0}）",
        "The default boot entry now points to the system restored in this run ({0})",
    ),
    (
        "default_switching",
        "默认启动项未指向本次还原的系统（{0}），立即改为 {1}；否则还原完成后开机进的还是旧系统",
        "The default boot entry did not point to the system restored in this run ({0}); changed it \
to {1} immediately, otherwise the next boot would still enter the old system",
    ),
    (
        "device_has_letter",
        "disk{0}/partition{1} 在现场对应盘符 {2}:",
        "disk{0}/partition{1} maps to drive {2}: on this machine",
    ),
    (
        "device_no_letter",
        "disk{0}/partition{1} 在现场没有盘符（按设备路径核对）",
        "disk{0}/partition{1} has no drive letter on this machine (checked by device path)",
    ),
    (
        "esp_mount_failed",
        "按任务记录挂载 ESP 失败（{0}）；改走免盘符探测",
        "Failed to mount the ESP as recorded in the task ({0}); falling back to letter-free probing",
    ),
    (
        "esp_mounted_early",
        "STEP 1 前已按任务记录挂载 ESP 到 {0}:（提前解析 EFI 根）",
        "ESP mounted to {0}: before STEP 1 as recorded in the task (EFI root resolved early)",
    ),
    (
        "esp_probed_early",
        "STEP 1 前免盘符探测到 ESP 并解析到 {0}:（提前解析 EFI 根）",
        "Probed the ESP without a drive letter before STEP 1 and resolved it to {0}: \
(EFI root resolved early)",
    ),
    (
        "mount_identity_verified",
        "挂载稳定身份已核验：准备期 {0}/{1}，现场 {2}/{3}",
        "Mount identity verified: planned {0}/{1}, live {2}/{3}",
    ),
    (
        "mountvol_output",
        "mountvol 输出：{0}",
        "mountvol output: {0}",
    ),
    (
        "mountvol_table_unavailable",
        "mountvol 挂载表不可用（{0}）；退回逐字母探测",
        "The mountvol table is unavailable ({0}); falling back to probing one letter at a time",
    ),
    (
        "panic_backtrace",
        "panic in thread [{0}] at {1}: {2}\nbacktrace (RUST_BACKTRACE=1 可展开): {3}",
        "panic in thread [{0}] at {1}: {2}\nbacktrace (set RUST_BACKTRACE=1 to expand): {3}",
    ),
    (
        "re_registration_degraded",
        "恢复环境注册已按裁定降级跳过：{0}",
        "Recovery-environment registration skipped by the agreed degradation: {0}",
    ),
    (
        "recovery_done_log_write",
        "Recovery completed; cleanup pending: {0}; 且无法写入日志：{1}",
        "Recovery completed; cleanup pending: {0}; and the log could not be written: {1}",
    ),
    (
        "recovery_done_status_write",
        "Recovery completed; cleanup pending: {0}; 且无法写入 status 错误字段：{1}",
        "Recovery completed; cleanup pending: {0}; and the status error field could not be \
written: {1}",
    ),
    (
        "recovery_failed",
        "Recovery failed: {0}（原始错误已落 Recovery.log）",
        "Recovery failed: {0} (the original error is in Recovery.log)",
    ),
    (
        "recovery_failed_log_write",
        "Recovery failed: {0}; 且无法写入日志：{1}",
        "Recovery failed: {0}; and the log could not be written: {1}",
    ),
    (
        "recovery_failed_status_write",
        "Recovery failed: {0}; 且无法写入 status 错误字段：{1}",
        "Recovery failed: {0}; and the status error field could not be written: {1}",
    ),
    (
        "restore_identity_rebound",
        "恢复目标稳定身份已核验，现场地址重绑定：{0} -> {1}",
        "Restore target identity verified; live address rebound: {0} -> {1}",
    ),
    (
        "resume_abandoned",
        "用户放弃待续跑任务（task={0} stage={1}）：按终态收口",
        "The user abandoned the pending-resume task (task={0} stage={1}): closed as a terminal state",
    ),
    (
        "resume_declined",
        "用户选择本次不自动重启续跑（task={0} stage={1}）；任务、启动对象与载荷全部保留，可用 resume 命令或主窗口「取消待重启任务」继续处理",
        "The user declined the automatic resume restart this time (task={0} stage={1}); the task, \
boot objects and payload are all preserved — continue with the resume command or the main \
window's \"Cancel pending reboot task\"",
    ),
    (
        "resume_detected",
        "Detected durable interrupted task after normal Windows startup; resuming task {0} operation={1} stage={2}（计划中的跨阶段第 {3}/{4} 次）",
        "Detected a durable interrupted task after a normal Windows start; resuming task {0} \
operation={1} stage={2} (planned cross-stage attempt {3}/{4})",
    ),
    (
        "resume_limit_reached",
        "自动续跑已达跨阶段总上限 {0} 次（已重启 {1} 次）：不再自动重启，交由用户决定（任务与载荷全部保留）",
        "Auto-resume reached the cross-stage cap of {0} ({1} restarts already): no more automatic \
restarts, it is now up to the user (task and payload are fully preserved)",
    ),
    (
        "resume_restart_requested",
        "Restart requested to resume pending task in Windows RE（跨阶段第 {0}/{1} 次）",
        "Restart requested to resume the pending task in Windows RE (cross-stage attempt {0}/{1})",
    ),
    (
        "route_secondary",
        "收尾路由=第二系统：bcdboot 不带 /d（带 /addlast），default 指向新还原的系统（随后 assert 回读确认）",
        "Finish routing = secondary system: bcdboot without /d (with /addlast); default points to \
the newly restored system (asserted by read-back afterwards)",
    ),
    (
        "route_unique",
        "收尾路由=重建唯一系统：bcdboot 不带 /d，default 指向新 loader（apply 策略=指向还原目标）",
        "Finish routing = rebuild the single system: bcdboot without /d; default points to the new \
loader (apply policy = point at the restore target)",
    ),
    (
        "volume_resolved_by_guid",
        "卷已按卷 GUID 解析到现场盘符 {0}:（任务记录 {1}）",
        "Volume resolved by volume GUID to live drive {0}: (task recorded {1})",
    ),
    (
        "volume_resolved_by_probe",
        "卷已按身份逐字母探测解析到 {0}:（任务记录 {1}）",
        "Volume resolved by identity probing over letters to {0}: (task recorded {1})",
    ),
    (
        "volume_resolved_same_letter",
        "卷已解析：记录盘符 {0}: 仍指向同一卷（身份复核通过）",
        "Volume resolved: the recorded drive {0}: still points at the same volume (identity re-verified)",
    ),
    // DISM apply 失败的六类因果（`failure_policy::ApplyFailureKind::cause`）。
    // ★ 措辞纪律：这些文案会被拼进 Err 文本、可能再次进入 `classify_apply_failure`
    //   的输入，所以英文里**不能出现分类模式的整串**（`access is denied` /
    //   `not enough space` / `disk is full` / `being used by another process` /
    //   `sharing violation`），否则 Busy 的文案会被认成 AccessDenied 之类。
    //   语义本来就一致的（corrupt / timed out）可以保留。
    (
        "cause_access_denied",
        "目标卷上受保护目录的权限改写被拒（访问被拒）。这类失败正是 wimlib 通道的设计目标，将自动改用 wimlib 重试。",
        "Rewriting permissions on a protected directory of the target volume was denied. \
This is exactly what the wimlib channel was built for; it will retry with wimlib automatically.",
    ),
    (
        "cause_busy",
        "目标卷正被其它程序占用（文件句柄未释放）。请关闭占用目标卷的程序后重试。",
        "The target volume is locked by another program (a file handle is still open). \
Close whatever is using it and retry.",
    ),
    (
        "cause_disk_full",
        "目标卷剩余空间不足，镜像无法完整写入。请清理目标卷或换用更大的卷后重试；不要将部分写入视为成功。",
        "The target volume is out of free space, so the image cannot be written completely. \
Free up space or use a larger volume and retry; do not treat a partial write as success.",
    ),
    (
        "cause_image_corrupt",
        "镜像文件损坏或不完整（校验未通过）。请重新制作备份镜像；同一镜像换任何应用引擎都会失败。",
        "The image is corrupt or incomplete (verification failed). Rebuild the backup; \
any apply engine will fail on the same image.",
    ),
    (
        "cause_timed_out",
        "应用镜像超过等待上限被终止（上限可在设置中调整，默认不限制）。卷上可能留有半成品，重跑前建议先格式化目标卷。",
        "Applying the image timed out and was terminated (the limit is configurable in settings, \
unlimited by default). The volume may hold a partial write; format the target before retrying.",
    ),
    (
        "cause_unknown",
        "应用镜像失败，原因未识别。请把本日志交给维护者；原始输出已保留在下方。",
        "Applying the image failed for an unrecognised reason. Hand this log to the maintainer; \
the raw output is preserved below.",
    ),
    (
        "apply_classified",
        "[ERROR] DISM apply 失败分类：{0}——{1}",
        "[ERROR] DISM apply failure classified as {0}: {1}",
    ),
    (
        "fallback_ok",
        "降级成功：wimlib 完成 apply，任务继续",
        "Fallback succeeded: wimlib finished the apply, the task continues",
    ),
    (
        "fallback_skip_corrupt",
        "未自动降级：镜像损坏类失败换应用引擎同样会失败，重试无益",
        "Fallback skipped: an image-corruption failure would fail on any apply engine; retrying is pointless",
    ),
    (
        "fallback_skip_missing",
        "未尝试自动降级：wimlib 二进制不可用——{0}",
        "Fallback not attempted: the wimlib binary is unavailable — {0}",
    ),
    (
        "fallback_start",
        "[WARN] DISM apply 失败（{0}），自动降级 wimlib 重试一次（等待上限：不限制）",
        "[WARN] DISM apply failed ({0}); falling back to wimlib once (wait limit: unlimited)",
    ),
    // apply 关键路径：这两条是排障时最先看的（等待上限、退出码）。
    // ★ 安全性已核实：`failure_policy::strip_self_log_lines` 只剥"时间戳行"
    //   与"命令行回显"（`/Apply-Image` 等 ASCII 参数），**与语言无关**；
    //   失败分类本身靠 DISM 的英文错误串与 NTSTATUS 码，也不依赖中文，
    //   故翻译这两条不会破坏 D1/D2 的自动降级判据。
    (
        "dism_result",
        "DISM 结果：{0}",
        "DISM result: {0}",
    ),
    (
        "dism_start",
        "DISM 开始（等待上限 {0}）：{1}",
        "DISM start (wait limit {0}): {1}",
    ),
    (
        "log_location",
        "日志位置：",
        "Log location: ",
    ),
    (
        "log_label",
        "日志：",
        "Log: ",
    ),
    (
        "log_unified_note",
        "（统一日志根，不随 %TEMP% 清理丢失）",
        " (unified log root, not lost when %TEMP% is cleaned)",
    ),
    (
        "online_done",
        "操作完成：命令、回读和副档簿记均已通过",
        "Operation complete: command, read-back and sidecar bookkeeping all passed",
    ),
    (
        "online_failed_prefix",
        "[ERROR] 在线操作未完成：",
        "[ERROR] Online operation did not complete: ",
    ),
    (
        "online_failed_suffix",
        "；不要将部分产物视为有效备份",
        "; do not treat partial output as a valid backup",
    ),
    // 恢复环境进度窗：BCD 回退 / 命令提示符 / 引导修复编辑器。
    (
        "actor_click",
        "用户点击",
        "User click",
    ),
    (
        "actor_key_b",
        "用户按 B 键",
        "User pressed B",
    ),
    (
        "re_boot_editor_failed",
        "引导修复编辑窗口创建失败",
        "Failed to create the boot-repair editor window",
    ),
    (
        "re_cmd_open_failed",
        "打开命令提示符失败：CreateProcessW 返回 0",
        "Failed to open a command prompt: CreateProcessW returned 0",
    ),
    (
        "re_cmd_opened",
        "已打开命令提示符（用户请求，恢复流程不受影响）",
        "Command prompt opened (user request; the recovery flow is unaffected)",
    ),
    (
        "re_rollback_busy",
        "BCD 回退：上一次回退仍在执行，本次请求已忽略",
        "BCD rollback: the previous rollback is still running; this request was ignored",
    ),
    (
        "re_rollback_done",
        "{0} BCD 回退：已按字节恢复准备期 BCD 快照",
        "{0} BCD rollback: the preparation-time BCD snapshot was restored byte-for-byte",
    ),
    (
        "re_rollback_failed",
        "{0} BCD 回退失败：{1}",
        "{0} BCD rollback failed: {1}",
    ),
    (
        "re_rollback_unavailable",
        "BCD 回退尚不可用：恢复流程尚未装载任务目录/ESP 身份",
        "BCD rollback not available yet: the recovery flow has not loaded the task directory / ESP identity",
    ),
    (
        "re_window_close_timeout",
        "进度窗口线程 {0} 秒内没有退出，`close_and_wait` 放弃等待并继续恢复链（窗口可能卡在日志刷新或 BCD 回退）",
        "The progress-window thread did not exit within {0}s; `close_and_wait` gave up waiting and \
continued the recovery chain (the window may be stuck refreshing logs or rolling back BCD)",
    ),
    (
        "re_window_closed_paused",
        "用户关闭进度窗口（当时处于暂停）：恢复链将中断并按失败处理，现场保留",
        "The user closed the progress window while paused: the recovery chain will be aborted as a \
failure (scene preserved)",
    ),
    // 恢复环境进度窗：暂停 / 继续 / 逃出。用户就在屏幕前看着这些行，
    // 英文用户尤其需要（这是"卡住了吗"和"我按了暂停所以它停了"的唯一解释）。
    (
        "re_pause_limit",
        "暂停总上限 {0} 秒（累计，本次继续后不再重置）；剩余约 {1} 秒，超时将中断恢复链并按失败处理",
        "Pause limit {0}s in total (cumulative, resuming does not reset it); about {1}s left, \
after which the recovery chain is aborted and treated as a failure",
    ),
    (
        "re_pause_limit_reached",
        "暂停已达总上限 {0} 秒：中断恢复链，按失败处理（现场保留）",
        "Pause reached the {0}s total limit: aborting the recovery chain as a failure \
(scene preserved)",
    ),
    (
        "re_pause_timeout_err",
        "暂停等待超时，恢复已中断",
        "Pause wait timed out; recovery aborted",
    ),
    (
        "re_pause_window_closed",
        "进度窗口已关闭且当时处于暂停：中断恢复链，按失败处理（现场保留）",
        "The progress window was closed while paused: aborting the recovery chain as a failure \
(scene preserved)",
    ),
    (
        "re_paused",
        "恢复已暂停（用户请求），停在：{0}",
        "Recovery paused (user request), stopped at: {0}",
    ),
    (
        "re_resumed",
        "恢复已继续（用户请求）",
        "Recovery resumed (user request)",
    ),
    (
        "re_window_closed_err",
        "进度窗口已关闭，恢复已中断",
        "The progress window was closed; recovery aborted",
    ),
    // PE 安装 / 镜像完整性预检 / 恢复环境注册 / 第二系统绑定。
    (
        "boot_comp_delete_failed",
        "补偿删除 {0} 失败（可能留下孤儿 BCD 对象）：{1}",
        "Compensating delete of {0} failed (an orphan BCD object may remain): {1}",
    ),
    (
        "boot_comp_delete_ok",
        "补偿删除 {0}：成功{1}",
        "Compensating delete of {0}: succeeded{1}",
    ),
    (
        "op_failed",
        "{0} 失败：{1}",
        "{0} failed: {1}",
    ),
    (
        "pe_diskpart_result",
        "diskpart 执行结果：{0}",
        "diskpart result: {0}",
    ),
    (
        "pe_fmt_after",
        "PE 格式化后身份：{0}",
        "PE post-format identity: {0}",
    ),
    (
        "pe_fmt_before",
        "PE 格式化前身份：{0}；脚本：{1}",
        "PE pre-format identity: {0}; script: {1}",
    ),
    (
        "pe_fmt_verified",
        "格式化身份与新卷序列号已回读验证（文件系统跟随目标卷）",
        "Post-format identity and the new volume serial were read back and verified \
(the filesystem follows the target volume)",
    ),
    (
        "pe_output_suffix",
        " | 输出：{0}",
        " | output: {0}",
    ),
    (
        "secondary_letter_bound",
        "第二系统离线盘符已绑定并核验：{0}: -> {1}；原配置备份 {2}",
        "Secondary-system offline drive letter bound and verified: {0}: -> {1}; \
original config backed up to {2}",
    ),
    (
        "winre_independent_partition",
        "已启用 WinRE 位于独立恢复分区 {0}（现场盘符 {1}），恢复原件齐备，按独立分区重接",
        "WinRE is enabled on a dedicated recovery partition {0} (live letter {1}); \
the original files are intact, so it is reconnected on that partition",
    ),
    (
        "winre_secondary_reset",
        "第二系统克隆注册配置已在目标卷内备份并重置；原系统恢复对象未禁用",
        "The secondary-system clone registry config was backed up and reset inside the target \
volume; the original system's recovery object was not disabled",
    ),
    (
        "wp_boot_menu_dup",
        "启动菜单名与现有启动项重名，已自动消解：{0} -> {1}",
        "Boot menu name collided with an existing entry and was resolved automatically: {0} -> {1}",
    ),
    (
        "wp_overlay_marked",
        "覆盖式应用（取消格式化）命中已含 Windows 的目标卷：不再硬拒，已标记 overlay_no_acl，恢复期将走 wimlib 通道（镜像目录权限完整还原；DISM 该组合必然 RestoreDirMetadata 0x80070005）",
        "Overlay apply (format unchecked) hit a target volume that already contains Windows: \
it is no longer rejected; overlay_no_acl is marked and the recovery stage will use the wimlib \
channel (image directory permissions restored in full; DISM always fails this combination with \
RestoreDirMetadata 0x80070005)",
    ),
    (
        "wp_precheck_fast_path",
        "镜像完整性预检：镜像名「{0}」符合完整 Windows 特征，走快速通道放行（跳过挂载，省一次大镜像挂载）",
        "Image integrity pre-check: the image name \"{0}\" matches a full Windows image, so it is \
fast-pathed (mounting is skipped, saving one large-image mount)",
    ),
    (
        "wp_precheck_heuristic",
        "镜像完整性预检：只读挂载不可用，已按镜像名启发式放行；恢复阶段的离线注册前置检查兜底",
        "Image integrity pre-check: read-only mounting was unavailable, so it was passed by the \
image-name heuristic; the offline registration pre-check during recovery is the backstop",
    ),
    (
        "wp_precheck_mounted_ok",
        "镜像完整性预检：只读挂载已确认完整 Windows（reagentc.exe + SYSTEM 齐全）",
        "Image integrity pre-check: read-only mounting confirmed a full Windows image \
(reagentc.exe and SYSTEM present)",
    ),
    (
        "wp_volume_cache_flushed",
        "卷缓存已强制落盘：{0}:（{1}）",
        "Volume cache flushed: {0}: ({1})",
    ),
    // 恢复环境里的「引导修复方案」（选哪些系统进菜单、菜单名、超时）。
    (
        "boot_candidate_missing_components",
        "卷缺少常规 Windows 恢复组件，保留现有入口：{0}",
        "Volume lacks the usual Windows recovery components; keeping the existing entry: {0}",
    ),
    (
        "boot_candidate_probe_failed",
        "引导候选卷探测失败，跳过：{0}；{1}",
        "Boot candidate volume probe failed, skipped: {0}; {1}",
    ),
    (
        "boot_candidate_unreadable",
        "引导候选卷身份不可读，跳过：{0}；{1}",
        "Boot candidate volume identity unreadable, skipped: {0}; {1}",
    ),
    (
        "boot_menu_dup_display",
        "引导菜单默认名重复，已加盘符区分：{0} -> {1}",
        "Duplicate default boot menu name; disambiguated by drive letter: {0} -> {1}",
    ),
    (
        "boot_menu_dup_resolved",
        "启动菜单目标名与其它菜单项重名，已消解：{0} -> {1}",
        "Boot menu target name collided with another entry and was resolved: {0} -> {1}",
    ),
    (
        "boot_menu_name_kept",
        "引导菜单名称保持不变：{0}",
        "Boot menu name unchanged: {0}",
    ),
    (
        "boot_menu_name_written",
        "引导菜单名称写入：{0} -> {1}",
        "Boot menu name written: {0} -> {1}",
    ),
    (
        "boot_menu_plan_ready",
        "引导修复方案已准备：{0} 个系统；本次目标必选；无倒计时",
        "Boot-repair plan ready: {0} systems; this run's target is mandatory; no countdown",
    ),
    (
        "boot_menu_save_failed",
        "引导修复方案保存失败：{0}",
        "Failed to save the boot-repair plan: {0}",
    ),
    (
        "boot_menu_saved",
        "引导修复选择已保存，恢复继续运行",
        "Boot-repair selection saved; recovery continues",
    ),
    (
        "boot_repair_done",
        "引导修复完成：菜单 {0} 项，显示 {1} 秒；现有 PE 入口保留",
        "Boot repair finished: {0} menu entries, {1}s timeout; existing PE entries kept",
    ),
    (
        "default_decision",
        "默认启动项决策：{0}；{1}",
        "Default boot entry decision: {0}; {1}",
    ),
    (
        "label_new_entry",
        "（新建）",
        "(new)",
    ),
    // 恢复环境主链 / 引导决策的结论性文案。
    (
        "boot_candidates_ready",
        "引导修复候选已准备：第二阶段显示编辑按钮",
        "Boot-repair candidates prepared: the edit button appears in the second stage",
    ),
    (
        "data_volume_skip_efi",
        "目标卷不是 Windows 系统卷且不是新增系统，判定为数据卷还原：不需要 EFI 根，跳过提前挂载",
        "The target is neither a Windows system volume nor a new secondary system, so this is a \
data-volume restore: no EFI root is needed and the early mount is skipped",
    ),
    (
        "default_already_switched",
        "默认启动项已指向本次还原的系统",
        "The default boot entry already points to the system restored in this run",
    ),
    (
        "overlay_skip_format",
        "按任务选项跳过格式化（覆盖式应用）：目标卷上镜像没有的文件将保留",
        "Skipping the format per task options (overlay apply): files on the target that are not in \
the image will be kept",
    ),
    (
        "resume_paused",
        "自动续跑已暂停；任务阶段、启动对象和恢复载荷全部保留，可使用 resume 命令重试",
        "Auto-resume paused; the task stage, boot objects and recovery payload are all preserved — \
retry with the resume command",
    ),
    (
        "rollback_bcd_noted",
        "本次运行中用户按 B 回退了 BCD：ESP 上的 BCD 已还原为任务准备期快照，任务自建的引导项随之消失，续跑链校验失败属预期（不是新增缺陷）",
        "The user rolled back BCD with the B key during this run: the ESP's BCD was restored to the \
pre-task snapshot and the task's own boot entry disappeared with it, so a resume-chain \
verification failure is expected (not a new defect)",
    ),
    (
        "skip_boot_candidates_not_windows",
        "目标卷不是 Windows 系统卷，跳过引导修复候选探测",
        "The target volume is not a Windows system volume; skipping boot-repair candidate probing",
    ),
    // 编号步骤行。解析器只认 `STEP ` 前缀与 `n/m` 数字（`text_parsing`
    // 用 `line.find("STEP ")`），后缀是给人看的阶段名——翻译不影响进度跟踪。
    // ★ 2026-10-25 顺手统一了一处历史笔误：三处写「清理re启动项/配置」、
    //   一处写「清理恢复启动项/配置」，同一阶段两个名字，进度窗与日志对不上。
    (
        "step_apply",
        "STEP 2/4 还原镜像（时间长）",
        "STEP 2/4 Apply the image (takes a while)",
    ),
    (
        "step_capture",
        "STEP 2/4 捕获镜像（时间长）",
        "STEP 2/4 Capture the image (takes a while)",
    ),
    (
        "step_cleanup",
        "STEP 4/4 清理恢复启动项/配置",
        "STEP 4/4 Clean up recovery boot entries and configuration",
    ),
    (
        "step_mount",
        "STEP 1/4 挂载卷、校验",
        "STEP 1/4 Mount volumes and verify",
    ),
    (
        "step_verify",
        "STEP 3/4 校验",
        "STEP 3/4 Verify",
    ),
    (
        "stage_done",
        "操作完成",
        "Operation complete",
    ),
    (
        "step1_backup",
        "STEP 1/4 校验参数和卷身份；备份只读取源卷并写入镜像，不格式化任何卷、不改写启动引导",
        "STEP 1/4 Validating parameters and volume identity; backup only reads the \
         source volume and writes the image - no volume is formatted, boot entries are untouched",
    ),
    (
        "step1_restore",
        "STEP 1/4 校验参数和卷身份；还原到另一 Windows 卷时会格式化目标卷并写入引导，\
         还原完成后默认启动项指向本次还原的系统（原系统仍保留在菜单中）",
        "STEP 1/4 Validating parameters and volume identity; restoring to another Windows \
         volume formats the target and writes boot entries, after which the default entry \
         points to the restored system (the original system stays in the boot menu)",
    ),
    (
        "wimlib_result",
        "wimlib 结果：{0}",
        "wimlib result: {0}",
    ),
    (
        "wimlib_start",
        "wimlib 开始（覆盖式应用，还原安全描述符）：{0} {1}",
        "wimlib start (overlay apply, restoring security descriptors): {0} {1}",
    ),
    (
        "wait_ms",
        "{0} 毫秒",
        "{0} ms",
    ),
    (
        "wait_unlimited",
        "不限制",
        "unlimited",
    ),
];

/// 按指定语言取文案（**纯函数**）。
///
/// 抽成纯函数有两个好处：① 单测可以逐个语言断言，不必改进程级全局
/// （Rust 单测并行执行，改全局会污染其它用例，属 flaky 源头）；
/// ② 将来若要"同一条日志同时写双语"，可直接复用。
pub(crate) fn msg_in(language: MessageLanguage, key: &str) -> &'static str {
    for (entry_key, chinese, english) in MESSAGES {
        if *entry_key == key {
            return match language {
                MessageLanguage::Chinese => chinese,
                MessageLanguage::English => english,
            };
        }
    }
    // 缺失必须"看得见"：返回一个显眼的哨兵，而不是空串或错误语言。
    // （不能返回 `key` —— 它是 `&str` 而非 `&'static str`。key 都是编译期
    //  常量，真漏了会由 `every_key_has_both_languages` 在开发期挡住。）
    MISSING
}

/// 按**当前**语言取文案（进程级状态 → 纯函数）。
pub(crate) fn msg(key: &str) -> &'static str {
    msg_in(get(), key)
}

/// 带占位的文案：`{0}`、`{1}`… 按顺序替换（**纯函数**，理由同 `msg_in`）。
/// 用于"文案里夹着数字/路径/退出码"的日志行（如 `DISM 结果：退出码 0`）。
pub(crate) fn msg_tpl_in(language: MessageLanguage, key: &str, args: &[&str]) -> String {
    fill_placeholders(msg_in(language, key), args)
}

/// 单遍填空：`{n}` 按 `args[n]` 替换。
///
/// ★ **不能写成 `for i { text = text.replace("{i}", args[i]) }`**——那是多遍
/// 替换，前一轮填进去的**参数内容**会在后一轮被当成占位符再替换一次
/// （2026-10-25 实测过的真实缺陷）：路径 `C:\a{1}b.log` 作为 `{0}` 填入后，
/// 第二轮会把里面的 `{1}` 换成第二个参数，用户看到的是被篡改过的路径。
/// 这类 bug 只在"参数值恰好含 `{数字}`"时出现，靠人工测试几乎撞不到。
///
/// 未匹配的占位符原样保留（宁可显示 `{7}` 也不静默吞掉，便于发现漏传参数）。
pub(crate) fn fill_placeholders(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let key = &after[..close];
                match key.parse::<usize>().ok().and_then(|n| args.get(n)) {
                    Some(value) => out.push_str(value),
                    // 非数字占位（如 JSON 片段 `{"a":1}`）或越界索引：原样保留。
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                // 没有闭合的 `{`，剩下的全是字面量。
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

pub(crate) fn msg_tpl(key: &str, args: &[&str]) -> String {
    msg_tpl_in(get(), key, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 注意：这些用例**一律走纯函数 `msg_in`，不改进程级全局**。
    // `set`/`get` 是全局状态，Rust 单测并行执行，改它会污染同时运行的
    // 其它用例（例如 `online_operation` 里断言中文措辞的那条）。
    #[test]
    fn chinese_text_matches_the_historical_wording() {
        // 默认语言行为不变：与改造前的中文文案逐字一致，防止"翻译时手滑改了中文"。
        let text = msg_in(MessageLanguage::Chinese, "online_done");
        assert_eq!(text, "操作完成：命令、回读和副档簿记均已通过");
        assert!(msg_in(MessageLanguage::Chinese, "step1_backup").contains("不格式化"));
    }

    #[test]
    fn english_text_is_ascii_and_readable() {
        assert!(msg_in(MessageLanguage::English, "online_done").starts_with("Operation complete"));
        assert!(msg_in(MessageLanguage::English, "online_failed_prefix").starts_with("[ERROR] Online"));
        // 英文条目里不允许混入 CJK（防呆由 `every_key_has_both_languages` 兜底，
        // 这里再验一次最容易被复制粘贴污染的 STEP 文案）。
        let step = msg_in(MessageLanguage::English, "step1_restore");
        assert!(step.starts_with("STEP 1/4"), "STEP 前缀是判据依赖的 ASCII 特征：{step}");
    }

    #[test]
    fn template_placeholders_are_replaced_in_both_languages() {
        // 排障最先看的两条：退出码 / 等待上限。占位替换必须两边都对。
        assert_eq!(
            msg_tpl_in(MessageLanguage::Chinese, "dism_result", &["退出码 0"]),
            "DISM 结果：退出码 0"
        );
        assert_eq!(
            msg_tpl_in(MessageLanguage::English, "dism_result", &["exit code 0"]),
            "DISM result: exit code 0"
        );
        assert_eq!(
            msg_tpl_in(MessageLanguage::Chinese, "wait_ms", &["90"]),
            "90 毫秒"
        );
        assert_eq!(msg_tpl_in(MessageLanguage::English, "wait_ms", &["90"]), "90 ms");
    }

    #[test]
    fn unknown_key_returns_a_visible_sentinel() {
        // 缺失必须"看得见"：返回显眼哨兵，而不是空串或错误语言。
        assert_eq!(msg_in(MessageLanguage::Chinese, "no_such_key"), MISSING);
        assert_eq!(msg_in(MessageLanguage::English, "no_such_key"), MISSING);
    }

    #[test]
    fn settings_file_language_is_parsed() {
        // 界面选过 English → CLI 也跟着英文（此前 CLI 完全不理界面偏好）。
        assert_eq!(
            parse_settings_language("# comment\nui_language=en\n"),
            Some(MessageLanguage::English)
        );
        assert_eq!(
            parse_settings_language("failure_threshold=5\r\nui_language=zh\r\n"),
            Some(MessageLanguage::Chinese)
        );
        // 全称也认
        assert_eq!(
            parse_settings_language("ui_language=English"),
            Some(MessageLanguage::English)
        );
        // 值不认识 → None（保持默认中文，绝不瞎猜）
        assert_eq!(parse_settings_language("ui_language=fr"), None);
        // 没有这个键 → None
        assert_eq!(parse_settings_language("failure_threshold=5"), None);
        // 注释里出现同名字样不算数（否则注释能改行为）
        assert_eq!(parse_settings_language("# ui_language=en"), None);
    }

    #[test]
    fn settings_language_ignores_non_language_keys() {
        // dism_timeout_secs 之类的行不能被误读成语言。
        let text = "dism_timeout_secs=300\nfailure_action=pause\nui_language=en\n";
        assert_eq!(parse_settings_language(text), Some(MessageLanguage::English));
    }

    #[test]
    fn placeholders_are_filled_in_one_pass() {
        // 参数内容里的 `{1}` 不能被当成占位符二次替换。
        // 多遍 replace 的写法在这里会得到 "C:\aBANANA b.log"，路径被篡改。
        let text = fill_placeholders("[{0}] {1}", &["C:\\a{1}b.log", "BANANA"]);
        assert_eq!(text, "[C:\\a{1}b.log] BANANA");
    }

    #[test]
    fn unknown_or_out_of_range_placeholders_survive() {
        // 非数字占位（JSON 片段）与越界索引都原样保留——宁可露出 `{7}`，
        // 也不要静默吞掉，否则漏传参数只能靠肉眼发现。
        assert_eq!(fill_placeholders("{\"a\":1}", &[]), "{\"a\":1}");
        assert_eq!(fill_placeholders("{0} {7}", &["x"]), "x {7}");
        assert_eq!(fill_placeholders("no close {", &[]), "no close {");
    }

    #[test]
    fn every_key_has_both_languages() {
        // 防呆：表里不允许出现"只有中文没有英文"或两边一样的半吊子条目。
        for (key, chinese, english) in MESSAGES {
            assert!(!chinese.trim().is_empty(), "key={key} 缺中文");
            assert!(!english.trim().is_empty(), "key={key} 缺英文");
            assert_ne!(chinese, english, "key={key} 中英文一样，多半是漏翻");
            assert!(
                !english.chars().any(|c| c as u32 > 0x2FFF),
                "key={key} 的英文条目里混了 CJK：{english}"
            );
        }
    }
}
