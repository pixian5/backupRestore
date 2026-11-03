//! 任务执行日志解析、编号步骤识别与综合进度追踪子系统。
//!
//! 在备份与还原任务执行过程中，控制台输出包含大量异构信息（DISM 进度百分比、
//! wimlib-imagex 阶段性心跳、编号步骤标记 `STEP n/N`、时间戳及错误提示）。
//! 本模块负责从非结构化日志中提取结构化步骤快照（[`StepProgress`] 与 [`StepTracker`]），
//! 并为 UI 进度条、阶段标题及详情文本提供准确的语义映射（[`classify_log_line`])。

use crate::wim_parse::{
    parse_percent, parse_wimlib_progress, wimlib_detail_line, wimlib_overall_percent,
};

/// 编号步骤与总进度。
///
/// PE 里的恢复流程是 `STEP n/N <名称>` 形式的编号步骤（见 `main.rs` 的
/// `append_log(log, "STEP 1/4 …")`）。进度窗口要回答三个问题——
/// **一共几步、现在第几步、当前这一步到百分之几**——所以把这三件事算清楚。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct StepProgress {
    /// 已出现的步骤，按编号升序、去重：`(编号, 名称, 时分秒时间戳)`。
    pub steps: Vec<(u32, String, Option<String>)>,
    /// 总步骤数（`N`），从任一步骤的 `n/N` 里取；取不到就是 `None`。
    pub total: Option<u32>,
    /// 当前步骤号 = 已出现的最大编号。
    pub current: Option<u32>,
    /// 当前步骤内部的百分比（DISM 那种 0–100）。
    pub current_percent: Option<u32>,
    /// 总进度百分比：已完成整步 + 当前步骤的部分进度。
    pub overall_percent: Option<u32>,
}

/// 从日志行行首提取时分秒时间戳 `HH:MM:SS`。
pub(crate) fn extract_timestamp_hms(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('[')
        && let Some(end) = trimmed.find(']')
    {
        let inside = &trimmed[1..end];
        // 尝试解析 RFC3339 / ISO-8601 形如 "2026-09-30T08:18:08.054361500+00:00"
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(inside) {
            let local: chrono::DateTime<chrono::Local> = dt.into();
            return Some(local.format("%H:%M:%S").to_string());
        }
        // 尝试匹配尾部的 HH:MM:SS（例如 "2026-09-30 16:18:08" 或 "16:18:08"）
        // ★ 必须按**字符**数定位，不能按字节（H-GUI-2，2026-10-28 修）：
        //   旧写法 `inside.len() - 8` 是字节偏移，日志行里一旦有中文
        //   （步骤名、"执行 08:18:08" 之类），偏移就落进某个字符中间 →
        //   `&inside[22..]` 直接 panic（实测：`inside '八' (bytes 21..24)`）。
        //   而 GUI 窗口过程 panic = 整个进程 abort，用户看到的是「程序突然没了」。
        //   `char_indices().rev().nth(7)` 同时解决两件事：不足 8 个字符返回 None（跳过），
        //   够 8 个字符时给出**字符边界**的字节下标。
        if let Some((start, _)) = inside.char_indices().rev().nth(7) {
            let tail = &inside[start..];
            if tail.chars().filter(|c| *c == ':').count() == 2 {
                return Some(tail.to_string());
            }
        }
    }
    None
}

/// 按**字符**（不是字节）截断，超长时补一个省略号。
///
/// ★ 为什么需要它（H-GUI-1，2026-10-28 修）：WIM 索引 JSON 解析失败的诊断分支
///   曾写 `&images_text[..4096]`——而 `ImageName` 几乎都是中文，4096 只要落在
///   某个字符中间就 panic（`byte index is not a char boundary`）。
///   最讽刺的是这段代码**本身就在错误处理路径上**（解析失败才走它），
///   于是「诊断代码自己成了崩溃源」：用户看到的是「解析失败后程序直接消失」。
///   凡是「给人看的截断」，一律按字符算。
pub(crate) fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(max_chars).collect();
    truncated.push('…');
    truncated
}

/// 从一段日志文本里算出步骤进度。
///
/// `log` 是**累积**的日志（不是增量），因为步骤清单要把前面已完成的步骤也列出来；
/// 调用方每次刷新都从当前 offset 起读，所以这里只处理新读到的那段——
/// 真正的累积由 `StepTracker` 负责。
pub(crate) fn step_progress_of_new_lines(lines: &[&str]) -> StepProgress {
    let mut progress = StepProgress::default();
    for line in lines {
        let Some((index, total, name)) = parse_step_marker(line) else {
            continue;
        };
        progress.total = Some(total);
        if progress.steps.iter().any(|(seen, _, _)| *seen == index) {
            // 同一步骤可能被重打（续跑、重试），保留第一次的名字即可。
            continue;
        }
        let ts = extract_timestamp_hms(line);
        progress.steps.push((index, name, ts));
    }
    progress.steps.sort_by_key(|(index, _, _)| *index);
    progress.current = progress.steps.last().map(|(index, _, _)| *index);
    progress
}

/// 带累积的步骤跟踪器：把总进度一起算出来。
///
/// 单独一个结构体是因为进度窗口每次只读到日志的**增量**，而"一共几步"和
/// "已完成几步"必须跨增量累积——不然窗口会在两次刷新之间忘记自己走到哪。
#[derive(Debug, Default, Clone)]
pub(crate) struct StepTracker {
    inner: StepProgress,
}

impl StepTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一批新读到的日志行，返回累加后的进度快照。
    pub fn feed(&mut self, lines: &[&str]) -> StepProgress {
        let fresh = step_progress_of_new_lines(lines);
        if let Some(total) = fresh.total {
            self.inner.total = Some(total);
        }
        for (index, name, ts) in fresh.steps {
            if let Some(existing) = self
                .inner
                .steps
                .iter_mut()
                .find(|(seen, _, _)| *seen == index)
            {
                if existing.2.is_none() && ts.is_some() {
                    existing.2 = ts;
                }
            } else {
                let timestamp =
                    ts.or_else(|| Some(chrono::Local::now().format("%H:%M:%S").to_string()));
                self.inner.steps.push((index, name, timestamp));
            }
        }
        self.inner.steps.sort_by_key(|(index, _, _)| *index);
        let previous_current = self.inner.current;
        self.inner.current = self.inner.steps.last().map(|(index, _, _)| *index);
        if self.inner.current != previous_current {
            self.inner.current_percent = fresh.current_percent;
        } else {
            self.inner.current_percent = fresh.current_percent.or(self.inner.current_percent);
        }
        self.inner.overall_percent = self.overall();
        self.inner.clone()
    }

    /// 记录当前步骤内部的百分比（来自 DISM / wimlib 进度行）。
    pub fn set_current_percent(&mut self, percent: Option<u32>) {
        self.inner.current_percent = percent;
        self.inner.overall_percent = self.overall();
    }

    pub fn snapshot(&self) -> StepProgress {
        self.inner.clone()
    }

    /// 总进度 = (已完成整步数 + 当前步骤的部分) / 总步数。
    fn overall(&self) -> Option<u32> {
        let total = self.inner.total?;
        let current = self.inner.current?;
        if total == 0 {
            return None;
        }
        let done_steps = current.saturating_sub(1) as f64;
        let inside = self.inner.current_percent.unwrap_or(0).min(100) as f64 / 100.0;
        let overall = ((done_steps + inside) / total as f64) * 100.0;
        Some(overall.round().clamp(0.0, 100.0) as u32)
    }
}

/// 从任意一行日志里取出编号步骤标记 `(编号, 总数, 名称)`；取不到返回 `None`。
///
/// 日志行的真实形态是 `[2026-09-29T03:36:32.259862400+00:00] STEP 1/4 准备备份环境…`
/// —— `STEP` 前面有方括号时间戳。所以**不能**用 `strip_prefix("STEP ")` 只匹配行首：
/// 2026-09-30 之前 `classify_log_line` 和进度窗口都是那么写的，结果编号步骤
/// 从来没能被识别，PE 里显示的一直是那句不动的「正在准备恢复环境…」。
///
/// 放在这里让「识别编号步骤」只有一份实现：进度窗口、阶段标题、单测共用。
pub(crate) fn parse_step_marker(line: &str) -> Option<(u32, u32, String)> {
    let start = line.find("STEP ")?;
    let rest = line[start + 5..].trim();
    let (token, name) = rest.split_once(' ')?;
    if !token.contains('/') {
        return None;
    }
    let mut parts = token.splitn(2, '/');
    let (Some(index), Some(total)) = (parts.next(), parts.next()) else {
        return None;
    };
    let (Ok(index), Ok(total)) = (index.trim().parse::<u32>(), total.trim().parse::<u32>()) else {
        return None;
    };
    if index == 0 || total == 0 || index > total {
        return None;
    }
    let mut clean_name = name.trim().to_string();
    // 移除括号说明废话（如“（DISM，百分比见进度条）”）
    if let Some(idx) = clean_name.find("（DISM") {
        clean_name = clean_name[..idx].trim().to_string();
    } else if let Some(idx) = clean_name.find("(DISM") {
        clean_name = clean_name[..idx].trim().to_string();
    }
    Some((index, total, clean_name))
}

/// 解析日志行，返回 `(阶段标题, 进度百分比, 详情行)`。
pub(crate) fn classify_log_line(line: &str) -> (Option<String>, Option<u32>, Option<String>) {
    let mut stage = None;
    if line.contains("running dism.exe") {
        if line.contains("/Capture-Image") {
            stage = Some("正在备份系统分区…".to_string());
        } else if line.contains("/Apply-Image") {
            stage = Some("正在还原系统分区…".to_string());
        }
    } else if line.contains("Operating on the") || line.contains("Scanning") {
        stage = Some("正在扫描系统分区…".to_string());
    } else if line.contains("Saving image") {
        stage = Some("正在备份系统分区…".to_string());
    } else if line.contains("Applying image") {
        stage = Some("正在还原系统分区…".to_string());
    } else if line.contains("Backup capture finished") {
        stage = Some("备份完成，正在校验镜像…".to_string());
    } else if line.contains("The operation completed successfully") {
        // DISM 的一条命令成功，不代表哈希、副档和清理已完成。
        stage = Some("当前命令完成，等待后续校验".to_string());
    } else if line.contains("Boot entry cleaned; task marked successful")
        // ★ 双语都要认：日志用什么语言写取决于**运行时**的语言设置
        //   （v2.6.39 起的 i18n / `BACKUPRESTORE_LANG`）。只认中文的话，
        //   英文模式下这一条永远不命中 → 终态识别不到，进度窗停在上一阶段。
        //   文案从 i18n 表取（单一来源），避免两处硬编码不同步。
        || line.contains(crate::i18n::msg_in(
            crate::i18n::MessageLanguage::Chinese,
            "online_done",
        ))
        || line.contains(crate::i18n::msg_in(
            crate::i18n::MessageLanguage::English,
            "online_done",
        ))
    {
        stage = Some(crate::i18n::msg("stage_done").to_string());
    } else if line.contains("Recovery completed") {
        stage = Some("恢复已完成，正在清理".to_string());
    } else if line.contains("Recovery.exe started") || line.contains("started from env") {
        stage = Some("正在准备恢复环境…".to_string());
    } else if let Some((index, total, name)) = parse_step_marker(line) {
        // 备份/还原流程打的编号步骤标记，形如 "STEP 2/4 捕获系统分区镜像"。
        // 进度窗口据此显示「n/N <名称>」，让用户看清当前处在第几步。
        stage = Some(format!("{index}/{total} {name}"));
    }
    let wimlib = parse_wimlib_progress(line);
    // wimlib 的阶段内百分比要先折成整条 apply 的 0–100；DISM 的进度行照旧。
    let percent = wimlib
        .map(|(phase, phase_percent)| wimlib_overall_percent(phase, phase_percent))
        .or_else(|| parse_percent(line));
    let trimmed = line.trim();
    let detail = if let Some((phase, _)) = wimlib {
        // wimlib 的进度片段是 `\r` 原地刷新的英文计数行，原样进日志尾部会
        // 每秒刷掉一条真实消息，改写成一句中文进度。
        Some(wimlib_detail_line(phase, line))
    } else if trimmed.is_empty()
        || ((line.starts_with("[stdout] [") || line.starts_with('[')) && line.contains('%'))
        || line.starts_with("STEP ")
        || trimmed == "[stdout]"
        || trimmed == "[stderr]"
    {
        None // 空行、纯进度行、编号步骤行、纯标签空行都不占详情（步骤已由阶段标题展示）
    } else {
        Some(line.trim_end().to_string())
    };
    (stage, percent, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wimlib_progress_drives_the_progress_bar_and_gets_a_readable_detail() {
        // 这是 v2.6.27 实机「进度条一直是空」的回归护栏：
        // wimlib 的行首是计数，parse_percent 拼出来必然越界 → 曾经恒为 0。
        let (_, percent, detail) =
            classify_log_line("Extracting file data: 15 GiB of 68 GiB (22%) done");
        assert_eq!(percent, Some(27), "22% 的展开阶段应折成整条 apply 的 27%");
        assert_eq!(
            detail.as_deref(),
            Some("正在展开系统文件：15 GiB of 68 GiB (22%)")
        );
        // 英文标签换掉、数字保留。
        let (_, percent, detail) = classify_log_line("Creating files: 500 of 633699 (1%) done");
        assert_eq!(percent, Some(0));
        assert_eq!(
            detail.as_deref(),
            Some("正在创建文件：500 of 633699 (1%)")
        );
        // DISM 的老格式不受影响。
        assert_eq!(classify_log_line("[stdout] [=  45.6% =]").1, Some(45));
    }

    #[test]
    fn dism_success_does_not_finish_the_whole_transaction() {
        assert_ne!(
            classify_log_line("The operation completed successfully.")
                .0
                .as_deref(),
            Some("操作完成")
        );
        assert_eq!(
            classify_log_line("操作完成：命令、回读和副档簿记均已通过")
                .0
                .as_deref(),
            Some("操作完成")
        );
    }

    #[test]
    fn log_lines_map_to_stage_percent_and_detail() {
        let (stage, percent, detail) =
            classify_log_line("running dism.exe /Capture-Image /ImageFile:E:\\1.wim");
        assert_eq!(stage.as_deref(), Some("正在备份系统分区…"));
        assert_eq!(percent, None);
        assert!(detail.is_some());

        assert_eq!(
            classify_log_line("running dism.exe /Apply-Image")
                .0
                .as_deref(),
            Some("正在还原系统分区…")
        );
        assert_eq!(
            classify_log_line("Recovery completed").0.as_deref(),
            Some("恢复已完成，正在清理")
        );
        assert_eq!(
            classify_log_line("Boot entry cleaned; task marked successful")
                .0
                .as_deref(),
            Some("操作完成")
        );
        assert_eq!(
            classify_log_line("Backup capture finished").0.as_deref(),
            Some("备份完成，正在校验镜像…")
        );
        assert_eq!(
            classify_log_line("Recovery.exe started from env")
                .0
                .as_deref(),
            Some("正在准备恢复环境…")
        );
        assert_eq!(
            classify_log_line("Scanning the system partition")
                .0
                .as_deref(),
            Some("正在扫描系统分区…")
        );
    }

    #[test]
    fn step_markers_render_as_numbered_stage_and_stay_out_of_detail() {
        let (stage, _percent, detail) = classify_log_line("STEP 2/4 捕获系统分区镜像");
        assert_eq!(stage.as_deref(), Some("2/4 捕获系统分区镜像"));
        assert_eq!(detail, None, "编号步骤行不应占详情框（已由阶段标题展示）");

        // 没有 n/N 的 STEP 行**不**再当成编号步骤：进度窗口要回答"一共几步、
        // 现在第几步"，认不出编号就无法定位，给它一个"步骤：xxx"的标题反而
        // 会让用户以为它在步骤序列里。2026-09-30 收紧，真实日志都带编号。
        let (single, _, _) = classify_log_line("STEP 准备环境");
        assert_eq!(single, None);
    }

    #[test]
    fn progress_only_lines_and_blanks_leave_the_detail_box_alone() {
        assert_eq!(classify_log_line("[stdout] [=  45.6% =]").2, None);
        assert_eq!(classify_log_line("   ").2, None);
        assert_eq!(classify_log_line("[= 10% =]").2, None);
        // A real message keeps its detail even when it carries a percentage
        // only in a non-bracketed position.
        assert_eq!(
            classify_log_line("copied 10% of the payload").2.as_deref(),
            Some("copied 10% of the payload")
        );
        assert_eq!(
            classify_log_line("plain message\r\n").2.as_deref(),
            Some("plain message")
        );
    }

    /// 这条是 2026-09-30 的回归锁：此前用 `strip_prefix("STEP ")` 只匹配行首，
    /// 而真实形态是 `[时间戳] STEP 1/4 …`，于是编号步骤从来没能被识别，
    /// PE 进度窗口一直停在初始那句「正在准备恢复环境…」（用户实机反馈）。
    #[test]
    fn parse_step_marker_works_on_real_timestamped_log_lines() {
        let real = "[2026-09-29T03:36:32.259862400+00:00] STEP 1/4 准备备份环境（挂载卷、校验、必要时还原恢复环境）";
        let (index, total, name) =
            parse_step_marker(real).expect("必须能从真实日志行解析出步骤");
        assert_eq!((index, total), (1, 4));
        assert!(name.contains("准备备份环境"));

        // 阶段标题也要能认出来，不只是步骤跟踪器。
        let (stage, _percent, _detail) = classify_log_line(real);
        assert_eq!(
            stage.as_deref(),
            Some("1/4 准备备份环境（挂载卷、校验、必要时还原恢复环境）")
        );
    }

    /// 认不出步骤标记时返回 None，不能 panic、不能猜。
    #[test]
    fn parse_step_marker_returns_none_for_non_step_lines() {
        assert!(parse_step_marker("").is_none());
        assert!(parse_step_marker("[ts] The operation completed successfully.").is_none());
        assert!(parse_step_marker("[ts] STEP ").is_none());
        assert!(parse_step_marker("[ts] STEP abc/def name").is_none());
        assert!(parse_step_marker("[ts] STEP 9/4 impossible").is_none());
        // 没有 name 也不算
        assert!(parse_step_marker("[ts] STEP 1/4").is_none());
    }

    #[test]
    fn step_progress_reads_numbered_steps_across_increments() {
        let mut tracker = StepTracker::new();

        // 第一批：只看到第 1 步
        let first = tracker.feed(&["[stdout] STEP 1/4 准备备份环境（挂载卷、校验）"]);
        assert_eq!(first.total, Some(4));
        assert_eq!(first.current, Some(1));
        assert_eq!(first.steps.len(), 1);
        assert_eq!(first.steps[0].0, 1);
        assert!(first.steps[0].1.contains("准备备份环境"));

        // 第二批：第 2 步 + DISM 66%
        let second = tracker.feed(&["STEP 2/4 捕获系统分区镜像（DISM）", "[stdout] [66.0%]"]);
        assert_eq!(second.current, Some(2));
        assert_eq!(second.steps.len(), 2, "步骤清单必须跨增量累积");

        tracker.set_current_percent(Some(66));
        let snap = tracker.snapshot();
        // (1 + 0.66) / 4 = 41.5% → 42
        assert_eq!(snap.overall_percent, Some(42));
    }

    /// 重复打同一步骤不能重复入列（续跑/重试会重打）。
    #[test]
    fn step_progress_deduplicates_repeated_steps() {
        let mut tracker = StepTracker::new();
        tracker.feed(&["STEP 1/4 准备备份环境"]);
        let again = tracker.feed(&["STEP 1/4 准备备份环境", "STEP 2/4 捕获"]);
        assert_eq!(again.steps.len(), 2);
        assert_eq!(again.current, Some(2));
    }

    /// 畸形 STEP 行必须被忽略，不能把总数带偏。
    #[test]
    fn step_progress_ignores_malformed_step_lines() {
        let mut tracker = StepTracker::new();
        let none = tracker.feed(&[
            "STEP ",
            "STEP abc",
            "STEP 0/4 bad",
            "STEP 5/4 bad",
            "not a step",
        ]);
        assert!(none.total.is_none());
        assert!(none.steps.is_empty());
        assert!(none.current.is_none());
        assert!(none.overall_percent.is_none());
    }

    /// 没有编号步骤时（老格式日志）不能 panic，也不能算出百分比。
    #[test]
    fn step_progress_without_numbered_steps_reports_nothing() {
        let mut tracker = StepTracker::new();
        let progress =
            tracker.feed(&["[stdout] [100.0%]", "The operation completed successfully."]);
        assert!(progress.total.is_none());
        assert!(progress.current.is_none());
        assert!(
            progress.overall_percent.is_none(),
            "没有 n/N 就不该编造总进度"
        );
    }

    /// 当前步骤百分比要夹在 0–100，且总进度不超过 100。
    #[test]
    fn step_progress_clamps_percentages() {
        let mut tracker = StepTracker::new();
        tracker.feed(&["STEP 1/2 第一步"]);
        tracker.set_current_percent(Some(250));
        let snap = tracker.snapshot();
        assert_eq!(snap.overall_percent, Some(50), "250% 应夹成 100% → 半程");
        tracker.set_current_percent(Some(100));
        assert_eq!(tracker.snapshot().overall_percent, Some(50));
    }

    /// 步骤推进到下一步时，必须清空上一步残留的百分比。
    #[test]
    fn step_progress_resets_current_percent_when_advancing_step() {
        let mut tracker = StepTracker::new();
        tracker.feed(&["STEP 1/4 准备环境"]);
        tracker.set_current_percent(Some(100));
        assert_eq!(tracker.snapshot().current_percent, Some(100));

        // 进入第 2 步，无百分比时应被清空，不应残留上一步的 100%
        let second = tracker.feed(&["STEP 2/4 捕获镜像"]);
        assert_eq!(second.current, Some(2));
        assert_eq!(second.current_percent, None);
    }

    /// 提取时间戳并清理括号内的冗余废话（如“（DISM，百分比见进度条）”）。
    #[test]
    fn step_progress_extracts_timestamp_and_cleans_remarks() {
        let mut tracker = StepTracker::new();
        let log = "[2026-09-30T08:18:08.054361500+00:00] STEP 2/4 捕获系统分区镜像（DISM，百分比见进度条）";
        let progress = tracker.feed(&[log]);
        assert_eq!(progress.steps.len(), 1);
        assert_eq!(progress.steps[0].0, 2);
        assert_eq!(progress.steps[0].1, "捕获系统分区镜像");
        assert!(progress.steps[0].2.is_some());
    }

    /// 验证还原的四阶段简化标题格式正确，不带多余前缀与符号。
    #[test]
    fn step_markers_restore_stages_match_clean_format() {
        let lines = [
            "STEP 1/4 挂载卷、校验",
            "STEP 2/4 还原镜像（时间长）",
            "STEP 3/4 校验",
            "STEP 4/4 清理恢复启动项/配置",
        ];
        for line in lines {
            let (stage, _, _) = classify_log_line(line);
            assert!(stage.is_some());
            let title = stage.unwrap();
            assert!(!title.contains("步骤"));
            assert!(!title.contains('：'));
            assert!(
                title.starts_with("1/4 ")
                    || title.starts_with("2/4 ")
                    || title.starts_with("3/4 ")
                    || title.starts_with("4/4 ")
            );
        }
    }

    /// 纯 [stdout] / [stderr] 空标签行不占详情。
    #[test]
    fn classify_log_line_filters_empty_stdout_stderr_tags() {
        let (_, _, detail1) = classify_log_line("[stdout]");
        assert!(detail1.is_none());
        let (_, _, detail2) = classify_log_line("  [stdout]   ");
        assert!(detail2.is_none());
        let (_, _, detail3) = classify_log_line("[stderr]");
        assert!(detail3.is_none());
        let (_, _, detail4) = classify_log_line("[stdout] real message");
        assert_eq!(detail4.as_deref(), Some("[stdout] real message"));
    }
}

/// 多字节安全回归（H-GUI-1 / H-GUI-2）。
///
/// 背景：这两处都曾是**按字节**切片 `&s[..n]`。只要字符串里有中文（日志步骤名、
/// WIM 的 `ImageName` 几乎都是中文），n 就可能落在字符中间 → `char boundary` panic。
/// 而 GUI 的窗口过程一 panic 就是整个进程 abort，用户看到的是「程序突然没了」。
#[cfg(test)]
mod multibyte_safety {
    use super::*;

    /// inside 是 10 个三字节汉字（30 字节），`30-8=22` 落在第 8 个字符的中间
    /// → 旧实现 `&inside[22..]` 直接 panic。日志行出现中文就会炸。
    #[test]
    fn timestamp_extraction_survives_multibyte_tail() {
        assert_eq!(extract_timestamp_hms("[一二三四五六七八九十]"), None);
    }

    /// 正向：中/英文前缀 + 尾部 `HH:MM:SS` 都要照常取到（不能为了不 panic 就放弃解析）。
    #[test]
    fn timestamp_extraction_keeps_cjk_prefix() {
        assert_eq!(
            extract_timestamp_hms("[执行 08:18:08]").as_deref(),
            Some("08:18:08")
        );
        assert_eq!(
            extract_timestamp_hms("[2026-09-30 16:18:08]").as_deref(),
            Some("16:18:08")
        );
    }

    /// H-GUI-1：截断必须按**字符**算。8 个汉字是 24 字节，按字节取 4 会切在
    /// 字符中间；按字符取 4 才是「备份还原」+ 省略号。
    #[test]
    fn truncate_counts_chars_not_bytes() {
        assert_eq!(truncate_chars("备份还原镜像文件", 4), "备份还原…");
        // 没超长时原样返回，且**不加**省略号（否则用户以为内容被截了）
        assert_eq!(truncate_chars("备份还原镜像文件", 20), "备份还原镜像文件");
        // 恰好等于上限：不加省略号
        assert_eq!(truncate_chars("abcd", 4), "abcd");
        assert_eq!(truncate_chars("", 4), "");
    }
}
