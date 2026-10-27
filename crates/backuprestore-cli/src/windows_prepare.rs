//! Normal-Windows preparation implemented in Rust.
//!
//! The desktop program does not delegate task creation or system preparation
//! to PowerShell.  This module owns the public `prepare`, `list-volumes`,
//! `inspect-environment` and `wim-info` commands; it uses only Rust file I/O
//! plus the Windows inbox command-line tools that perform the OS operations.

use backuprestore_core::{
    BootMode, DestinationSpec, ImageAcceptance, ImageSpec, Operation, PayloadManifest, TargetRole,
    TargetSpec, Task, TaskError, TaskStore, VolumeIdentity, VolumeRoles, canonical_compression,
    sha256_file, validate_absolute_path, validate_volume_roles, write_json_atomic,
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
use std::process::{Command, Output, Stdio};
use std::ptr::null_mut;
use std::thread;
use std::time::Duration;

use crate::error_code::ErrCode;
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
    fn FlushFileBuffers(handle: *mut c_void) -> i32;
    fn GetLastError() -> u32;
}

const GENERIC_WRITE: u32 = 0x4000_0000;

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
/// EFI 系统分区的分区类型 GUID。**判定 ESP 必须用它，不能用盘符**——盘符在不同
/// 环境里会重排（恢复环境内尤其如此），本项目此前把 `S:` 当作 ESP 只是约定，
/// `S:` 被别的卷占用或 ESP 挂到别的盘符时都会判错（`text_parsing.rs` 与
/// `native_gui.rs` 的注释都记录过这个靠不住的假设）。
pub(crate) const EFI_TYPE: &str = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}";
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
    /// 还原前是否格式化目标卷（GUI「格式化目标分区」复选框，默认 true）。
    /// false = 覆盖式应用（不格式化，卷上镜像没有的文件保留），落盘进
    /// TargetSpec 供 RE 恢复链读取。
    format_target: bool,
    /// 恢复环境「连续失败 N 次后如何处理」策略（次数 + 行为）。
    /// 来源优先级：CLI `--fail-threshold` / `--fail-action` > 程序目录的
    /// `BackupRestore.settings.ini`（GUI 设置窗口写的）> 默认（3 次 + 自动逃出）。
    /// 落盘进 `RecoveryTask.env`，由恢复环境 `main.rs::recover_env_interactive` 读取。
    failure_policy: crate::failure_policy::FailurePolicy,
    /// DISM 等待上限（秒；0 = 不限制 = 历史行为）。来源优先级：
    /// CLI `--dism-timeout-secs` > 程序目录 `BackupRestore.settings.ini` 的
    /// `dism_timeout_secs` > 默认 0。落盘进 `RecoveryTask.env` 的
    /// `DISM_TIMEOUT_SECS`，恢复环境在入口把它灌进进程环境变量
    /// （`crate::dism_timeout::ENV_VAR`），本进程内所有 DISM 调用都按它等待。
    dism_timeout_secs: u32,
    /// 创建任务时的界面语言（`zh` / `en`），落盘进 `RecoveryTask.env` 的
    /// `UI_LANGUAGE`。恢复环境的提示文案跟着它走，避免英文界面用户在恢复
    /// 环境里突然看到中文。
    ui_language: String,
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
    // 2026-10-18：launcher-errors.log 与 GUI 读取方同走 `log_root`——这条日志
    // 记录的恰是「连任务都没建成就失败了」的最早期故障，不能放在可能正是
    // 还原目标的程序目录里。
    require_administrator(&crate::log_root::logs_dir().join("launcher-errors.log"))?;
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

/// 列出当前 BCD 里所有分区 loader 的描述（S5 撞名预检，GUI 创建任务前调用）。
///
/// 刻意**不带 /store、不挂 ESP**：`bcdedit /enum` 是读操作，普通用户权限即可
/// 完成，而 GUI 常态未提权（package 自启副本）；提权 prepare 侧的消解发生在
/// 任务创建之后，用户看不到结果——这里让 GUI 在创建任务前就能预检并回显。
///
/// 失败不报错、输出空列表：调用方（GUI）按「无法预检」处理并记日志，
/// 真正的承重消解在 prepare 侧（挂 ESP 读 /store，必然可用）。
/// 输出 GBK 的本地化字段名用 `decode_windows_bytes` 解，中文菜单名才不会
/// 被 from_utf8_lossy 弄花（弄花等于撞名检查对中文系统全废）。
///
/// 【2026-10-16 审查修订，日志缺口 10】失败原因写进输出的 `error` 字段：
/// 此前失败与"真的没有启动项"都表现为**空列表**，调用方无法区分——
/// 撞名预检会静默失效，而用户看到的菜单名重名问题要到任务创建时才炸。
#[cfg(windows)]
pub(crate) fn list_boot_entries() -> Result<(), TaskError> {
    let output = std::process::Command::new("bcdedit.exe")
        .args(["/enum", "all", "/v"])
        .stdin(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let (descriptions, probe_error) = match &output {
        Ok(output) if output.status.success() => (
            crate::boot_menu::all_loader_descriptions(&crate::text_parsing::decode_windows_bytes(
                &output.stdout,
            )),
            None,
        ),
        Ok(output) => {
            // 退出码非零：把 stderr 尾部一起带出来，让 GUI 能记下真实原因
            //（BCD 存储不可访问 / 权限不足 / 系统无 UEFI 等）。
            let mut text = crate::text_parsing::decode_windows_bytes(&output.stdout);
            text.push_str(&crate::text_parsing::decode_windows_bytes(&output.stderr));
            let tail: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect();
            let start = tail.len().saturating_sub(4);
            (
                Vec::new(),
                Some(format!(
                    "bcdedit /enum exited with {}: {}",
                    output.status,
                    tail[start..].join(" | ")
                )),
            )
        }
        Err(error) => (Vec::new(), Some(format!("bcdedit could not be started: {error}"))),
    };
    let mut report = serde_json::json!({ "descriptions": descriptions });
    if let Some(error) = probe_error {
        report["error"] = serde_json::json!(error);
    }
    println!("{}", report);
    Ok(())
}

pub(crate) fn inspect_environment() -> Result<(), TaskError> {
    let output = capture("cmd.exe", &["/d", "/c", "ver"])?;
    // `ver` is localized and may use the active Windows code page.  The
    // status panel is UTF-8 Rust text, so retain only its invariant ASCII
    // version token instead of leaking mojibake such as `�本` into Chinese UI.
    let windows = windows_version_summary(&output);
    let architecture = native_architecture();
    // 同 `assert_environment`：`reagentc /info` 有上下文级失败模式（退出码 9），
    // 失败不代表 WinRE 不可用；这里只用于**显示**，失败时退回 BCD 判是否有 WinRE，
    // 绝不因为日志 ACL 问题就让整个「刷新环境」动作失败。
    let winre_available = match capture("reagentc.exe", &["/info"]) {
        Ok(reagent) => reagent.contains("GLOBALROOT") || reagent.contains("Recovery\\WindowsRE"),
        Err(_) => recovery_identity_from_bcd().is_ok(),
    };
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

#[cfg(test)]
mod sidecar_name_tests {
    use super::sidecar_image_name;

    #[test]
    fn sidecar_image_name_fallback_chain() {
        // 副档记了真名：直接采用（2.6.16 起备份必记）
        assert_eq!(
            sidecar_image_name(Some("Windows 11 26200.9550 20261018"), 1, None),
            "Windows 11 26200.9550 20261018"
        );
        // 副档空白 + DISM 真名：用 DISM
        assert_eq!(sidecar_image_name(Some("  "), 1, Some("Real")), "Real");
        // 旧副档无此字段 + DISM 真名：用 DISM
        assert_eq!(sidecar_image_name(None, 2, Some("Real")), "Real");
        // 两者皆无：才落占位名，且带对索引号
        assert_eq!(
            sidecar_image_name(None, 3, None),
            "Windows Backup (index 3)"
        );
        assert_eq!(
            sidecar_image_name(None, 3, Some(" ")),
            "Windows Backup (index 3)"
        );
    }
}

/// sidecar 索引名的回退链：副档记录 → DISM 真名 → 占位名。
/// 占位名只允许出现在「副档没记、DISM 也没问到」的极端情况；正常情况下
/// 调用方（GUI 下拉、BCD 描述预填）拿到的是前两者之一。
fn clean_sidecar_name(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|name| !name.is_empty())
}

fn sidecar_image_name(recorded: Option<&str>, index: u32, dism_name: Option<&str>) -> String {
    if let Some(name) = clean_sidecar_name(recorded) {
        return name.to_string();
    }
    if let Some(name) = clean_sidecar_name(dism_name) {
        return name.to_string();
    }
    format!("Windows Backup (index {index})")
}

/// 任一索引缺少副档记录的索引名时，用一次 `/Get-WimInfo` 补齐（返回 索引→真名）。
/// DISM 失败不致命：退回占位名（与修复前行为一致），wim_info 的整体契约是「尽力呈现」。
fn sidecar_dism_names(
    image_path: &str,
    entries: &[(u32, backuprestore_core::BackupMetadata)],
) -> Result<std::collections::HashMap<u32, String>, TaskError> {
    let needs = entries.iter().any(|(_, meta)| {
        meta.image_name
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
    });
    if !needs {
        return Ok(std::collections::HashMap::new());
    }
    let mut names = std::collections::HashMap::new();
    if let Ok(output) = capture(
        "dism.exe",
        &[
            "/English",
            "/Get-WimInfo",
            &format!("/WimFile:{image_path}"),
        ],
    ) && let Ok(parsed) = parse_dism_images(&output)
    {
        for image in &parsed {
            let index = image.get("ImageIndex").and_then(|v| v.as_u64());
            let name = image
                .get("ImageName")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|n| !n.is_empty());
            if let (Some(index), Some(name)) = (index, name) {
                names.insert(index as u32, name.to_string());
            }
        }
    }
    Ok(names)
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
            // 老副档（2.6.15 及以前备份）没有 image_name 字段：问一次 DISM 拿真名。
            // 只读 WIM 目录（/Get-WimInfo 不解压镜像内容），新副档不受影响。
            let dism_names = sidecar_dism_names(&image_path, &sidecar_entries)?;
            // 用 sidecar 数据直接输出；索引名优先取副档记录，其次 DISM 真名
            let images: Vec<serde_json::Value> = sidecar_entries
                .iter()
                .map(|(idx, meta)| {
                    let display_name = sidecar_image_name(
                        meta.image_name.as_deref(),
                        *idx,
                        dism_names.get(idx).map(String::as_str),
                    );
                    json!({
                        // 标准 DISM 字段（大驼峰，保证 GUI 及现有解析器无缝识别）
                        "ImageIndex": idx,
                        "ImageName": display_name.clone(),
                        "ImageSize": meta.image_size,
                        "ImageDescription": format!("Computer: {}, Build: {}", meta.computer, meta.windows_build),
                        "ImageVersion": meta.windows_build,
                        "Architecture": meta.architecture,
                        "EditionId": meta.windows_edition,
                        "InstallationType": "Client",
                        // 扩展字段（小驼峰，供现代 CLI/sidecar 特性使用）
                        "index": idx,
                        "name": display_name,
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
    let mut format_target = true;
    // 失败处理策略：CLI 显式传参优先，缺省时读程序目录的设置文件（GUI 写的）。
    let mut fail_threshold: Option<u32> = None;
    let mut fail_action: Option<crate::failure_policy::FailureAction> = None;
    let mut fail_pause_max_wait: Option<u32> = None;
    let mut dism_timeout_secs: Option<u32> = None;
    let mut ui_language: Option<String> = None;
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
                ) && !crate::recovery_fault::valid(&fault)
                {
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
            // `--json` 由 main 统一处理（收尾输出那行结构化结论），这里只须
            // **接受**它——下面兜底是 `unknown prepare option` 硬报错，
            // 漏了这一行就会让 prepare --json 直接失败（2026-10-24 实测踩到：
            // online 用 `_ => {}` 忽略未知选项所以没事，prepare 是严格解析）。
            "--json" => {}
            // 取消「格式化目标分区」= 覆盖式应用（卷上镜像没有的文件保留）。
            // GUI 取消勾选时就是推这个参数（native_gui.rs 的 prepare args）。
            // 2026-10-14 漏接过一次：PrepareOptions/TargetSpec 两头都接好了，唯独少了
            // 这一行解析分支，而下面的兜底是 `unknown prepare option` 硬报错——
            // 于是「离线 + 取消格式化」整条通道在 prepare 阶段就直接失败。
            "--skip-format" => format_target = false,
            // 恢复环境失败处理策略（GUI 设置窗口 → 这两个参数 → RecoveryTask.env）。
            // 复用 `failure_policy` 的解析与校验，错误文案与 GUI 侧同一来源。
            "--fail-threshold" => {
                let raw = value("--fail-threshold", &mut args)?;
                fail_threshold = Some(
                    crate::failure_policy::FailurePolicy::parse_threshold(
                        &raw,
                        crate::pe_safety::MessageLanguage::Chinese,
                    )
                    .map_err(|reason| err(&format!("--fail-threshold: {reason}")))?,
                );
            }
            "--fail-action" => {
                let raw = value("--fail-action", &mut args)?;
                fail_action = Some(crate::failure_policy::FailureAction::parse(&raw).ok_or_else(
                    || err("--fail-action must be auto-escape or pause"),
                )?);
            }
            "--fail-pause-max-wait-secs" => {
                let raw = value("--fail-pause-max-wait-secs", &mut args)?;
                fail_pause_max_wait = Some(
                    crate::failure_policy::parse_pause_max_wait_secs(&raw).ok_or_else(|| {
                        err("--fail-pause-max-wait-secs must be 0..=86400 seconds (0 = unlimited)")
                    })?,
                );
            }
            "--dism-timeout-secs" => {
                let raw = value("--dism-timeout-secs", &mut args)?;
                dism_timeout_secs = Some(
                    crate::dism_timeout::parse_secs(&raw)
                        .map_err(|reason| err(&format!("--dism-timeout-secs: {reason}")))?,
                );
            }
            "--ui-language" => {
                let raw = value("--ui-language", &mut args)?.trim().to_ascii_lowercase();
                // 只认 zh*/en* 前缀（`zh-CN`、`en-US` 都接受），其余明确拒绝，
                // 避免写进 env 一个恢复环境看不懂的值。
                if raw.starts_with("zh") {
                    ui_language = Some("zh".to_string());
                } else if raw.starts_with("en") {
                    ui_language = Some("en".to_string());
                } else {
                    return Err(err("--ui-language must be zh or en"));
                }
            }
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
    // 失败处理策略：CLI 传了就用 CLI 的，没传就读程序目录的设置文件
    //（GUI 设置窗口写的；读不到 → 默认 3 次 + 自动逃出，与 v2.6.8 行为一致）。
    // 两者都只在 prepare 期解析一次并落盘，恢复环境不再回读设置文件——
    // 任务一旦创建，它的失败处理策略就固定了，事后改设置不会改变已有任务的行为。
    let failure_policy = {
        let base = executable_dir()
            .map(|dir| crate::failure_policy::FailurePolicy::load_from_dir(&dir))
            .unwrap_or_default();
        crate::failure_policy::FailurePolicy {
            threshold: fail_threshold.unwrap_or(base.threshold),
            action: fail_action.unwrap_or(base.action),
            pause_max_wait_secs: fail_pause_max_wait.unwrap_or(base.pause_max_wait_secs),
        }
    };
    // DISM 等待上限：同一套优先级（CLI > 设置文件 > 默认 0=不限制）。
    // `load_from_dir` 自己处理"设置文件不存在/读失败 → 默认"，键名与 GUI
    // 设置窗口写入的一致（同一个 `BackupRestore.settings.ini`）。
    let dism_timeout_secs = dism_timeout_secs.unwrap_or_else(|| {
        executable_dir()
            .map(|dir| crate::dism_timeout::load_from_dir(&dir))
            .unwrap_or(crate::dism_timeout::DEFAULT_SECS)
    });
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
        format_target,
        failure_policy,
        dism_timeout_secs,
        ui_language: ui_language.unwrap_or_else(|| "zh".to_string()),
    })
}

/// 显式放弃或收口一个任务。
///
/// 两条路径：
/// - **未写入目标卷**（Prepared/BootRequested/RecoveryStarted/Preflight）：走完整准备期补偿
///   （清启动请求 → 删对象 → 删载荷 → 恢复 BCD 快照）。
/// - **已终结**（Failed，2026-10-16 用户拍板新增）：走终态收口——只清本任务的引导对象与载荷、
///   删掉大工作目录，**保留**证据记录。擦除后失败的任务以前既不能续也不能清（永久悬置）。
///
/// 仍拒绝的情形：**非终态且已越过破坏性边界**（TargetErased 之后但未标终态）——那说明恢复链
/// 还在进行中，需要先继续原任务或人工恢复，不能从清理入口直接抹掉。
#[cfg(windows)]
pub(crate) fn abandon_task(root: String, task_id: String) -> Result<(), TaskError> {
    // 先拿到日志路径再查提权：这样「不是管理员」这件事本身也能落进任务日志，
    // 而不是只留一句弹框文案。
    let log = TaskStore::new(root.clone()).log_path(&task_id)?;
    require_administrator(&log)?;
    let store = TaskStore::new(root);
    let mut task = store.load(&task_id)?;
    let dir = store.task_dir(&task_id)?;
    let status: backuprestore_core::StatusRecord =
        backuprestore_core::read_json(store.status_path(&task_id)?)?;
    if status.task_id != task.task_id
        || status.stage != task.status
        || status.operation != task.operation
    {
        return Err(crate::errc(ErrCode::TaskStateInconsistent));
    }
    // 【终态收口，2026-10-16 用户拍板】已终结（Failed）的任务允许收口：
    // 只清本任务的引导对象与载荷、删掉大工作目录，**保留** task.json / status.json /
    // manifest.json / 日志等证据记录，供人工诊断。
    //
    // 为什么需要这条入口：任务在擦除后失败会停在 TargetErased；恢复环境的自动逃出
    // 现在会把它标成 Failed（见 `main.rs::auto_escape_to_windows`），但在此之前
    // （以及任何人工介入的场景）用户既不能续跑、也不能清理——这正是 2026-10-16
    // 实机暴露的「非终态任务永久悬置」。收口时**必须同时清 bootsequence**
    // （`disarm` 已含，且只认本任务的 GUID），否则等于把「下次启动又进恢复环境」
    // 的锁原样留给下一次启动。
    if task.status == backuprestore_core::Stage::Failed {
        append_log(
            &log,
            &format!(
                "Collecting terminal (Failed) task; operation={:?} — disarm boot objects, \
                 remove payload, keep evidence",
                task.operation
            ),
        )?;
        match crate::boot_entry::ReBootEntry::read(&dir)? {
            Some(entry) => {
                crate::boot_entry::disarm(&entry, &log)?;
                append_log(&log, "terminal cleanup: one-shot boot entry disarmed")?;
            }
            None => append_log(
                &log,
                "terminal cleanup: no boot entry record on disk; nothing to disarm",
            )?,
        }
        store.cleanup_task_artifacts(&task.task_id)?;
        append_log(
            &log,
            "Terminal (Failed) task collected; evidence records retained",
        )?;
        return Ok(());
    }
    if !task.status.can_abandon_before_target_write() {
        return Err(crate::errc_with(
            ErrCode::TaskAlreadyAdvanced,
            &[&format!("{:?}", task.status)],
        ));
    }
    append_log(
        &log,
        &format!(
            "User explicitly abandoned task before target writes; operation={:?} stage={:?}",
            task.operation, task.status
        ),
    )?;
    let efi = efi_identity(None)?;
    // 【2026-10-18 缺陷修复：ef71fccc 三按钮死锁（续）】补偿失败**不再中断收口**。
    // 能走到这里的任务都在破坏性边界之前（目标卷还没动过），补偿只是尽量把准备期
    // 对 BCD 的改动还原。ESP 上的 BCD 被系统占用（错误 32，BFSVC 持锁）时还原不了，
    // 此时中断整个放弃流程 = 用户又被卡住——和刚修好的阶段闸门是同一个死锁，
    // 只是换了个位置复发。改为记 ERROR 后继续：BCD 保持现状（可用「引导修复(M)」
    // 或 bcdedit 手工处理），任务照样标终态、一次性启动项照样撤销。
    if let Err(cleanup) = compensate_prepare_failure(&dir, &efi, &log) {
        append_log(
            &log,
            &format!(
                "[ERROR] abandon: BCD compensation incomplete ({cleanup}); continuing to close the task"
            ),
        )?;
    }
    // create_entry can copy the WIM to staging before its final boot-entry
    // record is durable. Re-check by stable volume identity and remove only
    // this task's directory, even if no ReBootEntry record survived.
    let staging_volume = task.image.as_ref().map(|image| &image.volume).or_else(|| {
        task.destination
            .as_ref()
            .map(|destination| &destination.volume)
    });
    if let Some(volume) = staging_volume {
        let root_path = crate::boot_cleanup::staging_volume_root(volume)?;
        let actual =
            volume_identity_at_path(&root_path.to_string_lossy(), volume.volume_guid.clone())?;
        if !actual.same_partition(volume)
            || actual.partition_offset != volume.partition_offset
            || actual.partition_size != volume.partition_size
        {
            return Err(crate::errc(ErrCode::AbandonPayloadVolumeMismatch));
        }
        crate::boot_cleanup::remove_staging(&root_path, &task.task_id)?;
    }
    store.write_failure(&mut task, 2, "用户在创建新任务前明确放弃")?;
    store.cleanup_task_artifacts(&task.task_id)?;
    append_log(
        &log,
        "Abandoned task cleanup completed; evidence records retained",
    )?;
    Ok(())
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
    // 准备日志先建好再跑输入校验：`validate_operation_inputs` 里的 DISM 预检
    // （镜像完整性探测）要往这里写 stdout/stderr。早先日志目录在校验**之后**
    // 才创建，那段失败就无处落盘——正是 2026-10-16 审查点名的日志缺口 1。
    // 2026-10-18：落点改走 `log_root`（用户要求：不放 PE、不放目标分区）。
    let bootstrap_log = crate::log_root::logs_dir().join("prepare-bootstrap.log");
    if let Some(parent) = bootstrap_log.parent() {
        fs::create_dir_all(parent)?;
    }
    let validation = validate_operation_inputs(
        &source,
        &target,
        &image_volume,
        image_path.as_deref(),
        &options,
        &bootstrap_log,
    )?;
    let image_acceptance = validation.acceptance;
    let store = TaskStore::new(executable_dir);
    // S5 新启动项撞名消解（docs/20261015-0725）：拟用的菜单名与现有任一启动项
    // 重名时追加 -YYYYMMDDHHMM。GUI 侧已按用户可见的方式消解过一次（幂等，
    // 已加后缀的名字不会二次加），但 CLI 直跑不过 GUI，必须在这里兜底——
    // 否则 validate 的「启动菜单出现同名项」会直接拒掉任务，而用户看到的
    // 名字是他自己起的，完全无法归因。读不到 BCD 不阻断：退化成不消解，
    // 由 validate 的同名守卫以明确错误拦住，也好过 silently 建出两个同名项。
    let options = {
        let mut options = options;
        if options.operation == Operation::CreateSecondary {
            let existing = crate::boot_menu::existing_loader_descriptions(&bootstrap_log);
            let resolved = crate::boot_menu::resolve_menu_name_collision(
                &options.boot_menu_name,
                &existing,
                chrono::Local::now(),
            );
            if resolved != options.boot_menu_name.trim() {
                append_log(
                    &bootstrap_log,
                    &crate::i18n::msg_tpl(
                        "wp_boot_menu_dup",
                        &[&options.boot_menu_name, &resolved],
                    ),
                )?;
            }
            options.boot_menu_name = resolved;
        }
        options
    };
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
    // 一个工作区共用同一份 EFI BCD 的 bootsequence，无法安全并发两个未终结的
    // 启动事务。载荷虽按 task ID 隔离，但那只防止文件互相覆盖，不解决 bootsequence
    // 争用；因此新任务只要发现任何未终结任务，都先停下交给用户选择继续或清理。
    // 这一步必须位于创建新 task.json、导出 BCD 和修改任何启动对象之前。
    let pending = store.incomplete_tasks()?;
    if !pending.is_empty() {
        let details = pending
            .iter()
            .map(|task| {
                let target = task
                    .target
                    .as_ref()
                    .map(|spec| spec.volume.partition_guid.as_str())
                    .unwrap_or("(无目标卷)");
                format!(
                    "id={} operation={:?} stage={:?} target={} created={}",
                    task.task_id, task.operation, task.status, target, task.created
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        append_log(
            &bootstrap_log,
            &format!("new preparation blocked by incomplete tasks: {details}"),
        )?;
        return Err(crate::errc_with(ErrCode::UnterminatedTaskExists, &[&details]));
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
            // 备份前镜像卷空间预检（2026-10-26）：在「决定重启去做」之前就挡掉
            // 明显装不下的目标，省得进恢复环境写了几十 GiB 才失败。★ 硬闸比例
            // 故意很低（源已用量的 5%），宁可放过也不误拒——WIM 压缩比随内容
            // 变化极大，按固定比例预估会拦掉真实可行的备份。
            if let Some(path) = image_path.as_ref() {
                ensure_backup_image_capacity(
                    options.source_drive,
                    path,
                    options.compress.as_deref().unwrap_or("fast"),
                )?;
            }
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
            // 优先复用 sidecar metadata 里的权威 sha256；无 sidecar 镜像且未勾选 verify_hash 时填充合规占位，彻底避免重复读盘。
            // 例外：用户已显式接受"与副档不符的当前镜像"时，权威值改成**接受时读到的真实哈希**。
            // 否则任务里留着副档旧值 + verifyHash=true，重启后离线侧必然再次拒绝，
            // 用户的强制继续被静默撤销（历史缺陷）。
            let accepted_sha256 = image_acceptance
                .as_ref()
                .filter(|accepted| !accepted.accepted_sha256.is_empty())
                .map(|accepted| accepted.accepted_sha256.clone());
            let sha256 = if let Some(accepted) = accepted_sha256 {
                accepted
            } else if let Ok(meta) = crate::read_index_metadata(Path::new(path), options.wim_index)
            {
                meta.image_sha256
            } else if options.verify_hash {
                sha256_file(path)?
            } else {
                "0".repeat(64)
            };
            task.verify_hash = Some(options.verify_hash);
            task.image_acceptance = image_acceptance.clone();
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
                // 目标性质在这里定案并落盘：RE 内 `%SystemDrive%` 是恢复环境的 RAM 盘 X:，
                // 探测不到用户的系统卷，所以离线侧只能读这个结论（见字段文档）。
                // 取不到活动系统身份时直接失败，不要退化成 false——那会让「重建唯一系统」
                // 误走第二系统收尾。
                is_active_system: target.same_partition(&active_system_volume_identity()?),
                // 格式化选项落盘（GUI 复选框 → prepare 参数 → RE 恢复链读取）
                format_target: options.format_target,
                // 「覆盖式应用到已含 Windows 卷」的路由标记（任务 #110）：
                // 由 validate 判定，RE 恢复链读取后走 wimlib 通道。
                overlay_no_acl: validation.overlay_no_acl,
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
    // 新启动通道：载荷放到**镜像卷**（re_staging），注册位只读，不污染系统卷。
    //
    // 2026-10-13 修正（docs/20261013-1936 对照实验 + 单变量矩阵 A/B/C）：
    // 1935 的「bootmgr 只解析系统卷盘符」结论不完整——真正的情况是
    // **盘符形式**的 ramdisk 规格只对系统卷生效，数据卷必须写
    // \Device\HarddiskVolumeN 设备路径形式（bcdedit 显示层两种都规范化成 [X:]，
    // 存储层不同，所以从回读看不出差别）。设备路径形式在数据卷上实测可引导
    // （GUID 目录 + 隐藏/系统属性 + 设备路径 = 成功；同结构换成盘符 = 回落）。
    // 修复落在 boot_entry（写规格时解析设备路径），载荷因此可以留在镜像卷：
    // 不污染 C:，单系统还原格式化目标卷也不会抹掉断电续跑的载荷。
    // 2.4.5 曾把载荷改到系统卷作权宜——那会污染 C: 且 restore-existing 断电后
    // 无法续跑，本轮回退。
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
            // 把稳定错误码一起落盘：Io/Json/InvalidTransition 的 Display 故意不带码，
            // 只有 error.code() 拿得到（界面「刷新任务状态」据此显示）。
            let _ = store.write_failure_coded(
                &mut task,
                error.code(),
                error.to_string(),
                Some(1),
            );
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
    // `--json` 下用单行 JSON 取代两行 `KEY=VALUE`：判据只认字段，不再解析
    // 自然语言，也不必再区分「哪些命令有 TASK_ID 哪些没有」。
    // 非 json 模式保持旧的两行输出（既有脚本依赖它，不动）。
    // 详情上报给 `main` 统一输出（operation 由 main 从 `--operation` 参数推断，
    // 口径与 online 一致）。非 json 模式保持旧的两行输出。
    crate::cli_json::set_detail(
        Some(task.task_id.clone()),
        Some(prepare_log.display().to_string()),
    );
    if !crate::cli_json::enabled() {
        println!("TASK_ID={}", task.task_id);
        println!("TASK_ROOT={}", task_dir.display());
    }
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
    // ★ 问题 11：这一段（建 BCD 条目 → 载荷覆盖 → DISM 挂载/注入 → 卸载提交）
    // 失败时以前直接 `?` 返回，界面上是"准备失败"，机器上却可能留着本任务的
    // BCD 条目、镜像卷载荷，`boot-entry.json` 还指向它们，下一次重启进的不是
    // 用户期待的东西。现在整段收进闭包，任一失败都先做统一补偿再返回原错误——
    // 绝不为了"看起来干净"而把补偿失败盖成成功。
    let boot_entry = (|| -> Result<crate::boot_entry::ReBootEntry, TaskError> {
        // ★ 2026-10-12 实测修复：载荷与 RecoveryTask.env 必须**先**注入 staged，
        // 再建 BCD 条目 / 把 staged 复制到镜像卷暂存区（create_entry 与
        // copy_into_staging 都会把 staged 拷到 devopts 实际启动的暂存 WIM）。
        // 原顺序是「create_entry + copy_into_staging → DISM 注入」，于是 devopts
        // 启动的暂存 WIM 保留的是**注册位里的旧 env**——而 env 是任务相关的
        // （含 TASK_ID）。RE 用 winpeshl 拉起 Recovery.exe 读到的就是上一个任务的
        // env，表现为两种故障：① 注册位没 env 时 os error 2（文件不存在）；
        // ② 注册位有旧 env 时 "task is already terminal at stage Failed"（读到了
        // 上一个已终态的任务）。注入必须先于任何拷贝，否则 devopts 永远带着陈旧 env。
        // 风险注：DISM 在本进程跑过后紧接着 bcdedit 校验镜像卷 WIM 历史上偶发
        // 「指定的设备无效」；这里 DISM 针对的是 F: 上的 staged，提交后 sleep 5s
        // 再拷贝到 H: 并校验，实测未复现（若复现，把 copy_into_staging 与
        // create_entry 拆开、bcdedit 校验放最前即可）。
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

        let boot_entry = crate::boot_entry::create_entry(
            &staged,
            &re_staging.boot_sdi,
            &re_staging.volume,
            task_dir.as_path(),
            &task.task_id,
            log,
        )?;
        crate::boot_entry::copy_into_staging(
            &staged,
            &re_staging.boot_sdi,
            &re_staging.volume,
            &task.task_id,
            log,
        )?;
        Ok(boot_entry)
    })();
    if let Err(error) = &boot_entry {
        // 真正执行统一补偿：撤销本任务已建的启动项、载荷，并把 BCD 恢复到
        // 建条目之前导出的字节级快照。补偿本身失败时一并报出，不掩盖原错误。
        append_log(
            log,
            &format!(
                "[ERROR] preparation failed after boot entry was created ({error}); compensating"
            ),
        )?;
        if let Err(cleanup) = compensate_prepare_failure(task_dir.as_path(), efi, log) {
            append_log(
                log,
                &format!(
                    "[ERROR] compensation incomplete after preparation failure ({error}); {cleanup}"
                ),
            )?;
        }
        return Err(err(&format!(
            "preparation failed: {error}; boot entry and payload rolled back (see prepare.log)"
        )));
    }
    let mut boot_entry = boot_entry.expect("checked above");

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
    crate::boot_entry::copy_into_staging(
        &staged,
        &re_staging.boot_sdi,
        &re_staging.volume,
        &task.task_id,
        log,
    )?;
    // ★ 把簿记里的载荷哈希刷新成**注入后**的实际值。
    // create_entry 在 DISM 注入之前就跑（顺序要点见上），那时的
    // copy_into_staging 复制的是干净原件，记下的是它的哈希；注入后载荷
    // WIM 被覆写成另一份，哈希随之改变。不刷新的话，rearm() 在断电续跑时
    // 拿活载荷比对这份过期记录必然不符，整条续跑路被封死
    // （2026-09-30 实机：stage 卡在 image-applied/75，日志
    //  "payload WIM hash differs from the prepared one; refusing to re-arm"）。
    // ★ 问题 11（续）：`arm_one_shot` 之后同样要补偿。这一段失败比以前更危险——
    // 武装成功但状态落盘失败时，机器带着一个一次性启动请求重启，却没有任何
    // 持久状态说明这是谁设的，等于把人推进一个没人负责的任务环境。
    // 因此这里也必须统一撤销（清 bootsequence、删对象、删载荷、恢复 BCD 快照）。
    let armed = (|| -> Result<(), TaskError> {
        crate::boot_entry::refresh_payload_hash(&mut boot_entry, task_dir.as_path(), log)?;
        // 武装是「人可以重启了」的唯一开关，放在这一步：载荷已注入、已覆盖到镜像卷、
        // 簿记已落盘。v1.7.14 之前它藏在 create_entry 里无条件执行，导致
        // `--no-reboot` 也会改 bootmgr 的 bootsequence（实机证据见 boot_entry::arm_one_shot）。
        crate::boot_entry::arm_one_shot(&boot_entry, log)?;
        store.write_transition(task, backuprestore_core::Stage::BootRequested)?;
        write_status_env(&task_dir, task, "boot-requested")?;
        Ok(())
    })();
    if let Err(error) = armed {
        append_log(
            log,
            &format!("[ERROR] failed while arming the boot request ({error}); compensating"),
        )?;
        if let Err(cleanup) = compensate_prepare_failure(task_dir.as_path(), efi, log) {
            append_log(
                log,
                &format!(
                    "[ERROR] compensation incomplete after arming failure ({error}); {cleanup}"
                ),
            )?;
        }
        return Err(err(&format!(
            "preparation failed while arming the boot request: {error}; rolled back (see prepare.log)"
        )));
    }
    if options.test_fault.as_deref() == Some("power-loss-window") {
        append_log(
            log,
            "Development test fault: stopped after durable boot-requested state; no shutdown requested",
        )?;
        return Ok(());
    }
    // ★ 首启回落修复（2026-10-13 E2E 实测，见 flush_volume 注释）：武装成功后、
    // 请求重启前，把载荷卷与 ESP 的卷缓存显式落盘。失败与 shutdown 失败同样
    // 走统一补偿（此刻 bootsequence 已武装，绝不能留一个"可能读不到载荷"的
    // 一次性启动给下次开机）。
    let flush_before_reboot = (|| -> Result<(), TaskError> {
        let staging = crate::windows_prepare::ensure_volume_mounted(&boot_entry.wim_volume, 'R', log)?;
        flush_volume(staging, "载荷卷", log)?;
        let esp_letter = ensure_volume_mounted(&efi, 'S', log)?;
        flush_volume(esp_letter, "ESP/BCD", log)?;
        Ok(())
    })();
    if let Err(error) = flush_before_reboot {
        append_log(
            log,
            &format!("[ERROR] pre-reboot volume flush failed ({error}); compensating"),
        )?;
        if let Err(cleanup) = compensate_prepare_failure(task_dir.as_path(), efi, log) {
            append_log(
                log,
                &format!(
                    "[ERROR] compensation incomplete after flush failure ({error}); {cleanup}"
                ),
            )?;
        }
        return Err(err(&format!(
            "preparation failed at pre-reboot flush: {error}; rolled back (see prepare.log)"
        )));
    }
    if let Err(error) = run_logged("shutdown.exe", &["/r", "/t", "0"], log) {
        // 重启请求失败：别把机器留在「下次开机进任务 RE，但没有人会来跑」的状态。
        // 走与其它准备失败点**同一个**补偿路径（清 bootsequence → 删对象 →
        // 删载荷 → 恢复 BCD 快照），避免两处清理逻辑各自漂移。
        append_log(
            log,
            &format!("[ERROR] shutdown request failed ({error}); compensating"),
        )?;
        if let Err(cleanup) = compensate_prepare_failure(task_dir.as_path(), efi, log) {
            append_log(
                log,
                &format!(
                    "[ERROR] compensation incomplete after shutdown failure ({error}); {cleanup}"
                ),
            )?;
        }
        return Err(err(&format!(
            "shutdown request failed: {error}; boot entry and payload rolled back (see prepare.log)"
        )));
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

/// 镜像完整性预检（2.4.2 引入，2.4.3 恢复）：还原类操作要求的"完整 Windows"标记探测。
///
/// 背景（2026-10-11 实验，docs/20261011-2140）：纯 WinPE `boot.wim` 走
/// create-secondary 会在**目标卷已被格式化、镜像已应用之后**才死在离线 WinRE
/// 注册前置检查（缺 `reagentc.exe` 与 `SYSTEM` hive）——目标白毁。
/// 这里把同样的判据前移到**擦除之前**：只读挂载镜像探测两个标记文件，
/// 纯 PE 直接拒绝并提示改用「安装 PE」（那才是装 PE 的正路）。
///
/// 为什么拒绝不能去掉（2026-10-13 复核结论，d4bc2c6 一度放行后恢复；
/// 2026-10-16 随 P5 降级裁定订正注册相关论据）：
/// 放行并不能让纯 PE 走通这条路。恢复环境注册一步在 2026-10-16 已按用户裁定
/// 改为「缺注册组件就降级跳过、照常报成功」（`winre_registration` 的 preflight），
/// 不再因此把任务判失败；但 `bcdboot` 建的是常规 Windows 启动项（无 `winpe yes`），
/// STEP 3.5 盘符绑定也要写目标 SYSTEM hive——纯 PE 两者皆不满足，照样走不通，
/// 唯一区别是失败时目标卷**已被格式化**。装 PE 的正路是「安装 PE」
/// （内存版/硬盘版，带 `winpe yes`、不做 WinRE 注册）。
///
/// 挂载使用 `/ReadOnly`，探测后 `/Unmount /Discard`，不改动镜像文件。
///
/// `log` 必须传入任务自己的 `prepare.log`：DISM 调用的 stdout/stderr 全部落盘，
/// 失败时错误消息里带输出尾部。此前的实现只取 `status`，挂载失败在日志里只剩
/// "mount failed" 三个字，事后无法定位（该 87 的真因是参数名，见 `wim_mount_args`）。
///
/// 【v2.6.23 性能分级】32GB WIM 的只读挂载实测 6–7 分钟，每次 prepare 都付这笔账
/// 太贵（用户点完「创建任务」要干等 8 分钟）。因此先花约 2 秒取镜像显示名：
/// 名字明确是完整 Windows 系统镜像时**快速放行**，只有名字可疑或取不到时才做
/// 权威挂载探测。安全判据并未回退——可疑的一律挂挂载判真，恢复阶段的离线注册
/// 前置检查是所有路径的硬兜底。
pub(crate) fn wim_full_windows_probe(
    image_path: &str,
    index: u32,
    log: &Path,
) -> Result<ProbeOutcome, TaskError> {
    // 先取名字（/Get-WimInfo，约 2 秒）。取不到时按空名处理 → 落回权威挂载。
    let name = wim_image_display_name(image_path, index, log).unwrap_or_default();
    if name_looks_like_full_windows(&name) {
        return Ok(ProbeOutcome::QuickPass(name));
    }
    match wim_mount_markers(image_path, index, log) {
        // 挂载成功：权威判定。缺标记 = 纯 PE，拒绝（此刻尚未格式化任何卷）。
        Ok(false) => Err(crate::errc(ErrCode::ImageNotCompleteWindows)),
        Ok(true) => Ok(ProbeOutcome::FullWindows),
        // 挂载本身失败：不硬拦，但调用方必须把这次降级写进日志。
        // 【2026-10-22 更正归因】此前这里记的是「有 VHD 附加时 /Mount-Wim 一律报 87」，
        // 属误判：真因是本函数把 `/Mount-Wim` 的参数写成了 `/ImageFile`（那是
        // `/Apply-Image` 的选项），DISM 恒报 87 → 权威探测自 2.4.2 引入起**从未成功过**，
        // 一直静默走下面的镜像名启发式。v2.6.22 起改用 `/WimFile:`（见 `wim_mount_args`）。
        // 仍保留降级分支：环境因素（镜像被别的进程占用、权限、挂载点磁盘满）确实会让
        // 挂载失败，此时硬拦会误杀合法还原；恢复阶段的离线注册前置检查仍是硬兜底。
        Err(_) => {
            let lower = name.to_ascii_lowercase();
            if lower.contains("windows pe") || lower.contains("winpe") {
                return Err(crate::errc_with(ErrCode::ImageIsWindowsPe, &[&name]));
            }
            Ok(ProbeOutcome::HeuristicPass)
        }
    }
}

/// 镜像显示名是否**明确**是完整 Windows 系统镜像——用来决定能不能跳过耗时的挂载探测。
///
/// 判据（故意偏保守，宁可多挂一次也不放过可疑镜像）：
/// - 必须含 "windows"；
/// - 不得含 PE 特征（"pe" / "preinstallation" / "recovery"）；
/// - 名字不能太短（< 8 字符视为可疑）。
///
/// 本程序自己生成的备份索引名形如「Windows 11 26200.9550 20261015」，恒命中 →
/// 常规路径零额外耗时；改过名的第三方/PE 镜像落回挂载权威判定。
fn name_looks_like_full_windows(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.len() < 8 {
        return false;
    }
    if lower.contains("pe") || lower.contains("preinstallation") || lower.contains("recovery") {
        return false;
    }
    lower.contains("windows")
}

/// 探测结果：FullWindows=挂载后权威确认；QuickPass=镜像名明确是完整 Windows、
/// 走快速通道放行；HeuristicPass=挂载不可用、按镜像名启发式放行
/// （调用方应记录每一次降级，便于事后核查）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeOutcome {
    FullWindows,
    QuickPass(String),
    HeuristicPass,
}

/// `/Mount-Wim` 的完整参数。
///
/// 单独抽出来是为了让单测能锁死参数名：**`/Mount-Wim` 只认 `/WimFile`**，
/// 写成 `/ImageFile`（那是 `/Apply-Image` 的选项）DISM 恒报 87。
/// 历史事故：自 2.4.2 引入至 v2.6.21，本参数一直是错的，导致「完整 Windows
/// 门禁」的权威探测从未生效、每次都静默降级为镜像名启发式（详见 docs/20261020-1025）。
fn wim_mount_args(image_path: &str, index: u32, mount_dir: &Path) -> Vec<String> {
    vec![
        "/Mount-Wim".to_string(),
        format!("/WimFile:{image_path}"),
        format!("/Index:{index}"),
        "/ReadOnly".to_string(),
        format!("/MountDir:{}", mount_dir.display()),
    ]
}

/// 只读挂载镜像并检查完整 Windows 标记（reagentc.exe + SYSTEM hive）。
/// 返回 Ok(true)=标记齐全；Ok(false)=缺标记（纯 PE）；Err=挂载/卸载失败。
///
/// 失败的错误消息带 DISM 输出尾部，完整输出已落进 `log`。
fn wim_mount_markers(image_path: &str, index: u32, log: &Path) -> Result<bool, TaskError> {
    let mount_dir = std::env::temp_dir().join(format!("br-wimprobe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&mount_dir);
    std::fs::create_dir_all(&mount_dir)?;
    let run = |args: &[&str]| -> Result<Output, TaskError> {
        crate::run_output_logged("dism.exe", args, log)
            .map_err(|error| crate::errc_with(ErrCode::DismSpawnFailed, &[&error.to_string()]))
    };
    let mount_args = wim_mount_args(image_path, index, &mount_dir);
    let mount_refs: Vec<&str> = mount_args.iter().map(String::as_str).collect();
    let mounted = run(&mount_refs)?;
    if !mounted.status.success() {
        let _ = std::fs::remove_dir_all(&mount_dir);
        let tail = crate::output_tail_summary(&mounted, 6);
        let detail = if tail.is_empty() {
            String::new()
        } else {
            format!("：{tail}")
        };
        return Err(crate::errc_with(
            ErrCode::ImageMountProbeFailed,
            &[&format!("{}", mounted.status), &detail],
        ));
    }
    let has_reagentc = mount_dir.join(r"Windows\System32\reagentc.exe").is_file();
    let has_system_hive = mount_dir
        .join(r"Windows\System32\config\SYSTEM")
        .is_file();
    // 无论判定结果如何都先卸载；卸载失败会锁住镜像，必须如实报错。
    let unmounted = run(&[
        "/Unmount-Wim",
        &format!("/MountDir:{}", mount_dir.display()),
        "/Discard",
    ])?;
    let _ = std::fs::remove_dir_all(&mount_dir);
    if !unmounted.status.success() {
        let tail = crate::output_tail_summary(&unmounted, 6);
        let detail = if tail.is_empty() {
            String::new()
        } else {
            format!("：{tail}")
        };
        return Err(crate::errc_with(
            ErrCode::ImageUnmountFailed,
            &[&format!("{}", unmounted.status), &detail],
        ));
    }
    Ok(has_reagentc && has_system_hive)
}

/// 读取镜像指定索引的显示名（Name 行；缺失时退回 Description）。
///
/// 失败时错误消息带 DISM 输出尾部（完整输出已落进 `log`），避免只留一句
/// "Get-WimInfo failed"。
fn wim_image_display_name(image_path: &str, index: u32, log: &Path) -> Result<String, TaskError> {
    let output = crate::run_output_logged(
        "dism.exe",
        &[
            "/English",
            "/Get-WimInfo",
            // 注意：/Get-WimInfo 的参数是 /WimFile（/ImageFile 是 Mount/Apply 的）。
            &format!("/WimFile:{image_path}"),
            &format!("/Index:{index}"),
        ],
        log,
    )?;
    if !output.status.success() {
        let tail = crate::output_tail_summary(&output, 6);
        let detail = if tail.is_empty() {
            String::new()
        } else {
            format!("：{tail}")
        };
        return Err(crate::errc_with(
            ErrCode::ImageNameReadFailed,
            &[&format!("{}", output.status), &detail],
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("name") && !value.trim().is_empty() {
            return Ok(value.trim().to_string());
        }
    }
    Ok(String::new())
}

/// 把一个卷的文件系统缓存强制落盘（打开卷设备句柄 + FlushFileBuffers）。
///
/// 2026-10-13 E2E 实测（docs/20261013-20xx 单变量矩阵 + 重武装复验）：prepare 在
/// 722MB 载荷被 DISM 注入原地覆写、bootsequence 武装完成后 **25 秒内** `shutdown /r /t 0`，
/// 首次引导 bootmgr 加载 ramdisk 失败而回落；**同一条目、同一载荷** 4 分钟后重新武装
/// 再启（Windows 已完整运行、缓存自然落盘）一次成功。优雅关机理论上会刷缓存，但
/// 虚拟化层写回缓存 + `/t 0` 极短窗口下实测不可靠。武装后、请求重启前对**载荷卷**
/// 与 **ESP**（BCD 也在这两个卷上）显式刷盘，堵掉这个窗口。
/// 刷盘失败按准备失败处理并走统一补偿——宁可不来这次任务，也不留"下次开机
/// 一次性启动被静默丢弃"的不可预期状态。
#[cfg(windows)]
fn flush_volume(letter: char, label: &str, log: &Path) -> Result<(), TaskError> {
    let path: Vec<u16> = format!(r"\\.\\{}:", letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle as isize == -1 {
        return Err(crate::errc_with(
            ErrCode::VolumeFlushOpenFailed,
            &[&letter.to_string(), label, &unsafe { GetLastError() }.to_string()],
        ));
    }
    let ok = unsafe { FlushFileBuffers(handle) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return Err(crate::errc_with(ErrCode::VolumeFlushFailed, &[&letter.to_string(), label]));
    }
    append_log(log, &crate::i18n::msg_tpl("wp_volume_cache_flushed", &[&format!("{letter}"), label]))?;
    Ok(())
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
        0 => Err(last_error.unwrap_or_else(|| crate::errc(ErrCode::NoBcdEfiPartition))),
        1 => Ok(matches.remove(0)),
        _ => {
            // 多 ESP 自动识别（2.4.2）：不再一律拒绝。判据是**活动 BCD 指纹**——
            // `bcdedit`（不带 /store）枚举的就是本次引导实际使用的 BCD；把每个
            // 候选 ESP 的 {bootmgr} 状态（default/displayorder/resumeobject）与它
            // 比对，**恰好一个**全等即认定该 ESP 是引导 ESP，安全选用。
            // 指纹不匹配的 ESP 属于别的系统/磁盘，写它会把引导改到错误的链上；
            // 指纹全等的候选不止一个（克隆盘）时仍然拒绝猜测——保护不变。
            if let Some(active) = active_esp_by_store_fingerprint(&matches) {
                return Ok(active.identity);
            }
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
            Err(crate::errc_with(ErrCode::MultipleEspAmbiguous, &[&list]))
        }
    }
}

/// 按活动 BCD 指纹在多个 ESP 候选里挑出引导 ESP。
///
/// 返回 `None` 表示无法唯一确定（活动指纹读不到 / 无匹配 / 多于一个匹配）。
fn active_esp_by_store_fingerprint(candidates: &[VolumeIdentity]) -> Option<ActiveEspPick> {
    let active = boot_manager_fingerprint(None)?;
    let mut hit: Option<ActiveEspPick> = None;
    for identity in candidates {
        // volume_guid 字段存的是完整路径（\\?\Volume{GUID}\），先剥成裸 {GUID} 再拼。
        let Some(guid) = crate::text_parsing::bare_volume_guid(&identity.volume_guid) else {
            continue;
        };
        let store = format!(r"\\?\Volume{guid}\EFI\Microsoft\Boot\BCD");
        let Some(fingerprint) = boot_manager_fingerprint(Some(&store)) else {
            continue;
        };
        if fingerprint != active {
            continue;
        }
        if hit.is_some() {
            // 克隆盘：两个 ESP 的 BCD 内容一致，仍拒绝猜测。
            return None;
        }
        hit = Some(ActiveEspPick {
            identity: identity.clone(),
            fingerprint,
        });
    }
    hit
}

/// 读取一个 BCD store 的 {bootmgr} 指纹；`store=None` 时读活动系统 store。
/// 指纹取 (default, display_order, resume_object) 的小写归一元组——这三个字段
/// 足以区分"本次引导链"与"别的系统的 BCD"，且复用既有解析器
/// （`parse_boot_manager_state` 已有本地化与别名相关单测）。
fn boot_manager_fingerprint(store: Option<&str>) -> Option<(String, Vec<String>, String)> {
    let mut arguments: Vec<String> = Vec::new();
    if let Some(store) = store {
        arguments.push("/store".into());
        arguments.push(store.to_string());
    }
    arguments.extend(["/enum".into(), "{bootmgr}".into(), "/v".into()]);
    let output = run_bcdedit_quiet(&arguments).ok()?;
    let state = crate::parse_boot_manager_state(&output)?;
    Some((
        state.default.to_ascii_lowercase(),
        state
            .display_order
            .iter()
            .map(|id| id.to_ascii_lowercase())
            .collect(),
        state.resume_object.unwrap_or_default().to_ascii_lowercase(),
    ))
}

/// 无日志副作用的 bcdedit 调用（ESP 识别属探测路径，不该污染任务日志）。
fn run_bcdedit_quiet(arguments: &[String]) -> Result<String, TaskError> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("bcdedit.exe")
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        // 【2026-10-27】同批修正：外部命令失败必须带码（原来是无码 `err()`，
        // `--json` 的 errorCode 会是 null）。BR-PE-039 = 外部程序失败，与
        // `capture_logged` / `run_command` 一致；bcdedit 的真实原因在输出里，
        // 所以附尾部（内存字符串，不重读文件）。
        return Err(crate::errc_with(
            ErrCode::ProgramFailed,
            &[
                "bcdedit.exe",
                &format!("{}", output.status),
                crate::text_parsing::tail_of_text(&text, crate::text_parsing::LOG_TAIL_MAX_BYTES),
            ],
        ));
    }
    Ok(text)
}

/// `active_esp_by_store_fingerprint` 的命中结果（持有克隆，避免借用纠缠）。
struct ActiveEspPick {
    identity: VolumeIdentity,
    #[allow(dead_code)] // 指纹留着便于将来落日志；当前仅用于唯一性判定。
    fingerprint: (String, Vec<String>, String),
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
        .ok_or_else(|| crate::errc(ErrCode::StableGuidMissing))?;
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

/// 准备失败的**统一补偿**：撤销本任务已产生的全部副作用。
///
/// ## 为什么需要它
///
/// 此前只有 `shutdown.exe` 那一步失败会走回滚。`create_entry`（写 BCD 对象）、
/// DISM 挂载/注入、载荷覆盖到镜像卷、`refresh_payload_hash`、`arm_one_shot`、状态落盘
/// 这几处失败都是直接 `?` 返回：界面报告"准备失败"，而机器上可能留着本任务的
/// BCD 条目、一次性启动请求，或镜像卷上的载荷文件，`boot-entry.json` 还指向它们。
/// 下一次重启进的不是用户期待的东西。
///
/// ## 补偿顺序与安全边界
///
/// 1. **先清一次性启动请求**：反向保证机器不会重启进任务 RE；
/// 2. **删 BCD 对象**：只删本任务记录在案的那两个 GUID，绝不用描述字符串匹配——
///    那会误删别的任务的对象（`{default}` 的事故就是这么来的）；
/// 3. **删镜像卷上的载荷**：按任务 ID 定位，只删本任务那一份；
/// 4. **恢复 BCD 快照**：`create_entry` 之前导出过字节级快照，此时才回到基线。
///
/// 只清理**已核验属于本任务**的对象；每一步都回读验证，任何一步失败都会
/// 带着完整错误继续，最后汇总成一个 `Result`——补偿失败绝不能静默变成
/// "准备失败但机器是干净的"。
///
/// 幂等：没有簿记（副作用还没建到那一步）时只做它能做的部分并返回成功。
#[cfg(windows)]
fn compensate_prepare_failure(
    task_dir: &Path,
    efi: &VolumeIdentity,
    log: &Path,
) -> Result<(), TaskError> {
    let mut problems: Vec<String> = Vec::new();
    let record = crate::boot_entry::ReBootEntry::read(task_dir)?;
    let residue = crate::boot_record::ResidueRecord::read(task_dir)?;
    if record.is_none() && residue.is_none() {
        // 副作用尚未进展到建 BCD 对象；这里仍要尝试恢复 BCD 快照，
        // 因为导出快照本身不写 BCD，但没有对象也就没有可撤销的东西。
        append_log(
            log,
            "preparation compensation: no boot entry recorded; nothing to disarm",
        )?;
    }
    // 正式簿记：走完整 disarm（清启动请求 → 删对象 → 删本任务载荷）。
    if let Some(entry) = record.as_ref()
        && let Err(error) = crate::boot_entry::disarm(entry, log)
    {
        problems.push(format!("boot entry disarm: {error}"));
    }
    // 半成品簿记：它的 `device`/`osdevice` 从未写成功，载荷也未必已就位，
    // 因此**只**删那两个 BCD 对象，不去核验启动链或删载荷——否则 disarm 会在
    // 载荷校验处报错，把"对象其实已经删掉了"说成补偿失败。
    if let Some(entry) = residue.as_ref() {
        if let Err(error) =
            crate::boot_entry::remove_entry_objects(&entry.loader_guid, &entry.devopts_guid, log)
        {
            problems.push(format!("residue objects removal: {error}"));
        }
        match crate::boot_record::ResidueRecord::clear(task_dir) {
            Ok(()) => append_log(log, "preparation compensation: residue record cleared")?,
            Err(error) => problems.push(format!("residue record cleanup: {error}")),
        }
    }
    if let Err(error) = rollback_boot_request(task_dir, efi, log) {
        problems.push(format!("BCD snapshot restore: {error}"));
    }
    if problems.is_empty() {
        append_log(
            log,
            "preparation compensation: all task-owned side effects undone and verified",
        )?;
        Ok(())
    } else {
        Err(crate::errc_with(
            ErrCode::PreparationCompensationIncomplete,
            &[&problems.join("; ")],
        ))
    }
}

/// 把 ESP 上的 BCD 恢复成任务准备期保存的快照。
///
/// 原先只在准备期失败补偿时内部调用（`rollback_boot_request`）；2026-10-16 起
/// 恢复界面的「BCD 回退」按钮也调它——用户在恢复环境里发现引导被改坏时，
/// 可以一键退回准备期状态，不必再从宿主侧挂 PE 光盘手改。
pub(crate) fn rollback_bcd_snapshot(
    task_dir: &Path,
    efi: &VolumeIdentity,
    log: &Path,
) -> Result<(), TaskError> {
    rollback_boot_request(task_dir, efi, log)
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
                    append_log(log, "Restored byte-for-byte EFI BCD snapshot")?;
                }
                Err(error) if error.raw_os_error() == Some(32) => {
                    // 【2026-10-18 缺陷修复】锁定时**不许**退回 `bcdedit /import`。
                    //
                    // 实测（run 2，在线还原 C→E）：ESP BCD 一旦被 BFSVC 以写方式
                    // 打开（本例是 STEP 3.6 的 bcdboot 预建启动项），原始文件即被
                    // 独占，raw copy 报错误 32（ERROR_SHARING_VIOLATION）。此时
                    // 走 `bcdedit /import` 虽能"导入成功"（BCD 文件 mtime 确实
                    // 变化），但它会替换 BFSVC 已打开的存储文件，使整个会话的引导
                    // 服务状态失效：此后 bcdboot / bcdedit 全部
                    // `BcdOpenStore failed ... c0000043`，直到重启才恢复
                    // （run 2 的 STEP 3.6 之后任务全挂即此链；手动复现两次，
                    // 重启后 bcdboot EXIT=0 证实毒化仅会话级）。
                    //
                    // 字节级快照落不了盘时，唯一安全的动作是明确失败：
                    // WinRE/PE 下系统未运行，raw 主路径本来就不会遇到这把锁，
                    // 用户重启进恢复环境后即可正常回退。
                    let detail = format!(
                        "ESP 上的 BCD 正被系统占用（错误 32，共享冲突），无法按字节回退。\
                         请重启进入恢复环境（WinRE/PE）后重试；在线会话下也可用\
                         「引导修复(M)」手工调整启动项"
                    );
                    append_log(log, &format!("EFI BCD raw restore blocked: {detail}"))?;
                    return Err(err(&detail));
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
    // 遗留分支：只有**旧任务**（v2.6.x 之前 prepare、当时 raw 快照被锁而回退到
    // 逻辑导出的任务目录）才走得通——现行三条准备链都在 raw 快照可用时写入
    // `bcd-before-raw`，上面的分支才是主路径。
    //
    // ⚠️ 毒性警告（2026-10-18 实测，见上面错误 32 分支的详细记录）：
    // `bcdedit /import` 会替换 BFSVC 已打开的存储文件，毒化当前会话的引导服务
    // 状态（此后 bcdboot / bcdedit 全部 BcdOpenStore failed，直到重启）。
    // 此路径仅因「没有字节级快照时至少还能试着逻辑回退」而保留；若将来要清理
    // 旧任务兼容，第一件该删的就是它，而不是把 /import 挪到别处复用。
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

/// 返回 `Some(..)` 表示用户在本次准备中显式接受了「与副档不符的当前镜像」，
/// 调用方必须把这份例外连同**接受时的真实哈希**一起写进任务，否则重启后
/// 离线侧会拿旧副档值再次拒绝（历史缺陷，见 [`ImageAcceptance`]）。
/// `validate_operation_inputs` 的产出（2026-10-22 任务 #110 扩展）。
pub(crate) struct ValidationOutcome {
    /// 用户显式接受了与副档不符的当前镜像（原返回值）。
    pub acceptance: Option<ImageAcceptance>,
    /// 命中「覆盖式应用（取消格式化）打到已含 Windows 的目标卷」组合。
    ///
    /// 2026-10-22 起该组合**不再硬拒**（用户裁定「换 wimlib」）：
    /// 准备期在此落盘 `overlay_no_acl=true`，恢复期（WinRE 链）据此走 wimlib
    /// 通道——DISM 该组合必然 `RestoreDirMetadata 0x80070005`（PE 内对照实验
    /// 钉死，见 docs/20261016-1413 §2），wimlib 自行获取 SeRestore/SeBackup
    /// 提权且该提权覆盖 SD 还原（2026-10-23 概念验证，docs/20261022-2308：
    /// tiny + 全量两级，镜像 SD 逐字节还原），故镜像目录权限完整还原、
    /// 个别受保护目录可能保留卷上现有权限（GUI 确认框明示）。
    /// GUI 侧有显式确认框明示该语义；CLI 直跑视为高级用户
    /// 显式选择 `--skip-format` 的意图。
    /// **名字沿袭历史**：`overlay_no_acl` 里的 `no_acl` 是 v2.6.25–v2.6.26
    /// 「跳过 SD 还原」时代的残留，当前语义 = 覆盖式应用（安全描述符随镜像
    /// 还原）；不改名是为了 task.json 序列化兼容。
    pub overlay_no_acl: bool,
}

fn validate_operation_inputs(
    source: &VolumeIdentity,
    target: &VolumeIdentity,
    image: &VolumeIdentity,
    image_path: Option<&str>,
    options: &PrepareOptions,
    log: &Path,
) -> Result<ValidationOutcome, TaskError> {
    match options.operation {
        Operation::Probe => Ok(ValidationOutcome {
            acceptance: None,
            overlay_no_acl: false,
        }),
        Operation::Backup => {
            if image.same_partition(source) {
                return Err(err("image volume must differ from backup source"));
            }
            // 盘符在这里**显式**取，不靠 400 行外那条为 BitLocker 写的
            // `assert_bitlocker_off(...ok_or_else...)` 顺带保证：那条守卫
            // 一旦被重构掉，这里就会退化成 panic。
            let source_letter = source
                .drive_letter
                .ok_or_else(|| err("backup source volume has no drive letter"))?;
            let image_letter = image
                .drive_letter
                .ok_or_else(|| err("image volume has no drive letter"))?;
            let required = source
                .partition_size
                .saturating_sub(volume_free_bytes(source_letter)?)
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
            if volume_free_bytes(image_letter)? < required {
                return Err(err("backup destination free space is insufficient"));
            }
            Ok(ValidationOutcome {
                acceptance: None,
                overlay_no_acl: false,
            })
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            // 「覆盖式应用到已含 Windows 卷」的路由标记：命中组合时置 true，
            // 由调用方写进 `TargetSpec.overlay_no_acl` 落盘（见
            // [`ValidationOutcome`] 文档）。未命中（含 Backup）恒 false。
            let mut overlay_no_acl = false;
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
            // 目标是不是当前正在运行的那套系统。这个结论下面要用三次：P0 安全闸、
            // 是否需要完整 Windows 镜像（P2）、以及落进任务给 RE 读的 is_active_system。
            let target_is_active = target.same_partition(&active_system_volume_identity()?);
            // 【P0 安全闸】create-secondary 绝不能把目标指向当前正在运行的系统卷。
            //
            // 在这之前唯一挡着的是上面那条 `source != target`，而它可以用「源选别的卷」
            // 绕开：实测 `--operation create-secondary --source-drive T --target-drive C`
            // 会被接受，重启进 RE 后 diskpart 直接格式化正在运行的系统盘。GUI 同样能
            // 构造（源卷选 T:、目标选 C:），因为 GUI 的离线分流只覆盖
            // backup / restore-existing，create-secondary 不走 `%SystemDrive%` 检查。
            //
            // restore-existing **不加**这条：它的目标就是当前系统卷，属于合法场景
            // （重建唯一系统，GUI 会弹窗走 PE/RE 离线链）。
            if options.operation == Operation::CreateSecondary && target_is_active {
                return Err(err(
                    "create-secondary target must not be the active Windows volume",
                ));
            }
            // 【2026-10-18 缺陷修复 H1 第二层：prepare 侧同判据】
            // restore-existing **反过来**：它的目标必须是当前正在运行的系统卷
            // （重建唯一系统）。此前这里只断言 `source == target`，而 A 入口的
            // GUI 目标自动选中会静默失效（下拉停在上一个入口选中的盘），于是
            // 「源＝目标＝某个数据盘」也能一路通到「格式化该盘 + 灌镜像」。
            // GUI 已加同判据拦截，这里是**不依赖 GUI 的最终防线**（CLI 直通、
            // 测试钩子、GUI 绕过都挡在这里）。
            //
            // 前提：`prepare` 只在线上 Windows 会话里跑（GUI/CLI 建任务阶段），
            // 故 `%SystemDrive%` 就是那套正在运行的 Windows；RE 侧执行的是
            // recover/resume，不走本函数。
            if options.operation == Operation::RestoreExisting && !target_is_active {
                return Err(err(
                    "restore-existing target must be the active Windows volume",
                ));
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
            // 擦除前确认镜像是完整 Windows（纯 PE 走这里会在格式化后才失败）。
            //
            // 【P2】但数据卷还原必须放行：数据卷镜像本来就没有 reagentc.exe / SYSTEM
            // hive，拿完整 Windows 的标准去卡它等于禁掉整个数据卷离线通道。判据用
            // **目标卷**当前有无 SYSTEM hive，与恢复期分叉 BCDBoot 的判据完全一致
            // （`main.rs` 的 data-volume 分支）：目标不是 Windows 卷 → 这次只铺数据、
            // 不碰引导，镜像缺 Windows 标记属正常。
            //
            // 本机此前是靠「挂载报 87 → 镜像名启发式放行」才通过的，换一台 DISM 挂载
            // 可用的机器，数据卷的离线还原会被硬拒（实测路径见 docs 待改清单 P2）。
            //
            // ⚠️ create-secondary 不在放行范围内：装第二系统必须是完整 Windows，
            // 否则产出一个进不了桌面的引导项。
            //
            // ⚠️ 活动系统卷**不做文件探测**：它的 SYSTEM hive 被系统独占，`probe_file`
            // 会 `File::open` 它并撞上共享违例（os error 32），那个错误不是 NotFound、
            // 会直接把准备阶段弄失败。而活动系统卷必然是 Windows 卷，无需探测。
            let target_is_windows = target_is_active
                || crate::pe_safety::target_is_windows_volume(&PathBuf::from(format!(
                    "{}:\\",
                    target.drive_letter.ok_or_else(|| crate::errc(ErrCode::TargetLetterMissing))?
                )))?;
            // 【准备期判定 · 2026-10-22 任务 #110：原「硬拒」已改「路由」】
            // 「系统卷 + 取消格式化（覆盖式应用）」对 DISM 是确定性失败组合
            // （根因见下）；2026-10-22 起不再挡在准备期，改为在此判定并落盘
            // `overlay_no_acl`，恢复期走 wimlib 通道。判定仍必须在
            // 准备期完成：RE 内探测不到宿主卷的 Windows 属性（`%SystemDrive%`
            // 是恢复环境自己的 RAM 盘 X:），与 `is_active_system` 同一理由。
            //
            // 精确根因（2026-10-16 PE 内对照实验钉死，见 docs/20261016-1413 §2 与
            // docs/20261016-1836）：DISM 覆盖式应用要把镜像里
            // `C:\Program Files\WindowsApps\...`（MSIX 应用包目录，owner =
            // NT SERVICE\TrustedInstaller，带 MSIX 专用 ACL）的目录元数据 /
            // Security Descriptor / owner 还原到卷上；该目录受 TrustedInstaller 保护，
            // 操作系统拒绝改写其 owner/ACL → `RestoreDirMetadata:(2418): Access is
            // denied` → `0x80070005` → DISM 立即整体放弃（exit 5）。
            // 实测旁证：`md` 在该目录下能建子目录、`rd` 能删（普通写入允许），
            // **只有"改写既有受保护目录元数据"这一步被拒**；格式化能过是因为受保护
            // 目录被清空，DISM 只需"新建"（允许）而非"改写"。
            //
            // 该约束与 RE/在线/PE 环境无关、与 DISM 版本无关：PE 里同镜像同样失败。
            // `--skip-format` 只对**数据卷**（无 Windows 的卷）有效（数据卷里没有
            // WindowsApps，2026-10-14 已在 T: 验证），对**已含 Windows 的系统卷**无效。
            //
            // 判据用「目标卷当前是否含 Windows」而非「是否活动系统卷」：任何既有
            // Windows 卷都带 TrustedInstaller 保护的目录，都会命中同一根因。
            //
            // 【2026-10-22 任务 #110：硬拒改路由】用户裁定「换 wimlib」
            // 后，本组合不再 `return Err`：置 `overlay_no_acl=true` 落进任务，
            // 恢复期走 wimlib 通道（wimlib 自行获取 SeRestore/SeBackup 提权，
            // RE 内可用；2026-10-23 概念验证 docs/20261022-2308 证明该提权
            // 同时覆盖 SD 还原——镜像目录权限完整还原，个别受保护目录可能
            // 保留卷上现有权限，GUI 确认框明示该语义）。
            // CLI 直跑 --skip-format 视为高级用户显式意图。
            if crate::pe_safety::overlay_restore_on_windows_volume(
                target_is_windows,
                options.format_target,
            ) {
                overlay_no_acl = true;
                append_log(
                    log,
                    crate::i18n::msg("wp_overlay_marked"),
                )?;
            }
            if options.operation == Operation::CreateSecondary || target_is_windows {
                // 降级放行必须留痕：否则事后无法判断「门禁到底跑没跑过」。
                // 2026-10-22 教训——参数错导致权威探测从未生效，而日志里一个字都没有。
                match wim_full_windows_probe(path, options.wim_index, log)? {
                    ProbeOutcome::FullWindows => append_log(
                        log,
                        crate::i18n::msg("wp_precheck_mounted_ok"),
                    )?,
                    ProbeOutcome::QuickPass(name) => append_log(
                        log,
                        &crate::i18n::msg_tpl("wp_precheck_fast_path", &[name.as_str()]),
                    )?,
                    ProbeOutcome::HeuristicPass => append_log(
                        log,
                        crate::i18n::msg("wp_precheck_heuristic"),
                    )?,
                }
            }
            // 配套备份 metadata（.index-N.metadata.json）存在时校验 WIM 完整性；
            // 优先比对物理文件大小，避免在提权准备阶段对 58GB 镜像再次耗时 2 分钟重复读盘计算哈希。
            // 缺失时视为第三方/PE WIM（如安装 WinRE/PE 为第二系统），跳过
            // 哈希校验与目标大小预检——DISM /Get-WimInfo 已确认 WIM 可读。
            let metadata = crate::read_index_metadata(Path::new(path), options.wim_index).ok();
            // 用户显式接受当前镜像时在这里产生例外记录；只有真正越过了副档判据才记，
            // 正常通过校验的镜像不写例外（避免例外退化成默认状态）。
            let mut acceptance: Option<ImageAcceptance> = None;
            if let Some(metadata) = &metadata {
                let file_size = fs::metadata(path)?.len();
                let size_conflict = metadata.image_size > 0 && file_size != metadata.image_size;
                if size_conflict && !options.force_restore_hash {
                    return Err(err("restore image size does not match metadata"));
                }
                // 用户要求：增加一个校验选项框，不勾选时仅对比大小，勾选时对比哈希
                if options.verify_hash {
                    let expected = &metadata.image_sha256;
                    // 这一次读盘是严格哈希模式本来就要付的代价；下面复用同一个值，
                    // 不额外再读一遍 58GB 镜像。
                    let actual = sha256_file(path)?;
                    let hash_conflict = !actual.eq_ignore_ascii_case(expected);
                    if hash_conflict && !options.force_restore_hash {
                        return Err(err("restore image hash does not match metadata"));
                    }
                    if hash_conflict || size_conflict {
                        // 记录"用户看过并接受的就是这份内容"。离线侧据此核验镜像
                        // 未再变化，旧副档只留痕。
                        acceptance = Some(ImageAcceptance {
                            accepted_sha256: actual,
                            accepted_size_bytes: file_size,
                            index: options.wim_index,
                            superseded_sidecar_sha256: Some(expected.clone()),
                            accepted_at: Utc::now(),
                        });
                    }
                } else if size_conflict {
                    // 仅对比大小模式：用户接受的是"当前这个大小"，不为此多读一次全盘哈希。
                    // 留空哈希表示本次例外不覆盖哈希判据，离线侧也只走大小校验。
                    acceptance = Some(ImageAcceptance {
                        accepted_sha256: String::new(),
                        accepted_size_bytes: file_size,
                        index: options.wim_index,
                        superseded_sidecar_sha256: Some(metadata.image_sha256.clone()),
                        accepted_at: Utc::now(),
                    });
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
            Ok(ValidationOutcome {
                acceptance,
                overlay_no_acl,
            })
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
    // 恢复环境失败处理策略 + 界面语言（2026-10-16 用户拍板做成设置项）。
    // 键名与取值都来自 `crate::failure_policy`，恢复环境读同一份常量。
    for (key, value) in options.failure_policy.env_pairs() {
        put_value(&mut values, key, value);
    }
    // DISM 等待上限（2026-10-16 审查补上）。只在非默认时写一行，避免给每个
    // 任务都多一行没有信息量的 0。
    if let Some((key, value)) = crate::dism_timeout::env_pair(options.dism_timeout_secs) {
        put_value(&mut values, key, value);
    }
    put_value(&mut values, "UI_LANGUAGE", options.ui_language.clone());
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
        // `reagentc /info` 有一个上下文级失败模式（退出码 9，DISM 写不了
        // `C:\Windows\Logs\DISM` 日志），**不代表 WinRE 真不可用**。所以：
        // /info 成功 → 按原判据（须报 GLOBALROOT 或 Recovery\WindowsRE）；
        // /info 失败 → 退回 BCD 能否推出 WinRE 卷来判"是否已注册"，判不出才拒绝。
        let winre_available = match capture("reagentc.exe", &["/info"]) {
            Ok(reagent) => reagent.contains("GLOBALROOT") || reagent.contains("Recovery\\WindowsRE"),
            Err(_) => recovery_identity_from_bcd().is_ok(),
        };
        if !winre_available {
            return Err(err("Windows RE is disabled or unavailable"));
        }
    }
    Ok(())
}

/// 【2026-10-17 第二轮核查，日志缺口 6】`fltmc` 的输出落盘。
///
/// 此前三个流全是 `Stdio::null()`，只按退出码判断 —— 提权失败时用户看到
/// 「需要管理员权限」，而日志里**看不出到底是没提权、还是 fltmc 本身不可用**
/// （后者在某些精简环境里会出现，此时真正的修法完全不同）。
fn require_administrator(log: &Path) -> Result<(), TaskError> {
    // 【2026-10-18 缺陷修复 M10】先跑真正的权限判据（`fltmc` 的退出码），
    // 日志写入单独容错。此前用 `capture_logged`：它第一行就是
    // `append_log(log, …)?`，一旦日志不可写（目录满 / ACL 限制 / `logs_dir`
    // 回退到不可写目录），错误会在 `fltmc` **之前**就返回，被这里一律翻成
    // 「administrator elevation is required」——用户反复提权仍然失败，
    // 真正的原因（日志写不了）被彻底带偏。
    // 用不写日志的 `capture`（非零退出即 Err），把「提权不够」与「日志写不了」
    // 两种失败分开：只有前者才报「需要管理员」。
    match capture("fltmc.exe", &[]) {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = append_log(log, &format!("administrator check failed: {error}"));
            Err(err("administrator elevation is required"))
        }
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

/// 备份前镜像卷空间预检：镜像所在卷可用空间低于硬闸 → 带码拒绝。
///
/// 与在线通道（`online_operation::capture`）用的是同一对 core 纯函数，保证两条
/// 链阈值一致。读不到卷空间（卷不存在/权限）时**静默放过**：预检只是提前止损，
/// 不是权威判据，真正的权威是捕获过程本身与它的回滚。
fn ensure_backup_image_capacity(
    source_letter: char,
    image_path: &str,
    compress: &str,
) -> Result<(), TaskError> {
    let image_letter = match Path::new(image_path)
        .parent()
        .and_then(|parent| parent.to_string_lossy().chars().next())
        .filter(|c| c.is_ascii_uppercase())
    {
        Some(letter) => letter,
        // 非盘符路径（UNC 等）：无法判定卷，放过。
        None => return Ok(()),
    };
    let (source_total, source_free, available) = match (
        disk_free_space(source_letter),
        volume_free_bytes(image_letter),
    ) {
        (Ok((total, free)), Ok(available)) => (total, free, available),
        _ => return Ok(()),
    };
    let used = source_total.saturating_sub(source_free);
    let floor = backuprestore_core::backup_space_floor(used);
    if available < floor {
        return Err(crate::errc_with(
            ErrCode::BackupSpaceInsufficient,
            &[
                &crate::text_parsing::format_bytes(Some(available)),
                &crate::text_parsing::format_bytes(Some(floor)),
                &crate::text_parsing::format_bytes(Some(used)),
            ],
        ));
    }
    // 低于保守估计只做记录不做拦截（估计值本身对含大量媒体的数据卷可能偏低）。
    let estimated = backuprestore_core::estimate_backup_bytes(used, compress);
    if available < estimated {
        let dir = crate::log_root::logs_dir();
        {
            let _ = append_log(
                &dir.join("prepare.log"),
                &crate::i18n::msg_tpl(
                    "ol_backup_space_warn",
                    &[
                        &crate::text_parsing::format_bytes(Some(available)),
                        &crate::text_parsing::format_bytes(Some(estimated)),
                        &crate::text_parsing::format_bytes(Some(used)),
                    ],
                ),
            );
        }
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

/// 当前**活动系统卷**（`%SystemDrive%`）的卷身份。
///
/// ⚠️ **只能在联机 Windows 下调用。** WinRE 里 `%SystemDrive%` 是恢复环境自己的
/// RAM 盘 `X:`（见 `main.rs` 对 X: 的说明），取到的身份是 RAM 盘而不是用户的系统卷。
/// 离线侧要判「目标是不是用户的活动系统」只能读 prepare 阶段写进任务的
/// `TargetSpec::target_is_active_system`，不要在 RE 内重新探测。
///
/// 按**卷身份**（disk/partition GUID）比对，不要用盘符字面量：盘符不是分区的物理
/// 属性，重启和 RE 内都会重排，系统盘也不一定是 `C:`。
pub(crate) fn active_system_volume_identity() -> Result<VolumeIdentity, TaskError> {
    let value = env::var("SystemDrive").map_err(|_| crate::errc(ErrCode::CurrentSystemVolumeUnknown))?;
    let letter = value
        .trim()
        .chars()
        .next()
        .ok_or_else(|| crate::errc(ErrCode::CurrentSystemVolumeUnknown))?
        .to_ascii_uppercase();
    volume_identity(letter)
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

/// 把 BCD 里的设备路径（`\Device\HarddiskVolume6` 或 `\\.\HarddiskVolume6`）
/// 解析成「磁盘号 + 分区号」。
///
/// 【2026-10-19 D4】独立恢复分区通常没有盘符，`reagentc` 写进 BCD 的恢复对象
/// 就是这种设备路径形态。要把「恢复对象指向哪个分区」这件事真正核验出来，
/// 只能走 `IOCTL_STORAGE_GET_DEVICE_NUMBER` + `IOCTL_DISK_GET_PARTITION_INFO_EX`，
/// 不能靠字符串比盘符（没有盘符可比）。
pub(crate) fn partition_of_device_path(device: &str) -> Result<(u32, u32), TaskError> {
    let name = crate::text_parsing::device_path_volume_name(device)
        .ok_or_else(|| crate::errc_with(ErrCode::DevicePathUnrecognized, &[device]))?;
    let path = format!(r"\\.\{name}");
    let disk = storage_device_number_at(&path)?;
    let physical = physical_volume_identity_at(&path, disk)?;
    Ok((disk, physical.partition_number))
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
    // 首选 `reagentc /info`。但它有一个**上下文级**的已知失败模式：退出码 9
    // （`WinReGetWimVersion failed to initialize DISM`），根因是 `C:\Windows\Logs\DISM`
    // 目录的 ACL 不给 Administrators 写权限、DISM 写不了自己的日志（2026-10-16
    // 实测钉死，见 `docs/20261016-1413` §1）。该失败**不代表 WinRE 真的坏了**，
    // 却会让整个准备链在 314ms 内 hard-fail（用户 2026-10-16 裁定：这种情况记警告
    // 继续，别在这里把流程掐死——真有问题是后面的 WIM 挂载/注入会自己报错）。
    //
    // 因此这里改为两级：
    //   ①`reagentc /info` 成功 → 按报告的分区身份继续（原逻辑）。
    //   ②失败 → 退回从**BCD 的 `recoverysequence` → ramdisk 卷路径**推导 WinRE 所在卷。
    let output = match capture("reagentc.exe", &["/info"]) {
        Ok(output) => output,
        Err(info_error) => {
            return recovery_identity_from_bcd()
                .map_err(|bcd_error| {
                    crate::errc_with(
                        ErrCode::WinreIdentityProbeFailed,
                        &[&info_error.to_string(), &bcd_error.to_string()],
                    )
                });
        }
    };
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

/// `reagentc /info` 失败时的兜底：从 BCD 里已注册的 WinRE 启动对象反推其所处卷。
///
/// 路径：`bcdedit /enum {bootmgr}` 找到当前系统加载器 → 其 `recoverysequence`
/// 指向的恢复对象 → 该对象的 `device`/`osdevice` 是
/// `ramdisk=[X:]\Recovery\WindowsRE\Winre.wim,{设备选项}` → 取出 `[X:]` 卷 → 用
/// `volume_identity` 拿卷身份（盘符在这里只是**读**的入口，identity 里绝不记它）。
///
/// 只在 `/info` 失败时调用，属**尽力而为**：任何一步取不到就如实报错，
/// 不制造假身份（`recovery` 身份下游还用于定位 boot.sdi 与写 RECOVERY 角色，
/// 给错比给不出更危险）。
fn recovery_identity_from_bcd() -> Result<VolumeIdentity, TaskError> {
    // 当前系统加载器 = `{bootmgr}.default`；`/v` 才输出真实 GUID。
    let bootmgr = capture(
        "bcdedit.exe",
        &["/enum", "{bootmgr}", "/v"],
    )?;
    let default_loader = extract_after(&bootmgr.to_ascii_lowercase(), "default")
        .and_then(|value| value.lines().next())
        .and_then(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .ok_or_else(|| crate::errc(ErrCode::BcdNoDefaultLoader))?;
    // 该加载器的 recoverysequence（WinRE 恢复对象的 GUID）。
    let os_block = capture("bcdedit.exe", &["/enum", &default_loader, "/v"])?;
    let recovery_guid = extract_after(&os_block.to_ascii_lowercase(), "recoverysequence")
        .and_then(|value| value.lines().next())
        .and_then(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .ok_or_else(|| crate::errc(ErrCode::BcdNoRecoverySequence))?;
    // 恢复对象的 device：`ramdisk=[X:]\Recovery\...\Winre.wim,{…}`。
    let recovery_block = capture("bcdedit.exe", &["/enum", &recovery_guid, "/v"])?;
    let device = extract_after(&recovery_block, "device")
        .and_then(|value| value.lines().next())
        .map(|line| line.trim().to_ascii_lowercase())
        .ok_or_else(|| crate::errc(ErrCode::BcdRecoveryNoDevice))?;
    // 从 `ramdisk=[x:]...` 里取卷标识（单字母盘符）。
    let letter = device
        .split_once("ramdisk=[")
        .and_then(|(_, rest)| rest.split_once(':'))
        .and_then(|(letter, _)| letter.chars().next())
        .filter(|c| c.is_ascii_alphanumeric())
        .ok_or_else(|| crate::errc(ErrCode::BcdRecoveryRamdiskUnrecognized))?;
    volume_identity(letter.to_ascii_uppercase())
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
    // `mountvol <letter>: <name>` 要求卷名为 `\\?\Volume{GUID}\` 形式。
    // 但 `VolumeIdentity.volume_guid` 在不同来源下格式不一：从盘符读回的是
    // `\\?\Volume{GUID}\`，而 `esp_identity_without_drive_letter` 走 verbatim
    // 路径得到的是裸 `{GUID}`。后者直接喂给 mountvol 会被当成非法名称而失败，
    // 进而落到 diskpart 兜底、在首选盘符被占用时彻底报错（见 2026-10-12 在线还原
    // 首跑失败）。这里统一规整为 mountvol 期望的形式，保证 GUID 挂载路径可用。
    let upper = trimmed.to_uppercase();
    let mount_arg = if upper.starts_with("\\\\?\\VOLUME{") {
        format!("{trimmed}\\")
    } else if trimmed.starts_with('{') {
        format!("\\\\?\\Volume{trimmed}\\")
    } else {
        format!("{trimmed}\\")
    };
    let mount_target = mount_arg.trim_end_matches('\\').to_string();
    let mut last_error = None;
    for letter in identity_drive_candidates(preferred) {
        if !is_drive_letter_available(letter) {
            continue;
        }
        let output = match Command::new("mountvol.exe")
            .args([format!("{letter}:"), mount_arg.clone()])
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
                    .eq_ignore_ascii_case(&mount_target) =>
            {
                return Ok(letter);
            }
            Ok(actual) => {
                last_error = Some(err(&format!(
                    "mountvol assigned {letter}: to {}, expected {mount_target}",
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `/Mount-Wim` 只认 `/WimFile`；写成 `/ImageFile`（那是 `/Apply-Image` 的选项）
    /// DISM 恒报 87。这条测试锁死参数名，防止「权威门禁静默失效」复发——
    /// 该缺陷自 2.4.2 起存在了近两个月，期间每次探测都静默降级成镜像名启发式，
    /// 直到 2026-10-20 逐字核对 DISM 参数才被发现（docs/20261020-1025）。
    #[test]
    fn wim_mount_args_use_wimfile_not_imagefile() {
        let args = wim_mount_args(r"D:\images\win.wim", 2, Path::new(r"C:\temp\mount"));
        assert_eq!(args.first().map(String::as_str), Some("/Mount-Wim"));
        assert!(
            args.iter().any(|a| a == r"/WimFile:D:\images\win.wim"),
            "缺少 /WimFile 参数：{args:?}"
        );
        assert!(
            !args.iter().any(|a| a.starts_with("/ImageFile:")),
            "/Mount-Wim 不接受 /ImageFile：{args:?}"
        );
        assert!(args.iter().any(|a| a == "/Index:2"), "{args:?}");
        assert!(args.iter().any(|a| a == "/ReadOnly"), "{args:?}");
        assert!(
            args.iter().any(|a| a == r"/MountDir:C:\temp\mount"),
            "{args:?}"
        );
    }

    /// 快速通道判据（v2.6.23）：常规备份名必须放行，任何 PE/短名/空名必须**落回挂载**。
    /// 这条判据宁可多挂一次（慢）也不能放行可疑镜像，所以负例比正例重要。
    #[test]
    fn name_looks_like_full_windows_gate() {
        // 正例：本程序默认索引名 + 常见系统镜像名
        for ok in [
            "Windows 11 26200.9550 20261015",
            "Windows Backup",
            "Windows 10 Enterprise",
            "Windows Server 2022",
            "Windows 11 Pro",
        ] {
            assert!(
                name_looks_like_full_windows(ok),
                "应当走快速通道：{ok}"
            );
        }
        // 负例：PE 特征、恢复环境、太短、空、不含 windows
        for bad in [
            "Microsoft Windows PE (x64)",
            "WinPE",
            "Windows Recovery Environment",
            "Windows Preinstallation Environment",
            "Backup",
            "",
            "MyData",
        ] {
            assert!(
                !name_looks_like_full_windows(bad),
                "应当落回挂载探测：{bad:?}"
            );
        }
    }
}
