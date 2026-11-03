//! WIM 镜像元数据解析、WIM 捕获阶段判定与 wimlib 还原进度计算。
//!
//! 包含 DISM 与 wimlib-imagex 输出的多语言与结构化解析逻辑：
//! - DISM JSON 镜像列表（`parse_wim_images`）；
//! - WIM 捕获预扫与写入阶段心跳判据（`capture_phase` / `capture_heartbeat_line`）；
//! - 还原自检行（`restore_selfcheck_line`）；
//! - wimlib 还原三阶段（Creating / Extracting / Metadata）百分比折算与进度提取。

use serde_json::Value;

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

/// 还原终态自检行：把「目标卷序列号前后 + Windows 内核文件是否存在」压成一行
/// 固定 ASCII 前缀的结论，写进任务日志。
///
/// ★ 故意不走 i18n：它是**判据数据行**（与 `STEP n/m`、`stage=success` 同类），
/// 交给语言层后判据会随界面语言漂移——2026-10-25 英文模式下终态识别失效正是这么
/// 来的（那时是拿中文原文当判据）。判据只认结构化前缀与字段值，永不翻译。
/// ★ 它取代的是人工「放哨兵文件 + 记序列号」这套验收动作：`serial_before` 与
/// `serial_after` 不同即证明目标卷被重写过，与哨兵消失等价。
pub(crate) fn restore_selfcheck_line(
    letter: char,
    before: &str,
    after: &str,
    ntoskrnl: bool,
) -> String {
    format!(
        "RESTORE_SELFCHECK target={letter} serial_before={before} serial_after={after} ntoskrnl={ntoskrnl}"
    )
}

/// WIM 头部长度上限：DISM 一创建镜像文件就写下 WIM 头（实测恰好 208 字节），
/// 此后在**全卷预扫**阶段一个字节都不写。用它区分「预扫中」与「正在写入」。
///
/// ★ 这个阈值来自 2026-10-26 实测，不是猜的：捕获 `F:\Program Files`
/// （8.7 GB / 32,732 文件）时镜像从 07:33:21 到 07:35:11 恒为 208 字节共 100 秒，
/// 之后 50 秒内写到 3.4 GB。所以「零字节静默」在开头是**正常**的，
/// 不能据此判定卡死——这正是旧版界面上「看起来死了」的真实原因。
pub(crate) const CAPTURE_HEADER_BYTES_MAX: u64 = 1024;

/// 捕获阶段判据：`pre_scan` = 只有 WIM 头、还没开始写数据；`writing` = 已在写。
pub(crate) fn capture_phase(image_bytes: u64) -> &'static str {
    if image_bytes <= CAPTURE_HEADER_BYTES_MAX {
        "pre_scan"
    } else {
        "writing"
    }
}

/// 捕获心跳行：把「已跑多久 + 镜像已经多大 + 处于哪个阶段」压成一行固定 ASCII，
/// 每 `interval` 秒写一次。
///
/// ★ 存在的理由：DISM 捕获整卷时**先全卷预扫、后写数据**，预扫期间镜像大小恒等于
/// WIM 头（208 字节）、`dism.log` 也一行不写、stdout 只有横幅——三层全是静默。
/// 没有心跳，用户与验收脚本都无法区分「在扫盘」与「卡死」，2026-10-26 那次
/// 58 分钟零产出的观测就是被迫用外部脚本每 30 秒采样才看清的。
/// ★ 与 `RESTORE_SELFCHECK` 同理：走固定前缀、不走 i18n，判据不能随界面语言漂移。
pub(crate) fn capture_heartbeat_line(elapsed_secs: u64, image_bytes: u64) -> String {
    format!(
        "CAPTURE_HEARTBEAT elapsed_secs={elapsed_secs} image_bytes={image_bytes} phase={}",
        capture_phase(image_bytes)
    )
}

/// 捕获停滞行：镜像在 `silent_secs` 秒内没有任何增长时追加一行。
///
/// ★ 只**记录**、绝不影响捕获：预扫时长随文件数线性增长（实测 8.7 GB 约 100 秒，
/// 构建卷那种海量小文件可以到几分钟），在这里杀进程会把正常备份也杀掉。
/// 它取代的是「界面上什么都没发生」这种无信息状态，而不是取代超时策略——
/// 超时仍由 `dism_timeout` 那个显式配置的逃生阀负责。
pub(crate) fn capture_stall_line(elapsed_secs: u64, image_bytes: u64, silent_secs: u64) -> String {
    format!(
        "CAPTURE_STALL elapsed_secs={elapsed_secs} image_bytes={image_bytes} silent_secs={silent_secs} phase={} note=dism_scans_whole_volume_before_first_write",
        capture_phase(image_bytes)
    )
}

/// 捕获收尾行：心跳线程停下时补一行总账，供验收脚本直接读。
pub(crate) fn capture_summary_line(elapsed_secs: u64, image_bytes: u64, samples: u64) -> String {
    format!(
        "CAPTURE_SUMMARY elapsed_secs={elapsed_secs} image_bytes={image_bytes} samples={samples} phase={}",
        capture_phase(image_bytes)
    )
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

/// `Creating files`（建空文件）→ `Extracting file data`（写数据，最耗时）→
/// `Applying metadata to files`（写元数据/安全描述符）。所以阶段名必须一起
/// 解析出来，否则进度条会在中途倒退两次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WimlibPhase {
    Creating,
    Extracting,
    Metadata,
}

impl WimlibPhase {
    /// 中文阶段名（用于日志尾部的可读进度行）。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Creating => "正在创建文件",
            Self::Extracting => "正在展开系统文件",
            Self::Metadata => "正在应用文件元数据",
        }
    }
}

/// 从一行 wimlib 进度输出里取 `(阶段, 阶段内百分比)`。
///
/// 真实形态（`\r` 原地刷新，重定向到日志后由调用方按 `\r`/`\n` 拆行）：
/// - `Creating files: 500 of 633699 (1%) done`
/// - `Extracting file data: 15 GiB of 68 GiB (22%) done`
/// - `Applying metadata to files: 500 of 633699 (1%) done`
///
/// **为什么不能用 `parse_percent`**：那个函数是给 DISM 的
/// `[=  45.6% =]` 写的——它把「最近的 `[`（没有就从行首）到 `%`」之间的
/// 数字全拼起来再解析。wimlib 的行首是计数（`500 of 633699`、`15 GiB of 68 GiB`），
/// 拼出来是 `5006336991`、`156822` 这种六位以上数字，必然落在 0–100 之外
/// → 永远 `None` → 进度条恒为 0。这是 v2.6.27 实机里「进度条一直是空」的根因。
pub(crate) fn parse_wimlib_progress(line: &str) -> Option<(WimlibPhase, u32)> {
    let phase = if line.contains("Creating files:") {
        WimlibPhase::Creating
    } else if line.contains("Extracting file data:") {
        WimlibPhase::Extracting
    } else if line.contains("Applying metadata to files:") {
        WimlibPhase::Metadata
    } else {
        return None;
    };
    // 百分比在行尾的最后一对括号里：取最后一个 '(' 之后到 '%' 之间的数字。
    let open = line.rfind('(')?;
    let close = open + line[open..].find('%')?;
    let digits: String = line[open + 1..close]
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let value = digits.parse::<f32>().ok()?;
    if !(0.0..=100.0).contains(&value) {
        return None;
    }
    Some((phase, value as u32))
}

/// 把 wimlib 的「阶段内百分比」折算成整个 apply 的 0–100。
///
/// 进度条只显示当前阶段的进度，而第 2 步「还原镜像」对用户来说是一件事，
/// 所以三个阶段要拼成一条单调的 0–100。权重按 2026-10-23 实测（68 GiB /
/// 633,699 条目，整条 apply 11 分 43 秒）：创建文件约 1 分钟、展开数据约
/// 9 分半、应用元数据约 1 分 15 秒 → 10% / 80% / 10%。
/// 三阶段顺序固定，映射天然单调（实测 2,639 条真实进度片段零回退）。
pub(crate) fn wimlib_overall_percent(phase: WimlibPhase, phase_percent: u32) -> u32 {
    let percent = phase_percent.min(100);
    let (base, span) = match phase {
        WimlibPhase::Creating => (0u32, 10u32),
        WimlibPhase::Extracting => (10, 80),
        WimlibPhase::Metadata => (90, 10),
    };
    (base + span * percent / 100).min(100)
}

/// 把 wimlib 的英文进度片段改写成一句可读的中文进度行。
///
/// 原样塞进日志尾部会每秒刷掉一条真实消息（一条 `Extracting file data: …`
/// 每秒来一次），而且 `Creating files: 500 of 633699 (1%) done` 这种英文计数
/// 对用户没有意义。保留数字、换掉标签。
pub(crate) fn wimlib_detail_line(phase: WimlibPhase, line: &str) -> String {
    let body = line
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(line)
        .trim()
        .trim_end_matches("done")
        .trim();
    format!("{}：{body}", phase.label())
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn restore_selfcheck_line_is_ascii_and_language_independent() {
        let line = restore_selfcheck_line('E', "08835EE0", "E880494B", true);
        assert_eq!(
            line,
            "RESTORE_SELFCHECK target=E serial_before=08835EE0 serial_after=E880494B ntoskrnl=true"
        );
        // 判据靠前缀与字段定位，整行必须是纯 ASCII（不随界面语言变化）。
        assert!(line.is_ascii(), "self-check line must stay ASCII: {line}");
        assert!(line.starts_with("RESTORE_SELFCHECK target="));
        // 覆盖式还原（序列号不变）也必须照实记，不能因为"没变"就不写。
        let overlay = restore_selfcheck_line('D', "56E51A05", "56E51A05", false);
        assert!(overlay.contains("serial_before=56E51A05 serial_after=56E51A05"));
        assert!(overlay.contains("ntoskrnl=false"));
    }

    #[test]
    fn capture_phase_splits_header_only_from_real_writing() {
        // 208 字节是 DISM 实测写下的 WIM 头长度；「只有头」必须判成预扫。
        assert_eq!(capture_phase(0), "pre_scan");
        assert_eq!(capture_phase(208), "pre_scan");
        assert_eq!(capture_phase(CAPTURE_HEADER_BYTES_MAX), "pre_scan");
        assert_eq!(capture_phase(CAPTURE_HEADER_BYTES_MAX + 1), "writing");
        assert_eq!(capture_phase(143_216_153), "writing");
    }

    #[test]
    fn capture_heartbeat_lines_are_ascii_and_language_independent() {
        let beat = capture_heartbeat_line(100, 208);
        assert_eq!(
            beat,
            "CAPTURE_HEARTBEAT elapsed_secs=100 image_bytes=208 phase=pre_scan"
        );
        assert!(beat.is_ascii(), "heartbeat must stay ASCII: {beat}");
        assert!(beat.starts_with("CAPTURE_HEARTBEAT elapsed_secs="));

        let stall = capture_stall_line(600, 208, 420);
        assert!(stall.is_ascii(), "stall must stay ASCII: {stall}");
        assert!(stall.contains("silent_secs=420"));
        assert!(stall.contains("phase=pre_scan"));
        // 停滞行只描述现象，不带任何「已失败」的结论：预扫慢不等于失败。
        assert!(!stall.contains("failed"));

        let summary = capture_summary_line(1234, 3_414_782_392, 41);
        assert!(summary.is_ascii(), "summary must stay ASCII: {summary}");
        assert!(summary.contains("image_bytes=3414782392"));
        assert!(summary.contains("phase=writing"));
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
    fn dism_percent_parses_decimals_and_rejects_out_of_range() {
        assert_eq!(parse_percent("[stdout] [=  45.6% =]"), Some(45));
        assert_eq!(parse_percent("[stdout] [==100.0%==]"), Some(100));
        assert_eq!(parse_percent("[  0.0%  ]"), Some(0));
        assert_eq!(parse_percent("no percent sign"), None);
        assert_eq!(parse_percent("[999%]"), None);
        assert_eq!(parse_percent("[abc%]"), None);
    }

    #[test]
    fn wimlib_progress_lines_yield_phase_and_percent() {
        assert_eq!(
            parse_wimlib_progress("Creating files: 500 of 633699 (1%) done"),
            Some((WimlibPhase::Creating, 1))
        );
        assert_eq!(
            parse_wimlib_progress("Extracting file data: 15 GiB of 68 GiB (22%) done"),
            Some((WimlibPhase::Extracting, 22))
        );
        assert_eq!(
            parse_wimlib_progress("Applying metadata to files: 633699 of 633699 (100%) done"),
            Some((WimlibPhase::Metadata, 100))
        );
        // 不是 wimlib 的行，不能误判。
        assert_eq!(parse_wimlib_progress("[stdout] [=  45.6% =]"), None);
        assert_eq!(parse_wimlib_progress("STEP 2/4 还原镜像（时间长）"), None);
        assert_eq!(parse_wimlib_progress("Creating files: 500 of 633699"), None);
    }

    #[test]
    fn wimlib_phases_fold_into_one_monotonic_zero_to_hundred() {
        // 三段各报各的 0–100，折成整条 apply 的一条 0–100，且不倒退。
        assert_eq!(wimlib_overall_percent(WimlibPhase::Creating, 0), 0);
        assert_eq!(wimlib_overall_percent(WimlibPhase::Creating, 100), 10);
        assert_eq!(wimlib_overall_percent(WimlibPhase::Extracting, 0), 10);
        assert_eq!(wimlib_overall_percent(WimlibPhase::Extracting, 50), 50);
        assert_eq!(wimlib_overall_percent(WimlibPhase::Extracting, 100), 90);
        assert_eq!(wimlib_overall_percent(WimlibPhase::Metadata, 0), 90);
        assert_eq!(wimlib_overall_percent(WimlibPhase::Metadata, 100), 100);
        // 越界夹紧，绝不把进度条推出 100。
        assert_eq!(wimlib_overall_percent(WimlibPhase::Metadata, 250), 100);
        let curve: Vec<u32> = [
            (WimlibPhase::Creating, 0),
            (WimlibPhase::Creating, 100),
            (WimlibPhase::Extracting, 0),
            (WimlibPhase::Extracting, 100),
            (WimlibPhase::Metadata, 0),
            (WimlibPhase::Metadata, 100),
        ]
        .into_iter()
        .map(|(phase, percent)| wimlib_overall_percent(phase, percent))
        .collect();
        assert!(
            curve.windows(2).all(|pair| pair[0] <= pair[1]),
            "阶段切换不能让进度条倒退：{curve:?}"
        );
    }
}

#[cfg(test)]
mod replay_probe {
    use super::*;

    /// 一次回放的结果：按 apply 分段后的百分比序列 + 解析失败的条数。
    struct Replay {
        segments: Vec<Vec<u32>>,
        /// `parse_wimlib_progress` 命中、但 `classify_log_line` 给不出百分比的条数。
        /// 单独计数而不是「不 push 就算了」——后者只能靠 `parsed == fragments`
        /// 间接发现，一旦两边同时漏就是不报错。
        unparsed: usize,
    }

    /// 把一份日志文本回放成按 apply 分段的结果。
    ///
    /// 日志跨多次 apply（同一次还原里 DISM 会 apply 多次），**每次 apply 的进度
    /// 都从 0 开始**，所以单调性必须按 apply 分段检查——整份日志一起看必然「倒退」。
    fn replay(text: &str) -> Replay {
        let mut segments: Vec<Vec<u32>> = vec![Vec::new()];
        let mut unparsed = 0usize;
        for line in text.split(['\n', '\r']) {
            if line.contains("Applying image") {
                segments.push(Vec::new());
                continue;
            }
            if parse_wimlib_progress(line).is_none() {
                continue;
            }
            match crate::text_parsing::classify_log_line(line).1 {
                Some(percent) => segments.last_mut().unwrap().push(percent),
                None => unparsed += 1,
            }
        }
        Replay {
            segments: segments.into_iter().filter(|s| !s.is_empty()).collect(),
            unparsed,
        }
    }

    /// 三条断言的**唯一实现**，两份输入（入库夹具 / 本机全量日志）共用。
    ///
    /// 返回解析出的片段数，供调用方按各自规模设下限——否则「夹具只剩 3 条」
    /// 也能全绿（判据规模必须自己有闸）。
    fn assert_replay(label: &str, text: &str) -> usize {
        let Replay { segments, unparsed } = replay(text);
        let parsed: usize = segments.iter().map(Vec::len).sum();
        let mut regress = 0usize;
        for segment in &segments {
            let mut high = 0u32;
            for value in segment {
                if *value < high {
                    regress += 1;
                }
                high = high.max(*value);
            }
        }
        eprintln!(
            "{label}: apply 段数={} parsed={parsed} unparsed={unparsed} regress={regress}",
            segments.len()
        );
        for (index, segment) in segments.iter().enumerate() {
            eprintln!(
                "  apply#{index}: {} 条，{}% → {}%",
                segment.len(),
                segment.first().unwrap(),
                segment.last().unwrap()
            );
        }
        // ★ 空集必须先拦：一条片段都没有时，下面三条断言全部成立于空集。
        assert!(
            !segments.is_empty(),
            "{label}: 一条 wimlib 进度片段都没有——判据等于没跑，不许算通过"
        );
        assert_eq!(unparsed, 0, "{label}: 有 {unparsed} 条片段解析不出百分比");
        assert_eq!(regress, 0, "{label}: 同一次 apply 内进度条不允许倒退");
        assert!(
            segments.iter().all(|s| *s.last().unwrap() == 100),
            "{label}: 每次 apply 都必须走到 100%"
        );
        parsed
    }

    /// **常态判据**（CI 也跑）：回放入库夹具。
    ///
    /// ★ 2026-10-27 改造。原来只有一份「读本机 `.test-artifacts` 的 82 MB 原始日志」的
    ///   用例，且日志不在时直接 `return` —— 于是**判据静默消失却仍显示通过**：
    ///   CI 上恒绿零信息，本机清一次 `.test-artifacts` 也就永久没了。
    ///   现在夹具入库（`tools/test/extract-wimlib-fixture.py` 从原始日志逐字节抽样：
    ///   2 个 apply 段、每阶段 30 条、共约 9 KB），**夹具缺失直接 panic**。
    #[test]
    fn replay_wimlib_progress_fixture_is_monotone_and_completes() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("wimlib-progress-227.log");
        let raw = std::fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "夹具必须入库（缺失 = 判据消失，不是跳过）：{}：{error}",
                path.display()
            )
        });
        let text = String::from_utf8_lossy(&raw);
        let parsed = assert_replay("fixture", &text);
        // 规模闸：抽稀到几乎没内容时，三条断言会变得很容易满足。
        assert!(
            parsed >= 100,
            "夹具覆盖不足（只有 {parsed} 条片段）：重新跑 tools/test/extract-wimlib-fixture.py"
        );
    }

    /// 定位本机 82 MB 原始日志。
    ///
    /// ★ 不能用 `env!("CARGO_MANIFEST_DIR")` 单一定位：那是**编译期**路径。
    ///   门禁（`tools/test/ignored-acceptance.sh`）在 Windows 上从
    ///   `<root>\crates\backuprestore-cli` 起跑、日志放在 `<root>\.test-artifacts\`，
    ///   而编译发生在 `X:` —— 拼出来的绝对路径必然不存在，用例会判 FAIL
    ///   （2026-10-27 实测：改成 CARGO_MANIFEST_DIR 后门禁立刻红）。
    ///   所以按「环境变量 → 当前目录相对 → 源码树」的顺序找，与旧实现的
    ///   cwd 语义保持一致。
    fn locate_full_log() -> std::path::PathBuf {
        if let Ok(explicit) = std::env::var("BR_WIMLIB_LOG") {
            return std::path::PathBuf::from(explicit);
        }
        let from_cwd = std::path::PathBuf::from("../../.test-artifacts/e2e-brimglog-227.log");
        if from_cwd.is_file() {
            return from_cwd;
        }
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(".test-artifacts")
            .join("e2e-brimglog-227.log")
    }

    /// 本机全量取证（82 MB 原始日志不入库）：
    /// `cargo test -p backuprestore-cli replay_real_wimlib_log -- --ignored --nocapture`
    ///
    /// 与常态判据共用 `assert_replay`，差异只在输入与规模。
    /// 可用 `BR_WIMLIB_LOG=<路径>` 指定日志；常态判据不依赖这份日志（夹具已入库）。
    #[test]
    #[ignore = "需要本机 .test-artifacts 里的 82MB 原始恢复日志（全量取证；常态判据见 fixture 版）"]
    fn replay_real_wimlib_log() {
        let path = locate_full_log();
        let raw = std::fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "全量取证需要原始日志：{}：{error}（可用 BR_WIMLIB_LOG 指定；\
                 常态判据不依赖它，见 replay_wimlib_progress_fixture_is_monotone_and_completes）",
                path.display()
            )
        });
        let text = String::from_utf8_lossy(&raw);
        let parsed = assert_replay("full", &text);
        assert!(
            parsed >= 2_000,
            "全量日志应有数千条片段，实际只有 {parsed}"
        );
    }
}
