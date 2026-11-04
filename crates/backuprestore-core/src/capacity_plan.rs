//! 容量规划、压缩规范、操作边界与 BitLocker 状态校验
//!
//! 提供备份/还原容量下限硬闸（`backup_space_floor` / `backup_space_shortfall`）、
//! 压缩格式规范化（`canonical_compression`）、镜像估计尺寸（`estimate_backup_bytes`）、
//! 目标卷容量前置判定（`ensure_capacity`）、操作跨分区合法性（`validate_operation`）
//! 以及 BitLocker 可访问性硬性守卫（`ensure_bitlocker_accessible`）。

use crate::{require_complete, BackupMetadata, Operation, TaskError, VolumeIdentity};

/// Normalize the only two supported WIM compression choices.
///
/// `fast` is the product's "压缩" choice and `none` is "不压缩". The former
/// maximum-compression mode is deliberately rejected here so GUI, CLI and old
/// task files cannot accidentally pass it to DISM through a fallback path.
pub fn canonical_compression(value: &str) -> Result<&'static str, TaskError> {
    match value {
        "fast" => Ok("fast"),
        "none" => Ok("none"),
        _ => Err(TaskError::coded(
            "BR-CORE-034",
            "compression must be fast or none; max compression is not supported",
        )),
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

/// 硬闸的**判定**本身：`Some(下限)` = 拦下（可用空间不足，附需要多少）；`None` = 放行。
///
/// 为什么把它抽成函数：判定原来在两条链（`windows_prepare` 的离线准备、
/// `online_operation` 的在线捕获）里各自写了一遍 `if free < floor`。
/// 同一个不等式抄两遍，就没法写「拦下」的判据——2026-10-27 之前
/// `BR-IMG-008` **只验过「不误拦」（可用空间够时放过），从未构造过「拦下」**：
/// 一个只在一边实现的守卫，测试只能测到它那一半。
///
/// 边界语义写死在这里：`available == floor` 放行（`<` 而非 `<=`），
/// 与文档「低于硬闸才拒绝」一致。
pub fn backup_space_shortfall(available_bytes: u64, source_used_bytes: u64) -> Option<u64> {
    let floor = backup_space_floor(source_used_bytes);
    (available_bytes < floor).then_some(floor)
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
        return Err(TaskError::coded(
            "BR-CORE-013",
            "image volume must differ from source volume",
        ));
    }
    if matches!(
        operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) && image.same_partition(target)
    {
        return Err(TaskError::coded(
            "BR-CORE-019",
            "image volume must differ from restore target",
        ));
    }
    if target.is_reserved_partition() {
        return Err(TaskError::coded(
            "BR-CORE-061",
            "reserved partition cannot be target",
        ));
    }
    Ok(())
}

pub fn ensure_capacity(target_size: u64, metadata: &BackupMetadata) -> Result<(), TaskError> {
    let required = metadata.required_target_size();
    if target_size < required {
        return Err(TaskError::coded(
            "BR-CORE-062",
            format!("target disk space insufficient: {target_size} < {required}"),
        ));
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
        return Err(TaskError::coded(
            "BR-CORE-063",
            "BitLocker is enabled or volume accessibility is unknown; suspend/unlock manually before V1 recovery",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn identity(label: &str, size_gib: u64) -> VolumeIdentity {
        let mut id = VolumeIdentity::new(format!("disk-{label}"), format!("part-{label}"));
        id.volume_guid = format!(r"\\?\Volume{{{label}-guid}}\");
        id.partition_type_guid = "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}".into();
        id.disk_number = Some(0);
        id.partition_number = Some(1);
        id.partition_size = size_gib * 1024 * 1024 * 1024;
        id.filesystem = "NTFS".into();
        id.volume_serial = "12345678".into();
        id
    }

    fn sample_metadata(captured_used_bytes: u64, reserved_bytes: u64, minimum_target_size: u64) -> BackupMetadata {
        BackupMetadata {
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
            captured_used_bytes,
            reserved_bytes,
            minimum_target_size,
            volume_serial: "x".into(),
            program_version: "0.0.0".into(),
            image_name: None,
        }
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
    fn backup_space_gate_blocks_below_floor_and_allows_at_floor() {
        const GIB: u64 = 1024 * 1024 * 1024;
        // 拦下：可用比下限少 1 字节 → 必须报，且回报的正是那个下限。
        assert_eq!(backup_space_shortfall(GIB - 1, 0), Some(GIB));
        assert_eq!(backup_space_shortfall(0, 100 * GIB), Some(100 * GIB / 20));

        // 边界：恰好等于下限 → 放行（低于硬闸才拒绝，是 < 不是 <=）。
        assert_eq!(backup_space_shortfall(GIB, 0), None);
        assert_eq!(
            backup_space_shortfall(100 * GIB / 20, 100 * GIB),
            None
        );

        // 充裕：远高于下限 → 放行。
        assert_eq!(backup_space_shortfall(200 * GIB, 100 * GIB), None);

        // 与阈值函数必须是同一套语义。
        for used in [0u64, GIB, 10 * GIB, 249 * GIB, 4096 * GIB] {
            let floor = backup_space_floor(used);
            assert_eq!(backup_space_shortfall(floor - 1, used), Some(floor), "used={used}");
            assert_eq!(backup_space_shortfall(floor, used), None, "used={used}");
        }
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
        // 估计值恒大于硬闸：警告先于硬闸触发。
        assert!(estimate_backup_bytes(used, "fast") > backup_space_floor(used));
        assert_eq!(estimate_backup_bytes(0, "none"), 0);
    }

    #[test]
    fn compression_supports_fast_and_none_but_rejects_max() {
        assert_eq!(canonical_compression("fast").unwrap(), "fast");
        assert_eq!(canonical_compression("none").unwrap(), "none");
        assert!(canonical_compression("max").is_err());
        assert!(canonical_compression("").is_err());
        assert!(canonical_compression("fastest").is_err());
    }

    #[test]
    fn operation_validation_boundary_matrix() {
        let source = identity("src", 100);
        let image = identity("img", 100);
        let target = identity("tgt", 100);

        // 合法操作四卷互斥校验：正常放行
        assert!(validate_operation(Operation::Backup, &source, &image, &target).is_ok());
        assert!(validate_operation(Operation::RestoreExisting, &source, &image, &target).is_ok());
        assert!(validate_operation(Operation::CreateSecondary, &source, &image, &target).is_ok());

        // 镜像卷与源卷同分区：必须被 BR-CORE-013 拦截
        assert_eq!(
            validate_operation(Operation::Backup, &source, &source, &target)
                .unwrap_err()
                .code(),
            Some("BR-CORE-013")
        );

        // 还原模式下，镜像卷与目标卷同分区：必须被 BR-CORE-019 拦截
        assert_eq!(
            validate_operation(Operation::RestoreExisting, &source, &target, &target)
                .unwrap_err()
                .code(),
            Some("BR-CORE-019")
        );
        assert_eq!(
            validate_operation(Operation::CreateSecondary, &source, &target, &target)
                .unwrap_err()
                .code(),
            Some("BR-CORE-019")
        );

        // 目标卷为系统保留分区（如 ESP/Recovery）：必须被 BR-CORE-061 拦截
        let mut reserved_target = target.clone();
        reserved_target.partition_type_guid = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}".into();
        assert_eq!(
            validate_operation(Operation::RestoreExisting, &source, &image, &reserved_target)
                .unwrap_err()
                .code(),
            Some("BR-CORE-061")
        );
    }

    #[test]
    fn ensure_capacity_boundary_matrix() {
        let metadata = sample_metadata(80, 20, 90);
        assert_eq!(metadata.required_target_size(), 100);

        // 不足 100 字节拦截
        let err = ensure_capacity(99, &metadata).unwrap_err();
        assert_eq!(err.code(), Some("BR-CORE-062"));

        // 恰好 100 字节与充裕放行
        assert!(ensure_capacity(100, &metadata).is_ok());
        assert!(ensure_capacity(200, &metadata).is_ok());
    }

    #[test]
    fn bitlocker_accessible_state_matrix() {
        // 全无或仅非受保护状态：放行
        assert!(ensure_bitlocker_accessible(&[]).is_ok());
        assert!(ensure_bitlocker_accessible(&[BitLockerState::Disabled]).is_ok());
        assert!(ensure_bitlocker_accessible(&[BitLockerState::Unlocked]).is_ok());
        assert!(ensure_bitlocker_accessible(&[
            BitLockerState::Disabled,
            BitLockerState::Unlocked
        ])
        .is_ok());

        // Protected 或 Unknown：拦截并返回 BR-CORE-063
        let err_protected = ensure_bitlocker_accessible(&[
            BitLockerState::Unlocked,
            BitLockerState::Protected,
        ])
        .unwrap_err();
        assert_eq!(err_protected.code(), Some("BR-CORE-063"));

        let err_unknown = ensure_bitlocker_accessible(&[
            BitLockerState::Disabled,
            BitLockerState::Unknown,
        ])
        .unwrap_err();
        assert_eq!(err_unknown.code(), Some("BR-CORE-063"));
    }
}
