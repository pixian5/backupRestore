//! Normal-Windows preparation implemented in Rust.
//!
//! The desktop program does not delegate task creation or system preparation
//! to PowerShell.  This module owns the public `prepare`, `list-volumes`,
//! `inspect-environment` and `wim-info` commands; it uses only Rust file I/O
//! plus the Windows inbox command-line tools that perform the OS operations.

use backuprestore_core::{
    BootMode, DestinationSpec, ImageSpec, Operation, PayloadManifest, TargetRole, TargetSpec, Task,
    TaskError, TaskStore, VolumeIdentity, sha256_file, validate_absolute_path, write_json_atomic,
};
use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::env;
use std::ffi::c_void;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr::null_mut;
use std::thread;
use std::time::Duration;

use crate::{append_log, capture_logged, err, run_logged};

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
    fn GetLastError() -> u32;
}

const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const FILE_SHARE_DELETE: u32 = 0x0000_0004;
const OPEN_EXISTING: u32 = 3;
const IOCTL_DISK_GET_PARTITION_INFO_EX: u32 = 0x0007_0048;
const IOCTL_DISK_GET_DRIVE_LAYOUT_EX: u32 = 0x0007_0050;
// STORAGE_DEVICE_NUMBER is returned for a volume handle and is independent
// of DiskPart's localized table headings or volume numbering.
const IOCTL_STORAGE_GET_DEVICE_NUMBER: u32 = 0x002d_1080;

const RESERVED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const EFI_TYPE: &str = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}";
const RECOVERY_TYPE: &str = "{de94bba4-06d1-4d40-a16a-bfd50179d6ac}";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub(crate) struct PrepareOptions {
    operation: Operation,
    source_drive: char,
    /// Only restore operations need a target. Keeping it optional prevents
    /// hidden probe/backup UI fields from accidentally becoming destructive
    /// validation inputs.
    target_drive: Option<char>,
    image_path: Option<String>,
    wim_index: u32,
    boot_menu_name: String,
    efi_drive: Option<char>,
    test_fault: Option<String>,
    allow_destructive: bool,
    no_reboot: bool,
    /// WIM 压缩率：max/fast/none，仅备份首次创建时生效。
    compress: Option<String>,
    /// 由自动重定位（还原目标 == 程序所在卷）启动的副本，跳过重定位检查。
    relocated: bool,
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
    /// True when the mounted volume contains a bootable Windows installation
    /// (`\Windows\System32\Config\SYSTEM`). Lets the GUI label current and
    /// offline Windows volumes so a user can tell them apart from plain data
    /// partitions before choosing a source or restore target.
    has_windows_installation: bool,
}

pub(crate) fn prepare(arguments: Vec<String>) -> Result<(), TaskError> {
    let options = parse_prepare_options(arguments)?;
    let executable_dir = executable_dir()?;
    let workspace_drive = drive_from_path(&executable_dir)?;
    if matches!(
        options.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) && options.target_drive == Some(workspace_drive)
    {
        if options.relocated {
            // 副本已运行在非还原目标卷，理论不会到达这里；防御性放行。
            // （重定位目标=镜像所在卷，镜像卷必须≠还原目标卷）
        } else {
            // 程序在待还原分区上：自动把程序复制到镜像所在卷，再从副本重启
            // prepare（带 --relocated），用户无需手工移动程序。
            return relocate_to_image_volume(&options, &executable_dir);
        }
    }
    let workspace = volume_identity(workspace_drive)?;
    let target = if matches!(
        options.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) {
        volume_identity(
            options
                .target_drive
                .ok_or_else(|| err("--target-drive is required for restore operations"))?,
        )?
    } else {
        workspace.clone()
    };
    if matches!(
        options.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) && workspace.same_partition(&target)
    {
        return Err(restore_workspace_target_error(
            workspace.drive_letter.unwrap_or(workspace_drive),
        ));
    }
    // Perform the no-overwrite guard before elevation. A direct CLI call must
    // fail locally without triggering UAC or touching any boot configuration.
    require_administrator()?;
    prepare_task(&executable_dir, &workspace, target, options)
}

/// 还原目标分区 == 程序所在分区时，自动把程序运行时复制到镜像所在卷，
/// 再从副本启动 prepare（带 --relocated），旧实例退出。用户无需手工移动。
///
/// 复制内容：主程序（BackupRestore.exe/当前 exe）+ Recovery.exe（同一二进制
/// 的副本，winpeshl 按此名启动）+ RecoveryLauncher.cmd + winpeshl.ini +
/// VCRUNTIME 运行库。目标目录固定为 `{镜像盘符}:\backupRestore-package`。
#[cfg(windows)]
fn relocate_to_image_volume(options: &PrepareOptions, executable_dir: &Path) -> Result<(), TaskError> {
    use std::os::windows::process::CommandExt;
    let image_path = options
        .image_path
        .as_ref()
        .ok_or_else(|| err("relocation requires --image-path"))?;
    let image_drive = drive_from_path(Path::new(image_path))?;
    let target_drive = options
        .target_drive
        .ok_or_else(|| err("relocation requires --target-drive"))?;
    if image_drive == target_drive {
        return Err(err(
            "cannot relocate: image volume must differ from the restore target",
        ));
    }
    let current_exe = std::env::current_exe()?;
    let dest_dir = PathBuf::from(format!("{image_drive}:\\backupRestore-package"));
    fs::create_dir_all(&dest_dir)?;

    // 主程序：以 BackupRestore.exe 为名复制；若当前 exe 名不同也原样复制。
    let exe_name = current_exe
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "BackupRestore.exe".to_string());
    let dest_exe = dest_dir.join(&exe_name);
    fs::copy(&current_exe, &dest_exe)?;
    let dest_recovery = dest_dir.join("Recovery.exe");
    fs::copy(&current_exe, &dest_recovery)?;
    // 启动包装器与配置：winpeshl.ini 指定 RecoveryLauncher.cmd 作为入口。
    for name in ["RecoveryLauncher.cmd", "winpeshl.ini"] {
        let source = executable_dir.join(name);
        if source.is_file() {
            fs::copy(&source, dest_dir.join(name))?;
        }
    }
    for runtime in ["VCRUNTIME140.dll", "VCRUNTIME140_1.dll"] {
        let source = executable_dir.join(runtime);
        if source.is_file() {
            fs::copy(&source, dest_dir.join(runtime))?;
        }
    }
    // MD5 校验副本与源一致，防止复制中途损坏。
    if backuprestore_core::sha256_file(&dest_exe)? != backuprestore_core::sha256_file(&current_exe)?
    {
        return Err(err("relocated executable hash mismatch; refusing to launch"));
    }
    // 从副本重启 prepare：原参数 + --relocated，隐藏窗口，继承管理员令牌。
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    arguments.push("--relocated".to_string());
    let mut command = std::process::Command::new(&dest_exe);
    command.args(&arguments);
    command.creation_flags(CREATE_NO_WINDOW);
    command.spawn().map_err(|error| {
        err(&format!(
            "failed to start relocated prepare at {}: {error}",
            dest_exe.display()
        ))
    })?;
    Ok(())
}

#[cfg(not(windows))]
fn relocate_to_image_volume(_options: &PrepareOptions, _executable_dir: &Path) -> Result<(), TaskError> {
    Err(err("auto-relocation is only available on Windows"))
}

/// 兜底报错：workspace 卷与还原目标卷同分区但盘符不同（如挂载点场景），
/// 无法自动重定位时给出明确提示。常规场景（盘符相同）已由自动重定位接管。
fn restore_workspace_target_error(workspace_drive: char) -> TaskError {
    err(&format!(
        "Cannot start restore: the program directory is on {workspace_drive}:, which is the restore target. Move the entire BackupRestore folder to another volume and run it again. No task, WinRE, BCD or reboot was requested."
    ))
}

fn restore_reserved_target_error() -> TaskError {
    err("EFI/MSR/Recovery partitions cannot be restore targets")
}

pub(crate) fn list_volumes() -> Result<(), TaskError> {
    let drives = discover_drives()?;
    println!("{}", serde_json::to_string(&drives)?);
    Ok(())
}

pub(crate) fn inspect_environment() -> Result<(), TaskError> {
    let output = capture("cmd.exe", &["/d", "/c", "ver"])?;
    // `ver` is localized and may use the active Windows code page.  The
    // status panel is UTF-8 Rust text, so retain only its invariant ASCII
    // version token instead of leaking mojibake such as `�本` into Chinese UI.
    let windows = windows_version_summary(&output);
    let architecture = native_architecture();
    let reagent = capture("reagentc.exe", &["/info"])?;
    let winre_available = reagent.contains("GLOBALROOT") || reagent.contains("Recovery\\WindowsRE");
    println!(
        "{}",
        serde_json::to_string(&json!({
            "windows": windows,
            "architecture": architecture,
            "winreAvailable": winre_available,
            "volumes": discover_drives()?,
        }))?
    );
    Ok(())
}

fn windows_version_summary(output: &str) -> String {
    let mut candidate = String::new();
    for character in output.chars() {
        if character.is_ascii_digit() || character == '.' {
            candidate.push(character);
        } else {
            if candidate.starts_with(|c: char| c.is_ascii_digit())
                && candidate.matches('.').count() >= 2
            {
                return format!("Windows {candidate}");
            }
            candidate.clear();
        }
    }
    if candidate.starts_with(|c: char| c.is_ascii_digit()) && candidate.matches('.').count() >= 2 {
        return format!("Windows {candidate}");
    }
    "Windows version unavailable".to_string()
}

#[cfg(test)]
mod environment_tests {
    use super::windows_version_summary;

    #[test]
    fn windows_version_summary_discards_localized_ver_text() {
        assert_eq!(
            windows_version_summary("Microsoft Windows [版本 10.0.26200.9168]\r\n"),
            "Windows 10.0.26200.9168"
        );
        assert_eq!(
            windows_version_summary("Microsoft Windows [Version 10.0.26100.1]\r\n"),
            "Windows 10.0.26100.1"
        );
        assert_eq!(
            windows_version_summary("localized output without a version"),
            "Windows version unavailable"
        );
    }
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

pub(crate) fn parse_prepare_options(arguments: Vec<String>) -> Result<PrepareOptions, TaskError> {
    let mut operation = None;
    let mut source_drive = None;
    let mut target_drive = None;
    let mut image_path = None;
    let mut wim_index = 1_u32;
    let mut boot_menu_name = String::from("Windows Backup");
    let mut efi_drive = None;
    let mut test_fault = None;
    let mut allow_destructive = false;
    let mut no_reboot = false;
    let mut compress = None;
    let mut relocated = false;
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
            "--test-efi-drive" => {
                efi_drive = Some(parse_drive(&value("--test-efi-drive", &mut args)?)?)
            }
            "--test-fault" => {
                let fault = value("--test-fault", &mut args)?;
                if !matches!(
                    fault.as_str(),
                    "identity-env-mismatch"
                        | "bcdboot-failure"
                        | "power-loss-window"
                        | "power-loss-target-erased"
                        | "power-loss-image-applied"
                        | "power-loss-boot-repaired"
                ) {
                    return Err(err(
                        "--test-fault must be identity-env-mismatch, bcdboot-failure, power-loss-window, power-loss-target-erased, power-loss-image-applied, or power-loss-boot-repaired",
                    ));
                }
                test_fault = Some(fault);
            }
            "--allow-destructive" => allow_destructive = true,
            "--no-reboot" => no_reboot = true,
            "--compress" => {
                let level = value("--compress", &mut args)?;
                if !matches!(level.as_str(), "max" | "fast" | "none") {
                    return Err(err("--compress must be max, fast or none"));
                }
                compress = Some(level);
            }
            "--relocated" => relocated = true,
            other => return Err(err(&format!("unknown prepare option: {other}"))),
        }
    }
    let operation = operation.ok_or_else(|| err("--operation is required"))?;
    let source_drive = source_drive.ok_or_else(|| err("--source-drive is required"))?;
    let target_drive = if matches!(
        operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) {
        Some(target_drive.ok_or_else(|| err("--target-drive is required for restore operations"))?)
    } else {
        target_drive
    };
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
    if matches!(
        test_fault.as_deref(),
        Some("identity-env-mismatch" | "bcdboot-failure")
    ) && efi_drive.is_none()
    {
        return Err(err(
            "identity-env-mismatch and bcdboot-failure require --test-efi-drive and are development-only",
        ));
    }
    Ok(PrepareOptions {
        operation,
        source_drive,
        target_drive,
        image_path,
        wim_index,
        boot_menu_name,
        efi_drive,
        test_fault,
        allow_destructive,
        no_reboot,
        compress,
        relocated,
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
    if matches!(
        options.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) {
        // Reject reserved restore targets explicitly before the BitLocker
        // probe. `manage-bde` cannot open EFI/MSR/Recovery partitions, so
        // without this early check a user selecting the EFI or Recovery
        // partition would see a confusing BitLocker error instead of the real
        // reason the target is ineligible.
        if target.is_reserved_partition() {
            return Err(restore_reserved_target_error());
        }
        assert_bitlocker_off(
            options
                .target_drive
                .ok_or_else(|| err("restore target drive is missing"))?,
        )?;
    }
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
    let store = TaskStore::new(executable_dir);
    let bootstrap_log = executable_dir.join(r"logs\prepare-bootstrap.log");
    if let Some(parent) = bootstrap_log.parent() {
        fs::create_dir_all(parent)?;
    }
    let cleanup =
        store.cleanup_terminal_tasks(backuprestore_core::DEFAULT_TERMINAL_TASK_RETENTION)?;
    if !cleanup.removed_task_ids.is_empty()
        || cleanup.skipped_nonterminal > 0
        || !cleanup.skipped_mounted.is_empty()
    {
        append_log(
            &bootstrap_log,
            &format!(
                "terminal task cleanup: removed={}, skipped_nonterminal={}, skipped_mounted={}",
                cleanup.removed_task_ids.len(),
                cleanup.skipped_nonterminal,
                cleanup.skipped_mounted.len()
            ),
        )?;
    }
    ensure_workspace_capacity(workspace, &recovery)?;

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
            // The one-time boot request is enabled only after the BCD
            // snapshot has been exported and hashed below. This lets us run
            // a complete side-effect-free task validation before any BCD or
            // WinRE mutation while still persisting a fully protected plan.
            boot_sequence_requested: false,
        },
    );
    task.source = Some(source.clone());
    task.workspace_volume = Some(workspace.clone());
    task.compress = options.compress.clone();
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

    // Validate the complete task before exporting BCD or touching WinRE. This
    // keeps malformed/reserved/undersized requests side-effect free even when
    // they arrived through the CLI instead of the GUI.
    task.validate()?;

    let task_dir = store.task_dir(&task.task_id)?;
    let prepare_log = task_dir.join("prepare.log");
    let bootstrap_bcd = executable_dir.join(format!(".backuprestore-{}.bcd", task.task_id));
    let bootstrap_arg = bootstrap_bcd.to_string_lossy().into_owned();
    run_logged("bcdedit.exe", &["/export", &bootstrap_arg], &bootstrap_log)?;
    task.boot_plan.previous_bcd_sha256 = Some(sha256_file(&bootstrap_bcd)?);
    task.boot_plan.boot_sequence_requested = true;
    task.validate()?;
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
    // recoveryLog 指向镜像同目录（如 E:\Recovery.log），与 WIM 并排便于查看；
    // GUI「刷新任务状态」据此显示。备份/还原都会把日志写到镜像同目录。
    let recovery_log_path = match options.image_path.as_ref() {
        Some(path) => Path::new(path)
            .parent()
            .map(|parent| parent.join("Recovery.log"))
            .unwrap_or_else(|| PathBuf::from("Recovery.log")),
        None => store.log_path(&task.task_id)?,
    };
    write_json_atomic(
        executable_dir.join("last-task.json"),
        &json!({
            "taskId": task.task_id,
            "operation": task.operation,
            "taskRoot": task_dir,
            "statusJson": store.status_path(&task.task_id)?,
            "recoveryLog": recovery_log_path,
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
    let raw_bcd_hash = snapshot_raw_bcd(&efi, &task_dir, log)?;
    // `bcdedit /export` is a logical export. Importing it may rewrite the
    // binary hive, so new tasks retain a byte-for-byte EFI store snapshot for
    // rollback while old tasks can still use the exported fallback.
    task.boot_plan.previous_bcd_sha256 = Some(raw_bcd_hash);
    task.validate()?;
    // `store.create` runs before the EFI snapshot is captured so the task
    // directory exists for the raw snapshot. Persist the updated boot-plan
    // hash before copying task.json into the WinRE payload; otherwise Recovery
    // would correctly reject a stale payload/task pair.
    write_json_atomic(store.task_path(&task.task_id)?, task)?;

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

    // 必须存在的运行时载荷：启动配置、恢复程序、启动包装器
    // RecoveryLauncher.cmd 读 RecoveryTask.env 后调用 Recovery.exe recover-env，
    // 是 winpeshl.ini 指定的入口，缺失会导致 WinRE 启动后无程序可跑、超时回 Windows。
    for name in ["winpeshl.ini", "Recovery.exe", "RecoveryLauncher.cmd"] {
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
    let recovery_hash = sha256_file(payload.join("Recovery.exe"))?;
    let task_hash = sha256_file(payload.join("task.json"))?;
    append_env(
        &env_path,
        &[("ORIGINAL_WINRE_SHA256", original_hash.as_str())],
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
    let manifest = PayloadManifest {
        task_id: task.task_id.clone(),
        recovery_sha256: recovery_hash,
        task_sha256: task_hash,
        original_winre_sha256: original_hash.clone(),
        staged_winre_sha256: staged_hash.clone(),
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

    // Do not replace the registered WinRE image until every task artifact and
    // its manifest are durable. If any subsequent preparation step fails,
    // restore the original image and BCD snapshot immediately so a failed
    // desktop launch cannot strand the machine with a half-installed WinRE.
    let restore_registered = || -> Result<(), TaskError> {
        fs::copy(original.join("Winre.wim"), &registered_wim)?;
        backuprestore_core::verify_sha256(&registered_wim, &original_hash)?;
        append_log(
            log,
            "Restored original registered WinRE after preparation failure",
        )?;
        Ok(())
    };
    if let Err(error) = (|| {
        fs::copy(&staged, &registered_wim)?;
        backuprestore_core::verify_sha256(&registered_wim, &staged_hash)
    })() {
        let _ = restore_registered();
        return Err(error);
    }
    if let Err(error) = run_logged("reagentc.exe", &["/boottore"], log) {
        let _ = restore_registered();
        return Err(error);
    }
    if let Err(error) = store.write_transition(task, backuprestore_core::Stage::BootRequested) {
        let _ = restore_registered();
        let _ = rollback_boot_request(&task_dir, &efi, log);
        return Err(error);
    }
    if let Err(error) = write_status_env(&task_dir, task, "boot-requested") {
        let _ = restore_registered();
        let _ = rollback_boot_request(&task_dir, &efi, log);
        return Err(error);
    }
    if options.test_fault.as_deref() == Some("power-loss-window") {
        append_log(
            log,
            "Development test fault: stopped after durable boot-requested state; no shutdown requested",
        )?;
        return Ok(());
    }
    if let Err(error) = run_logged("shutdown.exe", &["/r", "/t", "0"], log) {
        let _ = restore_registered();
        let _ = rollback_boot_request(&task_dir, &efi, log);
        return Err(error);
    }
    Ok(())
}

fn snapshot_raw_bcd(
    efi: &VolumeIdentity,
    task_dir: &Path,
    log: &Path,
) -> Result<String, TaskError> {
    let mounted_temporarily = efi.drive_letter.is_none();
    let letter = ensure_volume_mounted(efi, 'S', log)?;
    let source = PathBuf::from(format!(r"{letter}:\EFI\Microsoft\Boot\BCD"));
    let snapshot = task_dir.join("bcd-before-raw");
    let result = (|| {
        if !source.is_file() {
            return Err(err("EFI BCD store is missing while creating raw snapshot"));
        }
        // Keep a textual Boot Manager snapshot as well as the byte-level
        // fallback. A `bcdedit /export` store can expose `{default}` as an
        // alias after import, which is not sufficient to preserve which
        // loader was actually default. Reading the active store before any
        // mutation gives us the concrete display order for secondary mode.
        let source_arg = source.to_string_lossy().into_owned();
        let boot_manager = capture_logged(
            "bcdedit.exe",
            &["/store", &source_arg, "/enum", "all", "/v"],
            log,
        )
        .or_else(|_| capture_logged("bcdedit.exe", &["/enum", "all", "/v"], log))?;
        fs::write(task_dir.join("bcd-before-bootmgr.txt"), boot_manager)?;
        match fs::copy(&source, &snapshot) {
            Ok(_) => {
                let hash = sha256_file(&snapshot)?;
                backuprestore_core::verify_sha256(&source, &hash)?;
                append_log(log, "Captured byte-for-byte EFI BCD snapshot")?;
                Ok(hash)
            }
            Err(copy_error) if copy_error.raw_os_error() == Some(32) => {
                // The active Boot Manager keeps its hive open without
                // sharing, so a raw file copy can fail with
                // ERROR_SHARING_VIOLATION. The bootstrap export was captured
                // from this same active store before WinRE mutation; use it
                // as a logical rollback snapshot and log the fallback.
                let export = task_dir.join("bcd-before-export");
                if !export.is_file() {
                    return Err(copy_error.into());
                }
                let hash = sha256_file(&export)?;
                append_log(
                    log,
                    "Active EFI BCD is locked; using bcdedit logical export for rollback",
                )?;
                Ok(hash)
            }
            Err(error) => Err(error.into()),
        }
    })();
    if mounted_temporarily {
        let _ = Command::new("mountvol.exe")
            .args([format!("{letter}:"), "/D".to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    result
}

fn rollback_boot_request(
    task_dir: &Path,
    efi: &VolumeIdentity,
    log: &Path,
) -> Result<(), TaskError> {
    let raw_snapshot = task_dir.join("bcd-before-raw");
    if raw_snapshot.is_file() {
        let mounted_temporarily = efi.drive_letter.is_none();
        let letter = ensure_volume_mounted(efi, 'S', log)?;
        let store = PathBuf::from(format!(r"{letter}:\EFI\Microsoft\Boot\BCD"));
        let result = (|| {
            let expected = sha256_file(&raw_snapshot)?;
            match fs::copy(&raw_snapshot, &store) {
                Ok(_) => {
                    backuprestore_core::verify_sha256(&store, &expected)?;
                    append_log(
                        log,
                        "Restored byte-for-byte EFI BCD snapshot after preparation failure",
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
            Ok(())
        })();
        if mounted_temporarily {
            let _ = Command::new("mountvol.exe")
                .args([format!("{letter}:"), "/D".to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .status();
        }
        return result;
    }
    let snapshot = task_dir.join("bcd-before-export");
    if !snapshot.is_file() {
        return Err(err(
            "BCD snapshot is missing while rolling back preparation",
        ));
    }
    let snapshot_arg = snapshot.to_string_lossy().into_owned();
    run_logged("bcdedit.exe", &["/import", &snapshot_arg], log)?;
    append_log(
        log,
        "Restored logical BCD snapshot after preparation failure",
    )?;
    Ok(())
}

fn inject_winre_payload(mount: &Path, payload: &Path) -> Result<(), TaskError> {
    let system32 = mount.join(r"Windows\System32");
    if !system32.is_dir() {
        return Err(err("mounted WinRE has no Windows\\System32"));
    }
    for name in [
        "RecoveryTask.env",
        "task.json",
        "winpeshl.ini",
        "Recovery.exe",
        "RecoveryLauncher.cmd",
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
            // Appending is transactional: Recovery copies the current WIM to
            // a same-volume candidate before capture. Reserve that extra copy
            // here so a full destination cannot strand the original half-way
            // through an append attempt.
            let append_candidate_bytes = image_path
                .filter(|path| Path::new(path).is_file())
                .map(|path| fs::metadata(path).map(|metadata| metadata.len()))
                .transpose()?
                .unwrap_or(0);
            let required = required.saturating_add(append_candidate_bytes);
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
            let metadata = crate::read_index_metadata(Path::new(path), options.wim_index)?;
            let expected = &metadata.image_sha256;
            let actual = sha256_file(path)?;
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(err("restore image hash does not match metadata"));
            }
            let minimum = metadata
                .required_target_size()
                .max(metadata.source.partition_size);
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
    if options.test_fault.as_deref() == Some("identity-env-mismatch") {
        put_value(&mut values, "SOURCE_VOLUME_SERIAL", "FAULT-INJECTED".into());
    }
    if let Some(fault) = &options.test_fault {
        put_value(&mut values, "TEST_FAULT", fault.clone());
    }
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
        .creation_flags(CREATE_NO_WINDOW)
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
        .creation_flags(CREATE_NO_WINDOW)
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

pub(crate) fn volume_identity(letter: char) -> Result<VolumeIdentity, TaskError> {
    let volume_guid = capture("mountvol.exe", &[&format!("{letter}:"), "/L"])?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| err("mountvol returned no volume identity"))?
        .to_string();
    let (filesystem, volume_serial) = volume_information(letter)?;
    // Do not parse DiskPart's localized output here.  The volume handle gives
    // us the physical disk number, while PARTITION_INFORMATION_EX contains
    // the authoritative partition number alongside the GPT GUIDs.
    let disk_number = storage_device_number(letter)?;
    let physical = physical_volume_identity(letter, disk_number)?;
    Ok(VolumeIdentity {
        disk_guid: physical.disk_guid,
        partition_guid: physical.partition_guid,
        volume_guid,
        partition_type_guid: physical.partition_type_guid,
        disk_number: Some(disk_number),
        partition_number: Some(physical.partition_number),
        partition_offset: physical.partition_offset,
        partition_size: physical.partition_size,
        filesystem,
        volume_serial,
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

fn volume_information(letter: char) -> Result<(String, String), TaskError> {
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
    Ok((
        String::from_utf16_lossy(&filesystem[..length]),
        format!("{serial:08X}"),
    ))
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

struct PhysicalVolumeIdentity {
    disk_guid: String,
    partition_guid: String,
    partition_type_guid: String,
    partition_number: u32,
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
    let partition_number = read_u32(&partition, 24)?;
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
        partition_number,
        partition_offset,
        partition_size,
    })
}

fn storage_device_number(letter: char) -> Result<u32, TaskError> {
    let handle = open_device(&wide_null(&format!(r"\\.\{}:", letter)))?;
    // STORAGE_DEVICE_NUMBER is three DWORDs: device type, device number and
    // partition number. Only DeviceNumber is used here; the GPT partition
    // number is read from PARTITION_INFORMATION_EX so both values come from
    // the same native identity query family.
    let mut number = [0_u8; 12];
    let result = device_io_control(handle, IOCTL_STORAGE_GET_DEVICE_NUMBER, &mut number)
        .and_then(|_| read_u32(&number, 4));
    unsafe { CloseHandle(handle) };
    result
}

fn open_device(path: &[u16]) -> Result<*mut c_void, TaskError> {
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if handle as isize == -1 {
        let error_code = unsafe { GetLastError() };
        Err(err(&format!(
            "CreateFileW failed while reading volume identity (Windows error {error_code})"
        )))
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
    let identity = identity_from_diskpart(disk, partition, 'R')?;
    if !identity
        .partition_type_guid
        .eq_ignore_ascii_case(RECOVERY_TYPE)
    {
        return Err(err(
            "registered WinRE is not located on a GPT Recovery partition",
        ));
    }
    Ok(identity)
}

fn efi_identity(override_drive: Option<char>) -> Result<VolumeIdentity, TaskError> {
    if let Some(letter) = override_drive {
        let identity = volume_identity(letter)?;
        if !identity.partition_type_guid.eq_ignore_ascii_case(EFI_TYPE) {
            return Err(err("specified EFI drive is not a GPT EFI system partition"));
        }
        return Ok(identity);
    }
    // The system EFI partition is normally hidden and has no drive letter.
    // More importantly, a development/test EFI can already be mounted (for
    // example as E:). It must never win merely because it is visible first:
    // `mountvol /S` is Windows' authoritative way to expose the EFI that the
    // current system actually booted from. Only if that lookup genuinely
    // fails do we fall back to an already-mounted EFI for diagnostics.
    let mut last_error = None;
    for letter in identity_drive_candidates('Z') {
        if !is_drive_letter_available(letter) {
            continue;
        }
        let status = Command::new("mountvol.exe")
            .args([format!("{letter}:"), "/S".to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        if !status
            .as_ref()
            .map(|value| value.success())
            .unwrap_or(false)
        {
            if let Err(error) = status {
                last_error = Some(TaskError::Io(error));
            }
            continue;
        }
        let identity = volume_identity(letter);
        let _ = Command::new("mountvol.exe")
            .args([format!("{letter}:"), "/D".to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        match identity {
            Ok(mut identity) if identity.partition_type_guid.eq_ignore_ascii_case(EFI_TYPE) => {
                // The letter was only a temporary mount used to inspect the
                // hidden system ESP. Clear it before returning: later code
                // must call ensure_volume_mounted again rather than treating
                // an already-removed drive letter as authoritative.
                identity.drive_letter = None;
                return Ok(identity);
            }
            Ok(identity) => {
                last_error = Some(err(&format!(
                    "mountvol /S exposed disk {} partition {} but it is not an EFI partition",
                    identity.disk_number.unwrap_or_default(),
                    identity.partition_number.unwrap_or_default(),
                )));
            }
            Err(error) => last_error = Some(error),
        }
    }
    // Never fall back to an arbitrary mounted EFI here. A development EFI
    // may be visible as E: while the current system EFI is hidden; selecting
    // it would write the wrong BCD and make the task appear successful while
    // leaving the actual Boot Manager unchanged.
    Err(last_error.unwrap_or_else(|| err("system GPT EFI partition was not found")))
}

fn identity_from_diskpart(
    disk: u32,
    partition: u32,
    preferred: char,
) -> Result<VolumeIdentity, TaskError> {
    // First reuse an already-mounted partition.  A fixed preferred letter
    // is only a hint; it may be occupied by an unrelated volume in WinRE.
    for letter in 'C'..='Z' {
        let Ok(identity) = volume_identity(letter) else {
            continue;
        };
        if identity.disk_number == Some(disk) && identity.partition_number == Some(partition) {
            return Ok(identity);
        }
    }

    let mut last_error = None;
    for letter in identity_drive_candidates(preferred) {
        if !is_drive_letter_available(letter) {
            continue;
        }
        let script = format!(
            "select disk {disk}\r\nselect partition {partition}\r\nassign letter={letter}\r\n"
        );
        let temp = env::temp_dir().join(format!(
            "BackupRestore-identity-{disk}-{partition}-{letter}.txt"
        ));
        fs::write(&temp, script)?;
        let result = capture("diskpart.exe", &["/s", &temp.to_string_lossy()]);
        let _ = fs::remove_file(&temp);
        if let Err(error) = result {
            last_error = Some(error);
            continue;
        }
        match volume_identity(letter) {
            Ok(identity)
                if identity.disk_number == Some(disk)
                    && identity.partition_number == Some(partition) =>
            {
                return Ok(identity);
            }
            Ok(identity) => {
                last_error = Some(err(&format!(
                    "DiskPart assigned {letter}: to disk {} partition {}, expected disk {disk} partition {partition}",
                    identity.disk_number.unwrap_or_default(),
                    identity.partition_number.unwrap_or_default(),
                )));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        err(&format!(
            "unable to mount disk {disk} partition {partition} on an available drive letter"
        ))
    }))
}

fn identity_drive_candidates(preferred: char) -> impl Iterator<Item = char> {
    std::iter::once(preferred)
        .chain('C'..='Z')
        .filter(|letter| *letter != 'X')
        .collect::<Vec<_>>()
        .into_iter()
        .fold(Vec::new(), |mut letters, letter| {
            if !letters.contains(&letter) {
                letters.push(letter);
            }
            letters
        })
        .into_iter()
}

fn is_drive_letter_available(letter: char) -> bool {
    let Ok(output) = Command::new("mountvol.exe")
        .args([format!("{letter}:"), "/L".to_string()])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        // `mountvol X: /L` exits with code 1 when the letter has no mount
        // point (the normal free-letter case). Assignment below still has to
        // succeed and is followed by full identity verification.
        return true;
    }
    !String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .any(|line| !line.is_empty())
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
    if !mounted.same_partition(identity)
        || !mounted
            .volume_guid
            .eq_ignore_ascii_case(&identity.volume_guid)
        || mounted.partition_offset != identity.partition_offset
        || mounted.partition_size != identity.partition_size
        || !mounted
            .partition_type_guid
            .eq_ignore_ascii_case(&identity.partition_type_guid)
        || !mounted
            .filesystem
            .eq_ignore_ascii_case(&identity.filesystem)
    {
        return Err(err("mounted volume identity does not match task"));
    }
    mounted
        .drive_letter
        .ok_or_else(|| err("mounted volume has no drive letter"))
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
            has_windows_installation: Path::new(&format!(
                r"{letter}:\Windows\System32\Config\SYSTEM"
            ))
            .is_file(),
        });
    }
    Ok(drives)
}

pub(crate) fn parse_dism_images(output: &str) -> Result<Vec<Value>, TaskError> {
    let mut images = Vec::new();
    let mut index = None;
    let mut name = String::new();
    let mut description = String::new();
    let mut version = String::new();
    let mut architecture = String::new();
    let mut edition = String::new();
    let mut installation_type = String::new();
    let mut size_bytes = None;

    // DISM's text output is the fallback used by the GUI when the optional
    // Get-WindowsImage JSON command is unavailable.  Keep the parser scoped
    // to an active image: the header's own `Version:` line must not become an
    // image version.  Fields beyond Index/Name/Description are emitted when
    // a particular DISM build provides them, while remaining optional for
    // the standard `/Get-WimInfo` output.
    let flush = |images: &mut Vec<Value>,
                 index: &mut Option<u32>,
                 name: &mut String,
                 description: &mut String,
                 version: &mut String,
                 architecture: &mut String,
                 edition: &mut String,
                 installation_type: &mut String,
                 size_bytes: &mut Option<u64>| {
        let Some(index_value) = index.take() else {
            return;
        };
        images.push(json!({
            "ImageIndex": index_value,
            "ImageName": std::mem::take(name),
            "ImageDescription": std::mem::take(description),
            "ImageVersion": std::mem::take(version),
            "Architecture": std::mem::take(architecture),
            "EditionId": std::mem::take(edition),
            "InstallationType": std::mem::take(installation_type),
            "ImageSize": size_bytes.take(),
        }));
    };

    for line in output.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "index" => {
                flush(
                    &mut images,
                    &mut index,
                    &mut name,
                    &mut description,
                    &mut version,
                    &mut architecture,
                    &mut edition,
                    &mut installation_type,
                    &mut size_bytes,
                );
                index = value.parse::<u32>().ok().filter(|value| *value > 0);
            }
            // Ignore image fields before the first valid Index.  This also
            // prevents a localized/header `Version:` from leaking into the
            // first image's metadata.
            "name" if index.is_some() => name = value.to_string(),
            "description" if index.is_some() => description = value.to_string(),
            "version" if index.is_some() => version = value.to_string(),
            "architecture" if index.is_some() => architecture = value.to_string(),
            "edition" | "edition id" if index.is_some() => edition = value.to_string(),
            "installation type" if index.is_some() => installation_type = value.to_string(),
            "size" | "image size" if index.is_some() => size_bytes = parse_dism_size(value),
            _ => {}
        }
    }
    flush(
        &mut images,
        &mut index,
        &mut name,
        &mut description,
        &mut version,
        &mut architecture,
        &mut edition,
        &mut installation_type,
        &mut size_bytes,
    );
    if images.is_empty() {
        Err(err("DISM returned no WIM indexes"))
    } else {
        Ok(images)
    }
}

/// Parse DISM's size field, which is normally an integer followed by
/// `bytes`, but can be formatted with comma separators or binary units by
/// OEM/localized builds.  The GUI consumes a byte count, so normalize all
/// supported forms to bytes and leave malformed values absent.
fn parse_dism_size(value: &str) -> Option<u64> {
    let value = value.trim();
    let start = value.find(|character: char| character.is_ascii_digit())?;
    let value = &value[start..];
    let end = value
        .find(|character: char| !(character.is_ascii_digit() || matches!(character, ',' | '.')))
        .unwrap_or(value.len());
    let number = value[..end].replace(',', "");
    let numeric = number.parse::<f64>().ok()?;
    let unit = value[end..].trim().to_ascii_lowercase();
    let multiplier = if unit.starts_with("tib") || unit.starts_with("tb") {
        1024_f64.powi(4)
    } else if unit.starts_with("gib") || unit.starts_with("gb") {
        1024_f64.powi(3)
    } else if unit.starts_with("mib") || unit.starts_with("mb") {
        1024_f64.powi(2)
    } else if unit.starts_with("kib") || unit.starts_with("kb") {
        1024_f64
    } else {
        1_f64
    };
    let bytes = numeric * multiplier;
    if !bytes.is_finite() || bytes < 0.0 || bytes > u64::MAX as f64 {
        return None;
    }
    Some(bytes.round() as u64)
}

#[cfg(test)]
mod wim_parser_tests {
    use super::parse_dism_images;

    #[test]
    fn parses_multiple_indexes_and_optional_metadata() {
        let output = r#"
Deployment Image Servicing and Management tool
Version: 10.0.26100.1

Details for image : install.wim

Index : 1
Name : Windows 11 Home
Description : Windows 11 Home
Size : 15,728,640 bytes
Architecture : arm64
Edition Id : Core
Installation Type : Client

Index : 2
Name : Windows 11 Pro
Description : Windows 11 Pro for testing
Version : 10.0.26100.1
Size : 16,384 MiB
Architecture : arm64
Edition : Professional
Installation Type : Client
"#;

        let images = parse_dism_images(output).expect("DISM output should parse");
        assert_eq!(images.len(), 2);
        assert_eq!(images[0]["ImageIndex"], 1);
        assert_eq!(images[0]["ImageName"], "Windows 11 Home");
        assert_eq!(images[0]["ImageSize"], 15_728_640);
        assert_eq!(images[0]["Architecture"], "arm64");
        assert_eq!(images[0]["EditionId"], "Core");
        assert_eq!(images[1]["ImageIndex"], 2);
        assert_eq!(images[1]["ImageVersion"], "10.0.26100.1");
        assert_eq!(images[1]["ImageSize"], 16_384_u64 * 1024 * 1024);
        assert_eq!(images[1]["EditionId"], "Professional");
    }

    #[test]
    fn ignores_header_version_and_rejects_missing_indexes() {
        let output = "Version: 10.0.26100.1\nName: not an image\n";
        assert!(parse_dism_images(output).is_err());
    }
}

#[cfg(test)]
mod prepare_safety_tests {
    use super::{restore_reserved_target_error, restore_workspace_target_error};

    #[test]
    fn same_drive_restore_is_rejected_before_privileged_identity_queries() {
        assert_eq!(
            restore_workspace_target_error('C').to_string(),
            "invalid task: Cannot start restore: the program directory is on C:, which is the restore target. Move the entire BackupRestore folder to another volume and run it again. No task, WinRE, BCD or reboot was requested."
        );
    }

    #[test]
    fn reserved_restore_target_error_names_the_role() {
        assert_eq!(
            restore_reserved_target_error().to_string(),
            "invalid task: EFI/MSR/Recovery partitions cannot be restore targets"
        );
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
