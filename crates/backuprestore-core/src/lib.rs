//! Shared, platform-independent safety and task model for BackupRestore.
//!
//! The Windows front end and the WinRE recovery host must make the same
//! decisions about volume identity, task transitions and destructive
//! boundaries. This crate deliberately contains no Windows API calls.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
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

/// 卷角色冲突：某个任务角色所在分区与「承载注册 WinRE 的分区」是同一个。
///
/// 这台机器上 WinRE 可能注册在 OS 分区（`C:\Recovery\WindowsRE`）而不是独立
/// 的 GPT Recovery 分区，因此「系统盘」与「恢复环境宿主卷」常常是同一分区。
/// F1（还原目标）与 F3（备份源）都是这个冲突的不同侧面，共用一个枚举可以让
/// GUI / CLI / Recovery 三处拿到同一套错误语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeRoleConflict {
    /// 程序目录（工作区）在承载注册 WinRE 的分区上。
    WorkspaceOnRegisteredWinre,
    /// 镜像存放卷在承载注册 WinRE 的分区上。
    ImageOnRegisteredWinre,
    /// 备份源卷在承载注册 WinRE 的分区上（F3：镜像会被注入后的 Winre.wim 污染）。
    BackupSourceOnRegisteredWinre,
    /// 还原目标卷在承载注册 WinRE 的分区上（F1：格式化会摧毁恢复环境与回滚副本）。
    RestoreTargetOnRegisteredWinre,
}

impl VolumeRoleConflict {
    /// 稳定错误码，供日志、文档与 GUI 做分支判断，不随文案变化。
    pub fn code(self) -> &'static str {
        match self {
            Self::WorkspaceOnRegisteredWinre => "workspace-on-registered-winre",
            Self::ImageOnRegisteredWinre => "image-on-registered-winre",
            Self::BackupSourceOnRegisteredWinre => "backup-source-on-registered-winre",
            Self::RestoreTargetOnRegisteredWinre => "restore-target-on-registered-winre",
        }
    }
    /// 面向用户的中文说明。这是硬性阻断，必须说清「为什么」和「下一步怎么办」，
    /// 不能只说「把程序移走」（移走程序解决不了目标/源与恢复环境同分区的问题）。
    pub fn message(self) -> &'static str {
        match self {
            Self::WorkspaceOnRegisteredWinre => {
                "程序目录不能位于承载 Windows 恢复环境（WinRE）的分区：任务目录与恢复镜像同分区时，格式化或回滚会互相破坏。请把程序移到其它卷再运行。"
            }
            Self::ImageOnRegisteredWinre => {
                "镜像存放位置不能位于承载 Windows 恢复环境（WinRE）的分区。请更换镜像保存卷。"
            }
            Self::BackupSourceOnRegisteredWinre => {
                "备份源分区承载当前 Windows 恢复环境（WinRE）：离线备份需要临时改写注册的 Winre.wim，该副本会被一起捕获进镜像，产出带上次任务残留的脏镜像。请改用在线备份（--no-reboot），或先把 WinRE 迁移到独立恢复分区。"
            }
            Self::RestoreTargetOnRegisteredWinre => {
                "还原目标分区承载当前 Windows 恢复环境（WinRE）：格式化会删除 Winre.wim 及用于回滚的原件副本，系统将失去恢复环境且无法自动回滚。请先把 WinRE 迁出该分区，或选择其它还原目标。"
            }
        }
    }
}

/// 参与角色校验的四个卷。`source`/`target` 按操作类型可选：
/// 备份没有目标，还原的源与目标都必须给全。
#[derive(Debug, Clone, Copy)]
pub struct VolumeRoles<'a> {
    pub workspace: &'a VolumeIdentity,
    pub image: &'a VolumeIdentity,
    pub source: Option<&'a VolumeIdentity>,
    pub target: Option<&'a VolumeIdentity>,
}

/// 校验「任务角色卷」与「承载注册 WinRE 的卷」之间的关系（F1/F3 第一段防御）。
///
/// 判定只用 `disk_guid + partition_guid`（`same_partition`），不依赖盘符：
/// WinRE 里盘符会变，盘符相等不代表同一分区，分区 GUID 才是身份。
///
/// `mutates_registered_winre`：本次准备是否会把注入后的 WIM 覆盖回系统注册位置。
/// 只有这种注入模式才会污染镜像（`--no-reboot` 在线路径不改写注册 WIM，
/// 因此备份源与恢复环境同分区时是安全的，不应拒绝）。
///
/// `restore_clean_winre_before_capture`：方案 D 是否已落地——捕获之前先把源卷上的
/// 注册 WIM 覆写回任务暂存的那份干净原件，再让 DISM 捕获。此时磁盘上的注册 WIM
/// 已经是干净的，同一分区不再构成污染，因此放行；捕获前的换回若失败，执行层会
/// 直接终止任务（硬失败），不会静默产出脏镜像。传 `false` 表示仍在旧行为下，
/// 保留拒绝。
pub fn validate_volume_roles(
    roles: &VolumeRoles<'_>,
    operation: Operation,
    recovery: &VolumeIdentity,
    mutates_registered_winre: bool,
    restore_clean_winre_before_capture: bool,
) -> Result<(), VolumeRoleConflict> {
    if roles.workspace.same_partition(recovery) {
        return Err(VolumeRoleConflict::WorkspaceOnRegisteredWinre);
    }
    if roles.image.same_partition(recovery) {
        return Err(VolumeRoleConflict::ImageOnRegisteredWinre);
    }
    match operation {
        Operation::Backup => {
            // F3：源卷 == 恢复环境宿主卷，且本次会把注入后的 WIM 写回注册位置，
            // 且没有「捕获前换回干净原件」这一步来消除污染窗口。
            if mutates_registered_winre
                && !restore_clean_winre_before_capture
                && roles
                    .source
                    .is_some_and(|source| source.same_partition(recovery))
            {
                return Err(VolumeRoleConflict::BackupSourceOnRegisteredWinre);
            }
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            // F1：目标卷 == 恢复环境宿主卷，格式化会摧毁 WinRE 与其回滚副本。
            if roles
                .target
                .is_some_and(|target| target.same_partition(recovery))
            {
                return Err(VolumeRoleConflict::RestoreTargetOnRegisteredWinre);
            }
        }
        Operation::Probe => {}
    }
    Ok(())
}

/// 方案 D 开关：捕获前把源卷上的注册 WinRE 覆写回任务暂存的干净原件。
///
/// 设计、实测依据与回退方式见 `docs/winre-task-wim-phase2-2026-09-27.md` 第 9.5 节。
/// 置 `false` 即回到「备份源 == 恢复环境宿主分区时直接拒绝」的旧行为（F3 第一段），
/// 作为线上出问题时的一键回退，不需要改任何调用点。
pub const PLAN_D_RESTORE_CLEAN_WINRE_BEFORE_CAPTURE: bool = true;

/// 方案 D 判定：这次捕获是否需要先把源卷上的注册 WinRE 覆写回干净原件。
///
/// 成立条件只有一个：**备份源卷就是承载注册 WinRE 的那个卷**。此时注册 WIM 必然位于
/// `<源卷>:\Recovery\WindowsRE\Winre.wim`，而它此刻是本次任务注入后的副本——直接捕获
/// 会把任务残留（`Recovery.exe`/`task.json`/`RecoveryTask.env`）一起打进镜像（F3）。
///
/// 其它布局下源卷上根本没有注册 WIM，无需也不应改动任何系统文件。
/// 恢复环境身份缺失（旧任务、env 缺键）时返回 `false`：拿不到身份就不要动系统文件。
pub fn capture_source_hosts_registered_winre(
    source: &VolumeIdentity,
    recovery: Option<&VolumeIdentity>,
) -> bool {
    recovery.is_some_and(|recovery| source.same_partition(recovery))
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
    /// WIM 索引名（DISM /Name），备份时写入索引；用户可在 GUI 修改。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
    /// 备份开始时间（WinRE 里开始 DISM 捕获的时刻），旧 v2 元数据无此字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<DateTime<Utc>>,
    /// 备份总耗时（秒），旧 v2 元数据无此字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u64>,
    /// 备份平均速度（字节/秒），旧 v2 元数据无此字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_per_sec: Option<u64>,
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
    /// WIM 压缩率（fast/none），仅备份首次创建时生效；追加备份沿用已有压缩。
    /// `max` 已从产品功能中删除，旧任务中的该值必须拒绝执行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress: Option<String>,
    /// 保留最近 N 个 WIM 索引：备份追加成功后删除更旧索引（0/None=不清理）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_indexes: Option<u32>,
    /// 备份索引名（DISM /Name；None 时使用默认 "Windows Backup"）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_name: Option<String>,
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
            compress: None,
            keep_indexes: None,
            image_name: None,
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
        if let Some(compression) = self.compress.as_deref() {
            canonical_compression(compression)?;
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
/// Normalize the only two supported WIM compression choices.
///
/// `fast` is the product's "压缩" choice and `none` is "不压缩". The former
/// maximum-compression mode is deliberately rejected here so GUI, CLI and old
/// task files cannot accidentally pass it to DISM through a fallback path.
pub fn canonical_compression(value: &str) -> Result<&'static str, TaskError> {
    match value {
        "fast" => Ok("fast"),
        "none" => Ok("none"),
        _ => Err(TaskError::Invalid(
            "compression must be fast or none; max compression is not supported".into(),
        )),
    }
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

/// 生成 DISM `/ConfigFile` 配置文件内容（WimScript.ini 语法）。
///
/// 用途：备份捕获时可排除 \$Recycle.Bin、临时目录、更新缓存、浏览器缓存等
/// 可再生内容，缩小 WIM 镜像体积。
///
/// DISM 配置文件的排除表是"根路径锚定"的（带前导 `\` 只匹配根下精确路径），
/// 且通配符只能出现在"不以反斜杠开头的路径的最后一段"。因此 `\Users\*\
/// AppData\...` 这类中间通配符不在规范内，浏览器缓存目录必须在捕获时
/// 枚举真实的 `Users\<用户>\AppData\Local\...` 字面路径生成。
///
/// `source_root` 是待捕获卷的根（如 `X:\`，要求是物理卷根，枚举时不要求
/// 该卷可写）。返回的文本写入后传给：
/// `dism /Capture-Image ... /ConfigFile:<path>` 或
/// `dism /Append-Image ... /ConfigFile:<path>`。
pub fn build_capture_exclusions(source_root: &Path) -> Result<String, TaskError> {
    // 固定的根级可排除项（全部为 DISM 规范内的根路径锚定写法，无通配符）。
    // 注意：hiberfil.sys/pagefile.sys/swapfile.sys/\System Volume Information
    // 由 DISM 默认排除，无需在此重复。
    let mut lines = vec![
        // Parallels 在每个卷根创建同名占位文件并**始终独占持有**它。它不属于
        // 用户数据，却会让 `dism /Apply-Image` 在重建该文件时以
        // ERROR_SHARING_VIOLATION(0x80070020) 失败（实测 v1.7.4 在 T: 还原
        // 72% 处中断）。DISM 在打开文件之前就按排除表跳过，因此排掉它即可
        // 免除"还原前必须停掉 Parallels Tools Service"这一前置条件。
        // 实测排除匹配不区分大小写（VERDICT=CASE_INSENSITIVE）。
        // 非 Parallels 环境不存在该条目，排除它无副作用。
        "\\Mac disk".to_string(),
        "\\$Recycle.Bin".to_string(),
        "\\$WINDOWS.~BT".to_string(),
        "\\$WINDOWS.~WS".to_string(),
        "\\Windows.old".to_string(),
        "\\Temp".to_string(),
        "\\Windows\\Temp".to_string(),
        "\\Windows\\SoftwareDistribution\\Download".to_string(),
        "\\Windows\\Prefetch".to_string(),
        "\\Windows\\Logs".to_string(),
        "\\Windows\\Panther".to_string(),
        "\\ProgramData\\Microsoft\\Windows\\WER".to_string(),
    ];

    // 枚举真实用户配置文件，补上按用户字面路径的排除项。
    let users_root = source_root.join("Users");
    if let Ok(profiles) = fs::read_dir(&users_root) {
        let mut user_entries: Vec<String> = Vec::new();
        for profile in profiles.flatten() {
            let Some(name) = profile.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let local = users_root.join(&name).join("AppData").join("Local");
            // 每个用户自己的临时目录
            user_entries.push(format!("\\Users\\{name}\\AppData\\Local\\Temp"));
            // Chromium 系浏览器的媒体缓存目录（User Data 下每个配置子目录）
            for child in [
                "Google\\Chrome",
                "Microsoft\\Edge",
                "BraveSoftware\\Brave-Browser",
                "Vivaldi",
            ] {
                let mut base = local.clone();
                for part in child.split('\\') {
                    base = base.join(part);
                }
                let user_data = base.join("User Data");
                for profile_dir in read_subdirs(&user_data) {
                    user_entries.push(format!(
                        "\\Users\\{name}\\AppData\\Local\\{child}\\User Data\\{profile_dir}\\Cache"
                    ));
                    user_entries.push(format!(
                        "\\Users\\{name}\\AppData\\Local\\{child}\\User Data\\{profile_dir}\\Code Cache"
                    ));
                    user_entries.push(format!(
                        "\\Users\\{name}\\AppData\\Local\\{child}\\User Data\\{profile_dir}\\GPUCache"
                    ));
                    user_entries.push(format!(
                        "\\Users\\{name}\\AppData\\Local\\{child}\\User Data\\{profile_dir}\\Service Worker\\CacheStorage"
                    ));
                    user_entries.push(format!(
                        "\\Users\\{name}\\AppData\\Local\\{child}\\User Data\\{profile_dir}\\Service Worker\\ScriptCache"
                    ));
                }
            }
            // Opera：稳定配置文件直接位于 Opera Stable 下
            let opera = local.join("Opera Software").join("Opera Stable");
            if opera.is_dir() {
                user_entries.extend([
                    format!("\\Users\\{name}\\AppData\\Local\\Opera Software\\Opera Stable\\Cache"),
                    format!(
                        "\\Users\\{name}\\AppData\\Local\\Opera Software\\Opera Stable\\GPUCache"
                    ),
                ]);
            }
            // Firefox：Profiles 下每个配置的缓存目录
            let firefox = local.join("Mozilla").join("Firefox").join("Profiles");
            for profile_dir in read_subdirs(&firefox) {
                user_entries.extend([
                    format!(
                        "\\Users\\{name}\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\{profile_dir}\\cache2"
                    ),
                    format!(
                        "\\Users\\{name}\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\{profile_dir}\\cache2\\entries"
                    ),
                    format!(
                        "\\Users\\{name}\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\{profile_dir}\\OfflineCache"
                    ),
                    format!(
                        "\\Users\\{name}\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\{profile_dir}\\startupCache"
                    ),
                ]);
            }
            // 旧版 IE / 系统组件缓存
            let inet_cache = local.join("Microsoft").join("Windows").join("INetCache");
            if inet_cache.is_dir() {
                user_entries.push(format!(
                    "\\Users\\{name}\\AppData\\Local\\Microsoft\\Windows\\INetCache"
                ));
            }
        }
        user_entries.sort();
        user_entries.dedup();
        lines.extend(user_entries);
    }

    let mut text = String::from("[ExclusionList]\r\n");
    for line in lines {
        text.push_str(&line);
        text.push_str("\r\n");
    }
    Ok(text)
}

/// 生成 DISM 排除配置并落盘，返回可传给 `/ConfigFile:` 的路径。
///
/// **三个捕获入口必须共用本函数**：CLI 备份、GUI 在线备份、PE 备份。
/// 历史上 GUI 在线备份自己拼 `/Capture-Image`，漏掉了 `/ConfigFile`，
/// 于是"看起来有排除功能、实际一条都没生效"——v1.7.4 的 T: 备份因此把
/// Parallels 的 `\Mac disk` 装进镜像，还原时撞 0x80070020。
///
/// `source_root` 是待捕获卷的根（如 `T:\`）。
pub fn write_capture_exclusion_config(source_root: &Path) -> Result<PathBuf, TaskError> {
    let text = build_capture_exclusions(source_root)?;
    let path = env::temp_dir().join("BackupRestore-exclusions.ini");
    fs::write(&path, text)?;
    Ok(path)
}

/// 列出 `root` 下的一级子目录名（失败时返回空），供枚举浏览器配置目录使用。
fn read_subdirs(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_owned());
        }
    }
    names
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
    /// 构造一次角色校验：四个角色默认各占一个不同分区，"recovery" 固定为恢复环境宿主。
    fn roles<'a>(
        workspace: &'a VolumeIdentity,
        image: &'a VolumeIdentity,
        source: Option<&'a VolumeIdentity>,
        target: Option<&'a VolumeIdentity>,
    ) -> VolumeRoles<'a> {
        VolumeRoles {
            workspace,
            image,
            source,
            target,
        }
    }
    #[test]
    fn volume_roles_reject_workspace_and_image_on_registered_winre() {
        let recovery = identity("recovery", 100);
        let workspace = identity("workspace", 100);
        let image = identity("image", 100);
        let source = identity("source", 100);
        // 工作区落在恢复环境宿主分区上：任何操作都拒绝。
        assert_eq!(
            validate_volume_roles(
                &roles(&recovery, &image, Some(&source), None),
                Operation::Backup,
                &recovery,
                true,
                false
            ),
            Err(VolumeRoleConflict::WorkspaceOnRegisteredWinre)
        );
        // 镜像卷落在恢复环境宿主分区上：同样任何操作都拒绝。
        assert_eq!(
            validate_volume_roles(
                &roles(&workspace, &recovery, Some(&source), None),
                Operation::Backup,
                &recovery,
                true,
                false
            ),
            Err(VolumeRoleConflict::ImageOnRegisteredWinre)
        );
    }
    #[test]
    fn f1_restore_target_on_registered_winre_is_rejected() {
        let recovery = identity("recovery", 100);
        let workspace = identity("workspace", 100);
        let image = identity("image", 100);
        let source = identity("source", 100);
        let target = identity("target", 100);
        // 目标 != 恢复环境宿主：放行（正常布局）。
        assert!(
            validate_volume_roles(
                &roles(&workspace, &image, Some(&source), Some(&target)),
                Operation::RestoreExisting,
                &recovery,
                true,
                false
            )
            .is_ok()
        );
        // 目标 == 恢复环境宿主：F1 第一段拒绝，与是否改写注册 WIM 无关。
        for mutates in [true, false] {
            assert_eq!(
                validate_volume_roles(
                    &roles(&workspace, &image, Some(&source), Some(&recovery)),
                    Operation::CreateSecondary,
                    &recovery,
                    mutates,
                    false
                ),
                Err(VolumeRoleConflict::RestoreTargetOnRegisteredWinre)
            );
        }
    }
    #[test]
    fn f3_backup_source_on_registered_winre_depends_on_winre_mutation() {
        let recovery = identity("recovery", 100);
        let workspace = identity("workspace", 100);
        let image = identity("image", 100);
        let source = identity("source", 100);
        // 源 == 恢复环境宿主 + 旧注入模式（会写回注册 WIM）→ 拒绝，防脏镜像。
        assert_eq!(
            validate_volume_roles(
                &roles(&workspace, &image, Some(&recovery), None),
                Operation::Backup,
                &recovery,
                true,
                false
            ),
            Err(VolumeRoleConflict::BackupSourceOnRegisteredWinre)
        );
        // 同一布局但 --no-reboot（不改写注册 WIM）→ 放行，因为没有污染窗口。
        assert!(
            validate_volume_roles(
                &roles(&workspace, &image, Some(&recovery), None),
                Operation::Backup,
                &recovery,
                false,
                false
            )
            .is_ok()
        );
        // 源 != 恢复环境宿主 → 放行。
        assert!(
            validate_volume_roles(
                &roles(&workspace, &image, Some(&source), None),
                Operation::Backup,
                &recovery,
                true,
                false
            )
            .is_ok()
        );
    }
    #[test]
    fn f3_plan_d_restore_clean_winre_before_capture_unblocks_backup() {
        let recovery = identity("recovery", 100);
        let workspace = identity("workspace", 100);
        let image = identity("image", 100);
        let source = identity("source", 100);
        // 方案 D 未落地（旧行为）：源 == 恢复环境宿主 + 会改写注册 WIM → 仍然拒绝。
        assert_eq!(
            validate_volume_roles(
                &roles(&workspace, &image, Some(&recovery), None),
                Operation::Backup,
                &recovery,
                true,
                false
            ),
            Err(VolumeRoleConflict::BackupSourceOnRegisteredWinre)
        );
        // 方案 D 已落地：捕获前会把注册 WIM 换回干净原件，污染窗口消失 → 放行。
        assert!(
            validate_volume_roles(
                &roles(&workspace, &image, Some(&recovery), None),
                Operation::Backup,
                &recovery,
                true,
                true
            )
            .is_ok()
        );
        // 方案 D 不改变 F1：还原目标 == 恢复环境宿主依然拒绝。
        assert_eq!(
            validate_volume_roles(
                &roles(&workspace, &image, Some(&source), Some(&recovery)),
                Operation::RestoreExisting,
                &recovery,
                true,
                true
            ),
            Err(VolumeRoleConflict::RestoreTargetOnRegisteredWinre)
        );
        // 方案 D 不改变工作区/镜像卷两条硬约束。
        assert_eq!(
            validate_volume_roles(
                &roles(&workspace, &recovery, Some(&source), None),
                Operation::Backup,
                &recovery,
                true,
                true
            ),
            Err(VolumeRoleConflict::ImageOnRegisteredWinre)
        );
    }
    #[test]
    fn capture_source_hosts_registered_winre_matches_only_source_partition() {
        let recovery = identity("recovery", 100);
        let source = identity("source", 100);
        // 源 == 恢复环境宿主 → 捕获前必须换回干净原件。
        assert!(capture_source_hosts_registered_winre(
            &recovery,
            Some(&recovery)
        ));
        // 源 != 恢复环境宿主 → 不需要动系统文件。
        assert!(!capture_source_hosts_registered_winre(
            &source,
            Some(&recovery)
        ));
        // 拿不到恢复环境身份（旧任务/env 缺键）→ 不动系统文件。
        assert!(!capture_source_hosts_registered_winre(&recovery, None));
    }
    #[test]
    fn f1_and_f3_do_not_interfere() {
        let recovery = identity("recovery", 100);
        let workspace = identity("workspace", 100);
        let image = identity("image", 100);
        let source = identity("source", 100);
        let target = identity("target", 100);
        // 备份时即使传了 target（GUI 复用字段）也不应触发 F1。
        assert!(
            validate_volume_roles(
                &roles(&workspace, &image, Some(&source), Some(&recovery)),
                Operation::Backup,
                &recovery,
                true,
                false
            )
            .is_ok()
        );
        // 还原时源 == 恢复环境宿主也不应触发 F3（F3 只约束备份）。
        assert!(
            validate_volume_roles(
                &roles(&workspace, &image, Some(&recovery), Some(&target)),
                Operation::RestoreExisting,
                &recovery,
                true,
                false
            )
            .is_ok()
        );
    }
    /// 方案 D（捕获前换回干净原件）落地后，备份源 == 承载注册 WinRE 的分区
    /// 不再是硬拒绝：污染窗口已被换回动作消除。本测试锁定这一行为翻转，
    /// 并确认「关掉方案 D 开关」能立刻恢复旧的 F3 拒绝（线上回退路径）。
    #[test]
    fn plan_d_opens_f3_when_source_hosts_registered_winre() {
        let recovery = identity("recovery", 100);
        let workspace = identity("workspace", 100);
        let image = identity("image", 100);
        // 源与恢复环境宿主是同一分区（same_partition 只看 disk+partition GUID）。
        let source_eq_recovery = identity("recovery", 100);

        // 方案 D 开启：源 == 恢复宿主，且本次会改写注册 WIM（mutates=true），仍放行。
        assert!(
            validate_volume_roles(
                &roles(&workspace, &image, Some(&source_eq_recovery), None),
                Operation::Backup,
                &recovery,
                true,
                true
            )
            .is_ok(),
            "方案 D 开启时应放开 F3（源==恢复宿主）"
        );

        // 方案 D 关闭（回退到旧行为）：同样的布局必须仍然拒绝。
        assert_eq!(
            validate_volume_roles(
                &roles(&workspace, &image, Some(&source_eq_recovery), None),
                Operation::Backup,
                &recovery,
                true,
                false
            ),
            Err(VolumeRoleConflict::BackupSourceOnRegisteredWinre),
            "方案 D 关闭时必须恢复 F3 拒绝"
        );

        // 开关不影响 workspace/image 与恢复宿主冲突的判定（始终拒绝）。
        let workspace_eq_recovery = identity("recovery", 100);
        assert_eq!(
            validate_volume_roles(
                &roles(&workspace_eq_recovery, &image, Some(&source_eq_recovery), None),
                Operation::Backup,
                &recovery,
                true,
                true
            ),
            Err(VolumeRoleConflict::WorkspaceOnRegisteredWinre),
            "方案 D 不应放开 workspace==恢复宿主"
        );
    }
    #[test]
    fn backup_rejects_same_partition() {
        let mut task = backup_task();
        task.destination.as_mut().unwrap().volume.partition_guid = "source".into();
        assert!(task.validate().is_err());
    }
    #[test]
    fn compression_supports_fast_and_none_but_rejects_max() {
        assert_eq!(canonical_compression("fast").unwrap(), "fast");
        assert_eq!(canonical_compression("none").unwrap(), "none");
        assert!(canonical_compression("max").is_err());

        let mut task = backup_task();
        task.compress = Some("fast".into());
        assert!(task.validate().is_ok());
        task.compress = Some("none".into());
        assert!(task.validate().is_ok());
        task.compress = Some("max".into());
        assert!(task.validate().is_err());
        task.compress = Some("unknown".into());
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
            name: None,
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
            name: None,
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
            name: None,
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
            name: None,
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
            started: None,
            duration_secs: None,
            bytes_per_sec: None,
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

    #[test]
    fn capture_exclusions_include_fixed_and_per_user_browser_entries() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("backuprestore-excl-{suffix}"));
        let users = root.join("Users");
        let alice = users.join("alice").join("AppData").join("Local");
        fs::create_dir_all(
            alice
                .join("Google")
                .join("Chrome")
                .join("User Data")
                .join("Default")
                .join("Cache"),
        )
        .unwrap();
        fs::create_dir_all(
            alice
                .join("Mozilla")
                .join("Firefox")
                .join("Profiles")
                .join("abc.default")
                .join("cache2"),
        )
        .unwrap();

        let ini = build_capture_exclusions(&root).unwrap();
        let lines: Vec<&str> = ini.lines().collect();
        let list = lines
            .iter()
            .position(|l| *l == "[ExclusionList]")
            .expect("配置应包含 [ExclusionList] 节");
        let all: Vec<&str> = lines[list + 1..]
            .iter()
            .filter(|l| !l.is_empty())
            .copied()
            .collect();

        // 固定根级排除项
        for fixed in [
            "\\Mac disk",
            "\\$Recycle.Bin",
            "\\$WINDOWS.~BT",
            "\\Windows.old",
            "\\Temp",
            "\\Windows\\Temp",
            "\\Windows\\SoftwareDistribution\\Download",
        ] {
            assert!(all.contains(&fixed), "缺少固定排除项 {fixed}: {ini}");
        }
        // 每个用户字面路径
        assert!(all.contains(&"\\Users\\alice\\AppData\\Local\\Temp"));
        assert!(all.contains(
            &"\\Users\\alice\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\Cache"
        ));
        assert!(all.contains(
            &"\\Users\\alice\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\Code Cache"
        ));
        assert!(all.contains(
            &"\\Users\\alice\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\abc.default\\cache2"
        ));
        assert!(all.contains(&"\\Users\\alice\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\abc.default\\cache2\\entries"));
        // 规范内不得出现中间通配符（DISM 只允许最后一段通配）
        for entry in &all {
            let normalized = entry.replace('/', "\\");
            let parts: Vec<&str> = normalized.split('\\').collect();
            let (first, _) = parts.split_first().unwrap();
            if first.is_empty() {
                // 根路径锚定写法（\x\y）：除最后一段外其余段不得含 *
                if let Some((head, tail)) = parts.split_first() {
                    assert!(!head.contains('*'), "根锚定路径的首段不应含通配符: {entry}");
                    for part in tail.split_last().unwrap().1 {
                        assert!(
                            !part.contains('*'),
                            "根锚定路径的中间段不应含通配符: {entry}"
                        );
                    }
                }
            }
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capture_exclusions_without_users_keeps_fixed_list() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("backuprestore-excl2-{suffix}"));
        let ini = build_capture_exclusions(&root).unwrap();
        assert!(ini.starts_with("[ExclusionList]\r\n"));
        assert!(ini.contains("\r\n\\Mac disk\r\n"));
        assert!(ini.contains("\r\n\\$Recycle.Bin\r\n"));
        assert!(!ini.contains("\\Users\\"));
        let _ = fs::remove_dir_all(root);
    }

    /// 三个捕获入口都靠这个函数拿到 `/ConfigFile:` 路径；历史上 GUI 在线备份
    /// 自己拼参数、漏掉了它，导致 `\Mac disk` 进了镜像、还原撞 0x80070020。
    #[test]
    fn write_capture_exclusion_config_lands_file_with_parallels_placeholder() {
        let root = std::env::temp_dir().join(format!(
            "backuprestore-excl3-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = write_capture_exclusion_config(&root).unwrap();
        assert_eq!(path.file_name().unwrap(), "BackupRestore-exclusions.ini");
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\r\n\\Mac disk\r\n"),
            "配置应排除卷根占位符: {text}"
        );
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(root);
    }
}
