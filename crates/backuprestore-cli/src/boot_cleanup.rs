//! 任务 RE 暂存清理：只用卷身份，不相信跨重启保存的盘符；删除失败必须上报。

use backuprestore_core::{TaskError, VolumeIdentity};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const RE_STAGING_DIR: &str = "BackupRestoreRE";

/// 载荷子目录名：只接受规范 UUID，杜绝空段、分隔符与路径穿越。
pub(crate) fn staging_task_segment(task_id: &str) -> Result<&str, TaskError> {
    backuprestore_core::validate_task_id(task_id)
        .map_err(|_| TaskError::Invalid(format!("RE staging task id is not a UUID: {task_id}")))?;
    Ok(task_id)
}

/// 载荷目录相对卷根的路径：`BackupRestoreRE\<任务ID>`。
///
/// **唯一构造点**。BCD 元素（`ramdisksdipath`）、文件系统路径、续跑探测与清理
/// 全部从这里取同一个值；历史上这些地方各自拼字符串，于是"写进 BCD 的路径"
/// 和"实际放文件的路径"可以悄悄漂移。
///
/// 按任务隔离的原因：此前所有任务共用固定的 `BackupRestoreRE\Winre.wim`。
/// 任务 A 准备完还没执行，再准备任务 B 就会覆盖同一个文件，A 的 `rearm()`
/// 拿活载荷比对簿记哈希必然不符，于是 A 永久无法续跑；而清理任一任务都会
/// 删掉另一个任务仍然需要的载荷。
pub(crate) fn staging_relative_dir(task_id: &str) -> Result<String, TaskError> {
    Ok(format!(
        r"{RE_STAGING_DIR}\{}",
        staging_task_segment(task_id)?
    ))
}

/// 本任务载荷目录的**文件系统**路径。
///
/// 刻意不复用 [`staging_relative_dir`] 的字符串去 `join`：那个值是给 BCD 用的
/// Windows 形式，里面的 `\` 在本机（macOS）开发测试里不是路径分隔符，直接
/// `join` 会造出一个名字里带反斜杠的单层目录，让按任务隔离在本机测试中失效。
pub(crate) fn staging_task_path(root: &Path, task_id: &str) -> Result<PathBuf, TaskError> {
    Ok(root
        .join(RE_STAGING_DIR)
        .join(staging_task_segment(task_id)?))
}

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

/// 仅删除**本任务**的两个载荷文件和它自己的空目录，绝不递归删除未知内容，
/// 也绝不碰其他任务的载荷子目录。
/// 卷不可访问、文件被占用、目录非空等情况都不能伪报成功。
pub(crate) fn remove_staging(root: &Path, task_id: &str) -> Result<PathBuf, TaskError> {
    if !fs::metadata(root)?.is_dir() {
        return Err(TaskError::Invalid(
            "RE staging volume root is not a directory".into(),
        ));
    }
    let parent = root.join(RE_STAGING_DIR);
    let dir = staging_task_path(root, task_id)?;
    match fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(TaskError::Invalid(format!(
                "RE staging path is not a plain directory: {}",
                dir.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            prune_empty_parent(&parent)?;
            return Ok(dir);
        }
        Err(error) => return Err(error.into()),
    }
    for name in ["Winre.wim", "boot.sdi"] {
        remove_if_present(&dir.join(name), false)?;
    }
    remove_if_present(&dir, true)?;
    // 删除调用之后仍要回读；权限错误不能被 exists() 隐藏成“不存在”。
    match fs::symlink_metadata(&dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
        Ok(_) => {
            return Err(TaskError::Invalid(format!(
                "RE staging directory still exists: {}",
                dir.display()
            )));
        }
    }
    prune_empty_parent(&parent)?;
    Ok(dir)
}

/// 本任务子目录删掉后，顺手回收空的 `BackupRestoreRE\` 父目录。
/// 里面还有别的任务载荷时必须原样保留——那不是残留，是另一个任务正在用的东西。
fn prune_empty_parent(parent: &Path) -> Result<(), TaskError> {
    match fs::read_dir(parent) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Ok(());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    match fs::remove_dir(parent) {
        Ok(()) => Ok(()),
        // 竞态下别的任务刚放进新载荷：保留目录，不算失败。
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) if is_not_empty(&error) => Ok(()),
        Err(error) => Err(TaskError::Invalid(format!(
            "RE staging cleanup failed at {}: {error}",
            parent.display()
        ))),
    }
}

fn is_not_empty(error: &io::Error) -> bool {
    // ERROR_DIR_NOT_EMPTY = 145（Windows）；类 Unix 为 ENOTEMPTY。
    matches!(
        error.raw_os_error(),
        Some(145) | Some(66) | Some(39) | Some(90)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TASK: &str = "11111111-2222-4333-8444-555555555555";
    const OTHER: &str = "99999999-8888-4777-8666-555555555555";

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "br-cleanup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(staging_task_path(&root, TASK).unwrap()).unwrap();
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

    /// 载荷目录必须按任务隔离，而且只认规范 UUID——否则一个被改过的任务 ID
    /// 就能把清理指向 `BackupRestoreRE\` 本身或卷上的其他目录。
    #[test]
    fn staging_directory_is_per_task_and_rejects_unsafe_ids() {
        assert_eq!(
            staging_relative_dir(TASK).unwrap(),
            format!(r"BackupRestoreRE\{TASK}")
        );
        for bad in [
            "",
            "..",
            "Winre.wim",
            r"..\..\Windows",
            "11111111-2222-4333-8444",
            "11111111-2222-4333-8444-555555555555/x",
        ] {
            assert!(staging_relative_dir(bad).is_err(), "{bad} 应被拒绝");
        }
    }

    #[test]
    fn cleanup_removes_only_payload_and_is_idempotent() {
        let root = fixture();
        let dir = staging_task_path(&root, TASK).unwrap();
        fs::write(dir.join("Winre.wim"), b"payload").unwrap();
        fs::write(dir.join("boot.sdi"), b"sdi").unwrap();
        fs::write(root.join("keep.txt"), b"user data").unwrap();
        assert_eq!(remove_staging(&root, TASK).unwrap(), dir);
        assert!(!dir.exists());
        // 本任务是最后一个载荷，父目录一并回收。
        assert!(!root.join(RE_STAGING_DIR).exists());
        remove_staging(&root, TASK).unwrap();
        assert_eq!(fs::read(root.join("keep.txt")).unwrap(), b"user data");
        fs::remove_dir_all(root).unwrap();
    }

    /// 问题 9 的核心回归：清理任务 A 绝不能动到任务 B 仍然需要的载荷，
    /// 父目录也必须因为 B 还在而保留。
    #[test]
    fn cleanup_never_touches_another_tasks_payload() {
        let root = fixture();
        let mine = staging_task_path(&root, TASK).unwrap();
        let theirs = staging_task_path(&root, OTHER).unwrap();
        fs::create_dir_all(&theirs).unwrap();
        fs::write(mine.join("Winre.wim"), b"mine").unwrap();
        fs::write(theirs.join("Winre.wim"), b"theirs").unwrap();
        fs::write(theirs.join("boot.sdi"), b"theirs-sdi").unwrap();

        remove_staging(&root, TASK).unwrap();
        assert!(!mine.exists());
        assert_eq!(fs::read(theirs.join("Winre.wim")).unwrap(), b"theirs");
        assert_eq!(fs::read(theirs.join("boot.sdi")).unwrap(), b"theirs-sdi");
        assert!(root.join(RE_STAGING_DIR).is_dir(), "父目录仍被 B 使用");

        remove_staging(&root, OTHER).unwrap();
        assert!(!root.join(RE_STAGING_DIR).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_preserves_unknown_files_and_reports_nonempty_directory() {
        let root = fixture();
        let unknown = staging_task_path(&root, TASK).unwrap().join("unknown.txt");
        fs::write(&unknown, b"keep").unwrap();
        assert!(remove_staging(&root, TASK).is_err());
        assert_eq!(fs::read(unknown).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_propagates_file_deletion_errors() {
        let root = fixture();
        fs::create_dir(staging_task_path(&root, TASK).unwrap().join("Winre.wim")).unwrap();
        assert!(remove_staging(&root, TASK).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreachable_volume_is_not_successful_cleanup() {
        let root = fixture();
        fs::remove_dir_all(&root).unwrap();
        assert!(remove_staging(&root, TASK).is_err());
    }

    #[test]
    fn cleanup_rejects_an_invalid_task_id_instead_of_wiping_the_parent() {
        let root = fixture();
        fs::write(
            staging_task_path(&root, TASK).unwrap().join("Winre.wim"),
            b"payload",
        )
        .unwrap();
        assert!(remove_staging(&root, "not-a-uuid").is_err());
        assert!(
            staging_task_path(&root, TASK)
                .unwrap()
                .join("Winre.wim")
                .is_file()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
