//! BCD 引导条目输出解析、启动顺序判定与 GUID/Ramdisk 规范化。
//!
//! 跨平台纯解析模块（在 macOS 本机即可被单元测试完全覆盖），负责：
//! - `bcdedit /enum` 加载器条目、启动菜单顺序与默认项提取；
//! - 真正的 Windows 加载器甄别（排除 WinRE 条目与损坏加载项）；
//! - BCD 字段双语匹配（`bcd_field_name_is`）与 bootsequence 识别；
//! - GUID 严格校验与知名别名白名单过滤（防止空值误改 `{default}`）；
//! - Ramdisk 与 SDI 设备路径规范化（`ramdisk_spec` / `staging_sdi_path`）。

/// BOM 探测或按双字节交织 NUL 判定 UTF-16LE 字节流。
pub(crate) fn looks_like_utf16le(bytes: &[u8]) -> bool {
    let nul_count = bytes.iter().filter(|&&b| b == 0).count();
    bytes.starts_with(&[0xFF, 0xFE]) || (bytes.len() >= 2 && nul_count > bytes.len() / 4)
}

fn utf16le_to_string(bytes: &[u8]) -> String {
    let body = if bytes.starts_with(&[0xFF, 0xFE]) {
        &bytes[2..]
    } else {
        bytes
    };
    let units: Vec<u16> = body
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// 解码 bcdedit 输出文件的字节流（兼容 UTF-16LE 与 UTF-8）。
pub(crate) fn decode_bcdedit_bytes(bytes: &[u8]) -> String {
    if looks_like_utf16le(bytes) {
        utf16le_to_string(bytes)
    } else {
        String::from_utf8_lossy(bytes).to_string()
    }
}

/// 沿用「只做双语匹配、不猜值形态」的最小改动，把这些点统一到一处。
pub(crate) fn bcd_field_name_is(actual: &str, expected: &str) -> bool {
    if actual.eq_ignore_ascii_case(expected) {
        return true;
    }
    const PAIRS: [(&str, &str); 8] = [
        ("device", "设备"),
        ("osdevice", "操作系统设备"),
        ("path", "路径"),
        ("description", "描述"),
        ("identifier", "标识符"),
        ("default", "默认"),
        ("bootsequence", "启动序列"),
        ("recoverysequence", "恢复序列"),
    ];
    let want = expected.to_ascii_lowercase();
    PAIRS
        .iter()
        .any(|(ascii, zh)| *ascii == want && actual.trim() == *zh)
}

pub(crate) fn bcd_output_has_bootsequence(text: &str) -> bool {
    text.lines().any(|line| {
        let key = line.trim_start();
        let lower = key.to_ascii_lowercase();
        lower.starts_with("bootsequence") || key.starts_with("启动序列")
    })
}

pub(crate) struct BcdLoader {
    pub identifier: String,
    /// `device partition=X:` 里的盘符；不是分区设备（ramdisk/unknown…）时为 `None`。
    pub device_partition: Option<char>,
    pub loader_path: String,
    /// 条目里出现 `winpe` 字段即视为 PE 条目。
    ///
    /// 判据取「字段存在」而不是「值等于 Yes」是有意的**安全偏向**：值读错最多让
    /// 候选集变空（于是我们不动 `default`），而把 PE 条目误认成 Windows 会直接
    /// 把机器送回 PE——两种错的代价差一个数量级。
    pub is_winpe: bool,
}

fn braced_token(line: &str) -> Option<&str> {
    let start = line.find('{')?;
    let end = line[start..].find('}')? + start + 1;
    Some(&line[start..end])
}

fn identifier_of(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("identifier") || trimmed.starts_with("标识符") {
        braced_token(trimmed)
    } else {
        None
    }
}

fn partition_letter(line: &str) -> Option<char> {
    const KEY: &str = "partition=";
    let lower = line.to_ascii_lowercase();
    let start = lower.find(KEY)? + KEY.len();
    let mut chars = line.get(start..)?.chars();
    let letter = chars.next()?;
    (letter.is_ascii_alphabetic() && chars.next() == Some(':')).then(|| letter.to_ascii_uppercase())
}

/// 把 `/enum all /v` 拆成一条条加载器摘要。空行是段分隔符。
pub(crate) fn bcd_loader_entries(text: &str) -> Vec<BcdLoader> {
    let normalized = text.replace("\r\n", "\n");
    let mut loaders = Vec::new();
    for block in normalized.split("\n\n") {
        let mut identifier = None;
        let mut device_partition = None;
        let mut loader_path = String::new();
        let mut is_winpe = false;
        for line in block.lines() {
            if let Some(found) = identifier_of(line) {
                identifier = Some(found.to_owned());
                continue;
            }
            let trimmed = line.trim();
            let lower = trimmed.to_ascii_lowercase();
            if lower.starts_with("device") {
                device_partition = partition_letter(trimmed);
            } else if lower.starts_with("path") {
                loader_path = trimmed
                    .split_whitespace()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join(" ");
            } else if lower.starts_with("winpe") {
                is_winpe = true;
            }
        }
        if let Some(identifier) = identifier {
            loaders.push(BcdLoader {
                identifier,
                device_partition,
                loader_path,
                is_winpe,
            });
        }
    }
    loaders
}

/// `{bootmgr}` 的 `displayorder`（启动菜单顺序，**可能是多行**）。
///
/// 只认 Windows Boot Manager 那一段：`/enum all` 里的固件启动管理器也有一个
/// `displayorder`，混进来的话会把固件项排进候选顺序。
pub(crate) fn bcd_display_order(text: &str) -> Vec<String> {
    const WINDOWS_BOOT_MANAGER: &str = "{9dea862c-5cdd-4e70-acc1-f32b344d4795}";
    let normalized = text.replace("\r\n", "\n");
    for block in normalized.split("\n\n") {
        let is_boot_manager = block.lines().any(|line| {
            identifier_of(line).is_some_and(|found| found.eq_ignore_ascii_case(WINDOWS_BOOT_MANAGER))
        });
        if !is_boot_manager {
            continue;
        }
        let mut order = Vec::new();
        let mut reading = false;
        for line in block.lines() {
            let trimmed = line.trim();
            let key = trimmed.split_whitespace().next().unwrap_or("");
            if key.eq_ignore_ascii_case("displayorder") || key == "显示顺序" {
                reading = true;
            } else if !trimmed.starts_with('{') {
                // `toolsdisplayorder` 等后续字段必须隔离，否则会被当成菜单项。
                reading = false;
            }
            if reading
                && let Some(token) = braced_token(trimmed)
            {
                order.push(token.to_ascii_lowercase());
            }
        }
        return order;
    }
    Vec::new()
}

/// `{bootmgr}` 的 `default`（可能不存在，也可能指向一个已经删掉的条目）。
///
/// 只在 Windows Boot Manager 那一段里找，否则会把某个加载器自己段里的
/// `默认` 之类的行误当默认项。
pub(crate) fn bcd_boot_manager_default(text: &str) -> Option<String> {
    const WINDOWS_BOOT_MANAGER: &str = "{9dea862c-5cdd-4e70-acc1-f32b344d4795}";
    let normalized = text.replace("\r\n", "\n");
    for block in normalized.split("\n\n") {
        let is_boot_manager = block.lines().any(|line| {
            identifier_of(line).is_some_and(|found| found.eq_ignore_ascii_case(WINDOWS_BOOT_MANAGER))
        });
        if !is_boot_manager {
            continue;
        }
        for line in block.lines() {
            let trimmed = line.trim();
            let key = trimmed.split_whitespace().next().unwrap_or("");
            if key.eq_ignore_ascii_case("default") || key == "默认" {
                return braced_token(trimmed).map(|token| token.to_ascii_lowercase());
            }
        }
    }
    None
}

/// 认出「真正的 Windows 启动项」：非 PE、`path` 是 `winload.efi`、`device` 是
/// 分区盘符、且该盘符上真的存在这个加载器文件（`loader_exists` 由调用方用
/// `GetFileAttributesW` 之类去查——盘符是 **PE 视角**的，只有 PE 自己能判）。
///
/// 多条候选时按启动菜单顺序取第一条。认不出来就返回 `None`，调用方**不许**
/// 自己编一个 GUID 写进去（那正是上一版把 `default` 写成悬空值的原因）。
pub(crate) fn pick_windows_loader(
    text: &str,
    loader_exists: impl Fn(char) -> bool,
) -> Option<String> {
    let order = bcd_display_order(text);
    let mut candidates: Vec<BcdLoader> = bcd_loader_entries(text)
        .into_iter()
        .filter(|loader| !loader.is_winpe)
        .filter(|loader| {
            loader
                .loader_path
                .to_ascii_lowercase()
                .contains("winload.efi")
        })
        .filter(|loader| loader.device_partition.is_some_and(&loader_exists))
        .collect();
    candidates.sort_by_key(|loader| {
        order
            .iter()
            .position(|item| item.eq_ignore_ascii_case(&loader.identifier))
            .unwrap_or(usize::MAX)
    });
    candidates.into_iter().next().map(|loader| loader.identifier)
}

/// Return the first `{...}` GUID in `text` WITHOUT its braces; callers wrap
/// with `{}` as needed.
pub(crate) fn first_braced_guid(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text[start..].find('}')? + start;
    Some(text[start + 1..end].to_string())
}

/// 从 bcdedit / reagentc 的输出里抽取 GUID，兼容「有花括号」与「无花括号」两种形态，
/// 并统一补回花括号。抽不到一律返回 `None`（调用方必须当作致命错误）。
pub(crate) fn parse_guid(text: &str) -> Option<String> {
    let raw = text.find('{').and_then(|start| {
        text[start..]
            .find('}')
            .map(|end| text[start..start + end + 1].to_string())
    });
    let candidate = raw.or_else(|| {
        // 无花括号形态：`ccb31eed-bbb7-11f1-88f5-cbcfb69d515d`
        let mut found = None;
        for token in text.split(|c: char| c.is_whitespace() || c == ':' || c == ',') {
            let is_guid = token.len() == 36
                && token.as_bytes().get(8) == Some(&b'-')
                && token.as_bytes().get(13) == Some(&b'-')
                && token.as_bytes().get(18) == Some(&b'-')
                && token.as_bytes().get(23) == Some(&b'-')
                && token.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
            if is_guid {
                found = Some(token.to_string());
                break;
            }
        }
        found
    })?;
    // 统一补回花括号后再做严格校验：无花括号形态（reagentc /info 就是这样）曾被
    // 「要求花括号」的校验误杀，导致准备阶段直接失败。
    let normalized = if candidate.starts_with('{') {
        candidate
    } else {
        format!("{{{}}}", candidate)
    };
    if !is_guid(&normalized) {
        return None;
    }
    Some(normalized)
}

pub(crate) fn is_guid(value: &str) -> bool {
    let body = value.strip_prefix('{').and_then(|v| v.strip_suffix('}'));
    let Some(body) = body else {
        return false;
    };
    let parts: Vec<&str> = body.split('-').collect();
    parts.len() == 5
        && parts[0].len() == 8
        && parts[1].len() == 4
        && parts[2].len() == 4
        && parts[3].len() == 4
        && parts[4].len() == 12
        && body.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// `device`/`osdevice` 的值形态：
/// `ramdisk=[F:]\BackupRestoreRE\Winre.wim,{设备选项对象}`
pub(crate) fn ramdisk_spec(wim_path: &str, devopts: &str) -> String {
    let (volume, inside) = split_volume_and_path(wim_path);
    format!("ramdisk=[{volume}]{inside},{devopts}")
}

/// 把 `F:\BackupRestoreRE\Winre.wim` 拆成 `("F:", "\\BackupRestoreRE\\Winre.wim")`。
fn split_volume_and_path(wim_path: &str) -> (String, String) {
    let bytes: Vec<char> = wim_path.chars().collect();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == ':' {
        let volume: String = bytes[..2].iter().collect();
        let mut inside: String = bytes[2..].iter().collect();
        if !inside.starts_with('\\') {
            inside.insert(0, '\\');
        }
        return (volume, inside);
    }
    let mut inside = wim_path.to_string();
    if !inside.starts_with('\\') {
        inside.insert(0, '\\');
    }
    (String::new(), inside)
}

/// 从 `bcdedit /enum <osloader> /v` 的输出里取**设备选项对象** GUID。
pub(crate) fn ramdisk_device_options_guid(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        let key = trimmed.split_whitespace().next().unwrap_or("");
        if (bcd_field_name_is(key, "device") || bcd_field_name_is(key, "osdevice"))
            && let Some(guid) = parse_guid(trimmed)
        {
            return Some(guid);
        }
    }
    None
}

/// 载荷 RE 的 `ramdisksdipath`：**必须以 `\` 开头**（绝对路径）。
pub(crate) fn staging_sdi_path(staging_dir: &str) -> String {
    let dir = staging_dir.trim_start_matches('\\');
    format!("\\{dir}\\boot.sdi")
}

/// 严格 GUID 形态校验（带花括号的 8-4-4-4-12）。空值/畸形一律 `Err`。
pub(crate) fn require_guid(value: &str, what: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!(
            "BCD {what}: GUID is empty - refusing to touch {{default}}"
        ));
    }
    if !is_guid(trimmed) {
        return Err(format!("BCD {what}: not a valid GUID: {trimmed}"));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn is_well_known_identifier(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("{bootmgr}")
}

/// BCD 标识符校验：GUID，或白名单里的知名别名（当前只有 `{bootmgr}`）。
pub(crate) fn require_identifier(value: &str, what: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if is_well_known_identifier(trimmed) {
        return Ok(trimmed.to_string());
    }
    require_guid(trimmed, what)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le(text: &str, bom: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        if bom {
            bytes.extend_from_slice(&[0xFF, 0xFE]);
        }
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn bootsequence_detection_handles_english_chinese_and_absent_values() {
        assert!(bcd_output_has_bootsequence(
            "Windows Boot Manager\nidentifier {bootmgr}\nbootsequence {12345678-1234-1234-1234-123456789abc}"
        ));
        assert!(bcd_output_has_bootsequence(
            "Windows 启动管理器\n标识符 {bootmgr}\n启动序列 {12345678-1234-1234-1234-123456789abc}"
        ));
        assert!(!bcd_output_has_bootsequence(
            "Windows Boot Manager\nidentifier {bootmgr}\ndefault {current}"
        ));
        assert!(!bcd_output_has_bootsequence(""));
    }

    #[test]
    fn bcdedit_output_decodes_utf8_and_utf16_with_or_without_bom() {
        let guid = "{d2264b2f-1111-2222-3333-444455556666}";
        let line = format!("标识符                  {guid}");

        assert_eq!(decode_bcdedit_bytes(line.as_bytes()), line);
        assert_eq!(decode_bcdedit_bytes(&utf16le(&line, true)), line);
        assert_eq!(decode_bcdedit_bytes(&utf16le(&line, false)), line);

        for bytes in [
            line.as_bytes().to_vec(),
            utf16le(&line, true),
            utf16le(&line, false),
        ] {
            let decoded = decode_bcdedit_bytes(&bytes);
            assert_eq!(
                first_braced_guid(&decoded).as_deref(),
                Some("d2264b2f-1111-2222-3333-444455556666")
            );
        }
    }

    #[test]
    fn braced_guid_extraction_handles_missing_and_partial_braces() {
        assert_eq!(first_braced_guid("no braces here"), None);
        assert_eq!(first_braced_guid("{unterminated"), None);
        assert_eq!(first_braced_guid("{}"), Some(String::new()));
        // Only the first GUID is returned.
        assert_eq!(
            first_braced_guid("{first} {second}").as_deref(),
            Some("first")
        );
    }

    #[test]
    fn guid_parsing_accepts_both_shapes() {
        assert_eq!(
            parse_guid("已将该项成功复制到 {ccb31ee5-bbb7-11f1-88f5-cbcfb69d515d}。").as_deref(),
            Some("{ccb31ee5-bbb7-11f1-88f5-cbcfb69d515d}")
        );
        assert_eq!(
            parse_guid("引导配置数据(BCD)标识符: ccb31eed-bbb7-11f1-88f5-cbcfb69d515d").as_deref(),
            Some("{ccb31eed-bbb7-11f1-88f5-cbcfb69d515d}")
        );
        assert_eq!(parse_guid("Windows RE 状态: Enabled"), None);
        assert_eq!(parse_guid(""), None);
    }

    #[test]
    fn empty_guid_is_refused_before_any_bcd_write() {
        // 这条是硬约束：空 GUID 会让 bcdedit 作用于 {default}（真实事故）。
        let error = require_guid("", "test").unwrap_err();
        assert!(error.contains("refusing to touch"), "{error}");
        assert!(require_guid("{bootmgr}", "test").is_err());
        assert!(require_guid("{ccb31ee5-bbb7-11f1-88f5-cbcfb69d515d}", "test").is_ok());
    }

    #[test]
    fn device_options_guid_comes_from_the_device_line_not_the_first_guid() {
        // 实机形态：第一个 GUID 是 osloader 自己，设备选项对象只在 device/osdevice 行里。
        let enum_text = "Windows 启动加载器\r\n-------------------\r\n标识符                  {ccb31eed-bbb7-11f1-88f5-cbcfb69d515d}\r\ndevice                  ramdisk=[P:]\\Recovery\\WindowsRE\\Winre.wim,{ccb31eee-bbb7-11f1-88f5-cbcfb69d515d}\r\nosdevice                ramdisk=[P:]\\Recovery\\WindowsRE\\Winre.wim,{ccb31eee-bbb7-11f1-88f5-cbcfb69d515d}\r\n";
        assert_eq!(
            ramdisk_device_options_guid(enum_text).as_deref(),
            Some("{ccb31eee-bbb7-11f1-88f5-cbcfb69d515d}")
        );
        assert_eq!(ramdisk_device_options_guid("device partition=C:"), None);
        assert_eq!(ramdisk_device_options_guid(""), None);
    }

    #[test]
    fn staging_sdi_path_always_starts_with_a_backslash() {
        assert_eq!(
            staging_sdi_path("BackupRestoreRE"),
            r"\BackupRestoreRE\boot.sdi"
        );
        assert_eq!(
            staging_sdi_path(r"\BackupRestoreRE"),
            r"\BackupRestoreRE\boot.sdi"
        );
        // 载荷按任务隔离后，SDI 也落在任务自己的子目录里；
        // 实际传入的始终是 boot_cleanup::staging_relative_dir 的返回值。
        const TASK: &str = "11111111-2222-4333-8444-555555555555";
        assert_eq!(
            staging_sdi_path(&crate::boot_cleanup::staging_relative_dir(TASK).unwrap()),
            format!(r"\BackupRestoreRE\{TASK}\boot.sdi")
        );
    }

    /// 这条断言直接照抄实机 `bcdedit /enum` 回显：方括号只包卷，`]` 紧跟卷后闭合。
    /// 参照物是当前注册的 WinRE 条目：
    /// `device ramdisk=[C:]\Recovery\WindowsRE\Winre.wim,{b69adf6a-…}`。
    #[test]
    fn ramdisk_spec_closes_the_bracket_right_after_the_volume() {
        let spec = ramdisk_spec(
            r"F:\BackupRestoreRE\Winre.wim",
            "{11111111-2222-3333-4444-555555555555}",
        );
        assert_eq!(
            spec,
            r"ramdisk=[F:]\BackupRestoreRE\Winre.wim,{11111111-2222-3333-4444-555555555555}"
        );
        assert_eq!(spec.matches('[').count(), 1);
        assert_eq!(spec.matches(']').count(), 1);
    }

    /// 两种历史畸形形态都必须不再出现：
    /// * `[整条路径],{…}`  —— bcdedit 报「按规定设备无效」
    /// * `[整条路径,{…}`   —— bcdedit 报「指定的设备无效」（2026-09-29 ProcMon 实锤）
    #[test]
    fn ramdisk_spec_rejects_both_historical_malformed_shapes() {
        let spec = ramdisk_spec(
            r"F:\BackupRestoreRE\Winre.wim",
            "{dead0000-0000-0000-0000-000000000000}",
        );
        assert!(!spec.contains(r"[F:\"), "方括号里不能出现路径：{spec}");
        assert!(spec.contains("[F:]"), "方括号里必须正好是卷：{spec}");
        assert!(!spec.contains("],{"), "`]` 不能落在逗号前：{spec}");
    }

    /// 卷内相对路径（没有盘符）时不许瞎猜盘符，只补开头反斜杠。
    #[test]
    fn ramdisk_spec_without_a_drive_letter_keeps_the_volume_empty() {
        assert_eq!(
            ramdisk_spec(r"BackupRestoreRE\Winre.wim", "{1-1}"),
            r"ramdisk=[]\BackupRestoreRE\Winre.wim,{1-1}"
        );
    }

    /// 锁死「`{bootmgr}` 是知名别名、不是 GUID」这条实测结论（2026-09-29 实机报错：
    /// `BCD enum: not a valid GUID: {bootmgr}`，卡在 v1.7.12 第一次 verify）。
    #[test]
    fn require_identifier_accepts_the_bootmgr_alias() {
        assert_eq!(
            require_identifier("{bootmgr}", "enum").unwrap(),
            "{bootmgr}"
        );
        assert_eq!(
            require_identifier(" {BOOTMGR} ", "enum").unwrap(),
            "{BOOTMGR}"
        );
    }

    #[test]
    fn require_identifier_still_rejects_default_and_other_aliases() {
        for rejected in [
            "{default}",
            "{current}",
            "{ntldr}",
            "{fwbootmgr}",
            "{ramdiskoptions}",
        ] {
            assert!(
                require_identifier(rejected, "enum").is_err(),
                "{rejected} 必须被拒，否则可能误伤用户启动项"
            );
        }
        assert!(require_identifier("", "enum").is_err());
        assert!(require_identifier("  ", "enum").is_err());
        assert!(require_identifier("invalid-guid", "enum").is_err());
    }

    const MACHINE_ENUM: &str = "Windows 启动管理器\n--------------------\n标识符                  {9dea862c-5cdd-4e70-acc1-f32b344d4795}\ndefault                 {a6273652-d15f-11f1-aa4b-ff70ff0b7794}\ndisplayorder            {a6273652-d15f-11f1-aa4b-ff70ff0b7794}\n                        {831787dd-d2e1-11f1-89db-f371a71018a0}\ntoolsdisplayorder       {b2721d73-1db4-4c62-bf78-c548a880142d}\ntimeout                 10\n\nWindows 启动加载器\n-------------------\n标识符                  {831787dd-d2e1-11f1-89db-f371a71018a0}\ndevice                  partition=T:\npath                    \\Windows\\system32\\winload.efi\ndescription             BR-PE-DISK\nosdevice                partition=T:\nsystemroot              \\Windows\nnx                      OptIn\ndetecthal               Yes\nwinpe                   Yes\n\nWindows 启动加载器\n-------------------\n标识符                  {a6273652-d15f-11f1-aa4b-ff70ff0b7794}\ndevice                  partition=C:\npath                    \\Windows\\system32\\winload.efi\ndescription             本次还原的 Windows\nosdevice                partition=C:\nsystemroot              \\Windows\nnx                      OptIn\n";

    #[test]
    fn display_order_is_read_across_continuation_lines_and_stops_at_toolsdisplayorder() {
        assert_eq!(
            bcd_display_order(MACHINE_ENUM),
            vec![
                "{a6273652-d15f-11f1-aa4b-ff70ff0b7794}",
                "{831787dd-d2e1-11f1-89db-f371a71018a0}",
            ]
        );
    }

    #[test]
    fn boot_manager_default_is_read_and_kept_lowercase() {
        assert_eq!(
            bcd_boot_manager_default(MACHINE_ENUM).as_deref(),
            Some("{a6273652-d15f-11f1-aa4b-ff70ff0b7794}")
        );
    }

    #[test]
    fn windows_loader_is_picked_and_the_pe_entry_is_skipped() {
        let picked = pick_windows_loader(MACHINE_ENUM, |letter| letter == 'C' || letter == 'T');
        assert_eq!(
            picked.as_deref(),
            Some("{a6273652-d15f-11f1-aa4b-ff70ff0b7794}")
        );
    }

    /// 最危险的错法：把 PE 条目当成 Windows 写进 default → 重启又回 PE。
    #[test]
    fn pe_only_store_yields_nothing_instead_of_the_pe_entry() {
        let pe_only = MACHINE_ENUM
            .split("\n\n")
            .filter(|block| !block.contains("{a6273652"))
            .collect::<Vec<_>>()
            .join("\n\n");
        assert_eq!(pick_windows_loader(&pe_only, |_| true), None);
    }

    /// 盘符在 PE 里可能不指同一个卷：加载器文件不存在就不能当选。
    #[test]
    fn candidate_needs_the_loader_file_to_exist_on_that_letter() {
        assert_eq!(pick_windows_loader(MACHINE_ENUM, |letter| letter == 'T'), None);
    }

    /// 多候选时按启动菜单顺序取第一条（而不是输出里的顺序）。
    #[test]
    fn display_order_decides_between_two_candidates() {
        let two_windows = "Windows 启动管理器\nidentifier {9dea862c-5cdd-4e70-acc1-f32b344d4795}\ndefault {11111111-1111-1111-1111-111111111111}\ndisplayorder {22222222-2222-2222-2222-222222222222}\n             {11111111-1111-1111-1111-111111111111}\n\nWindows Boot Loader\nidentifier {11111111-1111-1111-1111-111111111111}\ndevice partition=C:\npath \\Windows\\system32\\winload.efi\n\nWindows Boot Loader\nidentifier {22222222-2222-2222-2222-222222222222}\ndevice partition=D:\npath \\Windows\\system32\\winload.efi\n";
        assert_eq!(
            pick_windows_loader(two_windows, |_| true).as_deref(),
            Some("{22222222-2222-2222-2222-222222222222}")
        );
    }

    /// 英文标签同样要认（同一台机器换了系统语言就不能瞎）。
    #[test]
    fn english_labels_and_ramdisk_entries_are_handled() {
        let english = "Windows Boot Manager\nidentifier {9dea862c-5cdd-4e70-acc1-f32b344d4795}\ndefault {aaaaaaaa-1111-1111-1111-111111111111}\ndisplayorder {aaaaaaaa-1111-1111-1111-111111111111}\n\nWindows Boot Loader\nidentifier {aaaaaaaa-1111-1111-1111-111111111111}\ndevice partition=C:\npath \\Windows\\system32\\winload.efi\n\nWindows Boot Loader\nidentifier {bbbbbbbb-1111-1111-1111-111111111111}\ndevice ramdisk=[C:]\\Recovery\\WindowsRE\\Winre.wim,{cccccccc-1111-1111-1111-111111111111}\npath \\windows\\system32\\winload.efi\nwinpe Yes\n";
        assert_eq!(
            pick_windows_loader(english, |_| true).as_deref(),
            Some("{aaaaaaaa-1111-1111-1111-111111111111}")
        );
    }

    /// ramdisk 条目（恢复环境的典型形态）没有 `partition=X:`，不能被当成候选。
    #[test]
    fn ramdisk_entries_are_not_candidates() {
        let ramdisk_only = "Windows Boot Loader\nidentifier {bbbbbbbb-1111-1111-1111-111111111111}\ndevice ramdisk=[C:]\\Recovery\\WindowsRE\\Winre.wim,{cccccccc-1111-1111-1111-111111111111}\npath \\windows\\system32\\winload.efi\n";
        assert_eq!(pick_windows_loader(ramdisk_only, |_| true), None);
    }
}
