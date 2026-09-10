#![cfg_attr(windows, windows_subsystem = "windows")]

//! Small Windows front-end/recovery host entry point.
//!
//! The GUI can call this binary for task validation and execution.  Keeping
//! the recovery path as a command-line program also makes it usable from
//! WinRE's `winpeshl.ini` without depending on a desktop runtime.

#[cfg(windows)]
use backuprestore_core::{BootMode, Operation, PayloadManifest, verify_image_file};
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
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use std::process::{ChildStderr, ChildStdout, Stdio};
#[cfg(windows)]
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
mod native_gui;
#[cfg(windows)]
mod windows_prepare;

fn usage() -> ! {
    eprintln!(
        "BackupRestore commands:\n  validate-task <task.json>\n  hash <file>\n  status <task-root> <task-id>\n  prepare --operation <probe|backup|restore-existing|create-secondary> --source-drive <letter> [--target-drive <letter>] [--image-path <absolute-wim>] [--wim-index <n>] [--boot-menu-name <name>] [--allow-destructive] [--no-reboot]\n  prepare ... [--test-efi-drive <letter>] [--test-fault <identity-env-mismatch|bcdboot-failure|power-loss-window>]  (development test only)\n  list-volumes\n  inspect-environment\n  wim-info <absolute-wim>\n  recover <task-root> <task-id> [--dry-run] [--efi-root <mounted EFI root>]\n  recover-env <RecoveryTask.env>\n  run-command <program> [args...]\n  --open-image <absolute-wim>  (GUI only)\n  --pe-desktop  (WinPE recovery desktop, GUI only)\n"
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
    #[cfg(windows)]
    if arguments
        .first()
        .is_some_and(|argument| argument == "--open-image")
    {
        let image = arguments
            .get(1)
            .ok_or_else(|| err("--open-image requires an absolute WIM path"));
        let result = image.and_then(|image| {
            backuprestore_core::validate_absolute_path(image)?;
            // This is a GUI launch option, not a task-preparation argument.
            // The native window consumes it once during initialization.
            unsafe { env::set_var("BACKUPRESTORE_OPEN_IMAGE", image) };
            launch_gui()
        });
        if let Err(error) = result {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    if arguments
        .first()
        .is_some_and(|argument| argument == "--pe-desktop")
    {
        // WinPE recovery desktop: full-screen shell-free landing window. The
        // desktop returns the operation tab the technician picked; hand it to
        // the main GUI so it opens directly on that page.
        let result = unsafe { native_gui::run_pe_desktop() }.and_then(|tab| {
            if let Some(tab) = tab {
                unsafe { env::set_var("BACKUPRESTORE_OPEN_TAB", tab.to_string()) };
                launch_gui()?;
            }
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    if arguments
        .first()
        .is_some_and(|argument| argument == "--pe-reboot")
    {
        // 桌面快捷方式入口：设置 bootsequence 指向已安装的 PE 恢复环境，
        // 下次重启自动进入 PE。非提升进程会先提权重启。
        let result = unsafe { native_gui::pe_reboot_standalone() };
        if let Err(error) = result {
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
        #[cfg(windows)]
        record_launch_error(&error);
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

/// Preparation can fail before a task directory exists, for example while the
/// native volume identity preflight is opening a newly attached disk. Persist
/// that diagnostic beside the program without creating a task or changing
/// WinRE/BCD, so the GUI and non-console executable do not lose the reason.
#[cfg(windows)]
fn record_launch_error(error: &TaskError) {
    let Ok(executable) = env::current_exe() else {
        return;
    };
    let Some(directory) = executable.parent() else {
        return;
    };
    let _ = append_log(
        &directory.join("logs").join("launcher-errors.log"),
        &error.to_string(),
    );
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

/// Resume a task that reached `boot-requested` but lost power before Windows
/// actually entered WinRE. The task was already explicitly authorized by the
/// user; on the next normal launch we revalidate its durable records and ask
/// Windows RE for the one-time boot again. We deliberately require exactly
/// one valid pending task and never guess when records are malformed or
/// ambiguous.
#[cfg(windows)]
pub(crate) fn resume_pending_boot_task() -> Result<bool, TaskError> {
    let executable = env::current_exe()?;
    let workspace = executable
        .parent()
        .ok_or_else(|| err("executable has no workspace directory"))?;
    let tasks_root = workspace.join("tasks");
    if !tasks_root.is_dir() {
        return Ok(false);
    }

    let mut pending = Vec::new();
    for entry in fs::read_dir(&tasks_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let task_id = entry.file_name().to_string_lossy().to_string();
        let task_path = entry.path().join("task.json");
        let status_path = entry.path().join("status.json");
        let Ok(task) = read_json::<Task>(&task_path) else {
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
            pending.push((task, entry.path()));
        }
    }

    if pending.is_empty() {
        return Ok(false);
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
        return Ok(false);
    }

    let (task, task_dir) = pending.pop().expect("pending length checked above");
    for required in [
        task_dir.join("payload").join("Recovery.exe"),
        task_dir.join("payload").join("RecoveryTask.env"),
        task_dir.join("payload").join("task.json"),
        task_dir.join("manifest.json"),
        task_dir.join("original").join("Winre.wim"),
        task_dir.join("stage").join("Winre.wim"),
    ] {
        if !required.is_file() {
            append_log(
                &task_dir.join("prepare.log"),
                &format!(
                    "Pending boot recovery refused: required artifact is missing: {}",
                    required.display()
                ),
            )?;
            return Ok(false);
        }
    }

    let log = task_dir.join("prepare.log");
    append_log(
        &log,
        &format!(
            "Detected durable interrupted task after normal Windows startup; resuming task {} operation={:?} stage={:?}",
            task.task_id, task.operation, task.status
        ),
    )?;
    let reagentc = Command::new("reagentc.exe")
        .args(["/boottore"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if !reagentc.success() {
        append_log(
            &log,
            &format!("Pending boot recovery failed: reagentc exited {reagentc}"),
        )?;
        return Err(err("unable to re-request Windows RE for pending task"));
    }
    append_log(
        &log,
        "Re-requested one-time Windows RE boot for pending task",
    )?;
    let shutdown = Command::new("shutdown.exe")
        .args(["/r", "/t", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if !shutdown.success() {
        append_log(
            &log,
            &format!("Pending boot recovery failed: shutdown exited {shutdown}"),
        )?;
        return Err(err("unable to restart into Windows RE for pending task"));
    }
    append_log(
        &log,
        "Restart requested to resume pending task in Windows RE",
    )?;
    Ok(true)
}

/// A prepared task may be a deliberate `--no-reboot` diagnostic and must not
/// start WinRE by itself. Every later non-terminal stage, however, represents
/// an already authorized recovery that was interrupted after WinRE staging;
/// it is safe to re-request the one-time WinRE boot after artifact validation.
#[cfg(any(windows, test))]
fn stage_resumable_after_interruption(stage: Stage) -> bool {
    matches!(
        stage,
        Stage::BootRequested
            | Stage::RecoveryStarted
            | Stage::Preflight
            | Stage::Capturing
            | Stage::TargetErased
            | Stage::ImageApplied
            | Stage::BootRepaired
    )
}

fn err(message: &str) -> TaskError {
    TaskError::Invalid(message.into())
}

/// Each WIM index owns a sidecar so appending a new capture never makes the
/// target-size and source identity of earlier indexes ambiguous.  The legacy
/// directory-level `metadata.json` remains a read fallback for images made by
/// older releases.
#[cfg(windows)]
fn index_metadata_path(image: &Path, index: u32) -> Result<PathBuf, TaskError> {
    let name = image
        .file_name()
        .ok_or_else(|| err("image path has no file name"))?
        .to_string_lossy();
    Ok(image.with_file_name(format!("{name}.index-{index}.metadata.json")))
}

#[cfg(windows)]
fn legacy_metadata_path(image: &Path) -> Result<PathBuf, TaskError> {
    image
        .parent()
        .map(|parent| parent.join("metadata.json"))
        .ok_or_else(|| err("image path has no parent"))
}

#[cfg(windows)]
fn read_index_metadata(
    image: &Path,
    index: u32,
) -> Result<backuprestore_core::BackupMetadata, TaskError> {
    let sidecar = index_metadata_path(image, index)?;
    let metadata: backuprestore_core::BackupMetadata = if sidecar.is_file() {
        read_json(sidecar)?
    } else {
        read_json(legacy_metadata_path(image)?)?
    };
    if metadata.wim_index != index {
        return Err(err(&format!(
            "backup metadata is for WIM index {}, not selected index {index}",
            metadata.wim_index
        )));
    }
    Ok(metadata)
}

/// Parse the workspace-relative task path written into `RecoveryTask.env`.
///
/// The value is deliberately relative to the root of the workspace volume,
/// for example `Tools\\BackupRestore\\tasks\\<task-id>`.  We split at the
/// *last* `\\tasks\\` marker so a perfectly valid program directory such as
/// `D:\\tasks` or `D:\\Tools\\tasks\\BackupRestore` is not confused with the
/// task-store suffix.  The old implementation rejected `store_rel == "tasks"`,
/// which made `D:\\tasks` unusable even though it is an ordinary directory.
#[cfg(any(windows, test))]
fn split_workspace_root_rel(value: &str, task_id: &str) -> Result<String, TaskError> {
    backuprestore_core::validate_relative_path(value)?;
    let normalized = value.replace('/', "\\");
    let (store_rel, relative_id) = normalized
        .rsplit_once("\\tasks\\")
        .ok_or_else(|| err("WORKSPACE_ROOT_REL must contain \\tasks\\"))?;
    if store_rel.is_empty() {
        return Err(err("WORKSPACE_ROOT_REL has an empty workspace path"));
    }
    if relative_id != task_id {
        return Err(err("WORKSPACE_ROOT_REL task id does not match TASK_ID"));
    }
    Ok(store_rel.to_string())
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
    use backuprestore_core::{validate_payload_files, validate_task_id};

    let values = read_env_file(&path)?;
    let task_id = env_required(&values, "TASK_ID")?;
    validate_task_id(&task_id)?;
    let workspace_root_rel = env_required(&values, "WORKSPACE_ROOT_REL")?;
    let store_rel = split_workspace_root_rel(&workspace_root_rel, &task_id)?;
    // Until the workspace volume is mounted only an ephemeral WinRE path is
    // available.  Switch to the task directory immediately after mounting;
    // all task/recovery logs that survive WinRE are stored there.
    let mut early_log = PathBuf::from(r"X:\BackupRestore-Recovery-early.log");
    let task_letter = mount_env_volume(&values, "WORKSPACE", 'T', &early_log, false)?;
    let store = TaskStore::new(PathBuf::from(format!(r"{}:\{store_rel}", task_letter)));
    let task_dir = store.task_dir(&task_id)?;
    early_log = task_dir.join("Recovery-early.log");
    // Mount Recovery before loading/validating the task so the emergency
    // guard always uses the Recovery volume letter, never the workspace
    // letter. The old ordering could attempt restoration under T:\Recovery.
    let recovery_letter = mount_env_volume(&values, "RECOVERY", 'R', &early_log, false)?;
    let mut cleanup_guard = WinreRestoreGuard::new(&values, &task_dir, &early_log, recovery_letter);
    let mut task = store.load(&task_id)?;
    if matches!(task.status, Stage::Success | Stage::Failed) {
        return Err(err(&format!(
            "task is already terminal at stage {:?}; refusing to run it again",
            task.status
        )));
    }
    let log = store.log_path(&task_id)?;
    let stage_before_failure = task.status;
    let mut efi_root = None;
    // WinRE cleanup is part of the recovery contract.  Defer the terminal
    // success write until the original registered WinRE image has been
    // restored and verified below.
    let result = (|| -> Result<(), TaskError> {
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
        let recovery_exe = task_dir.join("payload").join("Recovery.exe");
        let task_json = task_dir.join("payload").join("task.json");
        let recovery_task_env = task_dir.join("payload").join("RecoveryTask.env");
        let original = task_dir.join("original").join("Winre.wim");
        let staged = task_dir.join("stage").join("Winre.wim");
        if !recovery_exe.is_file() {
            return Err(err("Recovery.exe is required for every WinRE operation"));
        }
        verify_running_recovery_binary(&manifest)?;
        validate_payload_files(
            &manifest,
            &recovery_exe,
            &task_json,
            &recovery_task_env,
            &original,
            &staged,
        )?;
        // The payload task is the immutable copy staged before the one-time
        // boot request. Compare every field except the durable stage: after a
        // real interruption the workspace task is legitimately at
        // TargetErased/ImageApplied/BootRepaired while the immutable payload
        // remains Prepared.
        let payload_task: Task = read_json(&task_json)?;
        payload_task.validate()?;
        let mut comparable_payload = payload_task;
        let mut comparable_workspace = task.clone();
        // Drive letters are intentionally ephemeral.  WinRE remounts the
        // same GUID-identified volumes using its own letters (for example
        // P:/Q: become G:/H:).  Comparing those letters made every
        // interrupted task fail before it could resume, despite all stable
        // partition identities being identical.
        clear_task_drive_letters(&mut comparable_payload);
        clear_task_drive_letters(&mut comparable_workspace);
        comparable_payload.status = comparable_workspace.status;
        if comparable_payload != comparable_workspace {
            return Err(err(
                "payload task does not match the workspace task prepared for this recovery",
            ));
        }

        let efi_letter = if task.operation != Operation::Probe {
            let source_letter = mount_env_volume(&values, "SOURCE", 'S', &early_log, false)?;
            let image_letter = mount_env_volume(&values, "IMAGE", 'I', &early_log, false)?;
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
                let target_letter = mount_env_volume(
                    &values,
                    "TARGET",
                    'W',
                    &early_log,
                    matches!(
                        task.status,
                        Stage::TargetErased | Stage::ImageApplied | Stage::BootRepaired
                    ),
                )?;
                // WinRE may reuse E: for an image/data volume; keep EFI on a
                // late temporary letter to avoid mount collisions.
                let efi_letter = mount_env_volume(&values, "EFI", 'Z', &early_log, false)?;
                if let Some(target) = task.target.as_mut() {
                    target.volume.drive_letter = Some(target_letter);
                }
                Some(efi_letter)
            } else {
                None
            }
        } else if let Some(source) = task.source.as_mut() {
            // Probe tasks are allowed to keep their task files on the source
            // partition. Reuse the verified workspace mount when both names
            // identify the same partition.
            let workspace_volume = task
                .workspace_volume
                .as_ref()
                .ok_or_else(|| err("probe task is missing workspace volume identity"))?;
            if !workspace_volume.same_partition(source) {
                source.drive_letter =
                    Some(mount_env_volume(&values, "SOURCE", 'S', &early_log, false)?);
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
        efi_root = efi_letter.map(|letter| PathBuf::from(format!(r"{}:\", letter)));
        append_log(
            &log,
            &format!(
                "Recovery.exe started from env task={} operation={:?}",
                task_id, task.operation
            ),
        )?;
        recover_windows(
            &store,
            &mut task,
            &log,
            efi_root.as_deref(),
            Some(&values),
            false,
        )
    })();
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

#[cfg(any(windows, test))]
#[allow(dead_code)]
fn clear_task_drive_letters(task: &mut Task) {
    if let Some(source) = task.source.as_mut() {
        source.drive_letter = None;
    }
    if let Some(workspace) = task.workspace_volume.as_mut() {
        workspace.drive_letter = None;
    }
    if let Some(image) = task.image.as_mut() {
        image.volume.drive_letter = None;
    }
    if let Some(destination) = task.destination.as_mut() {
        destination.volume.drive_letter = None;
    }
    if let Some(target) = task.target.as_mut() {
        target.volume.drive_letter = None;
    }
}

/// `winpeshl.ini` starts this GUI-subsystem Rust binary directly.  Verify the
/// actual executable loaded from the task WinRE image as well as the immutable
/// workspace payload copy before touching any disk.  Checking only the copy in
/// the workspace would not prove that WinRE launched the staged Rust payload.
#[cfg(windows)]
fn verify_running_recovery_binary(manifest: &PayloadManifest) -> Result<(), TaskError> {
    let running = env::current_exe()?;
    backuprestore_core::verify_sha256(&running, &manifest.recovery_sha256).map_err(|error| {
        err(&format!(
            "running WinRE Recovery.exe does not match the prepared payload: {error}"
        ))
    })
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
fn env_u64(values: &BTreeMap<String, String>, key: &str) -> Result<u64, TaskError> {
    env_required(values, key)?
        .parse::<u64>()
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
        allow_reformatted_serial: bool,
    ) -> Result<(), TaskError> {
        for (suffix, expected, label) in [
            ("VOLUME_GUID", identity.volume_guid.as_str(), "volume"),
            ("DISK_GUID", identity.disk_guid.as_str(), "disk"),
            (
                "PARTITION_GUID",
                identity.partition_guid.as_str(),
                "partition",
            ),
            (
                "PARTITION_TYPE_GUID",
                identity.partition_type_guid.as_str(),
                "partition type",
            ),
            ("FILESYSTEM", identity.filesystem.as_str(), "filesystem"),
        ] {
            let actual = env_required(values, &format!("{prefix}_{suffix}"))?;
            if !expected.eq_ignore_ascii_case(&actual) {
                return Err(err(&format!(
                    "{prefix} {label} differs between task.json and RecoveryTask.env"
                )));
            }
        }
        for (suffix, expected, label) in [
            ("DISK_NUMBER", identity.disk_number, "disk number"),
            (
                "PARTITION_NUMBER",
                identity.partition_number,
                "partition number",
            ),
        ] {
            let actual = env_u32(values, &format!("{prefix}_{suffix}"))?;
            if expected != Some(actual) {
                return Err(err(&format!(
                    "{prefix} {label} differs between task.json and RecoveryTask.env"
                )));
            }
        }
        for (suffix, expected, label) in [
            (
                "PARTITION_OFFSET",
                identity.partition_offset,
                "partition offset",
            ),
            ("PARTITION_SIZE", identity.partition_size, "partition size"),
        ] {
            let actual = env_u64(values, &format!("{prefix}_{suffix}"))?;
            if expected != actual {
                return Err(err(&format!(
                    "{prefix} {label} differs between task.json and RecoveryTask.env"
                )));
            }
        }
        if !allow_reformatted_serial && !identity.volume_serial.trim().is_empty() {
            let actual = env_required(values, &format!("{prefix}_VOLUME_SERIAL"))?;
            if !identity.volume_serial.eq_ignore_ascii_case(&actual) {
                return Err(err(&format!(
                    "{prefix} volume serial differs between task.json and RecoveryTask.env"
                )));
            }
        }
        Ok(())
    }

    if let Some(source) = task.source.as_ref() {
        verify(values, "SOURCE", source, false)?;
    }
    let workspace_volume = task
        .workspace_volume
        .as_ref()
        .ok_or_else(|| err("task.json is missing workspace_volume identity"))?;
    verify(values, "WORKSPACE", workspace_volume, false)?;
    match task.operation {
        Operation::Backup => {
            let destination = task
                .destination
                .as_ref()
                .ok_or_else(|| err("backup task is missing destination"))?;
            verify(values, "IMAGE", &destination.volume, false)?;
            verify_image_absolute_path(values, destination.absolute_path.as_deref())?;
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let image = task
                .image
                .as_ref()
                .ok_or_else(|| err("restore task is missing image"))?;
            verify(values, "IMAGE", &image.volume, false)?;
            verify_image_absolute_path(values, image.absolute_path.as_deref())?;
            let target = task
                .target
                .as_ref()
                .ok_or_else(|| err("restore task is missing target"))?;
            verify(
                values,
                "TARGET",
                &target.volume,
                matches!(
                    task.status,
                    Stage::TargetErased | Stage::ImageApplied | Stage::BootRepaired
                ),
            )?;
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
    allow_reformatted_serial: bool,
) -> Result<char, TaskError> {
    let disk = env_u32(values, &format!("{prefix}_DISK_NUMBER"))?;
    let partition = env_u32(values, &format!("{prefix}_PARTITION_NUMBER"))?;
    let expected = env_required(values, &format!("{prefix}_VOLUME_GUID"))?;
    if let Some(existing) = find_mounted_volume(&expected) {
        append_log(
            log,
            &format!("{prefix} volume already mounted at {existing}:; reusing it"),
        )?;
        verify_mounted_volume(existing, &expected)?;
        verify_live_volume_identity(existing, values, prefix, allow_reformatted_serial)?;
        return Ok(existing);
    }
    // X: is the writable WinRE RAM disk. C: may be the offline Windows
    // volume (or unavailable), so never use it for the assignment script.
    let script = PathBuf::from(format!(
        r"X:\Windows\Temp\BackupRestore-assign-{letter}.txt"
    ));
    let existing = Command::new("mountvol")
        .arg(format!("{letter}:"))
        .arg("/L")
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if existing.status.success() {
        if let Some(actual) = String::from_utf8_lossy(&existing.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
        {
            if actual.eq_ignore_ascii_case(&expected) {
                verify_mounted_volume(letter, &expected)?;
                verify_live_volume_identity(letter, values, prefix, allow_reformatted_serial)?;
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
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    append_log(
        log,
        &format!("mountvol.exe direct assignment for {letter}: exited with {direct_status}"),
    )?;
    if direct_status.success() {
        verify_mounted_volume(letter, &expected)?;
        verify_live_volume_identity(letter, values, prefix, allow_reformatted_serial)?;
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
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            return Err(err(
                "diskpart timed out while assigning the recovery volume",
            ));
        }
        thread::sleep(Duration::from_millis(100));
    };
    append_log(
        log,
        &format!(
            "diskpart.exe completed with {status}; output={}",
            diskpart_log.display()
        ),
    )?;
    let _ = fs::remove_file(&script);
    verify_mounted_volume(letter, &expected)?;
    verify_live_volume_identity(letter, values, prefix, allow_reformatted_serial)?;
    Ok(letter)
}

#[cfg(windows)]
fn verify_live_volume_identity(
    letter: char,
    values: &BTreeMap<String, String>,
    prefix: &str,
    allow_reformatted_serial: bool,
) -> Result<(), TaskError> {
    // A volume GUID alone is insufficient: it can survive reformatting or a
    // stale mount assignment. Re-read the live GPT/device identity after
    // mounting and compare every immutable field recorded at preparation.
    let live = crate::windows_prepare::volume_identity(letter)?;
    for (suffix, actual, label) in [
        ("VOLUME_GUID", live.volume_guid.as_str(), "volume GUID"),
        ("DISK_GUID", live.disk_guid.as_str(), "disk GUID"),
        (
            "PARTITION_GUID",
            live.partition_guid.as_str(),
            "partition GUID",
        ),
        (
            "PARTITION_TYPE_GUID",
            live.partition_type_guid.as_str(),
            "partition type",
        ),
        ("FILESYSTEM", live.filesystem.as_str(), "filesystem"),
    ] {
        let expected = env_required(values, &format!("{prefix}_{suffix}"))?;
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(err(&format!(
                "{prefix} {label} differs after mounting: expected {expected}, got {actual}"
            )));
        }
    }
    for (suffix, actual, label) in [
        ("DISK_NUMBER", live.disk_number, "disk number"),
        (
            "PARTITION_NUMBER",
            live.partition_number,
            "partition number",
        ),
    ] {
        let expected = env_u32(values, &format!("{prefix}_{suffix}"))?;
        if actual != Some(expected) {
            return Err(err(&format!(
                "{prefix} {label} differs after mounting: expected {expected}, got {:?}",
                actual
            )));
        }
    }
    for (suffix, actual, label) in [
        (
            "PARTITION_OFFSET",
            live.partition_offset,
            "partition offset",
        ),
        ("PARTITION_SIZE", live.partition_size, "partition size"),
    ] {
        let expected = env_u64(values, &format!("{prefix}_{suffix}"))?;
        if actual != expected {
            return Err(err(&format!(
                "{prefix} {label} differs after mounting: expected {expected}, got {actual}"
            )));
        }
    }
    if !allow_reformatted_serial {
        if let Some(expected) = env_optional(values, &format!("{prefix}_VOLUME_SERIAL"))
            && !live.volume_serial.is_empty()
            && !live.volume_serial.eq_ignore_ascii_case(&expected)
        {
            return Err(err(&format!(
                "{prefix} volume serial differs after mounting"
            )));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn find_mounted_volume(expected: &str) -> Option<char> {
    for letter in 'C'..='Z' {
        let Ok(output) = Command::new("mountvol.exe")
            .args([format!("{letter}:"), "/L".to_string()])
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
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
        .creation_flags(CREATE_NO_WINDOW)
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
    let raw_snapshot = task_dir.join("bcd-before-raw");
    let efi_store = efi_root.map(|root| root.join("EFI\\Microsoft\\Boot\\BCD"));
    if let Some(efi_store) = efi_store.filter(|path| path.exists()) {
        if raw_snapshot.is_file() {
            let expected = sha256_file(&raw_snapshot)?;
            match fs::copy(&raw_snapshot, &efi_store) {
                Ok(_) => {
                    backuprestore_core::verify_sha256(&efi_store, &expected)?;
                    append_log(
                        log,
                        "Previous byte-for-byte EFI BCD snapshot restored after boot repair failure",
                    )?;
                }
                Err(error) if error.raw_os_error() == Some(32) => {
                    let snapshot_arg = raw_snapshot.to_string_lossy().into_owned();
                    run_logged("bcdedit.exe", &["/import", &snapshot_arg], log)?;
                    append_log(
                        log,
                        "Raw EFI BCD restore was locked; imported the saved BCD snapshot",
                    )?;
                }
                Err(error) => return Err(error.into()),
            }
        } else {
            // Older tasks only contain a logical BCD export. Import it into
            // the selected EFI store; copying the export bytes is not a valid
            // rollback because the file may be a different hive format.
            let snapshot_arg = snapshot.to_string_lossy().into_owned();
            run_logged("bcdedit.exe", &["/import", &snapshot_arg], log)?;
            append_log(
                log,
                "Legacy exported BCD snapshot restored after boot repair failure",
            )?;
        }
        // Do not invoke `bcdedit /store /enum` after the byte-level copy:
        // opening a BCD hive can rewrite its internal transaction metadata
        // and change the SHA-256 that we just verified. The copy plus hash
        // is the authoritative rollback proof; the logical fallback is
        // recorded separately above.
    } else {
        let snapshot_arg = snapshot.to_string_lossy().into_owned();
        run_logged("bcdedit.exe", &["/import", &snapshot_arg], log)?;
    }
    if efi_root.is_none() {
        append_log(
            log,
            "Previous exported BCD snapshot imported after boot repair failure",
        )?;
    }
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
            let metadata = read_index_metadata(path, image.index).map_err(|error| {
                err(&format!(
                    "backup metadata is required and must be valid: {error}"
                ))
            })?;
            if !metadata.image_sha256.eq_ignore_ascii_case(&image.sha256) {
                return Err(err("image hash does not match backup metadata"));
            }
            let target = task.target.as_ref().ok_or_else(|| err("missing target"))?;
            let required = metadata
                .required_target_size()
                .max(metadata.source.partition_size);
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
            let existing = destination_path.exists();
            if existing && !destination_path.is_file() {
                return Err(err("backup image path exists but is not a regular file"));
            }
            let previous_hash = existing
                .then(|| backuprestore_core::sha256_file(&destination_path))
                .transpose()?;
            let previous_indexes = if existing {
                wim_indexes(&destination_path, log)?
            } else {
                Vec::new()
            };
            let mut previous_metadata = BTreeMap::new();
            for index in &previous_indexes {
                previous_metadata.insert(*index, read_index_metadata(&destination_path, *index)?);
            }
            let legacy_path = legacy_metadata_path(&destination_path)?;
            let legacy_metadata: Option<BackupMetadata> = legacy_path
                .is_file()
                .then(|| read_json(&legacy_path))
                .transpose()
                .ok()
                .flatten();

            // A power loss can leave a partial WIM behind.  For the first
            // capture, only that partial file is removed.  For an existing
            // WIM, copy it to a same-volume candidate and append there; the
            // original remains recoverable until the candidate has passed
            // DISM inspection and is renamed into place.
            let _ = fs::remove_file(&partial);
            let new_index = if existing {
                let candidate = PathBuf::from(format!(
                    "{}.{}.append-candidate.wim",
                    destination_path.display(),
                    task.task_id
                ));
                let _ = fs::remove_file(&candidate);
                fs::copy(&destination_path, &candidate)?;
                let append_args = [
                    "/Append-Image",
                    &format!("/ImageFile:{}", candidate.display()),
                    &format!("/CaptureDir:{}", source_path.display()),
                    "/Name:Windows Backup",
                    "/CheckIntegrity",
                ];
                let append_result = run_logged("dism.exe", &append_args, log);
                if let Err(error) = append_result {
                    let _ = fs::remove_file(&candidate);
                    return Err(error);
                }
                let candidate_indexes = wim_indexes(&candidate, log)?;
                if candidate_indexes.len() != previous_indexes.len().saturating_add(1) {
                    let _ = fs::remove_file(&candidate);
                    return Err(err("WIM append did not create exactly one new index"));
                }
                let index = *candidate_indexes
                    .last()
                    .ok_or_else(|| err("WIM append returned no indexes"))?;
                fs::rename(&candidate, &destination_path)?;
                append_log(log, &format!("Appended backup as WIM index {index}"))?;
                index
            } else {
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
                let indexes = wim_indexes(&partial, log)?;
                if indexes != [1] {
                    return Err(err("first backup capture did not produce WIM index 1"));
                }
                fs::rename(&partial, &destination_path)?;
                1
            };
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
            for (index, metadata) in &mut previous_metadata {
                metadata.wim_index = *index;
                metadata.image_sha256 = image_sha256.clone();
                metadata.image_size = image_size;
                write_json_atomic(index_metadata_path(&destination_path, *index)?, metadata)?;
            }
            let metadata = BackupMetadata {
                version: 2,
                image_type: "wim".into(),
                created: Utc::now(),
                computer: context_value("COMPUTERNAME", "WinRE"),
                windows_edition: context_value("WINDOWS_EDITION", "unknown"),
                architecture: context_value("WINDOWS_ARCHITECTURE", &native_windows_architecture()),
                windows_build: context_value("WINDOWS_BUILD", "unknown"),
                wim_index: new_index,
                image_sha256: image_sha256.clone(),
                image_size,
                source,
                captured_used_bytes: context_u64("SOURCE_USED_BYTES", 0),
                reserved_bytes: context_u64("RESERVED_BYTES", 0),
                minimum_target_size: context_u64("MINIMUM_TARGET_SIZE", source_partition_size),
                volume_serial: source_volume_serial,
                program_version: PROGRAM_VERSION.into(),
            };
            write_json_atomic(
                index_metadata_path(&destination_path, new_index)?,
                &metadata,
            )?;

            // Retain a single legacy sidecar only when it clearly belongs to
            // this image.  New code always reads index-specific metadata;
            // this preserves old index-1 restores without clobbering another
            // WIM stored in the same directory.
            let legacy_belongs_to_image = if !legacy_path.exists() {
                true
            } else {
                legacy_metadata.as_ref().is_some_and(|value| {
                    previous_hash
                        .as_deref()
                        .is_some_and(|hash| value.image_sha256.eq_ignore_ascii_case(hash))
                })
            };
            if legacy_belongs_to_image {
                let legacy = previous_metadata
                    .get(&1)
                    .cloned()
                    .unwrap_or_else(|| metadata.clone());
                write_json_atomic(legacy_path, &legacy)?;
            }
            append_log(
                log,
                &format!("Backup metadata written for WIM index {new_index}"),
            )?;
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
                let entering_target_erased = task.status == Stage::Preflight;
                if task.status == Stage::Preflight {
                    store.write_transition(task, Stage::TargetErased)?;
                }
                format_target_partition(store, task, &target.volume, log)?;
                if !target_root.is_dir() {
                    return Err(err(
                        "restore target is unavailable after DiskPart format operation",
                    ));
                }
                verify_partition_identity_after_format(&target.volume, log)?;
                if entering_target_erased {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-target-erased",
                        Stage::TargetErased,
                        log,
                    );
                }
            }
            if matches!(task.status, Stage::TargetErased | Stage::ImageApplied) {
                let entering_image_applied = task.status == Stage::TargetErased;
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
                if entering_image_applied {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-image-applied",
                        Stage::ImageApplied,
                        log,
                    );
                }
            }
            let efi = find_efi_root(efi_root)?;
            if matches!(task.status, Stage::ImageApplied | Stage::BootRepaired) {
                let entering_boot_repaired = task.status == Stage::ImageApplied;
                // BootRepaired is recorded immediately before BCDBoot so a
                // failure is eligible for BCD rollback.  A retry from that
                // stage simply re-runs BCDBoot and validates its output.
                if task.status == Stage::ImageApplied {
                    store.write_transition(task, Stage::BootRepaired)?;
                }
                if entering_boot_repaired {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-boot-repaired",
                        Stage::BootRepaired,
                        log,
                    );
                }
                if metadata_context
                    .and_then(|values| env_optional(values, "TEST_FAULT"))
                    .as_deref()
                    == Some("bcdboot-failure")
                {
                    return Err(err("development test fault: BCDBoot failure injected"));
                }
                let windows_root = target_root.join("Windows");
                // Capture the primary BCD state from the byte-for-byte task
                // snapshot before BCDBoot.  It remains available even if a
                // power failure occurs after BCDBoot changes the live store.
                let previous_boot_manager = if target.role == TargetRole::NewWindows {
                    Some(boot_manager_state_from_task_snapshot(store, task, log)?)
                } else {
                    None
                };
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
                verify_bcd_target(&efi, &target_root, log)?;
                if target.role == TargetRole::NewWindows {
                    let menu_name = target
                        .boot_menu_name
                        .as_deref()
                        .ok_or_else(|| err("secondary target has no boot menu name"))?;
                    let identifier = set_secondary_boot_menu(&efi, &target_root, menu_name, log)?;
                    preserve_primary_boot_manager(
                        &efi,
                        previous_boot_manager.as_ref().expect("secondary BCD state"),
                        &identifier,
                        log,
                    )?;
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

/// Development-only fault injection for a power loss after a durable recovery
/// stage.  Reboot immediately without writing `failed` or cleaning WinRE, so
/// a subsequent normal-Windows GUI launch must detect and resume the task.
#[cfg(windows)]
fn interrupt_after_stage_if_requested(
    metadata_context: Option<&BTreeMap<String, String>>,
    fault: &str,
    stage: Stage,
    log: &Path,
) {
    if metadata_context
        .and_then(|values| env_optional(values, "TEST_FAULT"))
        .as_deref()
        != Some(fault)
    {
        return;
    }
    // A simulated power-loss task is deliberately booted again by the normal
    // GUI. Persist a one-shot marker before rebooting so the same injected
    // fault cannot fire forever when the resumed stage is re-entered.
    let marker = log.with_file_name(format!(".fault-{fault}.triggered"));
    if marker.is_file() {
        return;
    }
    if fs::write(&marker, format!("stage={stage:?}\n")).is_err() {
        let _ = append_log(
            log,
            "Development test fault marker could not be persisted; refusing injection",
        );
        return;
    }
    let _ = append_log(
        log,
        &format!(
            "Development test fault: simulating power loss after durable stage {stage:?}; rebooting without cleanup"
        ),
    );
    let _ = run_logged("wpeutil.exe", &["reboot"], log);
    std::process::exit(0);
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

#[cfg(windows)]
fn verify_partition_identity_after_format(
    expected: &backuprestore_core::VolumeIdentity,
    log: &Path,
) -> Result<(), TaskError> {
    let letter = expected
        .drive_letter
        .ok_or_else(|| err("formatted restore target lost its drive letter"))?;
    let actual = crate::windows_prepare::volume_identity(letter)?;
    for (name, left, right) in [
        (
            "disk GUID",
            expected.disk_guid.as_str(),
            actual.disk_guid.as_str(),
        ),
        (
            "partition GUID",
            expected.partition_guid.as_str(),
            actual.partition_guid.as_str(),
        ),
        (
            "partition type",
            expected.partition_type_guid.as_str(),
            actual.partition_type_guid.as_str(),
        ),
    ] {
        if !left.eq_ignore_ascii_case(right) {
            return Err(err(&format!(
                "formatted restore target {name} changed unexpectedly"
            )));
        }
    }
    if expected.disk_number != actual.disk_number
        || expected.partition_number != actual.partition_number
        || expected.partition_offset != actual.partition_offset
        || expected.partition_size != actual.partition_size
    {
        return Err(err(
            "formatted restore target disk/partition geometry changed unexpectedly",
        ));
    }
    append_log(log, "Formatted target partition identity re-verified")?;
    Ok(())
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
fn wim_indexes(path: &Path, log: &Path) -> Result<Vec<u32>, TaskError> {
    let output = capture_logged(
        "dism.exe",
        &[
            "/English",
            "/Get-WimInfo",
            &format!("/WimFile:{}", path.display()),
        ],
        log,
    )?;
    let mut indexes = crate::windows_prepare::parse_dism_images(&output)?
        .iter()
        .filter_map(|image| image.get("ImageIndex").and_then(serde_json::Value::as_u64))
        .filter_map(|index| u32::try_from(index).ok())
        .collect::<Vec<_>>();
    indexes.sort_unstable();
    indexes.dedup();
    if indexes.is_empty() || indexes.iter().any(|index| *index == 0) {
        return Err(err("DISM returned invalid WIM indexes"));
    }
    Ok(indexes)
}

#[cfg(windows)]
fn run_logged(program: &str, args: &[&str], log: &Path) -> Result<(), TaskError> {
    append_log(log, &format!("running {program} {}", args.join(" ")))?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
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
        .creation_flags(CREATE_NO_WINDOW)
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

#[cfg(any(windows, test))]
fn bcd_identifier_from_line(line: &str) -> Option<&str> {
    let start = line.find('{')?;
    let end = line[start..].find('}')? + start + 1;
    let identifier = &line[start..end];
    (identifier.len() == 38
        && identifier.starts_with('{')
        && identifier.ends_with('}')
        && identifier[1..37]
            .chars()
            .enumerate()
            .all(|(index, character)| {
                matches!(index, 8 | 13 | 18 | 23)
                    .then_some(character == '-')
                    .unwrap_or_else(|| character.is_ascii_hexdigit())
            }))
    .then_some(identifier)
}

#[cfg(windows)]
fn set_secondary_boot_menu(
    efi_root: &Path,
    target_root: &Path,
    menu_name: &str,
    log: &Path,
) -> Result<String, TaskError> {
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
    // Parse blank-line-separated BCD objects. The first GUID in a block is
    // the object identifier; later GUIDs belong to recovery/resume/inherit
    // references. Restrict matches to a Windows loader path so a
    // winresume.efi object cannot receive the secondary menu description.
    let normalized_output = output.replace("\r\n", "\n");
    let identifier = normalized_output
        .split("\n\n")
        .filter_map(|block| {
            let identifier = block.lines().find_map(bcd_identifier_from_line)?;
            let lower = block.to_ascii_lowercase();
            let is_loader = lower.contains("winload.efi");
            let targets_partition = lower.lines().any(|line| {
                let line = line.trim_start();
                (line.starts_with("device")
                    || line.starts_with("osdevice")
                    || line.starts_with("设备")
                    || line.starts_with("os 设备"))
                    && line.contains(&target_needle)
            });
            (is_loader && targets_partition).then_some(identifier.to_string())
        })
        .next()
        .ok_or_else(|| {
            err("BCDBoot created no Windows loader tied to the selected target partition")
        })?;
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
    Ok(identifier.clone())
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct BcdBootManagerState {
    default: String,
    display_order: Vec<String>,
}

/// Read the original Boot Manager state from the BCD snapshot captured while
/// preparing the task. BCDBoot documents that `/addlast` is ignored when `/s`
/// specifies a volume, and it can replace Boot Manager's default with the new
/// loader. The snapshot is therefore the source of truth across retries.
#[cfg(windows)]
fn boot_manager_state_from_task_snapshot(
    store: &TaskStore,
    task: &Task,
    log: &Path,
) -> Result<BcdBootManagerState, TaskError> {
    let task_dir = store.task_dir(&task.task_id)?;
    let text_snapshot = task_dir.join("bcd-before-bootmgr.txt");
    if text_snapshot.is_file() {
        let output = fs::read_to_string(&text_snapshot)?;
        return parse_boot_manager_state(&output)
            .ok_or_else(|| err("saved Boot Manager state is missing or invalid"));
    }
    let raw_snapshot = task_dir.join("bcd-before-raw");
    let snapshot = if raw_snapshot.is_file() {
        raw_snapshot
    } else {
        task_dir.join("bcd-before-export")
    };
    if !snapshot.is_file() {
        return Err(err(
            "task BCD snapshot is missing before secondary boot repair",
        ));
    }
    boot_manager_state_from_store(&snapshot, log)
}

#[cfg(windows)]
fn boot_manager_state_from_store(
    store: &Path,
    log: &Path,
) -> Result<BcdBootManagerState, TaskError> {
    let store_arg = store.to_string_lossy().into_owned();
    let output = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    parse_boot_manager_state(&output)
        .ok_or_else(|| err("existing Boot Manager state is missing or invalid"))
}

/// Restore the exact previous default and display order, appending the newly
/// created secondary loader last. This uses BCDEdit after BCDBoot because
/// BCDBoot ignores `/addlast` when called with an explicit EFI root.
#[cfg(windows)]
fn preserve_primary_boot_manager(
    efi_root: &Path,
    previous: &BcdBootManagerState,
    secondary_identifier: &str,
    log: &Path,
) -> Result<(), TaskError> {
    let store = efi_root.join("EFI\\Microsoft\\Boot\\BCD");
    let store_arg = store.to_string_lossy().into_owned();
    run_logged(
        "bcdedit.exe",
        &[
            "/store",
            &store_arg,
            "/set",
            "{bootmgr}",
            "default",
            &previous.default,
        ],
        log,
    )?;
    let mut expected_order: Vec<String> = previous
        .display_order
        .iter()
        .filter(|identifier| !identifier.eq_ignore_ascii_case(secondary_identifier))
        .cloned()
        .collect();
    if expected_order.is_empty() {
        expected_order.push(previous.default.clone());
    }
    expected_order.push(secondary_identifier.to_string());
    let mut display_args = vec![
        "/store".to_string(),
        store_arg.clone(),
        "/displayorder".to_string(),
    ];
    display_args.extend(expected_order.iter().cloned());
    let display_refs: Vec<&str> = display_args.iter().map(String::as_str).collect();
    run_logged("bcdedit.exe", &display_refs, log)?;
    let verified_text = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let verified = parse_boot_manager_state(&verified_text)
        .ok_or_else(|| err("Boot Manager state could not be read after preserving order"))?;
    if verified.default != previous.default
        || verified
            .display_order
            .last()
            .is_none_or(|identifier| !identifier.eq_ignore_ascii_case(secondary_identifier))
        || expected_order.iter().any(|identifier| {
            !verified
                .display_order
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(identifier))
        })
    {
        return Err(err(
            "Boot Manager default or secondary display order could not be preserved",
        ));
    }
    append_log(
        log,
        &format!(
            "Preserved Boot Manager default {}; restored {} original display-order entries and appended secondary loader {secondary_identifier}",
            previous.default,
            expected_order.len().saturating_sub(1),
        ),
    )?;
    Ok(())
}

/// Parse the minimal Boot Manager state required to preserve the existing
/// loader. The BCD field labels may be localized, but the value tokens are
/// invariant braced identifiers.
#[cfg(any(windows, test))]
fn parse_boot_manager_state(output: &str) -> Option<BcdBootManagerState> {
    // `/enum all` includes a firmware boot-manager object that also has a
    // `displayorder`. Scope the parser to the Windows Boot Manager object;
    // otherwise a secondary restore could put firmware entries in the
    // Windows loader menu.
    let output = output.replace("\r\n", "\n");
    let boot_manager = output.split("\n\n").find(|block| {
        block
            .lines()
            .find_map(bcd_identifier_from_line)
            .is_some_and(|identifier| {
                identifier.eq_ignore_ascii_case("{9dea862c-5cdd-4e70-acc1-f32b344d4795}")
            })
    })?;
    let mut default = None;
    let mut display_order = Vec::new();
    let mut reading_display_order = false;
    for line in boot_manager.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("default") || trimmed.starts_with("默认") {
            default = bcd_value_from_line(trimmed).map(str::to_owned);
            reading_display_order = false;
            continue;
        }
        if lower.starts_with("displayorder") || trimmed.starts_with("显示顺序") {
            reading_display_order = true;
        }
        if reading_display_order {
            if let Some(value) = bcd_value_from_line(trimmed) {
                display_order.push(value.to_owned());
            } else if !trimmed.is_empty() {
                reading_display_order = false;
            }
        }
    }
    Some(BcdBootManagerState {
        default: default?,
        display_order,
    })
}

/// Accept GUIDs and BCDEdit's well-known aliases, but nothing that could turn
/// captured BCD text into an argument injection.
#[cfg(any(windows, test))]
fn bcd_value_from_line(line: &str) -> Option<&str> {
    let start = line.find('{')?;
    let end = line[start..].find('}')? + start + 1;
    let value = &line[start..end];
    (value.len() >= 3
        && value[1..value.len() - 1]
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-'))
    .then_some(value)
}

#[cfg(windows)]
fn verify_bcd_target(efi_root: &Path, target_root: &Path, log: &Path) -> Result<(), TaskError> {
    let store = efi_root.join("EFI\\Microsoft\\Boot\\BCD");
    if !store.is_file() {
        return Err(err("BCD store is missing after BCDBoot"));
    }
    let target_letter = target_root
        .to_string_lossy()
        .chars()
        .next()
        .ok_or_else(|| err("restore target has no drive letter for BCD verification"))?
        .to_ascii_lowercase();
    let needle = format!("partition={target_letter}:");
    let store_arg = store.to_string_lossy().into_owned();
    let output = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let matched = output.lines().any(|line| {
        let lower = line.trim().to_ascii_lowercase();
        (lower.starts_with("device")
            || lower.starts_with("osdevice")
            || lower.starts_with("设备")
            || lower.starts_with("os 设备"))
            && lower.contains(&needle)
    });
    if !matched {
        return Err(err(&format!(
            "BCDBoot completed but no Windows loader points to the selected target {target_letter}:"
        )));
    }
    append_log(
        log,
        &format!("Verified BCD device/osdevice points to target {target_letter}:"),
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
    }
    Ok(())
}

fn run_command(program: &str, args: Vec<String>) -> Result<(), TaskError> {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(err(&format!("{program} failed with {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bcd_identifier_from_line, diskpart_format_script, parse_boot_manager_state,
        parse_recover_options, split_workspace_root_rel, stage_resumable_after_interruption,
    };
    use backuprestore_core::Stage;

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

    #[test]
    fn workspace_path_allows_directory_named_tasks() {
        let task_id = "11111111-1111-4111-8111-111111111111";
        assert_eq!(
            split_workspace_root_rel(&format!("tasks\\tasks\\{task_id}"), task_id).unwrap(),
            "tasks"
        );
        assert_eq!(
            split_workspace_root_rel(
                &format!("Tools\\tasks\\BackupRestore\\tasks\\{task_id}"),
                task_id
            )
            .unwrap(),
            "Tools\\tasks\\BackupRestore"
        );
        assert!(
            split_workspace_root_rel("..\\tasks\\11111111-1111-4111-8111-111111111111", task_id)
                .is_err()
        );
        assert!(
            split_workspace_root_rel(
                "tasks\\tasks\\22222222-2222-4222-8222-222222222222",
                task_id
            )
            .is_err()
        );
    }

    #[test]
    fn bcd_identifier_parser_ignores_localized_field_labels() {
        assert_eq!(
            bcd_identifier_from_line(
                "\u{fffd}\u{fffd}\u{fffd} {3f66f61e-a1ce-11f1-9d29-f5ba5ed7cdd2}"
            ),
            Some("{3f66f61e-a1ce-11f1-9d29-f5ba5ed7cdd2}")
        );
        assert_eq!(bcd_identifier_from_line("device partition=H:"), None);
        assert_eq!(bcd_identifier_from_line("{not-a-bcd-guid}"), None);
    }

    #[test]
    fn secondary_loader_selection_ignores_resume_object() {
        let output = "Windows Boot Loader\nidentifier {11111111-2222-4333-8444-555555555555}\ndevice partition=Q:\nosdevice partition=Q:\npath \\Windows\\system32\\winload.efi\n\nWindows Resume Application\nidentifier {22222222-3333-4444-8555-666666666666}\ndevice partition=Q:\npath \\Windows\\system32\\winresume.efi";
        let selected = output
            .split("\n\n")
            .filter_map(|block| {
                let identifier = block.lines().find_map(bcd_identifier_from_line)?;
                let lower = block.to_ascii_lowercase();
                let targets_partition = lower.lines().any(|line| {
                    let line = line.trim_start();
                    (line.starts_with("device") || line.starts_with("osdevice"))
                        && line.contains("partition=q:")
                });
                (lower.contains("winload.efi") && targets_partition).then_some(identifier)
            })
            .next();
        assert_eq!(selected, Some("{11111111-2222-4333-8444-555555555555}"));
    }

    #[test]
    fn boot_manager_state_parser_accepts_guid_and_alias_only() {
        assert_eq!(
            parse_boot_manager_state(
                "Firmware Boot Manager\nidentifier {aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\ndisplayorder {ffffffff-1111-4222-8333-444444444444}\n\nWindows Boot Manager\nidentifier {9dea862c-5cdd-4e70-acc1-f32b344d4795}\ndefault {11111111-2222-4333-8444-555555555555}\ndisplayorder {11111111-2222-4333-8444-555555555555}"
            )
            .unwrap()
            .default,
            "{11111111-2222-4333-8444-555555555555}"
        );
        assert_eq!(
            parse_boot_manager_state(
                "Windows Boot Manager\nidentifier {9dea862c-5cdd-4e70-acc1-f32b344d4795}\n默认 {default}\n显示顺序 {default}"
            )
            .unwrap()
            .display_order,
            vec!["{default}"]
        );
        assert!(parse_boot_manager_state("default dangerous;value").is_none());
    }

    #[test]
    fn interruption_resume_accepts_only_staged_nonterminal_tasks() {
        assert!(!stage_resumable_after_interruption(Stage::Prepared));
        assert!(stage_resumable_after_interruption(Stage::BootRequested));
        assert!(stage_resumable_after_interruption(Stage::TargetErased));
        assert!(stage_resumable_after_interruption(Stage::ImageApplied));
        assert!(stage_resumable_after_interruption(Stage::BootRepaired));
        assert!(!stage_resumable_after_interruption(Stage::Success));
        assert!(!stage_resumable_after_interruption(Stage::Failed));
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

    #[cfg(windows)]
    #[test]
    fn prepare_fault_options_require_efi_only_for_efi_faults() {
        let base = vec![
            "--operation".into(),
            "probe".into(),
            "--source-drive".into(),
            "P".into(),
        ];
        let mut power_loss = base.clone();
        power_loss.extend(["--test-fault".into(), "power-loss-window".into()]);
        assert!(
            super::windows_prepare::parse_prepare_options(power_loss).is_ok(),
            "power-loss-window uses the real system EFI by default"
        );

        for fault in ["identity-env-mismatch", "bcdboot-failure"] {
            let mut missing_efi = base.clone();
            missing_efi.extend(["--test-fault".into(), fault.into()]);
            assert!(
                super::windows_prepare::parse_prepare_options(missing_efi).is_err(),
                "{fault} must require an explicit development EFI"
            );
        }
        for fault in [
            "power-loss-target-erased",
            "power-loss-image-applied",
            "power-loss-boot-repaired",
        ] {
            let mut system_efi_fault = base.clone();
            system_efi_fault.extend(["--test-fault".into(), fault.into()]);
            assert!(
                super::windows_prepare::parse_prepare_options(system_efi_fault).is_ok(),
                "{fault} must use the current system EFI by default"
            );
        }
    }
}
