//! PE 的写盘契约与步骤编排；不依赖 Windows，故障测试直接覆盖真实编排。
use backuprestore_core::{TaskError, VolumeIdentity};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RestoreIntent {
    SystemReplace,
    SecondaryInstall,
    DataOverlay,
}

/// 同时拆解两种 Windows 分隔符，逐层拒绝可改变解析语义的路径组件。
pub(crate) fn relative_components(value: &str) -> Result<Vec<&str>, TaskError> {
    let parts: Vec<_> = value.split(['\\', '/']).collect();
    if parts.iter().any(|part| {
        part.is_empty()
            || *part == "."
            || *part == ".."
            || part.contains(':')
            || part.ends_with(['.', ' '])
            || part.chars().any(char::is_control)
    }) {
        return Err(crate::err("镜像相对路径包含无效组件"));
    }
    Ok(parts)
}

/// 只接受同一对象内的精确设备字段；描述文字和重复字段不能冒充加载器证据。
pub(crate) fn target_loader(text: &str, letter: char) -> Result<String, TaskError> {
    let text = text.replace("\r\n", "\n");
    let device = format!("partition={letter}:").to_ascii_lowercase();
    let mut found = Vec::new();
    for block in text.split("\n\n") {
        let field = |key: &str| {
            let values: Vec<_> = block
                .lines()
                .filter_map(|line| {
                    let mut parts = line.trim().splitn(2, char::is_whitespace);
                    (parts.next()?.eq_ignore_ascii_case(key))
                        .then(|| parts.next().unwrap_or("").trim())
                })
                .collect();
            (values.len() == 1).then(|| values[0].to_ascii_lowercase())
        };
        if field("device").as_deref() == Some(&device)
            && field("osdevice").as_deref() == Some(&device)
            && field("path").as_deref() == Some("\\windows\\system32\\winload.efi")
        {
            let id = block
                .lines()
                .find_map(crate::bcd_identifier_from_line)
                .ok_or_else(|| crate::err("目标加载器缺少对象 GUID"))?;
            found.push(id.to_string());
        }
    }
    if found.len() != 1 {
        return Err(crate::err("目标引导加载器不唯一或设备字段不完整"));
    }
    Ok(found.remove(0))
}

pub(crate) fn verify_binding(
    expected: &VolumeIdentity,
    actual: &VolumeIdentity,
    serial: bool,
) -> Result<(), TaskError> {
    if !expected.is_complete()
        || !actual.is_complete()
        || !expected.same_partition(actual)
        || !expected
            .partition_type_guid
            .eq_ignore_ascii_case(&actual.partition_type_guid)
        || expected.partition_offset != actual.partition_offset
        || expected.partition_size != actual.partition_size
        || expected.disk_number != actual.disk_number
        || expected.partition_number != actual.partition_number
        || (serial
            && (expected.volume_serial.is_empty()
                || expected.volume_serial != actual.volume_serial))
    {
        return Err(crate::err("卷身份、现场编号或几何已变化，拒绝写入"));
    }
    Ok(())
}

/// 跨启动只重新绑定现场地址；稳定身份、几何和文件系统仍须全部吻合。
/// 返回值用于本次操作，后续写入继续由 verify_binding 拒绝编号再次变化。
pub(crate) fn rebind_after_boot(
    expected: &VolumeIdentity,
    actual: &VolumeIdentity,
    serial: bool,
) -> Result<VolumeIdentity, TaskError> {
    if !expected.is_complete() {
        return Err(crate::err("准备期卷身份不完整，拒绝重新绑定"));
    }
    let mut rebound = expected.clone();
    rebound.disk_number = actual.disk_number;
    rebound.partition_number = actual.partition_number;
    rebound.drive_letter = actual.drive_letter;
    verify_binding(&rebound, actual, serial)?;
    if !expected.filesystem.eq_ignore_ascii_case(&actual.filesystem)
        || !crate::text_parsing::same_volume(&expected.volume_guid, &actual.volume_guid)
    {
        return Err(crate::err("跨启动卷身份或文件系统变化，拒绝重新绑定"));
    }
    Ok(rebound)
}

pub(crate) fn check_target(
    target: &VolumeIdentity,
    protected: &[VolumeIdentity],
    main: Option<&VolumeIdentity>,
    intent: RestoreIntent,
) -> Result<(), TaskError> {
    if !target.is_complete()
        || target.is_reserved_partition()
        || !target
            .partition_type_guid
            .eq_ignore_ascii_case("{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}")
        || !target.filesystem.eq_ignore_ascii_case("NTFS")
    {
        return Err(crate::err("目标必须是身份完整的 GPT/NTFS 基本数据分区"));
    }
    for role in protected {
        if !role.is_complete() || role.same_partition(target) {
            return Err(crate::err(
                "目标与镜像、工作区、运行程序或恢复载荷冲突，或必需角色未知",
            ));
        }
    }
    if intent == RestoreIntent::SecondaryInstall {
        let main = main.ok_or_else(|| crate::err("第二系统任务缺少已确认的主系统身份"))?;
        if !main.is_complete() || main.same_partition(target) {
            return Err(crate::err("第二系统不能覆盖主系统，确认不能豁免角色禁区"));
        }
    }
    Ok(())
}

/// 逐级检查目录，明确区分不存在、错误和重解析路径；未知不能降级为非系统。
pub(crate) fn probe_file(root: &Path, components: &[&str]) -> Result<bool, TaskError> {
    probe_file_inner(root, components, true)
}

/// 活动 BCD 可能由系统独占；这里只验证类型和祖先，内容可读性由 bcdedit 回读验证。
#[cfg(windows)]
pub(crate) fn probe_file_metadata(root: &Path, components: &[&str]) -> Result<bool, TaskError> {
    probe_file_inner(root, components, false)
}

fn probe_file_inner(root: &Path, components: &[&str], open: bool) -> Result<bool, TaskError> {
    let root_info = fs::symlink_metadata(root)?;
    if !root_info.is_dir() || root_info.file_type().is_symlink() {
        return Err(crate::err("探测卷根不可用或存在重解析"));
    }
    let mut path = root.to_path_buf();
    for (index, part) in components.iter().enumerate() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(info) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if info.file_attributes() & 0x400 != 0 {
                        return Err(crate::err("安全探测不跟随重解析点"));
                    }
                }
                if info.file_type().is_symlink() {
                    return Err(crate::err("安全探测不跟随链接"));
                }
                if index + 1 == components.len() {
                    if !info.is_file() {
                        return Err(crate::err("必要文件路径不是普通文件"));
                    }
                    if open {
                        fs::File::open(&path)?;
                    }
                    return Ok(true);
                }
                if !info.is_dir() {
                    return Err(crate::err("探测祖先路径不是目录"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // 再验证祖先仍可达，避免卷离线产生的 NotFound 被当成缺文件。
                fs::read_dir(path.parent().ok_or_else(|| crate::err("探测父路径缺失"))?)?;
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(crate::err("探测路径为空"))
}

/// 领取记录采用独占创建并刷新；记录存在（包括上轮崩溃留下的半记录）就拒绝重放。
pub(crate) fn claim_request(request: &Path) -> Result<Vec<u8>, TaskError> {
    let bytes = fs::read(request)?;
    let mut claim = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(request.with_extension("claimed"))?;
    claim.write_all(&bytes)?;
    claim.sync_all()?;
    Ok(bytes)
}

pub(crate) trait RestoreBackend {
    fn intent(&self) -> RestoreIntent;
    fn preflight(&mut self) -> Result<(), TaskError>;
    fn persist(&mut self, stage: &str) -> Result<(), TaskError>;
    fn format(&mut self) -> Result<(), TaskError>;
    fn apply(&mut self) -> Result<(), TaskError>;
    fn verify(&mut self) -> Result<(), TaskError>;
    fn boot(&mut self) -> Result<(), TaskError>;
}

pub(crate) fn run_restore(backend: &mut impl RestoreBackend) -> Result<(), TaskError> {
    let result = (|| {
        backend.preflight()?;
        if backend.intent() != RestoreIntent::DataOverlay {
            backend.persist("format-intent")?;
            backend.format()?;
            backend.persist("format-verified")?;
        }
        backend.persist("apply-intent")?;
        backend.apply()?;
        backend.verify()?;
        backend.persist("apply-verified")?;
        if backend.intent() != RestoreIntent::DataOverlay {
            backend.persist("boot-intent")?;
            backend.boot()?;
            backend.persist("boot-verified")?;
        }
        backend.persist("success")
    })();
    if let Err(ref error) = result {
        // 失败状态若也无法落盘，原先意图记录仍会阻止自动重放；不能改成成功。
        if let Err(write_error) = backend.persist(&format!("failed: {error}")) {
            return Err(crate::err(&format!(
                "{error}；失败状态落盘也失败：{write_error}"
            )));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loader_proof_requires_exact_unique_fields() {
        let good = "标识符 {11111111-1111-1111-1111-111111111111}\ndevice partition=T:\nosdevice partition=T:\npath \\Windows\\system32\\winload.efi";
        assert!(target_loader(good, 'T').is_ok());
        assert!(target_loader(&good.replace("osdevice", "description"), 'T').is_err());
        assert!(
            target_loader(
                &good.replace("osdevice partition=T:", "osdevice partition=T:\\other"),
                'T'
            )
            .is_err()
        );
        assert!(target_loader(&format!("{good}\ndevice partition=T:"), 'T').is_err());
        assert!(target_loader(&format!("{good}\n\n{good}"), 'T').is_err());
    }
    #[test]
    fn mixed_separators_cannot_skip_ancestor_checks() {
        assert_eq!(
            relative_components("folder/junction\\image.wim").unwrap(),
            vec!["folder", "junction", "image.wim"]
        );
        for path in ["a/../b", "a/./b", "/root", "a//b", "a/b:", "a./b", "a /b"] {
            assert!(relative_components(path).is_err(), "{path}");
        }
    }
    fn identity(partition: &str) -> VolumeIdentity {
        VolumeIdentity {
            disk_guid: "{11111111-1111-1111-1111-111111111111}".into(),
            partition_guid: partition.into(),
            volume_guid: "{22222222-2222-2222-2222-222222222222}".into(),
            partition_type_guid: "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}".into(),
            disk_number: Some(1),
            partition_number: Some(2),
            partition_offset: 1048576,
            partition_size: 128 * 1024 * 1024,
            filesystem: "NTFS".into(),
            volume_serial: "serial".into(),
            drive_letter: Some('V'),
        }
    }
    #[test]
    fn protected_roles_and_unknown_main_cannot_be_overridden() {
        let target = identity("target");
        let main = identity("main");
        assert!(check_target(&target, &[], Some(&main), RestoreIntent::SecondaryInstall).is_ok());
        assert!(check_target(&target, &[], None, RestoreIntent::SecondaryInstall).is_err());
        assert!(
            check_target(&target, &[], Some(&target), RestoreIntent::SecondaryInstall).is_err()
        );
        assert!(
            check_target(
                &target,
                std::slice::from_ref(&target),
                None,
                RestoreIntent::SystemReplace
            )
            .is_err()
        );
        let mut unknown = main.clone();
        unknown.partition_type_guid.clear();
        assert!(check_target(&target, &[unknown], None, RestoreIntent::SystemReplace).is_err());
        for kind in [
            "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}",
            "{de94bba4-06d1-4d40-a16a-bfd50179d6ac}",
            "unknown",
        ] {
            let mut reserved = target.clone();
            reserved.partition_type_guid = kind.into();
            assert!(check_target(&reserved, &[], None, RestoreIntent::SystemReplace).is_err());
        }
    }
    #[test]
    fn checked_numbers_cannot_change_before_write() {
        let before = identity("target");
        let mut after = before.clone();
        after.disk_number = Some(7);
        assert!(verify_binding(&before, &after, true).is_err());
        after = before.clone();
        after.partition_number = Some(4);
        assert!(verify_binding(&before, &after, true).is_err());
        after = before.clone();
        after.volume_serial = "new".into();
        assert!(verify_binding(&before, &after, true).is_err());
        assert!(verify_binding(&before, &after, false).is_ok());
    }

    #[test]
    fn boot_rebinding_keeps_stable_identity_and_pins_new_addresses() {
        let before = identity("target");
        let mut current = before.clone();
        current.disk_number = Some(7);
        current.partition_number = Some(4);
        current.drive_letter = Some('Q');
        let rebound = rebind_after_boot(&before, &current, true).unwrap();
        assert_eq!(rebound.disk_number, Some(7));
        assert_eq!(rebound.partition_number, Some(4));
        assert!(verify_binding(&rebound, &current, true).is_ok());
        let script =
            crate::diskpart_format_script(rebound.disk_number, rebound.partition_number).unwrap();
        assert!(script.starts_with("select disk 7\r\nselect partition 4\r\n"));
        current.disk_number = Some(8);
        assert!(verify_binding(&rebound, &current, true).is_err());
        for mutate in [
            |v: &mut VolumeIdentity| v.disk_guid = "foreign".into(),
            |v: &mut VolumeIdentity| v.partition_guid = "foreign".into(),
            |v: &mut VolumeIdentity| v.volume_guid = "foreign".into(),
            |v: &mut VolumeIdentity| v.partition_type_guid = "foreign".into(),
            |v: &mut VolumeIdentity| v.partition_offset += 4096,
            |v: &mut VolumeIdentity| v.partition_size += 4096,
            |v: &mut VolumeIdentity| v.filesystem = "FAT32".into(),
            |v: &mut VolumeIdentity| v.volume_serial = "foreign".into(),
            |v: &mut VolumeIdentity| v.disk_number = None,
        ] {
            let mut foreign = current.clone();
            mutate(&mut foreign);
            assert!(rebind_after_boot(&before, &foreign, true).is_err());
        }
    }
    struct Recorder {
        calls: Vec<String>,
        fail: &'static str,
        intent: RestoreIntent,
    }
    impl Recorder {
        fn call(&mut self, name: &str) -> Result<(), TaskError> {
            self.calls.push(name.into());
            if self.fail == name {
                Err(crate::err("注入失败"))
            } else {
                Ok(())
            }
        }
    }
    impl RestoreBackend for Recorder {
        fn intent(&self) -> RestoreIntent {
            self.intent
        }
        fn preflight(&mut self) -> Result<(), TaskError> {
            self.call("preflight")
        }
        fn persist(&mut self, s: &str) -> Result<(), TaskError> {
            self.call(s)
        }
        fn format(&mut self) -> Result<(), TaskError> {
            self.call("format")
        }
        fn apply(&mut self) -> Result<(), TaskError> {
            self.call("apply")
        }
        fn verify(&mut self) -> Result<(), TaskError> {
            self.call("verify")
        }
        fn boot(&mut self) -> Result<(), TaskError> {
            self.call("boot")
        }
    }
    #[test]
    fn every_failure_stops_all_following_writes() {
        let sequence = [
            "preflight",
            "format-intent",
            "format",
            "format-verified",
            "apply-intent",
            "apply",
            "verify",
            "apply-verified",
            "boot-intent",
            "boot",
            "boot-verified",
            "success",
        ];
        for (index, fail) in sequence.iter().enumerate() {
            let mut backend = Recorder {
                calls: vec![],
                fail,
                intent: RestoreIntent::SystemReplace,
            };
            assert!(run_restore(&mut backend).is_err());
            assert_eq!(&backend.calls[..index + 1], &sequence[..index + 1]);
            assert_eq!(backend.calls.len(), index + 2);
            assert!(backend.calls.last().unwrap().starts_with("failed:"));
        }
    }
    #[test]
    fn data_overlay_never_formats_or_repairs_boot() {
        let mut backend = Recorder {
            calls: vec![],
            fail: "",
            intent: RestoreIntent::DataOverlay,
        };
        run_restore(&mut backend).unwrap();
        assert!(
            !backend
                .calls
                .iter()
                .any(|s| s.contains("format") || s.contains("boot"))
        );
        assert_eq!(backend.calls.last().unwrap(), "success");
    }
    #[test]
    fn failed_or_completed_request_is_not_replayed() {
        let dir = std::env::temp_dir().join(format!(
            "br-claim-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("request.json");
        fs::write(&path, b"request").unwrap();
        assert_eq!(claim_request(&path).unwrap(), b"request");
        assert!(claim_request(&path).is_err());
        fs::write(&path, b"changed").unwrap();
        assert!(claim_request(&path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn missing_root_and_non_file_are_unknown_not_absent() {
        let dir = std::env::temp_dir().join(format!("br-probe-{}", std::process::id()));
        fs::create_dir_all(dir.join("directory")).unwrap();
        assert!(probe_file(&dir.join("offline"), &["SYSTEM"]).is_err());
        assert!(probe_file(&dir, &["directory"]).is_err());
        assert!(!probe_file(&dir, &["absent"]).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }
}
