// Windows 专用模块——非 Windows 编译时仅作类型检查，调用方被 cfg 门控。
#![cfg_attr(not(windows), allow(dead_code))]
//! 备份文件管理（P1-c）：镜像清单与旧索引删除。
//!
//! 背景：需求 §3 明确 `BackupRestore.exe` 负责「管理备份文件」，此前只有
//! `wim-info`（单镜像只读）+ 手工删文件 + capture 时的 `--keep-indexes`
//! 自动清理，用户没有可视化的「有哪些镜像、几个索引、哪些能删」入口。
//!
//! 本模块提供两块能力，GUI 的「备份文件管理…」对话框与 CLI 的
//! `list-images` / `delete-old-indexes` 共用：
//!
//! 1. [`list_images`]：列目录下的 WIM + 每个索引的元数据（sidecar 优先）。
//! 2. [`delete_oldest_indexes`]：删除最旧的若干索引，只保留最新 N 个。
//!    删除逻辑与 capture 的保留逻辑**同源**（DISM `/Delete-Image /Index:1`
//!    循环 + 逐次回读索引 + 副档重编号），但这里作用在**已发布的镜像**上，
//!    不碰候选事务。
//!
//! 纪律：删除是破坏性操作——调用方（GUI）必须先确认；副档重写用
//! `image_metadata::renumber_sidecars`（内部含写后回读），删完的镜像
//! 必须仍能被 `wim-info` 正常解析。

use std::collections::BTreeMap;
use std::path::PathBuf;

// 这三个只在 `#[cfg(windows)]` 的函数里用到：不加门控的话，本机 `cargo test`
// 会报「未使用的导入」——而删掉它们又会打断 Windows 构建。
#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::process::Output;
#[cfg(windows)]
use backuprestore_core::TaskError;
#[cfg(windows)]
use crate::error_code::ErrCode;
#[cfg(windows)]
pub(crate) use crate::prepare_plan::{ProbeOutcome, name_looks_like_full_windows, wim_mount_args};
pub(crate) use backuprestore_core::image_metadata::{
    index_metadata_path, legacy_metadata_path, read_index_metadata,
};

/// 一个索引的展示信息（sidecar 缺失时字段为 None，不致命）。
#[derive(Clone)]
pub(crate) struct IndexEntry {
    // 【2026-10-26】去掉冗余的 `index` 字段：`entries` 是 `BTreeMap<u32, _>`，
    // 索引号本就是键，字段从未被读（编译器 dead_code 告警）。
    pub name: Option<String>,
    pub created: Option<String>,
    pub source: Option<String>,
    pub image_size: Option<u64>,
}

/// 一个镜像文件的展示信息。
#[derive(Clone)]
pub(crate) struct ImageSummary {
    pub path: PathBuf,
    pub file_size: u64,
    /// DISM 实际列出的索引号（连续 1..=N）。
    pub indexes: Vec<u32>,
    pub entries: BTreeMap<u32, IndexEntry>,
}

/// `保留最近 N 个` 的钳制（纯函数，单测可锁）：至少留 1 个、不超过现有索引数。
/// total = 0（列不出索引的坏镜像）时返回 0，调用方按「无可删」处理。
pub(crate) fn clamp_keep(keep: u32, total: u32) -> u32 {
    if total == 0 {
        0
    } else {
        keep.max(1).min(total)
    }
}

/// 卷身份的短标签：`T: (BC9CBF66) 磁盘0/分区5`。
#[cfg(windows)]
fn source_label(source: &backuprestore_core::VolumeIdentity) -> String {
    let drive = source
        .drive_letter
        .map(|letter| format!("{letter}:"))
        .unwrap_or_else(|| "?".to_string());
    let disk = source
        .disk_number
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_string());
    let partition = source
        .partition_number
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_string());
    format!("{drive} ({}) 磁盘{disk}/分区{partition}", source.volume_serial)
}

/// 列目录下的 WIM 镜像及其索引元数据。
///
/// 每个镜像一次 `dism /Get-WimInfo`（拿真实索引号）+ sidecar 读取（拿名称/
/// 时间/源卷）。sidecar 缺失不致命——索引仍列出，只是描述信息为空。
/// 目录不存在 / 不是目录 → `Io` 错误（结构性码 `BR-IO-001`）。
#[cfg(windows)]
pub(crate) fn list_images(dir: &Path, log_path: &Path) -> Result<Vec<ImageSummary>, TaskError> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .map(|ext| ext.eq_ignore_ascii_case("wim"))
                    .unwrap_or(false)
        })
        .collect();
    paths.sort();
    let mut result = Vec::new();
    for path in paths {
        let file_metadata = fs::metadata(&path)?;
        let indexes = crate::online_operation::indexes(&path, log_path)?;
        let sidecars = backuprestore_core::image_metadata::load_sidecars(&path, &indexes)?;
        let mut entries = BTreeMap::new();
        for &index in &indexes {
            let sidecar = sidecars.get(&index);
            entries.insert(
                index,
                IndexEntry {
                    name: sidecar.and_then(|m| m.image_name.clone()),
                    created: sidecar.map(|m| {
                        m.created.format("%Y-%m-%d %H:%M:%S").to_string()
                    }),
                    source: sidecar.map(|m| source_label(&m.source)),
                    image_size: sidecar.map(|m| m.image_size),
                },
            );
        }
        result.push(ImageSummary {
            path,
            file_size: file_metadata.len(),
            indexes,
            entries,
        });
    }
    Ok(result)
}

/// 删除镜像最旧的若干个索引，只保留最新 `keep` 个。
///
/// 返回 `(删除数, 保留数)`。删除数 = 0 时什么都不做（已是目标状态）。
///
/// 与 capture 内的保留循环同源：每次删索引 1（最旧），删完立刻回读索引集合，
/// 与期望值不一致就中止（镜像留在半删状态由用户处置，绝不假装成功）；
/// 全部删完再一次性重写副档（`renumber_sidecars` 内部重算 sha256/尺寸并逐份回读）。
#[cfg(windows)]
pub(crate) fn delete_oldest_indexes(
    image: &Path,
    keep: u32,
    log_path: &Path,
) -> Result<(u32, u32), TaskError> {
    let before = crate::online_operation::indexes(image, log_path)?;
    let total = before.len() as u32;
    let keep = clamp_keep(keep, total);
    let removed = total - keep;
    if removed == 0 {
        return Ok((0, keep));
    }
    for remaining in (keep + 1..=total).rev() {
        crate::online_operation::dism(
            &[
                "/English".into(),
                "/Delete-Image".into(),
                format!("/ImageFile:{}", image.display()),
                "/Index:1".into(),
            ],
            log_path,
        )?;
        let actual = crate::online_operation::indexes(image, log_path)?;
        if actual != (1..remaining).collect::<Vec<_>>() {
            return Err(crate::errc(crate::error_code::ErrCode::DeleteIndexReadbackMismatch));
        }
    }
    backuprestore_core::image_metadata::renumber_sidecars(image, total, removed)?;
    Ok((removed, keep))
}

/// 备份文件管理的默认目录：项目约定的镜像卷 `H:\BRImages`；不存在则退回程序目录。
/// 只影响 `list-images` 的缺省值与 GUI 对话框初值，用户随时可以显式指定别的目录。
pub(crate) fn default_image_dir() -> String {
    let preferred = std::path::PathBuf::from("H:\\BRImages");
    if preferred.is_dir() {
        return preferred.to_string_lossy().into_owned();
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .to_string_lossy()
        .into_owned()
}

/// 字节数的人类可读格式（二进制单位，与界面其它容量显示一致）。
pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `list-images`：打印镜像与索引清单。人类可读模式给人看；`--json` 模式每镜像一行
/// 结构化结论（供自动化/排障），日志路径统一走 `cli_json` 单一出口。
#[cfg(windows)]
pub(crate) fn cli_list(args: Vec<String>) -> Result<(), TaskError> {
    let dir = args
        .into_iter()
        .find(|a| !a.starts_with("--"))
        .unwrap_or_else(default_image_dir);
    let dir = std::path::PathBuf::from(dir);
    let (log_dir, _note) = crate::online_log::resolve_dir("image-admin")
        .map_err(|error| crate::err(&format!("cannot create log directory: {error}")))?;
    let log_path = log_dir.join("admin.log");
    let images = list_images(&dir, &log_path)?;
    if crate::cli_json::enabled() {
        for image in &images {
            let indexes: Vec<_> = image
                .indexes
                .iter()
                .map(|index| {
                    let entry = image.entries.get(index);
                    serde_json::json!({
                        "index": index,
                        "name": entry.and_then(|e| e.name.clone()),
                        "created": entry.and_then(|e| e.created.clone()),
                        "source": entry.and_then(|e| e.source.clone()),
                        "image_size": entry.and_then(|e| e.image_size),
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::json!({
                    "command": "list-images",
                    "success": true,
                    "dir": dir.display().to_string(),
                    "image": image.path.display().to_string(),
                    "file_size": image.file_size,
                    "index_count": image.indexes.len(),
                    "indexes": indexes,
                    "log_path": log_path.display().to_string(),
                })
            );
        }
        if images.is_empty() {
            println!(
                "{}",
                serde_json::json!({
                    "command": "list-images",
                    "success": true,
                    "dir": dir.display().to_string(),
                    "image": null,
                    "index_count": 0,
                    "indexes": [],
                    "log_path": log_path.display().to_string(),
                })
            );
        }
    } else if images.is_empty() {
        println!("目录下没有 WIM 镜像：{}", dir.display());
    } else {
        for image in &images {
            println!(
                "{}  ({}, {} 个索引)",
                image.path.display(),
                format_size(image.file_size),
                image.indexes.len()
            );
            for index in &image.indexes {
                let entry = image.entries.get(index);
                let name = entry
                    .and_then(|e| e.name.clone())
                    .unwrap_or_else(|| "(无副档)".to_string());
                let created = entry
                    .and_then(|e| e.created.clone())
                    .unwrap_or_else(|| "-".to_string());
                let source = entry
                    .and_then(|e| e.source.clone())
                    .unwrap_or_else(|| "-".to_string());
                let size = entry
                    .and_then(|e| e.image_size)
                    .map(format_size)
                    .unwrap_or_else(|| "-".to_string());
                println!("  索引 {index:<3} {name:<24} {created}  {size:<12} {source}");
            }
        }
        println!("共 {} 个镜像：{}", images.len(), dir.display());
    }
    crate::cli_json::set_detail(None, Some(log_path.display().to_string()));
    Ok(())
}

/// `delete-old-indexes`：破坏性操作。删除前后各记一行日志；结果同时进 `--json`。
#[cfg(windows)]
pub(crate) fn cli_delete(args: Vec<String>) -> Result<(), TaskError> {
    let image = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .ok_or_else(|| crate::err("WIM image path is required"));
    let keep = args
        .iter()
        .position(|a| a == "--keep")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<u32>().ok())
        .ok_or_else(|| crate::err("--keep <n> is required"));
    let (image, keep) = image.and_then(|image| keep.map(|k| (image, k)))?;
    let image = std::path::PathBuf::from(image);
    let (log_dir, _note) = crate::online_log::resolve_dir("image-admin")
        .map_err(|error| crate::err(&format!("cannot create log directory: {error}")))?;
    let log_path = log_dir.join("admin.log");
    crate::append_log(
        &log_path,
        &format!(
            "delete-old-indexes started: image={} keep={keep}",
            image.display()
        ),
    )?;
    let (removed, kept) = delete_oldest_indexes(&image, keep, &log_path)?;
    crate::append_log(
        &log_path,
        &format!(
            "delete-old-indexes done: removed={removed} kept={kept} image={}",
            image.display()
        ),
    )?;
    if !crate::cli_json::enabled() {
        println!(
            "已删除 {removed} 个旧索引，保留最新 {kept} 个：{}",
            image.display()
        );
    }
    crate::cli_json::set_detail(None, Some(log_path.display().to_string()));
    Ok(())
}

#[cfg(windows)]
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
    if let Ok(output) = crate::process_runner::capture_output(
        "dism.exe",
        &[
            "/English",
            "/Get-WimInfo",
            &format!("/WimFile:{image_path}"),
        ],
    ) && let Ok(parsed) = crate::prepare_plan::parse_dism_images(&output)
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

#[cfg(windows)]
pub(crate) fn wim_info(image_path: String, skip_hash: bool) -> Result<(), TaskError> {
    use backuprestore_core::{read_json, sha256_file, validate_absolute_path};

    validate_absolute_path(&image_path)?;
    let path = PathBuf::from(&image_path);
    if !path.is_file() {
        return Err(crate::err(&format!("WIM image does not exist: {image_path}")));
    }

    if let Some(parent) = path.parent() {
        let wim_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut sidecar_entries: Vec<(u32, backuprestore_core::BackupMetadata)> = Vec::new();
        if let Ok(dir) = std::fs::read_dir(parent) {
            let prefix = format!("{wim_name}.index-");
            let suffix = ".metadata.json";
            for entry in dir.flatten() {
                let fname = entry.file_name();
                let fname_str = fname.to_string_lossy();
                if fname_str.starts_with(&prefix) && fname_str.ends_with(suffix) {
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
            sidecar_entries.sort_by_key(|(idx, _)| *idx);
            let dism_names = sidecar_dism_names(&image_path, &sidecar_entries)?;
            let images: Vec<serde_json::Value> = sidecar_entries
                .iter()
                .map(|(idx, meta)| {
                    let display_name = crate::prepare_plan::sidecar_image_name(
                        meta.image_name.as_deref(),
                        *idx,
                        dism_names.get(idx).map(String::as_str),
                    );
                    serde_json::json!({
                        "ImageIndex": idx,
                        "ImageName": display_name.clone(),
                        "ImageSize": meta.image_size,
                        "ImageDescription": format!("Computer: {}, Build: {}", meta.computer, meta.windows_build),
                        "ImageVersion": meta.windows_build,
                        "Architecture": meta.architecture,
                        "EditionId": meta.windows_edition,
                        "InstallationType": "Client",
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
                serde_json::to_string(&serde_json::json!({
                    "image": image_path,
                    "sha256": sidecar_entries.first().map(|(_, m)| m.image_sha256.as_str()).unwrap_or(""),
                    "images": images,
                    "source": "sidecar",
                }))?
            );
            return Ok(());
        }
    }

    let output = crate::process_runner::capture_output(
        "dism.exe",
        &[
            "/English",
            "/Get-WimInfo",
            &format!("/WimFile:{image_path}"),
        ],
    )?;
    let images = crate::prepare_plan::parse_dism_images(&output)?;

    let sha256 = if skip_hash {
        "skipped".to_string()
    } else {
        sha256_file(&path)?
    };

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "image": image_path,
            "sha256": sha256,
            "images": images,
            "source": "dism",
        }))?
    );
    Ok(())
}

#[cfg(windows)]
pub(crate) fn cli_wim_info(args: Vec<String>) -> Result<(), TaskError> {
    let skip_hash = args.iter().any(|a| a == "--skip-hash");
    let image_path = args
        .into_iter()
        .find(|a| !a.starts_with("--"))
        .ok_or_else(|| crate::err("WIM image path is required"))?;
    wim_info(image_path, skip_hash)
}

/// 权威挂载探测。安全判据并未回退——可疑的一律挂挂载判真，恢复阶段的离线注册
/// 前置检查是所有路径的硬兜底。
#[cfg(windows)]
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
        Err(_) => {
            let lower = name.to_ascii_lowercase();
            if lower.contains("windows pe") || lower.contains("winpe") {
                return Err(crate::errc_with(ErrCode::ImageIsWindowsPe, &[&name]));
            }
            Ok(ProbeOutcome::HeuristicPass)
        }
    }
}

/// 只读挂载镜像并检查完整 Windows 标记（reagentc.exe + SYSTEM hive）。
/// 返回 Ok(true)=标记齐全；Ok(false)=缺标记（纯 PE）；Err=挂载/卸载失败。
#[cfg(windows)]
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
#[cfg(windows)]
fn wim_image_display_name(image_path: &str, index: u32, log: &Path) -> Result<String, TaskError> {
    let output = crate::run_output_logged(
        "dism.exe",
        &[
            "/English",
            "/Get-WimInfo",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keep_never_below_one_or_above_total() {
        assert_eq!(clamp_keep(0, 3), 1);
        assert_eq!(clamp_keep(1, 3), 1);
        assert_eq!(clamp_keep(2, 3), 2);
        assert_eq!(clamp_keep(99, 3), 3);
        // 列不出索引的坏镜像：0 让调用方走「无可删」分支，绝不除零/下溢。
        assert_eq!(clamp_keep(5, 0), 0);
        assert_eq!(clamp_keep(0, 0), 0);
    }

    #[test]
    fn removed_count_is_total_minus_clamped_keep() {
        for (keep, total) in [(0u32, 1u32), (1, 1), (2, 5), (99, 4)] {
            let clamped = clamp_keep(keep, total);
            assert!(clamped >= 1 && clamped <= total);
            assert_eq!(total - clamped, total - clamp_keep(keep, total));
        }
    }

    #[test]
    fn format_size_handles_units_and_boundaries() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(1024 * 1024), "1.0 MiB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GiB");
        assert_eq!(format_size(1024 * 1024 * 1024 * 1024), "1.0 TiB");
    }
}
