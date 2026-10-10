//! 断电续跑的跨启动校验；纯逻辑同时在本机与 Windows 上测试。
use backuprestore_core::{Stage, Task, TaskError};
use std::collections::BTreeMap;

/// 读取单个 BCD 字段并保留 GUID 续行，防止漏掉其他任务的一次性请求。
pub(crate) fn field(text: &str, key: &str) -> Result<String, TaskError> {
    let mut found = false;
    let mut reading = false;
    let mut value = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        if parts
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case(key))
        {
            if found {
                return Err(crate::err("BCD 字段重复，拒绝继续"));
            }
            found = true;
            reading = true;
            value = parts.next().unwrap_or("").trim().to_string();
        } else if reading && trimmed.starts_with('{') {
            value.push(' ');
            value.push_str(trimmed);
        } else {
            reading = false;
        }
    }
    Ok(value)
}

pub(crate) fn allow_rearm(sequence: &str, loader: &str) -> Result<(), TaskError> {
    if !sequence.is_empty() && !sequence.eq_ignore_ascii_case(loader) {
        return Err(crate::err("存在其他一次性启动请求，拒绝覆盖"));
    }
    Ok(())
}

/// 准备期路径仅用于核对载荷位置，实际盘符必须来自现场卷身份。
///
/// 载荷目录按任务隔离，所以这里要连任务 ID 一起核对：一条簿记只能指向**它自己**
/// 那个子目录下的 `Winre.wim`，不能指到别的任务的载荷上去。
pub(crate) fn payload_path(saved: &str, letter: char, task_id: &str) -> Result<String, TaskError> {
    let expected = format!(
        r"\{}\Winre.wim",
        crate::boot_cleanup::staging_relative_dir(task_id)?
    );
    let bytes = saved.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !saved[2..].eq_ignore_ascii_case(&expected)
        || !letter.is_ascii_alphabetic()
    {
        return Err(crate::err("续跑载荷路径不属于任务暂存目录"));
    }
    Ok(format!("{letter}:{expected}"))
}

pub(crate) fn verify_chain(
    loader: &str,
    options: &str,
    wim: &str,
    devopts: &str,
    letter: char,
    task_id: &str,
) -> Result<(), TaskError> {
    let device = crate::text_parsing::ramdisk_spec(wim, devopts);
    // SDI 路径也随载荷目录按任务隔离；和 BCD 里的实际值必须同源同值。
    let sdi =
        crate::text_parsing::staging_sdi_path(&crate::boot_cleanup::staging_relative_dir(task_id)?);
    for (text, key, expected) in [
        (loader, "device", device.as_str()),
        (loader, "osdevice", device.as_str()),
        (loader, "path", r"\windows\system32\winload.efi"),
        (loader, "winpe", "Yes"),
        (options, "ramdisksdidevice", &format!("partition={letter}:")),
        (options, "ramdisksdipath", &sdi),
    ] {
        if !field(text, key)?.eq_ignore_ascii_case(expected) {
            return Err(crate::err(&format!("续跑启动链字段不符：{key}")));
        }
    }
    Ok(())
}

/// 仅已进入擦除阶段、且该角色就是目标分区时，允许格式化带来的序列号变化。
pub(crate) fn role_was_erased(task: &Task, values: &BTreeMap<String, String>, role: &str) -> bool {
    let Some(target) = &task.target else {
        return false;
    };
    matches!(
        task.status,
        Stage::TargetErased | Stage::ImageApplied | Stage::BootRepaired
    ) && [
        ("DISK_GUID", target.volume.disk_guid.as_str()),
        ("PARTITION_GUID", target.volume.partition_guid.as_str()),
        ("VOLUME_GUID", target.volume.volume_guid.as_str()),
    ]
    .iter()
    .all(|(suffix, expected)| {
        values
            .get(&format!("{role}_{suffix}"))
            .is_some_and(|actual| !expected.is_empty() && actual.eq_ignore_ascii_case(expected))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "{11111111-1111-1111-1111-111111111111}";
    #[test]
    fn only_the_erased_target_role_may_change_serial() {
        use backuprestore_core::{
            BootMode, BootPlan, Operation, TargetRole, TargetSpec, VolumeIdentity,
        };
        let mut task = Task::new(
            Operation::RestoreExisting,
            BootPlan {
                mode: BootMode::ReturnExisting,
                previous_bcd_sha256: None,
                menu_name: None,
                boot_sequence_requested: true,
            },
        );
        let mut volume = VolumeIdentity::new("disk", "target");
        volume.volume_guid = "volume".into();
        task.target = Some(TargetSpec {
            volume,
            role: TargetRole::ExistingWindows,
            boot_menu_name: None,
            minimum_size_bytes: 1,
        });
        let mut values = BTreeMap::from([
            ("SOURCE_DISK_GUID".into(), "disk".into()),
            ("SOURCE_PARTITION_GUID".into(), "target".into()),
            ("SOURCE_VOLUME_GUID".into(), "volume".into()),
        ]);
        assert!(!role_was_erased(&task, &values, "SOURCE"));
        for stage in [
            Stage::TargetErased,
            Stage::ImageApplied,
            Stage::BootRepaired,
        ] {
            task.status = stage;
            assert!(role_was_erased(&task, &values, "SOURCE"));
            assert!(!role_was_erased(&task, &values, "RECOVERY"));
        }
        values.insert("SOURCE_PARTITION_GUID".into(), "primary".into());
        assert!(!role_was_erased(&task, &values, "SOURCE"));
        task.status = Stage::Failed;
        assert!(!role_was_erased(&task, &values, "SOURCE"));
    }
    #[test]
    fn preserves_foreign_requests_even_on_continuation_lines() {
        let text = format!(
            "bootsequence {ID}\r\n              {{22222222-2222-2222-2222-222222222222}}\r\ntoolsdisplayorder {{memdiag}}"
        );
        assert!(allow_rearm(&field(&text, "bootsequence").unwrap(), ID).is_err());
        assert!(allow_rearm("", ID).is_ok());
        assert!(allow_rearm(ID, ID).is_ok());
        assert!(field("device one\ndevice two", "device").is_err());
    }
    /// 载荷按任务隔离后，跨启动核验必须连任务 ID 一起对：
    /// 同一台机器上另一个任务的载荷路径与 SDI 路径都不能被当成自己的。
    #[test]
    fn cross_boot_path_and_full_chain_are_verified() {
        const TASK: &str = "11111111-2222-4333-8444-555555555555";
        const OTHER: &str = "99999999-8888-4777-8666-555555555555";
        let wim =
            payload_path(&format!(r"F:\BackupRestoreRE\{TASK}\Winre.wim"), 'G', TASK).unwrap();
        assert_eq!(wim, format!(r"G:\BackupRestoreRE\{TASK}\Winre.wim"));
        let device = crate::text_parsing::ramdisk_spec(&wim, ID);
        let loader = format!(
            "device {device}\nosdevice {device}\npath \\windows\\system32\\winload.efi\nwinpe Yes"
        );
        let options = format!(
            "ramdisksdidevice partition=G:\nramdisksdipath \\BackupRestoreRE\\{TASK}\\boot.sdi"
        );
        verify_chain(&loader, &options, &wim, ID, 'G', TASK).unwrap();
        for bad in [
            loader.replace("osdevice", "other"),
            loader.replace("winpe Yes", "winpe No"),
            loader.replace("[G:]", "[F:]"),
        ] {
            assert!(verify_chain(&bad, &options, &wim, ID, 'G', TASK).is_err());
        }
        assert!(verify_chain(&loader, &options.replace("G:", "C:"), &wim, ID, 'G', TASK).is_err());
        // 另一个任务的 SDI 路径不得通过本任务的核验。
        assert!(verify_chain(&loader, &options, &wim, ID, 'G', OTHER).is_err());
        for path in [
            format!(r"F:\BackupRestoreRE\{TASK}\..\Winre.wim"),
            format!(r"F:\BackupRestoreRE\{OTHER}\Winre.wim"),
            r"F:\BackupRestoreRE\Winre.wim".into(),
            r"F:\else\Winre.wim".into(),
            r"\\server\Winre.wim".into(),
        ] {
            assert!(payload_path(&path, 'G', TASK).is_err(), "{path} 应被拒绝");
        }
        // 非法任务 ID 不得构造出任何路径。
        assert!(payload_path(&format!(r"F:\BackupRestoreRE\{TASK}\Winre.wim"), 'G', "x").is_err());
    }
}
