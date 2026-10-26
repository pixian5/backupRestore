//! 「最近任务状态」回执：让图形界面真正把 `status.json` 读给用户看。
//!
//! ## 为什么需要这个模块
//!
//! 恢复环境（PE / WinRE）里失败的任务，回到 Windows 后界面上一无所知：
//! `refresh_task_status` 只解析 `last-task.json` 里的 `taskId` / `operation` /
//! `taskRoot` / `prepareLog` / `recoveryLog` 五个字段，**一次都没读过
//! `status.json`**——而 `status.json` 才是写 `stage` / `error` 的地方。
//! `last-task.json` 里其实早就备好了 `statusJson` 绝对路径，只是没有消费方。
//!
//! 后果：任务在恢复环境里失败后，界面仍只报「任务目录 / 日志路径」，
//! 不显示「失败」二字、不显示失败原因。用户只能自己翻日志。
//!
//! ## 三条硬规则（判据纪律在本模块的具体化）
//!
//! 1. **「读不到」与「读出来是失败」必须分开表述。** 文件不存在、读不出来、
//!    解析失败、解析成功但阶段为 Failed，是四件不同的事，界面文案不能混成一句
//!    「状态不可用」——否则排查时无法定位是「没写」还是「写坏了」。
//! 2. **失败但 `error` 为空 = 程序缺陷**，要明确说出来（这条信息本身有价值），
//!    不能静默留空让用户以为「没事」。★ 这不是假想：实机 `H:\brwork\tasks` 里
//!    就存在 `stage=failed` 且 `error=""` 的任务。
//! 3. **只要 `error` 有值就展示，不过滤阶段。** `StatusRecord.error` 只在两条
//!    失败路径上写入（`write_failure*` 与 `write_recovery_error*`），创建任务时
//!    恒为 `None`；按阶段过滤反而会把恢复环境里的**可重试失败**（stage 仍是
//!    `capturing` 等，还要续跑）藏起来——那正是最需要看到的一类。
//!
//! ## ★ 为什么这里直接反序列化成 `StatusRecord`
//!
//! 第一版我手写了 `serde_json::Value` 取字段，键名写成了 snake_case（`task_id`
//! / `error_code`），阶段写成了 `Failed` / `Capturing`。**全错**：
//! `StatusRecord` 带 `#[serde(rename_all = "camelCase")]`（落盘是 `taskId` /
//! `errorCode`），`Stage` 带 `rename_all = "kebab-case"`（落盘是 `failed` /
//! `capturing`）。是 core 单测报「unknown variant `Failed`」才暴露的。
//! 改成用类型解析后，**键名与枚举只有一处真源**，这类漂移不可能再发生。
//!
//! ★ 本模块**无条件编译**（纯 JSON 与字符串逻辑，不碰 Win32 句柄），
//! 因此本机 `cargo test` 能覆盖全部分支——`native_gui.rs` 里的部分做不到。

use crate::gui_text::Language;
use backuprestore_core::{Stage, StatusRecord};

/// 稳定错误码的起始标记（与 `failure_receipt::with_code` 同一约定）。
const CODE_PREFIX: &str = "BR-";

/// 从任意文案里抽出第一个 `BR-域-序号`（返回**不含方括号**的码本身）。
///
/// 用途：判断「这条失败到底有没有可查的码」——旧版 status.json 只有 error
/// 文案里可能带码（`Coded` 的 `Display` 自带前缀）。
pub(crate) fn extract_code(text: &str) -> Option<&str> {
    let start = text.find(CODE_PREFIX)?;
    let rest = &text[start + CODE_PREFIX.len()..];
    let end = rest.find(']').unwrap_or(rest.len());
    let code = &rest[..end];
    // 只认「域-序号」形状：至少还要有一段 `-`，避免把 `BR-` 开头的
    // 自然语言（比如某个卷标）误当成码。
    if code.is_empty() || !code.contains('-') {
        return None;
    }
    Some(&text[start..start + CODE_PREFIX.len() + end])
}

/// `status.json` 的读取结果。**四种结局分开，不许合并成「状态不可用」。**
pub(crate) enum StatusOutcome {
    /// 文件还没生成（任务刚创建、或从未进入恢复环境）。
    Missing,
    /// 文件存在但读不出来（权限、占用、不是 UTF-8）。
    Unreadable(String),
    /// 读出来了但不是合法 JSON（写了一半、被截断、字段类型变了）。
    Malformed(String),
    /// 解析成功。
    Ok(StatusRecord),
}

/// 解析 `status.json` 的原文。
///
/// 直接反序列化成 `StatusRecord`：键名与枚举的 serde 表示只有一处真源，
/// 手写取值会漂移（见模块头「为什么」一节）。
pub(crate) fn parse_status(text: &str) -> Result<StatusRecord, String> {
    serde_json::from_str::<StatusRecord>(text).map_err(|error| error.to_string())
}

/// 把阶段翻成界面语言。**按枚举匹配，不比对字符串**——
/// 字符串比对的写法正是上一版把 `Failed` 当成合法值的原因。
pub(crate) fn stage_label(stage: Stage, language: Language) -> String {
    let label = match (language, stage) {
        (Language::Chinese, Stage::Prepared) => "已创建（尚未请求重启）",
        (Language::Chinese, Stage::BootRequested) => "已请求重启进入恢复环境",
        (Language::Chinese, Stage::RecoveryStarted) => "恢复环境已启动",
        (Language::Chinese, Stage::Preflight) => "恢复环境预检中",
        (Language::Chinese, Stage::Capturing) => "正在捕获镜像",
        (Language::Chinese, Stage::TargetErased) => "目标卷已擦除",
        (Language::Chinese, Stage::ImageApplied) => "镜像已写入",
        (Language::Chinese, Stage::BootRepaired) => "引导已修复",
        (Language::Chinese, Stage::RecoveryComplete) => "恢复环境阶段完成",
        (Language::Chinese, Stage::Success) => "成功",
        (Language::Chinese, Stage::Failed) => "失败",
        (Language::English, Stage::Prepared) => "created (reboot not requested yet)",
        (Language::English, Stage::BootRequested) => "reboot into recovery requested",
        (Language::English, Stage::RecoveryStarted) => "recovery environment started",
        (Language::English, Stage::Preflight) => "recovery preflight",
        (Language::English, Stage::Capturing) => "capturing image",
        (Language::English, Stage::TargetErased) => "target volume erased",
        (Language::English, Stage::ImageApplied) => "image applied",
        (Language::English, Stage::BootRepaired) => "boot repaired",
        (Language::English, Stage::RecoveryComplete) => "recovery stage complete",
        (Language::English, Stage::Success) => "success",
        (Language::English, Stage::Failed) => "failed",
    };
    label.to_string()
}

/// 生成「阶段 / 进度 / 错误码 / 失败原因」这几行。
///
/// ★ 行数有上限：状态框按 9 行设计，最坏情形（5 行任务信息 + 4 行状态）
/// 必须 ≤ 9，否则最后一行会被切掉。故只在有信息量时才输出该行。
pub(crate) fn render_status_lines(status: &StatusRecord, language: Language) -> Vec<String> {
    let mut lines = vec![format!(
        "{}：{}",
        label(language, "stage"),
        stage_label(status.stage, language)
    )];
    // 进度只在「进行中」有意义：0% 与 100% 由阶段本身表达，重复一遍是噪音。
    if status.progress > 0 && status.progress < 100 {
        lines.push(format!("{}：{}%", label(language, "progress"), status.progress));
    }
    if let Some(detail) = status.error.as_deref() {
        // 码的取用顺序：独立字段优先 → 其次从文案里抽（旧版只有文案带码）。
        let code = status
            .error_code_text
            .as_deref()
            .filter(|value| value.starts_with(CODE_PREFIX))
            .map(str::to_owned)
            .or_else(|| extract_code(detail).map(str::to_owned));
        match code {
            Some(code) => lines.push(format!("{}：{}", label(language, "error_code"), code)),
            // 有码字段但形状不对时也走这里：宁可说「没有码」，也不显示一坨垃圾。
            None => lines.push(label(language, "error_code_missing").to_string()),
        }
        if detail.is_empty() {
            // ★ 规则 2：失败却没原因（实机存在 `error: ""`），必须说破。
            lines.push(label(language, "error_missing").to_string());
        } else {
            lines.push(format!("{}：{}", label(language, "error"), detail));
        }
    } else if status.stage == Stage::Failed {
        // 终态失败却没有 error 字段：两件都没有，就两件都说破。
        lines.push(label(language, "error_code_missing").to_string());
        lines.push(label(language, "error_missing").to_string());
    }
    lines
}

/// 状态文件读不出来时的那一行。**四种结局各自一句，绝不合并。**
pub(crate) fn render_outcome_line(outcome: &StatusOutcome, language: Language) -> String {
    match outcome {
        StatusOutcome::Missing => label(language, "status_missing").to_string(),
        StatusOutcome::Unreadable(reason) => {
            format!("{}{reason}", label(language, "status_unreadable"))
        }
        StatusOutcome::Malformed(reason) => {
            format!("{}{reason}", label(language, "status_malformed"))
        }
        StatusOutcome::Ok(_) => String::new(),
    }
}

fn label(language: Language, key: &str) -> &'static str {
    match (language, key) {
        (Language::Chinese, "stage") => "阶段",
        (Language::Chinese, "progress") => "进度",
        (Language::Chinese, "error_code") => "错误码",
        (Language::Chinese, "error") => "失败原因",
        (Language::Chinese, "error_code_missing") => "错误码：无稳定错误码，请按日志定位",
        (Language::Chinese, "error_missing") => "失败原因：失败但未记录原因（程序缺陷，请提交日志）",
        (Language::Chinese, "status_missing") => {
            "状态：status.json 尚不存在（任务可能未进入恢复环境）"
        }
        (Language::Chinese, "status_unreadable") => "状态：status.json 无法读取：",
        (Language::Chinese, "status_malformed") => "状态：status.json 解析失败：",
        (Language::English, "stage") => "Stage",
        (Language::English, "progress") => "Progress",
        (Language::English, "error_code") => "Error code",
        (Language::English, "error") => "Failure reason",
        (Language::English, "error_code_missing") => {
            "Error code: no stable error code; check the logs"
        }
        (Language::English, "error_missing") => {
            "Failure reason: failed but no reason recorded (defect; please send the logs)"
        }
        (Language::English, "status_missing") => {
            "Status: status.json does not exist yet (the task may never have entered recovery)"
        }
        (Language::English, "status_unreadable") => "Status: status.json cannot be read: ",
        (Language::English, "status_malformed") => "Status: status.json parse failed: ",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use backuprestore_core::Operation;
    use chrono::Utc;

    /// 造一条 status.json 记录。字段与实机一致（camelCase / kebab-case）。
    fn record(stage: Stage, progress: u8, error: Option<&str>, code: Option<&str>) -> StatusRecord {
        StatusRecord {
            task_id: "0f7f2b1e-1111-2222-3333-444455556666".to_string(),
            operation: Operation::Backup,
            stage,
            progress,
            updated: Utc::now(),
            error_code: Some(1),
            error_code_text: code.map(str::to_owned),
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn failed_status_reports_code_and_reason() {
        let status = record(Stage::Failed, 0, Some("镜像不存在"), Some("BR-PE-039"));
        let lines = render_status_lines(&status, Language::Chinese);
        // 阶段 + 错误码 + 失败原因 = 3 行；进度 0% 不输出。
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[1].contains("BR-PE-039"), "{lines:?}");
        assert!(lines[2].contains("镜像不存在"), "{lines:?}");
    }

    #[test]
    fn in_progress_status_shows_percent_only() {
        let status = record(Stage::Capturing, 42, None, None);
        let lines = render_status_lines(&status, Language::Chinese);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[1].contains("42%"), "{lines:?}");
    }

    #[test]
    fn success_status_prints_only_stage() {
        // 100% 不再重复一遍进度；成功态没有 error，故只有一行。
        let status = record(Stage::Success, 100, None, None);
        let lines = render_status_lines(&status, Language::Chinese);
        assert_eq!(lines.len(), 1, "{lines:?}");
    }

    #[test]
    fn retryable_recovery_error_is_still_shown() {
        // ★ 规则 3：恢复环境的可重试失败 stage 仍是 capturing，但 error 有值。
        // 按阶段过滤会把它藏起来，那正是最需要看到的一类。
        let status = record(Stage::Capturing, 30, Some("DISM 超时"), Some("BR-IMG-008"));
        let lines = render_status_lines(&status, Language::Chinese);
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(lines[2].contains("BR-IMG-008"), "{lines:?}");
        assert!(lines[3].contains("DISM 超时"), "{lines:?}");
    }

    #[test]
    fn empty_error_string_is_called_out() {
        // ★ 实机样本：`"stage":"failed","error":""`。空串不等于「没失败」，
        // 也不等于「有原因」——必须显式说「未记录原因」。
        let status = record(Stage::Failed, 0, Some(""), None);
        let lines = render_status_lines(&status, Language::Chinese);
        assert!(lines.iter().any(|line| line.contains("未记录原因")), "{lines:?}");
    }

    #[test]
    fn failed_without_error_field_is_called_out() {
        let status = StatusRecord {
            error: None,
            ..record(Stage::Failed, 0, None, None)
        };
        let lines = render_status_lines(&status, Language::Chinese);
        assert!(lines.iter().any(|line| line.contains("未记录原因")), "{lines:?}");
        assert!(lines.iter().any(|line| line.contains("无稳定错误码")), "{lines:?}");
    }

    #[test]
    fn legacy_code_is_fallback_from_text() {
        // 旧版没有 error_code_text 字段，码只写在 error 文案里 → 仍要能显示。
        let status = record(Stage::Failed, 0, Some("[BR-IO-001] 磁盘已满"), None);
        let lines = render_status_lines(&status, Language::Chinese);
        assert!(lines.iter().any(|line| line.contains("BR-IO-001")), "{lines:?}");
    }

    #[test]
    fn non_br_error_code_text_is_not_shown_as_code() {
        // 防御：字段里是遗留整数字符串时，走「无稳定错误码」而不是显示垃圾。
        let status = record(Stage::Failed, 0, Some("失败"), Some("2"));
        let lines = render_status_lines(&status, Language::Chinese);
        assert!(lines.iter().any(|line| line.contains("无稳定错误码")), "{lines:?}");
    }

    #[test]
    fn parse_status_accepts_real_machine_shape() {
        // ★ 用实机 `H:\brwork\tasks` 里抄来的原文（camelCase + kebab-case）。
        // 手写键名的第一版就是在这里被 core 单测打回的。
        let raw = r#"{
            "taskId": "02138f14-c938-460b-8b1f-d05c72e7a8af",
            "operation": "backup",
            "stage": "failed",
            "progress": 0,
            "updated": "2026-10-22T00:40:59.095332300Z",
            "errorCode": 2,
            "error": ""
        }"#;
        let status = parse_status(raw).expect("实机形状必须能解析");
        assert_eq!(status.stage, Stage::Failed);
        assert_eq!(status.error.as_deref(), Some(""));
        assert_eq!(status.error_code_text, None);
    }

    #[test]
    fn parse_status_writes_kebab_case_stage_back() {
        // 往返一致：序列化出来的阶段仍是 kebab-case（不因本模块而改变契约）。
        let raw = r#"{"taskId":"x","operation":"backup","stage":"target-erased","progress":0,"updated":"2026-10-22T00:40:59Z"}"#;
        let status = parse_status(raw).unwrap();
        assert_eq!(status.stage, Stage::TargetErased);
        assert!(serde_json::to_string(&status).unwrap().contains("target-erased"));
    }

    #[test]
    fn malformed_json_is_distinguished_from_ok() {
        assert!(parse_status("not json").is_err());
        // 阶段写成 PascalCase 是错的——正是第一版的 bug，这里守住它。
        assert!(parse_status(r#"{"taskId":"x","operation":"backup","stage":"Failed","progress":0,"updated":"2026-10-22T00:40:59Z"}"#).is_err());
        assert!(parse_status(r#"{"taskId":"x","operation":"backup","stage":"failed","progress":0,"updated":"2026-10-22T00:40:59Z"}"#).is_ok());
    }

    #[test]
    fn four_outcomes_have_distinct_text() {
        let texts = [
            render_outcome_line(&StatusOutcome::Missing, Language::Chinese),
            render_outcome_line(&StatusOutcome::Unreadable("拒绝访问".into()), Language::Chinese),
            render_outcome_line(&StatusOutcome::Malformed("EOF".into()), Language::Chinese),
        ];
        for pair in texts.windows(2) {
            assert_ne!(pair[0], pair[1], "{pair:?}");
        }
        assert!(!texts[0].is_empty() && !texts[1].is_empty() && !texts[2].is_empty());
    }

    #[test]
    fn extract_code_finds_first_br_code() {
        assert_eq!(extract_code("[BR-PE-039] 镜像不存在"), Some("BR-PE-039"));
        assert_eq!(extract_code("没有码"), None);
        assert_eq!(extract_code("BR-"), None);
    }

    #[test]
    fn english_labels_are_available() {
        let status = record(Stage::Failed, 0, Some("镜像不存在"), Some("BR-PE-039"));
        let lines = render_status_lines(&status, Language::English);
        assert!(lines[0].contains("failed"), "{lines:?}");
        assert!(lines[1].contains("BR-PE-039"), "{lines:?}");
    }

    #[test]
    fn worst_case_fits_the_nine_row_status_box() {
        // 状态框按 9 行设计：5 行任务信息 + 4 行状态。这里守住上限。
        let status = record(Stage::Capturing, 30, Some("DISM 超时"), Some("BR-IMG-008"));
        assert!(render_status_lines(&status, Language::Chinese).len() <= 4);
    }
}
