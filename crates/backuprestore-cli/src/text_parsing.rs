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
//!
//! 历次抽离（同因：纯解析逻辑集中到跨平台模块，便于本机单测覆盖）：
//! - v2.9.6 `wim_parse`：WIM 元数据与 wimlib 进度解析；
//! - v2.9.7 `bcd_parse`：BCD 引导条目与设备路径规范化；
//! - v2.9.8 `step_progress`：任务日志解析与步骤追踪；
//! - v2.9.9 `volume_parse`：卷挂载判定、设备路径规范化与盘符时间线。

pub(crate) use crate::wim_parse::*;
pub(crate) use crate::bcd_parse::*;
pub(crate) use crate::step_progress::*;
// 卷解析的调用方（mountvol/盘符相关）多在 `#[cfg(windows)]` 函数体内，
// 非 Windows 的纯 `cargo check` 会把这条再导出误报成未使用；带 `--tests`
// 或在 Windows 上构建时均有真实使用点，故显式豁免。
#[allow(unused_imports)]
pub(crate) use crate::volume_parse::*;

/// Map the two-entry compression dropdown to DISM terms.
///
/// This mapping lives outside `native_gui.rs` so macOS tests can prove that
/// index 0 is "压缩"/`fast`, index 1 is "不压缩"/`none`, no third option maps
/// anywhere, and an unselected control safely keeps the default `fast`.
pub(crate) fn compression_from_ui_index(index: Option<usize>) -> Result<&'static str, String> {
    match index {
        None => Ok("fast"),
        Some(0) => Ok("fast"),
        Some(1) => Ok("none"),
        Some(_) => Err("compression dropdown has an unsupported third option".to_string()),
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

/// Which character encoding a PE console tool's captured output actually uses.
///
/// `systeminfo`, `diskpart`, `ipconfig`, `reagentc` and `manage-bde` all write
/// in the **console OEM code page** (CP936/GBK on zh-CN, CP437 on en-US), while
/// `bcdedit` writes UTF-16LE and our own JSON payloads are UTF-8. Sniffing is
/// unavoidable; the live WinRE acceptance run proved that decoding GBK bytes as
/// UTF-8 turns every Chinese label into replacement characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConsoleBytes {
    /// UTF-16LE, with or without a BOM.
    Utf16,
    /// Strictly valid UTF-8 (already decoded here so callers cannot guess wrong).
    Utf8(String),
    /// Neither: must be converted from the console's OEM code page.
    Oem,
}

/// Classify console output bytes. UTF-8 is tried **before** the OEM branch so a
/// genuinely UTF-8 source is never re-decoded through GBK, and UTF-16 is tried
/// first because its NUL pattern is never valid UTF-8 text of interest.
pub(crate) fn plan_console_bytes(bytes: &[u8]) -> ConsoleBytes {
    if looks_like_utf16le(bytes) {
        return ConsoleBytes::Utf16;
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => ConsoleBytes::Utf8(text.to_string()),
        Err(_) => ConsoleBytes::Oem,
    }
}

/// 失败回执里附带的输出尾部上限（字节）。
///
/// DISM/wimlib 的 stdout/stderr 已由执行通道落进日志文件，但 RE 主链
/// （`main.rs::run_logged`）与在线链（`online_operation::dism`）的失败错误
/// 历来只回退出码——「DISM 为什么失败」全在输出里，不回传就等于现场无法
/// 归因（2026-10-24 D2 前置修复）。8 KB 足够覆盖 DISM 的错误块
/// （真实失败案的错误行都在输出末尾几行），又不把整份进度日志灌进错误。
pub(crate) const LOG_TAIL_MAX_BYTES: usize = 8 * 1024;

/// 读文本文件的尾部至多 `max_bytes` 字节，按 UTF-8 宽松解码成 `String`。
///
/// 跨平台纯文件读（macOS 单测可覆盖）；按字节截断 + `from_utf8_lossy`，
/// 不因半个多字节字符 panic。文件不存在/不可读/空文件一律返回空串——
/// 附不上输出不该让「命令失败」这个主错误变形。
pub(crate) fn tail_text(path: &std::path::Path, max_bytes: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let Ok(len) = file.seek(SeekFrom::End(0)) else {
        return String::new();
    };
    let start = len.saturating_sub(max_bytes as u64);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.take(max_bytes as u64).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// 取**内存中**字符串的尾部至多 `max_bytes` 字节，并且**不切坏 UTF-8**。
///
/// 与 [`tail_text`] 的分工：那个从文件读，读不到就返回空串（附不上尾巴不该让
/// 主错误变形）；这个用于输出**已经在手**的场合（`capture_logged` 走
/// `Command::output()` 收集完 stdout+stderr）。两者必须都能在 mac 上单测。
///
/// 为什么不直接 `&text[text.len() - max..]`：那样会在多字节字符中间切开直接
/// panic。这里从候选起点往后挪到最近的 char 边界，代价最多 3 字节。
pub(crate) fn tail_of_text(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Win32 multiline `EDIT` controls only break lines on `\r\n`. A lone `\n` is
/// stored as an invisible control character, which is how the information
/// window's report header collapsed into a single unreadable row.
pub(crate) fn normalize_edit_newlines(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

// ---------------------------------------------------------------------------
// 第一件：PE 镜像自带的启动配置（`Windows\System32\winpeshl.ini`）比对
// ---------------------------------------------------------------------------

/// 产品期望的 PE 启动配置：唯一一行启动项，指向我们自己的恢复桌面。
///
/// ★ 这是**两处权威之一**：另一处是生成镜像的脚本
/// `poc/build-backuprestore-pe.ps1` 里的 `$expectedPeShell`。改壳模板必须同时改
/// 这两处，否则会出现「产品拒绝自己刚生成的新镜像」的自锁。
pub(crate) const EXPECTED_PE_SHELL: &str = concat!(
    "[LaunchApps]\n",
    "%SYSTEMROOT%\\System32\\Recovery.exe,--pe-desktop\n"
);

/// 去掉 BOM、统一换行、去掉尾部空白。只做这些——判据是「逐字等于」，
/// 不能顺手做「每行 trim」之类会掩盖差异的加工。
pub(crate) fn normalize_shell_text(text: &str) -> String {
    text.trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .trim_end()
        .to_owned()
}

/// 从镜像里读到的启动配置有什么问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeShellProblem {
    /// 镜像里根本没有这个文件：WinPE 会退到自带的 `startnet.cmd`，停在命令行。
    Missing,
    /// 内容不是产品期望的那一份。
    Mismatch { bytes: usize, actual: String },
    /// 配置写对了，但它指向的 `Recovery.exe` 不在镜像里（半成品镜像）。
    RecoveryExeMissing,
}

impl PeShellProblem {
    /// 一行英文说明，直接进 GUI 日志（日志历史上都是英文，不混中文）。
    pub(crate) fn describe(&self) -> String {
        match self {
            PeShellProblem::Missing => {
                "PE image has no Windows/System32/winpeshl.ini: WinPE would fall back to \
                 startnet.cmd and stop at a command prompt"
                    .to_string()
            }
            PeShellProblem::Mismatch { bytes, actual } => format!(
                "PE image winpeshl.ini is not the expected shell ({bytes} bytes): {actual}"
            ),
            PeShellProblem::RecoveryExeMissing => {
                "PE image winpeshl.ini points at Recovery.exe but that file is not in the image"
                    .to_string()
            }
        }
    }
}

/// 判定所需的两个事实，由调用方去镜像里取（取法是平台相关的，不能放这里）。
pub(crate) struct PeShellFacts<'a> {
    /// 读到的 shell 原文；`None` = 镜像里没有这个文件。
    pub shell: Option<&'a str>,
    /// 镜像里是否存在 `Windows/System32/Recovery.exe`。
    pub recovery_exe_present: bool,
}

/// 三条判据全过 → `None`；否则返回**第一条**命中的问题（按严重程度排序）。
///
/// ★ 为什么必须「逐字等于」而不是「包含 `Recovery.exe`」：见
/// `docs/20261029-0014-PE安装前输入WIM壳契约校验方案`. 简单地说是——真正要表达的
/// 不变量是「WinPE 启动后唯一在前台跑的程序必须是我们自己的」，而任何子串类判据
/// 都表达不了它（并列第二个入口、或「至少有一个入口能起来」都会照样放过坏件）。
pub(crate) fn judge_pe_shell(facts: &PeShellFacts<'_>) -> Option<PeShellProblem> {
    let Some(shell) = facts.shell else {
        return Some(PeShellProblem::Missing);
    };
    let normalized = normalize_shell_text(shell);
    if normalized != normalize_shell_text(EXPECTED_PE_SHELL) {
        return Some(PeShellProblem::Mismatch {
            bytes: shell.len(),
            actual: truncate_chars(&normalized.replace('\n', " | "), 160),
        });
    }
    if !facts.recovery_exe_present {
        return Some(PeShellProblem::RecoveryExeMissing);
    }
    None
}

/// Keep only the value rows of a `reg query "<key>"` listing.
///
/// Without `/v`, `reg` prints the key's values and then **every subkey path**.
/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` has dozens of subkeys,
/// which turned the 操作系统 group into kilobytes of paths and pushed the useful
/// tail of the report past the edit control's text limit. Value rows are
/// indented, the header and the subkey list are not. When nothing is indented
/// the original text is returned unchanged so real errors stay visible.
pub(crate) fn reg_key_values(text: &str) -> String {
    let mut values: Vec<String> = Vec::new();
    let mut seen_header = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            continue;
        }
        if trimmed.starts_with(' ') || trimmed.starts_with('\t') {
            if seen_header {
                values.push(trimmed.trim_start().to_string());
            }
            continue;
        }
        if !seen_header {
            seen_header = true;
            continue;
        }
        // First unindented line after the header: the subkey list starts here.
        break;
    }
    if values.is_empty() {
        return text.trim().to_string();
    }
    values.join("\n")
}

/// 将可能包含 Windows 控制台/ANSI 代码页（如 CP936/GBK）的字节切片解码为 UTF-8 字符串。
/// 优先尝试 UTF-8，若失败在 Windows 下调用 MultiByteToWideChar（CP_OEMCP / CP_ACP）解码，
/// 彻底避免控制台输出中文被损坏成 \u{FFFD}（锟斤拷）。
pub(crate) fn decode_windows_bytes(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    #[cfg(windows)]
    for cp in [1, 0] {
        if let Some(value) = decode_code_page(bytes, cp) {
            return value;
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// 控制台代码页解码只保留这一份 ABI 声明；GUI 与日志解析器共用。
#[cfg(windows)]
pub(crate) fn decode_code_page(bytes: &[u8], code_page: u32) -> Option<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MultiByteToWideChar(
            cp: u32,
            flags: u32,
            bytes: *const u8,
            length: i32,
            out: *mut u16,
            capacity: i32,
        ) -> i32;
    }
    let length = i32::try_from(bytes.len()).ok()?;
    if length == 0 {
        return None;
    }
    unsafe {
        let count = MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            length,
            std::ptr::null_mut(),
            0,
        );
        if count <= 0 {
            return None;
        }
        let mut out = vec![0u16; count as usize];
        let written = MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            length,
            out.as_mut_ptr(),
            count,
        );
        (written > 0).then(|| String::from_utf16_lossy(&out[..written as usize]))
    }
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

/// `reg query` accepts only one `/v ValueName` per invocation. Querying the
/// key without `/v` returns the complete value set and avoids the invalid
/// syntax that would otherwise turn the OS/BIOS sections into fallback text.
pub(crate) fn registry_query_all(key: &str) -> String {
    format!("reg query \"{key}\"")
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

// ---- 卷挂载判定、设备路径规范化与盘符时间线 ----
//
// 以上三类逻辑已于 v2.9.9 抽离至 `volume_parse.rs`（本模块顶部 glob 再导出，
// 调用方的 `crate::text_parsing::X` 路径不变）：mountvol 查询三态判定与清单
// 解析、卷路径 ↔ 设备路径转换、卷标识符归一化比较、ESP 日志路径拼装、
// `MountedVolumes` 盘符复用表、`letters_in_mask` / `letter_delta_line` 时间线。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_text_returns_last_bytes_and_survives_edge_cases() {
        let dir = std::env::temp_dir().join(format!("br-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.log");
        // 长文件：只取尾部，且必须从 max_bytes 边界之后开始。
        let mut content = String::new();
        for i in 0..1000 {
            content.push_str(&format!("line {i:04} padding-padding-padding\n"));
        }
        std::fs::write(&path, &content).unwrap();
        let tail = tail_text(&path, 64);
        assert!(tail.len() <= 64 + 8, "尾部超限：{}", tail.len());
        assert!(tail.contains("line 0999"), "{tail}");
        assert!(!tail.contains("line 0000"), "不应包含开头：{tail}");
        // 不存在的文件 / 目录：返回空串而不是 panic。
        assert_eq!(tail_text(&dir.join("missing.log"), 64), "");
        assert_eq!(tail_text(&dir, 64), "");
        // max_bytes 大于文件长度：整文件返回。
        std::fs::write(&path, "short").unwrap();
        assert_eq!(tail_text(&path, 4096), "short");
        // 半个多字节字符（UTF-8 截断）：宽松解码成替换字符，不 panic。
        std::fs::write(&path, [0xE4u8, 0xB8]).unwrap();
        assert_eq!(tail_text(&path, 1), "\u{FFFD}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_of_text_keeps_tail_and_never_splits_a_char() {
        // 短于上限：原样返回。
        assert_eq!(tail_of_text("short", 64), "short");
        // 长文本：只留尾部，且包含最后一行。
        let long: String = (0..500).map(|i| format!("line {i:04}\n")).collect();
        let tail = tail_of_text(&long, 64);
        assert!(tail.len() <= 64, "尾部超限：{}", tail.len());
        assert!(tail.contains("line 0499"), "{tail}");
        assert!(!tail.contains("line 0000"), "不应包含开头：{tail}");
        // ★ 关键：截断点落在 3 字节汉字的中间时不许 panic，也不许留下半个字符。
        //   "错误" = E9 94 99 E8 AF AF，"错" 占 3 字节；取 4 字节 → 起点落在
        //   第二个字中间 → 必须整体跳过该字（结果只剩 1 个完整汉字）。
        let chinese = "错误信息";
        for max in 1..=chinese.len() {
            let t = tail_of_text(chinese, max);
            assert!(t.len() <= max, "max={max} 超限：{}", t.len());
            // 能通过 from_utf8 就说明没有半个字符（&str 本身合法，但起点必须落在边界上）。
            assert!(chinese.ends_with(t), "max={max} 破坏了尾部语义：{t}");
        }
        // "错误信息" 四个汉字各 3 字节：取 4 字节 → 起点 8 落在「信」中间 →
        // 挪到 9，只剩最后一个字「息」（3 字节 ≤ 4）。
        assert_eq!(tail_of_text(chinese, 4), "息");
    }

    #[test]
    fn compression_dropdown_has_only_two_supported_mappings() {
        assert_eq!(compression_from_ui_index(None).unwrap(), "fast");
        assert_eq!(compression_from_ui_index(Some(0)).unwrap(), "fast");
        assert_eq!(compression_from_ui_index(Some(1)).unwrap(), "none");
        assert!(compression_from_ui_index(Some(2)).is_err());
    }

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
    fn registry_query_all_uses_the_supported_single_command_form() {
        let command = registry_query_all(r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        assert_eq!(
            command,
            r#"reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion""#
        );
        assert!(!command.contains(" /v "));
    }

    #[test]
    fn console_bytes_are_sniffed_before_any_lossy_guess() {
        // GBK "版本" is not valid UTF-8: it must go to the OEM branch instead of
        // being replaced character by character.
        let gbk: &[u8] = &[0xB0, 0xE6, 0xB1, 0xBE];
        assert_eq!(plan_console_bytes(gbk), ConsoleBytes::Oem);
        // Genuine UTF-8 must never be re-decoded through GBK.
        assert_eq!(
            plan_console_bytes("处理器: ARM64".as_bytes()),
            ConsoleBytes::Utf8("处理器: ARM64".to_string())
        );
        // Pure ASCII is valid UTF-8 as well, so the common case stays lossless.
        assert_eq!(
            plan_console_bytes(b"MINWINPC\n"),
            ConsoleBytes::Utf8("MINWINPC\n".to_string())
        );
        // UTF-16LE without a BOM (bcdedit's usual shape).
        let utf16: Vec<u8> = "HKLM"
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        assert_eq!(plan_console_bytes(&utf16), ConsoleBytes::Utf16);
        assert_eq!(
            decode_bcdedit_bytes(&utf16),
            String::from_utf16_lossy(&[0x0048u16, 0x004b, 0x004c, 0x004d])
        );
    }

    #[test]
    fn edit_text_normalizes_every_line_ending_to_crlf() {
        assert_eq!(normalize_edit_newlines("a\nb\r\nc\rd"), "a\r\nb\r\nc\r\nd");
        // Already CRLF must not become CRCRLF.
        assert_eq!(normalize_edit_newlines("a\r\nb"), "a\r\nb");
        assert_eq!(normalize_edit_newlines(""), "");
        assert!(
            !normalize_edit_newlines(
                &["BackupRestore 软硬件信息", "【概览】", "计算机名：MINWINPC"].join("\n")
            )
            .contains("\n\u{0}")
        );
    }

    #[test]
    fn reg_key_values_drops_the_subkey_listing() {
        let listing = [
            r#"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion"#,
            r#"    SystemRoot    REG_SZ    X:\windows"#,
            r#"    ProductName    REG_SZ    Windows 11 Pro"#,
            "",
            r#"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Accessibility"#,
            r#"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion\AEDebug"#,
        ]
        .join("\n");
        let values = reg_key_values(&listing);
        assert_eq!(
            values,
            [
                "SystemRoot    REG_SZ    X:\\windows",
                "ProductName    REG_SZ    Windows 11 Pro"
            ]
            .join("\n")
        );
        assert!(!values.contains("Accessibility"));
        assert!(!values.contains("AEDebug"));
    }

    #[test]
    fn reg_key_values_keeps_error_text_visible() {
        let error = "ERROR: The system was unable to find the specified registry key.";
        assert_eq!(reg_key_values(error), error);
        assert_eq!(reg_key_values(""), "");
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

    /// `decode_windows_bytes`：UTF-8 字节直接无损通过，不被误判成 OEM。
    #[test]
    fn decode_windows_bytes_decodes_utf8_correctly() {
        let msg = "正在备份系统分区";
        assert_eq!(decode_windows_bytes(msg.as_bytes()), msg);
    }

}

/// PE 镜像启动配置的比对。
///
/// 这组逻辑必须留在这里（而不是 `native_gui.rs`）：后者只在 Windows 上编译，
/// 放进去就等于在开发机上零覆盖。
#[cfg(test)]
mod pe_shell {
    use super::*;

    /// 与真机 `T:\Windows\System32\winpeshl.ini` 一致的那 61 字节。
    const CORRECT_SHELL: &str =
        "[LaunchApps]\n%SYSTEMROOT%\\System32\\Recovery.exe,--pe-desktop\n";

    /// 2026-09-13 那份旧镜像里的 89 字节坏件（第一入口真能启动、第二入口文件不存在）。
    const OLD_BROKEN_SHELL: &str = "[LaunchApps]\n%SYSTEMROOT%\\System32\\RecoveryLauncher.cmd\n%SYSTEMROOT%\\System32\\recenv.exe\n";

    #[test]
    fn expected_shell_constant_is_the_61_byte_correct_piece() {
        assert_eq!(EXPECTED_PE_SHELL, CORRECT_SHELL);
        assert_eq!(EXPECTED_PE_SHELL.len(), 61);
        assert_eq!(OLD_BROKEN_SHELL.len(), 89);
    }

    #[test]
    fn correct_shell_with_recovery_exe_passes() {
        assert_eq!(
            judge_pe_shell(&PeShellFacts {
                shell: Some(CORRECT_SHELL),
                recovery_exe_present: true,
            }),
            None
        );
    }

    #[test]
    fn crlf_bom_and_trailing_spaces_still_pass() {
        let windows_style = format!("\u{feff}{}", CORRECT_SHELL.replace('\n', "\r\n"));
        assert_eq!(
            judge_pe_shell(&PeShellFacts {
                shell: Some(&windows_style),
                recovery_exe_present: true,
            }),
            None
        );
    }

    #[test]
    fn old_two_entry_shell_is_rejected_and_reports_real_size() {
        let problem = judge_pe_shell(&PeShellFacts {
            shell: Some(OLD_BROKEN_SHELL),
            recovery_exe_present: true,
        })
        .expect("89-byte shell must not pass");
        match problem {
            PeShellProblem::Mismatch { bytes, .. } => assert_eq!(bytes, 89),
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    /// 宽松判据的生死线：只要不是逐字等于就必须红，哪怕它「看起来对」。
    #[test]
    fn shell_with_an_extra_second_entry_is_rejected() {
        let extra = "[LaunchApps]\n%SYSTEMROOT%\\System32\\Recovery.exe,--pe-desktop\n%SYSTEMROOT%\\System32\\cmd.exe\n";
        assert!(matches!(
            judge_pe_shell(&PeShellFacts {
                shell: Some(extra),
                recovery_exe_present: true,
            }),
            Some(PeShellProblem::Mismatch { .. })
        ));
    }

    /// 参数写错（少了 `--pe-desktop`）同样必须红：那样 Recovery.exe 会以普通模式起来。
    #[test]
    fn shell_without_the_desktop_argument_is_rejected() {
        let no_flag = "[LaunchApps]\n%SYSTEMROOT%\\System32\\Recovery.exe\n";
        assert!(matches!(
            judge_pe_shell(&PeShellFacts {
                shell: Some(no_flag),
                recovery_exe_present: true,
            }),
            Some(PeShellProblem::Mismatch { .. })
        ));
    }

    #[test]
    fn missing_shell_and_missing_recovery_exe_are_distinct() {
        assert_eq!(
            judge_pe_shell(&PeShellFacts {
                shell: None,
                recovery_exe_present: true,
            }),
            Some(PeShellProblem::Missing)
        );
        assert_eq!(
            judge_pe_shell(&PeShellFacts {
                shell: Some(CORRECT_SHELL),
                recovery_exe_present: false,
            }),
            Some(PeShellProblem::RecoveryExeMissing)
        );
    }
}
