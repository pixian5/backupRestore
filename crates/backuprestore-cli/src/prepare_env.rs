// Windows 专用模块——非 Windows 编译时仅作类型检查，调用方被 cfg 门控。
#![cfg_attr(not(windows), allow(dead_code))]
//! 准备期环境配置与状态序列化器（Prepare Environment & Status Generator）
//!
//! 负责：
//! - 生成写入 WinRE 离线恢复环境所需的 `RecoveryTask.env` 环境变量；
//! - 卷身份（`VolumeIdentity`）与关键几何/序列号序列化为环境变量键值对；
//! - 准备期状态文件 `status.env` 写入与阶段流转持久化；
//! - 备份与还原镜像路径的归一化与归属卷探测（`image_path_info`）。

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;

use backuprestore_core::{
    validate_absolute_path, Operation, Task, TaskError, VolumeIdentity,
};
use crate::prepare_plan::{parse_drive, PrepareOptions};
use crate::volume_device::{volume_free_bytes, volume_identity};
use crate::windows_prepare::{workspace_relative, RESERVED_BYTES};
use crate::err;

pub(crate) fn image_path_info(
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

pub(crate) fn write_recovery_env(
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
    // 恢复环境失败处理策略 + 界面语言。
    for (key, value) in options.failure_policy.env_pairs() {
        put_value(&mut values, key, value);
    }
    // DISM 等待上限。
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

pub(crate) fn append_env(path: &Path, values: &[(&str, &str)]) -> Result<(), TaskError> {
    let mut file = OpenOptions::new().append(true).open(path)?;
    for (key, value) in values {
        writeln!(file, "{key}={value}")?;
    }
    file.sync_all()?;
    Ok(())
}

pub(crate) fn write_status_env(task_dir: &Path, task: &Task, stage: &str) -> Result<(), TaskError> {
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

pub(crate) fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::Probe => "probe",
        Operation::Backup => "backup",
        Operation::RestoreExisting => "restore-existing",
        Operation::CreateSecondary => "create-secondary",
    }
}
