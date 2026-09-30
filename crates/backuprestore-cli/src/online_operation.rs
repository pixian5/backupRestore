//! 在线入口的执行事务：只在捕获、保留策略、校验和副档落盘全部成功后返回成功。
use crate::windows_command::{self, INFINITE};
use backuprestore_core::{BackupMetadata, TaskError, VolumeIdentity, image_metadata as im};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct OnlineOpParams {
    pub operation: String,
    pub source_drive: String,
    pub target_drive: String,
    pub image_path: String,
    pub index: String,
    pub compress: String,
    pub image_name: String,
    pub keep_indexes: Option<u32>,
}
pub(crate) struct OnlineResult {
    pub success: bool,
    pub detail: String,
}
struct ProgressGuard(std::sync::Arc<crate::recovery_progress::ProgressShared>);
impl Drop for ProgressGuard {
    fn drop(&mut self) {
        crate::recovery_progress::request_close(&self.0);
    }
}
fn log(path: &Path, message: &str) -> Result<(), TaskError> {
    crate::append_log(path, message)
}

pub(crate) fn execute(params: &OnlineOpParams) -> OnlineResult {
    let dir = std::env::temp_dir().join(format!(
        "BackupRestore-online-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    if let Err(error) = fs::create_dir(&dir) {
        return OnlineResult {
            success: false,
            detail: format!("无法创建日志目录：{error}"),
        };
    }
    let path = dir.join("Online.log");
    let progress = ProgressGuard(crate::recovery_progress::spawn(
        path.clone(),
        Some(if params.operation == "backup" {
            "ONLINE_BACKUP"
        } else {
            "ONLINE_RESTORE"
        }),
    ));
    let result = log(&path, "STEP 1/4 校验参数和卷身份；在线操作不修改启动项")
        .and_then(|()| execute_inner(params, &path));
    let mut success = result.is_ok();
    let final_message = match result {
        Ok(()) => "操作完成：命令、回读和副档簿记均已通过".to_string(),
        Err(error) => format!("[ERROR] 在线操作未完成：{error}；不要将部分产物视为有效备份"),
    };
    let log_error = log(&path, &final_message).err();
    drop(progress);
    if log_error.is_some() {
        success = false;
    }
    // 弹窗只显示摘要，完整输出始终保留在本轮独立日志，避免共享临时文件互相覆盖。
    OnlineResult {
        success,
        detail: format!(
            "{final_message}\n日志：{}{}",
            path.display(),
            log_error
                .map(|e| format!("\n写最终日志失败：{e}"))
                .unwrap_or_default()
        ),
    }
}
fn drive(value: &str) -> Result<char, TaskError> {
    let raw = value.trim_end_matches(':');
    if raw.len() != 1 || !raw.as_bytes()[0].is_ascii_alphabetic() {
        return Err(crate::err("invalid drive letter"));
    }
    Ok(raw.chars().next().unwrap().to_ascii_uppercase())
}
fn dism(args: &[String], log_path: &Path) -> Result<(), TaskError> {
    log(
        log_path,
        &format!("DISM 开始（等待至退出，无固定超时）：{args:?}"),
    )?;
    let outcome = windows_command::run_program("dism.exe", args, Some(log_path), INFINITE);
    log(log_path, &format!("DISM 结果：{outcome}"))?;
    if !outcome.is_success() {
        return Err(crate::err(&format!("DISM failed: {outcome}")));
    }
    Ok(())
}
fn indexes(image: &Path, log_path: &Path) -> Result<Vec<u32>, TaskError> {
    crate::wim_indexes(image, log_path)
}
fn execute_inner(params: &OnlineOpParams, log_path: &Path) -> Result<(), TaskError> {
    backuprestore_core::validate_absolute_path(&params.image_path)?;
    let image = Path::new(&params.image_path);
    let affected = drive(if params.operation == "backup" {
        &params.source_drive
    } else {
        &params.target_drive
    })?;
    let active = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    if affected == drive(&active)? {
        return Err(crate::err(
            "active Windows volume must be processed offline",
        ));
    }
    let identity = crate::windows_prepare::volume_identity(affected)?;
    if !identity.is_complete() {
        return Err(crate::err("online volume identity is incomplete"));
    }
    let image_drive = image
        .to_string_lossy()
        .chars()
        .next()
        .ok_or_else(|| crate::err("missing image volume"))?;
    let image_volume = crate::windows_prepare::volume_identity(image_drive)?;
    if identity.same_partition(&image_volume) {
        return Err(crate::err(
            "image and affected volume must be on different partitions",
        ));
    }
    let root = PathBuf::from(format!("{affected}:\\"));
    log(log_path, &format!("卷身份：{identity:?}"))?;
    match params.operation.as_str() {
        "backup" => {
            let (total, free) = crate::windows_prepare::disk_free_space(affected)?;
            capture(
                params,
                &root,
                identity,
                total.saturating_sub(free),
                log_path,
            )
        }
        "restore-existing" => {
            let index: u32 = params
                .index
                .parse()
                .map_err(|_| crate::err("invalid restore index"))?;
            if !indexes(image, log_path)?.contains(&index) {
                return Err(crate::err("restore index missing from WIM"));
            }
            log(
                log_path,
                "STEP 2/4 还原镜像（在线覆盖，不格式化、不修复引导）",
            )?;
            dism(
                &[
                    "/English".into(),
                    "/Apply-Image".into(),
                    format!("/ImageFile:{}", image.display()),
                    format!("/Index:{index}"),
                    format!("/ApplyDir:{}", root.display()),
                    "/CheckIntegrity".into(),
                    "/Verify".into(),
                ],
                log_path,
            )?;
            log(
                log_path,
                "STEP 3/4 回读目标卷身份；DISM 已执行完整性和应用校验",
            )?;
            let after = crate::windows_prepare::volume_identity(affected)?;
            if !identity.same_partition(&after)
                || identity.partition_offset != after.partition_offset
                || identity.partition_size != after.partition_size
                || identity.volume_serial != after.volume_serial
            {
                return Err(crate::err("restore target identity changed during apply"));
            }
            // 数据卷并不包含 Windows 内核，不能用 ntoskrnl.exe 存在与否判断数据还原成功。
            log(
                log_path,
                "STEP 4/4 完成在线还原；未执行格式化或系统引导修复",
            )
        }
        _ => Err(crate::err("unsupported online operation")),
    }
}

/// 候选目录保留到事务完全成功；失败留下日志指定的副本，原镜像不受追加/清理失败影响。
fn capture(
    params: &OnlineOpParams,
    source: &Path,
    identity: VolumeIdentity,
    used: u64,
    log_path: &Path,
) -> Result<(), TaskError> {
    let destination = Path::new(&params.image_path);
    let parent = destination
        .parent()
        .ok_or_else(|| crate::err("missing image parent"))?;
    let workspace = parent.join(format!(
        ".br-online-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::create_dir(&workspace)?;
    log(
        log_path,
        &format!(
            "候选目录：{}；失败时保留，原文件不覆盖",
            workspace.display()
        ),
    )?;
    let candidate = workspace.join("candidate.wim");
    let existing = match fs::metadata(destination) {
        Ok(m) if m.is_file() => true,
        Ok(_) => return Err(crate::err("backup destination is not a regular file")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    let before = if existing {
        indexes(destination, log_path)?
    } else {
        vec![]
    };
    let mut metadata = im::load_sidecars(destination, &before)?;
    let original_hash = if existing {
        Some(backuprestore_core::sha256_file(destination)?)
    } else {
        None
    };
    // 目录级旧副档只有哈希匹配时才属于本镜像；绝不覆盖同目录另一个 WIM 的记录。
    let legacy_path = im::legacy_metadata_path(destination)?;
    let legacy: Option<BackupMetadata> = match backuprestore_core::read_json(&legacy_path) {
        Ok(value) => Some(value),
        Err(TaskError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            log(
                log_path,
                &format!("[WARN] 旧版目录级副档无法读取，不作为本次依据：{error}"),
            )?;
            None
        }
    };
    let own_legacy = legacy.as_ref().is_some_and(|m| {
        original_hash
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case(&m.image_sha256))
            && before.contains(&m.wim_index)
    });
    if own_legacy {
        let value = legacy.as_ref().unwrap();
        metadata
            .entry(value.wim_index)
            .or_insert_with(|| value.clone());
    }
    for &index in &before {
        if !metadata.contains_key(&index) {
            log(
                log_path,
                &format!("[WARN] 旧索引 {index} 无副档，保留未知来源状态，不伪造源卷信息"),
            )?;
        }
    }
    // 备份原副档；发布过程中若写入失败，使用这些文件与 previous.wim 回滚。
    let mut saved_sidecars = Vec::new();
    for index in 1..=before.len() as u32 + 1 {
        let path = im::index_metadata_path(destination, index)?;
        match fs::read(&path) {
            Ok(bytes) => {
                fs::write(workspace.join(format!("original-{index}.json")), &bytes)?;
                saved_sidecars.push((path, Some(bytes)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => saved_sidecars.push((path, None)),
            Err(e) => return Err(e.into()),
        }
    }
    if own_legacy {
        saved_sidecars.push((legacy_path.clone(), Some(fs::read(&legacy_path)?)));
    }
    if existing {
        fs::copy(destination, &candidate)?;
    }
    let config = backuprestore_core::write_capture_exclusion_config(source)?;
    let started = Utc::now();
    let name = if params.image_name.is_empty() {
        "Windows Backup"
    } else {
        &params.image_name
    };
    let mut args = vec![
        "/English".into(),
        if existing {
            "/Append-Image".into()
        } else {
            "/Capture-Image".into()
        },
        format!("/ImageFile:{}", candidate.display()),
        format!("/CaptureDir:{}", source.display()),
        format!("/Name:{name}"),
        "/CheckIntegrity".into(),
        format!("/ConfigFile:{}", config.display()),
    ];
    if !existing {
        args.push(format!(
            "/Compress:{}",
            backuprestore_core::canonical_compression(&params.compress)?
        ));
    }
    log(log_path, "STEP 2/4 捕获镜像（候选副本，无固定超时）")?;
    dism(&args, log_path)?;
    let after = indexes(&candidate, log_path)?;
    let new_index = im::verify_appended_index(&before, &after)?;
    let duration = (Utc::now() - started).num_seconds().max(1) as u64;
    let size = fs::metadata(&candidate)?.len();
    let source_size = identity.partition_size;
    let serial = identity.volume_serial.clone();
    metadata.insert(
        new_index,
        BackupMetadata {
            version: 3,
            image_type: "wim".into(),
            created: Utc::now(),
            started: Some(started),
            duration_secs: Some(duration),
            bytes_per_sec: Some(size / duration),
            computer: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into()),
            windows_edition: "unknown".into(),
            architecture: crate::native_windows_architecture(),
            windows_build: "unknown".into(),
            wim_index: new_index,
            image_sha256: String::new(),
            image_size: size,
            source: identity,
            captured_used_bytes: used,
            reserved_bytes: 0,
            minimum_target_size: source_size,
            volume_serial: serial,
            program_version: backuprestore_core::PROGRAM_VERSION.into(),
        },
    );
    log(log_path, "STEP 3/4 保留策略、索引回读和最终哈希校验")?;
    let keep = params
        .keep_indexes
        .unwrap_or(new_index)
        .max(1)
        .min(new_index);
    let removed = new_index - keep;
    for remaining in (keep + 1..=new_index).rev() {
        dism(
            &[
                "/English".into(),
                "/Delete-Image".into(),
                format!("/ImageFile:{}", candidate.display()),
                "/Index:1".into(),
            ],
            log_path,
        )?;
        let actual = indexes(&candidate, log_path)?;
        if actual != (1..remaining).collect::<Vec<_>>() {
            return Err(crate::err(
                "retention index read-back mismatch; original image not replaced",
            ));
        }
    }
    let metadata = im::remap_sidecars(&metadata, removed);
    let hash = backuprestore_core::sha256_file(&candidate)?;
    let final_size = fs::metadata(&candidate)?.len();
    im::write_sidecars(&candidate, &metadata, &hash, final_size, 0)?;
    log(
        log_path,
        &format!("校验完成：sha256={hash} bytes={final_size} 保留={keep} 删除={removed}"),
    )?;
    // 防止另一入口在候选构建期间修改了原镜像；发布前必须重验。
    if let Some(expected) = original_hash {
        if backuprestore_core::sha256_file(destination)? != expected {
            return Err(crate::err(
                "original WIM changed concurrently; candidate retained, publication refused",
            ));
        }
    } else if destination.try_exists()? {
        return Err(crate::err(
            "destination appeared concurrently; publication refused",
        ));
    }
    log(
        log_path,
        "STEP 4/4 发布镜像并回读副档；旧文件在事务成功前保留",
    )?;
    let previous = workspace.join("previous.wim");
    if existing {
        fs::rename(destination, &previous)?;
    }
    let publish = (|| -> Result<(), TaskError> {
        fs::rename(&candidate, destination)?;
        im::write_sidecars(destination, &metadata, &hash, final_size, new_index)?;
        if own_legacy {
            if let Some(first) = metadata.get(&1) {
                let mut first = first.clone();
                first.image_sha256 = hash.clone();
                first.image_size = final_size;
                backuprestore_core::write_json_atomic(&legacy_path, &first)?;
                if backuprestore_core::read_json::<BackupMetadata>(&legacy_path)? != first {
                    return Err(crate::err("legacy metadata read-back mismatch"));
                }
            } else {
                fs::remove_file(&legacy_path)?;
            }
        }
        if fs::metadata(destination)?.len() != final_size {
            return Err(crate::err("published WIM size mismatch"));
        }
        Ok(())
    })();
    if let Err(error) = publish {
        // 即使日志盘已满，也必须尝试恢复；不能让记录错误短路回滚。
        let rollback_log_error = log(
            log_path,
            &format!("[ERROR] 发布失败：{error}；开始恢复原镜像和副档"),
        )
        .err();
        let rollback = (|| -> Result<(), TaskError> {
            if destination.try_exists()? {
                fs::rename(destination, workspace.join("failed-publication.wim"))?;
            }
            if existing {
                fs::rename(&previous, destination)?;
            }
            for (path, bytes) in saved_sidecars {
                if let Some(bytes) = bytes {
                    fs::write(&path, &bytes)?;
                    if fs::read(&path)? != bytes {
                        return Err(crate::err("sidecar rollback read-back mismatch"));
                    }
                } else {
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            Ok(())
        })();
        return Err(crate::err(&format!(
            "publication failed: {error}; rollback={rollback:?}; log_error={rollback_log_error:?}; artifacts={}",
            workspace.display()
        )));
    }
    // 只清理本事务创建的目录；不触及用户的其他文件、旧镜像或工具链。
    fs::remove_dir_all(&workspace)?;
    log(log_path, "镜像及副档已发布并回读；候选目录已清理")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drive_validation_rejects_command_text() {
        assert_eq!(drive("d:").unwrap(), 'D');
        for value in ["", "CC", "C & echo x", "1", "C:\\"] {
            assert!(drive(value).is_err());
        }
    }
    // 在 Windows 上使用临时目录而不是用户卷，测试真正的 DISM 跨入口簿记。
    #[test]
    #[ignore = "需要 Windows 管理员和 DISM；只在显式验收时运行"]
    fn online_capture_append_keep_and_data_apply() {
        let root = std::env::temp_dir().join(format!(
            "br-online-test-{}",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        let image = root.join("backup.wim");
        let log_path = root.join("test.log");
        let mut identity = VolumeIdentity::new("test-disk", "test-partition");
        identity.partition_size = 1024 * 1024;
        let mut params = OnlineOpParams {
            operation: "backup".into(),
            source_drive: "Z".into(),
            target_drive: "Y".into(),
            image_path: image.to_string_lossy().into(),
            index: "1".into(),
            compress: "fast".into(),
            image_name: "中文 & % ! ^ 备份".into(),
            keep_indexes: None,
        };
        fs::write(source.join("data.txt"), "first").unwrap();
        capture(&params, &source, identity.clone(), 5, &log_path).unwrap();
        let first = crate::read_index_metadata(&image, 1).unwrap();
        fs::write(source.join("data.txt"), "second").unwrap();
        capture(&params, &source, identity.clone(), 6, &log_path).unwrap();
        assert_eq!(indexes(&image, &log_path).unwrap(), vec![1, 2]);
        let refreshed = crate::read_index_metadata(&image, 1).unwrap();
        assert_eq!(first.source, refreshed.source);
        assert_ne!(first.image_sha256, refreshed.image_sha256);
        params.keep_indexes = Some(2);
        fs::write(source.join("data.txt"), "third").unwrap();
        capture(&params, &source, identity, 5, &log_path).unwrap();
        assert_eq!(indexes(&image, &log_path).unwrap(), vec![1, 2]);
        let hash = backuprestore_core::sha256_file(&image).unwrap();
        for index in [1, 2] {
            assert_eq!(
                crate::read_index_metadata(&image, index)
                    .unwrap()
                    .image_sha256,
                hash
            );
        }
        assert!(!im::index_metadata_path(&image, 3).unwrap().exists());
        let target = root.join("restored");
        fs::create_dir(&target).unwrap();
        dism(
            &[
                "/English".into(),
                "/Apply-Image".into(),
                format!("/ImageFile:{}", image.display()),
                "/Index:2".into(),
                format!("/ApplyDir:{}", target.display()),
                "/CheckIntegrity".into(),
                "/Verify".into(),
            ],
            &log_path,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(target.join("data.txt")).unwrap(),
            "third"
        );
        let original_sidecars =
            [1, 2].map(|i| fs::read(im::index_metadata_path(&image, i).unwrap()).unwrap());
        assert!(
            capture(
                &params,
                &root.join("missing-source"),
                VolumeIdentity::new("x", "y"),
                1,
                &log_path
            )
            .is_err()
        );
        assert_eq!(backuprestore_core::sha256_file(&image).unwrap(), hash);
        for (i, bytes) in original_sidecars.iter().enumerate() {
            assert_eq!(
                &fs::read(im::index_metadata_path(&image, i as u32 + 1).unwrap()).unwrap(),
                bytes
            );
        }
        let bad_sidecar = im::index_metadata_path(&image, 1).unwrap();
        fs::write(&bad_sidecar, "invalid json").unwrap();
        assert!(
            capture(
                &params,
                &source,
                VolumeIdentity::new("x", "y"),
                1,
                &log_path
            )
            .is_err()
        );
        assert_eq!(backuprestore_core::sha256_file(&image).unwrap(), hash);
        fs::write(bad_sidecar, &original_sidecars[0]).unwrap();
        println!("ONLINE_E2E_SUCCESS log={} hash={hash}", log_path.display());
        // 保留小体积验收日志和产物，不删除失败证据。
    }
}
