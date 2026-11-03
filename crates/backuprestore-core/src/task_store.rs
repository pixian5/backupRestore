//! 任务存储库（TaskStore）、原子文件 I/O 与状态事务持久化。
//!
//! Windows 前端与 WinRE 恢复环境依赖本模块持久化任务状态，确保在任何
//! 写入边界发生断电时均能一致恢复或重放。

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(windows)]
use std::io;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::ptr::null_mut;

use crate::{
    ImageSpec, Operation, PayloadManifest, Stage, StatusRecord, Task, TaskError,
    VolumeIdentity, default_progress,
};

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn MoveFileExW(existing_file_name: *const u16, new_file_name: *const u16, flags: u32) -> i32;
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut std::ffi::c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn FlushFileBuffers(file: *mut std::ffi::c_void) -> i32;
    fn CloseHandle(object: *mut std::ffi::c_void) -> i32;
}

#[cfg(windows)]
const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
#[cfg(windows)]
const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
#[cfg(windows)]
const GENERIC_READ: u32 = 0x8000_0000;
#[cfg(windows)]
const GENERIC_WRITE: u32 = 0x4000_0000;
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x0000_0001;
#[cfg(windows)]
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
#[cfg(windows)]
const FILE_SHARE_DELETE: u32 = 0x0000_0004;
#[cfg(windows)]
const OPEN_EXISTING: u32 = 3;
#[cfg(windows)]
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
#[cfg(windows)]
const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = -1isize as *mut std::ffi::c_void;

pub fn validate_task_id(value: &str) -> Result<(), TaskError> {
    canonical_task_id(value).map(|_| ())
}

pub fn canonical_task_id(value: &str) -> Result<String, TaskError> {
    Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| TaskError::coded("BR-CORE-035", "task_id is not a UUID"))
}

pub fn validate_sha256_text(name: &str, value: &str) -> Result<(), TaskError> {
    if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(TaskError::coded("BR-CORE-036", format!(
            "{name} sha256 must be 64 hex characters"
        )));
    }
    Ok(())
}

pub fn require_complete(name: &str, identity: &VolumeIdentity) -> Result<(), TaskError> {
    if identity.is_complete() {
        Ok(())
    } else {
        Err(TaskError::coded("BR-CORE-037", format!("{name} identity is incomplete")))
    }
}

pub fn validate_relative_path(value: &str) -> Result<(), TaskError> {
    if value.trim().is_empty() || value.contains('\0') || value.chars().any(char::is_control) {
        return Err(TaskError::coded("BR-CORE-038", "path must be a non-empty relative path"));
    }
    let normalized = value.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() || normalized.as_bytes().get(1) == Some(&b':') {
        return Err(TaskError::coded("BR-CORE-038", "path must be a non-empty relative path"));
    }
    for component in path.components() {
        if matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(TaskError::coded("BR-CORE-039", "path may not escape the volume root"));
        }
    }
    Ok(())
}

/// Validate a Windows absolute file path supplied by the user.
pub fn validate_absolute_path(value: &str) -> Result<(), TaskError> {
    if value.trim().is_empty() || value.contains('\0') || value.chars().any(char::is_control) {
        return Err(TaskError::coded("BR-CORE-040", "path must be a non-empty Windows absolute path"));
    }
    let normalized = value.replace('/', "\\");
    let bytes = normalized.as_bytes();
    if bytes.len() < 4 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return Err(TaskError::coded("BR-CORE-041", "path must use a drive-root form such as B:\\Backups\\Windows.wim"));
    }
    let rest = &normalized[3..];
    if rest.ends_with('\\') {
        return Err(TaskError::coded("BR-CORE-042", "absolute image path must identify a file, not a volume root"));
    }
    for part in rest.split('\\') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(TaskError::coded("BR-CORE-043", "absolute image path contains an invalid component"));
        }
    }
    Ok(())
}

pub fn sha256_file(path: impl AsRef<Path>) -> Result<String, TaskError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn verify_sha256(path: impl AsRef<Path>, expected: &str) -> Result<(), TaskError> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(TaskError::coded("BR-CORE-044", format!(
            "sha256 mismatch: expected {expected}, got {actual}"
        )))
    }
}

pub fn verify_image_file(path: impl AsRef<Path>, image: &ImageSpec) -> Result<(), TaskError> {
    let path = path.as_ref();
    let size = fs::metadata(path)?.len();
    if image.size_bytes != 0 && size != image.size_bytes {
        return Err(TaskError::coded("BR-CORE-045", format!(
            "image size mismatch: expected {}, got {size}",
            image.size_bytes
        )));
    }
    verify_sha256(path, &image.sha256)
}

pub fn validate_payload_files(
    manifest: &PayloadManifest,
    recovery: impl AsRef<Path>,
    task_json: impl AsRef<Path>,
    recovery_task_env: impl AsRef<Path>,
    original_winre: impl AsRef<Path>,
    staged_winre: impl AsRef<Path>,
) -> Result<(), TaskError> {
    if manifest.task_id.trim().is_empty() {
        return Err(TaskError::coded("BR-CORE-046", "payload task id is empty"));
    }
    for (name, path, expected) in [
        (
            "recovery",
            recovery.as_ref(),
            manifest.recovery_sha256.as_str(),
        ),
        ("task", task_json.as_ref(), manifest.task_sha256.as_str()),
        (
            "original WinRE",
            original_winre.as_ref(),
            manifest.original_winre_sha256.as_str(),
        ),
        (
            "staged WinRE",
            staged_winre.as_ref(),
            manifest.staged_winre_sha256.as_str(),
        ),
    ] {
        if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(TaskError::coded("BR-CORE-047", format!(
                "{name} payload hash is not SHA-256"
            )));
        }
        verify_sha256(path, expected)?;
    }
    if let Some(expected) = manifest.recovery_task_env_sha256.as_deref() {
        verify_sha256(recovery_task_env, expected)?;
    }
    Ok(())
}

/// Serialize to a sibling file, flush to disk, then rename into place.
pub fn write_json_atomic<T: Serialize>(path: impl AsRef<Path>, value: &T) -> Result<(), TaskError> {
    let path = path.as_ref();
    let parent = path
        .parent()
        .ok_or_else(|| TaskError::coded("BR-CORE-048", "JSON path has no parent"))?;
    fs::create_dir_all(parent)?;
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|x| x.to_str()).unwrap_or("json")
    ));
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(windows)]
    {
        let from = tmp
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let to = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let handle = unsafe {
            CreateFileW(
                to.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error().into());
        }
        let flushed = unsafe { FlushFileBuffers(handle) != 0 };
        unsafe {
            CloseHandle(handle);
        }
        if !flushed {
            return Err(io::Error::last_os_error().into());
        }
    }
    #[cfg(not(windows))]
    fs::rename(&tmp, path)?;
    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

pub fn read_json<T: for<'de> Deserialize<'de>>(path: impl AsRef<Path>) -> Result<T, TaskError> {
    let bytes = fs::read(path)?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    Ok(serde_json::from_slice(bytes)?)
}

#[derive(Serialize, Deserialize)]
struct StateTransaction {
    task: Task,
    status: StatusRecord,
}

#[derive(Debug, Clone)]
pub struct TaskStore {
    pub root: PathBuf,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CleanupReport {
    pub removed_task_ids: Vec<String>,
    pub skipped_nonterminal: usize,
    pub skipped_mounted: Vec<String>,
    pub skipped_malformed: usize,
}

impl TaskStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn task_dir(&self, task_id: &str) -> Result<PathBuf, TaskError> {
        let canonical = canonical_task_id(task_id)?;
        Ok(self.root.join("tasks").join(canonical))
    }

    pub fn task_path(&self, task_id: &str) -> Result<PathBuf, TaskError> {
        Ok(self.task_dir(task_id)?.join("task.json"))
    }

    pub fn status_path(&self, task_id: &str) -> Result<PathBuf, TaskError> {
        Ok(self.task_dir(task_id)?.join("status.json"))
    }

    pub fn log_path(&self, task_id: &str) -> Result<PathBuf, TaskError> {
        Ok(self.task_dir(task_id)?.join("recovery.log"))
    }

    fn transaction_path(&self, id: &str) -> Result<PathBuf, TaskError> {
        Ok(self.task_dir(id)?.join("state-transaction.json"))
    }

    pub fn create(&self, task: &Task) -> Result<(), TaskError> {
        task.validate()?;
        let dir = self.task_dir(&task.task_id)?;
        if dir.exists() {
            return Err(TaskError::coded("BR-CORE-049", "task directory already exists; refusing to overwrite it"));
        }
        fs::create_dir_all(&dir)?;
        write_json_atomic(self.task_path(&task.task_id)?, task)?;
        let status = StatusRecord {
            task_id: task.task_id.clone(),
            operation: task.operation,
            stage: task.status,
            progress: 0,
            updated: Utc::now(),
            error_code: None,
            error_code_text: None,
            error: None,
        };
        write_json_atomic(self.status_path(&task.task_id)?, &status)?;
        Ok(())
    }

    pub fn load(&self, task_id: &str) -> Result<Task, TaskError> {
        validate_task_id(task_id)?;
        self.replay_state_transaction(task_id)?;
        let task: Task = read_json(self.task_path(task_id)?)?;
        let requested_id = Uuid::parse_str(task_id)
            .map_err(|_| TaskError::coded("BR-CORE-035", "task_id is not a UUID"))?;
        let actual_id = Uuid::parse_str(&task.task_id)
            .map_err(|_| TaskError::coded("BR-CORE-050", "task file task_id is not a UUID"))?;
        if actual_id != requested_id {
            return Err(TaskError::coded("BR-CORE-051", "task file id does not match requested task id"));
        }
        task.validate()?;
        Ok(task)
    }

    pub fn incomplete_tasks(&self) -> Result<Vec<Task>, TaskError> {
        let tasks_root = self.root.join("tasks");
        if !tasks_root.is_dir() {
            return Ok(Vec::new());
        }

        let mut incomplete = Vec::new();
        for entry in fs::read_dir(&tasks_root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path();
            let Some(id) = path.file_name().and_then(|name| name.to_str()) else {
                return Err(TaskError::coded("BR-CORE-052", format!(
                    "existing task directory has a non-Unicode name: {}",
                    path.display()
                )));
            };
            validate_task_id(id)?;
            let status: StatusRecord = read_json(path.join("status.json")).map_err(|error| {
                TaskError::coded("BR-CORE-053", format!(
                    "cannot inspect status for existing task {id} at {}: {error}",
                    path.display()
                ))
            })?;
            if status.operation == Operation::Probe {
                continue;
            }
            if !status.stage.is_incomplete() {
                continue;
            }
            let task: Task = read_json(path.join("task.json")).map_err(|error| {
                TaskError::coded("BR-CORE-054", format!(
                    "cannot inspect existing task {id} at {}: {error}",
                    path.display()
                ))
            })?;
            task.validate().map_err(|error| {
                TaskError::coded("BR-CORE-055", format!("existing task {id} is invalid: {error}"))
            })?;
            if task.task_id != id
                || status.task_id != task.task_id
                || status.operation != task.operation
                || status.stage != task.status
            {
                return Err(TaskError::coded("BR-CORE-056", format!(
                    "existing task {id} has inconsistent task/status records; inspect {} before creating another task",
                    path.display()
                )));
            }
            if task.status.is_incomplete() {
                incomplete.push(task);
            }
        }
        Ok(incomplete)
    }

    pub fn write_transition(
        &self,
        task: &mut Task,
        next: Stage,
    ) -> Result<StatusRecord, TaskError> {
        let status = task.transition(next)?;
        self.commit_state(task, &status)?;
        Ok(status)
    }

    pub fn write_failure(
        &self,
        task: &mut Task,
        code: i32,
        error: impl Into<String>,
    ) -> Result<StatusRecord, TaskError> {
        self.write_failure_coded(task, None, error, Some(code))
    }

    pub fn write_failure_coded(
        &self,
        task: &mut Task,
        code_text: Option<&str>,
        error: impl Into<String>,
        legacy_code: Option<i32>,
    ) -> Result<StatusRecord, TaskError> {
        if !task.status.can_transition_to(Stage::Failed) {
            return Err(TaskError::coded("BR-CORE-057", format!(
                "cannot fail task from {:?}",
                task.status
            )));
        }
        task.status = Stage::Failed;
        let status = StatusRecord {
            task_id: task.task_id.clone(),
            operation: task.operation,
            stage: Stage::Failed,
            progress: 0,
            updated: Utc::now(),
            error_code: legacy_code,
            error_code_text: code_text.map(str::to_owned),
            error: Some(error.into()),
        };
        self.commit_state(task, &status)?;
        Ok(status)
    }

    pub fn write_recovery_error(
        &self,
        task: &Task,
        error: impl Into<String>,
    ) -> Result<StatusRecord, TaskError> {
        self.write_recovery_error_coded(task, None, error)
    }

    pub fn write_recovery_error_coded(
        &self,
        task: &Task,
        code_text: Option<&str>,
        error: impl Into<String>,
    ) -> Result<StatusRecord, TaskError> {
        if !task.status.is_incomplete() {
            return Err(TaskError::coded("BR-CORE-058", "终态任务不能记录可重试错误"));
        }
        let status = StatusRecord {
            task_id: task.task_id.clone(),
            operation: task.operation,
            stage: task.status,
            progress: default_progress(task.status),
            updated: Utc::now(),
            error_code: Some(1),
            error_code_text: code_text.map(str::to_owned),
            error: Some(error.into()),
        };
        self.commit_state(task, &status)?;
        Ok(status)
    }

    pub fn cleanup_task_artifacts(&self, task_id: &str) -> Result<bool, TaskError> {
        let path = self.task_dir(task_id)?;
        if !path.is_dir() {
            return Ok(false);
        }
        let task = self.load(task_id)?;
        let status: StatusRecord = read_json(self.status_path(task_id)?)?;
        if task.status.is_incomplete()
            || status.stage != task.status
            || status.task_id != task.task_id
        {
            return Err(TaskError::coded("BR-CORE-059", "未完成任务禁止清理恢复材料"));
        }
        let mount = path.join("mount");
        if mount.is_dir() && fs::read_dir(&mount)?.next().is_some() {
            return Err(TaskError::coded("BR-CORE-060", format!(
                "task {task_id} still has a mounted WinRE tree"
            )));
        }
        let mut removed = false;
        for name in ["mount", "stage", "original", "payload"] {
            let artifact = path.join(name);
            if artifact.is_dir() {
                fs::remove_dir_all(artifact)?;
                removed = true;
            }
        }
        Ok(removed)
    }

    pub fn cleanup_terminal_tasks(&self, keep_latest: usize) -> Result<CleanupReport, TaskError> {
        let tasks_root = self.root.join("tasks");
        if !tasks_root.is_dir() {
            return Ok(CleanupReport::default());
        }

        let mut report = CleanupReport::default();
        let mut terminal = Vec::new();
        for entry in fs::read_dir(&tasks_root)? {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(id) = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
            else {
                report.skipped_malformed += 1;
                continue;
            };
            let task = read_json::<Task>(path.join("task.json"));
            let status = read_json::<StatusRecord>(path.join("status.json"));
            let (Ok(task), Ok(status)) = (task, status) else {
                report.skipped_malformed += 1;
                continue;
            };
            if task.validate().is_err() || status.operation != task.operation {
                report.skipped_malformed += 1;
                continue;
            }
            let task_terminal = matches!(task.status, Stage::Success | Stage::Failed);
            let status_terminal = matches!(status.stage, Stage::Success | Stage::Failed);
            if task.task_id != id
                || status.task_id != task.task_id
                || task.status != status.stage
                || !task_terminal
                || !status_terminal
            {
                report.skipped_nonterminal += 1;
                continue;
            }
            terminal.push((status.updated, path, id));
        }

        terminal.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        for (_, path, id) in terminal.into_iter().skip(keep_latest) {
            let mount = path.join("mount");
            let mounted = mount.is_dir() && fs::read_dir(&mount)?.next().is_some();
            if mounted {
                report.skipped_mounted.push(id);
                continue;
            }
            if self.cleanup_task_artifacts(&id)? {
                report.removed_task_ids.push(id);
            }
        }
        Ok(report)
    }

    pub(crate) fn commit_state(&self, task: &Task, status: &StatusRecord) -> Result<(), TaskError> {
        let transaction = StateTransaction {
            task: task.clone(),
            status: status.clone(),
        };
        write_json_atomic(self.transaction_path(&task.task_id)?, &transaction)?;
        self.replay_state_transaction(&task.task_id)
    }

    pub(crate) fn replay_state_transaction(&self, id: &str) -> Result<(), TaskError> {
        let path = self.transaction_path(id)?;
        if !path.try_exists()? {
            return Ok(());
        }
        let transaction: StateTransaction = read_json(&path)?;
        let task = &transaction.task;
        let status = &transaction.status;
        task.validate()?;
        if canonical_task_id(&task.task_id)? != canonical_task_id(id)?
            || status.task_id != task.task_id
            || status.operation != task.operation
            || status.stage != task.status
        {
            return Err(TaskError::coded("BR-CORE-073", "状态事务身份或阶段不一致"));
        }
        let mut current: Task = read_json(self.task_path(id)?)?;
        if current.status != task.status && !current.status.can_transition_to(task.status) {
            return Err(TaskError::coded("BR-CORE-074", "状态事务存在非法阶段跳转"));
        }
        current.status = task.status;
        let mut expected = task.clone();
        for value in [&mut current, &mut expected] {
            if let Some(v) = value.source.as_mut() {
                v.drive_letter = None;
            }
            if let Some(v) = value.workspace_volume.as_mut() {
                v.drive_letter = None;
            }
            if let Some(v) = value.target.as_mut() {
                v.volume.drive_letter = None;
            }
            if let Some(v) = value.image.as_mut() {
                v.volume.drive_letter = None;
            }
            if let Some(v) = value.destination.as_mut() {
                v.volume.drive_letter = None;
            }
        }
        if current != expected {
            return Err(TaskError::coded("BR-CORE-075", "状态事务改变了任务身份或参数"));
        }
        write_json_atomic(self.task_path(id)?, task)?;
        write_json_atomic(self.status_path(id)?, status)?;
        fs::remove_file(path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use crate::{BootMode, BootPlan, DestinationSpec};

    fn identity(value: &str, size: u64) -> VolumeIdentity {
        let mut i = VolumeIdentity::new("disk", value);
        i.volume_guid = format!("volume-{value}");
        i.partition_type_guid = "{00000000-0000-0000-0000-000000000000}".into();
        i.disk_number = Some(0);
        i.partition_number = Some(1);
        i.partition_size = size;
        i.filesystem = "NTFS".into();
        i
    }

    fn backup_task() -> Task {
        let mut task = Task::new(
            Operation::Backup,
            BootPlan {
                mode: BootMode::ReturnExisting,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: None,
                boot_sequence_requested: true,
            },
        );
        task.source = Some(identity("source", 100));
        task.workspace_volume = Some(identity("task", 100));
        task.destination = Some(DestinationSpec {
            volume: identity("image", 100),
            absolute_path: None,
            relative_path: "MyBackup/Windows.wim".into(),
        });
        task
    }
    #[test]
    fn atomic_task_store_round_trip_and_failure() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("backuprestore-core-{suffix}"));
        let store = TaskStore::new(&root);
        let mut task = backup_task();
        store.create(&task).unwrap();
        assert_eq!(store.load(&task.task_id).unwrap().status, Stage::Prepared);
        assert_eq!(
            store.load(&task.task_id.to_uppercase()).unwrap().task_id,
            task.task_id
        );
        assert!(store.create(&task).is_err());
        store
            .write_transition(&mut task, Stage::BootRequested)
            .unwrap();
        store
            .write_transition(&mut task, Stage::RecoveryStarted)
            .unwrap();
        store.write_failure(&mut task, 123, "test failure").unwrap();
        let status: StatusRecord = read_json(store.status_path(&task.task_id).unwrap()).unwrap();
        assert_eq!(status.stage, Stage::Failed);
        assert_eq!(status.error_code, Some(123));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn write_failure_coded_persists_the_stable_code() {
        // 稳定错误码必须能进 status.json：界面「刷新任务状态」据此显示
        // `[BR-域-序号]`，而 `Io`/`Json` 这类失败的码不在文案里。
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("br-coded-{suffix}"));
        let store = TaskStore::new(&root);
        let mut task = backup_task();
        store.create(&task).unwrap();
        store
            .write_failure_coded(&mut task, Some("BR-IO-001"), "磁盘已满", Some(1))
            .unwrap();
        let status: StatusRecord = read_json(store.status_path(&task.task_id).unwrap()).unwrap();
        assert_eq!(status.error_code_text.as_deref(), Some("BR-IO-001"));
        assert_eq!(status.error.as_deref(), Some("磁盘已满"));
        assert_eq!(status.stage, Stage::Failed);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn write_failure_without_code_stays_code_free() {
        // 无码失败（用户主动放弃）不得凭空造码——界面会显示「无稳定错误码」。
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("br-nocode-{suffix}"));
        let store = TaskStore::new(&root);
        let mut task = backup_task();
        store.create(&task).unwrap();
        store.write_failure(&mut task, 2, "用户主动放弃").unwrap();
        let status: StatusRecord = read_json(store.status_path(&task.task_id).unwrap()).unwrap();
        assert_eq!(status.error_code_text, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_status_json_without_code_text_still_parses() {
        // ★ 旧 status.json 没有 error_code_text 字段：必须照样读得进来，
        // 否则历史任务会在 `parse_status` 处整体失败（读状态是排查的第一步）。
        // 原文抄自实机 `H:\brwork\tasks`：camelCase 键名 + kebab-case 阶段。
        let legacy = r#"{
            "taskId": "0f7f2b1e-1111-2222-3333-444455556666",
            "operation": "backup",
            "stage": "failed",
            "progress": 0,
            "updated": "2026-10-20T01:02:03Z",
            "errorCode": 1,
            "error": "镜像不存在"
        }"#;
        let status: StatusRecord = serde_json::from_str(legacy).unwrap();
        assert_eq!(status.error_code_text, None);
        assert_eq!(status.error.as_deref(), Some("镜像不存在"));
        assert_eq!(status.error_code, Some(1));
    }

    #[test]
    fn recovery_error_coded_keeps_stage_and_carries_code() {
        // 可重试失败：阶段不变（还要续跑），但码要落盘。
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("br-recov-{suffix}"));
        let store = TaskStore::new(&root);
        let mut task = backup_task();
        store.create(&task).unwrap();
        store
            .write_transition(&mut task, Stage::BootRequested)
            .unwrap();
        store
            .write_recovery_error_coded(&task, Some("BR-IMG-008"), "DISM 超时")
            .unwrap();
        let status: StatusRecord = read_json(store.status_path(&task.task_id).unwrap()).unwrap();
        assert_eq!(status.stage, Stage::BootRequested, "可重试错误不得改阶段");
        assert_eq!(status.error_code_text.as_deref(), Some("BR-IMG-008"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_complete_can_transition_to_failed_so_cleanup_failures_can_close_the_task() {
        // 【2026-10-18 缺陷修复 H2】`RecoveryComplete` 现在有 `(…, Failed)` 出边。
        // 缺这条边时，STEP 4 清理阶段失败 → `write_failure` 被状态机拒 → 任务
        // 永远停在 RecoveryComplete，而它仍在 `can_resume_after_interruption` 里
        // （下次启动又被发现 → 再失败），`abandon-task` 在该阶段同样被拒。
        assert!(Stage::RecoveryComplete.can_transition_to(Stage::Failed));
        assert!(Stage::RecoveryComplete.can_transition_to(Stage::Success));
        // 只放开通往终态的出边，不开放倒回中间阶段。
        assert!(!Stage::RecoveryComplete.can_transition_to(Stage::Prepared));
        assert!(!Stage::RecoveryComplete.can_transition_to(Stage::ImageApplied));
    }

    #[test]
    fn pending_task_policy_only_abandons_before_target_writes_and_resumes_persisted_stages() {
        // 【2026-10-18 缺陷修复：ef71fccc 三按钮死锁】`Capturing` **加入**可放弃
        // 集合：该阶段只是把源卷捕获成暂存 WIM，还原目标还没被碰过（目标擦除发生
        // 在 `TargetErased`），而此前它不在集合里会让「存在未完成任务」对话框的
        // 三条路全部走不通（是=重启续跑不推进 / 否=放弃被本策略拒 / 取消=不建任务）。
        for stage in [
            Stage::Prepared,
            Stage::BootRequested,
            Stage::RecoveryStarted,
            Stage::Preflight,
            Stage::Capturing,
        ] {
            assert!(stage.can_abandon_before_target_write(), "{stage:?}");
        }
        for stage in [
            Stage::TargetErased,
            Stage::ImageApplied,
            Stage::BootRepaired,
            Stage::RecoveryComplete,
            Stage::Success,
            Stage::Failed,
        ] {
            assert!(!stage.can_abandon_before_target_write(), "{stage:?}");
        }
        for stage in [
            Stage::BootRequested,
            Stage::RecoveryStarted,
            Stage::Preflight,
            Stage::Capturing,
            Stage::TargetErased,
            Stage::ImageApplied,
            Stage::BootRepaired,
            Stage::RecoveryComplete,
        ] {
            assert!(stage.can_resume_after_interruption(), "{stage:?}");
        }
        assert!(!Stage::Prepared.can_resume_after_interruption());
        assert!(!Stage::Success.can_resume_after_interruption());
        assert!(!Stage::Failed.can_resume_after_interruption());
    }

    #[test]
    fn interrupted_state_transaction_replays_at_each_write_boundary() {
        for written_views in 0..=2 {
            let root = std::env::temp_dir().join(format!("br-state-{}", Uuid::new_v4()));
            let store = TaskStore::new(&root);
            let mut task = backup_task();
            task.status = Stage::RecoveryComplete;
            store.create(&task).unwrap();
            let status = task.transition(Stage::Success).unwrap();
            let journal = store
                .task_dir(&task.task_id)
                .unwrap()
                .join("state-transaction.json");
            write_json_atomic(&journal, &serde_json::json!({"task":task,"status":status})).unwrap();
            if written_views >= 1 {
                write_json_atomic(store.task_path(&task.task_id).unwrap(), &task).unwrap();
            }
            if written_views >= 2 {
                write_json_atomic(store.status_path(&task.task_id).unwrap(), &status).unwrap();
            }
            assert_eq!(store.load(&task.task_id).unwrap().status, Stage::Success);
            let visible: StatusRecord =
                read_json(store.status_path(&task.task_id).unwrap()).unwrap();
            assert_eq!(visible.stage, Stage::Success);
            assert!(!journal.exists());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn retryable_error_and_cleanup_error_preserve_recovery_materials() {
        for stage in [
            Stage::Capturing,
            Stage::TargetErased,
            Stage::ImageApplied,
            Stage::BootRepaired,
            Stage::RecoveryComplete,
        ] {
            let root = std::env::temp_dir().join(format!("br-retry-{}", Uuid::new_v4()));
            let store = TaskStore::new(&root);
            let mut task = backup_task();
            task.status = stage;
            store.create(&task).unwrap();
            let payload = store.task_dir(&task.task_id).unwrap().join("payload");
            fs::create_dir(&payload).unwrap();
            fs::write(payload.join("Recovery.exe"), b"recovery").unwrap();
            for _ in 0..3 {
                store.write_recovery_error(&task, "持续读写错误").unwrap();
            }
            assert_eq!(store.load(&task.task_id).unwrap().status, stage);
            assert!(store.cleanup_task_artifacts(&task.task_id).is_err());
            assert!(
                store
                    .cleanup_terminal_tasks(0)
                    .unwrap()
                    .removed_task_ids
                    .is_empty()
            );
            assert_eq!(fs::read(payload.join("Recovery.exe")).unwrap(), b"recovery");
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn state_transaction_cannot_replace_authorized_target_or_skip_stages() {
        let root = std::env::temp_dir().join(format!("br-state-invalid-{}", Uuid::new_v4()));
        let store = TaskStore::new(&root);
        let task = backup_task();
        store.create(&task).unwrap();
        let journal = store
            .task_dir(&task.task_id)
            .unwrap()
            .join("state-transaction.json");
        let mut changed = task.clone();
        let status = changed.transition(Stage::RecoveryStarted).unwrap();
        changed.source.as_mut().unwrap().partition_guid = "other".into();
        write_json_atomic(
            &journal,
            &serde_json::json!({"task":changed,"status":status}),
        )
        .unwrap();
        assert!(store.load(&task.task_id).is_err());
        fs::remove_file(&journal).unwrap();
        assert_eq!(store.load(&task.task_id).unwrap(), task);
        fs::remove_dir_all(root).unwrap();
    }

    /// 事务里的身份必须与**目录 id**、且事务内 `task`/`status` 两半必须自洽。
    ///
    /// 这段只有断电残留的事务文件才会走到——正常流程 `commit_state` 写出来的两份
    /// 永远自洽，所以它在重构里最容易被"顺手简化掉"（看着像不可能成立的分支）。
    /// 四种不一致各验一次，并且断言**错误码**而不是文案：码是契约，文案会翻译。
    #[test]
    fn state_transaction_rejects_identity_mismatch() {
        for mismatch in ["status_task_id", "task_id", "operation", "stage"] {
            let root = std::env::temp_dir().join(format!("br-state-ident-{}", Uuid::new_v4()));
            let store = TaskStore::new(&root);
            let task = backup_task();
            store.create(&task).unwrap();
            let journal = store
                .task_dir(&task.task_id)
                .unwrap()
                .join("state-transaction.json");

            let mut moved = task.clone();
            let mut status = moved.transition(Stage::RecoveryStarted).unwrap();
            match mismatch {
                "status_task_id" => status.task_id = Uuid::new_v4().to_string(),
                // 任务与状态**一起**换 id，这样只剩「与目录 id 对不上」这一条触发，
                // 否则两半同时报错、分不清是哪一条守住的。
                "task_id" => {
                    let other = Uuid::new_v4().to_string();
                    moved.task_id.clone_from(&other);
                    status.task_id.clone_from(&other);
                }
                "operation" => status.operation = Operation::RestoreExisting,
                _ => status.stage = Stage::Prepared,
            }
            write_json_atomic(&journal, &serde_json::json!({"task":moved,"status":status})).unwrap();

            let error = store.load(&task.task_id).unwrap_err();
            assert_eq!(error.code(), Some("BR-CORE-073"), "{mismatch}: {error}");
            // 拒绝之后必须把现场留在原地，不能顺手删事务文件——否则证据没了，
            // 用户再打开任务只会看到一个"莫名其妙停在旧阶段"的任务。
            assert!(journal.exists(), "{mismatch}: 报错时不得删掉事务文件");
            assert_eq!(
                store.load(&task.task_id).map(|loaded| loaded.status).ok(),
                None,
                "{mismatch}: 事务仍在时不能悄悄忽略它继续读"
            );
            assert_eq!(read_json::<Task>(store.task_path(&task.task_id).unwrap()).unwrap(), task);
            fs::remove_dir_all(root).unwrap();
        }
    }

    /// 事务只能沿阶段图往前推。断电残留的事务若要求从 `Prepared` 一步到 `Success`，
    /// 重放必须拒绝——否则一次文件损坏就能让任务"跳过全部步骤"变成已完成。
    #[test]
    fn state_transaction_rejects_an_illegal_stage_jump() {
        let root = std::env::temp_dir().join(format!("br-state-jump-{}", Uuid::new_v4()));
        let store = TaskStore::new(&root);
        let task = backup_task(); // 盘上停在 Prepared
        store.create(&task).unwrap();
        let journal = store
            .task_dir(&task.task_id)
            .unwrap()
            .join("state-transaction.json");

        let mut jumped = task.clone();
        jumped.status = Stage::Success;
        let status = StatusRecord {
            task_id: jumped.task_id.clone(),
            operation: jumped.operation,
            stage: Stage::Success,
            progress: 100,
            updated: Utc::now(),
            error_code: None,
            error_code_text: None,
            error: None,
        };
        write_json_atomic(&journal, &serde_json::json!({"task":jumped,"status":status})).unwrap();

        let error = store.load(&task.task_id).unwrap_err();
        assert_eq!(error.code(), Some("BR-CORE-074"), "{error}");
        // 盘上两个视图都不能被这次非法重放改写。
        assert_eq!(read_json::<Task>(store.task_path(&task.task_id).unwrap()).unwrap(), task);
        let visible: StatusRecord = read_json(store.status_path(&task.task_id).unwrap()).unwrap();
        assert_eq!(visible.stage, Stage::Prepared);
        fs::remove_dir_all(root).unwrap();
    }

    /// 盘符重排必须被**容忍**：WinRE 挂载后盘符会变，事务里记的盘符与盘上不同是正常事，
    /// 不属于身份篡改。`replay_state_transaction` 里那段把五个 `drive_letter` 归零的
    /// 循环就是为此而存在——谁把它当"多余代码"删掉，这条用例立刻红。
    ///
    /// 两个方向都验：盘上没有盘符→事务里有，以及两边都有但不同。
    #[test]
    fn state_transaction_tolerates_drive_letter_reassignment() {
        for disk_letter in [None, Some('G')] {
            let root = std::env::temp_dir().join(format!("br-state-letter-{}", Uuid::new_v4()));
            let store = TaskStore::new(&root);
            let mut task = backup_task();
            if let Some(letter) = disk_letter {
                // 盘上视图带着旧盘符：还原到另一台机器/重新挂载后完全可能。
                task.source.as_mut().unwrap().drive_letter = Some(letter);
                task.workspace_volume.as_mut().unwrap().drive_letter = Some(letter);
                task.destination.as_mut().unwrap().volume.drive_letter = Some(letter);
            }
            store.create(&task).unwrap();
            let journal = store
                .task_dir(&task.task_id)
                .unwrap()
                .join("state-transaction.json");

            let mut moved = task.clone();
            let status = moved.transition(Stage::RecoveryStarted).unwrap();
            moved.source.as_mut().unwrap().drive_letter = Some('F');
            moved.workspace_volume.as_mut().unwrap().drive_letter = Some('H');
            moved.destination.as_mut().unwrap().volume.drive_letter = Some('W');
            write_json_atomic(&journal, &serde_json::json!({"task":moved,"status":status})).unwrap();

            let loaded = store.load(&task.task_id).expect("盘符变化不该被当成身份篡改");
            assert_eq!(loaded.status, Stage::RecoveryStarted);
            assert_eq!(loaded.source.as_ref().unwrap().drive_letter, Some('F'));
            assert_eq!(loaded.workspace_volume.as_ref().unwrap().drive_letter, Some('H'));
            assert_eq!(
                loaded.destination.as_ref().unwrap().volume.drive_letter,
                Some('W')
            );
            assert!(!journal.exists(), "重放成功后事务文件必须清掉");
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn incomplete_tasks_lists_valid_pending_tasks_and_ignores_terminal_tasks() {
        let root = std::env::temp_dir().join(format!("br-incomplete-{}", Uuid::new_v4()));
        let store = TaskStore::new(&root);
        let mut pending = backup_task();
        store.create(&pending).unwrap();
        store
            .write_transition(&mut pending, Stage::BootRequested)
            .unwrap();

        let mut terminal = backup_task();
        store.create(&terminal).unwrap();
        store
            .write_failure(&mut terminal, 9, "finished failure")
            .unwrap();

        let tasks = store.incomplete_tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_id, pending.task_id);
        assert_eq!(tasks[0].status, Stage::BootRequested);
        fs::remove_dir_all(root).unwrap();
    }

    /// 探测任务停在 `Prepared` 却不能算「未终结」：它带 `--no-reboot`，准备成功即
    /// 完成，没有等待续跑的阶段。算进来会让每次探测都留下一个僵尸任务，之后创建
    /// 任何任务都先弹「是否放弃旧任务」。备份任务同样停在非终态时必须照常列出。
    #[test]
    fn incomplete_tasks_ignores_probe_tasks_but_keeps_real_pending_ones() {
        let root = std::env::temp_dir().join(format!("br-incomplete-probe-{}", Uuid::new_v4()));
        let store = TaskStore::new(&root);
        // 探测任务：只有 operation 不同，阶段与下面的备份任务一致（Prepared）。
        let mut probe = Task::new(
            Operation::Probe,
            BootPlan {
                mode: BootMode::ReturnExisting,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: None,
                boot_sequence_requested: false,
            },
        );
        probe.source = Some(identity("source", 100));
        probe.workspace_volume = Some(identity("task", 100));
        store.create(&probe).unwrap();

        let mut pending = backup_task();
        store.create(&pending).unwrap();
        store
            .write_transition(&mut pending, Stage::BootRequested)
            .unwrap();

        let tasks = store.incomplete_tasks().unwrap();
        assert_eq!(tasks.len(), 1, "探测任务不应算作未终结：{tasks:?}");
        assert_eq!(tasks[0].task_id, pending.task_id);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_tasks_fails_closed_on_malformed_or_inconsistent_records() {
        let root = std::env::temp_dir().join(format!("br-incomplete-invalid-{}", Uuid::new_v4()));
        let store = TaskStore::new(&root);
        let mut task = backup_task();
        store.create(&task).unwrap();
        store
            .write_transition(&mut task, Stage::BootRequested)
            .unwrap();
        let status_path = store.status_path(&task.task_id).unwrap();
        let mut status: StatusRecord = read_json(&status_path).unwrap();
        status.stage = Stage::Prepared;
        write_json_atomic(&status_path, &status).unwrap();
        let error = store.incomplete_tasks().unwrap_err().to_string();
        assert!(error.contains("inconsistent task/status"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_keeps_recent_terminal_tasks_and_skips_incomplete_tasks() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("backuprestore-cleanup-{suffix}"));
        let store = TaskStore::new(&root);
        let mut old = backup_task();
        store.create(&old).unwrap();
        store
            .write_transition(&mut old, Stage::BootRequested)
            .unwrap();
        store
            .write_transition(&mut old, Stage::RecoveryStarted)
            .unwrap();
        store.write_transition(&mut old, Stage::Preflight).unwrap();
        store.write_transition(&mut old, Stage::Capturing).unwrap();
        store.write_transition(&mut old, Stage::Success).unwrap();
        fs::create_dir_all(store.task_dir(&old.task_id).unwrap().join("stage")).unwrap();
        fs::write(
            store
                .task_dir(&old.task_id)
                .unwrap()
                .join("stage")
                .join("Winre.wim"),
            b"large-test-artifact",
        )
        .unwrap();

        let mut recent = backup_task();
        store.create(&recent).unwrap();
        store
            .write_transition(&mut recent, Stage::BootRequested)
            .unwrap();

        let report = store.cleanup_terminal_tasks(0).unwrap();
        assert_eq!(report.removed_task_ids, vec![old.task_id.clone()]);
        assert!(store.task_dir(&old.task_id).unwrap().exists());
        assert!(!store.task_dir(&old.task_id).unwrap().join("stage").exists());
        assert!(store.task_dir(&recent.task_id).unwrap().exists());
        assert_eq!(report.skipped_nonterminal, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn read_json_accepts_windows_powershell_utf8_bom() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("backuprestore-bom-{suffix}.json"));
        fs::write(&path, b"\xEF\xBB\xBF{\"ok\":true}").unwrap();
        let value: serde_json::Value = read_json(&path).unwrap();
        assert_eq!(value["ok"], true);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn atomic_json_overwrite_replaces_a_complete_record() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("backuprestore-atomic-{suffix}.json"));
        write_json_atomic(&path, &serde_json::json!({ "generation": 1 })).unwrap();
        write_json_atomic(&path, &serde_json::json!({ "generation": 2 })).unwrap();
        let value: serde_json::Value = read_json(&path).unwrap();
        assert_eq!(value["generation"], 2);
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn task_store_rejects_mismatched_task_id() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("backuprestore-task-id-{suffix}"));
        let store = TaskStore::new(&root);
        let task = backup_task();
        store.create(&task).unwrap();
        let mut replacement = backup_task();
        replacement.status = Stage::BootRequested;
        write_json_atomic(store.task_path(&task.task_id).unwrap(), &replacement).unwrap();
        let error = store.load(&task.task_id).unwrap_err().to_string();
        assert!(error.contains("does not match requested task id"));
        let _ = fs::remove_dir_all(root);
    }
}
