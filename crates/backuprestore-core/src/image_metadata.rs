//! 各入口共用的 WIM 副档簿记。缺失可识别，损坏和写入失败不得伪装成成功。
use crate::{BackupMetadata, TaskError, read_json, sha256_file, write_json_atomic};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub fn index_metadata_path(image: &Path, index: u32) -> Result<PathBuf, TaskError> {
    if index == 0 {
        return Err(TaskError::Invalid("WIM index cannot be zero".into()));
    }
    let name = image
        .file_name()
        .ok_or_else(|| TaskError::Invalid("image path has no file name".into()))?
        .to_string_lossy();
    Ok(image.with_file_name(format!("{name}.index-{index}.metadata.json")))
}
pub fn legacy_metadata_path(image: &Path) -> Result<PathBuf, TaskError> {
    image
        .parent()
        .map(|p| p.join("metadata.json"))
        .ok_or_else(|| TaskError::Invalid("image path has no parent".into()))
}
pub fn read_index_metadata(image: &Path, index: u32) -> Result<BackupMetadata, TaskError> {
    let metadata: BackupMetadata = match read_json(index_metadata_path(image, index)?) {
        Ok(value) => value,
        Err(TaskError::Io(error)) if error.kind() == ErrorKind::NotFound => {
            read_json(legacy_metadata_path(image)?)?
        }
        Err(error) => return Err(error),
    };
    if metadata.wim_index != index {
        return Err(TaskError::Invalid(format!(
            "backup metadata is for WIM index {}, not selected index {index}",
            metadata.wim_index
        )));
    }
    Ok(metadata)
}

/// 不把损坏的副档当作第三方镜像的「正常缺失」。读取完毕后再开始修改镜像。
pub fn load_sidecars(
    image: &Path,
    indexes: &[u32],
) -> Result<BTreeMap<u32, BackupMetadata>, TaskError> {
    let mut result = BTreeMap::new();
    for &index in indexes {
        match read_json::<BackupMetadata>(index_metadata_path(image, index)?) {
            Ok(value) if value.wim_index == index => {
                result.insert(index, value);
            }
            Ok(_) => {
                return Err(TaskError::Invalid(format!(
                    "sidecar index mismatch: {index}"
                )));
            }
            Err(TaskError::Io(error)) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(result)
}

/// 完整索引集合回读：不能只检查「有一个新编号」，还要保证旧编号全部仍在。
pub fn verify_appended_index(before: &[u32], after: &[u32]) -> Result<u32, TaskError> {
    let expected: Vec<u32> = (1..=before.len() as u32).collect();
    let next = before.len() as u32 + 1;
    if before != expected || after != (1..=next).collect::<Vec<_>>() {
        return Err(TaskError::Invalid(format!(
            "WIM append index read-back mismatch: before={before:?} after={after:?}"
        )));
    }
    Ok(next)
}

/// 全量快照先读入内存，再生成新编号，防止覆盖尚未读取的旧副档。
pub fn remap_sidecars(
    values: &BTreeMap<u32, BackupMetadata>,
    removed: u32,
) -> BTreeMap<u32, BackupMetadata> {
    values
        .iter()
        .filter(|(index, _)| **index > removed)
        .map(|(index, value)| {
            let mut value = value.clone();
            value.wim_index = index - removed;
            (value.wim_index, value)
        })
        .collect()
}

/// 使用已算好的哈希统一写回并逐份反序列化核对；一轮只需遍历大镜像一次。
pub fn write_sidecars(
    image: &Path,
    values: &BTreeMap<u32, BackupMetadata>,
    hash: &str,
    size: u64,
    old_count: u32,
) -> Result<(), TaskError> {
    for (&index, metadata) in values {
        let mut value = metadata.clone();
        value.wim_index = index;
        value.image_sha256 = hash.into();
        value.image_size = size;
        let path = index_metadata_path(image, index)?;
        write_json_atomic(&path, &value)?;
        if read_json::<BackupMetadata>(&path)? != value {
            return Err(TaskError::Invalid(format!(
                "sidecar write read-back mismatch: {}",
                path.display()
            )));
        }
    }
    // 即使某个留存索引没有副档，也必须移除旧编号的残留，不能误指向已删除的源卷。
    for index in 1..=old_count {
        if !values.contains_key(&index) {
            remove_sidecar(&index_metadata_path(image, index)?)?;
        }
    }
    Ok(())
}
fn remove_sidecar(path: &Path) -> Result<(), TaskError> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => Err(TaskError::Invalid(format!(
            "sidecar still exists: {}",
            path.display()
        ))),
    }
}
pub fn sync_sidecars(
    image: &Path,
    indexes: impl IntoIterator<Item = u32>,
) -> Result<(String, u64), TaskError> {
    let indexes: Vec<_> = indexes.into_iter().collect();
    let values = load_sidecars(image, &indexes)?;
    let hash = sha256_file(image)?;
    let size = fs::metadata(image)?.len();
    write_sidecars(image, &values, &hash, size, 0)?;
    Ok((hash, size))
}
pub fn renumber_sidecars(image: &Path, total: u32, removed: u32) -> Result<Vec<u32>, TaskError> {
    if removed >= total {
        return Err(TaskError::Invalid("cannot remove all WIM indexes".into()));
    }
    let values = load_sidecars(image, &(1..=total).collect::<Vec<_>>())?;
    let values = remap_sidecars(&values, removed);
    let hash = sha256_file(image)?;
    let size = fs::metadata(image)?.len();
    write_sidecars(image, &values, &hash, size, total)?;
    Ok(values.keys().copied().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VolumeIdentity;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("br-sidecar-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
        fn image(&self) -> PathBuf {
            self.0.join("backup.wim")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn metadata(index: u32) -> BackupMetadata {
        BackupMetadata {
            version: 3,
            image_type: "wim".into(),
            created: chrono::Utc::now(),
            started: None,
            duration_secs: None,
            bytes_per_sec: None,
            computer: format!("source-{index}"),
            windows_edition: "unknown".into(),
            architecture: "arm64".into(),
            windows_build: "unknown".into(),
            wim_index: index,
            image_sha256: "old".into(),
            image_size: 1,
            source: VolumeIdentity::new("disk", format!("partition-{index}")),
            captured_used_bytes: index as u64 * 100,
            reserved_bytes: 0,
            minimum_target_size: index as u64 * 1000,
            volume_serial: format!("serial-{index}"),
            program_version: "test".into(),
        }
    }
    #[test]
    fn append_requires_complete_index_set() {
        assert_eq!(verify_appended_index(&[], &[1]).unwrap(), 1);
        assert_eq!(verify_appended_index(&[1, 2], &[1, 2, 3]).unwrap(), 3);
        for after in [vec![1, 3], vec![1, 2, 2, 3], vec![2, 3], vec![1, 2], vec![]] {
            assert!(verify_appended_index(&[1, 2], &after).is_err());
        }
    }
    #[test]
    fn renumber_preserves_every_source_and_refreshes_hash() {
        for removed in [1, 2, 3] {
            let f = Fixture::new();
            let image = f.image();
            fs::write(&image, "changed WIM").unwrap();
            for i in 1..=4 {
                write_json_atomic(index_metadata_path(&image, i).unwrap(), &metadata(i)).unwrap();
            }
            let kept = renumber_sidecars(&image, 4, removed).unwrap();
            assert_eq!(kept, (1..=4 - removed).collect::<Vec<_>>());
            for i in kept {
                let m = read_index_metadata(&image, i).unwrap();
                assert_eq!(m.computer, format!("source-{}", i + removed));
                assert_eq!(m.minimum_target_size, (i + removed) as u64 * 1000);
                assert_eq!(m.image_sha256, sha256_file(&image).unwrap());
                assert_eq!(m.image_size, 11);
            }
            for i in 5 - removed..=4 {
                assert!(!index_metadata_path(&image, i).unwrap().exists());
            }
        }
    }
    #[test]
    fn missing_retained_sidecar_never_keeps_deleted_source() {
        let f = Fixture::new();
        let image = f.image();
        fs::write(&image, "WIM").unwrap();
        for i in [1, 3] {
            write_json_atomic(index_metadata_path(&image, i).unwrap(), &metadata(i)).unwrap();
        }
        renumber_sidecars(&image, 3, 1).unwrap();
        assert!(!index_metadata_path(&image, 1).unwrap().exists());
        assert_eq!(read_index_metadata(&image, 2).unwrap().computer, "source-3");
    }
    #[test]
    fn corrupt_sidecar_is_error_not_missing() {
        let f = Fixture::new();
        let image = f.image();
        fs::write(&image, "WIM").unwrap();
        let path = index_metadata_path(&image, 1).unwrap();
        fs::write(&path, "broken json").unwrap();
        assert!(sync_sidecars(&image, [1]).is_err());
        assert!(renumber_sidecars(&image, 2, 1).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "broken json");
    }
    #[test]
    fn append_sync_keeps_identity_and_rejects_write_failure() {
        let f = Fixture::new();
        let image = f.image();
        fs::write(&image, "new WIM").unwrap();
        let values = BTreeMap::from([(1, metadata(1)), (2, metadata(2))]);
        write_sidecars(&image, &values, "final", 7, 0).unwrap();
        assert_eq!(
            read_index_metadata(&image, 1).unwrap().source,
            values[&1].source
        );
        assert_eq!(
            read_index_metadata(&image, 2).unwrap().image_sha256,
            "final"
        );
        let p = index_metadata_path(&image, 2).unwrap();
        fs::remove_file(&p).unwrap();
        fs::create_dir(&p).unwrap();
        assert!(write_sidecars(&image, &values, "another", 8, 0).is_err());
    }
    #[test]
    fn legacy_only_used_for_matching_index() {
        let f = Fixture::new();
        let image = f.image();
        write_json_atomic(legacy_metadata_path(&image).unwrap(), &metadata(1)).unwrap();
        assert_eq!(read_index_metadata(&image, 1).unwrap().computer, "source-1");
        assert!(read_index_metadata(&image, 2).is_err());
        assert!(index_metadata_path(&image, 0).is_err());
    }
    #[test]
    fn sidecar_io_errors_and_invalid_removal_are_not_success() {
        let f = Fixture::new();
        let image = f.image();
        fs::write(&image, "WIM").unwrap();
        fs::create_dir(index_metadata_path(&image, 1).unwrap()).unwrap();
        assert!(load_sidecars(&image, &[1]).is_err());
        assert!(read_index_metadata(&image, 1).is_err());
        assert!(renumber_sidecars(&image, 2, 2).is_err());
    }
}
