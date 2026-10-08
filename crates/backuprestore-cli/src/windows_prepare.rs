//! Normal-Windows preparation implemented in Rust.
//!
//! The desktop program does not delegate task creation or system preparation
//! to PowerShell.  This module owns the public `prepare`, `list-volumes`,
//! `inspect-environment` and `wim-info` commands; it uses only Rust file I/O
//! plus the Windows inbox command-line tools that perform the OS operations.

use backuprestore_core::{
    BootMode, DestinationSpec, ImageSpec, Operation, PayloadManifest, TargetRole, TargetSpec, Task,
    TaskError, TaskStore, VolumeIdentity, VolumeRoles, canonical_compression, sha256_file,
    validate_absolute_path, validate_volume_roles, write_json_atomic,
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

use crate::{append_log, capture_logged, err, run_logged, winre_payload};

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
    fn GetVolumeNameForVolumeMountPointW(
        volume_mount_point: *const u16,
        volume_name: *mut u16,
        buffer_length: u32,
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
#[allow(dead_code)]
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
    /// WIM 索引名（备份时 DISM /Name；默认 "Windows Backup"）。
    image_name: Option<String>,
    /// 保留最近 N 个 WIM 索引（备份追加成功后清理更旧索引；None=不清理）。
    keep_indexes: Option<u32>,
    boot_menu_name: String,
    efi_drive: Option<char>,
    test_fault: Option<String>,
    allow_destructive: bool,
    no_reboot: bool,
    /// WIM 压缩率：fast/none，仅备份首次创建时生效。
    compress: Option<String>,
    /// 还原时跳过镜像哈希校验（GUI 已向用户确认档案缺失/不匹配仍继续）。
    force_restore_hash: bool,
    /// 还原时严格校验镜像哈希（GUI 勾选了对比哈希，默认未勾选仅对比大小）。
    verify_hash: bool,
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
        // This must happen before the identity probe, UAC, task creation, or
        // any WinRE/BCD work. The application never copies itself or picks a
        // substitute volume on the user's behalf.
        return Err(restore_workspace_target_error(workspace_drive));
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

/// Program and restore target must remain on different partitions. This is
/// deliberately a local refusal: moving the entire program directory is the
/// only supported resolution, never an automatic copy or volume selection.
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

pub(crate) fn wim_info(image_path: String, skip_hash: bool) -> Result<(), TaskError> {
    use backuprestore_core::read_json;

    validate_absolute_path(&image_path)?;
    let path = PathBuf::from(&image_path);
    if !path.is_file() {
        return Err(err(&format!("WIM image does not exist: {image_path}")));
    }

    // ── 优先尝试读取同目录 sidecar 元数据（*.metadata.json） ──────────────────
    // 命名规则：<wim文件名>.index-<N>.metadata.json
    // 若存在任意一个，直接反序列化呈现，跳过 DISM 解析和哈希计算（毫秒级）。
    if let Some(parent) = path.parent() {
        let wim_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // 扫描同目录所有 .index-N.metadata.json 文件，按索引号排序
        let mut sidecar_entries: Vec<(u32, backuprestore_core::BackupMetadata)> = Vec::new();
        if let Ok(dir) = std::fs::read_dir(parent) {
            let prefix = format!("{wim_name}.index-");
            let suffix = ".metadata.json";
            for entry in dir.flatten() {
                let fname = entry.file_name();
                let fname_str = fname.to_string_lossy();
                if fname_str.starts_with(&prefix) && fname_str.ends_with(suffix) {
                    // 提取索引号
                    let mid = &fname_str[prefix.len()..fname_str.len() - suffix.len()];
                    if let Ok(idx) = mid.parse::<u32>()
                        && let Ok(meta) =
                            read_json::<backuprestore_core::BackupMetadata>(entry.path())
                    {
                        sidecar_entries.push((idx, meta));
                    }
                }
            }
        }
        if !sidecar_entries.is_empty() {
            // 按索引号升序排列
            sidecar_entries.sort_by_key(|(idx, _)| *idx);
            // 用 sidecar 数据直接输出，无需启动 DISM 或读取 WIM
            let images: Vec<serde_json::Value> = sidecar_entries
                .iter()
                .map(|(idx, meta)| {
                    json!({
                        // 标准 DISM 字段（大驼峰，保证 GUI 及现有解析器无缝识别）
                        "ImageIndex": idx,
                        "ImageName": format!("Windows Backup (index {})", idx),
                        "ImageSize": meta.image_size,
                        "ImageDescription": format!("Computer: {}, Build: {}", meta.computer, meta.windows_build),
                        "ImageVersion": meta.windows_build,
                        "Architecture": meta.architecture,
                        "EditionId": meta.windows_edition,
                        "InstallationType": "Client",
                        // 扩展字段（小驼峰，供现代 CLI/sidecar 特性使用）
                        "index": idx,
                        "name": format!("Windows Backup (index {})", idx),
                        "imageSize": meta.image_size,
                        "sha256": meta.image_sha256,
                        "createdAt": meta.created,
                        "computer": meta.computer,
                        "windowsEdition": meta.windows_edition,
                        "architecture": meta.architecture,
                        "windowsBuild": meta.windows_build,
                        "capturedUsedBytes": meta.captured_used_bytes,
                        "minimumTargetSize": meta.minimum_target_size,
                        "programVersion": meta.program_version,
                        "source": "sidecar",
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string(&json!({
                    "image": image_path,
                    // sidecar 内已记录完整 sha256，输出第一个索引的作为整包标识
                    // （多索引时各自 sha256 不同，这里仅做兼容展示）
                    "sha256": sidecar_entries.first().map(|(_, m)| m.image_sha256.as_str()).unwrap_or(""),
                    "images": images,
                    "source": "sidecar",  // 标记数据来源
                }))?
            );
            return Ok(());
        }
    }

    // ── 无 sidecar：回退到 DISM 解析 ─────────────────────────────────────────
    let output = capture(
        "dism.exe",
        &[
            "/English",
            "/Get-WimInfo",
            &format!("/WimFile:{image_path}"),
        ],
    )?;
    let images = parse_dism_images(&output)?;

    // sha256 计算：若调用方传入 --skip-hash，则跳过耗时的流式哈希
    let sha256 = if skip_hash {
        // 跳过整包哈希计算（适合快速预览；还原时应去掉 --skip-hash 以严格校验）
        "skipped".to_string()
    } else {
        sha256_file(&path)?
    };

    println!(
        "{}",
        serde_json::to_string(&json!({
            "image": image_path,
            "sha256": sha256,
            "images": images,
            "source": "dism",  // 标记数据来源
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
    let mut image_name = None;
    let mut keep_indexes = None;
    let mut boot_menu_name = String::from("Windows Backup");
    let mut efi_drive = None;
    let mut test_fault = None;
    let mut allow_destructive = false;
    let mut no_reboot = false;
    let mut compress = None;
    let mut force_restore_hash = false;
    let mut verify_hash = false;
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
            "--image-name" => image_name = Some(value("--image-name", &mut args)?),
            "--keep-indexes" => {
                keep_indexes = Some(
                    value("--keep-indexes", &mut args)?
                        .parse()
                        .map_err(|_| err("--keep-indexes must be a non-negative integer"))?,
                );
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
            // v1.7.11 起载荷固定放镜像卷、注册位只读，没有「RE 暂存卷」可选。
            // 旧脚本若还带这个开关，直接明确拒绝，避免调用方以为改选生效了。
            "--re-scratch-drive" => {
                return Err(err(
                    "--re-scratch-drive is obsolete: v1.7.11 keeps the payload on the image \
                     volume and never touches the registered WinRE",
                ));
            }
            "--compress" => {
                let level = value("--compress", &mut args)?;
                let level = canonical_compression(&level).map_err(|_| {
                    err("--compress must be fast or none; max compression is not supported")
                })?;
                compress = Some(level.to_string());
            }
            "--force-restore-hash" => force_restore_hash = true,
            "--verify-hash" => verify_hash = true,
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
    if let Some(name) = &image_name
        && (name.trim().is_empty()
            || name.chars().count() > 256
            || name.chars().any(char::is_control))
    {
        return Err(err("--image-name is invalid"));
    }
    if let Some(keep) = keep_indexes {
        // 0 = 全部保留（不清理），与 GUI 留空语义一致。
        if keep == 0 {
            keep_indexes = None;
        }
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
        image_name,
        keep_indexes,
        force_restore_hash,
        verify_hash,
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
    if workspace.same_partition(&efi) {
        return Err(err("program directory cannot be on EFI volume"));
    }
    if image_volume.same_partition(&efi) {
        return Err(err("image volume cannot be on EFI volume"));
    }
    // v1.7.11 新启动通道：注册 WinRE **全程只读**，因此
    //   - F1（还原目标 == 注册 WinRE 宿主）：格式化不再威胁「续跑启动源」（它在镜像卷上）；
    //   - F3（备份源 == 注册 WinRE 宿主）：载荷从不写注册位，镜像天然干净。
    // 两道冲突门与迁出闸门随之删除；仅保留 workspace/镜像卷不能落在注册 WinRE 分区
    // 这条基础规则（那是程序自身资产，与任务启动源无关）。
    let roles = VolumeRoles {
        workspace,
        image: &image_volume,
        source: Some(&source),
        target: matches!(
            options.operation,
            Operation::RestoreExisting | Operation::CreateSecondary
        )
        .then_some(&target),
    };
    if let Some(conflict) = validate_volume_roles(&roles, options.operation, &recovery).err() {
        return Err(err(conflict.message()));
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
            // 备份索引名与保留最近 N 个索引：写入任务供恢复执行时使用。
            task.image_name = options.image_name.clone();
            task.keep_indexes = options.keep_indexes;
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let path = image_path.as_ref().expect("restore image path validated");
            let file_size = fs::metadata(path)?.len();
            // 优先复用 sidecar metadata 里的权威 sha256；无 sidecar 镜像且未勾选 verify_hash 时填充合规占位，彻底避免重复读盘
            let sha256 =
                if let Ok(meta) = crate::read_index_metadata(Path::new(path), options.wim_index) {
                    meta.image_sha256
                } else if options.verify_hash {
                    sha256_file(path)?
                } else {
                    "0".repeat(64)
                };
            task.verify_hash = Some(options.verify_hash);
            task.image = Some(ImageSpec {
                volume: image_volume.clone(),
                absolute_path: Some(path.clone()),
                relative_path: image_relative.clone(),
                sha256,
                size_bytes: file_size,
                index: options.wim_index,
                name: None,
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
    // 新启动通道：载荷放到镜像卷（re_staging），注册位只读。
    let re_staging = ReStaging {
        volume: image_volume.clone(),
        boot_sdi: PathBuf::from(format!(
            r"{}:\Recovery\WindowsRE\boot.sdi",
            ensure_volume_mounted(&recovery, 'R', &prepare_log)?
        )),
    };
    let result = prepare_payload(
        executable_dir,
        &store,
        &mut task,
        &recovery,
        &efi,
        &options,
        &prepare_log,
        &bootstrap_bcd,
        &re_staging,
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

/// 新启动通道（v1.7.11）需要的「载荷 RE 暂存位置」信息。
///
/// * `volume` — 承载载荷 WIM 的卷（默认 = 镜像卷；本来就可写、且不被还原格式化）。
/// * `boot_sdi` — RAM 盘模板文件来源（注册卷上的 `boot.sdi`，可能不存在）。
pub struct ReStaging {
    pub volume: VolumeIdentity,
    pub boot_sdi: PathBuf,
}

// These values have different ownership and rollback roles. Keeping the
// parameters explicit makes preparation ordering and cleanup dependencies
// auditable at the call site.
#[allow(clippy::too_many_arguments)]
fn prepare_payload(
    executable_dir: &Path,
    store: &TaskStore,
    task: &mut Task,
    recovery: &VolumeIdentity,
    efi: &VolumeIdentity,
    options: &PrepareOptions,
    log: &Path,
    bootstrap_bcd: &Path,
    re_staging: &ReStaging,
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
    let raw_bcd_hash = snapshot_raw_bcd(efi, &task_dir, log)?;
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

    // 载荷 Recovery.exe 与主程序按契约同源；若同目录那份是旧件，
    // stage_static_payload 会改用正在运行的可执行文件并回一条告警——
    // 这条必须落进准备日志，否则"离线侧跑的是旧代码"只能事后从
    // 镜像元数据的 programVersion 里反推。
    if let Some(warning) = winre_payload::stage_static_payload(executable_dir, &payload)? {
        append_log(log, &format!("payload staging warning: {warning}"))?;
    }

    let env_path = payload.join("RecoveryTask.env");
    // 迁出任务：env 的 RECOVERY_* 直接指向 RE 暂存卷（先于注入烘焙进 WIM），
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
    // ★ v1.7.11 顺序要点：**先建 BCD 条目，再做 DISM 注入**。
    // 实机结论：同一串 argv 下，bcdedit 在「本进程刚跑过 DISM」时校验镜像卷上的 WIM 会
    // 失败（报「指定的设备无效」，对象状态经 /enum 核验完全正确；换个进程立刻重试就成功）。
    // 而建条目只需要「镜像卷上有一个合法 WIM」——干净的注册 WIM 副本就够。
    // 一次性启动留到载荷注入并拷贝完成之后再武装（此时才允许人重启进来）。
    let mut boot_entry = crate::boot_entry::create_entry(
        &staged,
        &re_staging.boot_sdi,
        &re_staging.volume,
        task_dir.as_path(),
        log,
    )?;
    crate::boot_entry::copy_into_staging(&staged, &re_staging.boot_sdi, &re_staging.volume, log)?;
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
    let result = winre_payload::inject_winre_payload(&mount, &payload);
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

    // v1.7.11 新启动通道：**不写注册位、不跑 reagentc /boottore**。
    // 注入后的载荷 WIM 覆盖到镜像卷的 BackupRestoreRE\，然后武装我们自建的那条
    // BCD 条目（条目本身在 DISM 之前就建好了，见上面的顺序要点）。
    // 注册位全程只是只读资产来源，因此迁出 / WINRE_HOME / 待回家收尾 / Plan D / F1·F3 全部不需要。
    // 依据：docs/20260929-1300-pe-channel-poc-winre-wim-boots-from-image-volume.md
    crate::boot_entry::copy_into_staging(&staged, &re_staging.boot_sdi, &re_staging.volume, log)?;
    // ★ 把簿记里的载荷哈希刷新成**注入后**的实际值。
    // create_entry 在 DISM 注入之前就跑（顺序要点见上），那时的
    // copy_into_staging 复制的是干净原件，记下的是它的哈希；注入后载荷
    // WIM 被覆写成另一份，哈希随之改变。不刷新的话，rearm() 在断电续跑时
    // 拿活载荷比对这份过期记录必然不符，整条续跑路被封死
    // （2026-09-30 实机：stage 卡在 image-applied/75，日志
    //  "payload WIM hash differs from the prepared one; refusing to re-arm"）。
    crate::boot_entry::refresh_payload_hash(&mut boot_entry, task_dir.as_path(), log)?;
    // 武装是「人可以重启了」的唯一开关，放在这一步：载荷已注入、已覆盖到镜像卷、
    // 簿记已落盘。v1.7.14 之前它藏在 create_entry 里无条件执行，导致
    // `--no-reboot` 也会改 bootmgr 的 bootsequence（实机证据见 boot_entry::arm_one_shot）。
    crate::boot_entry::arm_one_shot(&boot_entry, log)?;
    store.write_transition(task, backuprestore_core::Stage::BootRequested)?;
    write_status_env(&task_dir, task, "boot-requested")?;
    if options.test_fault.as_deref() == Some("power-loss-window") {
        append_log(
            log,
            "Development test fault: stopped after durable boot-requested state; no shutdown requested",
        )?;
        return Ok(());
    }
    if let Err(error) = run_logged("shutdown.exe", &["/r", "/t", "0"], log) {
        // 重启请求失败：撤销这条一次性启动与自建条目，别把机器留在
        // 「下次开机进任务 RE，但没有人会来跑」的状态。
        match crate::boot_entry::ReBootEntry::read(task_dir.as_path()) {
            Ok(Some(entry)) => {
                if let Err(cleanup_error) = crate::boot_entry::disarm(&entry, log) {
                    append_log(
                        log,
                        &format!(
                            "[ERROR] shutdown failed: {error}; RE rollback failed: {cleanup_error}; staged files or BCD objects may remain"
                        ),
                    )?;
                }
            }
            Ok(None) => append_log(
                log,
                "[WARN] shutdown rollback: RE entry record missing; cleanup cannot be verified",
            )?,
            Err(read_error) => append_log(
                log,
                &format!("[ERROR] shutdown rollback: cannot read RE entry: {read_error}"),
            )?,
        }
        if let Err(rollback_error) = rollback_boot_request(&task_dir, efi, log) {
            append_log(
                log,
                &format!(
                    "[ERROR] shutdown failed: {error}; boot request rollback failed: {rollback_error}"
                ),
            )?;
        }
        return Err(error);
    }
    Ok(())
}

/// ESP 上 BCD 存储的路径，**优先零盘符**。
///
/// 方案 A（v1.8.2）的核心入口。返回 `(路径, 本次是否为此挂了盘符)`：
/// 第二个元素是 `Some(letter)` 时，调用方用完必须把盘符卸掉，
/// 否则「临时盘符」会变成新的污染源——那正是 S 盘反复出现的原因。
///
/// 为什么能零盘符：verbatim 卷路径下 `CreateFileW` / `CopyFileExW` /
/// `bcdedit` / `dism` 全都接受。2026-09-29 实机验证（`docs/20260929-2014-S盘自动打开问题定位与修复方案.md` 第六·补节）：
///   - 读：两条路径 copy 出的 BCD 副本 SHA-256 相同
///   - 写：`set {bootmgr} default` 后 `/enum` 输出 `fc /b` 逐字节无差异
///   - 写：`/set` `/create` `/delete` 三个动词均成功
///   - 枚举：卷路径可直接列出 ESP 全部文件
///
/// 卷 GUID 缺失（导入的旧任务、DiskPart 探测失败）时才退回挂盘符。
fn efi_bcd_store_path(
    efi: &VolumeIdentity,
    preferred: char,
    log: &Path,
) -> Result<(PathBuf, Option<char>), TaskError> {
    if let Some(volume) = efi.volume_path() {
        let mut path = PathBuf::from(volume);
        path.push("EFI");
        path.push("Microsoft");
        path.push("Boot");
        path.push("BCD");
        append_log(
            log,
            &format!("plan A: verbatim BCD store path = {}", path.display()),
        )?;
        return Ok((path, None));
    }
    let letter = ensure_volume_mounted(efi, preferred, log)?;
    append_log(
        log,
        &format!("plan A: volume GUID absent; fell back to drive letter {letter}:"),
    )?;
    Ok((
        PathBuf::from(format!(r"{letter}:\EFI\Microsoft\Boot\BCD")),
        Some(letter),
    ))
}

/// 按**卷路径**读完整卷身份，**不分配盘符**（方案 A 的核心）。
///
/// 与 [`volume_identity`] 的差别只在"用哪个路径打开卷"：这里是 verbatim 卷路径，
/// 那边是盘符路径。三个 Win32 调用（卷信息、存储设备号、GPT 分区信息）都经
/// `CreateFileW`/`GetVolumeInformationW`，两者等价。
///
/// 2026-09-29 实机验证过等价性（`docs/20260929-2014-S盘自动打开问题定位与修复方案.md` 第六·补节）：
/// 通过盘符与通过卷路径拿到的 BCD 副本 SHA-256 完全相同，写操作也不做归一化。
pub(crate) fn volume_identity_at_path(
    path: &str,
    volume_guid: String,
) -> Result<VolumeIdentity, TaskError> {
    // 两种路径形态都要用，别混：
    //   - 卷路径（Win32 卷 API、GetVolumeInformationW、bcdedit /store）
    //   - `\\.\Volume{GUID}`     设备路径（DeviceIoControl 查分区/磁盘）
    // 卷路径 → 设备路径（转换本体在 text_parsing，有单测）。
    // 2026-09-29 实机踩过：拿卷路径去 CreateFileW + DeviceIoControl 会报 161。
    let device_path = crate::text_parsing::volume_path_to_device_path(path)
        .ok_or_else(|| err("volume path has no GUID; cannot derive its device path"))?;
    let (filesystem, volume_serial) = volume_information_at(path)?;
    let disk_number = storage_device_number_at(&device_path)?;
    let physical = physical_volume_identity_at(&device_path, disk_number)?;
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
        // 身份里**绝不**记盘符：它只是本次运行的临时挂载，重启后可能变成别的字母。
        drive_letter: None,
    })
}

/// 按**路径**取文件系统名与卷序列号。路径可以是盘符路径，也可以是 verbatim
/// 卷路径——底下的 `GetVolumeInformationW` 两者都吃。
/// 这是「零盘符读卷身份」的第一块砖。
fn volume_information_at(path: &str) -> Result<(String, String), TaskError> {
    let path = wide_null(path);
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
        let code = unsafe { GetLastError() };
        let shown = String::from_utf16_lossy(&path);
        return Err(err(&format!(
            "GetVolumeInformationW failed for {shown} with Win32 error {code}"
        )));
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

/// 按**卷路径**取物理磁盘号。盘符只是路径的一种写法；verbatim 卷路径同样能拿到。
fn storage_device_number_at(path: &str) -> Result<u32, TaskError> {
    let handle = open_device(&wide_null(path))?;
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

/// 按**卷路径**读 GPT 分区身份（方案 A 零盘符的关键一块）。
///
/// 盘符形态与 verbatim 卷路径都交给 `CreateFileW`，拿到的句柄对
/// `IOCTL_STORAGE_GET_DEVICE_NUMBER` / `IOCTL_DISK_GET_PARTITION_INFO_EX`
/// 一视同仁。这样连「这个卷是不是 ESP」（分区类型 GUID）都能在不分配盘符的
/// 前提下判定。
fn physical_volume_identity_at(
    path: &str,
    disk_number: u32,
) -> Result<PhysicalVolumeIdentity, TaskError> {
    let partition_handle = open_device(&wide_null(path))?;
    let mut partition = vec![0_u8; 160];
    let partition_query = device_io_control(
        partition_handle,
        IOCTL_DISK_GET_PARTITION_INFO_EX,
        &mut partition,
    )
    .and_then(|_| {
        let partition_offset = read_u64(&partition, 8)?;
        let partition_size = read_u64(&partition, 16)?;
        let partition_number = read_u32(&partition, 24)?;
        let partition_type_guid = format_guid(&partition[32..48])?;
        let partition_guid = format_guid(&partition[48..64])?;
        Ok((
            partition_offset,
            partition_size,
            partition_number,
            partition_type_guid,
            partition_guid,
        ))
    });
    let _ = unsafe { CloseHandle(partition_handle) };
    let (partition_offset, partition_size, partition_number, partition_type_guid, partition_guid) =
        partition_query?;
    let disk_path = format!(r"\\.\PhysicalDrive{}", disk_number);
    let disk_handle = open_device(&wide_null(&disk_path))?;
    let mut layout = vec![0_u8; 65_536];
    let layout_query = device_io_control(disk_handle, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, &mut layout)
        .and_then(|_| {
            if read_u32(&layout, 0)? != 1 {
                return Err(err("selected disk is not GPT"));
            }
            format_guid(&layout[8..24])
        });
    let _ = unsafe { CloseHandle(disk_handle) };
    let disk_guid = layout_query?;
    Ok(PhysicalVolumeIdentity {
        disk_guid,
        partition_guid,
        partition_type_guid,
        partition_number,
        partition_offset,
        partition_size,
    })
}

/// 原生枚举包括已挂载卷；验证 EFI 类型及 BCD，多个候选或探测错误均拒绝猜测。
pub(crate) fn esp_identity_without_drive_letter() -> Result<VolumeIdentity, TaskError> {
    let mut matches: Vec<VolumeIdentity> = Vec::new();
    let mut last_error: Option<TaskError> = None;
    for volume_path in native_volume_paths()? {
        // verbatim 路径形如 \\?\Volume{GUID}\，卷 GUID 取中段。
        let Some(guid) = volume_path
            .trim()
            .strip_prefix(&crate::text_parsing::esp_volume_prefix())
            .and_then(|rest| rest.strip_suffix('\\'))
        else {
            continue;
        };
        match volume_identity_at_path(&volume_path, guid.to_string()) {
            Ok(identity) => {
                if !identity.partition_type_guid.eq_ignore_ascii_case(EFI_TYPE) {
                    continue;
                }
                // 未知错误不能当成不存在后随意挑另一个 ESP。
                if crate::pe_safety::probe_file_metadata(
                    std::path::Path::new(&volume_path),
                    &["EFI", "Microsoft", "Boot", "BCD"],
                )? {
                    matches.push(identity);
                }
            }
            Err(error) => last_error = Some(error),
        }
    }
    match matches.len() {
        0 => Err(last_error.unwrap_or_else(|| err("未发现具有可读取 BCD 的 GPT EFI 分区"))),
        1 => Ok(matches.remove(0)),
        _ => {
            let list = matches
                .iter()
                .map(|identity| {
                    format!(
                        "disk {} partition {}",
                        identity.disk_number.unwrap_or_default(),
                        identity.partition_number.unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            Err(err(&format!(
                "multiple EFI system partitions found ({list}); pass --efi-drive to choose one"
            )))
        }
    }
}

/// 直接枚举卷 GUID，不解析 mountvol 的本地化提示，不分配盘符。
pub(crate) fn native_volume_paths() -> Result<Vec<String>, TaskError> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FindFirstVolumeW(name: *mut u16, size: u32) -> *mut std::ffi::c_void;
        fn FindNextVolumeW(handle: *mut std::ffi::c_void, name: *mut u16, size: u32) -> i32;
        fn FindVolumeClose(handle: *mut std::ffi::c_void) -> i32;
    }
    let mut buffer = vec![0u16; 1024];
    let handle = unsafe { FindFirstVolumeW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if handle as isize == -1 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut paths = Vec::new();
    loop {
        let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        paths.push(String::from_utf16_lossy(&buffer[..len]));
        if unsafe { FindNextVolumeW(handle, buffer.as_mut_ptr(), buffer.len() as u32) } == 0 {
            let error = std::io::Error::last_os_error();
            unsafe {
                FindVolumeClose(handle);
            }
            if error.raw_os_error() != Some(18) {
                return Err(error.into());
            }
            break;
        }
    }
    Ok(paths)
}

/// 从稳定卷 GUID 回读现场身份；调用方另行检查编号、几何及卷序列号。
pub(crate) fn refresh_identity(expected: &VolumeIdentity) -> Result<VolumeIdentity, TaskError> {
    let root = expected
        .volume_path()
        .ok_or_else(|| err("缺少稳定卷 GUID"))?;
    let mut actual = volume_identity_at_path(&root, expected.volume_guid.clone())?;
    actual.drive_letter = expected.drive_letter;
    Ok(actual)
}

fn snapshot_raw_bcd(
    efi: &VolumeIdentity,
    task_dir: &Path,
    log: &Path,
) -> Result<String, TaskError> {
    // 方案 A（v1.8.2）：优先 verbatim 卷路径，不分配盘符。
    let (source, letter) = efi_bcd_store_path(efi, 'S', log)?;
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
    if let Some(letter) = letter {
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
        // 同上：零盘符优先，读不到卷 GUID 才挂。
        let (store, letter) = efi_bcd_store_path(efi, 'S', log)?;
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
        if let Some(letter) = letter {
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
            // 配套备份 metadata（.index-N.metadata.json）存在时校验 WIM 完整性；
            // 优先比对物理文件大小，避免在提权准备阶段对 58GB 镜像再次耗时 2 分钟重复读盘计算哈希。
            // 缺失时视为第三方/PE WIM（如安装 WinRE/PE 为第二系统），跳过
            // 哈希校验与目标大小预检——DISM /Get-WimInfo 已确认 WIM 可读。
            let metadata = crate::read_index_metadata(Path::new(path), options.wim_index).ok();
            if let Some(metadata) = &metadata {
                let file_size = fs::metadata(path)?.len();
                if metadata.image_size > 0
                    && file_size != metadata.image_size
                    && !options.force_restore_hash
                {
                    return Err(err("restore image size does not match metadata"));
                }
                // 用户要求：增加一个校验选项框，不勾选时仅对比大小，勾选时对比哈希
                if options.verify_hash {
                    let expected = &metadata.image_sha256;
                    let actual = sha256_file(path)?;
                    if !actual.eq_ignore_ascii_case(expected) && !options.force_restore_hash {
                        return Err(err("restore image hash does not match metadata"));
                    }
                }
            }
            let minimum = metadata
                .as_ref()
                .map(|metadata| {
                    metadata
                        .required_target_size()
                        .max(metadata.source.partition_size)
                })
                .unwrap_or(0);
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

pub(crate) fn insert_identity(
    values: &mut BTreeMap<String, String>,
    prefix: &str,
    identity: &VolumeIdentity,
) {
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
    // 数据卷（无 SYSTEM hive）可在当前 Windows 在线备份/还原，无需恢复环境；
    // 仅系统卷（存在 SYSTEM hive）要求 WinRE 可用（备份/还原在 PE/RE 中执行）。
    if Path::new(&format!(
        r"{}:\Windows\System32\config\SYSTEM",
        source_drive
    ))
    .is_file()
    {
        let reagent = capture("reagentc.exe", &["/info"])?;
        if !(reagent.contains("GLOBALROOT") || reagent.contains("Recovery\\WindowsRE")) {
            return Err(err("Windows RE is disabled or unavailable"));
        }
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
    // Keep the Win32 error code: "no volume mounted here" and "the volume API
    // failed" need different answers from whoever reads the log, and WinRE is
    // where that distinction has cost the most time.
    let volume_guid = match mounted_volume_guid(letter) {
        Ok(Some(volume_guid)) => volume_guid,
        Ok(None) => {
            return Err(err(&format!(
                "volume {letter}: has no mounted volume identity"
            )));
        }
        Err(code) => {
            return Err(err(&format!(
                "volume {letter}: volume identity query failed with Win32 error {code}"
            )));
        }
    };
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

pub(crate) fn volume_identity_with_known_guid(
    letter: char,
    volume_guid: String,
) -> Result<VolumeIdentity, TaskError> {
    let (filesystem, volume_serial) = volume_information(letter)?;
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

/// Look up the volume GUID path mounted at `letter:`.
///
/// `Ok(None)` means the letter is genuinely unassigned; `Err(code)` carries
/// the Win32 error so callers can log it rather than reporting every failure
/// as "no volume".
pub(crate) fn mounted_volume_guid(letter: char) -> Result<Option<String>, u32> {
    let root = wide_null(&format!(r"{}:\", letter));
    let mut volume = [0_u16; 128];
    let success = unsafe {
        GetVolumeNameForVolumeMountPointW(root.as_ptr(), volume.as_mut_ptr(), volume.len() as u32)
    };
    if success == 0 {
        let error = unsafe { GetLastError() };
        // ERROR_PATH_NOT_FOUND/ERROR_FILE_NOT_FOUND are normal for an
        // unassigned letter; preserve other errors for WinRE diagnostics.
        if error == 3 || error == 2 {
            return Ok(None);
        }
        return Err(error);
    }
    let Some(length) = volume.iter().position(|value| *value == 0) else {
        return Err(122);
    };
    Ok(Some(String::from_utf16_lossy(&volume[..length])))
}

fn volume_free_bytes(letter: char) -> Result<u64, TaskError> {
    disk_free_space(letter).map(|(_, free)| free)
}

pub(crate) fn disk_free_space(letter: char) -> Result<(u64, u64), TaskError> {
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
    volume_information_at(&format!(r"{letter}:\\"))
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
    physical_volume_identity_at(&format!(r"\\.\{letter}:"), disk_number)
}

fn storage_device_number(letter: char) -> Result<u32, TaskError> {
    storage_device_number_at(&format!(r"\\.\{letter}:"))
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
        // path 一起报出来：2026-09-29 方案 A 实机栽在「卷路径 != 设备路径」上，
        // 只有 CreateFileW 错误码而没有路径，等于没给线索。
        let shown = String::from_utf16_lossy(path)
            .trim_matches(char::from(0))
            .to_string();
        Err(err(&format!(
            "CreateFileW failed on {shown} while reading volume identity (Windows error {error_code})"
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
    // Windows 官方支持两种 WinRE 注册形态：GPT Recovery 分区（de94bba4-…）或
    // OS 分区的 \Recovery\WindowsRE（reagentc /enable 在多数现代系统上会把
    // WinRE 复制回 OS 分区，无法稳定留在 Recovery 分区）。两种形态的 BCD
    // ramdisk 引导行为一致（已在 Parallels ARM64 实机验证可正常引导进 WinRE）。
    // 这里不再强制要求 Recovery 分区类型：identity 仅用于“工作区/镜像卷不得
    // 与恢复分区同卷”的安全检查，按 reagentc 报告的真实分区继续即可；
    // 若 WinRE 文件缺失，后续 WIM 挂载/注入会自行报错。
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
    // 方案 A（v1.8.2）：**先试零盘符**。
    // 在「未挂载的隐藏卷」里找分区类型 GUID 为 EFI 且真有 Boot/BCD 的那一个，
    // 全程不分配盘符。拿不到再退回下面这套 mountvol 循环——那是保底，不是首选。
    if let Ok(identity) = esp_identity_without_drive_letter() {
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
        // ★ 2026-09-29 实机测量：**`mountvol X: /S` 的退出码不可信**。
        // 提权会话里逐盘符实测（`.test-artifacts/elev-channel/mvwhy.txt`）：
        //   Z: EXIT=1 → 但随后 `/L` 显示 ESP 已经挂在 Z: 上（成功却报失败）
        //   Y: EXIT=0 → `/L` 显示的仍是同一个卷（ESP 已挂，重复 `/S` 无事可做）
        // 原来的代码信退出码，于是把成功的那一次当失败跳过去、换下一个盘符
        // 再挂一次同一个卷——这正是 S:/Z: 盘符反复出现、窗口"不可访问"的机制来源。
        // 现在改成：发起 `/S` 之后**用 `/L` 确认真的挂上了**，只看结果不看退出码。
        let _ = Command::new("mountvol.exe")
            .args([format!("{letter}:"), "/S".to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        if !mountvol_letter_shows_volume(letter) {
            last_error = Some(err(&format!(
                "mountvol /S did not expose a volume at {letter}:; not the boot ESP?"
            )));
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

/// `mountvol X: /L` 是否报告该盘符上**有**卷。
///
/// 这是挂载成功的唯一可靠判据：`/S` 的退出码在实机上会骗人（见
/// `efi_identity` 里 2026-09-29 的测量记录），`/L` 的结果不会。
#[cfg(windows)]
fn mountvol_letter_shows_volume(letter: char) -> bool {
    let Ok(output) = Command::new("mountvol.exe")
        .args([format!("{letter}:"), "/L".to_string()])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return false;
    };
    // 判据本体在 text_parsing（macOS 也编译、有单测），这里只是跑一下 mountvol。
    crate::text_parsing::mountvol_listing_has_volume(&String::from_utf8_lossy(&output.stdout))
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

/// Find a drive letter that Windows already exposes for `identity`.
///
/// Matching on the disk/partition GUIDs (and the volume GUID when known) is
/// what makes the resume path independent from numeric coordinates: drive
/// letters are reassigned between Windows and WinRE, but the GUIDs are stable.
fn mounted_letter_for_identity(identity: &VolumeIdentity) -> Option<char> {
    let volume_guid = identity.volume_guid.trim();
    for letter in 'C'..='Z' {
        if !Path::new(&format!(r"{}:\", letter)).is_dir() {
            continue;
        }
        let Ok(actual) = volume_identity(letter) else {
            continue;
        };
        if actual.same_partition(identity) {
            return Some(letter);
        }
        if !volume_guid.is_empty() && actual.volume_guid.eq_ignore_ascii_case(volume_guid) {
            return Some(letter);
        }
    }
    None
}

/// Attach `\\?\Volume{...}` to a free drive letter with `mountvol`.
fn mount_volume_guid(volume_guid: &str, preferred: char) -> Result<char, TaskError> {
    let trimmed = volume_guid.trim().trim_end_matches('\\').to_string();
    if trimmed.is_empty() {
        return Err(err("volume GUID is empty"));
    }
    let mut last_error = None;
    for letter in identity_drive_candidates(preferred) {
        if !is_drive_letter_available(letter) {
            continue;
        }
        let output = match Command::new("mountvol.exe")
            .args([format!("{letter}:"), trimmed.clone()])
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            Ok(output) => output,
            Err(error) => {
                last_error = Some(err(&format!("mountvol {letter}: failed to start: {error}")));
                continue;
            }
        };
        if !output.status.success() {
            last_error = Some(err(&format!(
                "mountvol {letter}: exited with {}",
                output.status
            )));
            continue;
        }
        // Assignment succeeding is not proof: verify the letter really points
        // at the volume we asked for before handing it back.
        match volume_identity(letter) {
            Ok(actual)
                if actual
                    .volume_guid
                    .trim()
                    .trim_end_matches('\\')
                    .eq_ignore_ascii_case(&trimmed) =>
            {
                return Ok(letter);
            }
            Ok(actual) => {
                last_error = Some(err(&format!(
                    "mountvol assigned {letter}: to {}, expected {trimmed}",
                    actual.volume_guid
                )));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| err("unable to mount the volume GUID on an available drive letter")))
}

/// Reused by the normal-Windows boot-resume path so that resumption can reach
/// the registered WinRE file even when it lives on a hidden recovery partition.
pub(crate) fn ensure_volume_mounted(
    identity: &VolumeIdentity,
    preferred: char,
    _log: &Path,
) -> Result<char, TaskError> {
    if let Some(letter) = identity.drive_letter {
        // 保存的盘符只是提示，跨启动后可能已经属于另一分区。
        if let Ok(actual) = volume_identity(letter)
            && identity.same_partition(&actual)
            && identity.partition_offset == actual.partition_offset
            && identity.partition_size == actual.partition_size
        {
            return Ok(letter);
        }
    }
    // Prefer reusing a mount point that already exists. The registered WinRE
    // very often lives on the OS partition, which Windows already exposes, so
    // this is both the common case and the cheapest one.
    if let Some(letter) = mounted_letter_for_identity(identity) {
        return Ok(letter);
    }
    // Attach the volume by its GUID path. This is the only strategy that works
    // for identities without numeric disk/partition coordinates (imported or
    // legacy tasks) and for hidden recovery partitions DiskPart refuses to
    // expose by number.
    if !identity.volume_guid.trim().is_empty()
        && let Ok(letter) = mount_volume_guid(&identity.volume_guid, preferred)
    {
        return Ok(letter);
    }
    let disk = identity.disk_number.ok_or_else(|| {
        err("volume has no disk number and could not be matched by GUID or an existing mount point")
    })?;
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
    use super::{
        parse_prepare_options, restore_reserved_target_error, restore_workspace_target_error,
    };

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

    #[test]
    fn rejects_removed_auto_relocation_switch() {
        let error = parse_prepare_options(vec!["--relocated".into()]).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown prepare option: --relocated")
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
