//! Shared, platform-independent safety and task model for BackupRestore.
//!
//! The Windows front end and the WinRE recovery host must make the same
//! decisions about volume identity, task transitions and destructive
//! boundaries. This crate deliberately contains no Windows API calls.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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
pub mod task_spec;
pub use task_spec::*;
pub mod task_lifecycle;
pub use task_lifecycle::*;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_new_initial_state() {
        let plan = BootPlan {
            mode: BootMode::ReturnExisting,
            previous_bcd_sha256: None,
            menu_name: None,
            boot_sequence_requested: false,
        };
        let task = Task::new(Operation::Backup, plan);
        assert_eq!(task.version, TASK_VERSION);
        assert_eq!(task.operation, Operation::Backup);
        assert_eq!(task.status, Stage::Prepared);
        assert!(task.boot_once);
        assert!(task.source.is_none());
        assert!(task.target.is_none());
        assert!(!task.task_id.is_empty());
    }
}
