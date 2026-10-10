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
    if value.get("ImageIndex").is_some()
        || value.get("imageIndex").is_some()
        || value.get("index").is_some()
    {
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
        // 兼容 ImageIndex / imageIndex / index
        let index_str = {
            let s = json_text(&item, "ImageIndex");
            if !s.is_empty() {
                s
            } else {
                let s2 = json_text(&item, "imageIndex");
                if !s2.is_empty() {
                    s2
                } else {
                    json_text(&item, "index")
                }
            }
        };
        let index = index_str
            .parse::<u32>()
            .map_err(|_| "WIM metadata contains an invalid image index".to_string())?;
        if index == 0 {
            return Err("WIM metadata contains image index 0".to_string());
        }
        // 兼容 ImageSize / imageSize
        let size_bytes = json_text(&item, "ImageSize")
            .parse::<u64>()
            .or_else(|_| json_text(&item, "imageSize").parse::<u64>())
            .ok();
        // 兼容 ImageName / name
        let name = {
            let n = json_text(&item, "ImageName");
            if !n.is_empty() {
                n
            } else {
                json_text(&item, "name")
            }
        };
        images.push(WimImageInfo {
            index,
            name,
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

/// Does this byte string look like UTF-16LE text?
///
/// bcdedit writes UTF-16LE on Chinese/Japanese systems, sometimes without a
/// BOM, so the interleaved-NUL pattern is detected as well as the BOM.
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

/// Decode a bcdedit output file's bytes.
pub(crate) fn decode_bcdedit_bytes(bytes: &[u8]) -> String {
    if looks_like_utf16le(bytes) {
        utf16le_to_string(bytes)
    } else {
        String::from_utf8_lossy(bytes).to_string()
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

/// Win32 multiline `EDIT` controls only break lines on `\r\n`. A lone `\n` is
/// stored as an invisible control character, which is how the information
/// window's report header collapsed into a single unreadable row.
pub(crate) fn normalize_edit_newlines(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

/// Whether a `bcdedit /enum {bootmgr}` listing still contains `bootsequence`.
///
/// The delete command returns non-zero when the value was already absent, so
/// its exit code alone cannot distinguish a harmless no-op from a failed
/// cleanup. Re-enumerating the entry and checking the field is locale-neutral
/// for the two project-supported images: English prints `bootsequence`, while
/// zh-CN prints `启动序列`.
pub(crate) fn bcd_output_has_bootsequence(text: &str) -> bool {
    text.lines().any(|line| {
        let key = line.trim_start();
        let lower = key.to_ascii_lowercase();
        lower.starts_with("bootsequence") || key.starts_with("启动序列")
    })
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
/// 编号步骤与总进度。
///
/// PE 里的恢复流程是 `STEP n/N <名称>` 形式的编号步骤（见 `main.rs` 的
/// `append_log(log, "STEP 1/4 …")`）。进度窗口要回答三个问题——
/// **一共几步、现在第几步、当前这一步到百分之几**——所以把这三件事算清楚。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct StepProgress {
    /// 已出现的步骤，按编号升序、去重：`(编号, 名称, 时分秒时间戳)`。
    pub steps: Vec<(u32, String, Option<String>)>,
    /// 总步骤数（`N`），从任一步骤的 `n/N` 里取；取不到就是 `None`。
    pub total: Option<u32>,
    /// 当前步骤号 = 已出现的最大编号。
    pub current: Option<u32>,
    /// 当前步骤内部的百分比（DISM 那种 0–100）。
    pub current_percent: Option<u32>,
    /// 总进度百分比：已完成整步 + 当前步骤的部分进度。
    pub overall_percent: Option<u32>,
}

/// 从日志行行首提取时分秒时间戳 `HH:MM:SS`。
pub(crate) fn extract_timestamp_hms(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('[')
        && let Some(end) = trimmed.find(']')
    {
        let inside = &trimmed[1..end];
        // 尝试解析 RFC3339 / ISO-8601 形如 "2026-09-30T08:18:08.054361500+00:00"
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(inside) {
            let local: chrono::DateTime<chrono::Local> = dt.into();
            return Some(local.format("%H:%M:%S").to_string());
        }
        // 尝试匹配尾部的 HH:MM:SS（例如 "2026-09-30 16:18:08" 或 "16:18:08"）
        if inside.len() >= 8 {
            let tail = &inside[inside.len() - 8..];
            if tail.chars().filter(|c| *c == ':').count() == 2 {
                return Some(tail.to_string());
            }
        }
    }
    None
}

/// 从一段日志文本里算出步骤进度。
///
/// `log` 是**累积**的日志（不是增量），因为步骤清单要把前面已完成的步骤也列出来；
/// 调用方每次刷新都从当前 offset 起读，所以这里只处理新读到的那段——
/// 真正的累积由 `StepTracker` 负责（见 `text_parsing::StepTracker`）。
pub(crate) fn step_progress_of_new_lines(lines: &[&str]) -> StepProgress {
    let mut progress = StepProgress::default();
    for line in lines {
        let Some((index, total, name)) = parse_step_marker(line) else {
            continue;
        };
        progress.total = Some(total);
        if progress.steps.iter().any(|(seen, _, _)| *seen == index) {
            // 同一步骤可能被重打（续跑、重试），保留第一次的名字即可。
            continue;
        }
        let ts = extract_timestamp_hms(line);
        progress.steps.push((index, name, ts));
    }
    progress.steps.sort_by_key(|(index, _, _)| *index);
    progress.current = progress.steps.last().map(|(index, _, _)| *index);
    progress
}

/// 带累积的步骤跟踪器：把总进度一起算出来。
///
/// 单独一个结构体是因为进度窗口每次只读到日志的**增量**，而"一共几步"和
/// "已完成几步"必须跨增量累积——不然窗口会在两次刷新之间忘记自己走到哪。
#[derive(Debug, Default, Clone)]
pub(crate) struct StepTracker {
    inner: StepProgress,
}

impl StepTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一批新读到的日志行，返回累加后的进度快照。
    pub fn feed(&mut self, lines: &[&str]) -> StepProgress {
        let fresh = step_progress_of_new_lines(lines);
        if let Some(total) = fresh.total {
            self.inner.total = Some(total);
        }
        for (index, name, ts) in fresh.steps {
            if let Some(existing) = self
                .inner
                .steps
                .iter_mut()
                .find(|(seen, _, _)| *seen == index)
            {
                if existing.2.is_none() && ts.is_some() {
                    existing.2 = ts;
                }
            } else {
                let timestamp =
                    ts.or_else(|| Some(chrono::Local::now().format("%H:%M:%S").to_string()));
                self.inner.steps.push((index, name, timestamp));
            }
        }
        self.inner.steps.sort_by_key(|(index, _, _)| *index);
        let previous_current = self.inner.current;
        self.inner.current = self.inner.steps.last().map(|(index, _, _)| *index);
        if self.inner.current != previous_current {
            self.inner.current_percent = fresh.current_percent;
        } else {
            self.inner.current_percent = fresh.current_percent.or(self.inner.current_percent);
        }
        self.inner.overall_percent = self.overall();
        self.inner.clone()
    }

    /// 记录当前步骤内部的百分比（来自 DISM 进度行）。
    pub fn set_current_percent(&mut self, percent: Option<u32>) {
        self.inner.current_percent = percent;
        self.inner.overall_percent = self.overall();
    }

    pub fn snapshot(&self) -> StepProgress {
        self.inner.clone()
    }

    /// 总进度 = (已完成整步数 + 当前步骤的部分) / 总步数。
    fn overall(&self) -> Option<u32> {
        let total = self.inner.total?;
        let current = self.inner.current?;
        if total == 0 {
            return None;
        }
        let done_steps = current.saturating_sub(1) as f64;
        let inside = self.inner.current_percent.unwrap_or(0).min(100) as f64 / 100.0;
        let overall = ((done_steps + inside) / total as f64) * 100.0;
        Some(overall.round().clamp(0.0, 100.0) as u32)
    }
}

// 从任意一行日志里取出编号步骤标记 `(编号, 总数, 名称)`；取不到返回 `None`。
//
// 日志行的真实形态是 `[2026-09-29T03:36:32.259862400+00:00] STEP 1/4 准备备份环境…`
// —— `STEP` 前面有方括号时间戳。所以**不能**用 `strip_prefix("STEP ")` 只匹配行首：
// 2026-09-30 之前 `classify_log_line` 和进度窗口都是那么写的，结果编号步骤
// 从来没能被识别，PE 里显示的一直是那句不动的「正在准备恢复环境…」。
//
// 放在这里让「识别编号步骤」只有一份实现：进度窗口、阶段标题、单测共用。
pub(crate) fn parse_step_marker(line: &str) -> Option<(u32, u32, String)> {
    let start = line.find("STEP ")?;
    let rest = line[start + 5..].trim();
    let (token, name) = rest.split_once(' ')?;
    if !token.contains('/') {
        return None;
    }
    let mut parts = token.splitn(2, '/');
    let (Some(index), Some(total)) = (parts.next(), parts.next()) else {
        return None;
    };
    let (Ok(index), Ok(total)) = (index.trim().parse::<u32>(), total.trim().parse::<u32>()) else {
        return None;
    };
    if index == 0 || total == 0 || index > total {
        return None;
    }
    let mut clean_name = name.trim().to_string();
    // 移除括号说明废话（如“（DISM，百分比见进度条）”）
    if let Some(idx) = clean_name.find("（DISM") {
        clean_name = clean_name[..idx].trim().to_string();
    } else if let Some(idx) = clean_name.find("(DISM") {
        clean_name = clean_name[..idx].trim().to_string();
    }
    Some((index, total, clean_name))
}

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
    } else if line.contains("The operation completed successfully") {
        // DISM 的一条命令成功，不代表哈希、副档和清理已完成。
        stage = Some("当前命令完成，等待后续校验".to_string());
    } else if line.contains("Boot entry cleaned; task marked successful")
        || line.contains("操作完成：命令、回读和副档簿记均已通过")
    {
        stage = Some("操作完成".to_string());
    } else if line.contains("Recovery completed") {
        stage = Some("恢复已完成，正在清理".to_string());
    } else if line.contains("Recovery.exe started") || line.contains("started from env") {
        stage = Some("正在准备恢复环境…".to_string());
    } else if let Some((index, total, name)) = parse_step_marker(line) {
        // 备份/还原流程打的编号步骤标记，形如 "STEP 2/4 捕获系统分区镜像"。
        // 进度窗口据此显示「n/N <名称>」，让用户看清当前处在第几步。
        stage = Some(format!("{index}/{total} {name}"));
    }
    let percent = parse_percent(line);
    let trimmed = line.trim();
    let detail = if trimmed.is_empty()
        || ((line.starts_with("[stdout] [") || line.starts_with('[')) && line.contains('%'))
        || line.starts_with("STEP ")
        || trimmed == "[stdout]"
        || trimmed == "[stderr]"
    {
        None // 空行、纯进度行、编号步骤行、纯标签空行都不占详情（步骤已由阶段标题展示）
    } else {
        Some(line.trim_end().to_string())
    };
    (stage, percent, detail)
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

// ---- BCD GUID / ramdisk 设备串解析（v1.7.11 新启动通道；宿主也编译，便于单测） ----

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

/// `device`/`osdevice` 的值形态（2026-09-29 用 ProcMon 抓到真实命令行后校准）：
///
/// ```text
/// ramdisk=[F:]\BackupRestoreRE\Winre.wim,{设备选项对象}
/// ```
///
/// **方括号里只包「卷」，`]` 紧跟在卷后面闭合**，随后才是卷内绝对路径，最后逗号接设备
/// 选项对象。这一点此前连错两轮：先写成 `[整条路径],{…}`（`]` 在逗号前），bcdedit 报
/// 「按规定设备无效」；然后误判成「没有配对 `]`」，改成 `[整条路径,{…}`，于是稳定报
/// 「指定的设备无效」。真正的形态以 `bcdedit /enum` 回显为准（见 `bcd-final.txt`：
/// `device ramdisk=[C:]\Recovery\WindowsRE\Winre.wim,{…}`）。
///
/// 曾经因为这个畸形串，整整一轮把失败归因成「产品进程上下文导致 bcdedit 拒绝 ramdisk」，
/// 并为此加了子进程代跑、句柄不继承、wmic 等一堆绕路——全是白做的。教训写在
/// `docs/20260929-193000-...` 里：**先抓真实 argv，再谈进程上下文**。
pub(crate) fn ramdisk_spec(wim_path: &str, devopts: &str) -> String {
    let (volume, inside) = split_volume_and_path(wim_path);
    format!("ramdisk=[{volume}]{inside},{devopts}")
}

/// 把 `F:\BackupRestoreRE\Winre.wim` 拆成 `("F:", "\\BackupRestoreRE\\Winre.wim")`。
///
/// 没有盘符前缀时退回 `("", 原串)`——调用方给的是卷内相对路径，补上开头反斜杠即可，
/// 绝不自作主张猜一个盘符（猜错就是往别的卷上指启动项）。
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
///
/// 必须从 `device`/`osdevice` 行里取：该输出第一个 GUID 是 **osloader 自己**，
/// 直接「取第一个 GUID」会把它当成设备选项对象去 `/copy`，随后
/// `bcdedit /set ramdisksdidevice` 报「指定的元素无法识别」（2026-09-29 实机踩坑）。
pub(crate) fn ramdisk_device_options_guid(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if (trimmed.starts_with("device ") || trimmed.starts_with("osdevice "))
            && let Some(guid) = parse_guid(trimmed)
        {
            return Some(guid);
        }
    }
    None
}

/// 严格 GUID 形态校验（带花括号的 8-4-4-4-12）。空值/畸形一律 `Err`，
/// 这是「绝不误伤 `{default}`」这条硬约束的守门人：bcdedit 在标识符参数为空时会
/// 静默作用于 `{default}`（真实事故：曾把 Windows 11 启动项 device 改成 ramdisk）。
/// 载荷 RE 的 `ramdisksdipath`：**必须以 `\` 开头**（绝对路径）。
/// 少了开头反斜杠时 bcdedit 在接受 `device ramdisk=…` 这一步报「指定的设备无效」
/// （2026-09-29 实机踩坑：少了 `\` 整整卡了一轮）。
pub(crate) fn staging_sdi_path(staging_dir: &str) -> String {
    let dir = staging_dir.trim_start_matches('\\');
    format!("\\{dir}\\boot.sdi")
}

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

/// `bcdedit` 接受 `{bootmgr}` 这类**知名别名**（well-known identifier），它们不是
/// 8-4-4-4-12 形态，过不了 [`require_guid`]。
///
/// 但**绝不**把 `require_guid` 整个放宽成接受任意别名：那条守门人是为「标识符参数为空时
/// bcdedit 静默作用于 `{default}`」立的（真实事故：曾把 Windows 11 启动项 device 改成
/// ramdisk）。放任别名进去，`{default}` 就从同一个口子混进来了。所以这里只认白名单里的
/// 一个，且白名单里**故意不放 `{default}`/`{current}`/`{ntldr}` 这些指向用户启动项的东西**。
/// ESP 上本项目**取证/日志**文件的目录。
///
/// 2026-09-29 实测：`S:\` 根目录堆了 38 个 `.txt`/`.log`（约 93 KB，跨度 09-11~09-26）。
/// ESP 是引导分区、容量百 MB 级，根目录还混着 `EFI\`；固件每次枚举根目录都要扫过它们。
/// 这些文件**没有一个是控制通道**：PE 启动时真正需要固定路径的只有
/// `pe-task.txt`/`.done`/`pe-task-result.txt`（见 [`ESP_CONTROL_FILES`]）。
pub(crate) const ESP_LOG_DIR: &str = "S:\\BackupRestore\\logs";

/// 必须留在 `S:\` 根的控制通道文件名（PE 启动最早期按固定路径读取）。
pub(crate) const ESP_CONTROL_FILES: [&str; 3] =
    ["pe-task.txt", "pe-task.txt.done", "pe-task-result.txt"];

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

#[cfg(test)]
pub(crate) fn rewrite_s_root(command: &str, esp_root: &str) -> String {
    // 退回盘符形态时不需要任何替换。
    if esp_root == r"S:\" {
        return command.to_string();
    }
    let bytes = command.as_bytes();
    let mut out = String::with_capacity(command.len() + esp_root.len());
    let mut index = 0_usize;
    while index < bytes.len() {
        // 只看「S 后面紧跟冒号」这一种形态，且前后都必须像盘符边界。
        let is_drive = bytes[index] == b'S'
            && index + 1 < bytes.len()
            && bytes[index + 1] == b':'
            // 前一个字符是分隔符：否则 H:\pe-wim1.wim 里的 S 也会被误换。
            && (index == 0
                || matches!(
                    bytes[index - 1],
                    b' ' | b'>' | b'(' | b'=' | 0x22 | b'<'
                ))
            // 后一个字符是反斜杠或结尾：System32 里的 S 后面跟 y，不算盘符。
            && (index + 2 >= bytes.len() || bytes[index + 2] == b'\\');
        if is_drive {
            // `S:\` 是三个字符（S、冒号、反斜杠），esp_root 自带尾部反斜杠，
            // 所以三个都要吃掉。少吃掉一个就会留下 `卷根\\` 或 `卷根\:`，
            // 路径立刻失效（2026-09-29 实测两种都踩到过）。
            out.push_str(esp_root);
            index += if index + 2 < bytes.len() && bytes[index + 2] == 92_u8 {
                3
            } else {
                2
            };
            continue;
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

pub(crate) fn esp_log_path(name: &str) -> String {
    if ESP_CONTROL_FILES.contains(&name) {
        return format!("S:\\{name}");
    }
    let _ = std::fs::create_dir_all(ESP_LOG_DIR);
    format!("{ESP_LOG_DIR}\\{name}")
}

pub(crate) fn is_well_known_identifier(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("{bootmgr}")
}

/// BCD 标识符校验：GUID，或白名单里的知名别名（当前只有 `{bootmgr}`）。
///
/// 之所以单独开这一个口子而不是散着判断：`boot_entry.rs` 整个模块是 `#[cfg(windows)]`，
/// 那里的单测在 macOS 上一次都不编译（2026-09-29 `ramdisk=` 值畸形就是这么漏过去的）。
/// 校验放本模块，单测就跟着 `cargo test` 在 macOS 上跑。
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
    use serde_json::json;

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
    fn dism_success_does_not_finish_the_whole_transaction() {
        assert_ne!(
            classify_log_line("The operation completed successfully.")
                .0
                .as_deref(),
            Some("操作完成")
        );
        assert_eq!(
            classify_log_line("操作完成：命令、回读和副档簿记均已通过")
                .0
                .as_deref(),
            Some("操作完成")
        );
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
            Some("恢复已完成，正在清理")
        );
        assert_eq!(
            classify_log_line("Boot entry cleaned; task marked successful")
                .0
                .as_deref(),
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
    fn step_markers_render_as_numbered_stage_and_stay_out_of_detail() {
        let (stage, _percent, detail) = classify_log_line("STEP 2/4 捕获系统分区镜像");
        assert_eq!(stage.as_deref(), Some("2/4 捕获系统分区镜像"));
        assert_eq!(detail, None, "编号步骤行不应占详情框（已由阶段标题展示）");

        // 没有 n/N 的 STEP 行**不**再当成编号步骤：进度窗口要回答"一共几步、
        // 现在第几步"，认不出编号就无法定位，给它一个"步骤：xxx"的标题反而
        // 会让用户以为它在步骤序列里。2026-09-30 收紧，真实日志都带编号。
        let (single, _, _) = classify_log_line("STEP 准备环境");
        assert_eq!(single, None);
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

    /// 守门人不能被别名白名单连带废掉：`{default}` 之类**一律拒**。
    /// bcdedit 在标识符参数为空/不可解析时会静默作用于 `{default}`
    /// （真实事故：曾把 Windows 11 启动项 device 改成 ramdisk）。
    /// 锁死「挂载成功只能看 `/L`，不能看 `/S` 退出码」这条实测结论。
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
    /// 真实日志行带方括号时间戳，编号步骤必须仍能识别。
    ///
    /// 这条是 2026-09-30 的回归锁：此前用 `strip_prefix("STEP ")` 只匹配行首，
    /// 而真实形态是 `[时间戳] STEP 1/4 …`，于是编号步骤从来没能被识别，
    /// PE 进度窗口一直停在初始那句「正在准备恢复环境…」（用户实机反馈）。
    #[test]
    fn parse_step_marker_works_on_real_timestamped_log_lines() {
        let real = "[2026-09-29T03:36:32.259862400+00:00] STEP 1/4 准备备份环境（挂载卷、校验、必要时还原恢复环境）";
        let (index, total, name) = parse_step_marker(real).expect("必须能从真实日志行解析出步骤");
        assert_eq!((index, total), (1, 4));
        assert!(name.contains("准备备份环境"));

        // 阶段标题也要能认出来，不只是步骤跟踪器。
        let (stage, _percent, _detail) = classify_log_line(real);
        assert_eq!(
            stage.as_deref(),
            Some("1/4 准备备份环境（挂载卷、校验、必要时还原恢复环境）")
        );
    }

    /// 认不出步骤标记时返回 None，不能 panic、不能猜。
    #[test]
    fn parse_step_marker_returns_none_for_non_step_lines() {
        assert!(parse_step_marker("").is_none());
        assert!(parse_step_marker("[ts] The operation completed successfully.").is_none());
        assert!(parse_step_marker("[ts] STEP ").is_none());
        assert!(parse_step_marker("[ts] STEP abc/def name").is_none());
        assert!(parse_step_marker("[ts] STEP 9/4 impossible").is_none());
        // 没有 name 也不算
        assert!(parse_step_marker("[ts] STEP 1/4").is_none());
    }

    #[test]
    fn step_progress_reads_numbered_steps_across_increments() {
        let mut tracker = StepTracker::new();

        // 第一批：只看到第 1 步
        let first = tracker.feed(&["[stdout] STEP 1/4 准备备份环境（挂载卷、校验）"]);
        assert_eq!(first.total, Some(4));
        assert_eq!(first.current, Some(1));
        assert_eq!(first.steps.len(), 1);
        assert_eq!(first.steps[0].0, 1);
        assert!(first.steps[0].1.contains("准备备份环境"));

        // 第二批：第 2 步 + DISM 66%
        let second = tracker.feed(&["STEP 2/4 捕获系统分区镜像（DISM）", "[stdout] [66.0%]"]);
        assert_eq!(second.current, Some(2));
        assert_eq!(second.steps.len(), 2, "步骤清单必须跨增量累积");

        tracker.set_current_percent(Some(66));
        let snap = tracker.snapshot();
        // (1 + 0.66) / 4 = 41.5% → 42
        assert_eq!(snap.overall_percent, Some(42));
    }

    /// 重复打同一步骤不能重复入列（续跑/重试会重打）。
    #[test]
    fn step_progress_deduplicates_repeated_steps() {
        let mut tracker = StepTracker::new();
        tracker.feed(&["STEP 1/4 准备备份环境"]);
        let again = tracker.feed(&["STEP 1/4 准备备份环境", "STEP 2/4 捕获"]);
        assert_eq!(again.steps.len(), 2);
        assert_eq!(again.current, Some(2));
    }

    /// 畸形 STEP 行必须被忽略，不能把总数带偏。
    #[test]
    fn step_progress_ignores_malformed_step_lines() {
        let mut tracker = StepTracker::new();
        let none = tracker.feed(&[
            "STEP ",
            "STEP abc",
            "STEP 0/4 bad",
            "STEP 5/4 bad",
            "not a step",
        ]);
        assert!(none.total.is_none());
        assert!(none.steps.is_empty());
        assert!(none.current.is_none());
        assert!(none.overall_percent.is_none());
    }

    /// 没有编号步骤时（老格式日志）不能 panic，也不能算出百分比。
    #[test]
    fn step_progress_without_numbered_steps_reports_nothing() {
        let mut tracker = StepTracker::new();
        let progress =
            tracker.feed(&["[stdout] [100.0%]", "The operation completed successfully."]);
        assert!(progress.total.is_none());
        assert!(progress.current.is_none());
        assert!(
            progress.overall_percent.is_none(),
            "没有 n/N 就不该编造总进度"
        );
    }

    /// 当前步骤百分比要夹在 0–100，且总进度不超过 100。
    #[test]
    fn step_progress_clamps_percentages() {
        let mut tracker = StepTracker::new();
        tracker.feed(&["STEP 1/2 第一步"]);
        tracker.set_current_percent(Some(250));
        let snap = tracker.snapshot();
        assert_eq!(snap.overall_percent, Some(50), "250% 应夹成 100% → 半程");
        tracker.set_current_percent(Some(100));
        assert_eq!(tracker.snapshot().overall_percent, Some(50));
    }

    /// 步骤推进到下一步时，必须清空上一步残留的百分比。
    #[test]
    fn step_progress_resets_current_percent_when_advancing_step() {
        let mut tracker = StepTracker::new();
        tracker.feed(&["STEP 1/4 准备环境"]);
        tracker.set_current_percent(Some(100));
        assert_eq!(tracker.snapshot().current_percent, Some(100));

        // 进入第 2 步，无百分比时应被清空，不应残留上一步的 100%
        let second = tracker.feed(&["STEP 2/4 捕获镜像"]);
        assert_eq!(second.current, Some(2));
        assert_eq!(second.current_percent, None);
    }

    /// 提取时间戳并清理括号内的冗余废话（如“（DISM，百分比见进度条）”）。
    #[test]
    fn step_progress_extracts_timestamp_and_cleans_remarks() {
        let mut tracker = StepTracker::new();
        let log = "[2026-09-30T08:18:08.054361500+00:00] STEP 2/4 捕获系统分区镜像（DISM，百分比见进度条）";
        let progress = tracker.feed(&[log]);
        assert_eq!(progress.steps.len(), 1);
        assert_eq!(progress.steps[0].0, 2);
        assert_eq!(progress.steps[0].1, "捕获系统分区镜像");
        assert!(progress.steps[0].2.is_some());
    }

    /// 验证还原的四阶段简化标题格式正确，不带多余前缀与符号。
    #[test]
    fn step_markers_restore_stages_match_clean_format() {
        let lines = [
            "STEP 1/4 挂载卷、校验",
            "STEP 2/4 还原镜像（时间长）",
            "STEP 3/4 校验",
            "STEP 4/4 清理re启动项/配置",
        ];
        for line in lines {
            let (stage, _, _) = classify_log_line(line);
            assert!(stage.is_some());
            let title = stage.unwrap();
            assert!(!title.contains("步骤"));
            assert!(!title.contains('：'));
            assert!(
                title.starts_with("1/4 ")
                    || title.starts_with("2/4 ")
                    || title.starts_with("3/4 ")
                    || title.starts_with("4/4 ")
            );
        }
    }

    /// 纯 [stdout] / [stderr] 空标签行不占详情。
    #[test]
    fn classify_log_line_filters_empty_stdout_stderr_tags() {
        let (_, _, detail1) = classify_log_line("[stdout]");
        assert!(detail1.is_none());
        let (_, _, detail2) = classify_log_line("  [stdout]   ");
        assert!(detail2.is_none());
        let (_, _, detail3) = classify_log_line("[stderr]");
        assert!(detail3.is_none());
        let (_, _, detail4) = classify_log_line("[stdout] real message");
        assert_eq!(detail4.as_deref(), Some("[stdout] real message"));
    }

    /// UTF-8 解码直接无损通过。
    #[test]
    fn decode_windows_bytes_decodes_utf8_correctly() {
        let msg = "正在备份系统分区";
        assert_eq!(decode_windows_bytes(msg.as_bytes()), msg);
    }

    #[test]
    fn rewrite_s_root_only_replaces_the_esp_drive_letter() {
        let bs = 92u8 as char;
        let mut root = String::new();
        root.push(bs);
        root.push(bs);
        root.push('?');
        root.push(bs);
        root.push_str("Volume{d08d796f-f082-4402-bdbb-a4a6a09ac53f}");
        root.push(bs);

        assert_eq!(
            rewrite_s_root(r"cmd /c dir S:\ > S:\out.txt 2>&1", &root),
            format!("cmd /c dir {root} > {root}out.txt 2>&1")
        );
        // 别的盘符路径里的 S 不能被碰。
        assert_eq!(
            rewrite_s_root(
                r"cmd /c dism /ImageFile:H:\pe-wim1.wim > S:\diag1.txt 2>&1",
                &root
            ),
            format!("cmd /c dism /ImageFile:H:\\pe-wim1.wim > {root}diag1.txt 2>&1")
        );
        // 退回盘符形态时原样返回。
        assert_eq!(
            rewrite_s_root(r"cmd /c dir S:\ > S:\out.txt", r"S:\"),
            r"cmd /c dir S:\ > S:\out.txt"
        );
        // S 出现在词中间（如 System32）不能动。
        assert_eq!(
            rewrite_s_root(
                r"cmd /c if exist C:\Windows\System32\Config\SYSTEM echo S > S:\c.txt",
                &root
            ),
            format!("cmd /c if exist C:\\Windows\\System32\\Config\\SYSTEM echo S > {root}c.txt")
        );
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

    /// 锁住「控制通道留 ESP 根、其余日志进子目录」这条约定。
    /// 这条不是美观问题：PE 启动最早期按固定路径读 S:\pe-task.txt，改目录会让
    /// 整个自动执行链断掉；而 ESP 是引导分区，根目录堆日志会拖慢固件枚举。
    /// （2026-09-29 实测根目录已堆了 38 个 txt/log，约 93 KB。）
    #[test]
    fn esp_log_path_keeps_control_channel_at_the_root() {
        for control in ["pe-task.txt", "pe-task.txt.done", "pe-task-result.txt"] {
            let p = esp_log_path(control);
            assert_eq!(p, format!("S:\\{control}"));
        }
        for log in ["diag1.txt", "bcd-all.txt", "exit-pe.log"] {
            let p = esp_log_path(log);
            assert!(p.starts_with("S:\\BackupRestore\\logs\\"), "{p} 应在子目录");
        }
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
        assert!(
            require_guid("{bootmgr}", "enum").is_err(),
            "旧入口仍然只认 GUID"
        );
    }
}
