//! 任务操作类型与 11 Stage 状态机模型
//!
//! 定义任务的高层操作类别（`Operation`）、生命周期流转状态机（`Stage`）、
//! 阶段跃迁合法性图谱、中断恢复/放弃边界以及基准进度折算（`default_progress`）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    /// Non-destructive wiring/WinRE probe used by Phase 0 and diagnostics.
    Probe,
    Backup,
    RestoreExisting,
    CreateSecondary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Prepared,
    BootRequested,
    RecoveryStarted,
    Preflight,
    Capturing,
    TargetErased,
    ImageApplied,
    BootRepaired,
    /// 磁盘、引导与恢复注册已验证；只剩任务启动项清理。
    RecoveryComplete,
    Success,
    Failed,
}

impl Stage {
    pub fn can_transition_to(self, next: Self) -> bool {
        use Stage::*;
        matches!(
            (self, next),
            (Prepared, BootRequested)
                | (Prepared, RecoveryStarted)
                | (Prepared, Failed)
                | (BootRequested, RecoveryStarted)
                | (BootRequested, Failed)
                | (RecoveryStarted, Preflight)
                | (RecoveryStarted, Failed)
                | (Preflight, Capturing)
                | (Preflight, TargetErased)
                | (Preflight, Success)
                | (Preflight, Failed)
                | (Capturing, Success)
                | (Capturing, Failed)
                | (TargetErased, ImageApplied)
                | (TargetErased, Failed)
                | (ImageApplied, BootRepaired)
                | (ImageApplied, Failed)
                | (BootRepaired, Success)
                | (BootRepaired, Failed)
                | (Preflight, RecoveryComplete)
                | (Capturing, RecoveryComplete)
                | (BootRepaired, RecoveryComplete)
                | (RecoveryComplete, Success)
                // 【2026-10-18 缺陷修复 H2】允许从 RecoveryComplete 标失败。
                // 此前这条边缺失，而 `write_failure` 第一行就是
                // `if !task.status.can_transition_to(Stage::Failed) { return Err(…) }`：
                // 任务在 STEP 4 清理阶段失败 → 自动逃出写失败被拒 → 只记一行日志 →
                // 任务**永远停在 RecoveryComplete**。而该阶段仍在
                // `can_resume_after_interruption` 里 → 下次启动又被发现 → 再提示重启
                // → 再失败，`abandon-task` 在该阶段同样被拒：**用户没有任何自救出口**。
                // 此刻磁盘/引导/恢复注册都已验证完毕，标失败只影响清理的记账口径。
                | (RecoveryComplete, Failed)
        )
    }

    /// A task at one of these stages has not begun modifying its restore target,
    /// so an explicit user abandonment may remove only its task-owned boot entry
    /// and recovery payload before another task is prepared.
    pub fn can_abandon_before_target_write(self) -> bool {
        matches!(
            self,
            Self::Prepared
                | Self::BootRequested
                | Self::RecoveryStarted
                | Self::Preflight
                // 【2026-10-18 缺陷修复：ef71fccc 三按钮死锁】`Capturing` 也要可放弃。
                // 该阶段是「把源卷捕获成暂存 WIM」——**还原目标卷还没被碰过**
                // （目标擦除发生在后面的 `TargetErased`，故它不在
                // `is_destructive_boundary` 里）。此前它不在这个集合里，于是
                // 「存在未完成任务」对话框的三条路全部走不通：是=重启续跑
                // （状态机不推进）、否=放弃清理（被本函数拒）、取消=不建新任务
                // → 任务永久卡在 Capturing，还会被 DISM 烘进备份镜像。
                | Self::Capturing
        )
    }

    /// A task past preparation may be resumed through its persisted boot entry.
    pub fn can_resume_after_interruption(self) -> bool {
        matches!(
            self,
            Self::BootRequested
                | Self::RecoveryStarted
                | Self::Preflight
                | Self::Capturing
                | Self::TargetErased
                | Self::ImageApplied
                | Self::BootRepaired
                | Self::RecoveryComplete
        )
    }

    pub fn is_destructive_boundary(self) -> bool {
        matches!(
            self,
            Self::TargetErased | Self::ImageApplied | Self::BootRepaired
        )
    }

    pub fn is_incomplete(self) -> bool {
        !matches!(self, Self::Success | Self::Failed)
    }
}

pub fn default_progress(stage: Stage) -> u8 {
    match stage {
        Stage::Prepared => 0,
        Stage::BootRequested => 2,
        Stage::RecoveryStarted => 5,
        Stage::Preflight => 10,
        Stage::Capturing => 20,
        Stage::TargetErased => 35,
        Stage::ImageApplied => 75,
        Stage::BootRepaired => 90,
        Stage::RecoveryComplete => 95,
        Stage::Success => 100,
        Stage::Failed => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_accepts_recovery_paths() {
        assert!(Stage::Prepared.can_transition_to(Stage::BootRequested));
        assert!(Stage::Prepared.can_transition_to(Stage::Failed));
        assert!(Stage::Preflight.can_transition_to(Stage::Capturing));
        assert!(Stage::Preflight.can_transition_to(Stage::TargetErased));
        assert!(Stage::Preflight.can_transition_to(Stage::Success));
        assert!(Stage::ImageApplied.can_transition_to(Stage::BootRepaired));
        assert!(!Stage::Prepared.can_transition_to(Stage::Success));
        // RecoveryComplete 标失败通道测试（缺陷修复 H2 关键断言）
        assert!(Stage::RecoveryComplete.can_transition_to(Stage::Failed));
        assert!(Stage::RecoveryComplete.can_transition_to(Stage::Success));
    }

    #[test]
    fn stage_abandon_before_target_write_boundaries() {
        // 在动盘前均允许放弃
        assert!(Stage::Prepared.can_abandon_before_target_write());
        assert!(Stage::BootRequested.can_abandon_before_target_write());
        assert!(Stage::RecoveryStarted.can_abandon_before_target_write());
        assert!(Stage::Preflight.can_abandon_before_target_write());
        assert!(Stage::Capturing.can_abandon_before_target_write());

        // 已经动盘（目标擦除及后续）绝对不可放弃
        assert!(!Stage::TargetErased.can_abandon_before_target_write());
        assert!(!Stage::ImageApplied.can_abandon_before_target_write());
        assert!(!Stage::BootRepaired.can_abandon_before_target_write());
        assert!(!Stage::RecoveryComplete.can_abandon_before_target_write());
        assert!(!Stage::Success.can_abandon_before_target_write());
        assert!(!Stage::Failed.can_abandon_before_target_write());
    }

    #[test]
    fn stage_resume_after_interruption_boundaries() {
        assert!(!Stage::Prepared.can_resume_after_interruption());
        assert!(Stage::BootRequested.can_resume_after_interruption());
        assert!(Stage::RecoveryStarted.can_resume_after_interruption());
        assert!(Stage::Preflight.can_resume_after_interruption());
        assert!(Stage::Capturing.can_resume_after_interruption());
        assert!(Stage::TargetErased.can_resume_after_interruption());
        assert!(Stage::ImageApplied.can_resume_after_interruption());
        assert!(Stage::BootRepaired.can_resume_after_interruption());
        assert!(Stage::RecoveryComplete.can_resume_after_interruption());
        assert!(!Stage::Success.can_resume_after_interruption());
        assert!(!Stage::Failed.can_resume_after_interruption());
    }

    #[test]
    fn stage_destructive_boundary_check() {
        assert!(Stage::TargetErased.is_destructive_boundary());
        assert!(Stage::ImageApplied.is_destructive_boundary());
        assert!(Stage::BootRepaired.is_destructive_boundary());

        assert!(!Stage::Prepared.is_destructive_boundary());
        assert!(!Stage::BootRequested.is_destructive_boundary());
        assert!(!Stage::RecoveryStarted.is_destructive_boundary());
        assert!(!Stage::Preflight.is_destructive_boundary());
        assert!(!Stage::Capturing.is_destructive_boundary());
        assert!(!Stage::RecoveryComplete.is_destructive_boundary());
        assert!(!Stage::Success.is_destructive_boundary());
        assert!(!Stage::Failed.is_destructive_boundary());
    }

    #[test]
    fn stage_incomplete_and_terminal_states() {
        assert!(!Stage::Success.is_incomplete());
        assert!(!Stage::Failed.is_incomplete());

        assert!(Stage::Prepared.is_incomplete());
        assert!(Stage::BootRequested.is_incomplete());
        assert!(Stage::RecoveryStarted.is_incomplete());
        assert!(Stage::Preflight.is_incomplete());
        assert!(Stage::Capturing.is_incomplete());
        assert!(Stage::TargetErased.is_incomplete());
        assert!(Stage::ImageApplied.is_incomplete());
        assert!(Stage::BootRepaired.is_incomplete());
        assert!(Stage::RecoveryComplete.is_incomplete());
    }

    #[test]
    fn default_progress_values() {
        assert_eq!(default_progress(Stage::Prepared), 0);
        assert_eq!(default_progress(Stage::BootRequested), 2);
        assert_eq!(default_progress(Stage::RecoveryStarted), 5);
        assert_eq!(default_progress(Stage::Preflight), 10);
        assert_eq!(default_progress(Stage::Capturing), 20);
        assert_eq!(default_progress(Stage::TargetErased), 35);
        assert_eq!(default_progress(Stage::ImageApplied), 75);
        assert_eq!(default_progress(Stage::BootRepaired), 90);
        assert_eq!(default_progress(Stage::RecoveryComplete), 95);
        assert_eq!(default_progress(Stage::Success), 100);
        assert_eq!(default_progress(Stage::Failed), 0);
    }

    #[test]
    fn operation_serde_roundtrip() {
        let ops = [
            (Operation::Probe, "\"probe\""),
            (Operation::Backup, "\"backup\""),
            (Operation::RestoreExisting, "\"restore-existing\""),
            (Operation::CreateSecondary, "\"create-secondary\""),
        ];

        for (op, want_json) in ops {
            let serialized = serde_json::to_string(&op).unwrap();
            assert_eq!(serialized, want_json);
            let deserialized: Operation = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, op);
        }
    }
}
