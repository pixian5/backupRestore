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
//! - `pause_max_wait_secs`：**只对 `PauseInRecovery` 生效**——从第一次进入暂停态
//!   起算的**总**等待上限（秒；0 = 不限制 = 历史行为）。见下面「为什么要有它」。
//!
//! ## 为什么 `pause` 需要一个等待上限（2026-10-16 审查，卡死 1）
//!
//! `auto-escape` 有自动逃出兜底，`pause` 刻意没有——弹框是模态的，不点就一直等。
//! 这在「有人值守」语义下是对的，但在**无人值守**场景下等于放弃这台机器：
//! 键盘未就绪、屏幕被切走、VM 没人看，机器就永远停在恢复环境里。
//! `pause_max_wait_secs` 是逃生阀：超时后执行与「是」按钮完全相同的清理并重启。
//!
//! 默认 0（不限制）是**有意的**：把它设成某个固定秒数会把「有人值守但看得久」
//! 的正常用法也杀掉，等于用一个误杀换掉一个挂死。逃生阀交给用户开。
//!
//! 判据与失败次数阈值**解耦**：上限只约束"等多久"，不改变"失败几次才暂停"。
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
/// `pause` 策略的等待上限：0 = 不限制（历史行为）；上限 24 小时。
pub(crate) const DEFAULT_PAUSE_MAX_WAIT_SECS: u32 = 0;
pub(crate) const MAX_PAUSE_MAX_WAIT_SECS: u32 = 24 * 60 * 60;
/// 设置文件名（与程序同目录；程序已在该目录写 `logs/`，无需提权即可写）。
pub(crate) const SETTINGS_FILE_NAME: &str = "BackupRestore.settings.ini";
/// 落进 `RecoveryTask.env` 的键名。
pub(crate) const ENV_THRESHOLD: &str = "FAILURE_THRESHOLD";
pub(crate) const ENV_ACTION: &str = "FAILURE_ACTION";
pub(crate) const ENV_PAUSE_MAX_WAIT: &str = "FAILURE_PAUSE_MAX_WAIT_SECS";

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
                "达到次数后不自动处理：停在恢复环境弹框等你选择——「是」= 立即执行上面那套清理并重启，「否」= 继续重试。可另设最长等待秒数，超时后自动执行与「是」相同的清理。"
            }
            (FailureAction::PauseInRecovery, MessageLanguage::English) => {
                "No automatic action: the recovery environment waits with a prompt — Yes = run the same cleanup and reboot, No = retry again. You can also set a maximum wait; after it expires the same cleanup as \"Yes\" runs automatically."
            }
        }
    }
}

/// 解析 `pause` 等待上限。`Some(0)` = 不限制；`None` = 无法识别。
///
/// 只接受纯 ASCII 数字：`+30`、`30.0`、全角「３」一律拒绝（与
/// `parse_threshold` 同一条理由——静默回退默认会让用户以为设置生效了）。
pub(crate) fn parse_pause_max_wait_secs(text: &str) -> Option<u32> {
    let trimmed = text.trim();
    if !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let value: u32 = trimmed.parse().ok()?;
    (value <= MAX_PAUSE_MAX_WAIT_SECS).then_some(value)
}

/// GUI 文本框用的 `pause` 等待上限解析（错误文案与 CLI 侧同一来源）。
pub(crate) fn parse_pause_max_wait_text(text: &str, language: MessageLanguage) -> Result<u32, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(match language {
            MessageLanguage::Chinese => "请填写等待秒数（0 = 不限制）".to_string(),
            MessageLanguage::English => "Enter the wait in seconds (0 = unlimited)".to_string(),
        });
    }
    parse_pause_max_wait_secs(trimmed).ok_or_else(|| {
        match language {
            MessageLanguage::Chinese => format!(
                "等待秒数必须是 0 到 {MAX_PAUSE_MAX_WAIT_SECS} 之间的整数"
            ),
            MessageLanguage::English => format!(
                "Enter a whole number of seconds between 0 and {MAX_PAUSE_MAX_WAIT_SECS}"
            ),
        }
    })
}

/// 一条完整的失败处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FailurePolicy {
    pub(crate) threshold: u32,
    pub(crate) action: FailureAction,
    /// `pause` 策略的总等待上限（秒；0 = 不限制）。对 `auto-escape` 无意义。
    pub(crate) pause_max_wait_secs: u32,
}

impl Default for FailurePolicy {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            action: FailureAction::default(),
            pause_max_wait_secs: DEFAULT_PAUSE_MAX_WAIT_SECS,
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
        Ok(Self {
            threshold,
            action,
            pause_max_wait_secs: DEFAULT_PAUSE_MAX_WAIT_SECS,
        })
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
        let pause_max_wait_secs = values
            .get(ENV_PAUSE_MAX_WAIT)
            .and_then(|value| parse_pause_max_wait_secs(value))
            .unwrap_or(DEFAULT_PAUSE_MAX_WAIT_SECS);
        Self {
            threshold,
            action,
            pause_max_wait_secs,
        }
    }

    /// 落进 `RecoveryTask.env` 的三行（键, 值）。
    pub(crate) fn env_pairs(&self) -> [(&'static str, String); 3] {
        [
            (ENV_THRESHOLD, self.threshold.to_string()),
            (ENV_ACTION, self.action.env_name().to_string()),
            (
                ENV_PAUSE_MAX_WAIT,
                self.pause_max_wait_secs.to_string(),
            ),
        ]
    }

    /// `pause` 策略是否配有等待上限（0 = 不限制）。
    pub(crate) fn has_pause_wait_limit(&self) -> bool {
        self.action == FailureAction::PauseInRecovery && self.pause_max_wait_secs > 0
    }

    /// 【2026-10-17 修复】暂停总上限**灌进进程环境变量**时应写的秒数。
    ///
    /// 为什么必须有这个函数：`pause_max_wait_secs` 此前只驱动
    /// `MessageBoxTimeoutW`（失败达阈值弹框那条路），而恢复链里**用户点「暂停」
    /// 按钮**后的等待走的是 `recovery_progress::wait_if_paused`，它读环境变量
    /// `PAUSE_TIMEOUT_ENV_VAR` —— 那个变量**全仓没有任何写入方**，等于永久 0。
    /// 结果是：用户在设置里配了上限，对"暂停后没人来点继续"这一路完全不生效。
    /// `has_pause_wait_limit` 只回答"有没有"，这里给出"值是多少"，两者同源。
    // 只在 Windows 的 `recover_env` 里被调用；非 Windows 构建下允许未使用，
    // 避免 `dead_code` 噪音（行为由上面的单测在 macOS 上照常锁定）。
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn pause_timeout_env_value(&self) -> u32 {
        if self.action == FailureAction::PauseInRecovery {
            self.pause_max_wait_secs
        } else {
            // auto-escape 从不进入暂停态；写 0 让读取方保持"不限制"。
            0
        }
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
    ///
    /// `bcd_rolled_back` = 本次运行用户按过 B 回退过 BCD。此时续跑链校验必然失败，
    /// **重试也几乎必然再失败**——2026-10-23 实机撞见用户连点两次重试、每次都在同一处
    /// 失败。把这一点写进正文，让用户带着信息选，而不是替用户决定（是否自动收口为终态
    /// 属于产品取舍，已由用户 2026-10-23 拍板「不自动终态，只把信息说清」）。
    pub(crate) fn retry_prompt(
        &self,
        error: &str,
        consecutive_failures: u32,
        language: MessageLanguage,
        bcd_rolled_back: bool,
    ) -> String {
        let rollback_note = if bcd_rolled_back {
            match language {
                MessageLanguage::Chinese => {
                    "\n⚠ 本次运行你按过「B」回退 BCD：引导链已还原为任务准备期快照，\
                     重试很可能再次失败。若确实想中止，请选「取消」。\n"
                }
                MessageLanguage::English => {
                    "\n⚠ BCD was rolled back with \"B\" in this run: the boot chain is back to \
                     the prepared snapshot, so a retry will most likely fail again. \
                     Choose \"Cancel\" if you really want to stop.\n"
                }
            }
        } else {
            ""
        };
        match language {
            MessageLanguage::Chinese => format!(
                "恢复暂未完成：{error}\n{rollback_note}\n\
                 已保存的任务与恢复材料会保留。排除磁盘、镜像或空间问题后选择“重试”，继续同一任务；\
                 选择“取消”将立即清理本次一次性启动项、把任务标记为终态（现场保留）并重启回正常 Windows。\n\
                 恢复已完成的任务只重试清理，不会再次擦除目标。\n\n\
                 这是第 {consecutive_failures} 次失败（{}）。",
                self.plan_note(language)
            ),
            MessageLanguage::English => format!(
                "Recovery did not finish: {error}\n{rollback_note}\n\
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
    ///
    /// 配了等待上限时把上限也写进正文——用户看到"最多等 N 秒"才知道自己还有
    /// 多少时间，而不是以为会无限等下去（反之也一样）。
    pub(crate) fn pause_prompt(
        &self,
        error: &str,
        consecutive_failures: u32,
        language: MessageLanguage,
    ) -> String {
        let wait_note = self.pause_wait_note(language);
        match language {
            MessageLanguage::Chinese => format!(
                "恢复已连续失败 {consecutive_failures} 次，达到设定的阈值（{} 次）。\n\n\
                 按当前设置，程序不会自动处理，请选择：\n\
                 • 「是」：立即清理本次一次性启动项、把任务标记为终态（现场保留），并重启回正常 Windows——\
                 与「自动清理」模式做的事完全相同。\n\
                 • 「否」：继续重试同一任务。\n\n\
                 最近一次失败：{error}\n\n\
                 {wait_note}\
                 （可在 Windows 端程序的「设置 → 恢复环境失败处理」里改为「自动清理并重启」。）",
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
                 {wait_note}\
                 (Change this under Settings -> Recovery failure handling.)",
                self.threshold
            ),
        }
    }

    /// 弹框里描述等待上限的那一句。无上限时说"会一直等"——**必须如实**，
    /// 不能让用户以为有兜底而实际没有（2026-10-16 审查，卡死 1）。
    fn pause_wait_note(&self, language: MessageLanguage) -> String {
        match (self.pause_max_wait_secs, language) {
            (0, MessageLanguage::Chinese) => {
                "本弹框不点就一直等，方便你先排查（未设置最长等待时间）。\n".to_string()
            }
            (0, MessageLanguage::English) => {
                "This prompt waits indefinitely so you can inspect first (no maximum wait set).\n"
                    .to_string()
            }
            (secs, MessageLanguage::Chinese) => format!(
                "本弹框最长等待 {secs} 秒（从第一次暂停起算）；超时将自动执行与「是」相同的清理并重启回正常 Windows。\n"
            ),
            (secs, MessageLanguage::English) => format!(
                "This prompt waits at most {secs} seconds (counted from the first pause); after that it runs the same cleanup as \"Yes\" and reboots to Windows.\n"
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
            (FailureAction::PauseInRecovery, MessageLanguage::Chinese) => {
                if self.pause_max_wait_secs == 0 {
                    format!("累计 {} 次后会暂停并等你手动选择是否清理（不设上限）", self.threshold)
                } else {
                    format!(
                        "累计 {} 次后会暂停并等你手动选择是否清理（最多等 {} 秒后自动清理）",
                        self.threshold, self.pause_max_wait_secs
                    )
                }
            }
            (FailureAction::PauseInRecovery, MessageLanguage::English) => {
                if self.pause_max_wait_secs == 0 {
                    format!(
                        "after {} failures it will pause and wait for you to decide (no time limit)",
                        self.threshold
                    )
                } else {
                    format!(
                        "after {} failures it will pause and wait for you to decide (auto-cleanup after {} seconds)",
                        self.threshold, self.pause_max_wait_secs
                    )
                }
            }
        }
    }

    /// 恢复环境日志里记一行「本次用的是哪套策略」，便于事后取证。
    pub(crate) fn log_line(&self, consecutive_failures: u32) -> String {
        format!(
            "failure policy: threshold={} action={} pause_max_wait_secs={} consecutive_failures={consecutive_failures}",
            self.threshold,
            self.action.env_name(),
            self.pause_max_wait_secs
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
                SETTINGS_PAUSE_MAX_WAIT_KEY => {
                    values.insert(ENV_PAUSE_MAX_WAIT.to_string(), value);
                }
                _ => {}
            }
        }
        Self::from_values(&values)
    }

    /// 写设置（**合并**，只替换自己这两个键，其余键原样保留）。
    ///
    /// 为什么必须是合并而不是整文件重写：设置文件是**共享**的。
    /// 2026-10-16 起 `dism_timeout` 也往同一个 `BackupRestore.settings.ini` 里
    /// 写 `dism_timeout_secs`。任何一处整文件重写都会把另一处**静默**冲掉——
    /// 用户改一次失败次数，DISM 上限就悄悄回到默认（不限制），而两个键的语义
    /// 完全不相干，这种丢失事后极难归因。合并写把风险从"设计上必然发生"
    /// 降到"只丢自己管的那两个键"。
    ///
    /// 目录不存在时自动创建。
    pub(crate) fn save_to_dir(&self, executable_dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(executable_dir)?;
        let path = Self::settings_path(executable_dir);
        let existing = fs::read_to_string(&path).unwrap_or_default();
        let mut kept: Vec<&str> = existing
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                // 【2026-10-18 缺陷修复 L7】注释行必须**先**整行排除。
                // 此前只按 `split_once('=')` 取 key 过滤，而三条注释里都含 `=`
                // （`1..=20`、「 = 」、`（0 = …）`）→ 注释行被当成普通行留下，
                // 下面又 `kept.extend` 追加三行新注释：每次保存多 3 行，
                // 设置文件线性膨胀。
                if trimmed.starts_with('#') || trimmed.starts_with(';') {
                    return false;
                }
                let key = trimmed.split_once('=').map(|(key, _)| key.trim());
                key != Some(SETTINGS_THRESHOLD_KEY)
                    && key != Some(SETTINGS_ACTION_KEY)
                    && key != Some(SETTINGS_PAUSE_MAX_WAIT_KEY)
            })
            .collect();
        if kept.is_empty() {
            kept.push("# BackupRestore 程序设置（本文件由程序自动生成，可手工编辑）");
        }
        // 先各自成行再 push：`&format!(...)` 的临时 String 会随语句结束而失效，
        // 不能直接塞进 `Vec<&str>`。
        let threshold_comment = format!(
            "# failure_threshold: 恢复环境连续失败多少次后触发处理（{}..={}）",
            MIN_THRESHOLD, MAX_THRESHOLD
        );
        let action_comment = "# failure_action: auto-escape = 自动清理并重启回 Windows；pause = 暂停在恢复环境等用户手动处理".to_string();
        let pause_wait_comment = "# failure_pause_max_wait_secs: pause 策略从第一次暂停起算的总等待秒数（0 = 不限制；超时后自动清理并重启）".to_string();
        let threshold_line = format!("{SETTINGS_THRESHOLD_KEY}={}", self.threshold);
        let action_line = format!("{SETTINGS_ACTION_KEY}={}", self.action.env_name());
        let pause_wait_line = format!("{SETTINGS_PAUSE_MAX_WAIT_KEY}={}", self.pause_max_wait_secs);
        kept.extend([
            threshold_comment.as_str(),
            action_comment.as_str(),
            pause_wait_comment.as_str(),
            threshold_line.as_str(),
            action_line.as_str(),
            pause_wait_line.as_str(),
        ]);
        let mut text = kept.join("\n");
        text.push('\n');
        // 【2026-10-18 缺陷修复 L8】原子落盘（tmp + rename）。
        // 此前 `fs::write` 直写：写到一半被杀（断电/强杀）会留下截断的设置文件，
        // `load_from_dir` 只能**静默**退回默认值——连用户显式设过的
        // `dism_timeout_secs` 一起丢，且没有任何提示。
        let tmp = path.with_extension("ini.tmp");
        fs::write(&tmp, text)?;
        match fs::rename(&tmp, &path) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = fs::remove_file(&tmp);
                Err(error)
            }
        }
    }
}

/// 设置文件里的键名（与 `RecoveryTask.env` 的键名分开：一个给用户看，
/// 一个是恢复环境的内部契约）。
pub(crate) const SETTINGS_THRESHOLD_KEY: &str = "failure_threshold";
pub(crate) const SETTINGS_ACTION_KEY: &str = "failure_action";
pub(crate) const SETTINGS_PAUSE_MAX_WAIT_KEY: &str = "failure_pause_max_wait_secs";

/// 【2026-10-17 修复】暂停总上限的进程环境变量名。
///
/// 常量必须放在这里而不是让读取方（`recovery_progress`）自己写字符串字面量：
/// 那正是本 bug 的成因 —— 写入方与读取方各写各的，谁也没对上，
/// 于是"配了上限却不生效"是**静默**的。键名单一来源，改一处即同步。
// 只在 Windows 的 `recover_env` 写入 / `recovery_progress` 读取；非 Windows 构建
// 下没有引用点，允许未使用以免 `dead_code` 噪音。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const PAUSE_TIMEOUT_ENV_VAR: &str = "BACKUPRESTORE_PAUSE_TIMEOUT_SECS";

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
        let policy = FailurePolicy {
            threshold: 5,
            action: FailureAction::PauseInRecovery,
            pause_max_wait_secs: 120,
        };
        let pairs = policy.env_pairs();
        let map = values(&[
            (pairs[0].0, pairs[0].1.as_str()),
            (pairs[1].0, pairs[1].1.as_str()),
            (pairs[2].0, pairs[2].1.as_str()),
        ]);
        assert_eq!(FailurePolicy::from_values(&map), policy);
    }

    #[test]
    fn pause_max_wait_parsing_rejects_junk_instead_of_guessing() {
        assert_eq!(parse_pause_max_wait_secs(" 90 "), Some(90));
        assert_eq!(parse_pause_max_wait_secs("0"), Some(0));
        assert_eq!(
            parse_pause_max_wait_secs(&MAX_PAUSE_MAX_WAIT_SECS.to_string()),
            Some(MAX_PAUSE_MAX_WAIT_SECS)
        );
        for junk in ["", "   ", "+90", "90.0", "９０", "86401", "-1", "abc", "90s"] {
            assert_eq!(
                parse_pause_max_wait_secs(junk),
                None,
                "{junk:?} should be rejected"
            );
        }
        // GUI 文本框解析：空串单独给"请填写"，其余走统一错误。
        assert_eq!(
            parse_pause_max_wait_text("30", MessageLanguage::Chinese),
            Ok(30)
        );
        assert!(parse_pause_max_wait_text("", MessageLanguage::Chinese).is_err());
        assert!(parse_pause_max_wait_text("abc", MessageLanguage::English).is_err());
    }

    #[test]
    fn pause_wait_limit_is_independent_of_the_failure_threshold() {
        // 上限只约束"等多久"，不改变"失败几次才暂停"——两者必须解耦。
        let policy = FailurePolicy {
            threshold: 3,
            action: FailureAction::PauseInRecovery,
            pause_max_wait_secs: 600,
        };
        assert!(policy.pause_now(3));
        assert!(!policy.pause_now(2));
        assert!(policy.has_pause_wait_limit());
        // 0 = 不限制（历史行为），auto-escape 时这个字段无意义。
        let unlimited = FailurePolicy::default();
        assert!(!unlimited.has_pause_wait_limit());
        let auto = FailurePolicy {
            pause_max_wait_secs: 600,
            ..FailurePolicy::default()
        };
        assert!(!auto.has_pause_wait_limit());
    }

    #[test]
    fn pause_timeout_env_value_feeds_the_progress_window_wait() {
        // 【2026-10-17 修复】`wait_if_paused`（用户点「暂停」那条路）读的是进程
        // 环境变量，而它的写入方此前**不存在** → 恒为 0。这里钉死导出值：
        // pause 策略给出配置值，auto-escape 给 0（它从不进入暂停态）。
        let pause = FailurePolicy {
            threshold: 3,
            action: FailureAction::PauseInRecovery,
            pause_max_wait_secs: 600,
        };
        assert_eq!(pause.pause_timeout_env_value(), 600);
        // 配了上限但策略是 auto-escape：暂停态不会来临，导出 0 让读取方不限制，
        // 免得拿一个无关的数字去中断恢复链。
        let auto = FailurePolicy {
            threshold: 3,
            action: FailureAction::AutoEscape,
            pause_max_wait_secs: 600,
        };
        assert_eq!(auto.pause_timeout_env_value(), 0);
        // pause 且未配上限 → 0 = 不限制（默认零回归）。
        let unlimited = FailurePolicy {
            threshold: 3,
            action: FailureAction::PauseInRecovery,
            pause_max_wait_secs: 0,
        };
        assert_eq!(unlimited.pause_timeout_env_value(), 0);
    }

    #[test]
    fn pause_prompt_states_the_real_wait_behaviour() {
        // 无上限时必须如实说"一直等"——不能让用户以为有兜底而实际没有。
        let unlimited = FailurePolicy {
            threshold: 3,
            action: FailureAction::PauseInRecovery,
            pause_max_wait_secs: 0,
        };
        let text = unlimited.pause_prompt("boom", 3, MessageLanguage::Chinese);
        assert!(text.contains("不点就一直等"), "{text}");
        assert!(text.contains("未设置最长等待时间"), "{text}");
        assert!(!text.contains("最长等待 0 秒"), "{text}");

        // 有上限时把秒数写清楚。
        let limited = FailurePolicy {
            threshold: 3,
            action: FailureAction::PauseInRecovery,
            pause_max_wait_secs: 300,
        };
        let text = limited.pause_prompt("boom", 3, MessageLanguage::Chinese);
        assert!(text.contains("最长等待 300 秒"), "{text}");
        assert!(text.contains("从第一次暂停起算"), "{text}");
        let text = limited.pause_prompt("boom", 3, MessageLanguage::English);
        assert!(text.contains("at most 300 seconds"), "{text}");
    }

    #[test]
    fn log_line_records_the_pause_wait_limit_for_forensics() {
        let policy = FailurePolicy {
            threshold: 4,
            action: FailureAction::PauseInRecovery,
            pause_max_wait_secs: 45,
        };
        let line = policy.log_line(2);
        assert!(line.contains("threshold=4"), "{line}");
        assert!(line.contains("action=pause"), "{line}");
        assert!(line.contains("pause_max_wait_secs=45"), "{line}");
        assert!(line.contains("consecutive_failures=2"), "{line}");
    }

    #[test]
    fn settings_round_trip_carries_the_pause_wait_limit() {
        let dir = std::env::temp_dir().join(format!(
            "backuprestore-failure-policy-wait-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let policy = FailurePolicy {
            threshold: 9,
            action: FailureAction::PauseInRecovery,
            pause_max_wait_secs: 777,
        };
        policy.save_to_dir(&dir).expect("save");
        assert_eq!(FailurePolicy::load_from_dir(&dir), policy);
        // 缺文件 → 默认（含 pause_max_wait_secs = 0）
        let missing = dir.join("nope");
        assert_eq!(FailurePolicy::load_from_dir(&missing), FailurePolicy::default());
        // 坏文件 → 默认（不能因为设置坏了让 GUI 起不来）
        fs::write(FailurePolicy::settings_path(&dir), "garbage\n===\n").expect("write");
        assert_eq!(FailurePolicy::load_from_dir(&dir), FailurePolicy::default());
        // 只写两个旧键（历史文件没有第三个）→ 第三个回退默认，不报错。
        fs::write(
            FailurePolicy::settings_path(&dir),
            "failure_threshold=7\nfailure_action=pause\n",
        )
        .expect("write");
        let legacy = FailurePolicy::load_from_dir(&dir);
        assert_eq!(legacy.threshold, 7);
        assert_eq!(legacy.action, FailureAction::PauseInRecovery);
        assert_eq!(legacy.pause_max_wait_secs, DEFAULT_PAUSE_MAX_WAIT_SECS);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompts_mention_the_configured_numbers_and_buttons() {
        let auto = FailurePolicy::new(4, FailureAction::AutoEscape).expect("valid");
        let text = auto.retry_prompt("boom", 2, MessageLanguage::Chinese, false);
        assert!(text.contains("这是第 2 次失败"), "{text}");
        assert!(text.contains("累计 4 次后会自动清理"), "{text}");
        assert!(!text.contains("暂停"), "{text}");

        let pause = FailurePolicy::new(4, FailureAction::PauseInRecovery).expect("valid");
        let text = pause.retry_prompt("boom", 2, MessageLanguage::Chinese, false);
        assert!(text.contains("累计 4 次后会暂停"), "{text}");

        let pause_prompt = pause.pause_prompt("boom", 4, MessageLanguage::Chinese);
        // 按钮语义必须写清楚：是 = 手动清理，否 = 继续重试。
        assert!(pause_prompt.contains("连续失败 4 次"), "{pause_prompt}");
        assert!(pause_prompt.contains("「是」"), "{pause_prompt}");
        assert!(pause_prompt.contains("「否」"), "{pause_prompt}");
        assert!(pause_prompt.contains("不会"), "{pause_prompt}");
    }

    /// 【2026-10-23 用户拍板】按 B 回退过 BCD 时，重试对话框要说清
    /// 「重试很可能再次失败」——**但不自动替用户终态**，选择权留给用户。
    #[test]
    fn retry_prompt_warns_after_a_bcd_rollback_but_keeps_the_choice() {
        let policy = FailurePolicy::new(4, FailureAction::AutoEscape).expect("valid");
        // 回退过：双语都要带警示，且「取消」按钮的语义仍写明（没有替用户决定）
        for language in [MessageLanguage::Chinese, MessageLanguage::English] {
            let text = policy.retry_prompt("boom", 1, language, true);
            let warned = match language {
                MessageLanguage::Chinese => {
                    text.contains("按过「B」回退 BCD") && text.contains("重试很可能再次失败")
                }
                MessageLanguage::English => {
                    text.contains("rolled back") && text.contains("most likely fail again")
                }
            };
            assert!(warned, "回退过必须警示：{text}");
            assert!(
                text.contains("取消") || text.contains("Cancel"),
                "仍要保留取消这条退路：{text}"
            );
        }
        // 没回退过：一个字都不许多
        let plain = policy.retry_prompt("boom", 1, MessageLanguage::Chinese, false);
        assert!(!plain.contains("重试很可能再次失败"), "{plain}");
        assert!(!plain.contains("按过「B」"), "{plain}");
    }

    #[test]
    fn prompts_carry_no_markdown_markup() {
        // Win32 的 STATIC / MessageBox 不渲染 Markdown，`**粗体**` 会原样显示成
        // 星号（2026-10-16 实机截图抓到：弹框里出现「程序**不会**自动处理」）。
        // 这条护栏锁住「文案里不许出现 Markdown 标记」。
        for language in [MessageLanguage::Chinese, MessageLanguage::English] {
            for action in [FailureAction::AutoEscape, FailureAction::PauseInRecovery] {
                let policy = FailurePolicy {
                    threshold: 3,
                    action,
                    pause_max_wait_secs: DEFAULT_PAUSE_MAX_WAIT_SECS,
                };
                for text in [
                    action.label(language).to_string(),
                    action.detail(language).to_string(),
                    policy.plan_note(language),
                    policy.retry_prompt("e", 1, language, false),
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
                let policy = FailurePolicy {
                    threshold: 3,
                    action,
                    pause_max_wait_secs: DEFAULT_PAUSE_MAX_WAIT_SECS,
                };
                assert!(!policy.plan_note(language).is_empty());
                assert!(!policy.retry_prompt("e", 1, language, false).is_empty());
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
