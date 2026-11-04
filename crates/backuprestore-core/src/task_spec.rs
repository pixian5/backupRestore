//! 任务配置子结构与副档元数据模型 (Task Specification & Sidecar Metadata).
//!
//! 将任务相关的目标规格 (`TargetSpec`)、镜像规格 (`ImageSpec`)、启动规划 (`BootPlan`)、
//! 载荷清单 (`PayloadManifest`) 以及备份副档元数据 (`BackupMetadata`) 集中建模，
//! 保证任务准备端 (CLI/GUI) 与 WinRE 执行端反序列化契约与语义 100% 严格一致。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::volume_role::VolumeIdentity;

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
pub fn default_format_target() -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_identity() -> VolumeIdentity {
        let mut id = VolumeIdentity::new("disk", "C:");
        id.volume_guid = "volume-1234".into();
        id.filesystem = "NTFS".into();
        id.partition_size = 100_000_000_000;
        id
    }

    #[test]
    fn image_acceptance_covers_logic() {
        let hash = "a".repeat(64);
        let acceptance = ImageAcceptance {
            accepted_sha256: hash.clone(),
            accepted_size_bytes: 4096,
            index: 1,
            superseded_sidecar_sha256: Some("b".repeat(64)),
            accepted_at: Utc::now(),
        };

        // 完全匹配
        assert!(acceptance.covers(1, &hash, 4096));
        // 大小写不敏感匹配
        assert!(acceptance.covers(1, &hash.to_uppercase(), 4096));
        // 索引不匹配
        assert!(!acceptance.covers(2, &hash, 4096));
        // 大小不匹配
        assert!(!acceptance.covers(1, &hash, 4095));
        // 哈希不匹配
        assert!(!acceptance.covers(1, &"c".repeat(64), 4096));

        // 空哈希永不放行任何镜像
        let empty_hash_acceptance = ImageAcceptance {
            accepted_sha256: String::new(),
            accepted_size_bytes: 4096,
            index: 1,
            superseded_sidecar_sha256: None,
            accepted_at: Utc::now(),
        };
        assert!(!empty_hash_acceptance.covers(1, "", 4096));
        assert!(!empty_hash_acceptance.covers(1, &hash, 4096));
    }

    #[test]
    fn backup_metadata_required_target_size() {
        let mut metadata = BackupMetadata {
            version: 1,
            image_type: "wim".into(),
            created: Utc::now(),
            started: None,
            duration_secs: None,
            bytes_per_sec: None,
            computer: "PC".into(),
            windows_edition: "Enterprise".into(),
            architecture: "arm64".into(),
            windows_build: "26100".into(),
            wim_index: 1,
            image_sha256: "d".repeat(64),
            image_size: 10_000_000,
            source: sample_identity(),
            captured_used_bytes: 50_000_000,
            reserved_bytes: 10_000_000,
            minimum_target_size: 70_000_000,
            volume_serial: "SERIAL1".into(),
            program_version: "2.10.3".into(),
            image_name: Some("Win11".into()),
        };

        // 70_000_000 > 50_000_000 + 10_000_000 = 60_000_000，取 70_000_000
        assert_eq!(metadata.required_target_size(), 70_000_000);

        // captured + reserved = 80_000_000 > 70_000_000，取 80_000_000
        metadata.captured_used_bytes = 70_000_000;
        assert_eq!(metadata.required_target_size(), 80_000_000);
    }

    #[test]
    fn target_spec_default_format_target_and_deserialization() {
        assert!(default_format_target());

        let json = r#"{
            "volume": {
                "diskGuid": "disk-123",
                "partitionGuid": "part-456"
            },
            "role": "existing-windows",
            "isActiveSystem": false
        }"#;

        // formatTarget 缺省时应按 default_format_target() 赋为 true
        let spec: TargetSpec = serde_json::from_str(json).unwrap();
        assert!(spec.format_target);
        assert!(!spec.overlay_no_acl);
        assert_eq!(spec.role, TargetRole::ExistingWindows);
    }

    #[test]
    fn boot_plan_and_payload_manifest_serialization() {
        let plan = BootPlan {
            mode: BootMode::AddSecondary,
            previous_bcd_sha256: Some("abcd".into()),
            menu_name: Some("Secondary Windows".into()),
            boot_sequence_requested: true,
        };
        let serialized = serde_json::to_string(&plan).unwrap();
        let deserialized: BootPlan = serde_json::from_str(&serialized).unwrap();
        assert_eq!(plan, deserialized);
        assert!(serialized.contains("add-secondary"));
        assert!(serialized.contains("previousBcdSha256"));

        let manifest = PayloadManifest {
            task_id: "task-001".into(),
            recovery_sha256: "rec-sha".into(),
            task_sha256: "tsk-sha".into(),
            original_winre_sha256: "winre-sha".into(),
            staged_winre_sha256: "staged-sha".into(),
            created_by_version: "2.10.3".into(),
            recovery_task_env_sha256: Some("env-sha".into()),
        };
        let m_json = serde_json::to_string(&manifest).unwrap();
        let m_deser: PayloadManifest = serde_json::from_str(&m_json).unwrap();
        assert_eq!(manifest, m_deser);
    }

    #[test]
    fn destination_spec_serialization() {
        let dest = DestinationSpec {
            volume: sample_identity(),
            absolute_path: Some("D:\\Backups\\win.wim".into()),
            relative_path: "Backups\\win.wim".into(),
        };
        let s = serde_json::to_string(&dest).unwrap();
        let back: DestinationSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(dest, back);
    }
}
