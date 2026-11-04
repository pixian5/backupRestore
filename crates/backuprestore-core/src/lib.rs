//! Shared, platform-independent safety and task model for BackupRestore.
//!
//! The Windows front end and the WinRE recovery host must make the same
//! decisions about volume identity, task transitions and destructive
//! boundaries. This crate deliberately contains no Windows API calls.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use std::io;

use thiserror::Error;
use uuid::Uuid;

pub mod image_metadata;
pub mod operation_safety;
pub mod capture_exclusion;
pub use capture_exclusion::*;
pub mod task_store;
pub use task_store::*;
pub mod volume_role;
pub use volume_role::*;
pub mod capacity_plan;
pub use capacity_plan::*;
pub mod stage;
pub use stage::*;


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

fn missing(name: &str) -> TaskError {
    TaskError::coded("BR-CORE-033", format!("missing {name}"))
}
fn valid_menu_name(value: &str) -> bool {
    value.chars().count() <= 256 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn backup_rejects_same_partition() {
        let mut task = backup_task();
        task.destination.as_mut().unwrap().volume.partition_guid = "source".into();
        assert!(task.validate().is_err());
    }
    #[test]
    fn task_validation_checks_compression() {
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
}
