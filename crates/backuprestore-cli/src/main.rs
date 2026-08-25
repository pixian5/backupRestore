#![cfg_attr(windows, windows_subsystem = "windows")]

//! Small Windows front-end/recovery host entry point.
//!
//! The GUI can call this binary for task validation and execution.  Keeping
//! the recovery path as a command-line program also makes it usable from
//! WinRE's `winpeshl.ini` without depending on a desktop runtime.

#[cfg(windows)]
use backuprestore_core::{BootMode, Operation, verify_image_file};
use backuprestore_core::{Stage, StatusRecord, Task, TaskError, TaskStore, read_json, sha256_file};
use chrono::Utc;
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
#[cfg(windows)]
use std::io::Read;
use std::io::Write;
#[cfg(windows)]
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use std::process::{ChildStderr, ChildStdout, Stdio};
#[cfg(windows)]
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
mod native_gui;
#[cfg(windows)]
mod windows_prepare;

fn usage() -> ! {
    eprintln!(
        "BackupRestore commands:\n  validate-task <task.json>\n  hash <file>\n  status <task-root> <task-id>\n  prepare --operation <probe|backup|restore-existing|create-secondary> --source-drive <letter> --target-drive <letter> [--image-path <absolute-wim>] [--wim-index <n>] [--boot-menu-name <name>] [--allow-destructive] [--no-reboot]\n  prepare ... --test-efi-drive <letter>  (development test only)\n  list-volumes\n  inspect-environment\n  wim-info <absolute-wim>\n  recover <task-root> <task-id> [--dry-run] [--efi-root <mounted EFI root>]\n  recover-env <RecoveryTask.env>\n  run-command <program> [args...]\n"
    );
    std::process::exit(2)
}

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.is_empty() && should_launch_gui() {
        if let Err(error) = launch_gui() {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        return;
    }
    let mut args = arguments.into_iter();
    let result = match args.next().as_deref() {
        Some("validate-task") => args
            .next()
            .ok_or_else(|| err("task.json is required"))
            .and_then(validate_task),
        Some("hash") => args
            .next()
            .ok_or_else(|| err("file is required"))
            .and_then(|p| {
                sha256_file(p).map(|h| {
                    println!("{h}");
                })
            }),
        Some("status") => {
            let root = args.next().ok_or_else(|| err("task root is required"));
            let id = args.next().ok_or_else(|| err("task id is required"));
            root.and_then(|r| id.map(|i| (r, i)))
                .and_then(|(r, i)| show_status(r, i))
        }
        #[cfg(windows)]
        Some("prepare") => windows_prepare::prepare(args.collect()),
        #[cfg(windows)]
        Some("list-volumes") => windows_prepare::list_volumes(),
        #[cfg(windows)]
        Some("inspect-environment") => windows_prepare::inspect_environment(),
        #[cfg(windows)]
        Some("wim-info") => args
            .next()
            .ok_or_else(|| err("WIM image path is required"))
            .and_then(windows_prepare::wim_info),
        Some("recover") => {
            let root = args.next().ok_or_else(|| err("task root is required"));
            let id = args.next().ok_or_else(|| err("task id is required"));
            let options = parse_recover_options(args.collect());
            root.and_then(|r| id.map(|i| (r, i)))
                .and_then(|(r, i)| options.map(|options| (r, i, options)))
                .and_then(|(r, i, options)| recover(r, i, options))
        }
        Some("recover-env") => args
            .next()
            .ok_or_else(|| err("RecoveryTask.env is required"))
            .and_then(recover_env),
        Some("run-command") => {
            let program = args.next().ok_or_else(|| err("program is required"));
            program.and_then(|p| run_command(&p, args.collect()))
        }
        _ => usage(),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[derive(Default)]
struct RecoverOptions {
    dry_run: bool,
    efi_root: Option<PathBuf>,
}

fn parse_recover_options(arguments: Vec<String>) -> Result<RecoverOptions, TaskError> {
    let mut options = RecoverOptions::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--dry-run" => options.dry_run = true,
            "--efi-root" => {
                let root = arguments
                    .next()
                    .ok_or_else(|| err("--efi-root requires a mounted EFI root path"))?;
                if root.trim().is_empty() {
                    return Err(err("--efi-root cannot be empty"));
                }
                options.efi_root = Some(PathBuf::from(root));
            }
            _ => return Err(err(&format!("unknown recover option: {argument}"))),
        }
    }
    Ok(options)
}

#[cfg(not(windows))]
fn should_launch_gui() -> bool {
    false
}

#[cfg(windows)]
fn should_launch_gui() -> bool {
    env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })
        .is_some_and(|stem| stem.eq_ignore_ascii_case("BackupRestore"))
}

#[cfg(not(windows))]
fn launch_gui() -> Result<(), TaskError> {
    Err(err("the GUI launcher is only available on Windows"))
}

#[cfg(windows)]
fn launch_gui() -> Result<(), TaskError> {
    native_gui::run()
}

fn err(message: &str) -> TaskError {
    TaskError::Invalid(message.into())
}

fn validate_task(path: String) -> Result<(), TaskError> {
    let task: Task = read_json(&path)?;
    task.validate()?;
    println!(
        "valid task {} operation={:?} status={:?}",
        task.task_id, task.operation, task.status
    );
    Ok(())
}

fn show_status(root: String, id: String) -> Result<(), TaskError> {
    let store = TaskStore::new(root);
    let task = store.load(&id)?;
    let status: StatusRecord = read_json(store.status_path(&id)?)?;
    if status.task_id != task.task_id {
        return Err(err("status file task_id does not match task.json"));
    }
    if status.operation != task.operation {
        return Err(err("status file operation does not match task.json"));
    }
    println!(
        "{}",
        serde_json::to_string_pretty::<backuprestore_core::StatusRecord>(&status)?
    );
    eprintln!("operation={:?} stage={:?}", task.operation, task.status);
    Ok(())
}

fn append_log(path: &Path, message: &str) -> Result<(), TaskError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "[{}] {message}", Utc::now().to_rfc3339())?;
    file.sync_all()?;
    Ok(())
}

fn recover(root: String, id: String, options: RecoverOptions) -> Result<(), TaskError> {
    let store = TaskStore::new(root);
    let mut task = store.load(&id)?;
    let log = store.log_path(&id)?;
    append_log(
        &log,
        &format!(
            "Recovery started operation={:?} task={}",
            task.operation, task.task_id
        ),
    )?;
    if task.status == Stage::Success {
        append_log(&log, "Task already successful; refusing to run twice")?;
        return Ok(());
    }
    if task.status == Stage::Failed {
        return Err(err(
            "task is failed; create a new task after reviewing recovery.log",
        ));
    }
    if options.dry_run {
        println!(
            "dry-run: task={} operation={:?} stage={:?}",
            task.task_id, task.operation, task.status
        );
        if let Some(image) = &task.image {
            println!(
                "image={} sha256={}",
                image
                    .absolute_path
                    .as_deref()
                    .unwrap_or(&image.relative_path),
                image.sha256
            );
        }
        if let Some(target) = &task.target {
            println!(
                "target partition={} role={:?}",
                target.volume.partition_guid, target.role
            );
        }
        append_log(&log, "Dry-run completed; no disk operation performed")?;
        return Ok(());
    }
    if !cfg!(windows) {
        return Err(err(
            "real Recovery execution is only available on Windows/WinRE; use --dry-run on this host",
        ));
    }
    if let Some(efi_root) = options.efi_root.as_deref() {
        if !efi_root.is_dir() {
            return Err(err("--efi-root must be an existing mounted EFI directory"));
        }
        append_log(
            &log,
            &format!(
                "Using explicit EFI root for direct recovery: {}",
                efi_root.display()
            ),
        )?;
    }
    let result = recover_windows(
        &store,
        &mut task,
        &log,
        options.efi_root.as_deref(),
        None,
        true,
    );
    if let Err(error) = &result {
        let _ = store.write_failure(&mut task, 1, error.to_string());
    }
    result
}

#[cfg(not(windows))]
fn recover_env(_path: String) -> Result<(), TaskError> {
    Err(err(
        "recover-env is only available on Windows/WinRE; use recover --dry-run on this host",
    ))
}

#[cfg(windows)]
fn recover_env(path: String) -> Result<(), TaskError> {
    use backuprestore_core::{PayloadManifest, validate_payload_files, validate_task_id};

    let values = read_env_file(&path)?;
    let task_id = env_required(&values, "TASK_ID")?;
    validate_task_id(&task_id)?;
    let workspace_root_rel = env_required(&values, "WORKSPACE_ROOT_REL")?;
    backuprestore_core::validate_relative_path(&workspace_root_rel)?;
    let normalized_workspace_root = workspace_root_rel.replace('/', "\\");
    let (store_rel, relative_id) = normalized_workspace_root
        .rsplit_once("\\tasks\\")
        .ok_or_else(|| err("WORKSPACE_ROOT_REL must contain \\tasks\\"))?;
    if store_rel.is_empty() || store_rel.eq_ignore_ascii_case("tasks") {
        return Err(err("WORKSPACE_ROOT_REL has an invalid workspace path"));
    }
    if relative_id != task_id {
        return Err(err("WORKSPACE_ROOT_REL task id does not match TASK_ID"));
    }
    // Until the workspace volume is mounted only an ephemeral WinRE path is
    // available.  Switch to the task directory immediately after mounting;
    // all task/recovery logs that survive WinRE are stored there.
    let mut early_log = PathBuf::from(r"X:\BackupRestore-Recovery-early.log");
    let task_letter = mount_env_volume(&values, "WORKSPACE", 'T', &early_log)?;
    let store = TaskStore::new(PathBuf::from(format!(r"{}:\{store_rel}", task_letter)));
    let task_dir = store.task_dir(&task_id)?;
    early_log = task_dir.join("Recovery-early.log");
    let mut cleanup_guard = WinreRestoreGuard::new(&values, &task_dir, &early_log, task_letter);
    let mut task = store.load(&task_id)?;
    if matches!(task.status, Stage::Success | Stage::Failed) {
        return Err(err(&format!(
            "task is already terminal at stage {:?}; refusing to run it again",
            task.status
        )));
    }
    let persisted_status: StatusRecord = read_json(store.status_path(&task_id)?)?;
    if !persisted_status.task_id.eq_ignore_ascii_case(&task.task_id) {
        return Err(err("status.json task_id does not match task.json"));
    }
    if persisted_status.operation != task.operation {
        return Err(err("status.json operation does not match task.json"));
    }
    // The normal-host preparation records boot-requested in status.json
    // without rewriting the payload task.json. Accept exactly that one-step
    // ahead state; any other divergence indicates a torn or tampered task.
    if persisted_status.stage != task.status
        && !(task.status == Stage::Prepared && persisted_status.stage == Stage::BootRequested)
    {
        return Err(err("status.json stage does not match task.json"));
    }
    verify_task_identity_env(&values, &task)?;
    let manifest: PayloadManifest = read_json(task_dir.join("manifest.json"))?;
    if !manifest.task_id.eq_ignore_ascii_case(&task.task_id) {
        return Err(err("payload manifest task_id does not match task.json"));
    }
    let launcher = task_dir.join("payload").join("RecoveryLauncher.cmd");
    let recovery_exe = task_dir.join("payload").join("Recovery.exe");
    let task_json = task_dir.join("payload").join("task.json");
    let recovery_task_env = task_dir.join("payload").join("RecoveryTask.env");
    let original = task_dir.join("original").join("Winre.wim");
    let staged = task_dir.join("stage").join("Winre.wim");
    if !recovery_exe.is_file() {
        return Err(err("Recovery.exe is required for every WinRE operation"));
    }
    validate_payload_files(
        &manifest,
        &launcher,
        &recovery_exe,
        &task_json,
        &recovery_task_env,
        &original,
        &staged,
    )?;

    let recovery_letter = mount_env_volume(&values, "RECOVERY", 'R', &early_log)?;
    let efi_letter = if task.operation != Operation::Probe {
        let source_letter = mount_env_volume(&values, "SOURCE", 'S', &early_log)?;
        let image_letter = mount_env_volume(&values, "IMAGE", 'I', &early_log)?;
        if let Some(source) = task.source.as_mut() {
            source.drive_letter = Some(source_letter);
        }
        if let Some(image) = task.image.as_mut() {
            image.volume.drive_letter = Some(image_letter);
        }
        if let Some(destination) = task.destination.as_mut() {
            destination.volume.drive_letter = Some(image_letter);
        }
        if task.target.is_some() {
            let target_letter = mount_env_volume(&values, "TARGET", 'W', &early_log)?;
            // WinRE may reuse E: for an image/data volume; keep EFI on a
            // late temporary letter to avoid mount collisions.
            let efi_letter = mount_env_volume(&values, "EFI", 'Z', &early_log)?;
            if let Some(target) = task.target.as_mut() {
                target.volume.drive_letter = Some(target_letter);
            }
            Some(efi_letter)
        } else {
            None
        }
    } else if let Some(source) = task.source.as_mut() {
        // Probe tasks are allowed to keep their task files on the source
        // partition.  That partition is already mounted as T: above, and
        // assigning a second letter in WinRE is unreliable (and can fail
        // after diskpart has partially changed the mount state).  Reuse the
        // verified task mount instead of trying to mount it again as S:.
        let workspace_volume = task
            .workspace_volume
            .as_ref()
            .ok_or_else(|| err("probe task is missing workspace volume identity"))?;
        if !workspace_volume.same_partition(source) {
            source.drive_letter = Some(mount_env_volume(&values, "SOURCE", 'S', &early_log)?);
        } else {
            source.drive_letter = Some(task_letter);
            append_log(
                &early_log,
                &format!("Probe source is the task partition; reusing {task_letter}:"),
            )?;
        }
        None
    } else {
        None
    };

    let log = store.log_path(&task_id)?;
    append_log(
        &log,
        &format!(
            "Recovery.exe started from env task={} operation={:?}",
            task_id, task.operation
        ),
    )?;
    let efi_root = efi_letter.map(|letter| PathBuf::from(format!(r"{}:\", letter)));
    let stage_before_failure = task.status;
    // WinRE cleanup is part of the recovery contract.  Defer the terminal
    // success write until the original registered WinRE image has been
    // restored and verified below.
    let result = recover_windows(
        &store,
        &mut task,
        &log,
        efi_root.as_deref(),
        Some(&values),
        false,
    );
    if let Err(error) = &result {
        let should_rollback_bcd =
            task.status == Stage::BootRepaired || stage_before_failure == Stage::BootRepaired;
        let _ = store.write_failure(&mut task, 1, error.to_string());
        append_log(&log, &format!("Recovery failed: {error}"))?;
        if should_rollback_bcd {
            if let Err(rollback) = restore_bcd_snapshot(&task_dir, efi_root.as_deref(), &log) {
                append_log(&log, &format!("BCD rollback failed: {rollback}"))?;
            }
        }
    }
    let cleanup = restore_original_winre(&values, &task_dir, &log, recovery_letter);
    if let Err(error) = &cleanup {
        append_log(&log, &format!("WinRE cleanup failed: {error}"))?;
    }
    match (result, cleanup) {
        (Ok(()), Ok(())) => {
            store.write_transition(&mut task, Stage::Success)?;
            append_log(&log, "WinRE cleanup completed; task marked successful")?;
            cleanup_guard.disarm();
            run_logged("wpeutil.exe", &["reboot"], &log)?;
            Ok(())
        }
        (Ok(()), Err(cleanup_error)) => {
            // The disk operation may have completed, but leaving the
            // registered WinRE image modified is not a successful task.
            // Keep the task terminally failed until an operator repairs the
            // registered WinRE state; the guard still gets one final
            // best-effort restore on drop.
            if let Err(status_error) = store.write_failure(
                &mut task,
                2,
                format!("WinRE cleanup failed: {cleanup_error}"),
            ) {
                append_log(
                    &log,
                    &format!("Unable to persist WinRE cleanup failure: {status_error}"),
                )?;
            }
            Err(cleanup_error)
        }
        (Err(recovery_error), cleanup_result) => {
            if let Err(cleanup_error) = cleanup_result {
                append_log(
                    &log,
                    &format!("WinRE cleanup also failed after recovery error: {cleanup_error}"),
                )?;
            } else {
                cleanup_guard.disarm();
            }
            Err(recovery_error)
        }
    }
}

#[cfg(windows)]
struct WinreRestoreGuard<'a> {
    values: &'a BTreeMap<String, String>,
    task_dir: &'a Path,
    log: &'a Path,
    recovery_letter: char,
    active: bool,
}

#[cfg(windows)]
impl<'a> WinreRestoreGuard<'a> {
    fn new(
        values: &'a BTreeMap<String, String>,
        task_dir: &'a Path,
        log: &'a Path,
        recovery_letter: char,
    ) -> Self {
        Self {
            values,
            task_dir,
            log,
            recovery_letter,
            active: true,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

#[cfg(windows)]
impl Drop for WinreRestoreGuard<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ =
                restore_original_winre(self.values, self.task_dir, self.log, self.recovery_letter);
        }
    }
}

#[cfg(windows)]
fn read_env_file(path: impl AsRef<Path>) -> Result<BTreeMap<String, String>, TaskError> {
    let mut values = BTreeMap::new();
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| err("RecoveryTask.env contains a malformed line"))?;
        if key.is_empty()
            || !key.chars().enumerate().all(|(index, ch)| {
                ch == '_' || ch.is_ascii_alphanumeric() && (index > 0 || ch.is_ascii_alphabetic())
            })
        {
            return Err(err("RecoveryTask.env contains an invalid key"));
        }
        if values.insert(key.to_string(), value.to_string()).is_some() {
            return Err(err("RecoveryTask.env contains a duplicate or empty key"));
        }
    }
    Ok(values)
}

#[cfg(windows)]
fn env_required(values: &BTreeMap<String, String>, key: &str) -> Result<String, TaskError> {
    values
        .get(key)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| err(&format!("RecoveryTask.env is missing {key}")))
}

#[cfg(windows)]
fn env_u32(values: &BTreeMap<String, String>, key: &str) -> Result<u32, TaskError> {
    env_required(values, key)?
        .parse::<u32>()
        .map_err(|_| err(&format!("RecoveryTask.env {key} is not a number")))
}

#[cfg(windows)]
fn env_optional(values: &BTreeMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

#[cfg(windows)]
fn env_optional_u64(values: &BTreeMap<String, String>, key: &str) -> Option<u64> {
    env_optional(values, key).and_then(|value| value.parse::<u64>().ok())
}

#[cfg(windows)]
fn verify_task_identity_env(
    values: &BTreeMap<String, String>,
    task: &Task,
) -> Result<(), TaskError> {
    fn verify(
        values: &BTreeMap<String, String>,
        prefix: &str,
        identity: &backuprestore_core::VolumeIdentity,
    ) -> Result<(), TaskError> {
        for (suffix, expected, actual) in [
            ("VOLUME_GUID", identity.volume_guid.as_str(), "volume"),
            ("DISK_GUID", identity.disk_guid.as_str(), "disk"),
            (
                "PARTITION_GUID",
                identity.partition_guid.as_str(),
                "partition",
            ),
        ] {
            if let Some(value) = env_optional(values, &format!("{prefix}_{suffix}")) {
                if !expected.is_empty() && !expected.eq_ignore_ascii_case(&value) {
                    return Err(err(&format!(
                        "{prefix} {actual} identity differs between task.json and RecoveryTask.env"
                    )));
                }
            }
        }
        if let Some(number) = identity.disk_number {
            if let Some(expected) = env_optional_u64(values, &format!("{prefix}_DISK_NUMBER")) {
                if number as u64 != expected {
                    return Err(err(&format!(
                        "{prefix} disk number differs between task.json and RecoveryTask.env"
                    )));
                }
            }
        }
        if let Some(number) = identity.partition_number {
            if let Some(expected) = env_optional_u64(values, &format!("{prefix}_PARTITION_NUMBER"))
            {
                if number as u64 != expected {
                    return Err(err(&format!(
                        "{prefix} partition number differs between task.json and RecoveryTask.env"
                    )));
                }
            }
        }
        if identity.partition_size > 0 {
            if let Some(expected) = env_optional_u64(values, &format!("{prefix}_PARTITION_SIZE")) {
                if identity.partition_size != expected {
                    return Err(err(&format!(
                        "{prefix} partition size differs between task.json and RecoveryTask.env"
                    )));
                }
            }
        }
        if identity.partition_offset > 0 {
            if let Some(expected) = env_optional_u64(values, &format!("{prefix}_PARTITION_OFFSET"))
            {
                if identity.partition_offset != expected {
                    return Err(err(&format!(
                        "{prefix} partition offset differs between task.json and RecoveryTask.env"
                    )));
                }
            }
        }
        if !identity.partition_type_guid.trim().is_empty() {
            if let Some(expected) = env_optional(values, &format!("{prefix}_PARTITION_TYPE_GUID")) {
                if !identity.partition_type_guid.eq_ignore_ascii_case(&expected) {
                    return Err(err(&format!(
                        "{prefix} partition type differs between task.json and RecoveryTask.env"
                    )));
                }
            }
        }
        if !identity.filesystem.trim().is_empty() {
            if let Some(expected) = env_optional(values, &format!("{prefix}_FILESYSTEM")) {
                if !identity.filesystem.eq_ignore_ascii_case(&expected) {
                    return Err(err(&format!(
                        "{prefix} filesystem differs between task.json and RecoveryTask.env"
                    )));
                }
            }
        }
        if !identity.volume_serial.trim().is_empty() {
            if let Some(expected) = env_optional(values, &format!("{prefix}_VOLUME_SERIAL")) {
                if !identity.volume_serial.eq_ignore_ascii_case(&expected) {
                    return Err(err(&format!(
                        "{prefix} volume serial differs between task.json and RecoveryTask.env"
                    )));
                }
            }
        }
        Ok(())
    }

    if let Some(source) = task.source.as_ref() {
        verify(values, "SOURCE", source)?;
    }
    let workspace_volume = task
        .workspace_volume
        .as_ref()
        .ok_or_else(|| err("task.json is missing workspace_volume identity"))?;
    verify(values, "WORKSPACE", workspace_volume)?;
    match task.operation {
        Operation::Backup => {
            let destination = task
                .destination
                .as_ref()
                .ok_or_else(|| err("backup task is missing destination"))?;
            verify(values, "IMAGE", &destination.volume)?;
            verify_image_absolute_path(values, destination.absolute_path.as_deref())?;
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let image = task
                .image
                .as_ref()
                .ok_or_else(|| err("restore task is missing image"))?;
            verify(values, "IMAGE", &image.volume)?;
            verify_image_absolute_path(values, image.absolute_path.as_deref())?;
            let target = task
                .target
                .as_ref()
                .ok_or_else(|| err("restore task is missing target"))?;
            verify(values, "TARGET", &target.volume)?;
        }
        Operation::Probe => {}
    }
    Ok(())
}

#[cfg(windows)]
fn verify_image_absolute_path(
    values: &BTreeMap<String, String>,
    expected: Option<&str>,
) -> Result<(), TaskError> {
    let expected = expected.ok_or_else(|| err("task is missing image absolute path"))?;
    backuprestore_core::validate_absolute_path(expected)?;
    let actual = env_required(values, "IMAGE_ABSOLUTE_PATH")?;
    backuprestore_core::validate_absolute_path(&actual)?;
    if !expected.eq_ignore_ascii_case(&actual) {
        return Err(err(
            "image absolute path differs between task.json and RecoveryTask.env",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn mount_env_volume(
    values: &BTreeMap<String, String>,
    prefix: &str,
    letter: char,
    log: &Path,
) -> Result<char, TaskError> {
    let disk = env_u32(values, &format!("{prefix}_DISK_NUMBER"))?;
    let partition = env_u32(values, &format!("{prefix}_PARTITION_NUMBER"))?;
    let expected = env_required(values, &format!("{prefix}_VOLUME_GUID"))?;
    if let Some(existing) = find_mounted_volume(&expected) {
        append_log(
            log,
            &format!("{prefix} volume already mounted at {existing}:; reusing it"),
        )?;
        return Ok(existing);
    }
    let script = PathBuf::from(format!(
        r"C:\Windows\Temp\BackupRestore-assign-{letter}.txt"
    ));
    let existing = Command::new("mountvol")
        .arg(format!("{letter}:"))
        .arg("/L")
        .output()?;
    if existing.status.success() {
        if let Some(actual) = String::from_utf8_lossy(&existing.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
        {
            if actual.eq_ignore_ascii_case(&expected) {
                return Ok(letter);
            }
            return Err(err(&format!(
                "volume {letter}: is already mounted to {actual}, refusing to replace it"
            )));
        }
    }
    let direct_status = Command::new("mountvol.exe")
        .args([format!("{letter}:"), expected.clone()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    append_log(
        log,
        &format!("mountvol.exe direct assignment for {letter}: exited with {direct_status}"),
    )?;
    if direct_status.success() {
        verify_mounted_volume(letter, &expected)?;
        return Ok(letter);
    }
    let body =
        format!("select disk {disk}\r\nselect partition {partition}\r\nassign letter={letter}\r\n");
    fs::write(&script, body)?;
    let script_arg = script.to_string_lossy().into_owned();
    append_log(
        log,
        &format!("running diskpart.exe /s {script_arg} with file-backed output"),
    )?;
    let diskpart_log = log.with_extension("diskpart.log");
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&diskpart_log)?;
    let stderr = stdout.try_clone()?;
    let mut child = Command::new("diskpart.exe")
        .args(["/s", script_arg.as_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    thread::sleep(Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
    append_log(
        log,
        &format!(
            "diskpart.exe given 5s to apply assignment, then terminated; output={}",
            diskpart_log.display()
        ),
    )?;
    let _ = fs::remove_file(&script);
    verify_mounted_volume(letter, &expected)?;
    Ok(letter)
}

#[cfg(windows)]
fn find_mounted_volume(expected: &str) -> Option<char> {
    for letter in 'C'..='Z' {
        let Ok(output) = Command::new("mountvol.exe")
            .args([format!("{letter}:"), "/L".to_string()])
            .stdin(Stdio::null())
            .output()
        else {
            continue;
        };
        if output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .any(|line| !line.is_empty() && line.eq_ignore_ascii_case(expected))
        {
            return Some(letter);
        }
    }
    None
}

#[cfg(windows)]
fn verify_mounted_volume(letter: char, expected: &str) -> Result<(), TaskError> {
    let root = PathBuf::from(format!("{letter}:\\"));
    if !root.is_dir() {
        return Err(err(&format!(
            "volume {letter}: is not accessible after assignment"
        )));
    }
    let output = Command::new("mountvol.exe")
        .arg(format!("{letter}:"))
        .arg("/L")
        .output()?;
    if !output.status.success() {
        return Err(err(&format!("mountvol failed while verifying {letter}:")));
    }
    let output_text = String::from_utf8_lossy(&output.stdout);
    let actual = output_text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| err(&format!("volume {letter}: has no mountvol identity")))?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(err(&format!(
            "volume identity mismatch for {letter}: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn restore_original_winre(
    values: &BTreeMap<String, String>,
    task_dir: &Path,
    log: &Path,
    recovery_letter: char,
) -> Result<(), TaskError> {
    let original = task_dir.join("original").join("Winre.wim");
    let registered = PathBuf::from(format!(
        r"{}:\Recovery\WindowsRE\Winre.wim",
        recovery_letter
    ));
    if !original.exists() || !registered.parent().is_some_and(Path::exists) {
        return Err(err("original or registered WinRE image is missing"));
    }
    fs::copy(&original, &registered)?;
    let expected = env_required(values, "ORIGINAL_WINRE_SHA256")?;
    backuprestore_core::verify_sha256(&registered, &expected)?;
    append_log(log, "Original registered WinRE restored and verified")?;
    Ok(())
}

#[cfg(windows)]
fn restore_bcd_snapshot(
    task_dir: &Path,
    efi_root: Option<&Path>,
    log: &Path,
) -> Result<(), TaskError> {
    let snapshot = task_dir.join("bcd-before-export");
    if !snapshot.exists() {
        return Err(err("BCD snapshot is missing"));
    }
    let efi_store = efi_root.map(|root| root.join("EFI\\Microsoft\\Boot\\BCD"));
    if let Some(efi_store) = efi_store.filter(|path| path.exists()) {
        let snapshot_arg = snapshot.to_string_lossy().into_owned();
        let store_arg = efi_store.to_string_lossy().into_owned();
        run_logged(
            "bcdedit.exe",
            &["/store", &store_arg, "/import", &snapshot_arg],
            log,
        )?;
    } else {
        let snapshot_arg = snapshot.to_string_lossy().into_owned();
        run_logged("bcdedit.exe", &["/import", &snapshot_arg], log)?;
    }
    append_log(
        log,
        "Previous BCD snapshot imported after boot repair failure",
    )?;
    Ok(())
}

#[cfg(windows)]
fn native_windows_architecture() -> String {
    let reported = [
        env::var("PROCESSOR_ARCHITEW6432").ok(),
        env::var("PROCESSOR_ARCHITECTURE").ok(),
    ];
    if reported
        .iter()
        .flatten()
        .any(|value| value.eq_ignore_ascii_case("ARM64"))
    {
        "arm64".into()
    } else if reported
        .iter()
        .flatten()
        .any(|value| value.eq_ignore_ascii_case("AMD64"))
    {
        "x64".into()
    } else {
        "unknown".into()
    }
}

#[cfg(not(windows))]
fn recover_windows(
    _store: &TaskStore,
    _task: &mut Task,
    _log: &Path,
    _efi_root: Option<&Path>,
    _metadata_context: Option<&BTreeMap<String, String>>,
    _finalize_success: bool,
) -> Result<(), TaskError> {
    Err(err(
        "real Recovery execution is only available on Windows/WinRE",
    ))
}

#[cfg(windows)]
fn recover_windows(
    store: &TaskStore,
    task: &mut Task,
    log: &Path,
    efi_root: Option<&Path>,
    metadata_context: Option<&BTreeMap<String, String>>,
    finalize_success: bool,
) -> Result<(), TaskError> {
    use backuprestore_core::TargetRole;
    if task.status == Stage::Prepared {
        store.write_transition(task, Stage::RecoveryStarted)?;
    }
    if task.status == Stage::BootRequested {
        store.write_transition(task, Stage::RecoveryStarted)?;
    }
    if task.status == Stage::RecoveryStarted {
        store.write_transition(task, Stage::Preflight)?;
    }
    let image_path = task
        .image
        .as_ref()
        .map(|image| resolve_volume_path(&image.volume, &image.relative_path))
        .transpose()?;
    if let Some(path) = &image_path {
        let image = task.image.as_ref().expect("image path implies image");
        verify_image_file(path, image)?;
        if matches!(
            task.operation,
            Operation::RestoreExisting | Operation::CreateSecondary
        ) {
            let metadata_path = path
                .parent()
                .ok_or_else(|| err("image path has no parent"))?
                .join("metadata.json");
            let metadata: serde_json::Value = read_json(&metadata_path).map_err(|error| {
                err(&format!(
                    "backup metadata is required and must be valid: {error}"
                ))
            })?;
            let metadata_hash = metadata
                .get("imageSha256")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| err("backup metadata has no imageSha256"))?;
            if !metadata_hash.eq_ignore_ascii_case(&image.sha256) {
                return Err(err("image hash does not match backup metadata"));
            }
            let target = task.target.as_ref().ok_or_else(|| err("missing target"))?;
            let explicit_minimum = metadata
                .get("minimumTargetSize")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let source_size = metadata
                .get("source")
                .and_then(|source| source.get("partitionSize"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let captured = metadata
                .get("capturedUsedBytes")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let reserved = metadata
                .get("reservedBytes")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let required = explicit_minimum
                .max(source_size)
                .max(captured.saturating_add(reserved));
            if required == 0 || target.volume.partition_size < required {
                return Err(err(&format!(
                    "target partition is too small: {} < {}",
                    target.volume.partition_size, required
                )));
            }
        }
        run_logged(
            "dism.exe",
            &["/Get-WimInfo", &format!("/WimFile:{}", path.display())],
            log,
        )?;
    }
    match task.operation {
        Operation::Probe => {
            append_log(log, "Probe task validated; no disk operation requested")?;
            if task.status == Stage::RecoveryStarted {
                store.write_transition(task, Stage::Preflight)?;
            }
            if task.status == Stage::Preflight && finalize_success {
                store.write_transition(task, Stage::Success)?;
            }
        }
        Operation::Backup => {
            use backuprestore_core::{BackupMetadata, PROGRAM_VERSION, write_json_atomic};
            let source = task.source.clone().ok_or_else(|| err("missing source"))?;
            let destination = task
                .destination
                .clone()
                .ok_or_else(|| err("missing destination"))?;
            let source_path = resolve_volume_root(&source)?;
            let destination_path =
                resolve_volume_path(&destination.volume, &destination.relative_path)?;
            let partial = PathBuf::from(format!("{}.partial", destination_path.display()));
            if let Some(parent) = partial.parent() {
                fs::create_dir_all(parent)?;
            }
            match task.status {
                Stage::Preflight => {
                    store.write_transition(task, Stage::Capturing)?;
                }
                Stage::Capturing => {
                    append_log(
                        log,
                        "Resuming backup from capturing stage; replacing partial image",
                    )?;
                }
                stage => return Err(err(&format!("backup cannot resume from stage {stage:?}"))),
            }
            // A power loss can leave a partial WIM behind.  DISM does not
            // reliably overwrite an existing partial image, so remove only
            // this task-owned temporary file before restarting capture.
            let _ = fs::remove_file(&partial);
            run_logged(
                "dism.exe",
                &[
                    "/Capture-Image",
                    &format!("/ImageFile:{}", partial.display()),
                    &format!("/CaptureDir:{}", source_path.display()),
                    "/Name:Windows Backup",
                    "/Compress:max",
                    "/CheckIntegrity",
                ],
                log,
            )?;
            run_logged(
                "dism.exe",
                &["/Get-WimInfo", &format!("/WimFile:{}", partial.display())],
                log,
            )?;
            let _ = fs::remove_file(&destination_path);
            fs::rename(&partial, &destination_path)?;
            let image_size = fs::metadata(&destination_path)?.len();
            let image_sha256 = backuprestore_core::sha256_file(&destination_path)?;
            let source_volume_serial = source.volume_serial.clone();
            let source_partition_size = source.partition_size;
            let context_value = |key: &str, fallback: &str| {
                metadata_context
                    .and_then(|values| env_optional(values, key))
                    .unwrap_or_else(|| fallback.to_string())
            };
            let context_u64 = |key: &str, fallback: u64| {
                metadata_context
                    .and_then(|values| env_optional_u64(values, key))
                    .unwrap_or(fallback)
            };
            let metadata = BackupMetadata {
                version: 1,
                image_type: "wim".into(),
                created: Utc::now(),
                computer: context_value("COMPUTERNAME", "WinRE"),
                windows_edition: context_value("WINDOWS_EDITION", "unknown"),
                architecture: context_value("WINDOWS_ARCHITECTURE", &native_windows_architecture()),
                windows_build: context_value("WINDOWS_BUILD", "unknown"),
                wim_index: 1,
                image_sha256,
                image_size,
                source,
                captured_used_bytes: context_u64("SOURCE_USED_BYTES", 0),
                reserved_bytes: context_u64("RESERVED_BYTES", 0),
                minimum_target_size: context_u64("MINIMUM_TARGET_SIZE", source_partition_size),
                volume_serial: source_volume_serial,
                program_version: PROGRAM_VERSION.into(),
            };
            let metadata_path = destination_path
                .parent()
                .ok_or_else(|| err("backup destination has no parent"))?
                .join("metadata.json");
            write_json_atomic(metadata_path, &metadata)?;
            append_log(log, "Backup metadata written and image hash recorded")?;
            if finalize_success {
                store.write_transition(task, Stage::Success)?;
            }
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let target = task.target.clone().ok_or_else(|| err("missing target"))?;
            let image_index = task.image.as_ref().map(|image| image.index).unwrap_or(1);
            if target.role == TargetRole::NewWindows
                && task.boot_plan.mode != BootMode::AddSecondary
            {
                return Err(err("secondary target requires add-secondary boot plan"));
            }
            let image_path = image_path.ok_or_else(|| err("missing image"))?;
            let target_root = resolve_volume_root(&target.volume)?;
            // TargetErased is deliberately persisted before formatting.  If
            // power fails after that write, formatting and applying the WIM
            // are safe to repeat on the explicitly selected target.  Older
            // task versions persisted ImageApplied before DISM completed, so
            // resume from ImageApplied also re-runs Apply-Image defensively.
            if matches!(task.status, Stage::Preflight | Stage::TargetErased) {
                if task.status == Stage::Preflight {
                    store.write_transition(task, Stage::TargetErased)?;
                }
                format_target_partition(store, task, &target.volume, log)?;
                if !target_root.is_dir() {
                    return Err(err(
                        "restore target is unavailable after DiskPart format operation",
                    ));
                }
            }
            if matches!(task.status, Stage::TargetErased | Stage::ImageApplied) {
                run_logged(
                    "dism.exe",
                    &[
                        "/Apply-Image",
                        &format!("/ImageFile:{}", image_path.display()),
                        &format!("/Index:{}", image_index),
                        &format!("/ApplyDir:{}", target_root.display()),
                    ],
                    log,
                )?;
                if task.status == Stage::TargetErased {
                    store.write_transition(task, Stage::ImageApplied)?;
                }
            }
            let efi = find_efi_root(efi_root)?;
            if matches!(task.status, Stage::ImageApplied | Stage::BootRepaired) {
                // BootRepaired is recorded immediately before BCDBoot so a
                // failure is eligible for BCD rollback.  A retry from that
                // stage simply re-runs BCDBoot and validates its output.
                if task.status == Stage::ImageApplied {
                    store.write_transition(task, Stage::BootRepaired)?;
                }
                let windows_root = target_root.join("Windows");
                let mut bcd_args = vec![
                    windows_root.to_string_lossy().into_owned(),
                    "/s".to_string(),
                    efi.to_string_lossy().into_owned(),
                    "/f".to_string(),
                    "UEFI".to_string(),
                    "/v".to_string(),
                ];
                if target.role == TargetRole::NewWindows {
                    bcd_args.push("/addlast".to_string());
                }
                let bcd_refs: Vec<&str> = bcd_args.iter().map(String::as_str).collect();
                run_logged("bcdboot.exe", &bcd_refs, log)?;
                let boot_manager = efi.join("EFI\\Microsoft\\Boot\\bootmgfw.efi");
                if !boot_manager.exists() {
                    return Err(err("BCDBoot reported success but bootmgfw.efi is missing"));
                }
                if target.role == TargetRole::NewWindows {
                    let menu_name = target
                        .boot_menu_name
                        .as_deref()
                        .ok_or_else(|| err("secondary target has no boot menu name"))?;
                    set_secondary_boot_menu(&efi, &target_root, menu_name, log)?;
                }
                if finalize_success {
                    store.write_transition(task, Stage::Success)?;
                }
            } else {
                return Err(err(&format!(
                    "restore cannot resume from stage {:?}",
                    task.status
                )));
            }
        }
    }
    append_log(log, "Recovery completed")?;
    Ok(())
}

#[cfg(windows)]
fn resolve_volume_root(volume: &backuprestore_core::VolumeIdentity) -> Result<PathBuf, TaskError> {
    volume
        .drive_letter
        .map(|letter| PathBuf::from(format!("{}:\\", letter)))
        .ok_or_else(|| {
            err("Recovery volume has no resolved drive letter; front end must record or assign one")
        })
}

#[cfg_attr(not(windows), allow(dead_code))]
fn diskpart_format_script(
    disk_number: Option<u32>,
    partition_number: Option<u32>,
) -> Result<String, TaskError> {
    let disk_number = disk_number
        .ok_or_else(|| err("restore target has no verified disk number for DiskPart formatting"))?;
    let partition_number = partition_number.ok_or_else(|| {
        err("restore target has no verified partition number for DiskPart formatting")
    })?;
    if partition_number == 0 {
        return Err(err(
            "restore target has an invalid partition number for DiskPart",
        ));
    }
    Ok(format!(
        "select disk {disk_number}\r\nselect partition {partition_number}\r\nformat fs=ntfs quick\r\n"
    ))
}

#[cfg(windows)]
fn format_target_partition(
    store: &TaskStore,
    task: &Task,
    target: &backuprestore_core::VolumeIdentity,
    log: &Path,
) -> Result<(), TaskError> {
    let task_dir = store.task_dir(&task.task_id)?;
    let script = task_dir.join("diskpart-format.txt");
    let body = diskpart_format_script(target.disk_number, target.partition_number)?;
    fs::write(&script, body)?;
    let script_arg = script.to_string_lossy().into_owned();
    append_log(
        log,
        &format!(
            "Formatting verified target with diskpart.exe: disk={} partition={} script={script_arg}",
            target.disk_number.unwrap_or_default(),
            target.partition_number.unwrap_or_default(),
        ),
    )?;
    run_logged("diskpart.exe", &["/s", script_arg.as_str()], log)
}
#[cfg(windows)]
fn resolve_volume_path(
    volume: &backuprestore_core::VolumeIdentity,
    relative: &str,
) -> Result<PathBuf, TaskError> {
    Ok(resolve_volume_root(volume)?.join(relative.replace('\\', std::path::MAIN_SEPARATOR_STR)))
}
#[cfg(windows)]
fn find_efi_root(override_root: Option<&Path>) -> Result<PathBuf, TaskError> {
    override_root
        .map(Path::to_path_buf)
        .ok_or_else(|| err("EFI root must be explicitly mounted before recovery"))
}
#[cfg(windows)]
fn run_logged(program: &str, args: &[&str], log: &Path) -> Result<(), TaskError> {
    append_log(log, &format!("running {program} {}", args.join(" ")))?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let log_file = OpenOptions::new().create(true).append(true).open(log)?;
    let sink = Arc::new(Mutex::new(log_file));
    let stdout: ChildStdout = child
        .stdout
        .take()
        .ok_or_else(|| err(&format!("{program} stdout was not piped")))?;
    let stderr: ChildStderr = child
        .stderr
        .take()
        .ok_or_else(|| err(&format!("{program} stderr was not piped")))?;
    let stdout_sink = Arc::clone(&sink);
    let stdout_thread = thread::spawn(move || stream_to_log("stdout", stdout, stdout_sink));
    let stderr_sink = Arc::clone(&sink);
    let stderr_thread = thread::spawn(move || stream_to_log("stderr", stderr, stderr_sink));
    let status = child.wait()?;
    stdout_thread
        .join()
        .map_err(|_| err(&format!("{program} stdout reader panicked")))??;
    stderr_thread
        .join()
        .map_err(|_| err(&format!("{program} stderr reader panicked")))??;
    if !status.success() {
        return Err(err(&format!("{program} failed with {status}")));
    }
    Ok(())
}

#[cfg(windows)]
fn capture_logged(program: &str, args: &[&str], log: &Path) -> Result<String, TaskError> {
    append_log(log, &format!("capturing {program} {}", args.join(" ")))?;
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !text.is_empty() {
        append_log(log, &text)?;
    }
    if !output.status.success() {
        return Err(err(&format!("{program} failed with {}", output.status)));
    }
    Ok(text)
}

#[cfg(windows)]
fn set_secondary_boot_menu(
    efi_root: &Path,
    target_root: &Path,
    menu_name: &str,
    log: &Path,
) -> Result<(), TaskError> {
    let store = efi_root.join("EFI\\Microsoft\\Boot\\BCD");
    if !store.exists() {
        return Err(err("BCD store is missing after BCDBoot"));
    }
    let store_arg = store.to_string_lossy().into_owned();
    let output = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let letter = target_root
        .to_string_lossy()
        .chars()
        .next()
        .ok_or_else(|| err("secondary target has no drive letter"))?
        .to_ascii_lowercase();
    let target_needle = format!("partition={letter}:");
    let mut current_id: Option<String> = None;
    let mut matched = Vec::new();
    let mut os_loaders = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if (lower.starts_with("identifier") || trimmed.starts_with("标识符"))
            && trimmed.find('{').is_some()
        {
            let start = trimmed.find('{').unwrap_or(0);
            let end = trimmed[start..]
                .find('}')
                .map(|offset| start + offset + 1)
                .unwrap_or(trimmed.len());
            current_id = Some(trimmed[start..end].to_string());
        }
        if lower.contains("winload") {
            if let Some(identifier) = current_id.as_ref() {
                if !os_loaders.contains(identifier) {
                    os_loaders.push(identifier.clone());
                }
            }
        }
        if lower.contains(&target_needle) {
            if let Some(identifier) = current_id.as_ref() {
                if !matched.contains(identifier) {
                    matched.push(identifier.clone());
                }
            }
        }
    }
    let identifier = matched
        .last()
        .or_else(|| os_loaders.last())
        .ok_or_else(|| err("BCDBoot created no identifiable Windows loader"))?;
    let description_args = [
        "/store",
        store_arg.as_str(),
        "/set",
        identifier.as_str(),
        "description",
        menu_name,
    ];
    run_logged("bcdedit.exe", &description_args, log)?;
    let verify = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    if !verify
        .to_ascii_lowercase()
        .contains(&menu_name.to_ascii_lowercase())
    {
        return Err(err("BCD menu name was not visible after update"));
    }
    append_log(
        log,
        &format!("Secondary Windows loader {identifier} named {menu_name}"),
    )?;
    Ok(())
}

#[cfg(windows)]
fn stream_to_log<R: Read>(
    label: &str,
    stream: R,
    sink: Arc<Mutex<std::fs::File>>,
) -> Result<(), TaskError> {
    let mut reader = BufReader::new(stream);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        if reader.read_until(b'\n', &mut bytes)? == 0 {
            break;
        }
        // DISM/bcdboot use the Windows console code page on localized hosts;
        // stdout is not guaranteed to be UTF-8. Preserve every byte in the
        // log with replacement decoding instead of failing the recovery task
        // after the native operation has already started.
        let line = String::from_utf8_lossy(&bytes);
        {
            let mut file = sink
                .lock()
                .map_err(|_| err("recovery log lock was poisoned"))?;
            write!(file, "[{label}] {line}")?;
            file.flush()?;
        }
        print!("{label}: {line}");
    }
    Ok(())
}

fn run_command(program: &str, args: Vec<String>) -> Result<(), TaskError> {
    let status = Command::new(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(err(&format!("{program} failed with {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::{diskpart_format_script, parse_recover_options};

    #[test]
    fn recover_options_accept_explicit_efi_root() {
        let options =
            parse_recover_options(vec!["--dry-run".into(), "--efi-root".into(), "E:\\".into()])
                .expect("options should parse");
        assert!(options.dry_run);
        assert_eq!(options.efi_root.unwrap().to_string_lossy(), "E:\\");
    }

    #[test]
    fn recover_options_reject_unknown_or_incomplete_flags() {
        assert!(parse_recover_options(vec!["--unknown".into()]).is_err());
        assert!(parse_recover_options(vec!["--efi-root".into()]).is_err());
    }

    #[test]
    fn diskpart_format_script_uses_verified_target_numbers() {
        assert_eq!(
            diskpart_format_script(Some(3), Some(2)).unwrap(),
            "select disk 3\r\nselect partition 2\r\nformat fs=ntfs quick\r\n"
        );
        assert!(diskpart_format_script(None, Some(2)).is_err());
        assert!(diskpart_format_script(Some(3), None).is_err());
        assert!(diskpart_format_script(Some(3), Some(0)).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn prepare_options_reject_missing_image_or_invalid_drive() {
        assert!(
            super::windows_prepare::parse_prepare_options(vec![
                "--operation".into(),
                "probe".into(),
                "--source-drive".into(),
                "C".into(),
            ])
            .is_ok()
        );
        assert!(
            super::windows_prepare::parse_prepare_options(vec![
                "--operation".into(),
                "restore-existing".into(),
                "--source-drive".into(),
                "C".into(),
            ])
            .is_err()
        );
        assert!(
            super::windows_prepare::parse_prepare_options(vec![
                "--operation".into(),
                "probe".into(),
                "--source-drive".into(),
                "CC".into(),
            ])
            .is_err()
        );
    }
}
