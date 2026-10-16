//! 恢复环境「连续失败 N 次后如何处理」策略：判据、文案与设置的**单一来源**。
//!
//! ## 为什么需要这个模块
//!
//! 恢复链在恢复环境里失败后，一次性启动项（`bootsequence`）仍指向恢复环境；
//! 若失败是确定性的，机器会在「失败 → 重启 → 又失败」之间无限打转
//! （2026-10-16 实测：GUI 与 RE 都没给用户自救手段，只能从宿主侧挂 PE 光盘手改 BCD）。
//!
//! v2.6.8 的做法是「连败 3 次直接自动逃出」——阈值硬编码在 `main.rs` 的
//! `AUTO_ESCAPE_THRESHOLD`，行为也只有一种，用户无法选择。本模块把它变成
//! **用户可设置**的策略：
//!
//! - `threshold`：连续失败多少次后触发（`1..=20`，默认 `3`）。
//! - `action`：触发后怎么处理
//!   - [`FailureAction::AutoEscape`]：自动清一次性启动项 + 把任务标成终态
//!     （现场保留）+ 重启回正常 Windows —— **默认，等于 v2.6.8 的历史行为**。
//!   - [`FailureAction::PauseInRecovery`]：**不自动处理**，弹框停在恢复环境里
//!     （弹框是模态的，机器会一直等），由用户按按钮手动执行与 `AutoEscape`
//!     **完全相同**的那套处理。
//!
//! ## 单一来源（P3「文案分叉」前科的护栏）
//!
//! 判据与文案都只在本模块定义，三处接线共用：
//! ① GUI 设置窗口（读写设置）② `windows_prepare` 落盘进 `RecoveryTask.env`
//! ③ 恢复环境 `main.rs::recover_env_interactive` 执行。
//! 任何一处想改文案，都必须改这里——否则就是重演 `native_gui.rs` 曾经
//! 自己复制一份硬编码文案的老问题。
//!
//! 模块**不依赖 Windows**（只有文件读写与字符串处理），因此本机 macOS 上
//! 就能跑单测覆盖全部分支。
use crate::pe_safety::MessageLanguage;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// 连续失败多少次后触发。默认 3：与 v2.6.8 的历史行为一致。
pub(crate) const DEFAULT_THRESHOLD: u32 = 3;
/// 阈值下限：1 = 第一次失败就触发。
pub(crate) const MIN_THRESHOLD: u32 = 1;
/// 阈值上限。上限的意义是防呆：用户输入 100000 等于关闭该保护，不如明确拒绝。
pub(crate) const MAX_THRESHOLD: u32 = 20;
/// 设置文件名（与程序同目录；程序已在该目录写 `logs/`，无需提权即可写）。
pub(crate) const SETTINGS_FILE_NAME: &str = "BackupRestore.settings.ini";
/// 落进 `RecoveryTask.env` 的键名。
pub(crate) const ENV_THRESHOLD: &str = "FAILURE_THRESHOLD";
pub(crate) const ENV_ACTION: &str = "FAILURE_ACTION";

/// 触发阈值后的处理方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FailureAction {
    /// 自动清理一次性启动项、把任务标成终态（现场保留）并重启回正常 Windows。
    #[default]
    AutoEscape,
    /// 不自动处理：弹框停在恢复环境，等用户按按钮手动执行同一套处理。
    PauseInRecovery,
}

impl FailureAction {
    /// 写进 `RecoveryTask.env` / 设置文件的值（ASCII，语言无关）。
    pub(crate) fn env_name(self) -> &'static str {
        match self {
            FailureAction::AutoEscape => "auto-escape",
            FailureAction::PauseInRecovery => "pause",
        }
    }

    /// 容错解析：接受几种常见写法与大小写差异；无法识别时返回 `None`，
    /// 由调用方退回默认值（绝不因为一个坏值让恢复环境起不来）。
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let normalized: String = value
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|ch| *ch != '-' && *ch != '_' && !ch.is_whitespace())
            .collect();
        match normalized.as_str() {
            "autoescape" | "auto" | "escape" | "automatic" => Some(FailureAction::AutoEscape),
            "pause" | "pauseinrecovery" | "hold" | "manual" | "wait" => {
                Some(FailureAction::PauseInRecovery)
            }
            _ => None,
        }
    }

    /// 设置窗口里的单选标签。
    pub(crate) fn label(self, language: MessageLanguage) -> &'static str {
        match (self, language) {
            (FailureAction::AutoEscape, MessageLanguage::Chinese) => {
                "自动清理并重启回正常 Windows（推荐）"
            }
            (FailureAction::AutoEscape, MessageLanguage::English) => {
                "Clean up and reboot to Windows automatically (recommended)"
            }
            (FailureAction::PauseInRecovery, MessageLanguage::Chinese) => {
                "暂停在恢复环境，由我按按钮手动清理"
            }
            (FailureAction::PauseInRecovery, MessageLanguage::English) => {
                "Pause in the recovery environment and let me clean up manually"
            }
        }
    }

    /// 设置窗口里的说明文字（解释两种行为各自做什么）。
    pub(crate) fn detail(self, language: MessageLanguage) -> &'static str {
        match (self, language) {
            (FailureAction::AutoEscape, MessageLanguage::Chinese) => {
                "达到次数后不再询问：立即清理本次一次性启动项、把任务标记为终态（现场保留），并重启回正常 Windows。"
            }
            (FailureAction::AutoEscape, MessageLanguage::English) => {
                "No prompt after the limit: disarm the one-shot boot entry, mark the task terminal (evidence kept), and reboot to Windows."
            }
            (FailureAction::PauseInRecovery, MessageLanguage::Chinese) => {
                "达到次数后不自动处理：停在恢复环境弹框等你选择——「是」= 立即执行上面那套清理并重启，「否」= 继续重试。弹框不点就一直等，方便你先排查。"
            }
            (FailureAction::PauseInRecovery, MessageLanguage::English) => {
                "No automatic action: the recovery environment waits with a prompt — Yes = run the same cleanup and reboot, No = retry again. It waits indefinitely so you can inspect first."
            }
        }
    }
}

/// 一条完整的失败处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FailurePolicy {
    pub(crate) threshold: u32,
    pub(crate) action: FailureAction,
}

impl Default for FailurePolicy {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            action: FailureAction::default(),
        }
    }
}

impl FailurePolicy {
    /// 构造并校验阈值范围。`Err` 里是给用户看的中文原因。
    pub(crate) fn new(threshold: u32, action: FailureAction) -> Result<Self, String> {
        if !(MIN_THRESHOLD..=MAX_THRESHOLD).contains(&threshold) {
            return Err(format!(
                "失败次数必须在 {MIN_THRESHOLD} 到 {MAX_THRESHOLD} 之间"
            ));
        }
        Ok(Self { threshold, action })
    }

    /// 解析用户在文本框里输入的次数。空串按「未填写」拒绝（而不是静默当成 0）。
    pub(crate) fn parse_threshold(text: &str, language: MessageLanguage) -> Result<u32, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(match language {
                MessageLanguage::Chinese => "请填写失败次数".to_string(),
                MessageLanguage::English => "Enter the number of failures".to_string(),
            });
        }
        // 只接受纯 ASCII 数字：`+3`、`3.0`、全角「３」一律拒绝，
        // 免得用户以为设置生效了而实际落回默认值。
        if !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(match language {
                MessageLanguage::Chinese => {
                    format!("失败次数必须是 {MIN_THRESHOLD} 到 {MAX_THRESHOLD} 之间的整数")
                }
                MessageLanguage::English => format!(
                    "Enter a whole number between {MIN_THRESHOLD} and {MAX_THRESHOLD}"
                ),
            });
        }
        let value: u32 = trimmed
            .parse()
            .map_err(|_| match language {
                MessageLanguage::Chinese => {
                    format!("失败次数必须是 {MIN_THRESHOLD} 到 {MAX_THRESHOLD} 之间的整数")
                }
                MessageLanguage::English => format!(
                    "Enter a whole number between {MIN_THRESHOLD} and {MAX_THRESHOLD}"
                ),
            })?;
        Self::new(value, FailureAction::default()).map(|policy| policy.threshold)
    }

    /// 从 `RecoveryTask.env` 的键值表读取策略。任何缺失或非法值都**静默回退**
    /// 到默认（恢复环境必须能起来；策略读坏了也不该阻断恢复）。
    pub(crate) fn from_values(values: &BTreeMap<String, String>) -> Self {
        let threshold = values
            .get(ENV_THRESHOLD)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|value| (MIN_THRESHOLD..=MAX_THRESHOLD).contains(value))
            .unwrap_or(DEFAULT_THRESHOLD);
        let action = values
            .get(ENV_ACTION)
            .and_then(|value| FailureAction::parse(value))
            .unwrap_or_default();
        Self { threshold, action }
    }

    /// 落进 `RecoveryTask.env` 的两行（键, 值）。
    pub(crate) fn env_pairs(&self) -> [(&'static str, String); 2] {
        [
            (ENV_THRESHOLD, self.threshold.to_string()),
            (ENV_ACTION, self.action.env_name().to_string()),
        ]
    }

    /// 是否已达到触发次数（两种行为共用这个判据）。
    pub(crate) fn reached(&self, consecutive_failures: u32) -> bool {
        consecutive_failures >= self.threshold
    }

    /// 达到触发次数、且策略是「自动逃出」→ 立即自动处理，不弹框。
    pub(crate) fn auto_escape_now(&self, consecutive_failures: u32) -> bool {
        self.action == FailureAction::AutoEscape && self.reached(consecutive_failures)
    }

    /// 达到触发次数、且策略是「暂停」→ 弹框等用户手动处理。
    pub(crate) fn pause_now(&self, consecutive_failures: u32) -> bool {
        self.action == FailureAction::PauseInRecovery && self.reached(consecutive_failures)
    }

    /// 未达阈值时的「重试 / 取消」弹框正文（含当前策略的预告）。
    pub(crate) fn retry_prompt(
        &self,
        error: &str,
        consecutive_failures: u32,
        language: MessageLanguage,
    ) -> String {
        match language {
            MessageLanguage::Chinese => format!(
                "恢复暂未完成：{error}\n\n\
                 已保存的任务与恢复材料会保留。排除磁盘、镜像或空间问题后选择“重试”，继续同一任务；\
                 选择“取消”将立即清理本次一次性启动项、把任务标记为终态（现场保留）并重启回正常 Windows。\n\
                 恢复已完成的任务只重试清理，不会再次擦除目标。\n\n\
                 这是第 {consecutive_failures} 次失败（{}）。",
                self.plan_note(language)
            ),
            MessageLanguage::English => format!(
                "Recovery did not finish: {error}\n\n\
                 The task and its recovery materials are kept. Fix the disk, image or space issue and choose \
                 \"Retry\" to continue the same task; \"Cancel\" immediately disarms the one-shot boot entry, \
                 marks the task terminal (evidence kept) and reboots to Windows.\n\
                 A finished task only retries cleanup and never erases the target again.\n\n\
                 This is failure #{consecutive_failures} ({}).",
                self.plan_note(language)
            ),
        }
    }

    /// 达到阈值且策略为「暂停」时的弹框正文：**明确写清每个按钮做什么**。
    pub(crate) fn pause_prompt(
        &self,
        error: &str,
        consecutive_failures: u32,
        language: MessageLanguage,
    ) -> String {
        match language {
            MessageLanguage::Chinese => format!(
                "恢复已连续失败 {consecutive_failures} 次，达到设定的阈值（{} 次）。\n\n\
                 按当前设置，程序不会自动处理，请选择：\n\
                 • 「是」：立即清理本次一次性启动项、把任务标记为终态（现场保留），并重启回正常 Windows——\
                 与「自动清理」模式做的事完全相同。\n\
                 • 「否」：继续重试同一任务。\n\n\
                 最近一次失败：{error}\n\n\
                 （可在 Windows 端程序的「设置 → 恢复环境失败处理」里改为「自动清理并重启」。\
                 本弹框不点就一直等，方便你先排查。）",
                self.threshold
            ),
            MessageLanguage::English => format!(
                "Recovery failed {consecutive_failures} times in a row, reaching the configured limit \
                 ({}).\n\n\
                 The current setting does not act automatically. Choose:\n\
                 • Yes: disarm the one-shot boot entry now, mark the task terminal (evidence kept) and \
                 reboot to Windows — exactly what the automatic mode does.\n\
                 • No: retry the same task.\n\n\
                 Latest failure: {error}\n\n\
                 (Change this under Settings -> Recovery failure handling. This prompt waits indefinitely \
                 so you can inspect first.)",
                self.threshold
            ),
        }
    }

    /// 弹框标题（两种行为共用一个标题，避免多份文案漂移）。
    pub(crate) fn prompt_title(language: MessageLanguage) -> &'static str {
        match language {
            MessageLanguage::Chinese => "BackupRestore 恢复需要处理\0",
            MessageLanguage::English => "BackupRestore recovery needs attention\0",
        }
    }

    /// 弹框尾部的「策略预告」一句话。
    pub(crate) fn plan_note(&self, language: MessageLanguage) -> String {
        match (self.action, language) {
            (FailureAction::AutoEscape, MessageLanguage::Chinese) => {
                format!("累计 {} 次后会自动清理并重启回正常系统", self.threshold)
            }
            (FailureAction::AutoEscape, MessageLanguage::English) => format!(
                "after {} failures it will clean up and reboot to Windows automatically",
                self.threshold
            ),
            (FailureAction::PauseInRecovery, MessageLanguage::Chinese) => format!(
                "累计 {} 次后会暂停并等你手动选择是否清理",
                self.threshold
            ),
            (FailureAction::PauseInRecovery, MessageLanguage::English) => format!(
                "after {} failures it will pause and wait for you to decide",
                self.threshold
            ),
        }
    }

    /// 恢复环境日志里记一行「本次用的是哪套策略」，便于事后取证。
    pub(crate) fn log_line(&self, consecutive_failures: u32) -> String {
        format!(
            "failure policy: threshold={} action={} consecutive_failures={consecutive_failures}",
            self.threshold,
            self.action.env_name()
        )
    }

    /// 设置文件路径（程序同目录）。
    pub(crate) fn settings_path(executable_dir: &Path) -> PathBuf {
        executable_dir.join(SETTINGS_FILE_NAME)
    }

    /// 读设置。文件不存在 / 读失败 / 值非法 → 默认值（GUI 必须能起来）。
    ///
    /// 注意键名映射：设置文件用**小写**键（`failure_threshold`，给人看的），
    /// `from_values` 用**大写**键（`FAILURE_THRESHOLD`，`RecoveryTask.env` 的内部
    /// 契约）。这里显式映射一次，否则「设置里改了、恢复环境却不生效」这种静默
    /// 失败会一直藏着（本模块的单测就抓到过这个 bug）。
    pub(crate) fn load_from_dir(executable_dir: &Path) -> Self {
        let Ok(text) = fs::read_to_string(Self::settings_path(executable_dir)) else {
            return Self::default();
        };
        let mut values: BTreeMap<String, String> = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().to_string();
            match key.trim() {
                SETTINGS_THRESHOLD_KEY => {
                    values.insert(ENV_THRESHOLD.to_string(), value);
                }
                SETTINGS_ACTION_KEY => {
                    values.insert(ENV_ACTION.to_string(), value);
                }
                _ => {}
            }
        }
        Self::from_values(&values)
    }

    /// 写设置（覆盖）。目录不存在时自动创建。
    pub(crate) fn save_to_dir(&self, executable_dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(executable_dir)?;
        let text = format!(
            "# BackupRestore 程序设置（本文件由程序自动生成，可手工编辑）\n\
             # failure_threshold: 恢复环境连续失败多少次后触发处理（{}..={}）\n\
             # failure_action: auto-escape = 自动清理并重启回 Windows；pause = 暂停在恢复环境等用户手动处理\n\
             failure_threshold={}\n\
             failure_action={}\n",
            MIN_THRESHOLD,
            MAX_THRESHOLD,
            self.threshold,
            self.action.env_name()
        );
        fs::write(Self::settings_path(executable_dir), text)
    }
}

/// 设置文件里的键名（与 `RecoveryTask.env` 的键名分开：一个给用户看，
/// 一个是恢复环境的内部契约）。
pub(crate) const SETTINGS_THRESHOLD_KEY: &str = "failure_threshold";
pub(crate) const SETTINGS_ACTION_KEY: &str = "failure_action";

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn default_matches_historical_behavior() {
        // 默认必须等于 v2.6.8 的历史行为：3 次 + 自动逃出。
        let policy = FailurePolicy::default();
        assert_eq!(policy.threshold, 3);
        assert_eq!(policy.action, FailureAction::AutoEscape);
        assert!(!policy.auto_escape_now(2));
        assert!(policy.auto_escape_now(3));
        assert!(policy.auto_escape_now(4));
    }

    #[test]
    fn threshold_range_is_enforced() {
        assert!(FailurePolicy::new(1, FailureAction::AutoEscape).is_ok());
        assert!(FailurePolicy::new(MAX_THRESHOLD, FailureAction::AutoEscape).is_ok());
        assert!(FailurePolicy::new(0, FailureAction::AutoEscape).is_err());
        assert!(FailurePolicy::new(MAX_THRESHOLD + 1, FailureAction::AutoEscape).is_err());
    }

    #[test]
    fn threshold_text_parser_rejects_junk_instead_of_guessing() {
        assert_eq!(
            FailurePolicy::parse_threshold(" 7 ", MessageLanguage::Chinese),
            Ok(7)
        );
        // 空串、正号、小数、全角数字、超范围一律拒绝——静默回退默认会让
        // 用户以为设置生效了，是比报错更坏的结果。
        for junk in ["", "   ", "+3", "3.0", "３", "0", "21", "abc"] {
            assert!(
                FailurePolicy::parse_threshold(junk, MessageLanguage::Chinese).is_err(),
                "{junk:?} should be rejected"
            );
        }
    }

    #[test]
    fn action_parser_is_lenient_but_not_wild() {
        for value in ["auto-escape", "AUTO_ESCAPE", "autoescape", " Escape "] {
            assert_eq!(FailureAction::parse(value), Some(FailureAction::AutoEscape));
        }
        for value in ["pause", "PAUSE", "pause-in-recovery", " hold ", "manual"] {
            assert_eq!(
                FailureAction::parse(value),
                Some(FailureAction::PauseInRecovery)
            );
        }
        for value in ["", "nonsense", "pause!"] {
            assert_eq!(FailureAction::parse(value), None);
        }
    }

    #[test]
    fn from_values_falls_back_to_default_on_missing_or_bad_input() {
        // 缺失 → 默认
        assert_eq!(
            FailurePolicy::from_values(&values(&[])),
            FailurePolicy::default()
        );
        // 非法阈值 → 退回默认阈值；合法行为仍生效
        let policy = FailurePolicy::from_values(&values(&[
            (ENV_THRESHOLD, "not-a-number"),
            (ENV_ACTION, "pause"),
        ]));
        assert_eq!(policy.threshold, DEFAULT_THRESHOLD);
        assert_eq!(policy.action, FailureAction::PauseInRecovery);
        // 越界阈值 → 退回默认
        assert_eq!(
            FailurePolicy::from_values(&values(&[(ENV_THRESHOLD, "999")])).threshold,
            DEFAULT_THRESHOLD
        );
        // 非法行为 → 退回默认（自动逃出）
        assert_eq!(
            FailurePolicy::from_values(&values(&[(ENV_ACTION, "??")])).action,
            FailureAction::AutoEscape
        );
    }

    #[test]
    fn pause_policy_never_auto_escapes() {
        let policy = FailurePolicy::new(2, FailureAction::PauseInRecovery).expect("valid");
        assert!(!policy.auto_escape_now(2));
        assert!(!policy.auto_escape_now(99));
        assert!(policy.pause_now(2));
        assert!(policy.pause_now(99));
        assert!(!policy.pause_now(1));
        // 反向：自动逃出策略永不进「暂停」分支。
        let auto = FailurePolicy::new(2, FailureAction::AutoEscape).expect("valid");
        assert!(!auto.pause_now(99));
    }

    #[test]
    fn env_pairs_round_trip() {
        let policy = FailurePolicy::new(5, FailureAction::PauseInRecovery).expect("valid");
        let pairs = policy.env_pairs();
        let map = values(&[
            (pairs[0].0, pairs[0].1.as_str()),
            (pairs[1].0, pairs[1].1.as_str()),
        ]);
        assert_eq!(FailurePolicy::from_values(&map), policy);
    }

    #[test]
    fn prompts_mention_the_configured_numbers_and_buttons() {
        let auto = FailurePolicy::new(4, FailureAction::AutoEscape).expect("valid");
        let text = auto.retry_prompt("boom", 2, MessageLanguage::Chinese);
        assert!(text.contains("这是第 2 次失败"), "{text}");
        assert!(text.contains("累计 4 次后会自动清理"), "{text}");
        assert!(!text.contains("暂停"), "{text}");

        let pause = FailurePolicy::new(4, FailureAction::PauseInRecovery).expect("valid");
        let text = pause.retry_prompt("boom", 2, MessageLanguage::Chinese);
        assert!(text.contains("累计 4 次后会暂停"), "{text}");

        let pause_prompt = pause.pause_prompt("boom", 4, MessageLanguage::Chinese);
        // 按钮语义必须写清楚：是 = 手动清理，否 = 继续重试。
        assert!(pause_prompt.contains("连续失败 4 次"), "{pause_prompt}");
        assert!(pause_prompt.contains("「是」"), "{pause_prompt}");
        assert!(pause_prompt.contains("「否」"), "{pause_prompt}");
        assert!(pause_prompt.contains("不会"), "{pause_prompt}");
    }

    #[test]
    fn prompts_carry_no_markdown_markup() {
        // Win32 的 STATIC / MessageBox 不渲染 Markdown，`**粗体**` 会原样显示成
        // 星号（2026-10-16 实机截图抓到：弹框里出现「程序**不会**自动处理」）。
        // 这条护栏锁住「文案里不许出现 Markdown 标记」。
        for language in [MessageLanguage::Chinese, MessageLanguage::English] {
            for action in [FailureAction::AutoEscape, FailureAction::PauseInRecovery] {
                let policy = FailurePolicy { threshold: 3, action };
                for text in [
                    action.label(language).to_string(),
                    action.detail(language).to_string(),
                    policy.plan_note(language),
                    policy.retry_prompt("e", 1, language),
                    policy.pause_prompt("e", 3, language),
                ] {
                    for marker in ["**", "__", "`", "##"] {
                        assert!(
                            !text.contains(marker),
                            "user-facing text must not contain markdown {marker:?}: {text}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_language_has_non_empty_text_for_every_branch() {
        // 防「加了枚举分支忘了文案」：中英双语全覆盖且都不为空。
        for language in [MessageLanguage::Chinese, MessageLanguage::English] {
            assert!(!FailurePolicy::prompt_title(language).is_empty());
            for action in [FailureAction::AutoEscape, FailureAction::PauseInRecovery] {
                assert!(!action.label(language).is_empty());
                assert!(!action.detail(language).is_empty());
                let policy = FailurePolicy { threshold: 3, action };
                assert!(!policy.plan_note(language).is_empty());
                assert!(!policy.retry_prompt("e", 1, language).is_empty());
                assert!(!policy.pause_prompt("e", 3, language).is_empty());
            }
        }
        // 标题带 NUL 结尾（MessageBoxW 需要）。
        assert!(FailurePolicy::prompt_title(MessageLanguage::Chinese).ends_with('\0'));
    }

    #[test]
    fn log_line_records_the_policy_and_counter_for_forensics() {
        // 这行会写进 RE 里的 X:\BackupRestore-escape.log，是事后判断
        // 「当时用的是哪套策略、连败到第几次」的唯一凭据，格式必须钉住。
        let policy = FailurePolicy::new(4, FailureAction::PauseInRecovery).expect("valid");
        let line = policy.log_line(2);
        assert!(line.contains("threshold=4"), "{line}");
        assert!(line.contains("action=pause"), "{line}");
        assert!(line.contains("consecutive_failures=2"), "{line}");
        assert!(policy.log_line(0).contains("consecutive_failures=0"));
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let dir = std::env::temp_dir().join(format!(
            "backuprestore-failure-policy-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let policy = FailurePolicy::new(9, FailureAction::PauseInRecovery).expect("valid");
        policy.save_to_dir(&dir).expect("save");
        assert_eq!(FailurePolicy::load_from_dir(&dir), policy);
        // 缺文件 → 默认
        let missing = dir.join("nope");
        assert_eq!(FailurePolicy::load_from_dir(&missing), FailurePolicy::default());
        // 坏文件 → 默认（不能因为设置坏了让 GUI 起不来）
        fs::write(FailurePolicy::settings_path(&dir), "garbage\n===\n").expect("write");
        assert_eq!(FailurePolicy::load_from_dir(&dir), FailurePolicy::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_keys_are_mapped_not_guessed() {
        // 设置文件用小写键、env 用大写键。**不能只把文件原样塞进 `from_values`**：
        // 键名不同会让它永远读不到值、静默回退默认（本测试就是为此而写）。
        assert_ne!(SETTINGS_THRESHOLD_KEY, ENV_THRESHOLD);
        assert_ne!(SETTINGS_ACTION_KEY, ENV_ACTION);

        let dir = std::env::temp_dir().join(format!(
            "backuprestore-failure-policy-keys-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        // 手写一份设置文件（模拟用户手工编辑），键名用小写。
        fs::write(
            FailurePolicy::settings_path(&dir),
            "# comment\nfailure_threshold=7\nfailure_action=pause\n",
        )
        .expect("write");
        let policy = FailurePolicy::load_from_dir(&dir);
        assert_eq!(policy.threshold, 7);
        assert_eq!(policy.action, FailureAction::PauseInRecovery);
        // 未识别的键必须被忽略，不能污染解析。
        fs::write(
            FailurePolicy::settings_path(&dir),
            "unknown_key=1\nfailure_threshold=2\nfailure_action=auto-escape\n",
        )
        .expect("write");
        let policy = FailurePolicy::load_from_dir(&dir);
        assert_eq!(policy.threshold, 2);
        assert_eq!(policy.action, FailureAction::AutoEscape);
        let _ = fs::remove_dir_all(&dir);
    }
}
