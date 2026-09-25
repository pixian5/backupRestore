//! Pure text and output parsing shared by the Windows-only GUI, the WinRE
//! progress window and the recovery entry point.
//!
//! Everything here is platform independent on purpose. `native_gui.rs`,
//! `recovery_progress.rs` and the `recover-env` mount path are compiled only
//! on Windows, so logic that lives inside them cannot be exercised by
//! `cargo test` on the development machine. Historically that blind spot is
//! where the expensive regressions came from: the DISM index-name quoting bug
//! (`exit=87`), bcdedit's GBK/UTF-16 output, and conflating "letter is free"
//! with "the query never answered". Keeping these functions here means every
//! one of them is covered by the offline test suite on macOS.

use std::collections::BTreeMap;

use serde_json::Value;

/// What a `mountvol <letter>: /L` query actually told us.
///
/// The three cases must stay distinct. Treating `Unknown` as `Unmounted` is
/// what allowed the recovery path to walk past its own "refusing to replace
/// an existing mount" guard whenever the query timed out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VolumeMountQuery {
    /// The letter is mounted to this volume GUID path.
    Mounted(String),
    /// The letter has no volume mounted; assigning it is safe.
    Unmounted,
    /// The query did not answer (timeout) or answered unusably. The letter's
    /// state is unproven, so callers must not assign over it.
    Unknown,
}

/// Classify a finished `mountvol <letter>: /L` run.
///
/// A non-zero exit is how `mountvol` reports an unassigned letter, so that
/// stays `Unmounted`; the post-assignment identity re-check is the safety net
/// for the rarer case where the failure meant something else. A successful run
/// with no usable output proves nothing and must be `Unknown`.
pub(crate) fn classify_mountvol_output(
    exited_successfully: bool,
    stdout: &str,
) -> VolumeMountQuery {
    if !exited_successfully {
        return VolumeMountQuery::Unmounted;
    }
    match stdout.lines().map(str::trim).find(|line| !line.is_empty()) {
        Some(guid) => VolumeMountQuery::Mounted(guid.to_string()),
        None => VolumeMountQuery::Unknown,
    }
}

/// Read a JSON field as display text, accepting the string, number and bool
/// spellings that DISM and our own reports mix.
pub(crate) fn json_text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|item| {
            item.as_str()
                .map(ToOwned::to_owned)
                .or_else(|| item.as_u64().map(|number| number.to_string()))
                .or_else(|| item.as_i64().map(|number| number.to_string()))
                .or_else(|| item.as_bool().map(|flag| flag.to_string()))
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
pub(crate) struct WimImageInfo {
    pub(crate) index: u32,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) version: String,
    pub(crate) architecture: String,
    pub(crate) edition: String,
    pub(crate) installation_type: String,
    pub(crate) size_bytes: Option<u64>,
}

/// Unwrap the shapes DISM's WIM metadata arrives in: a bare array, a single
/// image object, or an `images` wrapper around either.
pub(crate) fn wim_image_items(value: &Value) -> Result<Vec<Value>, String> {
    if let Some(items) = value.as_array() {
        return Ok(items.clone());
    }
    if value.get("ImageIndex").is_some() || value.get("imageIndex").is_some() {
        return Ok(vec![value.clone()]);
    }
    if let Some(images) = value.get("images") {
        return wim_image_items(images);
    }
    Err("WIM metadata did not return an image object, array or images wrapper".to_string())
}

pub(crate) fn parse_wim_images(output: &str) -> Result<Vec<WimImageInfo>, String> {
    let value: Value = serde_json::from_str(output)
        .map_err(|error| format!("WIM metadata JSON parse failed: {error}"))?;
    let items = wim_image_items(&value)?;
    let mut images = Vec::with_capacity(items.len());
    for item in items {
        let index = json_text(&item, "ImageIndex")
            .parse::<u32>()
            .map_err(|_| "WIM metadata contains an invalid image index".to_string())?;
        if index == 0 {
            return Err("WIM metadata contains image index 0".to_string());
        }
        let size_bytes = json_text(&item, "ImageSize").parse::<u64>().ok();
        images.push(WimImageInfo {
            index,
            name: json_text(&item, "ImageName"),
            description: json_text(&item, "ImageDescription"),
            version: json_text(&item, "ImageVersion"),
            architecture: json_text(&item, "Architecture"),
            edition: json_text(&item, "EditionId"),
            installation_type: json_text(&item, "InstallationType"),
            size_bytes,
        });
    }
    if images.is_empty() {
        return Err("WIM contains no selectable image indexes".to_string());
    }
    Ok(images)
}

pub(crate) fn format_bytes(size: Option<u64>) -> String {
    let Some(size) = size else {
        return "?".to_string();
    };
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = size as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Quote a value before it is concatenated into a `cmd.exe` command string.
///
/// DISM rejected index names containing spaces with `exit=87` until this was
/// applied at every `run_cmd_to_file*` call site.
pub(crate) fn quote_argument(value: &str) -> String {
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// Decode a bcdedit output file's bytes.
///
/// bcdedit writes UTF-16LE on Chinese/Japanese systems, sometimes without a
/// BOM, so detect the interleaved-NUL pattern as well as the BOM.
pub(crate) fn decode_bcdedit_bytes(bytes: &[u8]) -> String {
    let nul_count = bytes.iter().filter(|&&b| b == 0).count();
    if bytes.starts_with(&[0xFF, 0xFE]) || (bytes.len() >= 2 && nul_count > bytes.len() / 4) {
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
    } else {
        String::from_utf8_lossy(bytes).to_string()
    }
}

/// Return the first `{...}` GUID in `text` WITHOUT its braces; callers wrap
/// with `{}` as needed.
pub(crate) fn first_braced_guid(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text[start..].find('}')? + start;
    Some(text[start + 1..end].to_string())
}

/// Parse the percentage out of a DISM progress line, e.g.
/// `[stdout] [=  45.6% =]` → `Some(45)`. Find a `%`, walk back to the nearest
/// `[`, and keep only digits and the decimal point in between.
pub(crate) fn parse_percent(line: &str) -> Option<u32> {
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'%' {
            continue;
        }
        let mut start = 0;
        for back in (0..index).rev() {
            if bytes[back] == b'[' {
                start = back + 1;
                break;
            }
        }
        let digits: String = line[start..index]
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(value) = digits.parse::<f32>()
            && (0.0..=100.0).contains(&value)
        {
            return Some(value as u32);
        }
    }
    None
}

/// Derive the progress window's stage caption, percentage and detail line
/// from one recovery log line.
pub(crate) fn classify_log_line(line: &str) -> (Option<String>, Option<u32>, Option<String>) {
    let mut stage = None;
    if line.contains("running dism.exe") {
        if line.contains("/Capture-Image") {
            stage = Some("正在备份系统分区…".to_string());
        } else if line.contains("/Apply-Image") {
            stage = Some("正在还原系统分区…".to_string());
        }
    } else if line.contains("Operating on the") || line.contains("Scanning") {
        stage = Some("正在扫描系统分区…".to_string());
    } else if line.contains("Saving image") {
        stage = Some("正在备份系统分区…".to_string());
    } else if line.contains("Applying image") {
        stage = Some("正在还原系统分区…".to_string());
    } else if line.contains("Backup capture finished") {
        stage = Some("备份完成，正在校验镜像…".to_string());
    } else if line.contains("The operation completed successfully")
        || line.contains("Recovery completed")
    {
        stage = Some("操作完成".to_string());
    } else if line.contains("Recovery.exe started") || line.contains("started from env") {
        stage = Some("正在准备恢复环境…".to_string());
    }
    let percent = parse_percent(line);
    let detail = if line.trim().is_empty()
        || ((line.starts_with("[stdout] [") || line.starts_with('[')) && line.contains('%'))
    {
        None // 空行和纯进度行不占详情
    } else {
        Some(line.trim_end().to_string())
    };
    (stage, percent, detail)
}

/// Keep only the `systeminfo` lines that describe CPU and memory, so the
/// report stays readable instead of dumping the whole hotfix list.
pub(crate) fn systeminfo_cpu_memory(systeminfo: &str) -> String {
    let wanted = [
        "处理器",
        "Processor",
        "物理内存",
        "Physical Memory",
        "虚拟内存",
        "Virtual Memory",
        "系统类型",
        "System Type",
    ];
    let mut kept: Vec<String> = Vec::new();
    let mut in_processor_list = false;
    for line in systeminfo.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            continue;
        }
        // `systeminfo` prints processors as an indented numbered sub-list
        // under its own heading; keep those continuation lines too.
        let starts_field = !line.starts_with(' ') && line.contains(':');
        if starts_field {
            in_processor_list = wanted
                .iter()
                .any(|needle| trimmed.starts_with(needle) || trimmed.contains(needle));
            if in_processor_list {
                kept.push(trimmed.to_string());
            }
            continue;
        }
        if in_processor_list {
            kept.push(trimmed.to_string());
        }
    }
    kept.join("\n")
}

/// Which volume GUID an earlier role already mounted, and at which letter.
///
/// diskpart's `assign letter=` MOVES a volume's drive letter instead of adding
/// a second one, so mounting one volume twice under two roles silently tears
/// the first role's mount down. That is not hypothetical: with WinRE
/// registered on the OS partition, RECOVERY and SOURCE are the same volume,
/// and assigning `S:` stole `R:`, leaving the cleanup unable to find
/// `R:\Recovery\WindowsRE` and the payload-injected WinRE registered with no
/// way back. Roles that share a volume must therefore share its letter.
#[derive(Debug, Default)]
pub(crate) struct MountedVolumes {
    by_guid: BTreeMap<String, char>,
}

impl MountedVolumes {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The letter an earlier role already mounted this volume at, if any.
    /// Volume GUIDs are compared case-insensitively.
    pub(crate) fn existing(&self, volume_guid: &str) -> Option<char> {
        self.by_guid.get(&Self::key(volume_guid)).copied()
    }

    /// Record the letter a role just mounted this volume at.
    pub(crate) fn record(&mut self, volume_guid: &str, letter: char) {
        self.by_guid.insert(Self::key(volume_guid), letter);
    }

    fn key(volume_guid: &str) -> String {
        volume_guid.to_ascii_uppercase()
    }
}

/// Raw, read-only command outputs used by the recovery desktop information window.
#[derive(Debug, Default)]
pub(crate) struct SystemInfoSources {
    pub(crate) hostname: String,
    pub(crate) systeminfo: String,
    pub(crate) os_registry: String,
    pub(crate) bios_registry: String,
    pub(crate) disks: String,
    pub(crate) volumes: String,
    pub(crate) display_devices: String,
    pub(crate) resolution: String,
    pub(crate) network: String,
    pub(crate) recovery: String,
    pub(crate) bitlocker: String,
    pub(crate) secure_boot: String,
}

const SYSTEM_INFO_UNAVAILABLE: &str = "未检测到/不可用";

fn system_info_value(value: &str) -> &str {
    if value.trim().is_empty() {
        SYSTEM_INFO_UNAVAILABLE
    } else {
        value.trim()
    }
}

/// Build one stable, scrollable report without making any optional probe a
/// fatal error. WinRE may lack PowerShell, WMI, display drivers or BitLocker
/// support, so every optional source falls back to a visible unavailable note.
pub(crate) fn format_system_info_report(
    sources: &SystemInfoSources,
    environment: &str,
    architecture: &str,
    collected_at: &str,
) -> String {
    // The array mixes borrowed literals with owned `format!` results, so every
    // entry is normalised to `String` at the point of construction.
    [
        "BackupRestore 软硬件信息".to_string(),
        "========================".to_string(),
        "说明：本窗口只读采集，不修改系统配置。".to_string(),
        "".to_string(),
        "【概览】".to_string(),
        format!("计算机名：{}", system_info_value(&sources.hostname)),
        format!("当前环境：{}", system_info_value(environment)),
        format!("程序版本：v{}", env!("CARGO_PKG_VERSION")),
        format!("采集时间：{}", system_info_value(collected_at)),
        format!("架构：{}", system_info_value(architecture)),
        "".to_string(),
        "【操作系统】".to_string(),
        system_info_value(&sources.os_registry).to_string(),
        "".to_string(),
        "【处理器与内存】".to_string(),
        system_info_value(&sources.systeminfo).to_string(),
        "".to_string(),
        "【主板与固件】".to_string(),
        system_info_value(&sources.bios_registry).to_string(),
        "".to_string(),
        "【显示设备】".to_string(),
        format!("当前分辨率：{}", system_info_value(&sources.resolution)),
        system_info_value(&sources.display_devices).to_string(),
        "".to_string(),
        "【存储】".to_string(),
        "磁盘：".to_string(),
        system_info_value(&sources.disks).to_string(),
        "".to_string(),
        "卷：".to_string(),
        system_info_value(&sources.volumes).to_string(),
        "".to_string(),
        "【网络】".to_string(),
        system_info_value(&sources.network).to_string(),
        "".to_string(),
        "【恢复与安全】".to_string(),
        "Windows RE：".to_string(),
        system_info_value(&sources.recovery).to_string(),
        "".to_string(),
        "BitLocker：".to_string(),
        system_info_value(&sources.bitlocker).to_string(),
        "".to_string(),
        "Secure Boot：".to_string(),
        system_info_value(&sources.secure_boot).to_string(),
        "".to_string(),
        "【按钮边界】".to_string(),
        "“返回 Windows”只恢复/设置 BCD 默认项 {current}、清理一次性启动状态并重启；".to_string(),
        "它不会还原原始 Winre.wim。恢复原始 WinRE 必须另行执行 WinRE 恢复流程。".to_string(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn system_info_report_has_stable_groups_and_safe_fallbacks() {
        let report = format_system_info_report(
            &SystemInfoSources::default(),
            "Windows RE",
            "ARM64",
            "2026-09-25 12:00:00",
        );
        for heading in [
            "【概览】",
            "【操作系统】",
            "【处理器与内存】",
            "【主板与固件】",
            "【显示设备】",
            "【存储】",
            "【网络】",
            "【恢复与安全】",
            "【按钮边界】",
        ] {
            assert!(report.contains(heading), "missing heading: {heading}");
        }
        assert!(report.contains("当前环境：Windows RE"));
        assert!(report.contains("架构：ARM64"));
        assert!(report.contains("未检测到/不可用"));
        assert!(report.contains("不会还原原始 Winre.wim"));
    }

    #[test]
    fn system_info_report_preserves_available_raw_outputs() {
        let sources = SystemInfoSources {
            hostname: "TEST-PC".to_string(),
            systeminfo: "CPU: Test CPU".to_string(),
            os_registry: "DisplayVersion=25H2".to_string(),
            bios_registry: "SystemManufacturer=Contoso".to_string(),
            disks: "Disk 0  Online".to_string(),
            volumes: "Volume 1  C:  NTFS".to_string(),
            display_devices: "GPU: Test GPU".to_string(),
            resolution: "1920x1080".to_string(),
            network: "IPv4: 192.0.2.10".to_string(),
            recovery: "Windows RE enabled".to_string(),
            bitlocker: "Protection On".to_string(),
            secure_boot: "UEFISecureBootEnabled=1".to_string(),
        };
        let report = format_system_info_report(&sources, "Windows", "x86_64", "now");
        for expected in [
            "计算机名：TEST-PC",
            "CPU: Test CPU",
            "DisplayVersion=25H2",
            "SystemManufacturer=Contoso",
            "Disk 0  Online",
            "Volume 1  C:  NTFS",
            "GPU: Test GPU",
            "当前分辨率：1920x1080",
            "IPv4: 192.0.2.10",
            "Windows RE enabled",
            "Protection On",
            "UEFISecureBootEnabled=1",
        ] {
            assert!(report.contains(expected), "missing output: {expected}");
        }
    }

    #[test]
    fn mountvol_query_keeps_timeout_distinct_from_unmounted() {
        assert_eq!(
            classify_mountvol_output(true, "\\\\?\\Volume{1234}\\\r\n"),
            VolumeMountQuery::Mounted("\\\\?\\Volume{1234}\\".to_string())
        );
        // A non-zero exit is how mountvol reports an unassigned letter.
        assert_eq!(
            classify_mountvol_output(false, ""),
            VolumeMountQuery::Unmounted
        );
        // Success with nothing usable proves nothing; it must not read as free.
        assert_eq!(
            classify_mountvol_output(true, ""),
            VolumeMountQuery::Unknown
        );
        assert_eq!(
            classify_mountvol_output(true, "   \r\n  \r\n"),
            VolumeMountQuery::Unknown
        );
        assert_ne!(
            classify_mountvol_output(true, ""),
            VolumeMountQuery::Unmounted
        );
    }

    #[test]
    fn mountvol_query_skips_leading_blank_lines() {
        assert_eq!(
            classify_mountvol_output(true, "\r\n\r\n    \\\\?\\Volume{abcd}\\   \r\n"),
            VolumeMountQuery::Mounted("\\\\?\\Volume{abcd}\\".to_string())
        );
    }

    #[test]
    fn json_text_reads_string_number_and_boolean_values() {
        let value = json!({
            "hasWindowsInstallation": true,
            "name": "C",
            "count": 7,
            "negative": -3,
        });
        assert_eq!(json_text(&value, "hasWindowsInstallation"), "true");
        assert_eq!(json_text(&value, "name"), "C");
        assert_eq!(json_text(&value, "count"), "7");
        assert_eq!(json_text(&value, "negative"), "-3");
        assert_eq!(json_text(&value, "missing"), "");
        assert_eq!(
            json_text(
                &json!({"hasWindowsInstallation": false}),
                "hasWindowsInstallation"
            ),
            "false"
        );
    }

    #[test]
    fn wim_metadata_accepts_array_single_object_and_wrapper() {
        let array = r#"[{"ImageIndex":"1"},{"ImageIndex":"2"}]"#;
        assert_eq!(parse_wim_images(array).unwrap().len(), 2);

        let single = r#"{"ImageIndex":1,"ImageName":"Win11"}"#;
        let images = parse_wim_images(single).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].index, 1);
        assert_eq!(images[0].name, "Win11");

        let wrapped = r#"{"images":[{"ImageIndex":3,"ImageName":"备份 2026"}]}"#;
        let images = parse_wim_images(wrapped).unwrap();
        assert_eq!(images[0].index, 3);
        assert_eq!(images[0].name, "备份 2026");
    }

    #[test]
    fn wim_metadata_rejects_unusable_indexes_and_shapes() {
        assert!(parse_wim_images("not json").is_err());
        assert!(parse_wim_images(r#"{"ImageIndex":0}"#).is_err());
        assert!(parse_wim_images(r#"{"ImageIndex":"abc"}"#).is_err());
        assert!(parse_wim_images("[]").is_err());
        assert!(parse_wim_images(r#"{"unrelated":1}"#).is_err());
    }

    #[test]
    fn wim_metadata_keeps_all_display_fields_and_optional_size() {
        let images = parse_wim_images(
            r#"[{"ImageIndex":2,"ImageName":"n","ImageDescription":"d","ImageVersion":"10.0.26100",
                 "Architecture":"ARM64","EditionId":"Professional","InstallationType":"Client",
                 "ImageSize":"1048576"}]"#,
        )
        .unwrap();
        let image = &images[0];
        assert_eq!(image.description, "d");
        assert_eq!(image.version, "10.0.26100");
        assert_eq!(image.architecture, "ARM64");
        assert_eq!(image.edition, "Professional");
        assert_eq!(image.installation_type, "Client");
        assert_eq!(image.size_bytes, Some(1_048_576));

        let no_size = parse_wim_images(r#"[{"ImageIndex":1}]"#).unwrap();
        assert_eq!(no_size[0].size_bytes, None);
    }

    #[test]
    fn byte_sizes_scale_and_report_unknown() {
        assert_eq!(format_bytes(None), "?");
        assert_eq!(format_bytes(Some(0)), "0 B");
        assert_eq!(format_bytes(Some(1023)), "1023 B");
        assert_eq!(format_bytes(Some(1024)), "1.0 KiB");
        assert_eq!(format_bytes(Some(1536)), "1.5 KiB");
        assert_eq!(format_bytes(Some(1024 * 1024)), "1.0 MiB");
        assert_eq!(format_bytes(Some(1024 * 1024 * 1024)), "1.0 GiB");
        // Saturates at GiB rather than inventing a TiB unit.
        assert_eq!(
            format_bytes(Some(2 * 1024 * 1024 * 1024 * 1024)),
            "2048.0 GiB"
        );
    }

    #[test]
    fn command_arguments_quote_spaces_quotes_and_empty_values() {
        // The exit=87 regression: an index name with a space must be quoted.
        assert_eq!(quote_argument("备份 2026"), "\"备份 2026\"");
        assert_eq!(quote_argument("plain"), "plain");
        assert_eq!(quote_argument(""), "\"\"");
        assert_eq!(quote_argument("a\tb"), "\"a\tb\"");
        assert_eq!(quote_argument("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_argument(r"C:\dir\file.wim"), r"C:\dir\file.wim");
        assert_eq!(
            quote_argument(r"C:\my dir\file.wim"),
            "\"C:\\my dir\\file.wim\""
        );
    }

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
    fn dism_percent_parses_decimals_and_rejects_out_of_range() {
        assert_eq!(parse_percent("[stdout] [=  45.6% =]"), Some(45));
        assert_eq!(parse_percent("[stdout] [==100.0%==]"), Some(100));
        assert_eq!(parse_percent("[  0.0%  ]"), Some(0));
        assert_eq!(parse_percent("no percent sign"), None);
        assert_eq!(parse_percent("[999%]"), None);
        assert_eq!(parse_percent("[abc%]"), None);
    }

    #[test]
    fn log_lines_map_to_stage_percent_and_detail() {
        let (stage, percent, detail) =
            classify_log_line("running dism.exe /Capture-Image /ImageFile:E:\\1.wim");
        assert_eq!(stage.as_deref(), Some("正在备份系统分区…"));
        assert_eq!(percent, None);
        assert!(detail.is_some());

        assert_eq!(
            classify_log_line("running dism.exe /Apply-Image")
                .0
                .as_deref(),
            Some("正在还原系统分区…")
        );
        assert_eq!(
            classify_log_line("Recovery completed").0.as_deref(),
            Some("操作完成")
        );
        assert_eq!(
            classify_log_line("Backup capture finished").0.as_deref(),
            Some("备份完成，正在校验镜像…")
        );
        assert_eq!(
            classify_log_line("Recovery.exe started from env")
                .0
                .as_deref(),
            Some("正在准备恢复环境…")
        );
        assert_eq!(
            classify_log_line("Scanning the system partition")
                .0
                .as_deref(),
            Some("正在扫描系统分区…")
        );
    }

    #[test]
    fn progress_only_lines_and_blanks_leave_the_detail_box_alone() {
        assert_eq!(classify_log_line("[stdout] [=  45.6% =]").2, None);
        assert_eq!(classify_log_line("   ").2, None);
        assert_eq!(classify_log_line("[= 10% =]").2, None);
        // A real message keeps its detail even when it carries a percentage
        // only in a non-bracketed position.
        assert_eq!(
            classify_log_line("copied 10% of the payload").2.as_deref(),
            Some("copied 10% of the payload")
        );
        assert_eq!(
            classify_log_line("plain message\r\n").2.as_deref(),
            Some("plain message")
        );
    }

    #[test]
    fn shared_volumes_reuse_one_letter_across_roles() {
        let mut mounts = MountedVolumes::new();
        let guid = "\\\\?\\Volume{761230e8-107c-4396-8c37-82273720183d}\\";
        assert_eq!(mounts.existing(guid), None);

        mounts.record(guid, 'R');
        // RECOVERY and SOURCE are the same volume when WinRE lives on the OS
        // partition; SOURCE must reuse R: instead of assigning S: over it.
        assert_eq!(mounts.existing(guid), Some('R'));

        // A different volume is still unmounted and gets its own letter.
        let other = "\\\\?\\Volume{93ec4ed8-db74-4ebb-8b73-cd771c07ac72}\\";
        assert_eq!(mounts.existing(other), None);
        mounts.record(other, 'S');
        assert_eq!(mounts.existing(other), Some('S'));
        assert_eq!(mounts.existing(guid), Some('R'));
    }

    #[test]
    fn volume_guid_matching_ignores_case() {
        let mut mounts = MountedVolumes::new();
        mounts.record("\\\\?\\Volume{ABCD-1234}\\", 'T');
        assert_eq!(mounts.existing("\\\\?\\volume{abcd-1234}\\"), Some('T'));
    }

    #[test]
    fn systeminfo_keeps_cpu_and_memory_and_drops_the_hotfix_list() {
        let raw = "\
主机名:           TEST-PC
OS 名称:          Microsoft Windows 11
系统类型:         ARM64-based PC
处理器:           安装了 1 个处理器。
                  [01]: ARMv8 (64-bit) Family 8 Model 1
物理内存总量:     16,384 MB
可用的物理内存:   9,001 MB
虚拟内存: 最大值: 20,480 MB
修补程序: 安装了 3 个修补程序。
                  [01]: KB5000001
";
        let kept = systeminfo_cpu_memory(raw);
        assert!(kept.contains("处理器:"));
        assert!(kept.contains("[01]: ARMv8 (64-bit) Family 8 Model 1"));
        assert!(kept.contains("物理内存总量:"));
        assert!(kept.contains("系统类型:"));
        // The hotfix list and its continuation lines must not leak in.
        assert!(!kept.contains("修补程序"));
        assert!(!kept.contains("KB5000001"));
        assert!(!kept.contains("OS 名称"));
    }

    #[test]
    fn systeminfo_filter_survives_english_output_and_empty_input() {
        let english = "\
Host Name:                 TEST-PC
Processor(s):              1 Processor(s) Installed.
                           [01]: ARMv8
Total Physical Memory:     16,384 MB
Hotfix(s):                 1 Hotfix(s) Installed.
";
        let kept = systeminfo_cpu_memory(english);
        assert!(kept.contains("Processor(s):"));
        assert!(kept.contains("[01]: ARMv8"));
        assert!(kept.contains("Total Physical Memory:"));
        assert!(!kept.contains("Hotfix"));
        assert_eq!(systeminfo_cpu_memory(""), "");
    }
}
