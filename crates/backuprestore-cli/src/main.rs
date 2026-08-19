//! Small Windows front-end/recovery host entry point.
//!
//! The GUI can call this binary for task validation and execution.  Keeping
//! the recovery path as a command-line program also makes it usable from
//! WinRE's `winpeshl.ini` without depending on a desktop runtime.

#[cfg(windows)]
use backuprestore_core::{BootMode, Operation, verify_image_file};
use backuprestore_core::{Stage, Task, TaskError, TaskStore, read_json, sha256_file};
use chrono::Utc;
#[cfg(windows)]
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::process::Command;
#[cfg(windows)]
use std::process::Stdio;

fn usage() -> ! {
    eprintln!(
        "BackupRestore commands:\n  validate-task <task.json>\n  hash <file>\n  status <task-root> <task-id>\n  recover <task-root> <task-id> [--dry-run]\n  recover-env <RecoveryTask.env>\n  run-command <program> [args...]\n"
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
        Some("recover") => {
            let root = args.next().ok_or_else(|| err("task root is required"));
            let id = args.next().ok_or_else(|| err("task id is required"));
            let dry_run = args.any(|x| x == "--dry-run");
            root.and_then(|r| id.map(|i| (r, i)))
                .and_then(|(r, i)| recover(r, i, dry_run))
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
    let executable = env::current_exe()?;
    let script = executable
        .parent()
        .ok_or_else(|| err("BackupRestore.exe has no parent directory"))?
        .join("BackupRestore.Gui.ps1");
    if !script.exists() {
        return Err(err(
            "BackupRestore.Gui.ps1 is missing beside BackupRestore.exe",
        ));
    }
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script.to_string_lossy().as_ref(),
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(err(&format!("GUI exited with {status}")))
    }
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
    let status = read_json(store.status_path(&id))?;
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

fn recover(root: String, id: String, dry_run: bool) -> Result<(), TaskError> {
    let store = TaskStore::new(root);
    let mut task = store.load(&id)?;
    let log = store.log_path(&id);
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
    if dry_run {
        println!(
            "dry-run: task={} operation={:?} stage={:?}",
            task.task_id, task.operation, task.status
        );
        if let Some(image) = &task.image {
            println!("image={} sha256={}", image.relative_path, image.sha256);
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
    let result = recover_windows(&store, &mut task, &log, None);
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
    use backuprestore_core::{PayloadManifest, validate_payload_files, verify_sha256};

    let values = read_env_file(&path)?;
    let task_id = env_required(&values, "TASK_ID")?;
    let task_root_rel = env_required(&values, "TASK_ROOT_REL")?;
    let (store_rel, relative_id) = task_root_rel
        .replace('/', "\\")
        .rsplit_once("\\tasks\\")
        .ok_or_else(|| err("TASK_ROOT_REL must contain \\tasks\\"))?;
    if relative_id != task_id {
        return Err(err("TASK_ROOT_REL task id does not match TASK_ID"));
    }
    let early_log = PathBuf::from(r"C:\WinRE-PoC\Recovery-rust.log");
    if let Some(parent) = early_log.parent() {
        fs::create_dir_all(parent)?;
    }
    mount_env_volume(&values, "TASK", 'T', &early_log)?;
    let store = TaskStore::new(PathBuf::from(format!(r"T:\{store_rel}")));
    let task_dir = store.task_dir(&task_id);
    let mut cleanup_guard = WinreRestoreGuard::new(&values, &task_dir, &early_log);
    let mut task = store.load(&task_id)?;
    let manifest: PayloadManifest = read_json(task_dir.join("manifest.json"))?;
    let launcher = task_dir.join("payload").join("RecoveryLauncher.cmd");
    let recovery_cmd = task_dir.join("payload").join("Recovery.cmd");
    let recovery_exe = task_dir.join("payload").join("Recovery.exe");
    let task_json = task_dir.join("payload").join("task.json");
    let original = task_dir.join("original").join("Winre.wim");
    let staged = task_dir.join("stage").join("Winre.wim");
    if recovery_exe.exists() {
        validate_payload_files(
            &manifest,
            &launcher,
            &recovery_exe,
            &task_json,
            &original,
            &staged,
        )?;
    } else if task.operation != Operation::Probe {
        return Err(err("Recovery.exe is required for a real operation"));
    } else {
        verify_sha256(&launcher, &manifest.launcher_sha256)?;
        verify_sha256(&recovery_cmd, &manifest.recovery_sha256)?;
        verify_sha256(&task_json, &manifest.task_sha256)?;
        verify_sha256(&original, &manifest.original_winre_sha256)?;
        verify_sha256(&staged, &manifest.staged_winre_sha256)?;
    }

    mount_env_volume(&values, "RECOVERY", 'R', &early_log)?;
    if task.operation != Operation::Probe {
        mount_env_volume(&values, "SOURCE", 'S', &early_log)?;
        mount_env_volume(&values, "IMAGE", 'I', &early_log)?;
        if let Some(source) = task.source.as_mut() {
            source.drive_letter = Some('S');
        }
        if let Some(image) = task.image.as_mut() {
            image.volume.drive_letter = Some('I');
        }
        if let Some(destination) = task.destination.as_mut() {
            destination.volume.drive_letter = Some('I');
        }
        if task.target.is_some() {
            mount_env_volume(&values, "TARGET", 'W', &early_log)?;
            mount_env_volume(&values, "EFI", 'E', &early_log)?;
            if let Some(target) = task.target.as_mut() {
                target.volume.drive_letter = Some('W');
            }
        }
    } else if let Some(source) = task.source.as_mut() {
        mount_env_volume(&values, "SOURCE", 'S', &early_log)?;
        source.drive_letter = Some('S');
    }

    let log = store.log_path(&task_id);
    append_log(
        &log,
        &format!(
            "Recovery.exe started from env task={} operation={:?}",
            task_id, task.operation
        ),
    )?;
    let efi_root = task.target.as_ref().map(|_| Path::new("E:\\"));
    let result = recover_windows(&store, &mut task, &log, efi_root);
    if let Err(error) = &result {
        let _ = store.write_failure(&mut task, 1, error.to_string());
        append_log(&log, &format!("Recovery failed: {error}"))?;
    }
    let cleanup = restore_original_winre(&values, &task_dir, &log);
    if let Err(error) = &cleanup {
        append_log(&log, &format!("WinRE cleanup failed: {error}"))?;
    }
    if cleanup.is_ok() {
        cleanup_guard.disarm();
        run_logged("wpeutil.exe", &["reboot"], &log)?;
    }
    result.and(cleanup)
}

#[cfg(windows)]
struct WinreRestoreGuard<'a> {
    values: &'a BTreeMap<String, String>,
    task_dir: &'a Path,
    log: &'a Path,
    active: bool,
}

#[cfg(windows)]
impl<'a> WinreRestoreGuard<'a> {
    fn new(values: &'a BTreeMap<String, String>, task_dir: &'a Path, log: &'a Path) -> Self {
        Self {
            values,
            task_dir,
            log,
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
            let _ = restore_original_winre(self.values, self.task_dir, self.log);
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
        if key.is_empty() || values.insert(key.to_string(), value.to_string()).is_some() {
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
fn mount_env_volume(
    values: &BTreeMap<String, String>,
    prefix: &str,
    letter: char,
    log: &Path,
) -> Result<(), TaskError> {
    let disk = env_u32(values, &format!("{prefix}_DISK_NUMBER"))?;
    let partition = env_u32(values, &format!("{prefix}_PARTITION_NUMBER"))?;
    let expected = env_required(values, &format!("{prefix}_VOLUME_GUID"))?;
    let script = PathBuf::from(format!(
        r"C:\Windows\Temp\BackupRestore-assign-{letter}.txt"
    ));
    let body =
        format!("select disk {disk}\r\nselect partition {partition}\r\nassign letter={letter}\r\n");
    fs::write(&script, body)?;
    let script_arg = script.to_string_lossy().into_owned();
    let result = run_logged("diskpart.exe", &["/s", script_arg.as_str()], log);
    let _ = fs::remove_file(&script);
    result?;
    let output = Command::new("mountvol")
        .arg(format!("{letter}:"))
        .arg("/L")
        .output()?;
    if !output.status.success() {
        return Err(err(&format!("mountvol failed for {letter}:")));
    }
    let actual = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| err(&format!("volume {letter}: has no mountvol identity")))?;
    if !actual.eq_ignore_ascii_case(&expected) {
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
) -> Result<(), TaskError> {
    let original = task_dir.join("original").join("Winre.wim");
    let registered = PathBuf::from(r"R:\Recovery\WindowsRE\Winre.wim");
    if !original.exists() || !registered.parent().is_some_and(Path::exists) {
        return Err(err("original or registered WinRE image is missing"));
    }
    fs::copy(&original, &registered)?;
    let expected = env_required(values, "ORIGINAL_WINRE_SHA256")?;
    backuprestore_core::verify_sha256(&registered, &expected)?;
    append_log(log, "Original registered WinRE restored and verified")?;
    Ok(())
}

#[cfg(not(windows))]
fn recover_windows(
    _store: &TaskStore,
    _task: &mut Task,
    _log: &Path,
    _efi_root: Option<&Path>,
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
            if task.status == Stage::Preflight {
                store.write_transition(task, Stage::Success)?;
            }
        }
        Operation::Backup => {
            use backuprestore_core::{BackupMetadata, PROGRAM_VERSION, write_json_atomic};
            let source = task.source.as_ref().ok_or_else(|| err("missing source"))?;
            let destination = task
                .destination
                .as_ref()
                .ok_or_else(|| err("missing destination"))?;
            let source_path = resolve_volume_root(source)?;
            let destination_path =
                resolve_volume_path(&destination.volume, &destination.relative_path)?;
            let partial = PathBuf::from(format!("{}.partial", destination_path.display()));
            if let Some(parent) = partial.parent() {
                fs::create_dir_all(parent)?;
            }
            store.write_transition(task, Stage::Capturing)?;
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
            let metadata = BackupMetadata {
                version: 1,
                image_type: "wim".into(),
                created: Utc::now(),
                computer: "WinRE".into(),
                windows_edition: "unknown".into(),
                architecture: "amd64".into(),
                windows_build: "unknown".into(),
                wim_index: 1,
                image_sha256,
                image_size,
                source: source.clone(),
                captured_used_bytes: 0,
                reserved_bytes: 0,
                minimum_target_size: source.partition_size,
                volume_serial: source.volume_serial.clone(),
                program_version: PROGRAM_VERSION.into(),
            };
            let metadata_path = destination_path
                .parent()
                .ok_or_else(|| err("backup destination has no parent"))?
                .join("metadata.json");
            write_json_atomic(metadata_path, &metadata)?;
            append_log(log, "Backup metadata written and image hash recorded")?;
            store.write_transition(task, Stage::Success)?;
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let target = task.target.as_ref().ok_or_else(|| err("missing target"))?;
            if target.role == TargetRole::NewWindows
                && task.boot_plan.mode != BootMode::AddSecondary
            {
                return Err(err("secondary target requires add-secondary boot plan"));
            }
            let image_path = image_path.ok_or_else(|| err("missing image"))?;
            let target_root = resolve_volume_root(&target.volume)?;
            store.write_transition(task, Stage::TargetErased)?;
            run_logged(
                "format.com",
                &[&target_root.to_string_lossy(), "/FS:NTFS", "/Q", "/Y"],
                log,
            )?;
            store.write_transition(task, Stage::ImageApplied)?;
            run_logged(
                "dism.exe",
                &[
                    "/Apply-Image",
                    &format!("/ImageFile:{}", image_path.display()),
                    &format!(
                        "/Index:{}",
                        task.image.as_ref().map(|x| x.index).unwrap_or(1)
                    ),
                    &format!("/ApplyDir:{}", target_root.display()),
                ],
                log,
            )?;
            let efi = find_efi_root(efi_root)?;
            store.write_transition(task, Stage::BootRepaired)?;
            let mut bcd_args = vec![
                format!("{}\\Windows", target_root.display()),
                "/s".to_string(),
                efi.to_string_lossy().into_owned(),
                "/f".to_string(),
                "UEFI".to_string(),
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
            store.write_transition(task, Stage::Success)?;
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
#[cfg(windows)]
fn resolve_volume_path(
    volume: &backuprestore_core::VolumeIdentity,
    relative: &str,
) -> Result<PathBuf, TaskError> {
    Ok(resolve_volume_root(volume)?.join(relative.replace('\\', std::path::MAIN_SEPARATOR_STR)))
}
#[cfg(windows)]
fn find_efi_root(override_root: Option<&Path>) -> Result<PathBuf, TaskError> {
    Ok(override_root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("S:\\")))
}
#[cfg(windows)]
fn run_logged(program: &str, args: &[&str], log: &Path) -> Result<(), TaskError> {
    append_log(log, &format!("running {program} {}", args.join(" ")))?;
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?
        .write_all(&output.stdout)?;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?
        .write_all(&output.stderr)?;
    if !output.status.success() {
        return Err(err(&format!("{program} failed with {}", output.status)));
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
