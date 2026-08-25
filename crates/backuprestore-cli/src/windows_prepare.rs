//! Normal-Windows preparation implemented in Rust.
//!
//! The desktop program does not delegate task creation or system preparation
//! to PowerShell.  This module owns the public `prepare`, `list-volumes`,
//! `inspect-environment` and `wim-info` commands; it uses only Rust file I/O
//! plus the Windows inbox command-line tools that perform the OS operations.

use backuprestore_core::{
    BootMode, DestinationSpec, ImageSpec, Operation, PayloadManifest, TargetRole, TargetSpec, Task,
    TaskError, TaskStore, VolumeIdentity, read_json, sha256_file, validate_absolute_path,
    write_json_atomic,
};
use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::env;
use std::ffi::c_void;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr::null_mut;
use std::thread;
use std::time::Duration;

use crate::{append_log, err, run_logged};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetDiskFreeSpaceExW(
        directory_name: *const u16,
        free_bytes_available: *mut u64,
        total_number_of_bytes: *mut u64,
        total_number_of_free_bytes: *mut u64,
    ) -> i32;
    fn GetVolumeInformationW(
        root_path_name: *const u16,
        volume_name_buffer: *mut u16,
        volume_name_size: u32,
        volume_serial_number: *mut u32,
        maximum_component_length: *mut u32,
        file_system_flags: *mut u32,
        file_system_name_buffer: *mut u16,
        file_system_name_size: u32,
    ) -> i32;
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: *mut c_void,
    ) -> *mut c_void;
    fn DeviceIoControl(
        device: *mut c_void,
        control_code: u32,
        input_buffer: *mut c_void,
        input_size: u32,
        output_buffer: *mut c_void,
        output_size: u32,
        bytes_returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const OPEN_EXISTING: u32 = 3;
const IOCTL_DISK_GET_PARTITION_INFO_EX: u32 = 0x0007_0048;
const IOCTL_DISK_GET_DRIVE_LAYOUT_EX: u32 = 0x0007_0050;

const RESERVED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const EFI_TYPE: &str = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}";
const RECOVERY_TYPE: &str = "{de94bba4-06d1-4d40-a16a-bfd50179d6ac}";

#[derive(Debug, Clone)]
struct PrepareOptions {
    operation: Operation,
    source_drive: char,
    target_drive: char,
    image_path: Option<String>,
    wim_index: u32,
    boot_menu_name: String,
    efi_drive: Option<char>,
    allow_destructive: bool,
    no_reboot: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DriveReport {
    letter: String,
    label: String,
    filesystem: String,
    size_bytes: u64,
    free_bytes: u64,
    volume_guid: String,
    disk_number: u32,
    partition_number: u32,
    partition_type_guid: String,
}

pub(crate) fn prepare(arguments: Vec<String>) -> Result<(), TaskError> {
    let options = parse_prepare_options(arguments)?;
    require_administrator()?;
    let executable_dir = executable_dir()?;
    let workspace = volume_identity(drive_from_path(&executable_dir)?)?;
    let target = volume_identity(options.target_drive)?;
    if matches!(
        options.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) && workspace.same_partition(&target)
    {
        return Err(err(&format!(
            "Cannot start restore: the program directory is on {}:, which is the restore target. Move the entire BackupRestore folder to another volume and run it again. No task, WinRE, BCD or reboot was requested.",
            workspace.drive_letter.unwrap_or('?')
        )));
    }
    prepare_task(&executable_dir, &workspace, target, options)
}

pub(crate) fn list_volumes() -> Result<(), TaskError> {
    let drives = discover_drives()?;
    println!("{}", serde_json::to_string(&drives)?);
    Ok(())
}

pub(crate) fn inspect_environment() -> Result<(), TaskError> {
    let output = capture("cmd.exe", &["/d", "/c", "ver"])?;
    let architecture = native_architecture();
    let reagent = capture("reagentc.exe", &["/info"])?;
    let winre_available = reagent.contains("GLOBALROOT") || reagent.contains("Recovery\\WindowsRE");
    println!(
        "{}",
        serde_json::to_string(&json!({
            "windows": output.trim(),
            "architecture": architecture,
            "winreAvailable": winre_available,
            "volumes": discover_drives()?,
        }))?
    );
    Ok(())
}

pub(crate) fn wim_info(image_path: String) -> Result<(), TaskError> {
    validate_absolute_path(&image_path)?;
    let path = PathBuf::from(&image_path);
    if !path.is_file() {
        return Err(err(&format!("WIM image does not exist: {image_path}")));
    }
    let output = capture(
        "dism.exe",
        &[
            "/English",
            "/Get-WimInfo",
            &format!("/WimFile:{image_path}"),
        ],
    )?;
    let images = parse_dism_images(&output)?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "image": image_path,
            "sha256": sha256_file(&path)?,
            "images": images,
        }))?
    );
    Ok(())
}

fn parse_prepare_options(arguments: Vec<String>) -> Result<PrepareOptions, TaskError> {
    let mut operation = None;
    let mut source_drive = None;
    let mut target_drive = None;
    let mut image_path = None;
    let mut wim_index = 1_u32;
    let mut boot_menu_name = String::from("Windows Backup");
    let mut efi_drive = None;
    let mut allow_destructive = false;
    let mut no_reboot = false;
    let mut args = arguments.into_iter();
    while let Some(flag) = args.next() {
        let value = |name: &str, args: &mut std::vec::IntoIter<String>| {
            args.next()
                .ok_or_else(|| err(&format!("{name} requires a value")))
        };
        match flag.as_str() {
            "--operation" => {
                operation = Some(match value("--operation", &mut args)?.as_str() {
                    "probe" => Operation::Probe,
                    "backup" => Operation::Backup,
                    "restore" | "restore-existing" => Operation::RestoreExisting,
                    "create-secondary" => Operation::CreateSecondary,
                    other => return Err(err(&format!("unsupported operation: {other}"))),
                });
            }
            "--source-drive" => {
                source_drive = Some(parse_drive(&value("--source-drive", &mut args)?)?)
            }
            "--target-drive" => {
                target_drive = Some(parse_drive(&value("--target-drive", &mut args)?)?)
            }
            "--image-path" => image_path = Some(value("--image-path", &mut args)?),
            "--wim-index" => {
                wim_index = value("--wim-index", &mut args)?
                    .parse()
                    .map_err(|_| err("--wim-index must be a positive integer"))?;
            }
            "--boot-menu-name" => boot_menu_name = value("--boot-menu-name", &mut args)?,
            "--efi-drive" => efi_drive = Some(parse_drive(&value("--efi-drive", &mut args)?)?),
            "--allow-destructive" => allow_destructive = true,
            "--no-reboot" => no_reboot = true,
            other => return Err(err(&format!("unknown prepare option: {other}"))),
        }
    }
    let operation = operation.ok_or_else(|| err("--operation is required"))?;
    let source_drive = source_drive.ok_or_else(|| err("--source-drive is required"))?;
    let target_drive = target_drive.unwrap_or(source_drive);
    if wim_index == 0 {
        return Err(err("--wim-index must be greater than zero"));
    }
    if boot_menu_name.trim().is_empty()
        || boot_menu_name.chars().count() > 256
        || boot_menu_name.chars().any(char::is_control)
    {
        return Err(err("--boot-menu-name is invalid"));
    }
    if operation != Operation::Probe && image_path.is_none() {
        return Err(err("--image-path is required for backup and restore"));
    }
    Ok(PrepareOptions {
        operation,
        source_drive,
        target_drive,
        image_path,
        wim_index,
        boot_menu_name,
        efi_drive,
        allow_destructive,
        no_reboot,
    })
}

fn prepare_task(
    executable_dir: &Path,
    workspace: &VolumeIdentity,
    target: VolumeIdentity,
    options: PrepareOptions,
) -> Result<(), TaskError> {
    assert_environment(options.source_drive)?;
    let source = volume_identity(options.source_drive)?;
    require_windows_volume(&source, "source")?;
    assert_bitlocker_off(options.source_drive)?;
    if workspace.is_reserved_partition() {
        return Err(err("program directory cannot be on EFI, MSR or Recovery"));
    }
    let (image_volume, image_path, image_relative) = if let Some(path) = &options.image_path {
        let (path, volume, relative) =
            image_path_info(path, options.operation != Operation::Backup)?;
        (volume, Some(path), relative)
    } else {
        (workspace.clone(), None, String::new())
    };
    if image_volume.is_reserved_partition() {
        return Err(err("image volume cannot be EFI, MSR or Recovery"));
    }
    assert_bitlocker_off(
        image_volume
            .drive_letter
            .ok_or_else(|| err("image volume has no drive letter"))?,
    )?;
    assert_bitlocker_off(options.target_drive)?;
    let recovery = recovery_identity()?;
    let efi = efi_identity(options.efi_drive)?;
    if workspace.same_partition(&recovery) || workspace.same_partition(&efi) {
        return Err(err("program directory cannot be on Recovery or EFI volume"));
    }
    if image_volume.same_partition(&recovery) || image_volume.same_partition(&efi) {
        return Err(err("image volume cannot be on Recovery or EFI volume"));
    }
    validate_operation_inputs(
        &source,
        &target,
        &image_volume,
        image_path.as_deref(),
        &options,
    )?;
    ensure_workspace_capacity(workspace, &recovery)?;

    let store = TaskStore::new(executable_dir);
    let boot_plan = match options.operation {
        Operation::CreateSecondary => BootMode::AddSecondary,
        _ => BootMode::ReturnExisting,
    };
    let mut task = Task::new(
        options.operation,
        backuprestore_core::BootPlan {
            mode: boot_plan,
            previous_bcd_sha256: None,
            menu_name: if options.operation == Operation::CreateSecondary {
                Some(options.boot_menu_name.clone())
            } else {
                None
            },
            boot_sequence_requested: true,
        },
    );
    task.source = Some(source.clone());
    task.workspace_volume = Some(workspace.clone());
    match options.operation {
        Operation::Backup => {
            task.destination = Some(DestinationSpec {
                volume: image_volume.clone(),
                absolute_path: image_path.clone(),
                relative_path: image_relative.clone(),
            });
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let path = image_path.as_ref().expect("restore image path validated");
            task.image = Some(ImageSpec {
                volume: image_volume.clone(),
                absolute_path: Some(path.clone()),
                relative_path: image_relative.clone(),
                sha256: sha256_file(path)?,
                size_bytes: fs::metadata(path)?.len(),
                index: options.wim_index,
            });
            task.target = Some(TargetSpec {
                volume: target.clone(),
                role: if options.operation == Operation::CreateSecondary {
                    TargetRole::NewWindows
                } else {
                    TargetRole::ExistingWindows
                },
                boot_menu_name: (options.operation == Operation::CreateSecondary)
                    .then(|| options.boot_menu_name.clone()),
                minimum_size_bytes: target.partition_size,
            });
        }
        Operation::Probe => {}
    }

    let task_dir = store.task_dir(&task.task_id)?;
    let prepare_log = task_dir.join("prepare.log");
    let bootstrap_bcd = executable_dir.join(format!(".backuprestore-{}.bcd", task.task_id));
    let bootstrap_log = executable_dir.join(r"logs\prepare-bootstrap.log");
    if let Some(parent) = bootstrap_log.parent() {
        fs::create_dir_all(parent)?;
    }
    let bootstrap_arg = bootstrap_bcd.to_string_lossy().into_owned();
    run_logged("bcdedit.exe", &["/export", &bootstrap_arg], &bootstrap_log)?;
    task.boot_plan.previous_bcd_sha256 = Some(sha256_file(&bootstrap_bcd)?);
    let result = prepare_payload(
        executable_dir,
        &store,
        &mut task,
        &recovery,
        &efi,
        &options,
        &prepare_log,
        &bootstrap_bcd,
    );
    let _ = fs::remove_file(&bootstrap_bcd);
    if let Err(error) = result {
        if task_dir.exists() {
            let _ = store.write_failure(&mut task, 1, error.to_string());
            let _ = append_log(&prepare_log, &format!("Preparation failed: {error}"));
        }
        return Err(error);
    }
    write_json_atomic(
        executable_dir.join("last-task.json"),
        &json!({
            "taskId": task.task_id,
            "operation": task.operation,
            "taskRoot": task_dir,
            "statusJson": store.status_path(&task.task_id)?,
            "recoveryLog": store.log_path(&task.task_id)?,
            "prepareLog": prepare_log,
            "imagePath": image_path,
            "created": Utc::now(),
        }),
    )?;
    println!("TASK_ID={}", task.task_id);
    println!("TASK_ROOT={}", task_dir.display());
    Ok(())
}

fn prepare_payload(
    executable_dir: &Path,
    store: &TaskStore,
    task: &mut Task,
    recovery: &VolumeIdentity,
    efi: &VolumeIdentity,
    options: &PrepareOptions,
    log: &Path,
    bootstrap_bcd: &Path,
) -> Result<(), TaskError> {
    let task_dir = store.task_dir(&task.task_id)?;
    let payload = task_dir.join("payload");
    let original = task_dir.join("original");
    let stage = task_dir.join("stage");
    let mount = task_dir.join("mount");
    store.create(task)?;
    fs::create_dir_all(&payload)?;
    fs::create_dir_all(&original)?;
    fs::create_dir_all(&stage)?;
    fs::create_dir_all(&mount)?;
    append_log(log, "Rust preparation started")?;

    fs::copy(bootstrap_bcd, task_dir.join("bcd-before-export"))?;

    let recovery_letter = ensure_volume_mounted(recovery, 'R', log)?;
    let registered_wim = PathBuf::from(format!(
        r"{}:\Recovery\WindowsRE\Winre.wim",
        recovery_letter
    ));
    if !registered_wim.is_file() {
        return Err(err("registered WinRE image is missing"));
    }
    fs::copy(&registered_wim, original.join("Winre.wim"))?;
    let original_hash = sha256_file(original.join("Winre.wim"))?;

    for name in ["RecoveryLauncher.cmd", "winpeshl.ini", "Recovery.exe"] {
        let source = executable_dir.join(name);
        if !source.is_file() {
            return Err(err(&format!(
                "required runtime payload is missing: {}",
                source.display()
            )));
        }
        fs::copy(source, payload.join(name))?;
    }
    for runtime in ["VCRUNTIME140.dll", "VCRUNTIME140_1.dll"] {
        let source = executable_dir.join(runtime);
        if source.is_file() {
            fs::copy(source, payload.join(runtime))?;
        }
    }

    let env_path = payload.join("RecoveryTask.env");
    write_recovery_env(&env_path, executable_dir, task, recovery, efi, options)?;
    fs::copy(store.task_path(&task.task_id)?, payload.join("task.json"))?;
    let launcher_hash = sha256_file(payload.join("RecoveryLauncher.cmd"))?;
    let recovery_hash = sha256_file(payload.join("Recovery.exe"))?;
    let task_hash = sha256_file(payload.join("task.json"))?;
    append_env(
        &env_path,
        &[
            ("EXPECTED_LAUNCHER_SHA256", launcher_hash.as_str()),
            ("EXPECTED_RECOVERY_SHA256", recovery_hash.as_str()),
            ("EXPECTED_TASK_SHA256", task_hash.as_str()),
            ("ORIGINAL_WINRE_SHA256", original_hash.as_str()),
        ],
    )?;

    let staged = stage.join("Winre.wim");
    fs::copy(&registered_wim, &staged)?;
    let staged_arg = staged.to_string_lossy().into_owned();
    let mount_arg = mount.to_string_lossy().into_owned();
    run_logged(
        "dism.exe",
        &[
            "/Mount-Image",
            &format!("/ImageFile:{staged_arg}"),
            "/Index:1",
            &format!("/MountDir:{mount_arg}"),
        ],
        log,
    )?;
    let result = inject_winre_payload(&mount, &payload);
    if let Err(error) = result {
        let _ = run_logged(
            "dism.exe",
            &[
                "/Unmount-Image",
                &format!("/MountDir:{mount_arg}"),
                "/Discard",
            ],
            log,
        );
        return Err(error);
    }
    run_logged(
        "dism.exe",
        &[
            "/Unmount-Image",
            &format!("/MountDir:{mount_arg}"),
            "/Commit",
        ],
        log,
    )?;
    thread::sleep(Duration::from_secs(5));
    let staged_hash = sha256_file(&staged)?;
    if !options.no_reboot {
        fs::copy(&staged, &registered_wim)?;
        backuprestore_core::verify_sha256(&registered_wim, &staged_hash)?;
    }

    let manifest = PayloadManifest {
        task_id: task.task_id.clone(),
        launcher_sha256: launcher_hash,
        recovery_sha256: recovery_hash,
        task_sha256: task_hash,
        original_winre_sha256: original_hash,
        staged_winre_sha256: staged_hash,
        created_by_version: backuprestore_core::PROGRAM_VERSION.into(),
        recovery_task_env_sha256: Some(sha256_file(&env_path)?),
    };
    write_json_atomic(task_dir.join("manifest.json"), &manifest)?;
    write_status_env(&task_dir, task, "prepared")?;
    if options.no_reboot {
        append_log(
            log,
            "Task prepared with --no-reboot; registered WinRE unchanged",
        )?;
        return Ok(());
    }
    run_logged("reagentc.exe", &["/boottore"], log)?;
    store.write_transition(task, backuprestore_core::Stage::BootRequested)?;
    write_status_env(&task_dir, task, "boot-requested")?;
    run_logged("shutdown.exe", &["/r", "/t", "0"], log)
}

fn inject_winre_payload(mount: &Path, payload: &Path) -> Result<(), TaskError> {
    let system32 = mount.join(r"Windows\System32");
    if !system32.is_dir() {
        return Err(err("mounted WinRE has no Windows\\System32"));
    }
    for name in [
        "RecoveryLauncher.cmd",
        "RecoveryTask.env",
        "task.json",
        "winpeshl.ini",
        "Recovery.exe",
        "VCRUNTIME140.dll",
        "VCRUNTIME140_1.dll",
    ] {
        let source = if name == "RecoveryTask.env" {
            payload.join(name)
        } else {
            payload.join(name)
        };
        if source.is_file() {
            let target = system32.join(name);
            if target.exists() {
                fs::remove_file(&target)?;
            }
            fs::copy(source, target)?;
        }
    }
    Ok(())
}

fn validate_operation_inputs(
    source: &VolumeIdentity,
    target: &VolumeIdentity,
    image: &VolumeIdentity,
    image_path: Option<&str>,
    options: &PrepareOptions,
) -> Result<(), TaskError> {
    match options.operation {
        Operation::Probe => Ok(()),
        Operation::Backup => {
            if image.same_partition(source) {
                return Err(err("image volume must differ from backup source"));
            }
            let required = source
                .partition_size
                .saturating_sub(volume_free_bytes(source.drive_letter.unwrap())?)
                .saturating_add(RESERVED_BYTES);
            if volume_free_bytes(image.drive_letter.unwrap())? < required {
                return Err(err("backup destination free space is insufficient"));
            }
            Ok(())
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            if !options.allow_destructive {
                return Err(err("restore requires --allow-destructive"));
            }
            if image.same_partition(target) {
                return Err(err("image volume must differ from restore target"));
            }
            if options.operation == Operation::RestoreExisting && !source.same_partition(target) {
                return Err(err("single-system restore target must equal source"));
            }
            if options.operation == Operation::CreateSecondary && source.same_partition(target) {
                return Err(err("second-system target must differ from source"));
            }
            let path = image_path.ok_or_else(|| err("restore image is missing"))?;
            if !Path::new(path).is_file() {
                return Err(err("restore image does not exist"));
            }
            let _ = capture(
                "dism.exe",
                &[
                    "/Get-WimInfo",
                    &format!("/WimFile:{path}"),
                    &format!("/Index:{}", options.wim_index),
                ],
            )?;
            let metadata: Value = read_json(
                Path::new(path)
                    .parent()
                    .ok_or_else(|| err("image has no parent"))?
                    .join("metadata.json"),
            )?;
            let expected = metadata
                .get("imageSha256")
                .and_then(Value::as_str)
                .ok_or_else(|| err("backup metadata has no imageSha256"))?;
            let actual = sha256_file(path)?;
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(err("restore image hash does not match metadata"));
            }
            let minimum = metadata
                .get("minimumTargetSize")
                .and_then(Value::as_u64)
                .or_else(|| {
                    metadata
                        .get("source")
                        .and_then(|source| source.get("partitionSize"))
                        .and_then(Value::as_u64)
                })
                .ok_or_else(|| err("backup metadata has no target-size requirement"))?;
            if target.partition_size < minimum {
                return Err(err("restore target is too small"));
            }
            Ok(())
        }
    }
}

fn image_path_info(
    value: &str,
    require_existing_file: bool,
) -> Result<(String, VolumeIdentity, String), TaskError> {
    validate_absolute_path(value)?;
    let normalized = value.replace('/', "\\");
    let drive = parse_drive(&normalized[..2])?;
    let root = format!(r"{}:\", drive);
    if !normalized[..3].eq_ignore_ascii_case(&root) {
        return Err(err("image path is not rooted on its drive"));
    }
    let relative = normalized[3..].to_string();
    backuprestore_core::validate_relative_path(&relative)?;
    let path = PathBuf::from(&normalized);
    if require_existing_file && !path.is_file() {
        return Err(err(&format!("image does not exist: {normalized}")));
    }
    if !require_existing_file && !path.parent().is_some_and(Path::is_dir) {
        return Err(err(&format!(
            "backup image parent directory does not exist: {normalized}"
        )));
    }
    Ok((normalized, volume_identity(drive)?, relative))
}

fn write_recovery_env(
    path: &Path,
    executable_dir: &Path,
    task: &Task,
    recovery: &VolumeIdentity,
    efi: &VolumeIdentity,
    options: &PrepareOptions,
) -> Result<(), TaskError> {
    let workspace = task
        .workspace_volume
        .as_ref()
        .ok_or_else(|| err("workspace is missing"))?;
    let source = task
        .source
        .as_ref()
        .ok_or_else(|| err("source is missing"))?;
    let task_root_rel = format!(
        "{}\\tasks\\{}",
        workspace_relative(executable_dir)?,
        task.task_id
    );
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    put_value(&mut values, "TASK_ID", task.task_id.clone());
    put_value(
        &mut values,
        "OPERATION",
        operation_name(task.operation).to_string(),
    );
    insert_identity(&mut values, "WORKSPACE", workspace);
    put_value(&mut values, "WORKSPACE_ROOT_REL", task_root_rel);
    insert_identity(&mut values, "RECOVERY", recovery);
    insert_identity(&mut values, "SOURCE", source);
    if let Some(source_drive) = source.drive_letter {
        let source_free = volume_free_bytes(source_drive)?;
        put_value(
            &mut values,
            "SOURCE_USED_BYTES",
            source
                .partition_size
                .saturating_sub(source_free)
                .to_string(),
        );
    }
    put_value(&mut values, "RESERVED_BYTES", RESERVED_BYTES.to_string());
    if task.operation == Operation::Backup {
        put_value(
            &mut values,
            "MINIMUM_TARGET_SIZE",
            source.partition_size.to_string(),
        );
    }
    if let Ok(computer) = env::var("COMPUTERNAME") {
        put_value(&mut values, "COMPUTERNAME", computer);
    }
    insert_identity(&mut values, "EFI", efi);
    if let Some(destination) = task.destination.as_ref() {
        insert_identity(&mut values, "IMAGE", &destination.volume);
        put_value(
            &mut values,
            "IMAGE_ABSOLUTE_PATH",
            destination.absolute_path.clone().unwrap_or_default(),
        );
        put_value(
            &mut values,
            "IMAGE_RELATIVE_PATH",
            destination.relative_path.clone(),
        );
    }
    if let Some(image) = task.image.as_ref() {
        insert_identity(&mut values, "IMAGE", &image.volume);
        put_value(
            &mut values,
            "IMAGE_ABSOLUTE_PATH",
            image.absolute_path.clone().unwrap_or_default(),
        );
        put_value(
            &mut values,
            "IMAGE_RELATIVE_PATH",
            image.relative_path.clone(),
        );
        put_value(&mut values, "IMAGE_SHA256", image.sha256.clone());
        put_value(&mut values, "WIM_INDEX", image.index.to_string());
    }
    if let Some(target) = task.target.as_ref() {
        insert_identity(&mut values, "TARGET", &target.volume);
        put_value(
            &mut values,
            "MINIMUM_TARGET_SIZE",
            target.minimum_size_bytes.to_string(),
        );
    }
    put_value(
        &mut values,
        "ALLOW_DESTRUCTIVE",
        if options.allow_destructive {
            "YES"
        } else {
            "NO"
        }
        .into(),
    );
    put_value(
        &mut values,
        "BOOT_MENU_NAME",
        options.boot_menu_name.clone(),
    );
    put_value(&mut values, "TASK_CREATED", Utc::now().to_rfc3339());
    put_value(
        &mut values,
        "PROGRAM_VERSION",
        backuprestore_core::PROGRAM_VERSION.into(),
    );
    let mut text = String::new();
    for (key, value) in values {
        if value.contains(['\r', '\n', '=']) {
            return Err(err(&format!(
                "recovery environment value is invalid: {key}"
            )));
        }
        text.push_str(&format!("{key}={value}\r\n"));
    }
    fs::write(path, text)?;
    Ok(())
}

fn put_value(values: &mut BTreeMap<String, String>, key: &str, value: String) {
    values.insert(key.to_string(), value);
}

fn insert_identity(values: &mut BTreeMap<String, String>, prefix: &str, identity: &VolumeIdentity) {
    let insert = |suffix: &str, value: String, values: &mut BTreeMap<String, String>| {
        values.insert(format!("{prefix}_{suffix}"), value);
    };
    insert("VOLUME_GUID", identity.volume_guid.clone(), values);
    insert("DISK_GUID", identity.disk_guid.clone(), values);
    insert("PARTITION_GUID", identity.partition_guid.clone(), values);
    insert(
        "DISK_NUMBER",
        identity.disk_number.unwrap_or_default().to_string(),
        values,
    );
    insert(
        "PARTITION_NUMBER",
        identity.partition_number.unwrap_or_default().to_string(),
        values,
    );
    insert(
        "PARTITION_OFFSET",
        identity.partition_offset.to_string(),
        values,
    );
    insert(
        "PARTITION_SIZE",
        identity.partition_size.to_string(),
        values,
    );
    insert(
        "PARTITION_TYPE_GUID",
        identity.partition_type_guid.clone(),
        values,
    );
    insert("FILESYSTEM", identity.filesystem.clone(), values);
    insert("VOLUME_SERIAL", identity.volume_serial.clone(), values);
}

fn append_env(path: &Path, values: &[(&str, &str)]) -> Result<(), TaskError> {
    let mut file = OpenOptions::new().append(true).open(path)?;
    for (key, value) in values {
        writeln!(file, "{key}={value}")?;
    }
    file.sync_all()?;
    Ok(())
}

fn write_status_env(task_dir: &Path, task: &Task, stage: &str) -> Result<(), TaskError> {
    fs::write(
        task_dir.join("status.env"),
        format!(
            "task_id={}\r\nstage={stage}\r\nprogress={}\r\noperation={}\r\n",
            task.task_id,
            if stage == "boot-requested" { 2 } else { 0 },
            operation_name(task.operation),
        ),
    )?;
    Ok(())
}

fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::Probe => "probe",
        Operation::Backup => "backup",
        Operation::RestoreExisting => "restore-existing",
        Operation::CreateSecondary => "create-secondary",
    }
}

fn assert_environment(source_drive: char) -> Result<(), TaskError> {
    if !Path::new(&format!(
        r"{}:\Windows\System32\config\SYSTEM",
        source_drive
    ))
    .is_file()
    {
        return Err(err(&format!(
            "source volume {source_drive}: has no Windows SYSTEM hive"
        )));
    }
    let reagent = capture("reagentc.exe", &["/info"])?;
    if !(reagent.contains("GLOBALROOT") || reagent.contains("Recovery\\WindowsRE")) {
        return Err(err("Windows RE is disabled or unavailable"));
    }
    Ok(())
}

fn require_administrator() -> Result<(), TaskError> {
    let status = Command::new("fltmc.exe")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(err("administrator elevation is required"))
    }
}

fn require_windows_volume(identity: &VolumeIdentity, name: &str) -> Result<(), TaskError> {
    if identity.is_reserved_partition() || !identity.filesystem.eq_ignore_ascii_case("NTFS") {
        return Err(err(&format!("{name} must be a non-reserved NTFS volume")));
    }
    Ok(())
}

fn assert_bitlocker_off(letter: char) -> Result<(), TaskError> {
    // `-protectionaserrorlevel` has inconsistent exit-code semantics across
    // Windows releases. Parse the inbox status report conservatively instead:
    // only a clearly unencrypted (0.0%) volume is allowed. Any other result,
    // including an unreadable or partially encrypted report, is rejected.
    let output = capture("manage-bde.exe", &["-status", &format!("{letter}:")])?;
    if output.contains("0.0%") && !output.contains("100.0%") {
        Ok(())
    } else {
        Err(err(&format!(
            "BitLocker protection is enabled or volume state is unknown on {letter}:"
        )))
    }
}

fn ensure_workspace_capacity(
    workspace: &VolumeIdentity,
    recovery: &VolumeIdentity,
) -> Result<(), TaskError> {
    let workspace_letter = workspace
        .drive_letter
        .ok_or_else(|| err("workspace volume has no drive letter"))?;
    let recovery_letter = recovery
        .drive_letter
        .ok_or_else(|| err("Recovery volume has no drive letter"))?;
    let registered_wim = PathBuf::from(format!(
        r"{}:\Recovery\WindowsRE\Winre.wim",
        recovery_letter
    ));
    let wim_size = fs::metadata(&registered_wim)
        .map_err(|_| err("registered WinRE image is unavailable for workspace capacity check"))?
        .len();
    let required = wim_size.saturating_mul(2).saturating_add(256 * 1024 * 1024);
    let available = volume_free_bytes(workspace_letter)?;
    if available < required {
        return Err(err(&format!(
            "program directory volume has insufficient free space for WinRE staging: {available} < {required}"
        )));
    }
    Ok(())
}

fn executable_dir() -> Result<PathBuf, TaskError> {
    env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| err("cannot determine executable directory"))
}

fn drive_from_path(path: &Path) -> Result<char, TaskError> {
    let raw = path.to_string_lossy();
    let bytes = raw.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return Err(err("program directory must use a drive-root path"));
    }
    Ok((bytes[0] as char).to_ascii_uppercase())
}

fn workspace_relative(path: &Path) -> Result<String, TaskError> {
    let raw = path.to_string_lossy().replace('/', "\\");
    if raw.len() < 4 {
        return Err(err("program directory is invalid"));
    }
    let relative = raw[3..].trim_matches('\\');
    if relative.is_empty() {
        return Err(err("program directory cannot be a volume root"));
    }
    backuprestore_core::validate_relative_path(relative)?;
    Ok(relative.to_string())
}

fn parse_drive(value: &str) -> Result<char, TaskError> {
    let normalized = value.trim().trim_end_matches(':');
    if normalized.len() == 1 && normalized.as_bytes()[0].is_ascii_alphabetic() {
        Ok((normalized.as_bytes()[0] as char).to_ascii_uppercase())
    } else {
        Err(err("drive must be a single letter"))
    }
}

fn capture(program: &str, args: &[&str]) -> Result<String, TaskError> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(text)
    } else {
        Err(err(&format!(
            "{program} failed with {}: {text}",
            output.status
        )))
    }
}

fn volume_identity(letter: char) -> Result<VolumeIdentity, TaskError> {
    let volume_guid = capture("mountvol.exe", &[&format!("{letter}:"), "/L"])?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| err("mountvol returned no volume identity"))?
        .to_string();
    let script =
        format!("list volume\r\nselect volume {letter}\r\ndetail volume\r\nlist partition\r\n");
    let temp = env::temp_dir().join(format!("BackupRestore-detail-{letter}.txt"));
    fs::write(&temp, script)?;
    let output = capture("diskpart.exe", &["/s", &temp.to_string_lossy()]);
    let _ = fs::remove_file(temp);
    let output = output?;
    let mut selected_numbers = Vec::new();
    for line in output.lines() {
        let lower = line.trim().to_ascii_lowercase();
        if lower.starts_with('*') {
            if let Some(number) = lower
                .split_whitespace()
                .find_map(|part| part.parse::<u32>().ok())
            {
                selected_numbers.push(number);
            }
        }
    }
    // Use DiskPart only for numbers; physical identity comes from native
    // DeviceIoControl queries below, not localised DiskPart text.
    let filesystem = volume_filesystem(letter)?;
    let disk_number = *selected_numbers
        .first()
        .ok_or_else(|| err("could not determine disk number"))?;
    let partition_number = *selected_numbers
        .get(1)
        .ok_or_else(|| err("could not determine partition number"))?;
    let physical = physical_volume_identity(letter, disk_number)?;
    Ok(VolumeIdentity {
        disk_guid: physical.disk_guid,
        partition_guid: physical.partition_guid,
        volume_guid,
        partition_type_guid: physical.partition_type_guid,
        disk_number: Some(disk_number),
        partition_number: Some(partition_number),
        partition_offset: physical.partition_offset,
        partition_size: physical.partition_size,
        filesystem,
        volume_serial: String::new(),
        drive_letter: Some(letter),
    })
}

fn volume_free_bytes(letter: char) -> Result<u64, TaskError> {
    disk_free_space(letter).map(|(_, free)| free)
}

fn disk_free_space(letter: char) -> Result<(u64, u64), TaskError> {
    let path = wide_null(&format!(r"{}:\", letter));
    let mut available = 0_u64;
    let mut total = 0_u64;
    let mut free = 0_u64;
    let ok = unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) };
    if ok == 0 {
        return Err(err(&format!("GetDiskFreeSpaceExW failed for {letter}:")));
    }
    Ok((total, available.min(free)))
}

fn volume_filesystem(letter: char) -> Result<String, TaskError> {
    let path = wide_null(&format!(r"{}:\", letter));
    let mut volume_name = vec![0_u16; 256];
    let mut filesystem = vec![0_u16; 64];
    let mut serial = 0_u32;
    let mut maximum_component_length = 0_u32;
    let mut flags = 0_u32;
    let ok = unsafe {
        GetVolumeInformationW(
            path.as_ptr(),
            volume_name.as_mut_ptr(),
            volume_name.len() as u32,
            &mut serial,
            &mut maximum_component_length,
            &mut flags,
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    };
    if ok == 0 {
        return Err(err(&format!("GetVolumeInformationW failed for {letter}:")));
    }
    let length = filesystem
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(filesystem.len());
    Ok(String::from_utf16_lossy(&filesystem[..length]))
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

struct PhysicalVolumeIdentity {
    disk_guid: String,
    partition_guid: String,
    partition_type_guid: String,
    partition_offset: u64,
    partition_size: u64,
}

fn physical_volume_identity(
    letter: char,
    disk_number: u32,
) -> Result<PhysicalVolumeIdentity, TaskError> {
    let partition_handle = open_device(&wide_null(&format!(r"\\.\{}:", letter)))?;
    let mut partition = vec![0_u8; 160];
    device_io_control(
        partition_handle,
        IOCTL_DISK_GET_PARTITION_INFO_EX,
        &mut partition,
    )?;
    unsafe { CloseHandle(partition_handle) };
    if read_u32(&partition, 0)? != 1 {
        return Err(err("selected volume is not a GPT partition"));
    }
    let partition_offset = read_u64(&partition, 8)?;
    let partition_size = read_u64(&partition, 16)?;
    let partition_type_guid = format_guid(&partition[32..48])?;
    let partition_guid = format_guid(&partition[48..64])?;

    let disk_handle = open_device(&wide_null(&format!(r"\\.\PhysicalDrive{}", disk_number)))?;
    let mut layout = vec![0_u8; 65_536];
    device_io_control(disk_handle, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, &mut layout)?;
    unsafe { CloseHandle(disk_handle) };
    if read_u32(&layout, 0)? != 1 {
        return Err(err("selected disk is not GPT"));
    }
    Ok(PhysicalVolumeIdentity {
        disk_guid: format_guid(&layout[8..24])?,
        partition_guid,
        partition_type_guid,
        partition_offset,
        partition_size,
    })
}

fn open_device(path: &[u16]) -> Result<*mut c_void, TaskError> {
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if handle as isize == -1 {
        Err(err("CreateFileW failed while reading volume identity"))
    } else {
        Ok(handle)
    }
}

fn device_io_control(
    handle: *mut c_void,
    control_code: u32,
    output: &mut [u8],
) -> Result<(), TaskError> {
    let mut returned = 0_u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            control_code,
            null_mut(),
            0,
            output.as_mut_ptr() as *mut c_void,
            output.len() as u32,
            &mut returned,
            null_mut(),
        )
    };
    if ok == 0 {
        Err(err("DeviceIoControl failed while reading GPT identity"))
    } else {
        Ok(())
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, TaskError> {
    let value: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| err("Windows identity buffer is truncated"))?
        .try_into()
        .map_err(|_| err("Windows identity buffer is invalid"))?;
    Ok(u32::from_le_bytes(value))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, TaskError> {
    let value: [u8; 8] = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| err("Windows identity buffer is truncated"))?
        .try_into()
        .map_err(|_| err("Windows identity buffer is invalid"))?;
    Ok(u64::from_le_bytes(value))
}

fn format_guid(bytes: &[u8]) -> Result<String, TaskError> {
    if bytes.len() != 16 {
        return Err(err("Windows GUID buffer is truncated"));
    }
    let data1 = u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .map_err(|_| err("Windows GUID is invalid"))?,
    );
    let data2 = u16::from_le_bytes(
        bytes[4..6]
            .try_into()
            .map_err(|_| err("Windows GUID is invalid"))?,
    );
    let data3 = u16::from_le_bytes(
        bytes[6..8]
            .try_into()
            .map_err(|_| err("Windows GUID is invalid"))?,
    );
    Ok(format!(
        "{{{data1:08x}-{data2:04x}-{data3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

fn recovery_identity() -> Result<VolumeIdentity, TaskError> {
    let output = capture("reagentc.exe", &["/info"])?;
    let lower = output.to_ascii_lowercase();
    let disk = extract_after(&lower, "harddisk")
        .and_then(|value| value.split('\\').next()?.parse().ok())
        .ok_or_else(|| err("WinRE disk identity is missing"))?;
    let partition = extract_after(&lower, "partition")
        .and_then(|value| value.split('\\').next()?.parse().ok())
        .ok_or_else(|| err("WinRE partition identity is missing"))?;
    let mut identity = identity_from_diskpart(disk, partition, 'R')?;
    identity.partition_type_guid = RECOVERY_TYPE.into();
    Ok(identity)
}

fn efi_identity(override_drive: Option<char>) -> Result<VolumeIdentity, TaskError> {
    if let Some(letter) = override_drive {
        let mut identity = volume_identity(letter)?;
        identity.partition_type_guid = EFI_TYPE.into();
        return Ok(identity);
    }
    for letter in 'C'..='Z' {
        let Ok(mut identity) = volume_identity(letter) else {
            continue;
        };
        if identity.filesystem.eq_ignore_ascii_case("FAT32")
            && identity.partition_size <= 1024 * 1024 * 1024
        {
            identity.partition_type_guid = EFI_TYPE.into();
            return Ok(identity);
        }
    }
    Err(err("EFI volume was not found; specify --efi-drive"))
}

fn identity_from_diskpart(
    disk: u32,
    partition: u32,
    preferred: char,
) -> Result<VolumeIdentity, TaskError> {
    let script = format!(
        "select disk {disk}\r\nselect partition {partition}\r\nassign letter={preferred}\r\n"
    );
    let temp = env::temp_dir().join(format!("BackupRestore-identity-{disk}-{partition}.txt"));
    fs::write(&temp, script)?;
    let _ = capture("diskpart.exe", &["/s", &temp.to_string_lossy()]);
    let _ = fs::remove_file(temp);
    volume_identity(preferred)
}

fn ensure_volume_mounted(
    identity: &VolumeIdentity,
    preferred: char,
    _log: &Path,
) -> Result<char, TaskError> {
    if let Some(letter) = identity.drive_letter {
        return Ok(letter);
    }
    let disk = identity
        .disk_number
        .ok_or_else(|| err("volume has no disk number"))?;
    let partition = identity
        .partition_number
        .ok_or_else(|| err("volume has no partition number"))?;
    let mounted = identity_from_diskpart(disk, partition, preferred)?;
    if !mounted
        .volume_guid
        .eq_ignore_ascii_case(&identity.volume_guid)
    {
        return Err(err("mounted volume identity does not match task"));
    }
    Ok(preferred)
}

fn discover_drives() -> Result<Vec<DriveReport>, TaskError> {
    let mut drives = Vec::new();
    for letter in 'C'..='Z' {
        if !Path::new(&format!(r"{}:\", letter)).is_dir() {
            continue;
        }
        let identity = match volume_identity(letter) {
            Ok(identity) => identity,
            Err(error) => {
                eprintln!("{letter}: {error}");
                continue;
            }
        };
        if identity.is_reserved_partition() {
            continue;
        }
        drives.push(DriveReport {
            letter: letter.to_string(),
            label: String::new(),
            filesystem: identity.filesystem,
            size_bytes: identity.partition_size,
            free_bytes: volume_free_bytes(letter)?,
            volume_guid: identity.volume_guid,
            disk_number: identity.disk_number.unwrap_or_default(),
            partition_number: identity.partition_number.unwrap_or_default(),
            partition_type_guid: identity.partition_type_guid,
        });
    }
    Ok(drives)
}

fn parse_dism_images(output: &str) -> Result<Vec<Value>, TaskError> {
    let mut images = Vec::new();
    let mut index = None;
    let mut name = String::new();
    let mut description = String::new();
    for line in output.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "index" => {
                if let Some(index) = index.take() {
                    images.push(
                        json!({"ImageIndex":index,"ImageName":name,"ImageDescription":description}),
                    );
                    name = String::new();
                    description = String::new();
                }
                index = value.trim().parse::<u32>().ok();
            }
            "name" => name = value.trim().to_string(),
            "description" => description = value.trim().to_string(),
            _ => {}
        }
    }
    if let Some(index) = index {
        images.push(json!({"ImageIndex":index,"ImageName":name,"ImageDescription":description}));
    }
    if images.is_empty() {
        Err(err("DISM returned no WIM indexes"))
    } else {
        Ok(images)
    }
}

fn extract_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    text.split_once(marker).map(|(_, value)| value)
}

fn native_architecture() -> String {
    env::var("PROCESSOR_ARCHITEW6432")
        .or_else(|_| env::var("PROCESSOR_ARCHITECTURE"))
        .unwrap_or_else(|_| "unknown".into())
        .to_ascii_lowercase()
}
