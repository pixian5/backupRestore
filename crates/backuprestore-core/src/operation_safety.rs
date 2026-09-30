//! 在线运行互斥与格式化回读规则；与 Windows API 分离以便故障注入测试。
use crate::{TaskError, VolumeIdentity};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct OperationGate(AtomicBool);
impl OperationGate {
    pub const fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    pub fn try_start(&self) -> bool {
        self.0
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
    pub fn is_busy(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    /// 只在主线程已消费结果，或线程启动失败时释放。
    pub fn finish(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
impl Default for OperationGate {
    fn default() -> Self {
        Self::new()
    }
}

pub fn verify_formatted_volume(
    before: &VolumeIdentity,
    after: &VolumeIdentity,
) -> Result<(), TaskError> {
    if !before.is_complete()
        || !after.is_complete()
        || !before.same_partition(after)
        || !before
            .partition_type_guid
            .eq_ignore_ascii_case(&after.partition_type_guid)
        || before.disk_number != after.disk_number
        || before.partition_number != after.partition_number
        || before.partition_offset != after.partition_offset
        || before.partition_size != after.partition_size
    {
        return Err(TaskError::Invalid(
            "formatted partition identity/geometry mismatch".into(),
        ));
    }
    if !after.filesystem.eq_ignore_ascii_case("NTFS")
        || before.volume_serial.is_empty()
        || after.volume_serial.is_empty()
        || before
            .volume_serial
            .eq_ignore_ascii_case(&after.volume_serial)
    {
        return Err(TaskError::Invalid(
            "format not verified: expected NTFS with a new volume serial".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> VolumeIdentity {
        VolumeIdentity {
            disk_guid: "disk".into(),
            partition_guid: "part".into(),
            volume_guid: "vol".into(),
            partition_type_guid: "data".into(),
            disk_number: Some(0),
            partition_number: Some(2),
            partition_offset: 1024,
            partition_size: 1048576,
            filesystem: "NTFS".into(),
            volume_serial: "old".into(),
            drive_letter: Some('D'),
        }
    }
    #[test]
    fn no_op_format_cannot_report_success() {
        let before = identity();
        assert!(verify_formatted_volume(&before, &before).is_err());
        let mut after = before.clone();
        after.volume_serial = "new".into();
        assert!(verify_formatted_volume(&before, &after).is_ok());
        after.filesystem = "FAT32".into();
        assert!(verify_formatted_volume(&before, &after).is_err());
    }
    #[test]
    fn wrong_partition_or_geometry_refused() {
        let before = identity();
        for field in ["disk", "partition", "offset", "size"] {
            let mut after = before.clone();
            after.volume_serial = "new".into();
            match field {
                "disk" => after.disk_number = Some(9),
                "partition" => after.partition_guid = "other".into(),
                "offset" => after.partition_offset += 1,
                _ => after.partition_size += 1,
            }
            assert!(verify_formatted_volume(&before, &after).is_err());
        }
    }
    #[test]
    fn duplicate_launch_is_blocked_until_result_consumed() {
        let gate = std::sync::Arc::new(OperationGate::new());
        assert!(gate.try_start());
        let other = gate.clone();
        assert!(
            !std::thread::spawn(move || other.try_start())
                .join()
                .unwrap()
        );
        assert!(gate.is_busy());
        gate.finish();
        assert!(gate.try_start());
        gate.finish();
    }
}
