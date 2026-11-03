//! 挂起引导任务与断电续跑决策控制器（Pending Task & Boot Resume Guard）。
//!
//! 负责：
//! - 宿主开机/程序启动时探测是否存在中断未完成的恢复任务（`find_pending_boot_task`）；
//! - 单阶段单次重试防打转标记（`claim_boot_resume_attempt` / `release_boot_resume_attempt`）；
//! - 跨阶段自动续跑总次数闸门（`allow_total_attempt` / `bump_total_attempt`）；
//! - 孤儿僵尸任务自动收口清理（`collect_orphan_tasks`）；
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
#[cfg(windows)]
use backuprestore_core::{Operation, Stage, StatusRecord, TaskStore, read_json};

#[cfg_attr(not(windows), allow(unused_imports))]
use crate::err;
#[cfg(windows)]
use crate::append_log;
#[cfg(windows)]
use crate::process_runner::{output_tail_summary, run_output_logged};
#[cfg(windows)]
use crate::recovery_engine::complete_recovery_cleanup;
#[cfg(windows)]
use crate::stage_resumable_after_interruption;
#[cfg(windows)]
use crate::windows_prepare;

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
    let tasks_root = workspace.join("tasks");
    if !tasks_root.is_dir() {
        return Ok(0);
    }
    let store = TaskStore::new(workspace);
    let log = crate::log_root::logs_dir().join("gui.log");
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
        let _ = crate::append_log(
            &log,
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

#[cfg(windows)]
pub(crate) fn abandon_pending_task(pending: &PendingResume) -> Result<(), TaskError> {
    append_log(
        &pending.task_dir.join("prepare.log"),
        &crate::i18n::msg_tpl(
            "resume_abandoned",
            &[&pending.task_id, &format!("{:?}", pending.stage)],
        ),
    )?;
    windows_prepare::abandon_task(
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
}
