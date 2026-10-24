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
        .join(crate::failure_policy::SETTINGS_FILE_NAME);
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
    let mut text = msg_in(language, key).to_string();
    for (index, value) in args.iter().enumerate() {
        text = text.replace(&format!("{{{index}}}"), value);
    }
    text
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
