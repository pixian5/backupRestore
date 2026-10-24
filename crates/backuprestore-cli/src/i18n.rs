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
pub(crate) fn init_from_env() {
    if let Ok(value) = std::env::var("BACKUPRESTORE_LANG") {
        if value.trim().to_ascii_lowercase().starts_with("en") {
            set(MessageLanguage::English);
        }
    }
}

/// 文案缺失时的哨兵。
const MISSING: &str = "<i18n:missing>";

/// 关键文案表：`(key, 中文, 英文)`。新增文案请按字母序插入并补单测。
const MESSAGES: &[(&str, &str, &str)] = &[
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
