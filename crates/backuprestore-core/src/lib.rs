//! Shared, platform-independent safety and task model for BackupRestore.
//!
//! The Windows front end and the WinRE recovery host must make the same
//! decisions about volume identity, task transitions and destructive
//! boundaries. This crate deliberately contains no Windows API calls.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn MoveFileExW(existing_file_name: *const u16, new_file_name: *const u16, flags: u32) -> i32;
}

#[cfg(windows)]
const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
#[cfg(windows)]
const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

pub const TASK_VERSION: u32 = 1;
pub const PROGRAM_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Keep a small amount of terminal history for diagnostics without allowing
/// repeated WinRE preparation tests to grow the workspace indefinitely.
pub const DEFAULT_TERMINAL_TASK_RETENTION: usize = 3;

#[derive(Debug, Error)]
pub enum TaskError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid task: {0}")]
    Invalid(String),
    #[error("invalid state transition: {from:?} -> {to:?}")]
    InvalidTransition { from: Stage, to: Stage },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    /// Non-destructive wiring/WinRE probe used by Phase 0 and diagnostics.
    Probe,
    Backup,
    RestoreExisting,
    CreateSecondary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Prepared,
    BootRequested,
    RecoveryStarted,
    Preflight,
    Capturing,
    TargetErased,
    ImageApplied,
    BootRepaired,
    Success,
    Failed,
}

impl Stage {
    pub fn can_transition_to(self, next: Self) -> bool {
        use Stage::*;
        matches!(
            (self, next),
            (Prepared, BootRequested)
                | (Prepared, RecoveryStarted)
                | (Prepared, Failed)
                | (BootRequested, RecoveryStarted)
                | (BootRequested, Failed)
                | (RecoveryStarted, Preflight)
                | (RecoveryStarted, Failed)
                | (Preflight, Capturing)
                | (Preflight, TargetErased)
                | (Preflight, Success)
                | (Preflight, Failed)
                | (Capturing, Success)
                | (Capturing, Failed)
                | (TargetErased, ImageApplied)
                | (TargetErased, Failed)
                | (ImageApplied, BootRepaired)
                | (ImageApplied, Failed)
                | (BootRepaired, Success)
                | (BootRepaired, Failed)
        )
    }
    pub fn is_destructive_boundary(self) -> bool {
        matches!(
            self,
            Self::TargetErased | Self::ImageApplied | Self::BootRepaired
        )
    }
    pub fn is_incomplete(self) -> bool {
        !matches!(self, Self::Success | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeIdentity {
    pub disk_guid: String,
    pub partition_guid: String,
    /// Windows `Get-Volume.UniqueId` / `mountvol` identifier. Optional for
    /// imported old tasks, but new tasks should always record it.
    #[serde(default)]
    pub volume_guid: String,
    #[serde(default)]
    pub partition_type_guid: String,
    #[serde(default)]
    pub disk_number: Option<u32>,
    #[serde(default)]
    pub partition_number: Option<u32>,
    #[serde(default)]
    pub partition_offset: u64,
    #[serde(default)]
    pub partition_size: u64,
    #[serde(default)]
    pub filesystem: String,
    #[serde(default)]
    pub volume_serial: String,
    /// A drive letter is only a temporary Recovery parameter, never identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_letter: Option<char>,
}

impl VolumeIdentity {
    pub fn new(disk_guid: impl Into<String>, partition_guid: impl Into<String>) -> Self {
        Self {
            disk_guid: disk_guid.into(),
            partition_guid: partition_guid.into(),
            volume_guid: String::new(),
            partition_type_guid: String::new(),
            disk_number: None,
            partition_number: None,
            partition_offset: 0,
            partition_size: 0,
            filesystem: String::new(),
            volume_serial: String::new(),
            drive_letter: None,
        }
    }
    pub fn is_complete(&self) -> bool {
        !self.disk_guid.trim().is_empty()
            && !self.partition_guid.trim().is_empty()
            && !self.volume_guid.trim().is_empty()
            && self.disk_number.is_some()
            && self.partition_number.is_some()
            && !self.partition_type_guid.trim().is_empty()
            && !self.filesystem.trim().is_empty()
            && self.partition_size > 0
    }
    pub fn same_partition(&self, other: &Self) -> bool {
        self.disk_guid.eq_ignore_ascii_case(&other.disk_guid)
            && self
                .partition_guid
                .eq_ignore_ascii_case(&other.partition_guid)
    }
    pub fn is_reserved_partition(&self) -> bool {
        let t = self.partition_type_guid.to_ascii_lowercase();
        matches!(
            t.as_str(),
            "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}"
                | "{e3c9e316-0b5c-4db8-817d-f92df00215ae}"
                | "{de94bba4-06d1-4d40-a16a-bfd50179d6ac}"
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSpec {
    pub volume: VolumeIdentity,
    /// User-facing Windows absolute path captured at task preparation time.
    /// Recovery uses `relative_path` only after re-mounting the verified volume
    /// because the drive letter may change in WinRE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_path: Option<String>,
    pub relative_path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DestinationSpec {
    pub volume: VolumeIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_path: Option<String>,
    pub relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetRole {
    ExistingWindows,
    NewWindows,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSpec {
    pub volume: VolumeIdentity,
    pub role: TargetRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_menu_name: Option<String>,
    #[serde(default)]
    pub minimum_size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BootMode {
    ReturnExisting,
    AddSecondary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootPlan {
    pub mode: BootMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_bcd_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu_name: Option<String>,
    #[serde(default)]
    pub boot_sequence_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadManifest {
    pub task_id: String,
    pub recovery_sha256: String,
    pub task_sha256: String,
    pub original_winre_sha256: String,
    pub staged_winre_sha256: String,
    pub created_by_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_task_env_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupMetadata {
    pub version: u32,
    #[serde(rename = "type")]
    pub image_type: String,
    pub created: DateTime<Utc>,
    pub computer: String,
    pub windows_edition: String,
    pub architecture: String,
    pub windows_build: String,
    pub wim_index: u32,
    pub image_sha256: String,
    pub image_size: u64,
    pub source: VolumeIdentity,
    pub captured_used_bytes: u64,
    pub reserved_bytes: u64,
    pub minimum_target_size: u64,
    pub volume_serial: String,
    pub program_version: String,
}
impl BackupMetadata {
    pub fn required_target_size(&self) -> u64 {
        self.minimum_target_size
            .max(self.captured_used_bytes.saturating_add(self.reserved_bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub task_id: String,
    pub version: u32,
    pub operation: Operation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<VolumeIdentity>,
    /// The volume containing the running program directory, tasks and recovery payload.
    /// It is intentionally independent of the source/image volume; only a restore
    /// target may not overwrite it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_volume: Option<VolumeIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<DestinationSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetSpec>,
    pub boot_plan: BootPlan,
    pub created: DateTime<Utc>,
    pub boot_once: bool,
    pub status: Stage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<PayloadManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusRecord {
    pub task_id: String,
    pub operation: Operation,
    pub stage: Stage,
    pub progress: u8,
    pub updated: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Task {
    pub fn new(operation: Operation, boot_plan: BootPlan) -> Self {
        Self {
            task_id: Uuid::new_v4().to_string(),
            version: TASK_VERSION,
            operation,
            source: None,
            workspace_volume: None,
            image: None,
            destination: None,
            target: None,
            boot_plan,
            created: Utc::now(),
            boot_once: true,
            status: Stage::Prepared,
            payload: None,
        }
    }
    pub fn validate(&self) -> Result<(), TaskError> {
        if self.version != TASK_VERSION {
            return Err(TaskError::Invalid(format!(
                "unsupported task version {} (expected {})",
                self.version, TASK_VERSION
            )));
        }
        validate_task_id(&self.task_id)?;
        if !self.boot_once {
            return Err(TaskError::Invalid(
                "recovery tasks must use one-time boot".into(),
            ));
        }
        if self.boot_plan.boot_sequence_requested && self.boot_plan.previous_bcd_sha256.is_none() {
            return Err(TaskError::Invalid(
                "boot sequence requires a previous BCD hash for rollback".into(),
            ));
        }
        if let Some(hash) = self.boot_plan.previous_bcd_sha256.as_deref() {
            validate_sha256_text("previous BCD", hash)?;
        }
        if let Some(payload) = self.payload.as_ref() {
            if payload.task_id != self.task_id {
                return Err(TaskError::Invalid(
                    "payload task id does not match task id".into(),
                ));
            }
            for (name, hash) in [
                ("recovery entry point", payload.recovery_sha256.as_str()),
                ("task", payload.task_sha256.as_str()),
                ("original WinRE", payload.original_winre_sha256.as_str()),
                ("staged WinRE", payload.staged_winre_sha256.as_str()),
            ] {
                validate_sha256_text(name, hash)?;
            }
        }
        match self.operation {
            Operation::Probe => {
                let source = self.source.as_ref().ok_or_else(|| missing("source"))?;
                require_complete("probe source", source)?;
                if self.image.is_some() || self.destination.is_some() || self.target.is_some() {
                    return Err(TaskError::Invalid(
                        "probe task cannot contain image, destination or target".into(),
                    ));
                }
            }
            Operation::Backup => {
                let source = self.source.as_ref().ok_or_else(|| missing("source"))?;
                let dest = self
                    .destination
                    .as_ref()
                    .ok_or_else(|| missing("destination"))?;
                require_complete("source", source)?;
                require_complete("destination volume", &dest.volume)?;
                if source.same_partition(&dest.volume) {
                    return Err(TaskError::Invalid(
                        "backup destination must differ from source partition".into(),
                    ));
                }
                if dest.volume.is_reserved_partition() {
                    return Err(TaskError::Invalid(
                        "EFI/MSR/Recovery partitions cannot store backup images".into(),
                    ));
                }
                if let Some(path) = dest.absolute_path.as_deref() {
                    validate_absolute_path(path)?;
                }
                validate_relative_path(&dest.relative_path)?;
                if self.image.is_some() || self.target.is_some() {
                    return Err(TaskError::Invalid(
                        "backup task cannot contain image or target".into(),
                    ));
                }
            }
            Operation::RestoreExisting | Operation::CreateSecondary => {
                let source = self.source.as_ref().ok_or_else(|| missing("source"))?;
                let image = self.image.as_ref().ok_or_else(|| missing("image"))?;
                let target = self.target.as_ref().ok_or_else(|| missing("target"))?;
                require_complete("source", source)?;
                require_complete("image volume", &image.volume)?;
                require_complete("target volume", &target.volume)?;
                if source.same_partition(&image.volume) {
                    return Err(TaskError::Invalid(
                        "image volume must differ from source volume".into(),
                    ));
                }
                if image.volume.is_reserved_partition() {
                    return Err(TaskError::Invalid(
                        "EFI/MSR/Recovery partitions cannot store restore images".into(),
                    ));
                }
                if let Some(path) = image.absolute_path.as_deref() {
                    validate_absolute_path(path)?;
                }
                if image.sha256.len() != 64 || !image.sha256.chars().all(|c| c.is_ascii_hexdigit())
                {
                    return Err(TaskError::Invalid(
                        "image sha256 must be 64 hex characters".into(),
                    ));
                }
                if image.index == 0 {
                    return Err(TaskError::Invalid(
                        "WIM index must be greater than zero".into(),
                    ));
                }
                if image.size_bytes == 0 {
                    return Err(TaskError::Invalid(
                        "restore image size must be greater than zero".into(),
                    ));
                }
                if target.minimum_size_bytes == 0 {
                    return Err(TaskError::Invalid(
                        "restore target minimum size must be greater than zero".into(),
                    ));
                }
                validate_relative_path(&image.relative_path)?;
                if image.volume.same_partition(&target.volume) {
                    return Err(TaskError::Invalid(
                        "image volume must differ from restore target".into(),
                    ));
                }
                if target.volume.is_reserved_partition() {
                    return Err(TaskError::Invalid(
                        "EFI/MSR/Recovery partitions cannot be restore targets".into(),
                    ));
                }
                match self.operation {
                    Operation::RestoreExisting => {
                        if !source.same_partition(&target.volume) {
                            return Err(TaskError::Invalid(
                                "restore-existing target must be the existing Windows source volume".into(),
                            ));
                        }
                        if target.role != TargetRole::ExistingWindows {
                            return Err(TaskError::Invalid(
                                "restore-existing target role must be existing-windows".into(),
                            ));
                        }
                        if !matches!(self.boot_plan.mode, BootMode::ReturnExisting) {
                            return Err(TaskError::Invalid(
                                "restore-existing must return to the existing boot entry".into(),
                            ));
                        }
                    }
                    Operation::CreateSecondary => {
                        if source.same_partition(&target.volume) {
                            return Err(TaskError::Invalid(
                                "secondary target must differ from the existing Windows source"
                                    .into(),
                            ));
                        }
                        if target.role != TargetRole::NewWindows {
                            return Err(TaskError::Invalid(
                                "create-secondary target role must be new-windows".into(),
                            ));
                        }
                        if target
                            .boot_menu_name
                            .as_deref()
                            .unwrap_or("")
                            .trim()
                            .is_empty()
                        {
                            return Err(TaskError::Invalid(
                                "create-secondary requires a boot menu name".into(),
                            ));
                        }
                        if !valid_menu_name(target.boot_menu_name.as_deref().unwrap_or("")) {
                            return Err(TaskError::Invalid(
                                "create-secondary boot menu name contains control characters or is too long".into(),
                            ));
                        }
                        if !matches!(self.boot_plan.mode, BootMode::AddSecondary) {
                            return Err(TaskError::Invalid(
                                "create-secondary must use add-secondary boot mode".into(),
                            ));
                        }
                        if self
                            .boot_plan
                            .menu_name
                            .as_deref()
                            .unwrap_or("")
                            .trim()
                            .is_empty()
                        {
                            return Err(TaskError::Invalid(
                                "create-secondary requires a boot plan menu name".into(),
                            ));
                        }
                        if !valid_menu_name(self.boot_plan.menu_name.as_deref().unwrap_or("")) {
                            return Err(TaskError::Invalid(
                                "create-secondary boot plan menu name contains control characters or is too long".into(),
                            ));
                        }
                    }
                    Operation::Probe | Operation::Backup => unreachable!(),
                }
            }
        }
        let workspace_volume = self
            .workspace_volume
            .as_ref()
            .ok_or_else(|| missing("workspace volume"))?;
        require_complete("workspace volume", workspace_volume)?;
        if workspace_volume.is_reserved_partition() {
            return Err(TaskError::Invalid(
                "EFI/MSR/Recovery partitions cannot store workspace files".into(),
            ));
        }
        if let Some(target) = self.target.as_ref()
            && workspace_volume.same_partition(&target.volume)
        {
            return Err(TaskError::Invalid(
                "workspace volume must differ from restore target".into(),
            ));
        }
        Ok(())
    }
    pub fn transition(&mut self, next: Stage) -> Result<StatusRecord, TaskError> {
        if !self.status.can_transition_to(next) {
            return Err(TaskError::InvalidTransition {
                from: self.status,
                to: next,
            });
        }
        self.status = next;
        Ok(StatusRecord {
            task_id: self.task_id.clone(),
            operation: self.operation,
            stage: next,
            progress: default_progress(next),
            updated: Utc::now(),
            error_code: None,
            error: None,
        })
    }
}

fn default_progress(stage: Stage) -> u8 {
    match stage {
        Stage::Prepared => 0,
        Stage::BootRequested => 2,
        Stage::RecoveryStarted => 5,
        Stage::Preflight => 10,
        Stage::Capturing => 20,
        Stage::TargetErased => 35,
        Stage::ImageApplied => 75,
        Stage::BootRepaired => 90,
        Stage::Success => 100,
        Stage::Failed => 0,
    }
}
fn missing(name: &str) -> TaskError {
    TaskError::Invalid(format!("missing {name}"))
}
fn valid_menu_name(value: &str) -> bool {
    value.chars().count() <= 256 && !value.chars().any(char::is_control)
}
pub fn validate_task_id(value: &str) -> Result<(), TaskError> {
    canonical_task_id(value).map(|_| ())
}
fn canonical_task_id(value: &str) -> Result<String, TaskError> {
    Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| TaskError::Invalid("task_id is not a UUID".into()))
}
fn validate_sha256_text(name: &str, value: &str) -> Result<(), TaskError> {
    if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(TaskError::Invalid(format!(
            "{name} sha256 must be 64 hex characters"
        )));
    }
    Ok(())
}
fn require_complete(name: &str, identity: &VolumeIdentity) -> Result<(), TaskError> {
    if identity.is_complete() {
        Ok(())
    } else {
        Err(TaskError::Invalid(format!("{name} identity is incomplete")))
    }
}

pub fn validate_relative_path(value: &str) -> Result<(), TaskError> {
    if value.trim().is_empty() || value.contains('\0') || value.chars().any(char::is_control) {
        return Err(TaskError::Invalid(
            "path must be a non-empty relative path".into(),
        ));
    }
    // Tasks are exchanged between Unix tooling and Windows. Normalize the
    // Windows separator before checking so `..\\x` cannot bypass this guard
    // when tests run on macOS.
    let normalized = value.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() || normalized.as_bytes().get(1) == Some(&b':') {
        return Err(TaskError::Invalid(
            "path must be a non-empty relative path".into(),
        ));
    }
    for component in path.components() {
        if matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(TaskError::Invalid(
                "path may not escape the volume root".into(),
            ));
        }
    }
    Ok(())
}

/// Validate a Windows absolute file path supplied by the user.
///
/// The path must use a drive-root form such as `B:\\Backups\\Windows.wim`.
/// UNC and volume-GUID paths are deliberately excluded from the GUI contract;
/// the task still stores the verified volume GUID separately so WinRE can
/// safely re-mount the same partition even if its drive letter changes.
pub fn validate_absolute_path(value: &str) -> Result<(), TaskError> {
    if value.trim().is_empty() || value.contains('\0') || value.chars().any(char::is_control) {
        return Err(TaskError::Invalid(
            "path must be a non-empty Windows absolute path".into(),
        ));
    }
    let normalized = value.replace('/', "\\");
    let bytes = normalized.as_bytes();
    if bytes.len() < 4 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return Err(TaskError::Invalid(
            "path must use a drive-root form such as B:\\Backups\\Windows.wim".into(),
        ));
    }
    let rest = &normalized[3..];
    if rest.ends_with('\\') {
        return Err(TaskError::Invalid(
            "absolute image path must identify a file, not a volume root".into(),
        ));
    }
    for part in rest.split('\\') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(TaskError::Invalid(
                "absolute image path contains an invalid component".into(),
            ));
        }
    }
    Ok(())
}

pub fn sha256_file(path: impl AsRef<Path>) -> Result<String, TaskError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    // Keep the 1 MiB hashing buffer on the heap. Windows ARM64's default
    // process stack is small enough that a stack array can terminate the
    // native Recovery.exe with STATUS_STACK_OVERFLOW.
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
        Err(TaskError::Invalid(format!(
            "sha256 mismatch: expected {expected}, got {actual}"
        )))
    }
}

pub fn verify_image_file(path: impl AsRef<Path>, image: &ImageSpec) -> Result<(), TaskError> {
    let path = path.as_ref();
    let size = fs::metadata(path)?.len();
    if image.size_bytes != 0 && size != image.size_bytes {
        return Err(TaskError::Invalid(format!(
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
        return Err(TaskError::Invalid("payload task id is empty".into()));
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
            return Err(TaskError::Invalid(format!(
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
        .ok_or_else(|| TaskError::Invalid("JSON path has no parent".into()))?;
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
        // Do not delete the old task/status record before installing the new
        // one. A power loss in that gap makes an otherwise recoverable task
        // look absent. The temporary file is in the same directory, so this
        // is a replace-in-place operation on the same NTFS volume.
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
    // Windows PowerShell 5.1's `Set-Content -Encoding UTF8` emits a UTF-8
    // BOM.  WinRE payload JSON is written by that host, so accept the BOM
    // before handing the document to serde_json.
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    Ok(serde_json::from_slice(bytes)?)
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
    pub fn create(&self, task: &Task) -> Result<(), TaskError> {
        task.validate()?;
        let dir = self.task_dir(&task.task_id)?;
        if dir.exists() {
            return Err(TaskError::Invalid(
                "task directory already exists; refusing to overwrite it".into(),
            ));
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
            error: None,
        };
        write_json_atomic(self.status_path(&task.task_id)?, &status)?;
        Ok(())
    }
    pub fn load(&self, task_id: &str) -> Result<Task, TaskError> {
        validate_task_id(task_id)?;
        let task: Task = read_json(self.task_path(task_id)?)?;
        let requested_id = Uuid::parse_str(task_id)
            .map_err(|_| TaskError::Invalid("task_id is not a UUID".into()))?;
        let actual_id = Uuid::parse_str(&task.task_id)
            .map_err(|_| TaskError::Invalid("task file task_id is not a UUID".into()))?;
        if actual_id != requested_id {
            return Err(TaskError::Invalid(
                "task file id does not match requested task id".into(),
            ));
        }
        task.validate()?;
        Ok(task)
    }
    pub fn write_transition(
        &self,
        task: &mut Task,
        next: Stage,
    ) -> Result<StatusRecord, TaskError> {
        let status = task.transition(next)?;
        write_json_atomic(self.task_path(&task.task_id)?, task)?;
        write_json_atomic(self.status_path(&task.task_id)?, &status)?;
        Ok(status)
    }
    pub fn write_failure(
        &self,
        task: &mut Task,
        code: i32,
        error: impl Into<String>,
    ) -> Result<StatusRecord, TaskError> {
        if !task.status.can_transition_to(Stage::Failed) {
            return Err(TaskError::Invalid(format!(
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
            error_code: Some(code),
            error: Some(error.into()),
        };
        write_json_atomic(self.task_path(&task.task_id)?, task)?;
        write_json_atomic(self.status_path(&task.task_id)?, &status)?;
        Ok(status)
    }

    /// Remove only the large WinRE working trees for one terminal task.
    /// Task/status/manifest/log/BCD evidence remains available for diagnosis.
    pub fn cleanup_task_artifacts(&self, task_id: &str) -> Result<bool, TaskError> {
        let path = self.task_dir(task_id)?;
        if !path.is_dir() {
            return Ok(false);
        }
        let mount = path.join("mount");
        if mount.is_dir() && fs::read_dir(&mount)?.next().is_some() {
            return Err(TaskError::Invalid(format!(
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

    /// Remove old terminal task artifacts while preserving recent evidence.
    ///
    /// Only tasks whose task and status records both say `success` or
    /// `failed` are eligible. Incomplete, malformed, or still-mounted tasks
    /// are deliberately left in place for operator recovery. The newest
    /// `keep_latest` terminal tasks are retained.
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
}

pub fn validate_operation(
    operation: Operation,
    source: &VolumeIdentity,
    image: &VolumeIdentity,
    target: &VolumeIdentity,
) -> Result<(), TaskError> {
    require_complete("source", source)?;
    require_complete("image", image)?;
    require_complete("target", target)?;
    if source.same_partition(image) {
        return Err(TaskError::Invalid(
            "image volume must differ from source volume".into(),
        ));
    }
    if matches!(
        operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) && image.same_partition(target)
    {
        return Err(TaskError::Invalid(
            "image volume must differ from restore target".into(),
        ));
    }
    if target.is_reserved_partition() {
        return Err(TaskError::Invalid(
            "reserved partition cannot be target".into(),
        ));
    }
    Ok(())
}
pub fn ensure_capacity(target_size: u64, metadata: &BackupMetadata) -> Result<(), TaskError> {
    let required = metadata.required_target_size();
    if target_size < required {
        return Err(TaskError::Invalid(format!(
            "target disk space insufficient: {target_size} < {required}"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitLockerState {
    Disabled,
    Unlocked,
    Protected,
    Unknown,
}
pub fn ensure_bitlocker_accessible(states: &[BitLockerState]) -> Result<(), TaskError> {
    if states
        .iter()
        .any(|s| matches!(s, BitLockerState::Protected | BitLockerState::Unknown))
    {
        return Err(TaskError::Invalid("BitLocker is enabled or volume accessibility is unknown; suspend/unlock manually before V1 recovery".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
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
    fn state_machine_accepts_recovery_paths() {
        assert!(Stage::Prepared.can_transition_to(Stage::BootRequested));
        assert!(Stage::Prepared.can_transition_to(Stage::Failed));
        assert!(Stage::Preflight.can_transition_to(Stage::Capturing));
        assert!(Stage::Preflight.can_transition_to(Stage::TargetErased));
        assert!(Stage::Preflight.can_transition_to(Stage::Success));
        assert!(Stage::ImageApplied.can_transition_to(Stage::BootRepaired));
        assert!(!Stage::Prepared.can_transition_to(Stage::Success));
    }
    #[test]
    fn backup_rejects_same_partition() {
        let mut task = backup_task();
        task.destination.as_mut().unwrap().volume.partition_guid = "source".into();
        assert!(task.validate().is_err());
    }
    #[test]
    fn arbitrary_drive_letters_are_temporary_hints() {
        let mut task = backup_task();
        task.source.as_mut().unwrap().drive_letter = Some('S');
        task.workspace_volume.as_mut().unwrap().drive_letter = Some('T');
        task.destination.as_mut().unwrap().volume.drive_letter = Some('B');
        assert!(task.validate().is_ok());

        let mut moved = task.clone();
        moved.source.as_mut().unwrap().drive_letter = Some('D');
        moved.destination.as_mut().unwrap().volume.drive_letter = Some('I');
        assert!(moved.validate().is_ok());
        assert!(
            task.source
                .as_ref()
                .unwrap()
                .same_partition(moved.source.as_ref().unwrap())
        );
    }
    #[test]
    fn workspace_volume_rejects_reserved_and_target_overlap() {
        let mut task = backup_task();
        task.workspace_volume.as_mut().unwrap().partition_type_guid =
            "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}".into();
        assert!(task.validate().is_err());

        let mut task = backup_task();
        task.workspace_volume.as_mut().unwrap().partition_guid = "source".into();
        assert!(task.validate().is_ok());

        let mut task = backup_task();
        task.workspace_volume.as_mut().unwrap().partition_guid = "image".into();
        assert!(task.validate().is_ok());

        let mut task = Task::new(
            Operation::RestoreExisting,
            BootPlan {
                mode: BootMode::ReturnExisting,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: None,
                boot_sequence_requested: true,
            },
        );
        task.source = Some(identity("source", 200));
        task.workspace_volume = Some(identity("source", 200));
        task.image = Some(ImageSpec {
            volume: identity("image", 100),
            absolute_path: None,
            relative_path: "backup.wim".into(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            index: 1,
        });
        task.target = Some(TargetSpec {
            volume: identity("source", 200),
            role: TargetRole::ExistingWindows,
            boot_menu_name: None,
            minimum_size_bytes: 100,
        });
        assert!(task.validate().is_err());
    }
    #[test]
    fn task_rejects_incomplete_workspace_identity() {
        let mut task = backup_task();
        task.workspace_volume
            .as_mut()
            .unwrap()
            .partition_type_guid
            .clear();
        assert!(task.validate().is_err());
        task.workspace_volume.as_mut().unwrap().partition_type_guid =
            "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}".into();
        task.workspace_volume.as_mut().unwrap().volume_guid.clear();
        assert!(task.validate().is_err());
    }
    #[test]
    fn menu_name_rejects_controls_and_unbounded_input() {
        assert!(valid_menu_name("Windows Backup"));
        assert!(!valid_menu_name("Windows\nBackup"));
        assert!(!valid_menu_name(&"x".repeat(257)));
    }
    #[test]
    fn reserved_volumes_and_dot_paths_are_rejected() {
        let mut backup = backup_task();
        backup
            .destination
            .as_mut()
            .unwrap()
            .volume
            .partition_type_guid = "{de94bba4-06d1-4d40-a16a-bfd50179d6ac}".into();
        assert!(backup.validate().is_err());
        assert!(validate_relative_path(".").is_err());
        assert!(validate_relative_path(".\\Windows.wim").is_err());

        let source = identity("source", 100);
        let mut image = identity("image", 100);
        image.partition_type_guid = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}".into();
        let target = identity("source", 100);
        let mut task = Task::new(
            Operation::RestoreExisting,
            BootPlan {
                mode: BootMode::ReturnExisting,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: None,
                boot_sequence_requested: true,
            },
        );
        task.source = Some(source);
        task.workspace_volume = Some(identity("workspace", 200));
        task.image = Some(ImageSpec {
            volume: image,
            absolute_path: None,
            relative_path: "backup.wim".into(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            index: 1,
        });
        task.target = Some(TargetSpec {
            volume: target,
            role: TargetRole::ExistingWindows,
            boot_menu_name: None,
            minimum_size_bytes: 100,
        });
        assert!(task.validate().is_err());
    }

    #[test]
    fn absolute_image_paths_require_a_drive_root() {
        assert!(validate_absolute_path(r"B:\Backups\Windows.wim").is_ok());
        assert!(validate_absolute_path(r"B:/Backups/Windows.wim").is_ok());
        assert!(validate_absolute_path(r"Backups\Windows.wim").is_err());
        assert!(validate_absolute_path(r"B:\Backups\..\Windows.wim").is_err());
        assert!(validate_absolute_path(r"B:\").is_err());

        let mut task = backup_task();
        task.destination.as_mut().unwrap().absolute_path = Some("backup.wim".into());
        assert!(task.validate().is_err());
        task.destination.as_mut().unwrap().absolute_path = Some(r"B:\Backups\Windows.wim".into());
        assert!(task.validate().is_ok());
    }
    #[test]
    fn restore_rejects_image_on_target_and_reserved_target() {
        let source = identity("source", 100);
        let image = identity("image", 100);
        assert!(validate_operation(Operation::RestoreExisting, &source, &image, &image).is_err());
        let mut efi = identity("efi", 100);
        efi.partition_type_guid = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}".into();
        assert!(validate_operation(Operation::RestoreExisting, &source, &image, &efi).is_err());
    }
    #[test]
    fn secondary_requires_explicit_name_and_mode() {
        let mut task = Task::new(
            Operation::CreateSecondary,
            BootPlan {
                mode: BootMode::AddSecondary,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: Some("Windows 11 Backup".into()),
                boot_sequence_requested: true,
            },
        );
        task.image = Some(ImageSpec {
            volume: identity("image", 100),
            absolute_path: None,
            relative_path: "backup.wim".into(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            index: 1,
        });
        task.target = Some(TargetSpec {
            volume: identity("target", 200),
            role: TargetRole::NewWindows,
            boot_menu_name: Some("Windows 11 Backup".into()),
            minimum_size_bytes: 100,
        });
        task.source = Some(identity("source", 200));
        task.workspace_volume = Some(identity("workspace", 200));
        assert!(task.validate().is_ok());
        task.target.as_mut().unwrap().boot_menu_name = None;
        assert!(task.validate().is_err());
    }
    #[test]
    fn restore_requires_source_size_and_target_capacity() {
        let mut task = Task::new(
            Operation::RestoreExisting,
            BootPlan {
                mode: BootMode::ReturnExisting,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: None,
                boot_sequence_requested: true,
            },
        );
        task.source = Some(identity("source", 200));
        task.workspace_volume = Some(identity("workspace", 200));
        task.image = Some(ImageSpec {
            volume: identity("image", 100),
            absolute_path: None,
            relative_path: "backup.wim".into(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            index: 1,
        });
        task.target = Some(TargetSpec {
            volume: identity("source", 200),
            role: TargetRole::ExistingWindows,
            boot_menu_name: None,
            minimum_size_bytes: 100,
        });
        assert!(task.validate().is_ok());
        task.source = None;
        assert!(task.validate().is_err());
        task.source = Some(identity("source", 200));
        task.image.as_mut().unwrap().size_bytes = 0;
        assert!(task.validate().is_err());
        task.image.as_mut().unwrap().size_bytes = 1;
        task.target.as_mut().unwrap().minimum_size_bytes = 0;
        assert!(task.validate().is_err());
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

    #[test]
    fn path_and_capacity_guards() {
        assert!(validate_relative_path("x\\y\\z.wim").is_ok());
        assert!(validate_relative_path("..\\evil.wim").is_err());
        let metadata = BackupMetadata {
            version: 1,
            image_type: "wim".into(),
            created: Utc::now(),
            computer: "test".into(),
            windows_edition: "Pro".into(),
            architecture: "amd64".into(),
            windows_build: "1".into(),
            wim_index: 1,
            image_sha256: "a".repeat(64),
            image_size: 1,
            source: identity("source", 100),
            captured_used_bytes: 80,
            reserved_bytes: 20,
            minimum_target_size: 90,
            volume_serial: "x".into(),
            program_version: "0.0.0".into(),
        };
        assert_eq!(metadata.required_target_size(), 100);
        assert!(ensure_capacity(99, &metadata).is_err());
        assert!(ensure_capacity(100, &metadata).is_ok());
    }
}
