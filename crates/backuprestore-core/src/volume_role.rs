//! 卷角色模型与跨分区冲突校验
//!
//! 提供卷身份元数据（`VolumeIdentity`）、verbatim 路径规范化（`\\?\Volume{GUID}\`）、
//! 卷角色集合（`VolumeRoles`）以及操作前卷角色与恢复分区冲突校验（`validate_volume_roles`）。

use serde::{Deserialize, Serialize};

use crate::Operation;

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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn volume_identity_new_and_completeness_matrix() {
        let mut id = VolumeIdentity::new("disk-1", "part-1");
        assert_eq!(id.disk_guid, "disk-1");
        assert_eq!(id.partition_guid, "part-1");
        assert_eq!(id.volume_guid, "");
        assert_eq!(id.partition_offset, 0);
        assert_eq!(id.partition_size, 0);
        assert!(!id.is_complete());

        id.volume_guid = r"\\?\Volume{00000000-0000-0000-0000-000000000001}\".into();
        id.disk_number = Some(0);
        id.partition_number = Some(1);
        id.partition_type_guid = "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}".into();
        id.filesystem = "NTFS".into();
        id.partition_size = 1024 * 1024 * 1024;
        assert!(id.is_complete());

        // 测试任何一个必需字段缺失或置零都会导致 is_complete 为 false
        let mut invalid = id.clone();
        invalid.disk_guid = "   ".into();
        assert!(!invalid.is_complete());

        let mut invalid = id.clone();
        invalid.partition_guid = "".into();
        assert!(!invalid.is_complete());

        let mut invalid = id.clone();
        invalid.volume_guid = "".into();
        assert!(!invalid.is_complete());

        let mut invalid = id.clone();
        invalid.disk_number = None;
        assert!(!invalid.is_complete());

        let mut invalid = id.clone();
        invalid.partition_number = None;
        assert!(!invalid.is_complete());

        let mut invalid = id.clone();
        invalid.partition_type_guid = " ".into();
        assert!(!invalid.is_complete());

        let mut invalid = id.clone();
        invalid.filesystem = "".into();
        assert!(!invalid.is_complete());

        let mut invalid = id.clone();
        invalid.partition_size = 0;
        assert!(!invalid.is_complete());
    }

    #[test]
    fn volume_identity_same_partition_and_reserved_check() {
        let v1 = VolumeIdentity::new("{DISK-GUID-1}", "{PART-GUID-A}");
        let v2 = VolumeIdentity::new("{disk-guid-1}", "{part-guid-a}");
        let v3 = VolumeIdentity::new("{DISK-GUID-1}", "{part-guid-b}");
        let v4 = VolumeIdentity::new("{DISK-GUID-2}", "{part-guid-a}");

        assert!(v1.same_partition(&v2));
        assert!(v2.same_partition(&v1));
        assert!(!v1.same_partition(&v3));
        assert!(!v1.same_partition(&v4));

        let mut id = VolumeIdentity::new("disk", "part");
        // ESP
        id.partition_type_guid = "{C12A7328-F81F-11D2-BA4B-00A0C93EC93B}".into();
        assert!(id.is_reserved_partition());
        // MSR
        id.partition_type_guid = "{E3C9E316-0B5C-4DB8-817D-F92DF00215AE}".into();
        assert!(id.is_reserved_partition());
        // Windows Recovery
        id.partition_type_guid = "{DE94BBA4-06D1-4D40-A16A-BFD50179D6AC}".into();
        assert!(id.is_reserved_partition());
        // Basic Data
        id.partition_type_guid = "{EBD0A0A2-B9E5-4433-87C0-68B6B72699C7}".into();
        assert!(!id.is_reserved_partition());
    }

    #[test]
    fn volume_role_conflict_codes_and_messages() {
        let conflicts = [
            (
                VolumeRoleConflict::WorkspaceOnRegisteredWinre,
                "workspace-on-registered-winre",
            ),
            (
                VolumeRoleConflict::ImageOnRegisteredWinre,
                "image-on-registered-winre",
            ),
            (
                VolumeRoleConflict::BackupSourceOnRegisteredWinre,
                "backup-source-on-registered-winre",
            ),
            (
                VolumeRoleConflict::RestoreTargetOnRegisteredWinre,
                "restore-target-on-registered-winre",
            ),
        ];

        for (conflict, code) in conflicts {
            assert_eq!(conflict.code(), code);
            assert!(!conflict.message().is_empty());
        }
    }
}
