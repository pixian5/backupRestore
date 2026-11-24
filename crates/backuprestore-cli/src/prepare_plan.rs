// 准备阶段纯规划与纯文本解析（跨平台逻辑）。
//
// 本模块拥有 PrepareOptions 解析、DISM 输出解析、镜像安全与命名门禁等纯计算逻辑；
// 完全不依赖任何 Win32 FFI 或 Windows 系统调用，在 macOS / Linux 测试目标下可直接运行单元测试。
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use backuprestore_core::{canonical_compression, Operation, TaskError, VolumeIdentity};
use serde_json::{json, Value};
use std::path::Path;
use crate::err;

#[derive(Debug, Clone)]
pub(crate) struct PrepareOptions {
    pub(crate) operation: Operation,
    pub(crate) source_drive: char,
    /// Only restore operations need a target. Keeping it optional prevents
    /// hidden probe/backup UI fields from accidentally becoming destructive
    /// validation inputs.
    pub(crate) target_drive: Option<char>,
    pub(crate) image_path: Option<String>,
    pub(crate) wim_index: u32,
    /// WIM 索引名（备份时 DISM /Name；默认 "Windows Backup"）。
    pub(crate) image_name: Option<String>,
    /// 保留最近 N 个 WIM 索引（备份追加成功后清理更旧索引；None=不清理）。
    pub(crate) keep_indexes: Option<u32>,
    pub(crate) boot_menu_name: String,
    pub(crate) efi_drive: Option<char>,
    pub(crate) test_fault: Option<String>,
    pub(crate) allow_destructive: bool,
    pub(crate) no_reboot: bool,
    /// WIM 压缩率：fast/none，仅备份首次创建时生效。
    pub(crate) compress: Option<String>,
    /// 还原时跳过镜像哈希校验（GUI 已向用户确认档案缺失/不匹配仍继续）。
    pub(crate) force_restore_hash: bool,
    /// 还原时严格校验镜像哈希（GUI 勾选了对比哈希，默认未勾选仅对比大小）。
    pub(crate) verify_hash: bool,
    /// 还原前是否格式化目标卷（GUI「格式化目标分区」复选框，默认 true）。
    /// false = 覆盖式应用（不格式化，卷上镜像没有的文件保留），落盘进
    /// TargetSpec 供 RE 恢复链读取。
    pub(crate) format_target: bool,
    /// 恢复环境「连续失败 N 次后如何处理」策略（次数 + 行为）。
    pub(crate) failure_policy: crate::failure_policy::FailurePolicy,
    /// DISM 等待上限（秒；0 = 不限制 = 历史行为）。
    pub(crate) dism_timeout_secs: u32,
    /// 创建任务时的界面语言（`zh` / `en`）。
    pub(crate) ui_language: String,
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
                let is_known = matches!(
                    fault.as_str(),
                    "identity-env-mismatch"
                        | "bcdboot-failure"
                        | "power-loss-window"
                        | "power-loss-target-erased"
                        | "power-loss-image-applied"
                        | "power-loss-boot-repaired"
                );
                #[cfg(windows)]
                let is_valid = is_known || crate::recovery_fault::valid(&fault);
                #[cfg(not(windows))]
                let is_valid = is_known;

                if !is_valid {
                    return Err(err(
                        "--test-fault must be identity-env-mismatch, bcdboot-failure, power-loss-window, power-loss-target-erased, power-loss-image-applied, or power-loss-boot-repaired",
                    ));
                }
                test_fault = Some(fault);
            }
            "--allow-destructive" => allow_destructive = true,
            "--no-reboot" => no_reboot = true,
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
            "--json" => {}
            "--skip-format" => format_target = false,
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
                    raw.trim()
                        .parse::<u32>()
                        .map_err(|_| err("--dism-timeout-secs must be a non-negative integer"))?,
                );
            }
            "--ui-language" => {
                let raw = value("--ui-language", &mut args)?;
                let lower = raw.trim().to_ascii_lowercase();
                if lower == "zh" || lower == "en" {
                    ui_language = Some(lower);
                }
            }
            other => return Err(err(&format!("unknown prepare option: {other}"))),
        }
    }
    let operation = operation.ok_or_else(|| err("--operation is required"))?;
    let source_drive = source_drive.ok_or_else(|| err("--source-drive is required"))?;
    let (target_drive, image_path) = match operation {
        Operation::Probe => (target_drive, image_path),
        Operation::Backup => {
            let image = image_path.ok_or_else(|| err("--image-path is required for backup"))?;
            (None, Some(image))
        }
        Operation::RestoreExisting => {
            let image = image_path.ok_or_else(|| err("--image-path is required for restore"))?;
            let target = target_drive.unwrap_or(source_drive);
            (Some(target), Some(image))
        }
        Operation::CreateSecondary => {
            let image =
                image_path.ok_or_else(|| err("--image-path is required for create-secondary"))?;
            let target = target_drive
                .ok_or_else(|| err("--target-drive is required for create-secondary"))?;
            (Some(target), Some(image))
        }
    };
    if let Some(fault) = test_fault.as_deref()
        && matches!(fault, "identity-env-mismatch" | "bcdboot-failure")
        && efi_drive.is_none()
    {
        return Err(err(&format!(
            "--test-fault {fault} requires --test-efi-drive"
        )));
    }

    let mut failure_policy = crate::failure_policy::FailurePolicy::default();
    if let Some(threshold) = fail_threshold {
        failure_policy.threshold = threshold;
    }
    if let Some(action) = fail_action {
        failure_policy.action = action;
    }
    if let Some(secs) = fail_pause_max_wait {
        failure_policy.pause_max_wait_secs = secs;
    }

    let dism_timeout_secs = dism_timeout_secs.unwrap_or(0);
    let ui_language = ui_language.unwrap_or_else(|| "zh".to_string());

    Ok(PrepareOptions {
        operation,
        source_drive,
        target_drive,
        image_path,
        wim_index,
        image_name,
        keep_indexes,
        boot_menu_name,
        efi_drive,
        test_fault,
        allow_destructive,
        no_reboot,
        compress,
        force_restore_hash,
        verify_hash,
        format_target,
        failure_policy,
        dism_timeout_secs,
        ui_language,
    })
}

pub(crate) fn parse_drive(value: &str) -> Result<char, TaskError> {
    let normalized = value.trim().trim_end_matches(':');
    if normalized.len() == 1 && normalized.as_bytes()[0].is_ascii_alphabetic() {
        Ok((normalized.as_bytes()[0] as char).to_ascii_uppercase())
    } else {
        Err(err("drive must be a single letter"))
    }
}

pub(crate) fn restore_workspace_target_error(workspace_drive: char) -> TaskError {
    err(&format!(
        "Cannot start restore: the program directory is on {workspace_drive}:, which is the restore target. Move the entire BackupRestore folder to another volume and run it again. No task, WinRE, BCD or reboot was requested."
    ))
}

pub(crate) fn restore_reserved_target_error() -> TaskError {
    err("EFI/MSR/Recovery partitions cannot be restore targets")
}

pub(crate) fn wim_mount_args(image_path: &str, index: u32, mount_dir: &Path) -> Vec<String> {
    vec![
        "/Mount-Wim".to_string(),
        format!("/WimFile:{image_path}"),
        format!("/Index:{index}"),
        "/ReadOnly".to_string(),
        format!("/MountDir:{}", mount_dir.display()),
    ]
}

pub(crate) fn name_looks_like_full_windows(name: &str) -> bool {
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

/// 「还原到原卷」的离线任务目标不是当前系统卷时，对镜像性质的判定结果
/// （2026-11-24 缺陷 1 修复）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OfflineTargetImageNature {
    /// 数据镜像：挂载后确认没有完整 Windows 的两个标记。可以还原到非系统卷。
    Data,
    /// 完整 Windows 镜像：不能走「还原到原卷」这条离线路径灌进非系统卷。
    Windows,
    /// 判定不了（挂载探测失败，副档也指认不了）。调用方按拒绝处理，并把原因带给用户。
    Unknown(String),
}

/// 判定「要把镜像离线写入一个非系统卷」时，这份镜像是不是完整 Windows。
///
/// # 为什么要这个函数（2026-11-24 缺陷 1）
///
/// 10-18 加的安全闸规定「还原到原卷的目标必须是当前系统卷」，防的是 A 入口的目标下拉
/// 静默失效后「系统镜像被灌进数据盘」。但它拦得太宽，把合法的「数据镜像 → 数据卷」
/// （界面「还原数据到非 C 盘」的离线方式）也一并封死了。这里把两种情况分开：
/// **看镜像是什么，而不是看目标卷是什么**——两种情况下目标都是数据卷。
///
/// # 两条判据，为什么是这两条
///
/// - **副档指认**：产品自己备份时会把源卷身份写进副档。源卷就是当前系统卷的镜像，
///   一定是完整 Windows。它让最常见的误选（拿系统盘的备份去还原数据盘）不必挂载就被拦下。
///   它只负责「认定是 Windows」，认定不了就交给挂载探测。
/// - **挂载探测**：只认挂载之后看到的结果（`reagentc.exe` 与 `SYSTEM` 两个标记）。
///   **不看镜像的索引名**——在线备份写进镜像的默认索引名就是 `Windows Backup`
///   （`online_operation.rs` 的 `capture`），数据镜像照样「像」；上面那个
///   `name_looks_like_full_windows` 的快速通道在这里不能用。
///
/// 判定不了时返回 [`OfflineTargetImageNature::Unknown`]，由调用方保守拒绝：
/// 错放一份 Windows 镜像进数据卷，比让用户换一台能挂载的机器更贵。
pub(crate) fn classify_offline_target_image(
    sidecar_source: Option<&VolumeIdentity>,
    active_system: &VolumeIdentity,
    mount_markers: Result<bool, String>,
) -> OfflineTargetImageNature {
    // 比的是分区身份（磁盘 GUID + 分区 GUID），不比序列号：系统盘还原过之后序列号会变，
    // 而分区身份不变。
    if let Some(source) = sidecar_source
        && source.same_partition(active_system)
    {
        return OfflineTargetImageNature::Windows;
    }
    match mount_markers {
        Ok(true) => OfflineTargetImageNature::Windows,
        Ok(false) => OfflineTargetImageNature::Data,
        Err(reason) => OfflineTargetImageNature::Unknown(reason),
    }
}

pub(crate) fn windows_version_summary(output: &str) -> String {
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

pub(crate) fn clean_sidecar_name(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|name| !name.is_empty())
}

pub(crate) fn sidecar_image_name(
    recorded: Option<&str>,
    index: u32,
    dism_name: Option<&str>,
) -> String {
    if let Some(name) = clean_sidecar_name(recorded) {
        return name.to_string();
    }
    if let Some(name) = clean_sidecar_name(dism_name) {
        return name.to_string();
    }
    format!("Windows Backup (index {index})")
}

pub(crate) fn parse_dism_size(value: &str) -> Option<u64> {
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

#[allow(dead_code)]
pub(crate) fn validate_prepare_options_safety(
    options: &PrepareOptions,
    workspace_drive: Option<char>,
) -> Result<(), TaskError> {
    if let (Some(target), Some(workspace)) = (options.target_drive, workspace_drive)
        && target == workspace
        && matches!(options.operation, Operation::RestoreExisting | Operation::CreateSecondary)
    {
        return Err(restore_workspace_target_error(workspace));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 离线还原到非系统卷：镜像性质判定（2026-11-24 缺陷 1 修复）──────────

    fn volume(disk: &str, partition: &str) -> VolumeIdentity {
        VolumeIdentity::new(disk, partition)
    }

    #[test]
    fn offline_target_image_follows_the_mounted_probe() {
        let active = volume("{disk-0}", "{part-c}");
        let other = volume("{disk-0}", "{part-f}");
        // 挂载后确认缺标记 → 数据镜像（放行）；有没有副档都一样
        assert_eq!(
            classify_offline_target_image(Some(&other), &active, Ok(false)),
            OfflineTargetImageNature::Data
        );
        assert_eq!(
            classify_offline_target_image(None, &active, Ok(false)),
            OfflineTargetImageNature::Data
        );
        // 挂载后标记齐全 → 完整 Windows（拒绝）；有没有副档都一样
        assert_eq!(
            classify_offline_target_image(Some(&other), &active, Ok(true)),
            OfflineTargetImageNature::Windows
        );
        assert_eq!(
            classify_offline_target_image(None, &active, Ok(true)),
            OfflineTargetImageNature::Windows
        );
    }

    #[test]
    fn offline_target_image_sidecar_of_the_system_volume_is_windows_without_mounting() {
        let active = volume("{disk-0}", "{part-c}");
        // 副档的源卷就是当前系统卷：不用挂载就认定是完整 Windows，
        // 挂载失败、甚至挂载说「缺标记」都不改判（副档是产品自己写的事实）。
        assert_eq!(
            classify_offline_target_image(Some(&active), &active, Err("mount failed".into())),
            OfflineTargetImageNature::Windows
        );
        assert_eq!(
            classify_offline_target_image(Some(&active), &active, Ok(false)),
            OfflineTargetImageNature::Windows
        );
    }

    #[test]
    fn offline_target_image_is_unknown_when_the_probe_fails() {
        let active = volume("{disk-0}", "{part-c}");
        let other = volume("{disk-2}", "{part-f}");
        // 挂载失败、副档也指认不了：判定不了，原因原样带出去给调用方拒绝时展示
        assert_eq!(
            classify_offline_target_image(Some(&other), &active, Err("exit code 5".into())),
            OfflineTargetImageNature::Unknown("exit code 5".into())
        );
        assert_eq!(
            classify_offline_target_image(None, &active, Err("cannot mount".into())),
            OfflineTargetImageNature::Unknown("cannot mount".into())
        );
    }

    #[test]
    fn offline_target_image_compares_partition_identity_case_insensitively() {
        // 磁盘 GUID 与分区 GUID 的大小写不应当影响「是不是同一个分区」
        let active = volume("{DISK-0}", "{PART-C}");
        let same_partition_other_case = volume("{disk-0}", "{part-c}");
        assert_eq!(
            classify_offline_target_image(Some(&same_partition_other_case), &active, Ok(false)),
            OfflineTargetImageNature::Windows
        );
    }

    #[test]
    fn parse_drive_handles_various_formats() {
        assert_eq!(parse_drive("c").unwrap(), 'C');
        assert_eq!(parse_drive("C:").unwrap(), 'C');
        assert_eq!(parse_drive("  d:  ").unwrap(), 'D');
        assert_eq!(parse_drive("Z").unwrap(), 'Z');

        assert!(parse_drive("").is_err());
        assert!(parse_drive("CC").is_err());
        assert!(parse_drive("1").is_err());
        assert!(parse_drive(":").is_err());
    }

    #[test]
    fn parse_prepare_options_probe_mode() {
        let opts = parse_prepare_options(vec![
            "--operation".into(),
            "probe".into(),
            "--source-drive".into(),
            "C".into(),
        ])
        .unwrap();
        assert_eq!(opts.operation, Operation::Probe);
        assert_eq!(opts.source_drive, 'C');
        assert_eq!(opts.target_drive, None);
        assert_eq!(opts.wim_index, 1);
        assert!(opts.format_target);
    }

    #[test]
    fn parse_prepare_options_backup_mode() {
        let opts = parse_prepare_options(vec![
            "--operation".into(),
            "backup".into(),
            "--source-drive".into(),
            "C:".into(),
            "--image-path".into(),
            r"D:\backup.wim".into(),
            "--compress".into(),
            "fast".into(),
            "--wim-index".into(),
            "2".into(),
        ])
        .unwrap();
        assert_eq!(opts.operation, Operation::Backup);
        assert_eq!(opts.source_drive, 'C');
        assert_eq!(opts.image_path.as_deref(), Some(r"D:\backup.wim"));
        assert_eq!(opts.compress.as_deref(), Some("fast"));
        assert_eq!(opts.wim_index, 2);
    }

    #[test]
    fn parse_prepare_options_restore_existing_defaults_target_to_source() {
        let opts = parse_prepare_options(vec![
            "--operation".into(),
            "restore-existing".into(),
            "--source-drive".into(),
            "C".into(),
            "--image-path".into(),
            r"D:\backup.wim".into(),
            "--skip-format".into(),
        ])
        .unwrap();
        assert_eq!(opts.operation, Operation::RestoreExisting);
        assert_eq!(opts.source_drive, 'C');
        assert_eq!(opts.target_drive, Some('C'));
        assert!(!opts.format_target);
    }

    #[test]
    fn parse_prepare_options_create_secondary_requires_target() {
        let res = parse_prepare_options(vec![
            "--operation".into(),
            "create-secondary".into(),
            "--source-drive".into(),
            "C".into(),
            "--image-path".into(),
            r"D:\backup.wim".into(),
        ]);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("--target-drive is required"));

        let ok = parse_prepare_options(vec![
            "--operation".into(),
            "create-secondary".into(),
            "--source-drive".into(),
            "C".into(),
            "--target-drive".into(),
            "F".into(),
            "--image-path".into(),
            r"D:\backup.wim".into(),
            "--boot-menu-name".into(),
            "Second Windows".into(),
        ])
        .unwrap();
        assert_eq!(opts_target(&ok), Some('F'));
        assert_eq!(ok.boot_menu_name, "Second Windows");
    }

    fn opts_target(opts: &PrepareOptions) -> Option<char> {
        opts.target_drive
    }

    #[test]
    fn parse_prepare_options_obsolete_scratch_switch_rejected() {
        let err = parse_prepare_options(vec![
            "--operation".into(),
            "backup".into(),
            "--source-drive".into(),
            "C".into(),
            "--image-path".into(),
            r"D:\x.wim".into(),
            "--re-scratch-drive".into(),
            "E".into(),
        ])
        .unwrap_err();
        assert!(err.to_string().contains("--re-scratch-drive is obsolete"));
    }

    #[test]
    fn parse_prepare_options_fail_policy_parameters() {
        let opts = parse_prepare_options(vec![
            "--operation".into(),
            "probe".into(),
            "--source-drive".into(),
            "C".into(),
            "--fail-threshold".into(),
            "5".into(),
            "--fail-action".into(),
            "pause".into(),
            "--fail-pause-max-wait-secs".into(),
            "600".into(),
            "--dism-timeout-secs".into(),
            "1800".into(),
            "--ui-language".into(),
            "en".into(),
        ])
        .unwrap();
        assert_eq!(opts.failure_policy.threshold, 5);
        assert_eq!(opts.failure_policy.action, crate::failure_policy::FailureAction::PauseInRecovery);
        assert_eq!(opts.failure_policy.pause_max_wait_secs, 600);
        assert_eq!(opts.dism_timeout_secs, 1800);
        assert_eq!(opts.ui_language, "en");
    }

    #[test]
    fn validate_prepare_safety_blocks_workspace_overlap() {
        let opts = parse_prepare_options(vec![
            "--operation".into(),
            "restore-existing".into(),
            "--source-drive".into(),
            "C".into(),
            "--image-path".into(),
            r"D:\x.wim".into(),
        ])
        .unwrap();
        assert!(validate_prepare_options_safety(&opts, Some('C')).is_err());
        assert!(validate_prepare_options_safety(&opts, Some('D')).is_ok());
    }

    #[test]
    fn wim_mount_args_use_wimfile_not_imagefile() {
        let args = wim_mount_args(r"D:\images\win.wim", 2, Path::new(r"C:\temp\mount"));
        assert_eq!(args.first().map(String::as_str), Some("/Mount-Wim"));
        assert!(args.iter().any(|a| a == r"/WimFile:D:\images\win.wim"));
        assert!(!args.iter().any(|a| a.starts_with("/ImageFile:")));
        assert!(args.iter().any(|a| a == "/ReadOnly"));
        assert!(args.iter().any(|a| a == r"/MountDir:C:\temp\mount"));
    }

    #[test]
    fn name_looks_like_full_windows_gate() {
        for ok in [
            "Windows 11 26200.9550 20261015",
            "Windows Backup",
            "Windows 10 Enterprise",
            "Windows Server 2022",
            "Windows 11 Pro",
        ] {
            assert!(name_looks_like_full_windows(ok), "expected pass: {ok}");
        }
        for bad in [
            "WinPE",
            "Windows PE",
            "Windows Preinstallation Environment",
            "Recovery",
            "short",
            "",
            "Ubuntu 22.04",
        ] {
            assert!(!name_looks_like_full_windows(bad), "expected reject: {bad}");
        }
    }

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

    #[test]
    fn sidecar_image_name_fallback_chain() {
        assert_eq!(
            sidecar_image_name(Some("Windows 11 26200.9550 20261018"), 1, None),
            "Windows 11 26200.9550 20261018"
        );
        assert_eq!(sidecar_image_name(Some("  "), 1, Some("Real")), "Real");
        assert_eq!(sidecar_image_name(None, 2, Some("Real")), "Real");
        assert_eq!(sidecar_image_name(None, 3, None), "Windows Backup (index 3)");
        assert_eq!(sidecar_image_name(None, 3, Some(" ")), "Windows Backup (index 3)");
    }

    #[test]
    fn parse_dism_size_handles_units_and_commas() {
        assert_eq!(parse_dism_size("1,024 bytes"), Some(1024));
        assert_eq!(parse_dism_size("10.5 GB"), Some((10.5 * 1024.0 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_dism_size("1 GiB"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_dism_size("512 MiB"), Some(512 * 1024 * 1024));
        assert_eq!(parse_dism_size("invalid"), None);
    }

    #[test]
    fn parses_multiple_indexes_and_optional_metadata() {
        let output = r#"
Index : 1
Name : Windows 11 Pro
Description : Primary System
Size : 15,457,267 bytes
WIM Bootable : No
Architecture : arm64
Version : 10.0.26100.1
Edition : Professional
Installation Type : Client

Index : 2
Name : Windows PE
Size : 512 MB
"#;
        let images = parse_dism_images(output).unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0]["ImageIndex"], 1);
        assert_eq!(images[0]["ImageName"], "Windows 11 Pro");
        assert_eq!(images[0]["Architecture"], "arm64");
        assert_eq!(images[1]["ImageIndex"], 2);
        assert_eq!(images[1]["ImageName"], "Windows PE");
    }

    #[test]
    fn ignores_header_version_and_rejects_missing_indexes() {
        let output = "Version: 10.0.26100.1\nName: not an image\n";
        assert!(parse_dism_images(output).is_err());
    }

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
