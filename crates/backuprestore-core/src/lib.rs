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

#[cfg(windows)]
use std::ptr::null_mut;
use thiserror::Error;
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

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

pub mod image_metadata;
pub mod operation_safety;
mod state_transaction;

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
    /// 带**稳定错误码**的失败。`code` 是契约：一旦发布不得更改，用户报障
    /// 直接报码即可定位到唯一一行（core 侧一律 `BR-CORE-NNN`）。
    /// `rendered` 是构造时的文案；core 没有语言层，这里保持英文技术原文，
    /// **用户可见的双语文案在 CLI 的 `error_code` 表里**。
    #[error("[{code}] {rendered}")]
    Coded {
        code: &'static str,
        rendered: String,
    },
}

impl TaskError {
    /// 稳定错误码（`BR-域-序号`）；无码的用法错误返回 `None`。
    ///
    /// 给 `--json` 用：判据可以直接判 `error_code == "BR-APPLY-001"`，
    /// 不必解析 `error` 文案里的 `[码] ` 前缀——**文案是给人看的，码才是契约**。
    ///
    /// 结构变体（`Io` / `Json` / `InvalidTransition`）给**固定码**但 `Display`
    /// 不加前缀：它们的文案是 OS 本地化串或 serde 原文，硬加 `[BR-…] ` 只会
    /// 让本来就难读的报错更难读；但自动化侧仍需区分「环境失败」还是「脚本
    /// 写错」，所以码要能拿到。
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Coded { code, .. } => Some(code),
            Self::Io(_) => Some("BR-IO-001"),
            Self::Json(_) => Some("BR-IO-002"),
            Self::InvalidTransition { .. } => Some("BR-TSK-028"),
            // 用法错误：英文原文本身唯一，故意无码（见 cli 的 error_code 文档）。
            Self::Invalid(_) => None,
        }
    }
}

impl TaskError {
    /// 带稳定错误码的构造。码的分配原则：**一条语义一个码**，同一条文案
    /// 出现在多处时共用一个码；码一旦发布不得重排（用户报障直接报码）。
    pub fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self::Coded {
            code,
            rendered: message.into(),
        }
    }
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
    /// 磁盘、引导与恢复注册已验证；只剩任务启动项清理。
    RecoveryComplete,
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
                | (Preflight, RecoveryComplete)
                | (Capturing, RecoveryComplete)
                | (BootRepaired, RecoveryComplete)
                | (RecoveryComplete, Success)
                // 【2026-10-18 缺陷修复 H2】允许从 RecoveryComplete 标失败。
                // 此前这条边缺失，而 `write_failure` 第一行就是
                // `if !task.status.can_transition_to(Stage::Failed) { return Err(…) }`：
                // 任务在 STEP 4 清理阶段失败 → 自动逃出写失败被拒 → 只记一行日志 →
                // 任务**永远停在 RecoveryComplete**。而该阶段仍在
                // `can_resume_after_interruption` 里 → 下次启动又被发现 → 再提示重启
                // → 再失败，`abandon-task` 在该阶段同样被拒：**用户没有任何自救出口**。
                // 此刻磁盘/引导/恢复注册都已验证完毕，标失败只影响清理的记账口径。
                | (RecoveryComplete, Failed)
        )
    }
    /// A task at one of these stages has not begun modifying its restore target,
    /// so an explicit user abandonment may remove only its task-owned boot entry
    /// and recovery payload before another task is prepared.
    pub fn can_abandon_before_target_write(self) -> bool {
        matches!(
            self,
            Self::Prepared
                | Self::BootRequested
                | Self::RecoveryStarted
                | Self::Preflight
                // 【2026-10-18 缺陷修复：ef71fccc 三按钮死锁】`Capturing` 也要可放弃。
                // 该阶段是「把源卷捕获成暂存 WIM」——**还原目标卷还没被碰过**
                // （目标擦除发生在后面的 `TargetErased`，故它不在
                // `is_destructive_boundary` 里）。此前它不在这个集合里，于是
                // 「存在未完成任务」对话框的三条路全部走不通：是=重启续跑
                // （状态机不推进）、否=放弃清理（被本函数拒）、取消=不建新任务
                // → 任务永久卡在 Capturing，还会被 DISM 烘进备份镜像。
                | Self::Capturing
        )
    }
    /// A task past preparation may be resumed through its persisted boot entry.
    pub fn can_resume_after_interruption(self) -> bool {
        matches!(
            self,
            Self::BootRequested
                | Self::RecoveryStarted
                | Self::Preflight
                | Self::Capturing
                | Self::TargetErased
                | Self::ImageApplied
                | Self::BootRepaired
                | Self::RecoveryComplete
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
    /// 卷的 **verbatim 路径**（`\\?\Volume{GUID}\`），方案 A 的基石。
    ///
    /// 有了它，读写一个卷就**不需要分配盘符**：Rust 的 `std::fs` 走
    /// `CreateFileW`/`CopyFileExW`，`bcdedit` / `dism` 这类 Win32 程序同样接受这种路径。
    /// 2026-09-29 实机验证过四件事（见 `docs/20260929-2014-S盘自动打开问题定位与修复方案.md` 第六·补节）：
    /// 读路径字节等价（副本 SHA-256 与盘符方式完全相同）、写操作**不**做归一化
    /// （`set` 后 `/enum` 输出 `fc /b` 逐字节无差异）、四个写动词全通过、
    /// `Directory::GetFiles` 能直接枚举卷内文件。
    ///
    /// `volume_guid` 为空时返回 `None`——调用方必须退回盘符路径，绝不能拼出一个
    /// 假路径（那会指到别的卷上去）。
    pub fn volume_path(&self) -> Option<String> {
        // 原生 GetVolumeNameForVolumeMountPointW 返回完整路径，隐藏卷枚举保存裸 GUID。
        // 两种来源必须生成同一个规范路径，不能把已成功读取的身份误判成缺失。
        let raw = self.volume_guid.trim();
        let guid = if let Some(rest) = raw.strip_prefix(r"\\?\Volume") {
            rest.strip_suffix('\\')?
        } else {
            raw
        };
        if guid.is_empty() {
            return None;
        }
        // 只接受带花括号的 GUID 形态，避免把 "C:" 之类的脏值拼进路径。
        if !(guid.starts_with('{') && guid.ends_with('}') && guid.len() == 38)
            || !guid.as_bytes()[1..37].iter().enumerate().all(|(i, b)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    *b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
        {
            return None;
        }
        // 拼接而非 format!：verbatim 路径里的反斜杠在字符串字面量里极难写对，
        // 一律用 ASCII 码拼，逐个字符都说得清。
        // 曾经的真实事故：`?` 后面少一个反斜杠，路径退化成 `\\?Volume{...}\`，
        // Windows 直接不认（CreateFileW 报 161/123），而单测是照实现抄的所以没拦住。
        const BACKSLASH: char = 92u8 as char;
        let mut path = String::new();
        path.push(BACKSLASH);
        path.push(BACKSLASH);
        path.push('?');
        path.push(BACKSLASH);
        path.push_str("Volume");
        path.push_str(guid);
        path.push(BACKSLASH);
        Some(path)
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
/// 角色卷校验。v1.7.11 起只保留与 WinRE 无关的基础角色规则：
/// workspace 与镜像卷都不能落在承载注册 WinRE 的分区上（那是**程序自身**的资产，
/// 与任务的启动源无关——启动源现在在镜像卷上，见 `crates/backuprestore-cli/src/boot_entry.rs`）。
///
/// F1（还原目标 == 注册 WinRE 宿主）与 F3（备份源 == 注册 WinRE 宿主）**已删除**：
/// 新启动通道全程只读注册位，格式化目标卷不再威胁续跑启动源，捕获也不再污染注册 WIM。
///
/// v1.9.1 优化：备份操作（Backup）仅读取源卷捕获 WIM，并向镜像卷写入临时启动载荷，
/// 绝不会格式化任何卷。因此在备份操作下，工作区落在承载 WinRE 的分区（如 C:）也是完全安全的；
/// 仅在还原（RestoreExisting / CreateSecondary）等会清空或重写目标卷的操作下，若工作区落在
/// 恢复分区或还原目标卷上才需要拦截。
pub fn validate_volume_roles(
    roles: &VolumeRoles<'_>,
    operation: Operation,
    recovery: &VolumeIdentity,
) -> Result<(), VolumeRoleConflict> {
    if operation != Operation::Backup && roles.workspace.same_partition(recovery) {
        return Err(VolumeRoleConflict::WorkspaceOnRegisteredWinre);
    }
    if roles.image.same_partition(recovery) {
        return Err(VolumeRoleConflict::ImageOnRegisteredWinre);
    }
    Ok(())
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

/// 用户在准备阶段对「镜像与副档记录不符」显式点了强制继续时留下的例外记录。
///
/// 为什么需要独立建模：旧实现只把 `force_restore_hash` 用在准备阶段的判断上，
/// 不落盘；任务里仍写着**副档的旧哈希**和 `verifyHash = true`。机器重启进恢复
/// 环境后，离线侧拿镜像真实内容再比那个旧值，必然不符，于是用户的强制继续
/// 被静默撤销，而且是在系统已经进入恢复环境之后才失败。
///
/// 这里记录的是**用户接受当时读到的真实哈希**，因此：
/// - 旧副档不再承担"来源真实性证明"的角色（只留痕，见
///   [`ImageAcceptance::superseded_sidecar_sha256`]）；
/// - 镜像在用户确认之后**再次变化**仍然会被检出（离线侧比的是接受时的真实值）；
/// - 例外绑定任务、镜像与索引，不会退化成通用的"跳过安全检查"。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAcceptance {
    /// 用户接受时镜像文件的真实 SHA-256（不是副档记录的值）。
    pub accepted_sha256: String,
    /// 用户接受时镜像文件的真实字节数。
    pub accepted_size_bytes: u64,
    /// 被接受的 WIM 索引；换索引不复用这次例外。
    pub index: u32,
    /// 接受时副档记录的哈希，仅留痕，不再作为判据。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_sidecar_sha256: Option<String>,
    /// 用户确认的时刻。
    pub accepted_at: DateTime<Utc>,
}

impl ImageAcceptance {
    /// 这次例外是否适用于给定镜像：必须同一索引、同一真实内容、同一大小。
    /// 任何一项不符都表示"不是用户当时看过并接受的那份镜像"，必须按未授权处理。
    pub fn covers(&self, index: u32, actual_sha256: &str, actual_size_bytes: u64) -> bool {
        self.index == index
            && self.accepted_size_bytes == actual_size_bytes
            && !self.accepted_sha256.is_empty()
            && self.accepted_sha256.eq_ignore_ascii_case(actual_sha256)
    }
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
    /// 准备任务时，目标卷**是不是当前正在运行的那套系统**（`%SystemDrive%`）。
    ///
    /// 恢复期的收尾行为必须按「目标性质」路由，而不是按「操作类型」：目标是另一套
    /// Windows 时，无论操作是 create-secondary 还是 restore-existing，都必须保住主系统
    /// 的默认启动项、修目标的盘符绑定、按第二系统注册 WinRE。此前这些收尾卡在
    /// `role == NewWindows`（= 只有 create-secondary 才做），于是离线把完整 Windows 镜像
    /// 还原到 C 以外的 Windows 卷时，产出的是「能启动但会崩、且主系统每次开机被劫持」
    /// 的半成品。
    ///
    /// 判断只能在**联机准备阶段**做：WinRE 里 `%SystemDrive%` 是恢复环境自己的 RAM 盘
    /// `X:`，离线侧重新探测只会得到 RAM 盘。所以结论在准备时落盘，由 RE 读取。
    ///
    /// 故意**不给** `#[serde(default)]`：缺字段时反序列化失败、恢复流程停在动盘之前，
    /// 比静默当成 `false` 安全——`false` 会让「重建唯一系统」场景误走第二系统收尾，
    /// 去改写当前运行系统的盘符绑定。
    pub is_active_system: bool,
    /// 还原前是否格式化目标卷（2026-10-14 用户拍板：所有还原入口都有显式格式化选项）。
    /// `true`（默认）= diskpart 快速格式化 + 校验成功后写镜像；`false` = 覆盖式应用
    /// （不格式化，卷上镜像没有的文件保留）。旧任务缺此字段按 `true` 走原路径。
    #[serde(default = "default_format_target")]
    pub format_target: bool,
    /// 「覆盖式应用（取消格式化）打到已含 Windows 的目标卷」时，恢复期走
    /// **wimlib** 通道而非 DISM（2026-10-22 用户裁定「换 wimlib」，任务 #110；
    /// 2026-10-23 产品化「保留安全描述符还原」）。
    ///
    /// 背景：DISM 覆盖式应用要还原 `Program Files\WindowsApps\...`（MSIX 包目录，
    /// owner=TrustedInstaller、带 MSIX 专用 ACL）的目录 SD/owner，被系统拒绝 →
    /// `RestoreDirMetadata 0x80070005` → exit 5。wimlib 自行获取 SeRestore/SeBackup
    /// 提权（POC E5 实证），RE/PE 上下文同样可用；2026-10-23 概念验证
    /// （docs/20261022-2308，tiny + 全量两级）进一步证明该提权**同时覆盖 SD 还原**
    /// ——镜像里的目录权限（ACL/owner）完整还原，个别受保护目录可能保留卷上现有
    /// 权限，GUI 侧有显式确认框明示此语义。
    ///
    /// 由准备期判定并落盘（RE 内重新探测不到宿主卷的 Windows 属性，同
    /// `is_active_system` 的理由）。`#[serde(default)]` 兜底 false：旧任务与
    /// 未命中该组合的任务恒走 DISM 原路径，零行为变化。
    ///
    /// **名字沿袭历史**：字段名里的 `no_acl` 是 v2.6.25–v2.6.26「跳过 SD 还原」
    /// （wimlib 带 `--no-acls`）时代的残留，当前语义 = 覆盖式应用（安全描述符随
    /// 镜像还原）；不改名是为了 task.json 序列化兼容。
    #[serde(default)]
    pub overlay_no_acl: bool,
}

/// 旧任务（2.5.2 及以前）没有 format_target 字段，反序列化按 true 兜底——
/// 与历史行为（恒格式化）一致，不会把旧任务的行为静默改变。
fn default_format_target() -> bool {
    true
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
    /// DISM `/Name` 真正写进 WIM 的索引名（备份时的有效名，空 image_name 时
    /// 备份侧记为 "Windows Backup"）。旧版副档（2.6.15 及以前）没有此字段，
    /// 读取侧据此回退 DISM 取真名——不得用占位名冒充（占位名会经 GUI 预填
    /// 流进 BCD 描述，2026-10-18 阶段3 实测踩坑）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_name: Option<String>,
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
    /// 还原时是否严格对比哈希（默认仅对比大小，为 false 或 None 时离线跳过全包哈希流式计算）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_hash: Option<bool>,
    /// 用户在准备阶段显式接受"与副档不符的当前镜像"时的例外记录（见 [`ImageAcceptance`]）。
    /// 为 None 表示没有任何例外，离线侧按常规判据执行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_acceptance: Option<ImageAcceptance>,
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
    /// 遗留整数字段（1/2/123…）：**没有任何消费方**，且不同调用点传的同一数字
    /// 含义并不相同，界面显示它只会误导用户去查一张对不上的表。保留仅为兼容
    /// 既有 status.json 的读取，新代码一律写 `error_code_text`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<i32>,
    /// 稳定错误码 `BR-域-序号`（例如 `BR-PE-039`）。
    /// 为什么单开一个字段：`write_failure` 的整型 `code` 装不下它，而
    /// `TaskError::Io` / `Json` / `InvalidTransition` 这三类的 `Display`
    /// **故意不带码**（文案是 OS 本地化串或 serde 原文），码只存在于这里。
    /// `Option` + `skip_serializing_if` → 旧 status.json 读进来是 `None`，不受影响。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code_text: Option<String>,
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
            verify_hash: None,
            image_acceptance: None,
            boot_plan,
            created: Utc::now(),
            boot_once: true,
            status: Stage::Prepared,
            payload: None,
        }
    }
    pub fn validate(&self) -> Result<(), TaskError> {
        if self.version != TASK_VERSION {
            return Err(TaskError::coded("BR-CORE-001", format!(
                "unsupported task version {} (expected {})",
                self.version, TASK_VERSION
            )));
        }
        validate_task_id(&self.task_id)?;
        if !self.boot_once {
            return Err(TaskError::coded("BR-CORE-002", "recovery tasks must use one-time boot"));
        }
        if self.boot_plan.boot_sequence_requested && self.boot_plan.previous_bcd_sha256.is_none() {
            return Err(TaskError::coded("BR-CORE-003", "boot sequence requires a previous BCD hash for rollback"));
        }
        if let Some(hash) = self.boot_plan.previous_bcd_sha256.as_deref() {
            validate_sha256_text("previous BCD", hash)?;
        }
        if let Some(compression) = self.compress.as_deref() {
            canonical_compression(compression)?;
        }
        if let Some(payload) = self.payload.as_ref() {
            if payload.task_id != self.task_id {
                return Err(TaskError::coded("BR-CORE-004", "payload task id does not match task id"));
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
        // 用户接受例外必须始终绑在本任务的镜像记录上。任务文件被改动时，
        // 这里要让一个「对不上自己镜像」的例外直接失败，而不是变成通用放行。
        if let Some(accepted) = self.image_acceptance.as_ref() {
            let image = self
                .image
                .as_ref()
                .ok_or_else(|| TaskError::coded("BR-CORE-005", "image acceptance has no image"))?;
            if accepted.index != image.index {
                return Err(TaskError::coded("BR-CORE-006", "image acceptance index does not match the task image index"));
            }
            if accepted.accepted_size_bytes != image.size_bytes {
                return Err(TaskError::coded("BR-CORE-007", "image acceptance size does not match the task image size"));
            }
            // 空哈希表示"仅大小"模式下的接受，不覆盖哈希判据；非空则必须与任务
            // 镜像哈希一致，否则离线侧会拿一个无人确认过的值当作已授权内容。
            if !accepted.accepted_sha256.is_empty() {
                validate_sha256_text("accepted image", &accepted.accepted_sha256)?;
                if !accepted.accepted_sha256.eq_ignore_ascii_case(&image.sha256) {
                    return Err(TaskError::coded("BR-CORE-008", "image acceptance hash does not match the task image hash"));
                }
            }
            if let Some(sidecar) = accepted.superseded_sidecar_sha256.as_deref() {
                validate_sha256_text("superseded sidecar", sidecar)?;
            }
        }
        match self.operation {
            Operation::Probe => {
                let source = self.source.as_ref().ok_or_else(|| missing("source"))?;
                require_complete("probe source", source)?;
                if self.image.is_some() || self.destination.is_some() || self.target.is_some() {
                    return Err(TaskError::coded("BR-CORE-009", "probe task cannot contain image, destination or target"));
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
                    return Err(TaskError::coded("BR-CORE-010", "backup destination must differ from source partition"));
                }
                if dest.volume.is_reserved_partition() {
                    return Err(TaskError::coded("BR-CORE-011", "EFI/MSR/Recovery partitions cannot store backup images"));
                }
                if let Some(path) = dest.absolute_path.as_deref() {
                    validate_absolute_path(path)?;
                }
                validate_relative_path(&dest.relative_path)?;
                if self.image.is_some() || self.target.is_some() {
                    return Err(TaskError::coded("BR-CORE-012", "backup task cannot contain image or target"));
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
                    return Err(TaskError::coded("BR-CORE-013", "image volume must differ from source volume"));
                }
                if image.volume.is_reserved_partition() {
                    return Err(TaskError::coded("BR-CORE-014", "EFI/MSR/Recovery partitions cannot store restore images"));
                }
                if let Some(path) = image.absolute_path.as_deref() {
                    validate_absolute_path(path)?;
                }
                if image.sha256.len() != 64 || !image.sha256.chars().all(|c| c.is_ascii_hexdigit())
                {
                    return Err(TaskError::coded("BR-CORE-015", "image sha256 must be 64 hex characters"));
                }
                if image.index == 0 {
                    return Err(TaskError::coded("BR-CORE-016", "WIM index must be greater than zero"));
                }
                if image.size_bytes == 0 {
                    return Err(TaskError::coded("BR-CORE-017", "restore image size must be greater than zero"));
                }
                if target.minimum_size_bytes == 0 {
                    return Err(TaskError::coded("BR-CORE-018", "restore target minimum size must be greater than zero"));
                }
                validate_relative_path(&image.relative_path)?;
                if image.volume.same_partition(&target.volume) {
                    return Err(TaskError::coded("BR-CORE-019", "image volume must differ from restore target"));
                }
                if target.volume.is_reserved_partition() {
                    return Err(TaskError::coded("BR-CORE-020", "EFI/MSR/Recovery partitions cannot be restore targets"));
                }
                match self.operation {
                    Operation::RestoreExisting => {
                        if !source.same_partition(&target.volume) {
                            return Err(TaskError::coded("BR-CORE-021", "restore-existing target must be the existing Windows source volume"));
                        }
                        if target.role != TargetRole::ExistingWindows {
                            return Err(TaskError::coded("BR-CORE-022", "restore-existing target role must be existing-windows"));
                        }
                        if !matches!(self.boot_plan.mode, BootMode::ReturnExisting) {
                            return Err(TaskError::coded("BR-CORE-023", "restore-existing must return to the existing boot entry"));
                        }
                    }
                    Operation::CreateSecondary => {
                        if source.same_partition(&target.volume) {
                            return Err(TaskError::coded("BR-CORE-024", "secondary target must differ from the existing Windows source"));
                        }
                        if target.role != TargetRole::NewWindows {
                            return Err(TaskError::coded("BR-CORE-025", "create-secondary target role must be new-windows"));
                        }
                        if target
                            .boot_menu_name
                            .as_deref()
                            .unwrap_or("")
                            .trim()
                            .is_empty()
                        {
                            return Err(TaskError::coded("BR-CORE-026", "create-secondary requires a boot menu name"));
                        }
                        if !valid_menu_name(target.boot_menu_name.as_deref().unwrap_or("")) {
                            return Err(TaskError::coded("BR-CORE-027", "create-secondary boot menu name contains control characters or is too long"));
                        }
                        if !matches!(self.boot_plan.mode, BootMode::AddSecondary) {
                            return Err(TaskError::coded("BR-CORE-028", "create-secondary must use add-secondary boot mode"));
                        }
                        if self
                            .boot_plan
                            .menu_name
                            .as_deref()
                            .unwrap_or("")
                            .trim()
                            .is_empty()
                        {
                            return Err(TaskError::coded("BR-CORE-029", "create-secondary requires a boot plan menu name"));
                        }
                        if !valid_menu_name(self.boot_plan.menu_name.as_deref().unwrap_or("")) {
                            return Err(TaskError::coded("BR-CORE-030", "create-secondary boot plan menu name contains control characters or is too long"));
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
            return Err(TaskError::coded("BR-CORE-031", "EFI/MSR/Recovery partitions cannot store workspace files"));
        }
        if let Some(target) = self.target.as_ref()
            && workspace_volume.same_partition(&target.volume)
        {
            return Err(TaskError::coded("BR-CORE-032", "workspace volume must differ from restore target"));
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
            error_code_text: None,
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
        Stage::RecoveryComplete => 95,
        Stage::Success => 100,
        Stage::Failed => 0,
    }
}
fn missing(name: &str) -> TaskError {
    TaskError::coded("BR-CORE-033", format!("missing {name}"))
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
        _ => Err(TaskError::coded("BR-CORE-034", "compression must be fast or none; max compression is not supported")),
    }
}
/// 备份开始前对「镜像所在卷」可用空间的**硬闸**下限（字节）。
///
/// 为什么比例定得这么低：WIM 的压缩比随内容变化极大——实测系统盘 fast 压缩产出
/// 约为源已用量的 13%（249 GiB 源 → 31.96 GiB 镜像），而装满已压缩媒体的数据卷
/// 可能接近 100%。任何按固定压缩比预估的硬闸都会误拒真实可行的备份，所以这里
/// **只挡明显不可能的情形**（源已用量的 5%，且绝对下限 1 GiB）：宁可放过，
/// 也不误拒。真正的「够不够」交给捕获过程中的失败与回滚来处理。
pub fn backup_space_floor(source_used_bytes: u64) -> u64 {
    const MIN_FLOOR: u64 = 1024 * 1024 * 1024; // 1 GiB
    (source_used_bytes / 20).max(MIN_FLOOR)
}

/// 新索引大小的保守估计（字节），**只用于日志警告，永不用于拒绝**。
///
/// `none` 时 WIM 与源已用量基本 1:1（略高：文件头与目录结构）；`fast` 取 0.7，
/// 对系统盘偏保守（实际常低于 0.2）、对含大量媒体的数据卷可能偏低——这正是它
/// 只能当警告、不能当闸门的原因。未识别的压缩值按 `fast` 处理。
pub fn estimate_backup_bytes(source_used_bytes: u64, compression: &str) -> u64 {
    let percent = match compression {
        "none" => 105,
        _ => 70,
    };
    source_used_bytes
        .saturating_mul(percent)
        .saturating_div(100)
}

pub fn validate_task_id(value: &str) -> Result<(), TaskError> {
    canonical_task_id(value).map(|_| ())
}
fn canonical_task_id(value: &str) -> Result<String, TaskError> {
    Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| TaskError::coded("BR-CORE-035", "task_id is not a UUID"))
}
fn validate_sha256_text(name: &str, value: &str) -> Result<(), TaskError> {
    if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(TaskError::coded("BR-CORE-036", format!(
            "{name} sha256 must be 64 hex characters"
        )));
    }
    Ok(())
}
fn require_complete(name: &str, identity: &VolumeIdentity) -> Result<(), TaskError> {
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
    // Tasks are exchanged between Unix tooling and Windows. Normalize the
    // Windows separator before checking so `..\\x` cannot bypass this guard
    // when tests run on macOS.
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
///
/// The path must use a drive-root form such as `B:\\Backups\\Windows.wim`.
/// UNC and volume-GUID paths are deliberately excluded from the GUI contract;
/// the task still stores the verified volume GUID separately so WinRE can
/// safely re-mount the same partition even if its drive letter changes.
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
        // `MoveFileExW(..., MOVEFILE_WRITE_THROUGH)` 刷新了重命名操作，
        // 但终态断电验收仍观测到文件元数据已更新、内容回退的窗口；
        // 对刚安装的最终文件再显式 FlushFileBuffers，确保 task/status
        // 的 JSON 内容在返回成功前已经交给 NTFS 持久化。
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
    /// Find every valid nonterminal task under this workspace.
    ///
    /// Preparation must not silently ignore an interrupted task: its boot entry,
    /// payload, or target may still be needed for recovery. A malformed or split
    /// task/status pair is an error rather than an empty result, so callers fail
    /// closed and can show the user the task directory that needs inspection.
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
            // 先读 status.json 定性。终态任务（success/failed）只是历史记录，它的
            // task.json 能不能读懂都不影响「现在能不能创建新任务」，所以挡在前面。
            //
            // 这一步是必须的：否则 Task 结构每新增一个必填字段，旧版本留下的已完成
            // 任务都会把新版本整个卡死。2026-10-14 实测——2.4.8 留下的 5 个终态任务
            // 缺 `isActiveSystem`，让 2.4.9 的 prepare 完全无法创建任务，报
            // 「cannot inspect existing task …: missing field」。
            //
            // 非终态任务仍然全程 fail-closed（下面的 validate + 一致性比对）：它的
            // 启动项、载荷或目标可能还等着恢复，读不懂必须报错给用户看，不能静默跳过。
            let status: StatusRecord = read_json(path.join("status.json")).map_err(|error| {
                TaskError::coded("BR-CORE-053", format!(
                    "cannot inspect status for existing task {id} at {}: {error}",
                    path.display()
                ))
            })?;
            // 【探测任务不参与「未终结」判定】
            // 探测是一次性的环境自检：`prepare --operation probe` 带 `--no-reboot`，
            // 准备成功就是它的全部成果——不重启、不写目标卷，**不存在「等着续跑」
            // 的语义**，因此它永远停在 `Prepared` 而不是终态。若照常算进来，
            // 每点一次「探测」就留下一个僵尸任务，之后创建任何任务都会先弹
            // 「是否放弃旧任务」——这恰恰是探测入口最不该有的副作用。
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
    /// 标记终态失败。**不带稳定错误码**（`error_code_text` 为 `None`）。
    ///
    /// 保留 `code: i32` 只是为了让既有调用点不动：这个整数没有消费方、
    /// 也不参与界面显示（详见 `StatusRecord::error_code` 的注释）。
    /// 凡手上有 `TaskError` 的调用点，请改用 [`Self::write_failure_coded`]
    /// 把 `error.code()` 一起落盘——否则 `Io` / `Json` / `InvalidTransition`
    /// 这三类失败在界面上永远看不到码。
    pub fn write_failure(
        &self,
        task: &mut Task,
        code: i32,
        error: impl Into<String>,
    ) -> Result<StatusRecord, TaskError> {
        self.write_failure_coded(task, None, error, Some(code))
    }

    /// 标记终态失败，并落盘稳定错误码 `BR-域-序号`。
    ///
    /// 用于「失败原因里看不到码」的三类错误：`Io` / `Json` / `InvalidTransition`
    /// 的 `Display` 故意不带前缀，码只在 `TaskError::code()` 里。
    /// 传 `None` 表示这确实是个无码失败（用户主动放弃等），**不伪造码**。
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

    /// 保留可续跑阶段和所有恢复材料；错误信息不等于任务终结。**不带稳定错误码**。
    pub fn write_recovery_error(
        &self,
        task: &Task,
        error: impl Into<String>,
    ) -> Result<StatusRecord, TaskError> {
        self.write_recovery_error_coded(task, None, error)
    }

    /// 同上，并落盘稳定错误码 `BR-域-序号`（见 [`Self::write_failure_coded`]）。
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

    /// Remove only the large WinRE working trees for one terminal task.
    /// Task/status/manifest/log/BCD evidence remains available for diagnosis.
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
        return Err(TaskError::coded("BR-CORE-013", "image volume must differ from source volume"));
    }
    if matches!(
        operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) && image.same_partition(target)
    {
        return Err(TaskError::coded("BR-CORE-019", "image volume must differ from restore target"));
    }
    if target.is_reserved_partition() {
        return Err(TaskError::coded("BR-CORE-061", "reserved partition cannot be target"));
    }
    Ok(())
}
pub fn ensure_capacity(target_size: u64, metadata: &BackupMetadata) -> Result<(), TaskError> {
    let required = metadata.required_target_size();
    if target_size < required {
        return Err(TaskError::coded("BR-CORE-062", format!(
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
        return Err(TaskError::coded("BR-CORE-063", "BitLocker is enabled or volume accessibility is unknown; suspend/unlock manually before V1 recovery"));
    }
    Ok(())
}

/// 程序自身安装目录（exe 所在目录）里可再生的部署件。
///
/// 只排这几个**文件**，不排整个目录：同目录下的 `tasks\`（任务证据）与
/// `logs\`（日志）必须留在镜像里——2026-10-24 用户拍板「证据全留」，
/// 而二进制是可再生过程产物，且会被 A 入口还原系统盘时随镜像回滚成旧版
/// （实测过：镜像元数据 programVersion 偏低、自启 GUI 标题 v2.6.31）。
const SELF_DEPLOY_FILES: &[&str] = &[
    "BackupRestore.exe",
    "Recovery.exe",
    "wimlib-imagex.exe",
    "libwim-15.dll",
    "winpe-winpeshl.ini",
];

/// 程序的历史分发目录（2026-10-24 起已不再部署，deploy 会 rmdir 清残留）。
/// 固定排除：旧镜像还原后会把它带回来，排除可让"还原后不复发"，
/// 与 deploy 的 rmdir 形成双保险。
const LEGACY_PACKAGE_DIR: &str = "\\Users\\Public\\backupRestore-package";

/// 取路径的卷键（盘符，大写，形如 `H:`）；非 `X:\...` 形式（UNC、Unix 路径等）
/// 返回 None。
///
/// 手写前两字符解析而不用 `Component::Prefix`：后者只在 Windows target 上
/// 产生，而单元测试常在 macOS 宿主上跑（本项目交叉编译），必须行为一致。
fn volume_key(p: &Path) -> Option<String> {
    let s = p.as_os_str().to_string_lossy();
    let mut chars = s.chars();
    let first = chars.next()?;
    let second = chars.next()?;
    if first.is_ascii_alphabetic() && second == ':' {
        Some(s[..2].to_ascii_uppercase())
    } else {
        None
    }
}

/// 把绝对路径转成「卷内相对路径」（反斜杠连接）；exe 就在卷根、或路径含
/// 非规范段（空段/`.`/`..`）时返回 None。
fn relative_to_volume_root(p: &Path) -> Option<String> {
    let s = p.as_os_str().to_string_lossy().replace('/', "\\");
    let (drive, rest) = (s.get(..2)?, s.get(2..)?);
    if !drive.chars().next()?.is_ascii_alphabetic() || !drive.ends_with(':') {
        return None;
    }
    let rest = rest.strip_prefix('\\')?;
    let mut parts: Vec<&str> = Vec::new();
    for seg in rest.split('\\') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return None;
        }
        parts.push(seg);
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("\\"))
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
///
/// `self_exe_dir` 是程序自身 exe 所在目录（调用方传
/// `current_exe().parent()`）：与 `source_root` 同卷时，把该目录下的
/// 部署件（[`SELF_DEPLOY_FILES`]）逐个排除。跨卷时无对应路径，不加项。
/// 该路径是不是**重解析点**（符号链接 / 目录联接点），且不跟随它。
///
/// Windows 侧必须看 `FILE_ATTRIBUTE_REPARSE_POINT`：Rust 的
/// `FileType::is_symlink()` 对目录联接点（IO_REPARSE_TAG_MOUNT_POINT）返回 false，
/// 而 Windows 卷根上的 `Documents and Settings` 正是联接点，用 `is_symlink()`
/// 会漏判。非 Windows 侧没有联接点概念，用符号链接近似即可（只为单测可跑）。
pub fn is_reparse_point(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        fs::symlink_metadata(path)
            .map(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }
}

pub fn build_capture_exclusions(source_root: &Path, self_exe_dir: Option<&Path>) -> Result<String, TaskError> {
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
        // 程序自身的历史分发目录（2026-10-24 起不再部署；旧镜像还原会带回）
        LEGACY_PACKAGE_DIR.to_string(),
    ];

    // 根级自引用联接点（2026-10-26）。
    //
    // Windows 卷根上的 `Documents and Settings` 是 XP 兼容名，实际是一个指向
    // **本卷 `Users` 目录**的联接点。不排除它，捕获就会把它当作一棵真实子树再遍历
    // 一遍 `Users`——文件数凭空翻倍，预扫时间同比例拉长（而 DISM 的预扫发生在写第一
    // 个字节之前，用户看到的就是「一直没动静」）。
    //
    // ★ 只在**确认它是重解析点**时才排除。数据卷上万一真有一个叫这个名字的普通
    //   目录，无脑排除会静默丢掉用户数据——排除表的错误是"少备份"，不报错。
    //
    // 写成具名常量而不是内联数组字面量：这个列表预期会增长（每发现一个根级
    // 自引用联接点就加一条），内联单元素字面量在 clippy 1.99 起会触发
    // `single_element_loop`，而 1.98 又不认识这条 lint——具名常量两种工具链都干净。
    const ROOT_JUNCTIONS: &[&str] = &["Documents and Settings"];
    for junction in ROOT_JUNCTIONS {
        if is_reparse_point(&source_root.join(junction)) {
            lines.push(format!("\\{junction}"));
        }
    }

    // 枚举真实用户配置文件，补上按用户字面路径的排除项。
    let users_root = source_root.join("Users");
    if let Ok(profiles) = fs::read_dir(&users_root) {
        let mut user_entries: Vec<String> = Vec::new();
        for profile in profiles.flatten() {
            // ★ 必须是目录：`C:\Users` 下还有一个 `desktop.ini` 文件，早期版本把它
            // 当成了用户名，于是排除表里出现了 `\Users\desktop.ini\AppData\Local\Temp`
            // 这种畸形条目（2026-10-26 在实机排除表里抓到）。
            if !profile.path().is_dir() {
                continue;
            }
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

    // 程序自身部署件（exe 同目录的五个可再生文件）。只排文件、不排目录：
    // 同目录的 tasks\（任务证据）与 logs\ 必须留在镜像里（证据全留，2026-10-24 拍板）。
    // 二进制被 A 入口还原系统盘时随镜像回滚成旧版是实测过的痛点
    // （镜像元数据 programVersion 偏低、自启 GUI 标题 v2.6.31）。
    if let Some(exe_dir) = self_exe_dir
        && let (Some(src_vol), Some(exe_vol)) = (volume_key(source_root), volume_key(exe_dir))
        && src_vol == exe_vol
        && let Some(rel) = relative_to_volume_root(exe_dir)
    {
        for file in SELF_DEPLOY_FILES {
            lines.push(format!("\\{rel}\\{file}"));
        }
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
/// `source_root` 是待捕获卷的根（如 `T:\`）。`self_exe_dir` 传
/// `current_exe().parent()`，同卷时排除程序自身部署件（见
/// [`build_capture_exclusions`]）；三个入口都在调用处统一传，避免漏接。
pub fn write_capture_exclusion_config(source_root: &Path, self_exe_dir: Option<&Path>) -> Result<PathBuf, TaskError> {
    let text = build_capture_exclusions(source_root, self_exe_dir)?;
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
    /// 锁死方案 A 的基石：verbatim 卷路径的**精确形态**。
    ///
    /// 这里错一位，后面 `bcdedit /store <path>` 就指到别的卷上——而启动项写错地方
    /// 是会把机器弄成不能启动的，所以断言到字符级。
    ///
    /// 测试用的期望值**独立构造**（不调 `volume_path`），否则就是"照实现抄"，
    /// 实现错了测试也跟着错——2026-09-29 已经因此漏掉一个 `?` 后少反斜杠的 bug。
    #[test]
    fn volume_path_has_the_exact_verbatim_shape() {
        let mut identity = VolumeIdentity::new("{disk}", "{part}");
        identity.volume_guid = "{d08d796f-f082-4402-bdbb-a4a6a09ac53f}".into();

        let bs = 92u8 as char;
        let mut want = String::new();
        want.push(bs);
        want.push(bs);
        want.push('?');
        want.push(bs);
        want.push_str("Volume{d08d796f-f082-4402-bdbb-a4a6a09ac53f}");
        want.push(bs);

        let got = identity.volume_path().expect("合法 GUID 必须给出路径");
        assert_eq!(got, want, "verbatim 路径形态不对");

        // 前缀四个字符单独验：反斜杠 反斜杠 问号 反斜杠。
        let prefix: Vec<char> = vec![92u8 as char, 92u8 as char, '?', 92u8 as char];
        assert_eq!(
            got.chars().take(4).collect::<Vec<char>>(),
            prefix,
            "verbatim 前缀错了：{got}"
        );
        assert!(got.ends_with(bs), "结尾少反斜杠：{got}");
    }

    /// 没有 GUID 时**必须**返回 `None` 而不是拼一个假路径。
    #[test]
    fn volume_path_refuses_to_invent_a_guid() {
        let mut identity = VolumeIdentity::new("{disk}", "{part}");
        assert_eq!(identity.volume_path(), None, "空 volume_guid");
        identity.volume_guid = "   ".into();
        assert_eq!(identity.volume_path(), None, "纯空白");
        identity.volume_guid = "d08d796f-f082-4402-bdbb-a4a6a09ac53f".into();
        assert_eq!(identity.volume_path(), None, "没花括号");
        identity.volume_guid = "{d08d796f}".into();
        assert_eq!(identity.volume_path(), None, "不是 36 位 GUID");
        identity.volume_guid = "C:".into();
        assert_eq!(identity.volume_path(), None, "盘符不是卷 GUID");
    }

    #[test]
    fn native_volume_path_and_bare_guid_resolve_identically() {
        let mut identity = VolumeIdentity::new("disk", "partition");
        identity.volume_guid = "{d08d796f-f082-4402-bdbb-a4a6a09ac53f}".into();
        let expected = identity.volume_path().unwrap();
        identity.volume_guid = expected.clone();
        assert_eq!(identity.volume_path(), Some(expected.clone()));
        identity.volume_guid = format!("{expected}escape");
        assert_eq!(identity.volume_path(), None);
        identity.volume_guid = "{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}".into();
        assert_eq!(identity.volume_path(), None);
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
    fn backup_space_floor_is_a_last_resort_gate_not_a_prediction() {
        const GIB: u64 = 1024 * 1024 * 1024;
        // 空源卷也至少要求 1 GiB——镜像容器本身有开销。
        assert_eq!(backup_space_floor(0), GIB);
        // 小源卷（10 GiB）按比例算只有 0.5 GiB，被绝对下限抬到 1 GiB。
        assert_eq!(backup_space_floor(10 * GIB), GIB);
        // 大源卷按比例：249 GiB 系统盘 → 硬闸 12.45 GiB，低于它实测的 31.96 GiB
        // 镜像，也就是**永远不会误拒**那次真实备份。
        assert_eq!(backup_space_floor(249 * GIB), 249 * GIB / 20);
        assert!(backup_space_floor(249 * GIB) < 32 * GIB);
        // 单调不减。
        assert!(backup_space_floor(100 * GIB) <= backup_space_floor(200 * GIB));
    }

    #[test]
    fn estimate_backup_bytes_is_conservative_and_only_informative() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let used = 100 * GIB;
        assert_eq!(estimate_backup_bytes(used, "fast"), 70 * GIB);
        assert_eq!(estimate_backup_bytes(used, "none"), 105 * GIB);
        // 未识别值按 fast，绝不 panic。
        assert_eq!(estimate_backup_bytes(used, "max"), 70 * GIB);
        assert_eq!(estimate_backup_bytes(used, ""), 70 * GIB);
        // 估计值恒大于硬闸：警告先于硬闸触发（硬闸是极端兜底，不是常规阈值）。
        assert!(estimate_backup_bytes(used, "fast") > backup_space_floor(used));
        assert!(estimate_backup_bytes(0, "none") == 0);
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
            is_active_system: true,
            format_target: true,
            overlay_no_acl: false,
        });
        assert!(task.validate().is_err());
    }
    /// 构造一个通过校验的还原任务，供「用户接受镜像」相关用例复用。
    fn restore_task_with_image(sha256: &str, size_bytes: u64, index: u32) -> Task {
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
            volume: identity("image", 400),
            absolute_path: None,
            relative_path: "backup.wim".into(),
            sha256: sha256.into(),
            size_bytes,
            index,
            name: None,
        });
        task.target = Some(TargetSpec {
            volume: identity("source", 200),
            role: TargetRole::ExistingWindows,
            boot_menu_name: None,
            minimum_size_bytes: 100,
            is_active_system: true,
            format_target: true,
            overlay_no_acl: false,
        });
        task.verify_hash = Some(true);
        task
    }

    /// 问题 13：用户在准备阶段强制继续后，任务必须记下**接受时的真实哈希**，
    /// 这样重启进恢复环境后同一份镜像仍能通过，而不是被旧副档值再次拒绝。
    #[test]
    fn accepted_image_survives_the_reboot_and_still_detects_later_tampering() {
        let actual = "b".repeat(64);
        let mut task = restore_task_with_image(&actual, 4096, 2);
        task.image_acceptance = Some(ImageAcceptance {
            accepted_sha256: actual.clone(),
            accepted_size_bytes: 4096,
            index: 2,
            superseded_sidecar_sha256: Some("c".repeat(64)),
            accepted_at: Utc::now(),
        });
        task.validate().unwrap();
        let accepted = task.image_acceptance.as_ref().unwrap();
        // 同一份镜像：例外适用，离线侧放行。
        assert!(accepted.covers(2, &actual, 4096));
        // 确认之后镜像再被改动（内容或大小任一变化）：例外不再适用。
        assert!(!accepted.covers(2, &"d".repeat(64), 4096));
        assert!(!accepted.covers(2, &actual, 4097));
        // 换索引不复用这次确认。
        assert!(!accepted.covers(1, &actual, 4096));
    }

    /// 例外必须始终绑在本任务自己的镜像记录上，不能被改成通用放行。
    #[test]
    fn image_acceptance_must_match_the_task_image_record() {
        let actual = "b".repeat(64);
        let base = restore_task_with_image(&actual, 4096, 2);
        let accepted = ImageAcceptance {
            accepted_sha256: actual.clone(),
            accepted_size_bytes: 4096,
            index: 2,
            superseded_sidecar_sha256: None,
            accepted_at: Utc::now(),
        };

        // 索引对不上。
        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            index: 1,
            ..accepted.clone()
        });
        assert!(task.validate().is_err());

        // 大小对不上。
        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            accepted_size_bytes: 8192,
            ..accepted.clone()
        });
        assert!(task.validate().is_err());

        // 哈希对不上任务镜像记录。
        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            accepted_sha256: "e".repeat(64),
            ..accepted.clone()
        });
        assert!(task.validate().is_err());

        // 非法哈希文本。
        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            accepted_sha256: "not-a-hash".into(),
            ..accepted.clone()
        });
        assert!(task.validate().is_err());
        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            superseded_sidecar_sha256: Some("zz".into()),
            ..accepted.clone()
        });
        assert!(task.validate().is_err());

        // 仅大小模式的接受：哈希留空，合法。
        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            accepted_sha256: String::new(),
            ..accepted.clone()
        });
        task.validate().unwrap();
        assert!(
            !task
                .image_acceptance
                .as_ref()
                .unwrap()
                .covers(2, &actual, 4096),
            "空哈希的接受不得覆盖哈希判据"
        );

        // 没有镜像的任务不允许携带例外。
        let mut task = backup_task();
        task.image_acceptance = Some(accepted);
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
            is_active_system: true,
            format_target: true,
            overlay_no_acl: false,
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
            is_active_system: false,
            format_target: true,
            overlay_no_acl: false,
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
            is_active_system: true,
            format_target: true,
            overlay_no_acl: false,
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
            image_name: None,
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

        let ini = build_capture_exclusions(&root, None).unwrap();
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
            "\\Users\\Public\\backupRestore-package",
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

    /// 2026-10-26 实机发现的两处排除表缺陷：
    ///
    /// ① 卷根 `Documents and Settings` 是指向本卷 `Users` 的自引用联接点，漏排就等于
    ///    把 `Users` 整棵树再遍历一遍，预扫时间翻倍（DISM 预扫发生在写第一个字节之前，
    ///    用户看到的是「一直没动静」）。
    /// ② `C:\Users` 下还存在 `desktop.ini` 这个**文件**，早期版本把它当用户名，生成
    ///    `\Users\desktop.ini\AppData\Local\Temp` 这种畸形条目。
    #[test]
    fn capture_exclusions_cover_root_junction_and_skip_non_directory_profiles() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("backuprestore-excl-junction-{suffix}"));
        let users = root.join("Users");
        fs::create_dir_all(users.join("alice").join("AppData").join("Local")).unwrap();
        // 非目录条目：不能被当成用户名。
        fs::write(users.join("desktop.ini"), "[.ShellClassInfo]").unwrap();

        // 没有联接点时，绝不能排除同名真实目录——排除表的错误是「少备份」，不报错。
        fs::create_dir_all(root.join("Documents and Settings")).unwrap();
        let ini = build_capture_exclusions(&root, None).unwrap();
        assert!(
            !ini.contains("\\Documents and Settings"),
            "普通同名目录不能被排除，否则静默丢用户数据: {ini}"
        );
        assert!(
            !ini.contains("\\Users\\desktop.ini\\"),
            "desktop.ini 是文件、不是用户: {ini}"
        );
        assert!(ini.contains("\\Users\\alice\\AppData\\Local\\Temp"));

        // 换成真正指向 Users 的联接点后，必须被排除。
        fs::remove_dir_all(root.join("Documents and Settings")).unwrap();
        let linked = link_dir(&users, &root.join("Documents and Settings"));
        if linked {
            let ini = build_capture_exclusions(&root, None).unwrap();
            assert!(
                ini.contains("\\Documents and Settings"),
                "自引用联接点必须排除: {ini}"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    /// 建目录链接（Windows 上退化为目录联接点）。不支持时返回 false，测试跳过。
    fn link_dir(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }

    #[test]
    fn capture_exclusions_without_users_keeps_fixed_list() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("backuprestore-excl2-{suffix}"));
        let ini = build_capture_exclusions(&root, None).unwrap();
        assert!(ini.starts_with("[ExclusionList]\r\n"));
        assert!(ini.contains("\r\n\\Mac disk\r\n"));
        assert!(ini.contains("\r\n\\$Recycle.Bin\r\n"));
        // 用户枚举条目不存在（注意：固定项里的程序分发目录含 \Users\，
        // 所以判据用浏览器缓存的特征段 AppData，而不是 \Users\）
        assert!(!ini.contains("AppData\\"));
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
        let path = write_capture_exclusion_config(&root, None).unwrap();
        assert_eq!(path.file_name().unwrap(), "BackupRestore-exclusions.ini");
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\r\n\\Mac disk\r\n"),
            "配置应排除卷根占位符: {text}"
        );
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(root);
    }

    /// 自身部署件排除（2026-10-24 用户拍板「不再往 C: 部署 exe」的配套）：
    /// exe 与源卷同卷 → 逐个排除五个可再生文件；跨卷 → 不加项；
    /// exe 就在卷根 → 不加项（排整个卷没有意义）。tasks\logs\ 证据不受影响。
    #[test]
    fn capture_exclusions_skip_self_deploy_files_same_volume() {
        // 用 Windows 风格的假路径（本测试在 macOS 上也跑）：卷键/相对路径
        // 都是纯字符串处理，不触盘。
        let root = Path::new("H:\\");
        let exe_dir = Path::new("H:\\brwork");

        let ini = build_capture_exclusions(root, Some(exe_dir)).unwrap();
        for file in [
            "BackupRestore.exe",
            "Recovery.exe",
            "wimlib-imagex.exe",
            "libwim-15.dll",
            "winpe-winpeshl.ini",
        ] {
            assert!(
                ini.contains(&format!("\\brwork\\{file}\r\n")),
                "应排除同卷自身部署件 {file}: {ini}"
            );
        }
        // 只排文件、不排目录：tasks\logs\ 证据必须能进镜像
        assert!(
            !ini.contains("\r\n\\brwork\r\n"),
            "不得整目录排除自身安装目录（tasks/logs 证据要留）: {ini}"
        );

        // 跨卷：不加任何自身部署件条目
        let ini2 = build_capture_exclusions(root, Some(Path::new("C:\\brwork"))).unwrap();
        assert!(
            !ini2.contains("\\brwork\\BackupRestore.exe"),
            "跨卷不应生成自身部署件排除项: {ini2}"
        );

        // exe 就在卷根：不加项
        let ini3 = build_capture_exclusions(root, Some(Path::new("H:\\"))).unwrap();
        assert!(
            !ini3.contains("\r\n\\BackupRestore.exe\r\n"),
            "exe 在卷根时不应生成排除项: {ini3}"
        );
    }

    #[test]
    fn validate_volume_roles_allows_workspace_on_recovery_for_backup() {
        let mut recovery = VolumeIdentity::new("disk-0", "part-c");
        recovery.drive_letter = Some('C');
        recovery.volume_guid = r"\\?\Volume{c-guid}\".into();
        recovery.disk_number = Some(0);
        recovery.partition_number = Some(4);

        let workspace = recovery.clone();

        let mut image = VolumeIdentity::new("disk-0", "part-f");
        image.drive_letter = Some('F');
        image.volume_guid = r"\\?\Volume{f-guid}\".into();
        image.disk_number = Some(0);
        image.partition_number = Some(5);

        let roles = VolumeRoles {
            workspace: &workspace,
            image: &image,
            source: Some(&recovery),
            target: None,
        };

        // 备份模式：工作区即使落在 recovery (C:) 上，也应放行
        assert!(validate_volume_roles(&roles, Operation::Backup, &recovery).is_ok());

        // 还原模式：工作区落在 recovery (C:) 上，必须拦截
        assert_eq!(
            validate_volume_roles(&roles, Operation::RestoreExisting, &recovery),
            Err(VolumeRoleConflict::WorkspaceOnRegisteredWinre)
        );

        // 镜像卷落在 recovery 上：无论何种模式都必须拦截
        let roles_image_conflict = VolumeRoles {
            workspace: &image,
            image: &recovery,
            source: Some(&recovery),
            target: None,
        };
        assert_eq!(
            validate_volume_roles(&roles_image_conflict, Operation::Backup, &recovery),
            Err(VolumeRoleConflict::ImageOnRegisteredWinre)
        );
    }
}
