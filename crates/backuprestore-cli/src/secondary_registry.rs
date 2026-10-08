//! 第二系统必须沿用镜像中的系统盘符，避免克隆系统从 D: 启动却仍访问 C:。
use backuprestore_core::TaskError;

fn system_letter(text: &str) -> Result<char, TaskError> {
    let rows: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (name, rest) = line.trim().split_once(char::is_whitespace)?;
            if !name.eq_ignore_ascii_case("SystemRoot") {
                return None;
            }
            let (kind, value) = rest.trim().split_once(char::is_whitespace)?;
            Some((kind, value.trim()))
        })
        .collect();
    let [("REG_SZ", path)] = rows.as_slice() else {
        return Err(crate::err("无法唯一读取第二系统原系统目录"));
    };
    let bytes = path.as_bytes();
    if bytes.len() != 10
        || !bytes[0].is_ascii_alphabetic()
        || !path[1..].eq_ignore_ascii_case(r":\Windows")
    {
        return Err(crate::err("第二系统的原系统目录不受支持"));
    }
    Ok((bytes[0] as char).to_ascii_uppercase())
}

fn gpt_mapping(guid: &str) -> Result<String, TaskError> {
    let id = uuid::Uuid::parse_str(guid.trim_matches(['{', '}']))
        .map_err(|_| crate::err("第二系统分区 GUID 无效"))?;
    Ok(b"DMIO:ID:"
        .iter()
        .chain(id.to_bytes_le().iter())
        .map(|byte| format!("{byte:02X}"))
        .collect())
}

fn drive_mappings(text: &str) -> Result<Vec<(String, String)>, TaskError> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        let Some(name) = parts.first() else {
            continue;
        };
        if !name.starts_with(r"\DosDevices\") {
            continue;
        }
        if parts.len() != 3
            || parts[1] != "REG_BINARY"
            || name.len() != 14
            || !name.as_bytes()[12].is_ascii_alphabetic()
            || !name.ends_with(':')
            || parts[2].len() % 2 != 0
            || !parts[2].bytes().all(|b| b.is_ascii_hexdigit())
            || rows
                .iter()
                .any(|(existing, _): &(String, String)| existing.eq_ignore_ascii_case(name))
        {
            return Err(crate::err("第二系统盘符映射格式不明确"));
        }
        rows.push((name.to_string(), parts[2].to_ascii_uppercase()));
    }
    Ok(rows)
}

#[cfg(windows)]
fn with_hive<T>(
    path: &std::path::Path,
    log: &std::path::Path,
    action: impl FnOnce(&str) -> Result<T, TaskError>,
) -> Result<T, TaskError> {
    let key = format!("HKLM\\BRSecondary{}", uuid::Uuid::new_v4().simple());
    crate::capture_logged("reg.exe", &["load", &key, &path.to_string_lossy()], log)?;
    let result = action(&key);
    let unload = crate::capture_logged("reg.exe", &["unload", &key], log);
    match (result, unload) {
        (Ok(value), Ok(_)) => Ok(value),
        (Err(error), Ok(_)) => Err(error),
        (_, Err(error)) => Err(crate::err(&format!("第二系统配置卸载失败：{error}"))),
    }
}

#[cfg(windows)]
pub(crate) fn repair(
    target: &std::path::Path,
    expected: &backuprestore_core::VolumeIdentity,
    log: &std::path::Path,
) -> Result<(), TaskError> {
    use crate::{capture_logged, pe_safety, windows_prepare as wp};
    use std::fs;
    let letter = target
        .to_string_lossy()
        .chars()
        .next()
        .ok_or_else(|| crate::err("目标盘符缺失"))?;
    pe_safety::verify_binding(expected, &wp::volume_identity(letter)?, false)?;
    let running = std::env::var("SystemDrive").map_err(|_| crate::err("当前系统盘未知"))?;
    if running.eq_ignore_ascii_case(&format!("{letter}:")) {
        return Err(crate::err("拒绝修改当前系统的离线配置"));
    }
    for hive in ["SYSTEM", "SOFTWARE"] {
        if !pe_safety::probe_file(target, &["Windows", "System32", "config", hive])? {
            return Err(crate::err("第二系统缺少注册表配置"));
        }
    }
    let config = target.join("Windows\\System32\\config");
    // 保留整个事务日志族；强制断电后的 hive 单文件复制可能无法加载。
    let backup = config.join("BackupRestore-secondary-original");
    pe_safety::probe_file(
        target,
        &[
            "Windows",
            "System32",
            "config",
            "BackupRestore-secondary-original",
            "SYSTEM",
        ],
    )?;
    if !backup.exists() {
        fs::create_dir(&backup)?;
    }
    for file in fs::read_dir(&config)? {
        let file = file?;
        let name = file.file_name();
        let text = name.to_string_lossy();
        if file.file_type()?.is_file()
            && ["SYSTEM", "SOFTWARE"]
                .iter()
                .any(|h| text == *h || text.starts_with(&format!("{h}.")))
        {
            let saved = backup.join(&name);
            if !saved.exists() {
                fs::copy(file.path(), &saved)?;
                if crate::sha256_file(&saved)? != crate::sha256_file(file.path())? {
                    return Err(crate::err("第二系统配置备份摘要不符"));
                }
            }
        }
    }
    let source_letter = with_hive(&config.join("SOFTWARE"), log, |key| {
        system_letter(&capture_logged(
            "reg.exe",
            &[
                "query",
                &format!("{key}\\Microsoft\\Windows NT\\CurrentVersion"),
                "/v",
                "SystemRoot",
            ],
            log,
        )?)
    })?;
    let wanted = format!(r"\DosDevices\{source_letter}:");
    let mapping = gpt_mapping(&expected.partition_guid)?;
    with_hive(&config.join("SYSTEM"), log, |key| {
        let mounted = format!("{key}\\MountedDevices");
        // 安装镜像可没有 MountedDevices；先保证键存在，不删除其他卷的映射。
        capture_logged("reg.exe", &["add", &mounted, "/f"], log)?;
        let before = drive_mappings(&capture_logged("reg.exe", &["query", &mounted], log)?)?;
        pe_safety::verify_binding(expected, &wp::volume_identity(letter)?, false)?;
        for (name, data) in &before {
            if *data == mapping && !name.eq_ignore_ascii_case(&wanted) {
                capture_logged("reg.exe", &["delete", &mounted, "/v", name, "/f"], log)?;
            }
        }
        capture_logged(
            "reg.exe",
            &[
                "add",
                &mounted,
                "/v",
                &wanted,
                "/t",
                "REG_BINARY",
                "/d",
                &mapping,
                "/f",
            ],
            log,
        )?;
        let after = drive_mappings(&capture_logged("reg.exe", &["query", &mounted], log)?)?;
        if after.iter().filter(|(_, data)| *data == mapping).count() != 1
            || !after
                .iter()
                .any(|(name, data)| name.eq_ignore_ascii_case(&wanted) && *data == mapping)
            || before.iter().any(|(name, data)| {
                !name.eq_ignore_ascii_case(&wanted)
                    && *data != mapping
                    && !after.contains(&(name.clone(), data.clone()))
            })
        {
            return Err(crate::err("第二系统盘符映射回读不符"));
        }
        Ok(())
    })?;
    // 卸载成功才算落盘完成；本机系统注册表和原系统文件均不修改。
    crate::append_log(
        log,
        &format!(
            "第二系统离线盘符已绑定并核验：{source_letter}: -> {}；原配置备份 {}",
            expected.partition_guid,
            backup.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_guid_in_windows_binary_order_and_parses_source_letter() {
        assert_eq!(
            gpt_mapping("{fd0a50cf-735d-4ed5-bb6d-a4ab0d4eff47}").unwrap(),
            "444D494F3A49443ACF500AFD5D73D54EBB6DA4AB0D4EFF47"
        );
        assert_eq!(
            system_letter("    SystemRoot    REG_SZ    C:\\Windows\r\n").unwrap(),
            'C'
        );
        assert!(
            system_letter("SystemRoot REG_SZ C:\\Windows\nSystemRoot REG_SZ D:\\Windows").is_err()
        );
        assert!(system_letter("SystemRoot REG_SZ \\\\server\\Windows").is_err());
        assert_eq!(
            drive_mappings(
                "    \\DosDevices\\C: REG_BINARY 444d\n    \\??\\Volume{other} REG_BINARY 8888"
            )
            .unwrap(),
            [(r"\DosDevices\C:".into(), "444D".into())]
        );
        assert!(drive_mappings("\\DosDevices\\C: REG_SZ bad").is_err());
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "需要已核验新快照及独立离线测试系统"]
    fn offline_secondary_mapping() {
        assert_eq!(
            std::env::var("BR_SECONDARY_MAPPING_ACK").as_deref(),
            Ok("fresh-snapshot-offline-secondary")
        );
        let letter = std::env::var("BR_SECONDARY_MAPPING_LETTER")
            .unwrap()
            .chars()
            .next()
            .unwrap();
        let expected = crate::windows_prepare::volume_identity(letter).unwrap();
        assert_eq!(
            expected.partition_guid,
            std::env::var("BR_SECONDARY_MAPPING_GUID").unwrap()
        );
        repair(
            &std::path::PathBuf::from(format!("{letter}:\\")),
            &expected,
            &std::path::PathBuf::from(std::env::var("BR_SECONDARY_MAPPING_LOG").unwrap()),
        )
        .unwrap();
    }
}
