//! 进度窗的**纯文本**逻辑：时间格式化、剩余时间估算、4 阶段骨架清单。
//!
//! 为什么单独成模块：`recovery_progress` 整份文件是 `#[cfg(windows)]`，在本机
//! （macOS）`cargo test` 里一行都跑不到——那里每加一个单测都是「写完就再也不会
//! 被执行」的死测试。而下面这些东西**一个 Win32 调用都没有**，纯粹是字符串与
//! 算术，正是最容易在边界上出错、也最值得钉住的部分。
//!
//! 与 `text_parsing` 的分工：那边负责「把日志解析成状态」，这边负责「把状态排成
//! 给人看的几行字」。两者都不依赖平台，也都不走 i18n（进度窗是恢复环境里的
//! 中文界面，且这些字符串从不参与任何判定）。

use crate::text_parsing::StepProgress;

/// 秒数 → `MM:SS`（不足一小时）或 `HH:MM:SS`。
///
/// 不足一小时不显示 `00:` 前缀是刻意的：进度窗右上角那块地方窄，
/// 大多数任务的剩余时间都在一小时内，多两位数字会把当前时间挤掉。
pub(crate) fn format_hms(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 按当前百分比线性外推「还要多少秒」；`None` = 这时候算不出来。
///
/// 进度 0 时任何外推都是无穷大（做过一次除法就会得到 0 或溢出），进度满时
/// 剩余时间没有意义——两种都返回 `None`，界面显示 `--:--` 而不是一个假数字。
/// 用 `saturating_sub` 而不是减法：百分比不单调（日志重放、阶段回退）时会
/// 出现「总估计小于已用」，减法在这里会 panic（debug）或绕回成天文数字（release）。
pub(crate) fn estimate_remaining_secs(elapsed_secs: u64, percent: u32) -> Option<u64> {
    if percent == 0 || percent >= 100 {
        return None;
    }
    let total_est = elapsed_secs * 100 / u64::from(percent);
    Some(total_est.saturating_sub(elapsed_secs))
}

/// 4 阶段骨架的默认标题。
///
/// 第 2 步按「备份还是还原」分叉：两者都是耗时最长的一步，标题写错会让用户
/// 以为自己点错了操作。第 4 步按「在线还是恢复环境」分叉：在线备份的收尾是
/// 发布副档，恢复环境的收尾是清理启动项，是两件完全不同的事。
pub(crate) fn stage_defaults(is_backup: bool, online: bool) -> [(u32, &'static str); 4] {
    [
        (1, "挂载卷、校验"),
        (2, if is_backup { "捕获镜像（时间长）" } else { "还原镜像（时间长）" }),
        (3, "校验"),
        (4, if online { "发布副档/完成在线操作" } else { "清理恢复启动项/配置" }),
    ]
}

/// 骨架清单需要的一切输入，显式列出而不是从 `ProgressShared` 里现取——
/// 让这个函数可在本机单测里被任意组合驱动。
pub(crate) struct SkeletonInput<'a> {
    pub is_backup: bool,
    pub online: bool,
    /// 日志里解析到的步骤进度（步骤名/时间戳/总数/当前步）。
    pub progress: &'a StepProgress,
    /// 日志里出现了终态「操作完成」。**最后一步到了 100% 也不算**：
    /// 只有终态落盘才代表真的结束，提前把整列打满勾会骗人。
    pub all_done: bool,
    /// 已用秒数，只用于当前步骤小箭头的闪烁（偶数秒亮、奇数秒用全角空格占位）。
    pub elapsed_secs: u64,
    /// 当前步骤内部百分比（DISM 的 0–100），有值就追加一行。
    pub latest_percent: Option<u32>,
}

/// 生成详情区的骨架清单（每行一条，调用方用 `\r\n` 连接）。
///
/// 三档标记：`✓` 已完成、`▶` 进行中（隔秒闪烁）、`·` 未开始。闪烁时用全角
/// 空格占位而不是删掉箭头——删掉会让整行文字横向抖动，每秒跳一次很刺眼。
pub(crate) fn stage_skeleton(input: &SkeletonInput<'_>) -> Vec<String> {
    let total_steps = input.progress.total.unwrap_or(4);
    let current = input.progress.current.unwrap_or(0);
    let mut body: Vec<String> = Vec::new();
    for (idx, default_name) in stage_defaults(input.is_backup, input.online) {
        // 任务自报只有两步时，不要凭空多显示两行没发生过的阶段。
        if idx > total_steps {
            continue;
        }
        let recorded = input.progress.steps.iter().find(|(i, _, _)| *i == idx);
        let name = recorded.map_or(default_name, |(_, n, _)| n.as_str());
        let ts = recorded.and_then(|(_, _, t)| t.as_deref());
        let mark = if input.all_done || idx < current {
            "✓"
        } else if idx == current {
            if input.elapsed_secs.is_multiple_of(2) {
                "▶"
            } else {
                "\u{3000}"
            }
        } else {
            "·"
        };
        let time_str = ts.map_or_else(String::new, |t| format!(" [{t}]"));
        body.push(format!("{mark} {idx}/{total_steps} {name}{time_str}"));
    }
    if let Some(step) = input.latest_percent {
        body.push(format!("—— 当前进度 {step}% ——"));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(total: u32, current: u32, steps: &[(u32, &str, Option<&str>)]) -> StepProgress {
        StepProgress {
            steps: steps
                .iter()
                .map(|(i, n, t)| (*i, (*n).to_string(), t.map(str::to_string)))
                .collect(),
            total: Some(total),
            current: Some(current),
            current_percent: None,
            overall_percent: None,
        }
    }

    #[test]
    fn format_hms_switches_to_hours_only_when_needed() {
        assert_eq!(format_hms(0), "00:00");
        assert_eq!(format_hms(9), "00:09");
        assert_eq!(format_hms(59), "00:59");
        assert_eq!(format_hms(60), "01:00");
        assert_eq!(format_hms(3599), "59:59");
        // 3600 是唯一的分叉点：少一秒仍是 MM:SS，正好一小时才开始带小时段。
        assert_eq!(format_hms(3600), "01:00:00");
        assert_eq!(format_hms(3661), "01:01:01");
        assert_eq!(format_hms(86_399), "23:59:59");
    }

    #[test]
    fn estimate_remaining_only_answers_for_a_partial_progress() {
        // 没有进度就没有依据外推；已经满了也不该说「还要 X 秒」。
        assert_eq!(estimate_remaining_secs(100, 0), None);
        assert_eq!(estimate_remaining_secs(100, 100), None);
        // 50% 用了 100 秒 ⇒ 总估 200 秒，还剩 100 秒。
        assert_eq!(estimate_remaining_secs(100, 50), Some(100));
        assert_eq!(estimate_remaining_secs(100, 25), Some(300));
        // 百分比不单调时（进度回退/日志重放）总估可能小于已用，必须收敛到 0 而不是 panic。
        assert_eq!(estimate_remaining_secs(100, 1), Some(9_900));
        assert_eq!(estimate_remaining_secs(0, 50), Some(0));
    }

    #[test]
    fn skeleton_marks_done_current_and_pending() {
        let p = progress(4, 2, &[(1, "挂载卷、校验", Some("10:00:01"))]);
        let body = stage_skeleton(&SkeletonInput {
            is_backup: false,
            online: false,
            progress: &p,
            all_done: false,
            elapsed_secs: 2, // 偶数秒 ⇒ 当前步骤亮箭头
            latest_percent: None,
        });
        assert_eq!(
            body,
            vec![
                "✓ 1/4 挂载卷、校验 [10:00:01]",
                "▶ 2/4 还原镜像（时间长）",
                "· 3/4 校验",
                "· 4/4 清理恢复启动项/配置",
            ]
        );
    }

    #[test]
    fn skeleton_blinks_the_current_marker_on_odd_seconds() {
        let p = progress(4, 2, &[]);
        let odd = stage_skeleton(&SkeletonInput {
            is_backup: false,
            online: false,
            progress: &p,
            all_done: false,
            elapsed_secs: 3,
            latest_percent: None,
        });
        // 奇数秒用全角空格占位，保证文字不横向抖动。
        assert_eq!(odd[1], "\u{3000} 2/4 还原镜像（时间长）");
    }

    #[test]
    fn skeleton_claims_nothing_before_the_final_state_is_recorded() {
        // 最后一步 100% 也不算完成：all_done 只认日志里的终态。
        let p = progress(4, 4, &[]);
        let body = stage_skeleton(&SkeletonInput {
            is_backup: false,
            online: false,
            progress: &p,
            all_done: false,
            elapsed_secs: 2,
            latest_percent: Some(100),
        });
        assert_eq!(body[0], "✓ 1/4 挂载卷、校验");
        assert_eq!(body[2], "✓ 3/4 校验");
        assert_eq!(body[3], "▶ 4/4 清理恢复启动项/配置");
        assert_eq!(body[4], "—— 当前进度 100% ——");
    }

    #[test]
    fn skeleton_ticks_everything_once_the_task_really_finished() {
        let p = progress(4, 4, &[]);
        let body = stage_skeleton(&SkeletonInput {
            is_backup: true,
            online: true,
            progress: &p,
            all_done: true,
            elapsed_secs: 5,
            latest_percent: Some(100),
        });
        assert!(body.iter().take(4).all(|line| line.starts_with('✓')), "{body:?}");
        assert_eq!(body[3], "✓ 4/4 发布副档/完成在线操作");
    }

    #[test]
    fn skeleton_does_not_invent_stages_the_task_never_reported() {
        // 任务自报两步：不该凭空多出 3/4、4/4 两行。
        let p = progress(2, 1, &[]);
        let body = stage_skeleton(&SkeletonInput {
            is_backup: false,
            online: false,
            progress: &p,
            all_done: false,
            elapsed_secs: 0,
            latest_percent: None,
        });
        assert_eq!(body.len(), 2);
        assert_eq!(body[1], "· 2/2 还原镜像（时间长）");
    }

    #[test]
    fn skeleton_stage_titles_follow_the_operation() {
        // 备份/在线两个分叉：写错会让用户以为自己点错了操作。
        assert_eq!(stage_defaults(true, false)[1].1, "捕获镜像（时间长）");
        assert_eq!(stage_defaults(false, false)[1].1, "还原镜像（时间长）");
        assert_eq!(stage_defaults(false, true)[3].1, "发布副档/完成在线操作");
        assert_eq!(stage_defaults(false, false)[3].1, "清理恢复启动项/配置");
    }
}
