//! 挂起引导任务与断电续跑决策控制器（Pending Task & Boot Resume Guard）。
//!
//! 负责：
//! - 宿主开机/程序启动时探测是否存在中断未完成的恢复任务（`find_pending_boot_task`）；
//! - 单阶段单次重试防打转标记（`claim_boot_resume_attempt` / `release_boot_resume_attempt`）；
//! - 跨阶段自动续跑总次数闸门（`allow_total_attempt` / `bump_total_attempt`）；
//! - 孤儿僵尸任务自动收口清理（`collect_orphan_tasks`；正在准备中的任务不算孤儿，见 `prepare_lock`）；
//! - 立即续跑、延后处置与放弃任务的生命周期调度。

#[cfg(any(windows, test))]
use std::fs::{self, OpenOptions};
#[cfg(any(windows, test))]
use std::io::Write;
#[cfg(any(windows, test))]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(windows)]
use std::env;

use backuprestore_core::TaskError;
#[cfg(any(windows, test))]
use backuprestore_core::Stage;
#[cfg(windows)]
use backuprestore_core::{Operation, StatusRecord, TaskStore, read_json};

#[cfg_attr(not(windows), allow(unused_imports))]
use crate::err;
#[cfg(windows)]
use crate::append_log;
#[cfg(windows)]
use crate::process_runner::{output_tail_summary, run_output_logged};
#[cfg(windows)]
use crate::recovery_engine::complete_recovery_cleanup;

/// A prepared task may be a deliberate `--no-reboot` diagnostic and must not
/// start WinRE by itself. Every later non-terminal stage, however, represents
/// an already authorized recovery that was interrupted after WinRE staging;
/// it is safe to re-request the one-time WinRE boot after artifact validation.
#[cfg(any(windows, test))]
pub(crate) fn stage_resumable_after_interruption(stage: Stage) -> bool {
    matches!(
        stage,
        Stage::BootRequested
            | Stage::RecoveryStarted
            | Stage::Preflight
            | Stage::Capturing
            | Stage::TargetErased
            | Stage::ImageApplied
            | Stage::BootRepaired
            | Stage::RecoveryComplete
    )
}

/// marker per durable stage permits resuming after later progress, while a
/// failed WinRE handoff cannot reboot Windows forever at the same stage.
#[cfg(any(windows, test))]
pub(crate) fn claim_boot_resume_attempt(task_dir: &Path, stage: backuprestore_core::Stage) -> Result<bool, TaskError> {
    let marker = task_dir.join(format!("boot-resume-{stage:?}.attempted"));
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(mut file) => {
            if let Err(error) = file
                .write_all(b"automatic WinRE resume claimed\n")
                .and_then(|()| file.sync_all())
            {
                drop(file);
                let _ = fs::remove_file(marker);
                return Err(error.into());
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
pub(crate) fn release_boot_resume_attempt(task_dir: &Path, stage: backuprestore_core::Stage) -> Result<(), TaskError> {
    let marker = task_dir.join(format!("boot-resume-{stage:?}.attempted"));
    match fs::remove_file(marker) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// 跨阶段**自动续跑的总次数上限**。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const MAX_TOTAL_RESUME_ATTEMPTS: u32 = 3;

/// 纯判据：累计已重启 `count` 次之后，是否还允许下一次自动续跑。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn allow_total_attempt(count: u32, max: u32) -> bool {
    count < max
}

/// 一个「已经授权、但被中断」的待续跑任务。
#[cfg(windows)]
#[derive(Clone)]
pub(crate) struct PendingResume {
    pub task_id: String,
    pub workspace: PathBuf,
    pub task_dir: PathBuf,
    pub operation: Operation,
    pub stage: Stage,
}

/// 跨阶段计数文件（任务目录内，与 per-stage 标记并列）。
#[cfg_attr(not(windows), allow(dead_code))]
const TOTAL_RESUME_MARKER: &str = "boot-resume-total.count";

#[cfg(windows)]
pub(crate) fn total_attempt_count(task_dir: &Path) -> u32 {
    fs::read_to_string(task_dir.join(TOTAL_RESUME_MARKER))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

#[cfg(windows)]
pub(crate) fn bump_total_attempt(task_dir: &Path) -> Result<u32, TaskError> {
    let count = total_attempt_count(task_dir).saturating_add(1);
    let final_path = task_dir.join(TOTAL_RESUME_MARKER);
    let tmp_path = task_dir.join(format!("{TOTAL_RESUME_MARKER}.tmp"));
    fs::write(&tmp_path, count.to_string())?;
    fs::rename(&tmp_path, &final_path)?;
    Ok(count)
}

#[cfg(windows)]
pub(crate) fn collect_orphan_tasks() -> Result<usize, TaskError> {
    let executable = env::current_exe()?;
    let workspace = executable
        .parent()
        .ok_or_else(|| err("executable has no workspace directory"))?;
    collect_orphan_tasks_in(workspace, &crate::log_root::logs_dir().join("gui.log"))
}

/// 在指定工作区（里面有 `tasks\` 目录）里收口孤儿任务。工作区单列成参数是为了能写单元测试
/// （`collect_orphan_tasks` 用的是可执行程序所在目录，测试里指不到临时目录）。
///
/// 【2026-11-24 实机缺陷】原来这里的判据只有三条（非终态 + 没写目标卷 + 没有启动项记录），
/// 「正在准备中的任务」三条全中：准备流程先落盘任务、最后才写启动项记录。现在多一条——
/// **在办的不动**（判据见 `prepare_lock::in_flight`），并留下一行日志说明为什么没动。
#[cfg(windows)]
pub(crate) fn collect_orphan_tasks_in(workspace: &Path, log: &Path) -> Result<usize, TaskError> {
    let tasks_root = workspace.join("tasks");
    if !tasks_root.is_dir() {
        return Ok(0);
    }
    let store = TaskStore::new(workspace);
    let mut collected = 0usize;
    for entry in fs::read_dir(&tasks_root)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let task_id = entry.file_name().to_string_lossy().to_string();
        let Ok(mut task) = store.load(&task_id) else {
            continue;
        };
        if !task.status.is_incomplete() || !task.status.can_abandon_before_target_write() {
            continue;
        }
        if crate::boot_entry::ReBootEntry::read(&entry.path())
            .ok()
            .flatten()
            .is_some()
        {
            continue;
        }
        // ★ 正在准备中的任务：另一个进程（命令行准备、另一个界面实例派出的准备子进程）还在写
        //   它的材料，启动项记录要到最后才写。这时候收口 = 那边随后报莫名其妙的错，甚至
        //   一次性启动已经武装（2026-11-24 两次实机复现，见 prepare_lock.rs）。一律不动。
        if crate::prepare_lock::in_flight(&entry.path()) {
            let _ = crate::append_log(
                log,
                &format!(
                    "orphan task guard: task {task_id} stage={:?} is still being prepared; left alone",
                    task.status
                ),
            );
            continue;
        }
        let _ = crate::append_log(
            log,
            &format!(
                "orphan task guard: task {task_id} stage={:?} has no boot entry and cannot resume; marking Failed",
                task.status
            ),
        );
        if store
            .write_failure(&mut task, 2, "自动收口：无一次性启动项，任务无法续跑")
            .is_err()
        {
            continue;
        }
        let _ = store.cleanup_task_artifacts(&task_id);
        collected += 1;
    }
    Ok(collected)
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub(crate) fn collect_orphan_tasks() -> Result<usize, TaskError> {
    Ok(0)
}

#[cfg(windows)]
pub(crate) fn find_pending_boot_task() -> Result<Option<PendingResume>, TaskError> {
    let executable = env::current_exe()?;
    let workspace = executable
        .parent()
        .ok_or_else(|| err("executable has no workspace directory"))?;
    let tasks_root = workspace.join("tasks");
    if !tasks_root.is_dir() {
        return Ok(None);
    }

    let mut pending = Vec::new();
    let mut skipped_without_entry: Vec<String> = Vec::new();
    for entry in fs::read_dir(&tasks_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let task_id = entry.file_name().to_string_lossy().to_string();
        let status_path = entry.path().join("status.json");
        let Ok(task) = TaskStore::new(workspace).load(&task_id) else {
            continue;
        };
        if !stage_resumable_after_interruption(task.status) || task.validate().is_err() {
            continue;
        }
        let Ok(status) = read_json::<StatusRecord>(&status_path) else {
            continue;
        };
        if status.task_id.eq_ignore_ascii_case(&task.task_id)
            && status.operation == task.operation
            && status.stage == task.status
            && task.task_id.eq_ignore_ascii_case(&task_id)
        {
            match read_json::<crate::boot_entry::ReBootEntry>(&entry.path().join("boot-entry.json")) {
                Ok(record) if !record.loader_guid.trim().is_empty() => {
                    if !crate::boot_entry::bcd_object_present(&record.loader_guid) {
                        skipped_without_entry.push(task_id.clone());
                        continue;
                    }
                }
                _ => {}
            }
            pending.push((task, entry.path()));
        }
    }

    if !skipped_without_entry.is_empty() {
        let log = workspace.join("logs").join("launcher-errors.log");
        append_log(
            &log,
            &format!(
                "Pending recovery skipped (boot entry no longer in BCD): {}",
                skipped_without_entry.join(",")
            ),
        )?;
    }

    if pending.is_empty() {
        return Ok(None);
    }
    if pending.len() != 1 {
        let log = workspace.join("logs").join("launcher-errors.log");
        append_log(
            &log,
            &format!(
                "Pending recovery not resumed: {} valid resumable tasks found",
                pending.len()
            ),
        )?;
        return Ok(None);
    }

    let Some((mut task, task_dir)) = pending.pop() else {
        return Ok(None);
    };
    if task.status == backuprestore_core::Stage::RecoveryComplete {
        complete_recovery_cleanup(
            &TaskStore::new(workspace),
            &mut task,
            &task_dir.join("prepare.log"),
        )?;
        return Ok(None);
    }
    Ok(Some(PendingResume {
        task_id: task.task_id.clone(),
        workspace: workspace.to_path_buf(),
        task_dir,
        operation: task.operation.clone(),
        stage: task.status,
    }))
}

#[cfg(windows)]
pub(crate) fn resume_pending_now(pending: &PendingResume) -> Result<bool, TaskError> {
    let log = pending.task_dir.join("prepare.log");
    let attempts = total_attempt_count(&pending.task_dir);
    if !allow_total_attempt(attempts, MAX_TOTAL_RESUME_ATTEMPTS) {
        append_log(
            &log,
            &crate::i18n::msg_tpl(
                "resume_limit_reached",
                &[
                    &MAX_TOTAL_RESUME_ATTEMPTS.to_string(),
                    &attempts.to_string(),
                ],
            ),
        )?;
        return Ok(false);
    }
    if !claim_boot_resume_attempt(&pending.task_dir, pending.stage)? {
        append_log(
            &log,
            &format!(
                "Automatic WinRE resume suppressed: stage {:?} already had its one retry; manual inspection is required",
                pending.stage
            ),
        )?;
        append_log(
            &log,
            crate::i18n::msg("resume_paused"),
        )?;
        return Ok(false);
    }
    for required in [
        pending.task_dir.join("payload").join("Recovery.exe"),
        pending.task_dir.join("payload").join("RecoveryTask.env"),
        pending.task_dir.join("payload").join("task.json"),
        pending.task_dir.join("manifest.json"),
        pending.task_dir.join("original").join("Winre.wim"),
        pending.task_dir.join("stage").join("Winre.wim"),
    ] {
        if !required.is_file() {
            append_log(
                &log,
                &format!(
                    "Pending boot recovery refused: required artifact is missing: {}",
                    required.display()
                ),
            )?;
            release_boot_resume_attempt(&pending.task_dir, pending.stage)?;
            return Ok(false);
        }
    }

    let planned = total_attempt_count(&pending.task_dir).saturating_add(1);
    append_log(
        &log,
        &crate::i18n::msg_tpl(
            "resume_detected",
            &[
                &pending.task_id,
                &format!("{:?}", pending.operation),
                &format!("{:?}", pending.stage),
                &planned.to_string(),
                &MAX_TOTAL_RESUME_ATTEMPTS.to_string(),
            ],
        ),
    )?;
    let Some(entry) = crate::boot_entry::ReBootEntry::read(&pending.task_dir)? else {
        append_log(
            &log,
            "Pending boot recovery abandoned: task has no recorded boot entry (old WinRE-channel task)",
        )?;
        release_boot_resume_attempt(&pending.task_dir, pending.stage)?;
        return Ok(false);
    };
    if let Err(error) = crate::boot_entry::rearm(&entry, &log) {
        append_log(
            &log,
            &format!("Pending boot recovery abandoned: our boot entry is unusable: {error}"),
        )?;
        release_boot_resume_attempt(&pending.task_dir, pending.stage)?;
        return Ok(false);
    }
    append_log(&log, "Re-armed our one-shot boot entry for pending task")?;
    let shutdown = run_output_logged("shutdown.exe", &["/r", "/t", "0"], &log)?;
    if !shutdown.status.success() {
        let tail = output_tail_summary(&shutdown, 6);
        let detail = if tail.is_empty() {
            String::new()
        } else {
            format!("：{tail}")
        };
        append_log(
            &log,
            &format!(
                "Pending boot recovery failed: shutdown exited {}{}",
                shutdown.status, detail
            ),
        )?;
        release_boot_resume_attempt(&pending.task_dir, pending.stage)?;
        return Err(err(&format!(
            "unable to restart into Windows RE for pending task{}",
            detail
        )));
    }
    let count = bump_total_attempt(&pending.task_dir)?;
    append_log(
        &log,
        &crate::i18n::msg_tpl(
            "resume_restart_requested",
            &[&count.to_string(), &MAX_TOTAL_RESUME_ATTEMPTS.to_string()],
        ),
    )?;
    Ok(true)
}

#[cfg(windows)]
pub(crate) fn defer_pending_task(pending: &PendingResume) -> Result<(), TaskError> {
    append_log(
        &pending.task_dir.join("prepare.log"),
        &crate::i18n::msg_tpl(
            "resume_declined",
            &[&pending.task_id, &format!("{:?}", pending.stage)],
        ),
    )
}

/// 显式放弃任务后的两步收口回执：
/// 1. 任务本身是否已标记终态并摘除一次性启动锁定（未修改破坏卷阶段或已 Failed）；
/// 2. BCD 备份回导等补偿步骤是否完整完成，未完成时保留警告供界面/命令行如实呈现。
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AbandonOutcome {
    pub task_id: String,
    pub log_path: String,
    pub already_failed: bool,
    pub bcd_warning: Option<String>,
    pub staging_cleaned: bool,
}

/// 把两步放弃结果格式化为界面提示正文，返回 `(提示文案, 是否带警告)`。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn format_abandon_feedback(outcome: &AbandonOutcome, english: bool) -> (String, bool) {
    if let Some(warning) = &outcome.bcd_warning {
        let text = if english {
            format!(
                "Step 1/2 (Done): Task {} was voided and the one-shot recovery boot lock was cleared. This machine will no longer auto-boot into the recovery environment for it.\n\nStep 2/2 (Warning): BCD snapshot compensation was incomplete ({warning}). If your boot menu looks unexpected, run Boot Repair (M) or inspect:\n{}",
                outcome.task_id, outcome.log_path
            )
        } else {
            format!(
                "第 1 步（已完成）：任务 {} 已作废并解除一次性重启锁定，本机不会再为它自动进入恢复环境。\n\n第 2 步（存在警告）：BCD 快照补偿未能完整恢复（{warning}）。如发现启动菜单异常，请使用「引导修复(M)」或查看日志：\n{}",
                outcome.task_id, outcome.log_path
            )
        };
        return (text, true);
    }
    let text = if outcome.already_failed {
        if english {
            format!(
                "Step 1/2 (Done): Failed task {} boot lock was cleared and temporary artifacts cleaned.\nStep 2/2 (Done): Evidence records kept (log: {}).",
                outcome.task_id, outcome.log_path
            )
        } else {
            format!(
                "第 1 步（已完成）：已解除失败任务 {} 的重启锁定并清理临时产物。\n第 2 步（已完成）：诊断证据已保留（日志：{}）。",
                outcome.task_id, outcome.log_path
            )
        }
    } else if english {
        format!(
            "Step 1/2 (Done): Task {} abandoned and marked terminal; this machine will no longer reboot into the recovery environment for it.\nStep 2/2 (Done): BCD compensation and staging cleanup completed (evidence kept: {}).",
            outcome.task_id, outcome.log_path
        )
    } else {
        format!(
            "第 1 步（已完成）：已作废并收口任务 {}，本机不会再为它重启进恢复环境。\n第 2 步（已完成）：BCD 补偿与暂存清理全部完成（诊断证据保留在：{}）。",
            outcome.task_id, outcome.log_path
        )
    };
    (text, false)
}

#[cfg(windows)]
pub(crate) fn abandon_pending_task(pending: &PendingResume) -> Result<AbandonOutcome, TaskError> {
    append_log(
        &pending.task_dir.join("prepare.log"),
        &crate::i18n::msg_tpl(
            "resume_abandoned",
            &[&pending.task_id, &format!("{:?}", pending.stage)],
        ),
    )?;
    crate::environment_admin::abandon_task_with_outcome(
        pending.workspace.to_string_lossy().into_owned(),
        pending.task_id.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::{MAX_TOTAL_RESUME_ATTEMPTS, allow_total_attempt, claim_boot_resume_attempt};

    #[test]
    fn allows_exactly_the_configured_number_of_reboots() {
        assert!(allow_total_attempt(0, MAX_TOTAL_RESUME_ATTEMPTS));
        assert!(allow_total_attempt(1, MAX_TOTAL_RESUME_ATTEMPTS));
        assert!(allow_total_attempt(2, MAX_TOTAL_RESUME_ATTEMPTS));
        assert!(
            !allow_total_attempt(3, MAX_TOTAL_RESUME_ATTEMPTS),
            "第 4 次必须被挡住"
        );
        assert_eq!(MAX_TOTAL_RESUME_ATTEMPTS, 3);
    }

    #[test]
    fn zero_max_forbids_every_attempt() {
        assert!(!allow_total_attempt(0, 0));
        assert!(!allow_total_attempt(5, 0));
    }

    #[test]
    fn automatic_boot_resume_is_limited_to_once_per_durable_stage() {
        let root = std::env::temp_dir().join(format!(
            "backuprestore-resume-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).unwrap();

        assert!(claim_boot_resume_attempt(&root, backuprestore_core::Stage::BootRequested).unwrap());
        assert!(!claim_boot_resume_attempt(&root, backuprestore_core::Stage::BootRequested).unwrap());
        assert!(claim_boot_resume_attempt(&root, backuprestore_core::Stage::RecoveryStarted).unwrap());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interruption_resume_accepts_only_staged_nonterminal_tasks() {
        use super::stage_resumable_after_interruption;
        use backuprestore_core::Stage;
        assert!(!stage_resumable_after_interruption(Stage::Prepared));
        assert!(stage_resumable_after_interruption(Stage::BootRequested));
        assert!(stage_resumable_after_interruption(Stage::TargetErased));
        assert!(stage_resumable_after_interruption(Stage::ImageApplied));
        assert!(stage_resumable_after_interruption(Stage::BootRepaired));
        assert!(!stage_resumable_after_interruption(Stage::Success));
        assert!(!stage_resumable_after_interruption(Stage::Failed));
    }

    #[test]
    fn abandon_feedback_reports_two_steps_and_surfaces_bcd_warning() {
        use super::{AbandonOutcome, format_abandon_feedback};

        let clean = AbandonOutcome {
            task_id: "task-123".into(),
            log_path: r"H:\brwork\tasks\task-123\prepare.log".into(),
            already_failed: false,
            bcd_warning: None,
            staging_cleaned: true,
        };
        let (zh_ok, warn_zh) = format_abandon_feedback(&clean, false);
        assert!(!warn_zh);
        assert!(zh_ok.contains("第 1 步（已完成）") && zh_ok.contains("第 2 步（已完成）"));
        let (en_ok, warn_en) = format_abandon_feedback(&clean, true);
        assert!(!warn_en);
        assert!(en_ok.contains("Step 1/2 (Done)") && en_ok.contains("Step 2/2 (Done)"));

        let partial = AbandonOutcome {
            bcd_warning: Some("BCD locked by BFSVC".into()),
            ..clean
        };
        let (zh_warn, is_warn_zh) = format_abandon_feedback(&partial, false);
        assert!(is_warn_zh);
        assert!(zh_warn.contains("第 1 步（已完成）") && zh_warn.contains("第 2 步（存在警告）"));
        assert!(zh_warn.contains("BCD locked by BFSVC"));
        let (en_warn, is_warn_en) = format_abandon_feedback(&partial, true);
        assert!(is_warn_en);
        assert!(en_warn.contains("Step 1/2 (Done)") && en_warn.contains("Step 2/2 (Warning)"));
        assert!(en_warn.contains("Boot Repair (M)"));
    }

    /// 守卫对「正在准备中的任务」的处理（2026-11-24 实机缺陷的回归测试，只能在 Windows 上跑）。
    /// 变异验证：把 `collect_orphan_tasks_in` 里的在办检查删掉，第一条断言必须变红。
    #[cfg(windows)]
    mod orphan_guard {
        use super::super::collect_orphan_tasks_in;
        use backuprestore_core::{
            BootMode, BootPlan, DestinationSpec, Operation, Stage, Task, TaskStore, VolumeIdentity,
        };

        fn identity(value: &str, size: u64) -> VolumeIdentity {
            let mut i = VolumeIdentity::new("disk", value);
            i.volume_guid = format!("volume-{value}");
            i.partition_type_guid = "{00000000-0000-0000-0000-000000000000}".into();
            i.disk_number = Some(0);
            i.partition_number = Some(1);
            i.partition_size = size;
            i.filesystem = "NTFS".into();
            i
        }

        fn backup_task() -> Task {
            let mut task = Task::new(
                Operation::Backup,
                BootPlan {
                    mode: BootMode::ReturnExisting,
                    previous_bcd_sha256: Some("a".repeat(64)),
                    menu_name: None,
                    boot_sequence_requested: true,
                },
            );
            task.source = Some(identity("source", 100));
            task.workspace_volume = Some(identity("task", 100));
            task.destination = Some(DestinationSpec {
                volume: identity("image", 100),
                absolute_path: None,
                relative_path: "MyBackup/Windows.wim".into(),
            });
            task
        }

        #[test]
        fn a_task_that_is_still_being_prepared_is_left_alone_and_collected_only_afterwards() {
            let root = std::env::temp_dir().join(format!(
                "backuprestore-orphan-guard-{}-{}",
                std::process::id(),
                chrono::Utc::now()
                    .timestamp_nanos_opt()
                    .unwrap_or_default()
            ));
            let store = TaskStore::new(&root);
            let task = backup_task();
            store.create(&task).unwrap();
            let dir = store.task_dir(&task.task_id).unwrap();
            // 准备做到一半的样子：目录都建了、载荷拷了一部分、启动项记录还没写。
            for name in ["mount", "original", "payload", "stage"] {
                std::fs::create_dir_all(dir.join(name)).unwrap();
            }
            std::fs::write(dir.join("payload").join("Recovery.exe"), b"half-written").unwrap();
            let log = root.join("gui.log");

            // 有进程握着在办锁：守卫必须一个都不收，任务与材料原样保留。
            let guard = crate::prepare_lock::hold(&dir).unwrap();
            assert_eq!(
                collect_orphan_tasks_in(&root, &log).unwrap(),
                0,
                "在办任务不能被收口"
            );
            assert_eq!(store.load(&task.task_id).unwrap().status, Stage::Prepared);
            assert!(dir.join("payload").join("Recovery.exe").is_file());
            assert!(dir.join("mount").is_dir());
            let written = std::fs::read_to_string(&log).unwrap();
            assert!(
                written.contains("is still being prepared; left alone"),
                "守卫必须留下「仍在准备、没动」的日志，实际写了：{written}"
            );
            assert!(
                !written.contains("marking Failed"),
                "在办任务不得被标失败，实际写了：{written}"
            );

            // 准备结束（进程崩溃时系统同样会放手）：没人握锁的孤儿必须照旧收口。
            drop(guard);
            assert_eq!(
                collect_orphan_tasks_in(&root, &log).unwrap(),
                1,
                "没人握锁的孤儿必须照旧收口"
            );
            assert_eq!(store.load(&task.task_id).unwrap().status, Stage::Failed);
            assert!(!dir.join("payload").exists());
            assert!(!dir.join("mount").exists());
            std::fs::remove_dir_all(&root).unwrap();
        }
    }
}
