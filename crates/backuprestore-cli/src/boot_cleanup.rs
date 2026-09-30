//! 任务 RE 暂存清理：只用卷身份，不相信跨重启保存的盘符；删除失败必须上报。

use backuprestore_core::{TaskError, VolumeIdentity};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const RE_STAGING_DIR: &str = "BackupRestoreRE";

/// 同时接受裸 GUID 和完整卷路径；拒绝盘符、相对路径及附带子路径的输入。
pub(crate) fn staging_volume_root(volume: &VolumeIdentity) -> Result<PathBuf, TaskError> {
    let guid = crate::text_parsing::bare_volume_guid(&volume.volume_guid)
        .filter(|guid| crate::text_parsing::is_guid(guid))
        .ok_or_else(|| TaskError::Invalid("RE staging volume has no valid GUID".into()))?;
    Ok(PathBuf::from(format!(r"\\?\Volume{guid}\")))
}

fn remove_if_present(path: &Path, directory: bool) -> Result<(), TaskError> {
    let result = if directory {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(TaskError::Invalid(format!(
            "RE staging cleanup failed at {}: {error}",
            path.display()
        ))),
    }
}

/// 仅删除本项目的两个载荷文件和空目录，绝不递归删除未知内容。
/// 卷不可访问、文件被占用、目录非空等情况都不能伪报成功。
pub(crate) fn remove_staging(root: &Path) -> Result<PathBuf, TaskError> {
    if !fs::metadata(root)?.is_dir() {
        return Err(TaskError::Invalid(
            "RE staging volume root is not a directory".into(),
        ));
    }
    let dir = root.join(RE_STAGING_DIR);
    match fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(TaskError::Invalid(format!(
                "RE staging path is not a plain directory: {}",
                dir.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(dir),
        Err(error) => return Err(error.into()),
    }
    for name in ["Winre.wim", "boot.sdi"] {
        remove_if_present(&dir.join(name), false)?;
    }
    remove_if_present(&dir, true)?;
    // 删除调用之后仍要回读；权限错误不能被 exists() 隐藏成“不存在”。
    match fs::symlink_metadata(&dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(dir),
        Err(error) => Err(error.into()),
        Ok(_) => Err(TaskError::Invalid(format!(
            "RE staging directory still exists: {}",
            dir.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "br-cleanup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join(RE_STAGING_DIR)).unwrap();
        root
    }

    #[test]
    fn staging_root_ignores_stale_letter_and_accepts_both_guid_shapes() {
        let guid = "{f0753766-30a4-410e-944f-38d139113634}";
        let expected = r"\\?\Volume{f0753766-30a4-410e-944f-38d139113634}\";
        let mut volume = VolumeIdentity::new("", "");
        volume.drive_letter = Some('F');
        for value in [guid, expected] {
            volume.volume_guid = value.into();
            assert_eq!(
                staging_volume_root(&volume).unwrap(),
                PathBuf::from(expected)
            );
        }
    }

    #[test]
    fn staging_root_rejects_paths_without_an_exact_guid() {
        let mut volume = VolumeIdentity::new("", "");
        for value in [
            "",
            "F:\\",
            "{invalid}",
            r"\\?\Volume{f0753766-30a4-410e-944f-38d139113634}\other",
        ] {
            volume.volume_guid = value.into();
            assert!(staging_volume_root(&volume).is_err());
        }
    }

    #[test]
    fn cleanup_removes_only_payload_and_is_idempotent() {
        let root = fixture();
        let dir = root.join(RE_STAGING_DIR);
        fs::write(dir.join("Winre.wim"), b"payload").unwrap();
        fs::write(dir.join("boot.sdi"), b"sdi").unwrap();
        fs::write(root.join("keep.txt"), b"user data").unwrap();
        assert_eq!(remove_staging(&root).unwrap(), dir);
        assert!(!dir.exists());
        remove_staging(&root).unwrap();
        assert_eq!(fs::read(root.join("keep.txt")).unwrap(), b"user data");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_preserves_unknown_files_and_reports_nonempty_directory() {
        let root = fixture();
        let unknown = root.join(RE_STAGING_DIR).join("unknown.txt");
        fs::write(&unknown, b"keep").unwrap();
        assert!(remove_staging(&root).is_err());
        assert_eq!(fs::read(unknown).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_propagates_file_deletion_errors() {
        let root = fixture();
        fs::create_dir(root.join(RE_STAGING_DIR).join("Winre.wim")).unwrap();
        assert!(remove_staging(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreachable_volume_is_not_successful_cleanup() {
        let root = fixture();
        fs::remove_dir_all(&root).unwrap();
        assert!(remove_staging(&root).is_err());
    }
}
