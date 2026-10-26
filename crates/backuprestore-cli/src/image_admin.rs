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
use backuprestore_core::TaskError;

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
}
