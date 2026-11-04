//! 卷挂载判定、设备路径规范化与盘符时间线追踪。
//!
//! 从 `text_parsing.rs` 抽离（v2.9.9）的跨平台纯解析模块，负责：
//! - `mountvol <letter>: /L` 查询结果三态判定（`VolumeMountQuery`）与
//!   `mountvol` 无参清单解析（卷 GUID ↔ 盘符配对、有卷判定）；
//! - 卷路径 → 设备路径转换与卷设备名提取（NT 命名空间，大小写无关）；
//! - verbatim 卷路径前缀拼装（反斜杠个数只有一处说了算）；
//! - 卷标识符归一化比较（裸 GUID / verbatim 路径 / 大小写差异一律判等）；
//! - ESP 取证日志路径拼装（子目录纪律：不写 ESP 根目录）；
//! - 恢复会话内多角色共享卷的盘符复用表（`MountedVolumes`）；
//! - 盘符时间线（`GetLogicalDrives` 位图 → 盘符、两次采样差异行，输出全 ASCII）。
//!
//! 与其他 `*_parse` 模块一样保持平台无关：Windows 专属的调用方
//! （`recovery_volume.rs`、`letter_trace.rs`、`prepare_bcd.rs` 等）在 macOS 上
//! 不编译，判断逻辑集中在本模块才能被本机 `cargo test` 完整覆盖——历史上
//! 「测试放在 `#[cfg(windows)]` 模块里所以一次都没跑」已经付过整轮排查的代价。

use std::collections::BTreeMap;

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

// ---- 卷路径 ↔ 设备路径（方案 A 必须的一步转换）----

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

// ---- 卷标识符归一化与比较 ----

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

// ---- ESP 取证日志路径（子目录纪律）----

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

// ---- 共享卷盘符复用表 ----

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
    /// bcdedit/bcdboot 写出的是大写 `\Device\HarddiskVolumeN`——`\device\` 与
    /// `\\.\` 两种前缀、大小写两种写法都必须解析出卷设备名。旧实现只认大写
    /// 前缀，PE 内 recover 的 WinRE 重接因此把合法设备路径误判成「形态无法
    /// 识别」，任务 failed。
    #[test]
    fn device_path_volume_name_handles_both_prefixes_case_insensitively() {
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

    /// verbatim 卷路径前缀的形状锁定：正好 `\\?\Volume`，不带尾反斜杠、全 ASCII。
    /// 「路径里到底有几个反斜杠」只有 [`esp_volume_prefix`] 一处说了算，
    /// 这里防的是有人绕开它手拼前缀导致 verbatim 路径退化。
    #[test]
    fn esp_volume_prefix_shape() {
        let prefix = esp_volume_prefix();
        assert_eq!(prefix, r"\\?\Volume");
        // 前缀不带尾反斜杠：拼 GUID 是调用方的事，避免出现 `Volume\\{...}`。
        assert!(!prefix.ends_with('\\'), "prefix={prefix}");
        assert!(prefix.is_ascii(), "prefix={prefix}");
    }

    /// `bare_volume_guid` 要从 verbatim 路径与裸 GUID 两种写法里都取出 GUID。
    #[test]
    fn bare_volume_guid_handles_verbatim_and_bare_shapes() {
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

    /// 卷标识符归一化：同一个卷的三种写法必须判为相等。
    ///
    /// 2026-09-30 实机事故的直接复现：restore-existing 的 EFI 挂载盘符挂成功了，
    /// 但校验拿 `{GUID}` 比 `\\?\Volume{GUID}\` 判为不同，于是换盘符重试，
    /// 把每个候选盘符都试一遍后整个任务失败。
    #[test]
    fn same_volume_tolerates_format_and_case_differences() {
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

    /// 锁住「日志进子目录、且路径只来自调用方给的已核验卷根」这条约定。
    ///
    /// 旧实现写死 `S:\BackupRestore\logs`（并为已废弃的 `pe-task.txt` 控制通道
    /// 留了 `S:\` 根的特例）。那等于假设 `S:` 就是 ESP——`S:` 被数据卷占用或 ESP
    /// 挂在别的盘符时，日志会落到无关卷上。现在盘符形态与 verbatim 卷路径都接受，
    /// 但必须由调用方传入，本函数不猜、也不做盘符字符串替换。
    /// ESP 是引导分区，根目录堆日志会拖慢固件枚举（2026-09-29 实测根目录曾堆 38 个
    /// txt/log，约 93 KB），所以一律进子目录。
    #[test]
    fn esp_log_path_on_ensures_subdir() {
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
