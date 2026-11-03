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

pub(crate) use crate::wim_parse::*;
pub(crate) use crate::bcd_parse::*;
pub(crate) use crate::step_progress::*;

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

/// Parse the `mountvol` listing (no arguments) into `(volume GUID, letters)`
/// pairs, in the order the listing reports them.
///
/// The listing is locale-independent where it matters: volume GUID lines are
/// always `\\?\Volume{...}\` and mount points are always `X:\` tokens, while
/// headers and the localized "no mount points" notice are ignored because they
/// match neither shape. A volume with several mount points lists them
/// space-separated on one line; folder mount points (no drive letter) are
/// skipped because roles only ever need a letter.
pub(crate) fn parse_mountvol_listing(output: &str) -> Vec<(String, Vec<char>)> {
    let mut result: Vec<(String, Vec<char>)> = Vec::new();
    let mut current: Option<(String, Vec<char>)> = None;
    for line in output.lines() {
        let trimmed = line.trim();
        let upper = trimmed.to_ascii_uppercase();
        if upper.starts_with("\\\\?\\VOLUME{") && trimmed.ends_with('\\') {
            if let Some(entry) = current.take() {
                result.push(entry);
            }
            current = Some((trimmed.to_string(), Vec::new()));
            continue;
        }
        if let Some((_, letters)) = current.as_mut() {
            for token in trimmed.split_whitespace() {
                let bytes = token.as_bytes();
                if bytes.len() == 3
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && bytes[2] == b'\\'
                {
                    let letter = (bytes[0] as char).to_ascii_uppercase();
                    if !letters.contains(&letter) {
                        letters.push(letter);
                    }
                }
            }
        }
    }
    if let Some(entry) = current.take() {
        result.push(entry);
    }
    result
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

    /// Every letter recorded so far, so candidate letters stay clear of the
    /// mounts earlier roles in this recovery session rely on.
    pub(crate) fn letters(&self) -> Vec<char> {
        self.by_guid.values().copied().collect()
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

// ---- ESP 路径与卷判定（宿主也编译，便于单测） ----

/// ESP 上本项目取证/日志文件所在的子目录（相对卷根）。
///
/// 2026-09-29 实测：ESP 根目录曾堆了 38 个 `.txt`/`.log`（约 93 KB，跨度 09-11~09-26）。
/// ESP 是引导分区、容量百 MB 级，根目录还混着 `EFI\`；固件每次枚举根目录都要扫过它们，
/// 所以诊断文件一律进子目录。
///
/// **不再带盘符**：旧值写死 `S:\BackupRestore\logs`，等于假设 `S:` 就是 ESP。
/// 该假设在 `S:` 被数据卷占用、或 ESP 已挂在别的盘符时成立不了，会把本项目的
/// 日志写到一个无关卷上。现在一律由调用方传入**已核验**的卷根
/// （`windows_prepare::esp_identity_without_drive_letter()` 的结果），见 [`esp_log_path_on`]。
pub(crate) const ESP_LOG_SUBDIR: &str = r"BackupRestore\logs";

/// 取证实录/日志在 ESP 上的路径。子目录不存在则顺带建好——调用方清一色是
/// `let _ = std::fs::write(...)`，目录缺失会静默失败，2026-09-29 已为这类静默失败
/// 付过好几轮排查代价。
/// 卷路径 → 设备路径（方案 A 必须的一步转换）。
///
/// 两种路径**不是一回事**，混用会在实机上报 `CreateFileW` 161/162：
/// - 卷路径 `\\?\Volume{GUID}\`：Win32 卷 API、`GetVolumeInformationW`、
///   普通文件读写（`std::fs`、`bcdedit /store`）吃的这个；
/// - 设备路径 `\\.\Volume{GUID}`：`CreateFileW` + `DeviceIoControl`
///   （`IOCTL_STORAGE_GET_DEVICE_NUMBER`、`IOCTL_DISK_GET_PARTITION_INFO_EX`）吃的这个。
///
/// 输入不是卷路径时**原样返回**：调用方传进来的可能已经是 `\\.\C:` 这种设备名，
/// 那时不需要转换。转换不出来（空 GUID）才算 `None`。
pub(crate) fn volume_path_to_device_path(path: &str) -> Option<String> {
    // 反斜杠一律用 ASCII 码拼：verbatim 路径里少一层就退化成普通路径，
    // 会指到别的卷上去，而这种错在日志里看不出来。
    const BACKSLASH: char = 92u8 as char;
    let trimmed = path.trim();
    let prefix = {
        let mut prefix = String::new();
        prefix.push(BACKSLASH);
        prefix.push(BACKSLASH);
        prefix.push('?');
        prefix.push(BACKSLASH);
        prefix.push_str("Volume");
        prefix
    };
    let Some(guid) = trimmed.strip_prefix(&prefix) else {
        return Some(path.to_string());
    };
    let guid = guid.trim_end_matches(BACKSLASH);
    if guid.is_empty() {
        return None;
    }
    let mut device = String::new();
    device.push(BACKSLASH);
    device.push(BACKSLASH);
    device.push('.');
    device.push(BACKSLASH);
    device.push_str("Volume");
    device.push_str(guid);
    Some(device)
}

/// 从卷设备路径里取出卷设备名（`HarddiskVolumeN`），**大小写无关**。
///
/// 【2026-10-19 实机缺陷修复】`winre_registration::field` 会把 BCD 字段值统一
/// 小写化，而 bcdedit/bcdboot 写进 BCD 的是 `\Device\HarddiskVolumeN` 大写形态。
/// 旧实现（`windows_prepare::partition_of_device_path`）只认大写前缀，于是 PE 内
/// recover 走到 WinRE 重接时，合法设备路径 `\device\harddiskvolume6` 被误判成
/// 「设备路径形态无法识别」→ 任务 failed。卷设备名对 NT 命名空间本就大小写
/// 无关，两种写法都必须放行。放这里是因为它必须能在非 Windows 构建下也跑单测。
pub(crate) fn device_path_volume_name(device: &str) -> Option<&str> {
    let trimmed = device.trim().trim_end_matches('\\');
    let lowered = trimmed.to_ascii_lowercase();
    // 前缀全是 ASCII，按字节切片的落点必然是字符边界。
    let offset = if lowered.starts_with(r"\device\") {
        r"\device\".len()
    } else if lowered.starts_with(r"\\.\") {
        r"\\.\".len()
    } else {
        return None;
    };
    Some(&trimmed[offset..])
}

/// verbatim 卷路径的前缀 `\\?\Volume`，用 ASCII 码拼出来。
///
/// 单独一个函数是为了让「路径里到底有几个反斜杠」这件事只有一个地方说了算。
/// 这一轮已经被这个坑了很多次：raw string 里的 `\` 有一个就是字面的一个，
/// 而普通字符串字面量里要写两个才对——混用就会拼出指错卷的路径。
pub(crate) fn esp_volume_prefix() -> String {
    const BACKSLASH: char = 92u8 as char;
    let mut prefix = String::new();
    prefix.push(BACKSLASH);
    prefix.push(BACKSLASH);
    prefix.push('?');
    prefix.push(BACKSLASH);
    prefix.push_str("Volume");
    prefix
}

/// 把命令串里的 `S:` 前缀换成 ESP 根（方案 A 的零盘符改造）。
///
/// 只替换**盘符后紧跟反斜杠或字符串结尾**的 `S:`，不动命令里别处的字母 S。
/// PE 里 `S:` 是本项目自己分配给 ESP 的（`X:` 是 RAM 盘），所以这个替换是安全的。
///
/// 放在本模块（而非 `native_gui.rs`）是为了让它在 macOS 的 `cargo test` 里也能跑——
/// 那个模块在 macOS 上根本不编译。2026-09-29 已经因为「测试放在 `#[cfg(windows)]`
/// 模块里」漏掉过一个 `ramdisk=` 值畸形的 bug，代价是整整一轮。
/// 把卷标识符归一成**裸 GUID**（`{xxxxxxxx-…}`）再比较。
///
/// 同一个卷在不同地方有不同写法，直接 `eq_ignore_ascii_case` 会把同一个卷判成两个：
/// - `mountvol X: /L` 给完整路径 `\\?\Volume{GUID}\`
/// - 任务 env / `VolumeIdentity.volume_guid` 存裸 GUID `{GUID}`
/// - `GetVolumeNameForVolumeMountPointW` 给的又是带尾反斜杠的完整路径
///
/// 2026-09-30 实机栽在这上面：restore-existing 的 EFI 挂载，盘符**挂成功了**
/// （`mountvol query Z: -> Mounted(\\?\Volume{…}\)`），但身份校验拿裸 GUID 比完整路径，
/// 报「volume identity mismatch」→ 换下一个盘符 → 全部候选试完 → 整个任务失败。
/// 日志看起来像"每个盘符都挂不上"，实际是"每次都挂上了但比输了"。
///
/// 返回 `None` 表示输入里找不到 GUID 形态，调用方必须**拒绝**而不是当成相等。
pub(crate) fn bare_volume_guid(value: &str) -> Option<&str> {
    let trimmed = value.trim().trim_end_matches('\\');
    // 完整路径形态：\\?\Volume{GUID}
    if let Some(rest) = trimmed.strip_prefix(r"\\?\Volume") {
        return Some(rest.trim());
    }
    // 裸 GUID 形态：{GUID}
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }
    None
}

/// 两个卷标识符是否指向同一个卷（自动归一化形态差异）。
pub(crate) fn same_volume(left: &str, right: &str) -> bool {
    match (bare_volume_guid(left), bare_volume_guid(right)) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        // 任一侧解析不出 GUID 就不能算相等——宁可误判为"不同"而重试，
        // 也不能把说不清的两个值当成同一个卷（那会写错启动项）。
        _ => false,
    }
}

/// 取证/日志文件在 ESP 上的完整路径，基于**已核验的卷根**。
///
/// `volume_root` 可以是 verbatim 卷路径（`\\?\Volume{GUID}\`）或带盘符的根
/// （`S:\`）；两种都必须是调用方已经确认指向该 ESP 的值，本函数不做猜测、
/// 也不替换盘符字符串。缺尾部反斜杠时补齐。
///
/// 子目录不存在则顺带建好——调用方清一色是 `let _ = ...write(...)`，
/// 目录缺失会静默失败，2026-09-29 已为这类静默失败付过好几轮排查代价。
pub(crate) fn esp_log_path_on(volume_root: &str, name: &str) -> String {
    let mut root = volume_root.to_string();
    if !root.ends_with('\\') {
        root.push('\\');
    }
    let dir = format!("{root}{ESP_LOG_SUBDIR}");
    let _ = std::fs::create_dir_all(&dir);
    format!("{dir}\\{name}")
}

/// `mountvol X: /L` 的输出是否说明该盘符上**有**卷。
///
/// 这是「ESP 挂载成功」的唯一可靠判据。2026-09-29 在提权会话里实测
/// （`.test-artifacts/elev-channel/mvwhy.txt`）：`mountvol X: /S` 的退出码会骗人——
/// 成功时可能返回 1，重复挂已挂载的卷时又返回 0。所以挂载后只能用 `/L` 确认。
///
/// 空输出（只有空白行）＝没挂上；有一行卷路径（`\\?\Volume{...}\`）＝挂上了。
pub(crate) fn mountvol_listing_has_volume(listing: &str) -> bool {
    listing.lines().map(str::trim).any(|line| !line.is_empty())
}

// ---------- 盘符时间线（纯逻辑；Windows 侧由 `letter_trace` 调用）----------
//
// 2026-10-29 用户裁决「盘符这件事要观测，把事实钉死」之后的落点：判断逻辑
// 全在这里，`letter_trace.rs` 只负责采样和写文件——这样单测在本机就能跑，
// 不必为了验一段字符串格式去开虚拟机。

/// `GetLogicalDrives()` 的位图 → 已挂载的盘符，只取 `C..=Z`。
///
/// 位 0 是 A、位 1 是 B：A/B 是软驱位，现代机器上没有，出现也只会是残留映射，
/// 与「恢复卷被谁挂了盘符」这个问题无关，所以直接不纳入时间线。
pub(crate) fn letters_in_mask(mask: u32) -> Vec<char> {
    ('C'..='Z')
        .filter(|letter| mask & (1u32 << (u32::from(*letter as u8) - u32::from(b'A'))) != 0)
        .collect()
}

/// 两次采样之间的盘符变化；**没变就返回 `None`**（调用方据此不写日志）。
///
/// 输出只含 ASCII：`t+1200ms +R now=CDERST` / `t+900ms -R now=CDEST` /
/// `t+1500ms +V -T now=CDEVS`。进 `prlctl` 的串一律 ASCII 是本项目铁律，
/// 这里的输出会被整段 cat 回来排障。
pub(crate) fn letter_delta_line(elapsed_ms: u64, before: &[char], after: &[char]) -> Option<String> {
    let added: String = after
        .iter()
        .filter(|letter| !before.contains(letter))
        .copied()
        .collect();
    let removed: String = before
        .iter()
        .filter(|letter| !after.contains(letter))
        .copied()
        .collect();
    if added.is_empty() && removed.is_empty() {
        return None;
    }
    let mut line = format!("t+{elapsed_ms}ms");
    if !added.is_empty() {
        line.push_str(&format!(" +{added}"));
    }
    if !removed.is_empty() {
        line.push_str(&format!(" -{removed}"));
    }
    line.push_str(&format!(
        " now={}",
        after.iter().collect::<String>()
    ));
    Some(line)
}

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
    fn mountvol_listing_parses_guids_and_letters_across_locales() {
        let listing = "\
Possible values for VolumeName along with current mount points are:

    \\\\?\\Volume{11111111-2222-3333-4444-555555555555}\\
        C:\\

    \\\\?\\Volume{aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}\\
        *** NO MOUNT POINTS ***

    \\\\?\\Volume{99999999-8888-7777-6666-555555555555}\\
        D:\\ E:\\

    \\\\?\\Volume{dddddddd-cccc-bbbb-aaaa-999999999999}\\
        \\\\?\\C:\\mount\\folder
";
        let parsed = parse_mountvol_listing(listing);
        assert_eq!(parsed.len(), 4);
        assert_eq!(
            parsed[0],
            (
                "\\\\?\\Volume{11111111-2222-3333-4444-555555555555}\\".to_string(),
                vec!['C']
            )
        );
        assert_eq!(
            parsed[1],
            (
                "\\\\?\\Volume{aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}\\".to_string(),
                Vec::<char>::new()
            )
        );
        // Multiple mount points on one line, letter-only, de-duplicated.
        assert_eq!(
            parsed[2],
            (
                "\\\\?\\Volume{99999999-8888-7777-6666-555555555555}\\".to_string(),
                vec!['D', 'E']
            )
        );
        // Folder mount points carry no drive letter and are skipped.
        assert_eq!(
            parsed[3],
            (
                "\\\\?\\Volume{dddddddd-cccc-bbbb-aaaa-999999999999}\\".to_string(),
                Vec::<char>::new()
            )
        );
    }

    #[test]
    fn mountvol_listing_tolerates_localized_headers_and_garbage() {
        // Chinese locale output: header lines and notices are localized, but
        // GUID lines and `X:\` tokens stay ASCII (mountvol writes them raw).
        let listing = "\
列出可用于 VolumeName 的可能值以及当前装入点:

    \\\\?\\Volume{761230e8-107c-4396-8c37-82273720183d}\\
        C:\\

    \\\\?\\Volume{f0753766-30a4-410e-944f-38d139113634}\\
        *** 没有装入点 ***
";
        let parsed = parse_mountvol_listing(listing);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].1, vec!['C']);
        assert!(parsed[1].1.is_empty());
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

    /// 卷路径 → 设备路径的转换（方案 A 的实机坑）。
    ///
    /// 2026-09-29 实机报 `CreateFileW failed while reading volume identity (Windows error 161)`
    /// (=ERROR_BAD_PATHNAME)：拿 `\\?\Volume{GUID}\` 去 `CreateFileW` + `DeviceIoControl`
    /// 是被拒的，卷 API 吃的路径和设备 API 吃的路径不是一回事。
    #[test]
    fn volume_path_to_device_path_shape() {
        let bs = 92u8 as char;
        let mut volume = String::new();
        volume.push(bs);
        volume.push(bs);
        volume.push('?');
        volume.push(bs);
        volume.push_str("Volume{d08d796f-f082-4402-bdbb-a4a6a09ac53f}");
        volume.push(bs);

        assert_eq!(
            volume_path_to_device_path(&volume).as_deref(),
            Some(r"\\.\Volume{d08d796f-f082-4402-bdbb-a4a6a09ac53f}")
        );
        // 尾部多余的反斜杠要去掉，否则设备路径多个尾分隔符。
        assert_eq!(
            volume_path_to_device_path(&format!("{volume}{bs}")).as_deref(),
            Some(r"\\.\Volume{d08d796f-f082-4402-bdbb-a4a6a09ac53f}")
        );
    }

    /// 非卷路径原样返回（盘符形态的设备路径已经是 `\\.\C:`）。
    #[test]
    fn volume_path_to_device_path_passes_through_device_names() {
        assert_eq!(
            volume_path_to_device_path(r"\\.\C:").as_deref(),
            Some(r"\\.\C:")
        );
        assert_eq!(
            volume_path_to_device_path(r"\\.\PhysicalDrive0").as_deref(),
            Some(r"\\.\PhysicalDrive0")
        );
    }

    /// 【2026-10-19 实机缺陷修复】BCD 字段值在本 crate 内统一小写化，而
    /// bcdedit/bcdboot 写出的是大写 `\Device\HarddiskVolumeN`——两种写法必须
    /// 解析出卷设备名。旧实现只认大写前缀，PE 内 recover 的 WinRE 重接因此
    /// 把合法设备路径误判成「形态无法识别」，任务 failed。
    #[test]
    fn device_path_volume_name_accepts_both_cases() {
        assert_eq!(
            device_path_volume_name(r"\Device\HarddiskVolume6"),
            Some("HarddiskVolume6")
        );
        assert_eq!(
            device_path_volume_name(r"\device\harddiskvolume6"),
            Some("harddiskvolume6")
        );
        assert_eq!(
            device_path_volume_name(r"\DEVICE\HARDDISKVOLUME6"),
            Some("HARDDISKVOLUME6")
        );
        assert_eq!(
            device_path_volume_name(r"\\.\HarddiskVolume6"),
            Some("HarddiskVolume6")
        );
        assert_eq!(
            device_path_volume_name(r"\\.\harddiskvolume6"),
            Some("harddiskvolume6")
        );
        // 空白与尾部反斜杠都要容得下（BCD 文本里的常见形态）。
        assert_eq!(
            device_path_volume_name("  \\Device\\HarddiskVolume6\\  "),
            Some("HarddiskVolume6")
        );
        for rejected in [
            "",
            "C:",
            "partition=C:",
            r"\Device",
            r"\DeviceHarddiskVolume6",
            r"\\?\GLOBALROOT\device\harddisk0\partition6\recovery\windowsre",
        ] {
            assert_eq!(device_path_volume_name(rejected), None, "{rejected}");
        }
    }
    /// 方案 A 的第一层筛选：**只保留未挂载的卷**。
    ///
    /// 这不是洁癖：ESP 通常隐藏，而开发用的 ESP 往往已经挂了盘符；挑已挂载的那个，
    /// 等于把用户正在看的盘符当成目标。真正的 ESP 判定（分区类型 GUID + BCD 文件）
    /// 在 `esp_identity_without_drive_letter`，这里只管"没挂载"这一条。
    /// 方案 A 的命令串改写：只换 ESP 盘符，别处出现字母 S 一律不动。
    ///
    /// 曾踩过的坑：`H:\pe-wim1.wim` 这种路径里也带字母 S，无脑替换会把它也改掉，
    /// 然后 DISM 就去读一个不存在的路径。另一个坑是只换 `S` 不换 `:`，会留下
    /// 一个孤零零的冒号（`卷根\:`），路径立刻失效。
    /// 卷标识符归一化：同一个卷的三种写法必须判为相等。
    ///
    /// 2026-09-30 实机事故的直接复现：restore-existing 的 EFI 挂载盘符挂成功了，
    /// 但校验拿 `{GUID}` 比 `\\?\Volume{GUID}\` 判为不同，于是换盘符重试，
    /// 把每个候选盘符都试一遍后整个任务失败。
    #[test]
    fn same_volume_treats_all_spellings_as_equal() {
        let bare = "{d08d796f-f082-4402-bdbb-a4a6a09ac53f}";
        let full = r"\\?\Volume{d08d796f-f082-4402-bdbb-a4a6a09ac53f}\";
        assert!(same_volume(bare, full));
        assert!(same_volume(full, bare));
        assert!(same_volume(bare, bare));
        // 大小写不同仍是同一个卷
        assert!(same_volume(
            "{D08D796F-F082-4402-BDBB-A4A6A09AC53F}",
            r"\\?\Volume{d08d796f-f082-4402-bdbb-a4a6a09ac53f}\"
        ));
    }

    /// 说不清的输入必须判为"不同"，不能当成相等。
    #[test]
    fn same_volume_refuses_to_guess() {
        assert!(!same_volume(
            "",
            r"\\?\Volume{d08d796f-f082-4402-bdbb-a4a6a09ac53f}\"
        ));
        assert!(!same_volume("C:", "{d08d796f-f082-4402-bdbb-a4a6a09ac53f}"));
        assert!(!same_volume("garbage", "garbage"));
        // 真的不同卷
        assert!(!same_volume(
            "{11111111-2222-3333-4444-555555555555}",
            r"\\?\Volume{d08d796f-f082-4402-bdbb-a4a6a09ac53f}\"
        ));
    }

    /// `bare_volume_guid` 要从三种写法里都取出 GUID。
    #[test]
    fn bare_volume_guid_extracts_from_every_spelling() {
        let guid = "{d08d796f-f082-4402-bdbb-a4a6a09ac53f}";
        assert_eq!(bare_volume_guid(guid), Some(guid));
        assert_eq!(
            bare_volume_guid(r"\\?\Volume{d08d796f-f082-4402-bdbb-a4a6a09ac53f}\"),
            Some(guid)
        );
        assert_eq!(bare_volume_guid(&format!("  {guid}  ")), Some(guid));
        assert_eq!(bare_volume_guid("C:"), None);
        assert_eq!(bare_volume_guid(""), None);
    }

    /// 步骤清单与总进度：PE 进度窗口靠它回答「一共几步 / 现在第几步 / 当前百分之几」。
    ///
    /// 2026-09-30 用户反馈 PE 里只显示一句「正在准备恢复环境…」，没有步骤也没有
    /// 百分比。这三个函数就是为此加的，逻辑放在 macOS 也编译的 text_parsing，
    /// 免得再犯「测试放在 #[cfg(windows)] 模块里所以一次都没跑」的老毛病。
    /// UTF-8 解码直接无损通过。
    #[test]
    fn decode_windows_bytes_decodes_utf8_correctly() {
        let msg = "正在备份系统分区";
        assert_eq!(decode_windows_bytes(msg.as_bytes()), msg);
    }

    #[test]
    fn mountvol_listing_reports_volume_presence_from_l_not_s_exit_code() {
        // 实机抓到的形态：`/L` 输出一行卷路径；没挂上时输出空或只有 CRLF。
        let real = format!("\\?\\Volume{}", "{d08d796f-f082-4402-bdbb-a4a6a09ac53f}\\");
        assert!(mountvol_listing_has_volume(&real));
        assert!(!mountvol_listing_has_volume(""));
        assert!(!mountvol_listing_has_volume("\r\n"));
        // 前后空白与 CRLF 都不该影响判断
        assert!(mountvol_listing_has_volume(&format!("  \r\n{real}  \r\n")));
        // 只有空白字符也等于没有卷
        assert!(!mountvol_listing_has_volume("   \t  "));
    }

    /// 锁住「日志进子目录、且路径只来自调用方给的已核验卷根」这条约定。
    ///
    /// 旧实现写死 `S:\BackupRestore\logs`（并为已废弃的 `pe-task.txt` 控制通道
    /// 留了 `S:\` 根的特例）。那等于假设 `S:` 就是 ESP——`S:` 被数据卷占用或 ESP
    /// 挂在别的盘符时，日志会落到无关卷上。现在盘符形态与 verbatim 卷路径都接受，
    /// 但必须由调用方传入，本函数不猜、也不做盘符字符串替换。
    /// ESP 是引导分区，根目录堆日志会拖慢固件枚举（2026-09-29 实测根目录曾堆 38 个
    /// txt/log，约 93 KB），所以一律进子目录。
    #[test]
    fn esp_log_path_is_built_from_the_caller_supplied_volume_root() {
        let verbatim = r"\\?\Volume{d08d796f-f082-4402-bdbb-a4a6a09ac53f}\";
        for log in ["diag1.txt", "bcd-all.txt", "exit-pe.log"] {
            // verbatim 卷根：原样前缀，不出现任何盘符。
            let p = esp_log_path_on(verbatim, log);
            assert_eq!(p, format!(r"{verbatim}BackupRestore\logs\{log}"));
            assert!(!p.contains("S:"), "{p} 不得引入盘符假设");
            // 盘符形态：也只用调用方给的那个盘符。
            assert_eq!(
                esp_log_path_on(r"Q:\", log),
                format!(r"Q:\BackupRestore\logs\{log}")
            );
            // 缺尾部反斜杠时补齐，不会拼出 `Q:BackupRestore`。
            assert_eq!(esp_log_path_on("Q:", log), esp_log_path_on(r"Q:\", log));
        }
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

/// 盘符时间线的纯逻辑：位图解析 + 变化描述。
///
/// 为什么单独一个 mod：这两条判据决定「时间线里到底记了什么」，写错了
/// 整份证据就是废纸（少记一个盘符 → 结论「没泄漏」；多记 → 追一个不存在的幽灵）。
#[cfg(test)]
mod drive_letter_timeline {
    use super::*;

    /// 位图 → 盘符：只认 C..Z，A/B 不进时间线。
    #[test]
    fn bitmask_maps_to_letters_and_skips_floppy_slots() {
        // A + C + D + Z
        let mask = 0b1 | (1 << 2) | (1 << 3) | (1 << 25);
        assert_eq!(letters_in_mask(mask), vec!['C', 'D', 'Z']);
        assert!(letters_in_mask(0b11).is_empty());
        assert_eq!(letters_in_mask(0), Vec::<char>::new());
    }

    /// 没变化必须返回 None——否则采样器每 200 ms 写一行，日志被灌满，
    /// 真正的那一次变化反而淹在里面。
    #[test]
    fn identical_snapshots_produce_no_line() {
        assert_eq!(letter_delta_line(200, &['C', 'D'], &['C', 'D']), None);
        assert_eq!(letter_delta_line(400, &[], &[]), None);
    }

    /// 新增、消失、同时发生三种情形，格式固定且全 ASCII。
    #[test]
    fn delta_line_reports_added_and_removed_letters() {
        assert_eq!(
            letter_delta_line(1200, &['C', 'D'], &['C', 'D', 'R']).as_deref(),
            Some("t+1200ms +R now=CDR")
        );
        assert_eq!(
            letter_delta_line(900, &['C', 'D', 'T'], &['C', 'D']).as_deref(),
            Some("t+900ms -T now=CD")
        );
        assert_eq!(
            letter_delta_line(1500, &['C', 'T'], &['C', 'V']).as_deref(),
            Some("t+1500ms +V -T now=CV")
        );
    }

    /// 判据只认 ASCII：这条日志要整段 `type` 回来看，混进一个非 ASCII 就会
    /// 在 `prlctl exec` 里变成乱码（项目里已踩过 iconv「Illegal byte sequence」）。
    #[test]
    fn timeline_lines_stay_ascii() {
        let line = letter_delta_line(7, &['C'], &['C', 'R', 'V']).unwrap();
        assert!(line.is_ascii(), "non-ascii timeline line: {line}");
    }
}
