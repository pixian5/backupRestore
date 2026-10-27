//! 第二系统必须沿用镜像中的系统盘符，避免克隆系统从 D: 启动却仍访问 C:。
use crate::error_code::ErrCode;
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
        return Err(crate::errc(ErrCode::SecondarySystemDirAmbiguous));
    };
    let bytes = path.as_bytes();
    if bytes.len() != 10
        || !bytes[0].is_ascii_alphabetic()
        || !path[1..].eq_ignore_ascii_case(r":\Windows")
    {
        return Err(crate::errc(ErrCode::SecondarySystemDirUnsupported));
    }
    Ok((bytes[0] as char).to_ascii_uppercase())
}

fn gpt_mapping(guid: &str) -> Result<String, TaskError> {
    let id = uuid::Uuid::parse_str(guid.trim_matches(['{', '}']))
        .map_err(|_| crate::errc(ErrCode::SecondaryPartitionGuidInvalid))?;
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
            return Err(crate::errc(ErrCode::SecondaryLetterMappingAmbiguous));
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
        (_, Err(error)) => Err(crate::errc_with(ErrCode::SecondaryHiveUnloadFailed, &[&error.to_string()])),
    }
}

#[cfg(windows)]
pub(crate) fn repair(
    target: &std::path::Path,
    // 目标卷当前挂载后的盘符。`target` 可能是 `\\?\Volume{…}\` 这类无盘符的 GUID 路径
    // （在线入口按卷身份取根路径时就是这种），此时不能从路径首字符推断盘符，故显式传入。
    letter: char,
    expected: &backuprestore_core::VolumeIdentity,
    log: &std::path::Path,
) -> Result<(), TaskError> {
    use crate::{capture_logged, pe_safety, windows_prepare as wp};
    use std::fs;
    pe_safety::verify_binding(expected, &wp::volume_identity(letter)?, false)?;
    let running = std::env::var("SystemDrive").map_err(|_| crate::errc(ErrCode::CurrentSystemVolumeUnknown))?;
    if running.eq_ignore_ascii_case(&format!("{letter}:")) {
        return Err(crate::errc(ErrCode::RefuseModifyCurrentSystemHive));
    }
    for hive in ["SYSTEM", "SOFTWARE"] {
        if !pe_safety::probe_file(target, &["Windows", "System32", "config", hive])? {
            return Err(crate::errc(ErrCode::SecondaryMissingHive));
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
                    return Err(crate::errc(ErrCode::SecondaryConfigDigestMismatch));
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
            return Err(crate::errc(ErrCode::SecondaryLetterReadbackMismatch));
        }
        Ok(())
    })?;
    // 卸载成功才算落盘完成；本机系统注册表和原系统文件均不修改。
    crate::append_log(
        log,
        &crate::i18n::msg_tpl(
            "secondary_letter_bound",
            &[
                &source_letter.to_string(),
                &expected.partition_guid,
                &backup.display().to_string(),
            ],
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
    /// **只读拒绝路径**（2026-10-27 新增）。
    ///
    /// 原 `offline_secondary_mapping` 需要真实第二系统分区，F: 于 10-27 删除后
    /// 它退化成「永远不跑」——而它前面那三道守卫恰恰是最该常态化的安全属性：
    ///   ① 卷身份不符 → 拒绝写入（BR-PE-004）
    ///   ② 目标是**当前系统盘** → 拒绝改自己的离线配置（BR-VOL-014）
    ///   ③ 目标缺 SYSTEM/SOFTWARE hive → 拒绝（BR-VOL-015）
    /// 三条都是**纯拒绝**：只读探测、一个字节都不写。所以在现有机器上就能验，
    /// 不需要第二系统、不需要快照。真正的「改配置」路径仍留在 `offline_secondary_mapping`。
    ///
    /// 约定：需要一个**非系统卷**做身份基准，默认 `H:`，可用 `BR_REJECT_DRIVE` 覆盖。
    #[cfg(windows)]
    #[test]
    #[ignore = "需要 Windows 管理员与一个非系统卷（默认 H:）做只读拒绝路径校验"]
    fn secondary_mapping_rejections() {
        use crate::windows_prepare::volume_identity;
        let log = std::env::temp_dir().join("br-secondary-reject.log");
        let drive = std::env::var("BR_REJECT_DRIVE").unwrap_or_else(|_| "H".to_string());
        let letter = drive
            .chars()
            .next()
            .expect("BR_REJECT_DRIVE 至少要有一个字符");
        let actual = volume_identity(letter).expect("基准卷应当存在（默认 H:）");

        // ★ 「不写」的判据必须是**前后不变**，不是「不存在」：这台机器上本来就有
        //   `BackupRestore-secondary-original`（历史第二系统测试留下的），
        //   把它当「不该存在」会得到一条与产品无关的假红。
        let system_drive = std::env::var("SystemDrive").expect("SystemDrive 必须存在");
        let backup_probe = std::path::PathBuf::from(&system_drive)
            .join("Windows\\System32\\config\\BackupRestore-secondary-original");
        let backup_existed_before = backup_probe.exists();

        // ① 身份不符：把 partition_guid 换成一个必然不存在的值。
        let mut wrong = actual.clone();
        wrong.partition_guid = "{00000000-0000-0000-0000-000000000000}".to_string();
        let error = repair(
            &std::path::PathBuf::from(format!("{letter}:\\")),
            letter,
            &wrong,
            &log,
        )
        .expect_err("身份不符必须拒绝写入");
        assert_eq!(error.code(), Some("BR-PE-004"), "{error}");

        // ② 目标缺 hive：用一个空临时目录（身份校验会过，但目录里没有 Windows\System32\config）。
        let empty = std::env::temp_dir().join(format!("br-no-hive-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let error = repair(&empty, letter, &actual, &log).expect_err("缺 hive 必须拒绝");
        assert_eq!(error.code(), Some("BR-VOL-015"), "{error}");
        std::fs::remove_dir_all(&empty).ok();

        // ③ 目标是当前系统盘：早于任何写入就返回，连 hive 都不读。
        let system = std::env::var("SystemDrive").expect("SystemDrive 必须存在");
        let system_letter = system.chars().next().unwrap();
        let system_identity = volume_identity(system_letter).expect("系统盘身份可读");
        let error = repair(
            &std::path::PathBuf::from(format!("{system_letter}:\\")),
            system_letter,
            &system_identity,
            &log,
        )
        .expect_err("改当前系统盘的离线配置必须拒绝");
        assert_eq!(error.code(), Some("BR-VOL-014"), "{error}");

        // 三条都只读：备份目录的**存在性必须与调用前一致**（既不许新建，也不许删掉旧的）。
        assert_eq!(
            backup_probe.exists(),
            backup_existed_before,
            "拒绝路径不得改动备份目录：{}",
            backup_probe.display()
        );
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
            letter,
            &expected,
            &std::path::PathBuf::from(std::env::var("BR_SECONDARY_MAPPING_LOG").unwrap()),
        )
        .unwrap();
    }
}
