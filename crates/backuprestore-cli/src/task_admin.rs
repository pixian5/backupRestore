//! 任务与证据管理：把 `<运行目录>\tasks` 下的任务目录读成界面行。
//!
//! **纯逻辑、无条件编译**（不碰任何 Win32 / 文件系统），所以 mac 上能单测全部分支。
//! IO 由调用方负责：本模块只接受「已读出的 JSON 文本 + 目录路径」，不自己开文件。
//!
//! 为什么单独成模块：
//! 1. `native_gui` 是 `cfg(windows)` 专属，写在那里的逻辑在 mac 上是**零信息**（编译器不打开）；
//! 2. 「任务列表怎么排序、坏记录怎么显示、失败原因怎么措辞」都是可测的纯函数，
//!    没必要和窗口过程绑在一起。
//!
//! ★ 读 JSON 一律**反序列化成类型**（`Task` / `StatusRecord`），绝不 `serde_json::Value`
//!   手写键名——`StatusRecord` 是 camelCase、`Stage` 是 kebab-case，v2.6.64 手写键名栽过两次。

use crate::gui_text::Language;
use crate::status_receipt;
use backuprestore_core::{Operation, Stage, StatusRecord, Task};

/// 一个任务的界面表示。
///
/// `status` 直接持有反序列化后的 `StatusRecord`，这样阶段/进度/错误码/失败原因的
/// 措辞可以完全复用 `status_receipt::render_status_lines`——**两处口径必须一致**，
/// 否则「状态框」和「任务详情」会对同一个故障说两种话。
#[derive(Debug, Clone)]
pub(crate) struct TaskRow {
    pub id: String,
    /// 任务目录的绝对路径（界面要显示，用户要能照着去翻）。
    pub dir: String,
    pub operation: Operation,
    pub status: StatusRecord,
}

/// 一条任务记录读不出来时的缺陷类别。**分开报，不合并成一句「记录损坏」**：
/// 修法完全不同（改 task.json / 改 status.json / 查目录是否被串改）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowDefect {
    /// `task.json` 读不出来或不是合法 JSON / 缺字段。
    TaskMalformed(String),
    /// `status.json` 读不出来。
    StatusMalformed(String),
    /// 两份记录的 `taskId` 不一致——目录可能被手工改过，绝不能按其中一份行事。
    IdMismatch { task: String, status: String },
}

impl TaskRow {
    pub fn stage(&self) -> Stage {
        self.status.stage
    }

    /// 未完成任务：可续跑，**禁止清理它的恢复材料**（core 也会拦，界面先拦一道）。
    pub fn is_incomplete(&self) -> bool {
        self.status.stage.is_incomplete()
    }

    /// 稳定错误码：独立字段优先，其次从文案里抽（旧版只有文案带码）。
    pub fn error_code(&self) -> Option<&str> {
        self.status
            .error_code_text
            .as_deref()
            .filter(|value| value.starts_with("[BR-") || value.starts_with("BR-"))
            .or_else(|| {
                self.status
                    .error
                    .as_deref()
                    .and_then(status_receipt::extract_code)
            })
    }

    /// 更新时间（本地时区，`YYYY-MM-DD HH:MM`）。界面给人看，UTC 会差 8 小时。
    pub fn stamp(&self) -> String {
        self.status
            .updated
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }

    /// 排序键：越新越大。用毫秒时间戳，避免字符串比较在跨月时出错。
    fn sort_key(&self) -> i64 {
        self.status.updated.timestamp_millis()
    }
}

/// 从两份 JSON 文本解析一行。坏数据返回缺陷而不是 panic——一个坏目录不该让整个列表打不开。
pub(crate) fn parse_row(
    task_json: &str,
    status_json: &str,
    dir: &str,
) -> Result<TaskRow, RowDefect> {
    let task: Task = serde_json::from_str(task_json)
        .map_err(|error| RowDefect::TaskMalformed(error.to_string()))?;
    let status = status_receipt::parse_status(status_json).map_err(RowDefect::StatusMalformed)?;
    if task.task_id != status.task_id {
        return Err(RowDefect::IdMismatch {
            task: task.task_id,
            status: status.task_id,
        });
    }
    Ok(TaskRow {
        id: task.task_id,
        dir: dir.to_string(),
        operation: task.operation,
        status,
    })
}

/// 排序：**最新的在前**（用户几乎总是关心最近一次），同一时刻按 id 升序保证结果稳定。
pub(crate) fn sort_rows(rows: &mut [TaskRow]) {
    rows.sort_by(|left, right| {
        right
            .sort_key()
            .cmp(&left.sort_key())
            .then_with(|| left.id.cmp(&right.id))
    });
}

/// `Operation` 的中英文短名。**不复用 `native_gui::operation_display`**：那个接受 `&str`
/// 且覆盖 GUI 页签里的 `install-pe-entry` 等非枚举值，而本模块要无条件编译、只能依赖 core 的枚举。
fn operation_label(language: Language, operation: Operation) -> &'static str {
    match (language, operation) {
        (Language::Chinese, Operation::Probe) => "探测",
        (Language::Chinese, Operation::Backup) => "备份",
        (Language::Chinese, Operation::RestoreExisting) => "还原系统",
        (Language::Chinese, Operation::CreateSecondary) => "第二系统",
        (Language::English, Operation::Probe) => "probe",
        (Language::English, Operation::Backup) => "backup",
        (Language::English, Operation::RestoreExisting) => "restore",
        (Language::English, Operation::CreateSecondary) => "secondary",
    }
}

fn field_label(language: Language, key: &str) -> &'static str {
    match (language, key) {
        (Language::Chinese, "task_id") => "任务 ID",
        (Language::Chinese, "operation") => "操作",
        (Language::Chinese, "updated") => "更新时间",
        (Language::Chinese, "dir") => "任务目录",
        (Language::Chinese, "prepare_log") => "准备日志",
        (Language::Chinese, "recovery_log") => "恢复日志",
        (Language::Chinese, "defect") => "记录缺陷",
        (Language::Chinese, "incomplete") => "⚠ 未完成：可续跑，清理已禁用",
        (Language::English, "task_id") => "Task ID",
        (Language::English, "operation") => "Operation",
        (Language::English, "updated") => "Updated",
        (Language::English, "dir") => "Task dir",
        (Language::English, "prepare_log") => "Prepare log",
        (Language::English, "recovery_log") => "Recovery log",
        (Language::English, "defect") => "Record defect",
        (Language::English, "incomplete") => "WARNING incomplete: resumable, cleanup disabled",
        // 兜底：本模块只传上面这些 key，走到这里说明新增了键却没补文案，
        // 给空串让它在界面里显眼（而不是 panic 掉整个任务窗口）。
        _ => "",
    }
}

/// 下拉框里的一行：`#1 10-27 01:15 备份 成功`
///
/// 只放够区分的信息——详情面板负责完整内容。序号从 1 开始，与界面一致。
pub(crate) fn combo_label(index: usize, row: &TaskRow, language: Language) -> String {
    let stage = status_receipt::stage_label(row.stage(), language);
    // 时间只取「月-日 时:分」：下拉框宽度有限，年份在详情里。
    let short = row
        .status
        .updated
        .with_timezone(&chrono::Local)
        .format("%m-%d %H:%M")
        .to_string();
    let base = format!(
        "#{} {} {} {}",
        index + 1,
        short,
        operation_label(language, row.operation),
        stage
    );
    // ★ 2.6.67：失败任务在下拉列表里直接带稳定错误码。用户在列表里就能分清
    //   「磁盘满」和「镜像不存在」，不必逐条点开详情；同时这也让 error_code()
    //   有了生产调用点（此前只有测试用它，release 构建报 dead_code）。
    match row.error_code() {
        Some(code) => format!("{base} {code}"),
        None => base,
    }
}

/// 坏记录的下拉行：仍然可选中（用户要看目录路径去人工处理），但内容明确标注缺陷。
pub(crate) fn defect_combo_label(index: usize, dir: &str, language: Language) -> String {
    let tail = dir.rsplit(['\\', '/']).next().unwrap_or(dir);
    let short: String = tail.chars().take(8).collect();
    match language {
        Language::Chinese => format!("#{} {} 记录损坏", index + 1, short),
        Language::English => format!("#{} {} broken record", index + 1, short),
    }
}

/// 坏记录的详情文本：说清**哪一份**坏了、原文错误是什么、目录在哪。
pub(crate) fn defect_lines(defect: &RowDefect, dir: &str, language: Language) -> Vec<String> {
    let (kind, detail) = match defect {
        RowDefect::TaskMalformed(error) => ("task.json", error.as_str()),
        RowDefect::StatusMalformed(error) => ("status.json", error.as_str()),
        RowDefect::IdMismatch { task, status } => {
            return vec![
                format!("{}：task.json 与 status.json 的 taskId 不一致", field_label(language, "defect")),
                format!("  task.json    = {task}"),
                format!("  status.json  = {status}"),
                format!("{}：{dir}", field_label(language, "dir")),
            ];
        }
    };
    vec![
        format!("{}：{kind}", field_label(language, "defect")),
        format!("  {detail}"),
        format!("{}：{dir}", field_label(language, "dir")),
    ]
}

/// 详情面板的多行文本。
///
/// 行序：任务 ID → 操作 → 阶段/进度/错误（复用状态框那套规则）→ 更新时间 → 路径。
/// ★ 未完成任务在**最前面**加一行警示，因为它是「清理按钮为什么是灰的」的唯一解释。
pub(crate) fn detail_lines(row: &TaskRow, language: Language) -> Vec<String> {
    let mut lines = Vec::new();
    if row.is_incomplete() {
        lines.push(field_label(language, "incomplete").to_string());
    }
    lines.push(format!("{}：{}", field_label(language, "task_id"), row.id));
    lines.push(format!(
        "{}：{}",
        field_label(language, "operation"),
        operation_label(language, row.operation)
    ));
    lines.extend(status_receipt::render_status_lines(&row.status, language));
    lines.push(format!("{}：{}", field_label(language, "updated"), row.stamp()));
    lines.push(format!("{}：{}", field_label(language, "dir"), row.dir));
    let join = |name: &str| {
        let path = std::path::Path::new(&row.dir).join(name);
        format!(
            "{}：{}",
            field_label(
                language,
                if name.starts_with("prepare") {
                    "prepare_log"
                } else {
                    "recovery_log"
                }
            ),
            path.display()
        )
    };
    lines.push(join("prepare.log"));
    lines.push(join("recovery.log"));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    /// 抄自实机 `H:\brwork\tasks` 的最小 task.json（camelCase 键 + kebab-case 操作）。
    /// ★ `bootPlan.mode` 是**必填**字段（`BootMode` 无 default）：漏了它 core 会报
    ///   `missing field bootPlan`，测试夹具必须与真机形状一致。
    fn task_json(id: &str, operation: &str) -> String {
        // 必填字段（core 里没有 default）：taskId / version / operation /
        // bootPlan.mode / created / bootOnce / status。少一个就 `missing field`。
        format!(
            r#"{{"taskId":"{id}","version":1,"operation":"{operation}","bootPlan":{{"mode":"return-existing"}},"created":"2026-10-27T01:00:00Z","bootOnce":false,"status":"success"}}"#
        )
    }

    fn status_json(id: &str, stage: &str, progress: u32, extra: &str) -> String {
        format!(
            r#"{{"taskId":"{id}","operation":"backup","stage":"{stage}","progress":{progress},"updated":"2026-10-27T01:07:00Z"{extra}}}"#
        )
    }

    #[test]
    fn successful_task_has_no_error_lines() {
        let row = parse_row(
            &task_json("a1", "backup"),
            &status_json("a1", "success", 100, ""),
            r"H:\brwork\tasks\a1",
        )
        .unwrap();
        assert_eq!(row.stage(), Stage::Success);
        assert!(row.error_code().is_none());
        assert!(!row.is_incomplete());
        let lines = detail_lines(&row, Language::Chinese);
        // 成功态不该出现「失败原因 / 错误码」这两个词。
        assert!(!lines.iter().any(|line| line.contains("失败原因")), "{lines:?}");
        assert!(!lines.iter().any(|line| line.contains("错误码")), "{lines:?}");
    }

    #[test]
    fn failed_with_code_shows_the_code() {
        let row = parse_row(
            &task_json("b2", "backup"),
            &status_json(
                "b2",
                "failed",
                0,
                r#","errorCodeText":"BR-PE-039","error":"镜像文件不存在""#,
            ),
            r"H:\brwork\tasks\b2",
        )
        .unwrap();
        assert_eq!(row.error_code(), Some("BR-PE-039"));
        let lines = detail_lines(&row, Language::Chinese);
        assert!(lines.iter().any(|line| line.contains("BR-PE-039")), "{lines:?}");
        assert!(lines.iter().any(|line| line.contains("镜像文件不存在")), "{lines:?}");
    }

    #[test]
    fn failed_with_empty_reason_is_called_out() {
        // ★ 实机真实存在 `error: ""`：失败却没记录原因，必须说破，不能留空。
        let row = parse_row(
            &task_json("c3", "backup"),
            &status_json("c3", "failed", 0, r#","error":"""#),
            r"H:\brwork\tasks\c3",
        )
        .unwrap();
        assert_eq!(row.status.error.as_deref(), Some(""));
        let lines = detail_lines(&row, Language::Chinese);
        assert!(
            lines.iter().any(|line| line.contains("未记录")),
            "空原因必须说破：{lines:?}"
        );
    }

    #[test]
    fn broken_task_json_reports_defect_instead_of_panicking() {
        let defect = parse_row(
            "{ 这不是 JSON",
            &status_json("d4", "success", 100, ""),
            r"H:\brwork\tasks\d4",
        )
        .unwrap_err();
        assert!(matches!(defect, RowDefect::TaskMalformed(_)));
        let lines = defect_lines(&defect, r"H:\brwork\tasks\d4", Language::Chinese);
        assert!(lines[0].contains("task.json"), "{lines:?}");
        assert!(lines.iter().any(|line| line.contains(r"H:\brwork\tasks\d4")));
    }

    #[test]
    fn broken_status_json_reports_defect() {
        let defect = parse_row(
            &task_json("e5", "backup"),
            r#"{"stage":"不存在的阶段"}"#,
            r"H:\brwork\tasks\e5",
        )
        .unwrap_err();
        assert!(matches!(defect, RowDefect::StatusMalformed(_)));
    }

    #[test]
    fn id_mismatch_is_flagged_not_ignored() {
        // 目录被手工改过时，两份记录的 taskId 会对不上；按其中一份行事会认错任务。
        let defect = parse_row(
            &task_json("f6", "backup"),
            &status_json("9999", "success", 100, ""),
            r"H:\brwork\tasks\f6",
        )
        .unwrap_err();
        assert_eq!(
            defect,
            RowDefect::IdMismatch {
                task: "f6".into(),
                status: "9999".into()
            }
        );
        let lines = defect_lines(&defect, r"H:\brwork\tasks\f6", Language::Chinese);
        assert!(lines.iter().any(|line| line.contains("不一致")), "{lines:?}");
    }

    #[test]
    fn capturing_task_is_incomplete_and_warned_first() {
        let row = parse_row(
            &task_json("g7", "backup"),
            &status_json("g7", "capturing", 45, ""),
            r"H:\brwork\tasks\g7",
        )
        .unwrap();
        assert!(row.is_incomplete(), "capturing 必须判为未完成");
        let lines = detail_lines(&row, Language::Chinese);
        assert!(lines[0].contains("未完成"), "警示必须在第一行：{lines:?}");
        assert!(lines.iter().any(|line| line.contains("45%")), "{lines:?}");
    }

    #[test]
    fn rows_sort_newest_first_and_stable_on_same_instant() {
        let stamp = |id: &str, minute: u32| {
            let updated = Utc.with_ymd_and_hms(2026, 10, 27, 1, minute, 0).unwrap();
            TaskRow {
                id: id.to_string(),
                dir: format!(r"H:\brwork\tasks\{id}"),
                operation: Operation::Backup,
                status: StatusRecord {
                    task_id: id.to_string(),
                    operation: Operation::Backup,
                    stage: Stage::Success,
                    progress: 100,
                    updated,
                    error_code: None,
                    error_code_text: None,
                    error: None,
                },
            }
        };
        let mut rows = vec![
            stamp("old", 10),
            stamp("bb", 30),
            stamp("aa", 30),
            stamp("newest", 50),
        ];
        sort_rows(&mut rows);
        let order: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        // 最新在前；同一时刻按 id 升序（"aa" 在 "bb" 之前）。
        assert_eq!(order, vec!["newest", "aa", "bb", "old"]);
    }

    #[test]
    fn combo_label_carries_index_time_operation_and_stage() {
        let row = parse_row(
            &task_json("h8", "restore-existing"),
            &status_json("h8", "success", 100, ""),
            r"H:\brwork\tasks\h8",
        )
        .unwrap();
        let label = combo_label(0, &row, Language::Chinese);
        assert!(label.starts_with("#1 "), "{label}");
        assert!(label.contains("还原系统"), "{label}");
        assert!(label.contains("成功"), "{label}");
        // 英文界面下不能混进中文。
        let english = combo_label(0, &row, Language::English);
        assert!(english.contains("restore") && english.contains("success"), "{english}");
    }

    #[test]
    fn combo_label_appends_code_only_for_failures() {
        // 2.6.67：失败任务在下拉里直接带码；成功任务不能凭空多出一段。
        let failed = parse_row(
            &task_json("k1", "backup"),
            &status_json(
                "k1",
                "failed",
                0,
                r#","errorCodeText":"BR-PE-039","error":"镜像文件不存在""#,
            ),
            r"H:\brwork\tasks\k1",
        )
        .unwrap();
        let label = combo_label(0, &failed, Language::Chinese);
        assert!(label.ends_with(" BR-PE-039"), "{label}");

        let ok = parse_row(
            &task_json("k2", "backup"),
            &status_json("k2", "success", 100, ""),
            r"H:\brwork\tasks\k2",
        )
        .unwrap();
        let ok_label = combo_label(0, &ok, Language::Chinese);
        assert!(!ok_label.contains("BR-"), "{ok_label}");
    }
}
