//! 恢复环境离线备份执行引擎（Recovery Backup Engine）。
//!
//! 承载 WinRE / PE 环境下的主备份执行逻辑：
//! - 源卷与目标镜像路径解析
//! - DISM 排除配置动态生成
//! - 初次捕获与增量追加策略（心跳观测、原子替换、完整性核验）
//! - WIM 索引元数据簿记（sidecars 写入、时间/耗时/速度统计）
//! - 多版本保留策略与历史索引轮转清理（`keep_indexes` 与 renumber）

use std::collections::BTreeMap;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::sync::Arc;

#[cfg(windows)]
use chrono::Utc;

use backuprestore_core::{Task, TaskError, TaskStore};
#[cfg(windows)]
use backuprestore_core::{
    canonical_compression, read_json, write_json_atomic, BackupMetadata, Stage, PROGRAM_VERSION,
};

#[cfg(windows)]
use crate::append_log;
use crate::err;
#[cfg(windows)]
use crate::process_runner::{capture_logged, run_logged};
#[cfg(windows)]
use crate::recovery_volume::{
    env_optional, env_optional_u64, native_windows_architecture, resolve_volume_path,
    resolve_volume_root,
};
#[cfg(windows)]
use crate::{index_metadata_path, legacy_metadata_path};

#[cfg(not(windows))]
#[allow(dead_code)]
pub(crate) fn execute_recovery_backup(
    _store: &TaskStore,
    _task: &mut Task,
    _log: &Path,
    _metadata_context: Option<&BTreeMap<String, String>>,
    _progress: Option<&()>,
    _finalize_success: bool,
) -> Result<(), TaskError> {
    Err(err(
        "real Recovery execution is only available on Windows/WinRE",
    ))
}

#[cfg(windows)]
pub(crate) fn execute_recovery_backup(
    store: &TaskStore,
    task: &mut Task,
    log: &Path,
    metadata_context: Option<&BTreeMap<String, String>>,
    progress: Option<&Arc<crate::recovery_progress::ProgressShared>>,
    finalize_success: bool,
) -> Result<(), TaskError> {
    append_log(log, crate::i18n::msg("step_mount"))?;
    if let Some(p) = progress {
        p.reset_start_time();
    }
    let source = task.source.clone().ok_or_else(|| err("missing source"))?;
    let destination = task
        .destination
        .clone()
        .ok_or_else(|| err("missing destination"))?;
    let source_path = resolve_volume_root(&source, log)?;
    // v1.7.11 新通道：载荷只在镜像卷上，注册位全程只读，所以捕获进镜像的
    // 天生就是任务前的干净 WinRE —— 方案 D 的「捕获前翻回干净件」不再需要。
    let destination_path =
        resolve_volume_path(&destination.volume, &destination.relative_path, log)?;
    // 备份索引名：任务自定义（GUI 默认「系统版本+年月日」，如
    // `Windows 11 26200.9550 20261015`）或兜底默认名。CLI 侧没有
    // image_name 时不能照搬 GUI 的算法——这里可能跑在 RE 里，读到的
    // "当前系统"是被还原的环境而不是被备份的源卷。
    let image_name = task
        .image_name
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Windows Backup".to_string());
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
    // 备份耗时统计：记录开始时间，捕获完成后计算总耗时与平均速度。
    let backup_started = Utc::now();
    append_log(
        log,
        &format!("Backup capture started at {}", backup_started.to_rfc3339()),
    )?;
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
    let mut previous_metadata =
        backuprestore_core::image_metadata::load_sidecars(&destination_path, &previous_indexes)?;
    let legacy_path = legacy_metadata_path(&destination_path)?;
    let legacy_metadata: Option<BackupMetadata> = legacy_path
        .is_file()
        .then(|| read_json(&legacy_path))
        .transpose()
        .ok()
        .flatten();

    if let Some(value) = legacy_metadata.as_ref()
        && previous_indexes.contains(&value.wim_index)
        && previous_hash
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case(&value.image_sha256))
    {
        previous_metadata
            .entry(value.wim_index)
            .or_insert_with(|| value.clone());
    }

    // 生成 DISM 排除配置（Parallels 卷根占位符/回收站/临时目录/更新缓存/
    // 各用户浏览器缓存），WinRE 的 TEMP 位于 X: RAM 盘，配置文件不会落在
    // 捕获卷内。排除项会显著缩小 WIM 体积，且全部为 DISM 规范内的根路径写法。
    // 与 GUI 在线备份、PE 备份共用 core 的实现，避免某条路径漏接 /ConfigFile。
    // self_exe_dir 用 store.root（= Windows 侧运行目录，RE 内由
    // `resume <task-root>` 参数带进来）而不是 current_exe()：RE 内
    // current_exe 是恢复环境里的暂存路径，与捕获卷不同卷，动态排除项
    // 会永远不生成（2026-10-24 实测：五个部署件全被打进 T: 镜像）。
    // 同卷时排除自身部署件；tasks\logs\ 证据仍全留。
    let exclude_config_path = backuprestore_core::write_capture_exclusion_config(
        &source_path,
        Some(store.root.as_path()),
    )?;
    let exclude_arg = format!("/ConfigFile:{}", exclude_config_path.display());

    // A power loss can leave a partial WIM behind.  For the first
    // capture, only that partial file is removed.  For an existing
    // WIM, copy it to a same-volume candidate and append there; the
    // original remains recoverable until the candidate has passed
    // DISM inspection and is renamed into place.
    let _ = fs::remove_file(&partial);
    append_log(log, crate::i18n::msg("step_capture"))?;
    let new_index = if existing {
        let candidate = PathBuf::from(format!(
            "{}.{}.append-candidate.wim",
            destination_path.display(),
            task.task_id
        ));
        let _ = fs::remove_file(&candidate);
        fs::copy(&destination_path, &candidate)?;
        let append_args = [
            "/English",
            "/Append-Image",
            &format!("/ImageFile:{}", candidate.display()),
            &format!("/CaptureDir:{}", source_path.display()),
            &format!("/Name:{image_name}"),
            "/CheckIntegrity",
            &exclude_arg,
        ];
        // 捕获心跳：恢复环境里这条链同样要「先全卷预扫再写」，期间镜像恒为
        // WIM 头、DISM 日志静默，进度窗口的百分比会长时间不动。心跳行让
        // 「还在扫盘」与「真的卡死」在日志里可区分（只观测，不干预）。
        let heartbeat =
            crate::capture_heartbeat::Heartbeat::start(candidate.clone(), log.to_path_buf());
        let append_result = run_logged("dism.exe", &append_args, log);
        drop(heartbeat);
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
        // 压缩率：仅首次创建 WIM 时生效；追加备份沿用 WIM 已有压缩设置。
        let compress_level =
            canonical_compression(task.compress.as_deref().unwrap_or("fast"))?;
        let heartbeat = crate::capture_heartbeat::Heartbeat::start(partial.clone(), log.to_path_buf());
        let capture_result = run_logged(
            "dism.exe",
            &[
                "/English",
                "/Capture-Image",
                &format!("/ImageFile:{}", partial.display()),
                &format!("/CaptureDir:{}", source_path.display()),
                &format!("/Name:{image_name}"),
                &format!("/Compress:{compress_level}"),
                "/CheckIntegrity",
                &exclude_arg,
            ],
            log,
        );
        drop(heartbeat);
        capture_result?;
        let indexes = wim_indexes(&partial, log)?;
        if indexes != [1] {
            return Err(err("first backup capture did not produce WIM index 1"));
        }
        fs::rename(&partial, &destination_path)?;
        1
    };
    let image_size = fs::metadata(&destination_path)?.len();
    // 备份耗时统计：总耗时与平均速度（字节/秒）。
    let backup_finished = Utc::now();
    let duration_secs = (backup_finished - backup_started).num_seconds().max(1) as u64;
    let bytes_per_sec = image_size / duration_secs;
    append_log(
        log,
        &format!(
            "Backup capture finished: started={} finished={} duration={}s speed={}/s image={} bytes",
            backup_started.to_rfc3339(),
            backup_finished.to_rfc3339(),
            duration_secs,
            bytes_per_sec,
            image_size,
        ),
    )?;
    append_log(log, crate::i18n::msg("step_verify"))?;
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
        version: 3,
        image_type: "wim".into(),
        created: Utc::now(),
        started: Some(backup_started),
        duration_secs: Some(duration_secs),
        bytes_per_sec: Some(bytes_per_sec),
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
        // 与 DISM /Name 的有效索引名一致，供 sidecar 快速路径呈现真名
        image_name: Some(image_name),
    };
    previous_metadata.insert(new_index, metadata.clone());
    backuprestore_core::image_metadata::write_sidecars(
        &destination_path,
        &previous_metadata,
        &image_sha256,
        image_size,
        0,
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
        let first_path = index_metadata_path(&destination_path, 1)?;
        if first_path.try_exists()? {
            let legacy: BackupMetadata = read_json(first_path)?;
            write_json_atomic(&legacy_path, &legacy)?;
        }
    }
    append_log(
        log,
        &format!("Backup metadata written for WIM index {new_index}"),
    )?;
    // 删除后立即回读索引，再使用共用快照重编号，任何读写错误都使任务失败。
    if let Some(keep) = task.keep_indexes.filter(|n| *n > 0) {
        let keep = keep.max(1).min(new_index);
        for remaining in (keep + 1..=new_index).rev() {
            run_logged(
                "dism.exe",
                &[
                    "/English",
                    "/Delete-Image",
                    &format!("/ImageFile:{}", destination_path.display()),
                    "/Index:1",
                ],
                log,
            )?;
            if wim_indexes(&destination_path, log)? != (1..remaining).collect::<Vec<_>>() {
                return Err(err("WIM retention index read-back mismatch"));
            }
        }
        if keep < new_index {
            backuprestore_core::image_metadata::renumber_sidecars(
                &destination_path,
                new_index,
                new_index - keep,
            )?;
            if legacy_belongs_to_image {
                let path = index_metadata_path(&destination_path, 1)?;
                if path.try_exists()? {
                    let first: BackupMetadata = read_json(path)?;
                    write_json_atomic(&legacy_path, &first)?;
                } else if legacy_path.try_exists()? {
                    fs::remove_file(&legacy_path)?;
                }
            }
            append_log(
                log,
                &format!("Kept latest {keep} WIM indexes; sidecars renumbered and verified"),
            )?;
        }
    }
    if finalize_success {
        append_log(log, crate::i18n::msg("step_cleanup"))?;
        store.write_transition(task, Stage::Success)?;
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn wim_indexes(path: &Path, log: &Path) -> Result<Vec<u32>, TaskError> {
    let output = capture_logged(
        "dism.exe",
        &[
            "/English",
            "/Get-WimInfo",
            &format!("/WimFile:{}", path.display()),
        ],
        log,
    )?;
    parse_wim_indexes_output(&output)
}

#[cfg(any(windows, test))]
#[allow(dead_code)]
pub(crate) fn parse_wim_indexes_output(output: &str) -> Result<Vec<u32>, TaskError> {
    let mut indexes = crate::prepare_plan::parse_dism_images(output)?
        .iter()
        .filter_map(|image| image.get("ImageIndex").and_then(serde_json::Value::as_u64))
        .filter_map(|index| u32::try_from(index).ok())
        .collect::<Vec<_>>();
    indexes.sort_unstable();
    indexes.dedup();
    if indexes.is_empty() || indexes.contains(&0) {
        return Err(err("DISM returned invalid WIM indexes"));
    }
    Ok(indexes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wim_indexes_handles_single_and_multiple() {
        let single_dism = r#"
Deployment Image Servicing and Management tool
Version: 10.0.26100.1

Details for image : C:\test.wim

Index : 1
Name : Windows 11 Pro
Description : Windows 11 Pro
Size : 15,000,000,000 bytes

The operation completed successfully.
"#;
        let indexes = parse_wim_indexes_output(single_dism).expect("parse single index");
        assert_eq!(indexes, vec![1]);

        let multi_dism = r#"
Deployment Image Servicing and Management tool
Version: 10.0.26100.1

Details for image : C:\test.wim

Index : 3
Name : Backup 3

Index : 1
Name : Backup 1

Index : 2
Name : Backup 2

The operation completed successfully.
"#;
        let indexes = parse_wim_indexes_output(multi_dism).expect("parse multi index");
        assert_eq!(indexes, vec![1, 2, 3]);
    }

    #[test]
    fn parse_wim_indexes_rejects_empty_or_zero() {
        let empty_dism = "The operation completed successfully.";
        assert!(parse_wim_indexes_output(empty_dism).is_err());

        let zero_index_dism = r#"
Index : 0
Name : Invalid
"#;
        assert!(parse_wim_indexes_output(zero_index_dism).is_err());
    }
}
