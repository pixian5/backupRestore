//! 系统还原收尾：启用目标系统的恢复环境，并核验实际 BCD 关联。
//!
//! 降级策略（用户 2026-10-16 裁定，原 P5，见 `docs/20261016-0048` §5）：
//! 本步骤发生在镜像铺完、引导修好**之后**，目标卷缺注册组件
//! （`reagentc.exe` / `SYSTEM` 注册表）时**不再让整个任务失败**——那样前面
//! 全部工作白费，而系统本身是好的。改为记警告 + 跳过注册 + 照常报成功，
//! 且跳过原因必须写入任务日志、由调用方在结尾复述，不得静默。
//! 身份不符、ESP 与活动 BCD 加载器不一致等**安全闸门仍旧硬失败**，
//! 缺 `Winre.wim` / `boot.sdi` 恢复原件也维持硬失败（既有故障测试矩阵锁定，
//! 超出本次裁定范围）。
use crate::error_code::ErrCode;
use backuprestore_core::TaskError;

#[cfg(all(test, windows))]
#[path = "winre_registration_fault_tests.rs"]
mod fault_tests;

/// 注册前置检查结论：`Proceed` 正常注册；`Skip` 按裁定降级跳过。
#[derive(Debug, PartialEq, Eq)]
enum Preflight {
    Proceed,
    Skip(String),
}

/// 注册结局，供调用方在任务日志里如实收尾。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RegistrationOutcome {
    /// 已完整注册并回读通过。
    Registered,
    /// 按裁定降级跳过；原因已由 preflight 写入任务日志。
    Skipped(String),
}

#[derive(Debug, PartialEq, Eq)]
struct Info {
    enabled: bool,
    location: Option<String>,
    recovery_id: Option<String>,
}

fn parse_info(text: &str) -> Result<Info, TaskError> {
    let states: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (label, value) = line.split_once(':')?;
            if !label.to_ascii_lowercase().contains("windows re") {
                return None;
            }
            match value.trim().to_ascii_lowercase().as_str() {
                // M8：简体中文 `reagentc /info` 输出「已启用 / 已禁用」。
                "enabled" | "已启用" => Some(true),
                "disabled" | "已禁用" => Some(false),
                _ => None,
            }
        })
        .collect();
    if states.len() != 1 {
        return Err(crate::errc(ErrCode::WinreStateAmbiguous));
    }
    let locations: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (_, value) = line.split_once(':')?;
            let value = value.trim().to_ascii_lowercase();
            value
                .starts_with("\\\\?\\globalroot\\device\\harddisk")
                .then_some(value)
        })
        .collect();
    if locations.len() > 1 {
        return Err(crate::errc(ErrCode::WinreLocationNotUnique));
    }
    let identifiers: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (label, value) = line.split_once(':')?;
            label.contains("BCD").then_some(value.trim())
        })
        .collect();
    let recovery_id = match identifiers.as_slice() {
        [value]
            if value.len() == 36
                && crate::bcd_identifier_from_line(&format!("{{{value}}}")).is_some() =>
        {
            Some(format!("{{{}}}", value.to_ascii_lowercase()))
        }
        [] if !states[0] => None,
        _ => return Err(crate::errc(ErrCode::WinreIdentifierAmbiguous)),
    };
    Ok(Info {
        enabled: states[0],
        location: locations.into_iter().next(),
        recovery_id,
    })
}

fn field(block: &str, name: &str) -> Result<String, TaskError> {
    let values: Vec<_> = block
        .lines()
        .filter_map(|line| {
            let mut parts = line.trim().splitn(2, char::is_whitespace);
            let key = parts.next()?;
            // M8：字段名双语匹配（中文 Windows 的「设备 / 操作系统设备 / 路径」）。
            crate::text_parsing::bcd_field_name_is(key, name)
                .then(|| parts.next().unwrap_or("").trim().to_ascii_lowercase())
        })
        .collect();
    if values.len() != 1 {
        return Err(crate::errc_with(ErrCode::BcdFieldDuplicate, &[name]));
    }
    Ok(values[0].clone())
}

fn object<'a>(text: &'a str, id: &str) -> Result<&'a str, TaskError> {
    let blocks: Vec<_> = text
        .split("\n\n")
        .filter(|block| {
            block
                .lines()
                .find_map(|line| {
                    let key = line.split_whitespace().next()?;
                    if key.eq_ignore_ascii_case("identifier") || key == "标识符" {
                        crate::bcd_identifier_from_line(line)
                    } else {
                        None
                    }
                })
                .is_some_and(|value| value.eq_ignore_ascii_case(id))
        })
        .collect();
    if blocks.len() != 1 {
        return Err(crate::errc(ErrCode::BcdObjectDuplicate));
    }
    Ok(blocks[0])
}

/// 已启用的 WinRE 实际落在哪个分区，以及本次会话里它的盘符（若该分区有盘符）。
///
/// 【2026-10-19 D4】旧代码假设「WinRE 一定在目标系统分区上」，把「已启用 WinRE
/// 在独立恢复分区」直接判成异常。实机上本机 WinRE 就在 `disk0/partition6`
/// （独立恢复分区，Y:），于是 bcdboot 成功后 `reconnect` 硬失败 → 任务 failed、
/// WinRE 被禁用、recoverysequence 被清。恢复分区是 Windows 的正常布局之一，
/// 只要它能被验证（分区存在 + winre.wim/boot.sdi 齐备），重接就是合法动作。
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecoveryPayload {
    location: String,
    disk: u32,
    partition: u32,
    letter: Option<char>,
}

impl RecoveryPayload {
    /// 同分区场景（WinRE 就在目标系统卷上）：盘符即目标盘符。
    #[cfg(test)]
    fn at_location(location: &str, letter: char) -> Self {
        let (disk, partition) = parse_recovery_location(location).unwrap_or((0, 0));
        Self {
            location: location.to_string(),
            disk,
            partition,
            letter: Some(letter),
        }
    }
}

/// 从 `reagentc /info` 的 WinRE 位置里解析磁盘号/分区号。
///
/// 接受 `\\?\globalroot\device\harddisk0\partition6\recovery\windowsre` 与
/// `\device\harddisk0\partition6\...` 两种写法；解析不出就返回 `None`，
/// 由调用方决定是拒绝还是退回原判据（绝不猜一个分区号出来）。
fn parse_recovery_location(value: &str) -> Option<(u32, u32)> {
    let normalized = value.trim().to_ascii_lowercase().replace('/', "\\");
    let mut disk = None;
    let mut partition = None;
    for part in normalized.split('\\').filter(|part| !part.is_empty()) {
        if let Some(rest) = part.strip_prefix("harddisk") {
            disk = rest.parse::<u32>().ok();
        } else if let Some(rest) = part.strip_prefix("partition") {
            partition = rest.parse::<u32>().ok();
        }
    }
    match (disk, partition) {
        (Some(disk), Some(partition)) if partition > 0 => Some((disk, partition)),
        _ => None,
    }
}

/// 生产入口：位置从 `info.location` 拿，盘符即目标盘符。
///
/// 只被单测直接用（生产走 `verify_with_payload` + 显式载荷），所以留
/// `#[cfg(test)]`；release 构建里它是死代码。
#[cfg(test)]
fn verify(
    info: &Info,
    text: &str,
    loader: &str,
    letter: char,
    location: &str,
) -> Result<(), TaskError> {
    verify_with_payload(
        info,
        text,
        loader,
        letter,
        &RecoveryPayload::at_location(location, letter),
    )
}

fn verify_with_payload(
    info: &Info,
    text: &str,
    loader: &str,
    system_letter: char,
    payload: &RecoveryPayload,
) -> Result<(), TaskError> {
    if !info.enabled
        || !info
            .location
            .as_deref()
            .is_some_and(|location| location.eq_ignore_ascii_case(&payload.location))
    {
        return Err(crate::errc(ErrCode::WinreNotEnabledOrWrongLocation));
    }
    if !crate::pe_safety::target_loader(text, system_letter)?.eq_ignore_ascii_case(loader) {
        return Err(crate::errc(ErrCode::WinreLoaderChanged));
    }
    let text = text.replace("\r\n", "\n");
    let os = object(&text, loader)?;
    let recovery = field(os, "recoverysequence")?;
    if crate::bcd_identifier_from_line(&recovery) != Some(recovery.as_str()) {
        return Err(crate::errc(ErrCode::RecoveryGuidNotUnique));
    }
    if field(os, "recoveryenabled")? != "yes" {
        return Err(crate::errc(ErrCode::RecoveryEntryNotEnabled));
    }
    if info.recovery_id.as_deref() != Some(recovery.as_str()) {
        return Err(crate::errc(ErrCode::RecoveryBcdMismatch));
    }
    verify_recovery_object(&text, &recovery, payload)
}

/// 把 `ramdisk=[<分区>]<路径>,{选项对象}` 拆成三段。
fn split_ramdisk_device(device: &str) -> Result<(String, String, String), TaskError> {
    let rest = device
        .strip_prefix("ramdisk=")
        .ok_or_else(|| crate::errc(ErrCode::RecoveryDeviceNotRamdisk))?;
    let (token, rest) = rest
        .strip_prefix('[')
        .and_then(|value| value.split_once(']'))
        .ok_or_else(|| crate::errc(ErrCode::RecoveryDeviceNoPartitionId))?;
    let (path, options) = rest
        .rsplit_once(',')
        .ok_or_else(|| crate::errc(ErrCode::RecoveryDeviceNoOptionObject))?;
    Ok((
        token.trim().to_string(),
        path.trim().to_string(),
        options.trim().to_string(),
    ))
}

/// 恢复对象里引用的分区，必须就是 `reagentc` 登记的 WinRE 所在分区。
///
/// 两种合法写法：盘符 `x:` 与设备路径 `\device\harddiskvolumen`（独立恢复分区
/// 没有盘符时 BCD 只能写设备路径）。判据统一落到「磁盘号 + 分区号」上。
fn verify_partition_token(
    token: &str,
    payload: &RecoveryPayload,
    label: &str,
) -> Result<(), TaskError> {
    let token = token.trim().trim_end_matches('\\');
    if token.len() == 2
        && token.ends_with(':')
        && let Some(letter) = token.chars().next().filter(char::is_ascii_alphabetic)
    {
        if let Some(expected) = payload.letter
            && letter.eq_ignore_ascii_case(&expected)
        {
            return Ok(());
        }
        return verify_partition_token_by_letter(letter, payload, label);
    }
    // 【2026-10-19 实机缺陷修复】前缀必须大小写无关：本 crate 的 field() 把 BCD
    // 字段值统一小写化（`\device\harddiskvolume6`），而 bcdedit/bcdboot 写出的是
    // 大写 `\Device\HarddiskVolume6`。只认一种写法就会把合法设备路径误判成
    // 「分区标识无法识别」，或在下一跳 partition_of_device_path 里再栽一次。
    let lowered = token.to_ascii_lowercase();
    if lowered.starts_with(r"\device\") || lowered.starts_with(r"\\.\") {
        return verify_partition_token_by_device(token, payload, label);
    }
    Err(crate::errc_with(ErrCode::PartitionTokenUnrecognized, &[label, token]))
}

#[cfg(windows)]
fn verify_partition_token_by_letter(
    letter: char,
    payload: &RecoveryPayload,
    label: &str,
) -> Result<(), TaskError> {
    let live = crate::windows_prepare::volume_identity(letter)?;
    if live.disk_number == Some(payload.disk) && live.partition_number == Some(payload.partition) {
        return Ok(());
    }
    Err(crate::errc_with(
        ErrCode::PartitionLetterMismatch,
        &[
            label,
            &letter.to_string(),
            &payload.disk.to_string(),
            &payload.partition.to_string(),
        ],
    ))
}

#[cfg(not(windows))]
fn verify_partition_token_by_letter(
    _letter: char,
    _payload: &RecoveryPayload,
    _label: &str,
) -> Result<(), TaskError> {
    Err(crate::errc(ErrCode::PartitionVerifyWindowsOnly))
}

#[cfg(windows)]
fn verify_partition_token_by_device(
    token: &str,
    payload: &RecoveryPayload,
    label: &str,
) -> Result<(), TaskError> {
    let (disk, partition) = crate::windows_prepare::partition_of_device_path(token)?;
    if disk == payload.disk && partition == payload.partition {
        return Ok(());
    }
    Err(crate::errc_with(
        ErrCode::PartitionDeviceMismatch,
        &[
            label,
            token,
            &disk.to_string(),
            &partition.to_string(),
            &payload.disk.to_string(),
            &payload.partition.to_string(),
        ],
    ))
}

#[cfg(not(windows))]
fn verify_partition_token_by_device(
    _token: &str,
    _payload: &RecoveryPayload,
    _label: &str,
) -> Result<(), TaskError> {
    Err(crate::errc(ErrCode::PartitionVerifyWindowsOnly))
}

fn verify_recovery_object(
    text: &str,
    recovery: &str,
    payload: &RecoveryPayload,
) -> Result<(), TaskError> {
    let re = object(text, recovery)?;
    let device = field(re, "device")?;
    let (token, path, options) = split_ramdisk_device(&device)?;
    if !path.eq_ignore_ascii_case("\\recovery\\windowsre\\winre.wim") {
        return Err(crate::errc(ErrCode::RecoveryLoaderNotOriginalWim));
    }
    if crate::bcd_identifier_from_line(&options) != Some(options.as_str()) {
        return Err(crate::errc(ErrCode::RecoveryLoaderOptionInvalid));
    }
    verify_partition_token(&token, payload, "恢复加载器设备")?;
    if field(re, "osdevice")? != device
        || field(re, "path")? != "\\windows\\system32\\winload.efi"
        || field(re, "winpe")? != "yes"
    {
        return Err(crate::errc(ErrCode::RecoveryLoaderIncomplete));
    }
    let ramdisk = object(text, &options)?;
    let sdi_device = field(ramdisk, "ramdisksdidevice")?;
    let sdi_token = sdi_device
        .strip_prefix("partition=")
        .ok_or_else(|| crate::errc(ErrCode::RamdiskDeviceFormatInvalid))?;
    verify_partition_token(sdi_token, payload, "恢复内存盘启动设备")?;
    if field(ramdisk, "ramdisksdipath")? != "\\recovery\\windowsre\\boot.sdi" {
        return Err(crate::errc(ErrCode::RamdiskFileNotInTarget));
    }
    Ok(())
}

trait Backend {
    fn preflight(&mut self) -> Result<Preflight, TaskError>;
    fn info(&mut self) -> Result<Info, TaskError>;
    fn reset_clone(&mut self, _info: &Info) -> Result<bool, TaskError> {
        Ok(false)
    }
    fn register(&mut self) -> Result<(), TaskError>;
    fn enable(&mut self) -> Result<(), TaskError>;
    fn reconnect(&mut self, info: &Info) -> Result<(), TaskError>;
    fn verify(&mut self, info: &Info) -> Result<(), TaskError>;
    fn checkpoint(&self, _point: &str) -> Result<(), TaskError> {
        Ok(())
    }
}

fn run(backend: &mut impl Backend) -> Result<RegistrationOutcome, TaskError> {
    // 缺注册组件：降级跳过（用户 2026-10-16 裁定）。此刻尚未做任何注册写入，
    // 跳过是干净的；checkpoint 让故障追踪链能看到这个决策点。
    if let Preflight::Skip(reason) = backend.preflight()? {
        backend.checkpoint("registration-skipped")?;
        return Ok(RegistrationOutcome::Skipped(reason));
    }
    let mut before = backend.info()?;
    if backend.reset_clone(&before)? {
        before = backend.info()?;
        if before.enabled {
            return Err(crate::errc(ErrCode::SecondaryCloneStillEnabled));
        }
    }
    // BCDBoot 可重建系统加载器；注册仍启用并不代表新加载器已有恢复关联。
    if !before.enabled {
        backend.register()?;
        backend.checkpoint("registration-register")?;
        backend.enable()?;
        backend.checkpoint("registration-enable")?;
    } else {
        backend.reconnect(&before)?;
        backend.checkpoint("registration-reconnect")?;
    }
    let after = backend.info()?;
    backend.verify(&after)?;
    Ok(RegistrationOutcome::Registered)
}

#[cfg(windows)]
pub(crate) fn repair(
    target: &std::path::Path,
    efi: &std::path::Path,
    expected: &backuprestore_core::VolumeIdentity,
    log: &std::path::Path,
) -> Result<RegistrationOutcome, TaskError> {
    repair_with_mode(target, efi, expected, log, false)
}

/// 仅由新增第二系统的已授权任务调用，允许重建目标内克隆来的注册配置。
#[cfg(windows)]
pub(crate) fn repair_secondary(
    target: &std::path::Path,
    efi: &std::path::Path,
    expected: &backuprestore_core::VolumeIdentity,
    log: &std::path::Path,
) -> Result<RegistrationOutcome, TaskError> {
    repair_with_mode(target, efi, expected, log, true)
}

#[cfg(windows)]
fn repair_with_mode(
    target: &std::path::Path,
    efi: &std::path::Path,
    expected: &backuprestore_core::VolumeIdentity,
    log: &std::path::Path,
    independent: bool,
) -> Result<RegistrationOutcome, TaskError> {
    use crate::{append_log, capture_logged, pe_safety, windows_prepare as wp};
    use std::{fs, path::Path};
    struct WindowsBackend<'a> {
        target: &'a Path,
        efi: &'a Path,
        log: &'a Path,
        expected: &'a backuprestore_core::VolumeIdentity,
        letter: char,
        loader: String,
        location: String,
        hashes: Vec<(std::path::PathBuf, String)>,
        original_sdi: Option<Vec<u8>>,
        independent: bool,
    }
    impl WindowsBackend<'_> {
        fn bcd(&self, active: bool) -> Result<String, TaskError> {
            let store = self.efi.join("EFI\\Microsoft\\Boot\\BCD");
            let path = store.to_string_lossy();
            let args = if active {
                vec!["/enum", "all", "/v"]
            } else {
                vec!["/store", &path, "/enum", "all", "/v"]
            };
            capture_logged("bcdedit.exe", &args, self.log)
        }
        fn reagent(&self, args: &[&str]) -> Result<String, TaskError> {
            let program = self.target.join("Windows\\System32\\reagentc.exe");
            // 详细诊断直接落到任务盘；不能依赖重启即消失的 X: 默认日志。
            let diagnostic = self.log.with_extension("reagent.log");
            let diagnostic = diagnostic.to_string_lossy();
            let mut args = args.to_vec();
            args.extend(["/logpath", &diagnostic]);
            capture_logged(&program.to_string_lossy(), &args, self.log)
        }
        fn identity(&self) -> Result<(), TaskError> {
            pe_safety::verify_binding(self.expected, &wp::volume_identity(self.letter)?, false)
        }
        /// 已启用 WinRE 的恢复载荷位置（D4）。
        ///
        /// 同分区（WinRE 在目标系统卷上）→ 直接用目标盘符，行为与旧代码一致。
        /// 独立恢复分区 → **必须**先证明它是一个真实存在、原件齐备的分区，才允许
        /// 重接；证明不了就硬失败，绝不在"位置不明"的情况下改 BCD。
        fn recovery_payload(&self, info: &Info) -> Result<RecoveryPayload, TaskError> {
            let location = info
                .location
                .clone()
                .ok_or_else(|| crate::errc(ErrCode::WinreLocationMissing))?;
            let (disk, partition) = parse_recovery_location(&location).ok_or_else(|| {
                crate::errc_with(ErrCode::WinreLocationUnparsable, &[&location])
            })?;
            if location.eq_ignore_ascii_case(&self.location) {
                return Ok(RecoveryPayload {
                    location,
                    disk,
                    partition,
                    letter: Some(self.letter),
                });
            }
            let letter = crate::letter_for_partition(disk, partition, self.log)?;
            let root = format!(
                r"\\?\GLOBALROOT\device\harddisk{disk}\partition{partition}\recovery\windowsre"
            );
            for name in ["winre.wim", "boot.sdi"] {
                let path = format!(r"{root}\{name}");
                let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                if size == 0 {
                    return Err(crate::errc_with(
                        ErrCode::WinrePartitionMissingFiles,
                        &[&location, name],
                    ));
                }
            }
            append_log(
                self.log,
                &crate::i18n::msg_tpl(
                    "winre_independent_partition",
                    &[location.as_str(), &format!("{letter:?}")],
                ),
            )?;
            Ok(RecoveryPayload {
                location,
                disk,
                partition,
                letter,
            })
        }
    }
    impl Backend for WindowsBackend<'_> {
        fn checkpoint(&self, point: &str) -> Result<(), TaskError> {
            crate::recovery_fault::checkpoint(point, self.log)
        }
        fn preflight(&mut self) -> Result<Preflight, TaskError> {
            // 离线注册要求「纯恢复环境」，但恢复环境有两种形态，二者都合法：
            //   · WinRE —— 有 Control\MiniNT 键（纯恢复环境专属标记）；
            //   · WinPE —— 无 MiniNT，但有 wpeutil.exe（PE 自带工具）。
            // 早先只认 MiniNT，把 PE 入口也挡在外面，于是「PE 作第二系统」会在
            // 目标卷已经格式化、镜像已经应用之后才失败（pe_operation 的 boot() 在
            // bcdboot 之后才调这里），白白毁掉目标卷。
            let is_winre = std::process::Command::new("reg.exe")
                .args(["query", r"HKLM\SYSTEM\CurrentControlSet\Control\MiniNT"])
                .output()
                .is_ok_and(|out| out.status.success());
            let is_winpe = std::path::Path::new(r"X:\Windows\System32\wpeutil.exe").is_file()
                || std::path::Path::new(r"C:\Windows\System32\wpeutil.exe").is_file();
            if !is_winre && !is_winpe {
                return Err(crate::errc(ErrCode::OfflineWinreReOnly));
            }
            // 自定义 WinPE 入口可能绕过 wpeinit；让系统工具读取本次真实固件类型。
            // wpeutil.exe 只有 WinPE 附带，纯 WinRE 环境没有它——那一支已经能自己
            // 判定固件类型，硬调会因「找不到程序」失败，故只在 PE 形态下执行。
            if is_winpe {
                capture_logged("wpeutil.exe", &["UpdateBootInfo"], self.log)?;
            }
            let firmware = capture_logged(
                "reg.exe",
                &[
                    "query",
                    "HKLM\\SYSTEM\\CurrentControlSet\\Control",
                    "/v",
                    "PEFirmwareType",
                ],
                self.log,
            )?;
            if !firmware.lines().any(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                fields == ["PEFirmwareType", "REG_DWORD", "0x2"]
            }) {
                return Err(crate::errc(ErrCode::WinreNeedsUefi));
            }
            self.identity()?;
            // 缺注册组件 = 降级跳过，不是任务失败（用户 2026-10-16 裁定，原 P5）。
            // 这一步在镜像铺完、引导修好之后，硬失败会让前面全部工作白费，而系统
            // 本身是好的。只降级"缺文件"：上面的身份/环境/固件闸门与后面的加载器
            // 一致性闸门仍旧硬失败，那些是安全判据而非能力缺失。
            // 原因必须落任务日志（调用方还会在结尾复述），不得静默跳过。
            for (parts, name) in [
                (
                    vec!["Windows", "System32", "reagentc.exe"],
                    "reagentc.exe",
                ),
                (
                    vec!["Windows", "System32", "config", "SYSTEM"],
                    "SYSTEM 注册表",
                ),
            ] {
                if !pe_safety::probe_file(self.target, &parts)? {
                    let reason = format!(
                        "目标卷缺少恢复环境注册组件（{name}），按裁定跳过恢复环境注册；\
                         系统可正常启动，但不含恢复环境（日后无法从恢复环境检修该系统）"
                    );
                    append_log(self.log, &format!("WinRE registration skipped: {reason}"))?;
                    return Ok(Preflight::Skip(reason));
                }
            }
            self.loader = pe_safety::target_loader(&self.bcd(false)?, self.letter)?;
            // reagentc 没有 /store，必须先确认活动存储中的同一目标，禁止误关联别的系统。
            if pe_safety::target_loader(&self.bcd(true)?, self.letter)? != self.loader {
                return Err(crate::errc(ErrCode::EspLoaderMismatch));
            }
            self.location = format!(
                "\\\\?\\globalroot\\device\\harddisk{}\\partition{}\\recovery\\windowsre",
                self.expected
                    .disk_number
                    .ok_or_else(|| crate::errc(ErrCode::TargetDiskNumberMissing))?,
                self.expected
                    .partition_number
                    .ok_or_else(|| crate::errc(ErrCode::TargetPartitionNumberMissing))?
            );
            for (name, source) in [
                (
                    "Winre.wim",
                    vec!["Windows", "System32", "Recovery", "Winre.wim"],
                ),
                (
                    "boot.sdi",
                    vec!["Windows", "Boot", "DVD", "EFI", "boot.sdi"],
                ),
            ] {
                let parts = ["Recovery", "WindowsRE", name];
                let destination = self.target.join("Recovery\\WindowsRE").join(name);
                // 仅使用本次还原镜像内的原件；绝不拿任务注入镜像作为系统 WinRE。
                if !pe_safety::probe_file(self.target, &parts)? {
                    if !pe_safety::probe_file(self.target, &source)? {
                        return Err(crate::errc_with(ErrCode::ImageMissingRecoveryPayload, &[name]));
                    }
                    let source = source
                        .iter()
                        .fold(self.target.to_path_buf(), |path, item| path.join(item));
                    let hash = crate::sha256_file(&source)?;
                    fs::create_dir_all(destination.parent().expect("恢复目录"))?;
                    fs::copy(&source, &destination)?;
                    if crate::sha256_file(&destination)? != hash {
                        return Err(crate::errc(ErrCode::RecoveryPayloadCopyVerifyFailed));
                    }
                }
                if fs::metadata(&destination)?.len() == 0 {
                    return Err(crate::errc(ErrCode::RecoveryPayloadEmpty));
                }
                self.hashes
                    .push((destination.clone(), crate::sha256_file(&destination)?));
                if name == "boot.sdi" {
                    self.original_sdi = Some(fs::read(&destination)?);
                }
            }
            append_log(
                self.log,
                &format!(
                    "WinRE registration target={} loader={} location={}",
                    self.letter, self.loader, self.location
                ),
            )?;
            Ok(Preflight::Proceed)
        }
        fn info(&mut self) -> Result<Info, TaskError> {
            let windows = self.target.join("Windows");
            parse_info(&self.reagent(&["/info", "/target", &windows.to_string_lossy()])?)
        }
        fn reset_clone(&mut self, info: &Info) -> Result<bool, TaskError> {
            if !self.independent
                || !info.enabled
                || info.location.as_deref() == Some(&self.location)
            {
                return Ok(false);
            }
            self.identity()?;
            // 不调用 /disable：克隆配置引用的是原系统对象，禁用会误伤原系统。
            // 只备份并移走已核验目标内部的两份配置，让系统工具为新加载器建立独立对象。
            for parts in [
                vec!["Windows", "System32", "Recovery", "ReAgent.xml"],
                vec!["Recovery", "WindowsRE", "ReAgent.xml"],
            ] {
                if !pe_safety::probe_file(self.target, &parts)? {
                    continue;
                }
                let path = parts
                    .iter()
                    .fold(self.target.to_path_buf(), |p, part| p.join(part));
                let backup = path.with_extension("xml.before-secondary");
                let mut backup_parts = parts.clone();
                *backup_parts.last_mut().expect("配置文件名") = "ReAgent.xml.before-secondary";
                let exists = pe_safety::probe_file(self.target, &backup_parts)?;
                let hash = crate::sha256_file(&path)?;
                if exists {
                    if crate::sha256_file(&backup)? != hash {
                        return Err(crate::errc(ErrCode::SecondaryBackupExists));
                    }
                } else {
                    fs::copy(&path, &backup)?;
                    if crate::sha256_file(&backup)? != hash {
                        return Err(crate::errc(ErrCode::SecondaryBackupDigestMismatch));
                    }
                }
                fs::remove_file(&path)?;
            }
            append_log(
                self.log,
                crate::i18n::msg("winre_secondary_reset"),
            )?;
            Ok(true)
        }
        fn register(&mut self) -> Result<(), TaskError> {
            self.identity()?;
            let windows = self.target.join("Windows");
            let recovery = self.target.join("Recovery\\WindowsRE");
            self.reagent(&[
                "/setreimage",
                "/path",
                &recovery.to_string_lossy(),
                "/target",
                &windows.to_string_lossy(),
            ])?;
            Ok(())
        }
        fn enable(&mut self) -> Result<(), TaskError> {
            self.identity()?;
            self.reagent(&["/enable", "/osguid", &self.loader])?;
            Ok(())
        }
        fn reconnect(&mut self, info: &Info) -> Result<(), TaskError> {
            self.identity()?;
            let payload = self.recovery_payload(info)?;
            let recovery = info
                .recovery_id
                .as_deref()
                .ok_or_else(|| crate::errc(ErrCode::WinreIdentifierMissing))?;
            let mut missing = false;
            // 两个存储都必须拥有经过完整核验的原件链；不能凭注册状态臆造恢复对象。
            for active in [false, true] {
                let text = self.bcd(active)?.replace("\r\n", "\n");
                verify_recovery_object(&text, recovery, &payload)?;
                let os = object(&text, &self.loader)?;
                let links: Vec<_> = os
                    .lines()
                    .filter(|line| {
                        line.split_whitespace()
                            .next()
                            .is_some_and(|key| key.eq_ignore_ascii_case("recoverysequence"))
                    })
                    .collect();
                if links.is_empty() {
                    missing = true;
                } else if field(os, "recoverysequence")? != recovery {
                    return Err(crate::errc(ErrCode::RecoveryAssociationConflict));
                }
                missing |= field(os, "recoveryenabled").ok().as_deref() != Some("yes");
            }
            if missing {
                self.identity()?;
                let store = self.efi.join("EFI\\Microsoft\\Boot\\BCD");
                for (key, value) in [("recoverysequence", recovery), ("recoveryenabled", "Yes")] {
                    capture_logged(
                        "bcdedit.exe",
                        &[
                            "/store",
                            &store.to_string_lossy(),
                            "/set",
                            &self.loader,
                            key,
                            value,
                        ],
                        self.log,
                    )?;
                }
                append_log(
                    self.log,
                    "WinRE existing registration reconnected to repaired system loader",
                )?;
            }
            Ok(())
        }
        fn verify(&mut self, info: &Info) -> Result<(), TaskError> {
            self.identity()?;
            let payload = self.recovery_payload(info)?;
            verify_with_payload(info, &self.bcd(false)?, &self.loader, self.letter, &payload)?;
            verify_with_payload(info, &self.bcd(true)?, &self.loader, self.letter, &payload)?;
            for (file, hash) in &self.hashes {
                if crate::sha256_file(file)? != *hash {
                    return Err(crate::errc(ErrCode::RecoveryPayloadMutated));
                }
            }
            append_log(
                self.log,
                "WinRE registration verified: Enabled; target location, recoverysequence, ramdisk and original hashes match",
            )
        }
    }
    let letter = target
        .to_string_lossy()
        .chars()
        .next()
        .filter(char::is_ascii_alphabetic)
        .ok_or_else(|| crate::errc(ErrCode::TargetLetterInvalid))?;
    let mut backend = WindowsBackend {
        target,
        efi,
        log,
        expected,
        letter,
        loader: String::new(),
        location: String::new(),
        hashes: Vec::new(),
        original_sdi: None,
        independent,
    };
    match run(&mut backend) {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            // reagentc 在启用失败时可能删除已经存在的 boot.sdi，不能让失败破坏原件。
            if let Some(bytes) = &backend.original_sdi {
                let rollback = (|| -> Result<(), TaskError> {
                    backend.identity()?;
                    pe_safety::probe_file(target, &["Recovery", "WindowsRE", "boot.sdi"])?;
                    let path = target.join("Recovery\\WindowsRE\\boot.sdi");
                    if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
                        fs::write(&path, bytes)?;
                        if fs::read(&path)? != *bytes {
                            return Err(crate::errc(ErrCode::BootSdiRestoreVerifyFailed));
                        }
                        append_log(
                            log,
                            "WinRE registration failed; original boot.sdi restored and verified",
                        )?;
                    }
                    Ok(())
                })();
                if let Err(rollback) = rollback {
                    return Err(crate::err(&format!("{error}; {rollback}")));
                }
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const OS: &str = "{11111111-1111-1111-1111-111111111111}";
    const RE: &str = "{22222222-2222-2222-2222-222222222222}";
    const RD: &str = "{33333333-3333-3333-3333-333333333333}";
    const LOC: &str = "\\\\?\\globalroot\\device\\harddisk1\\partition2\\recovery\\windowsre";
    /// 需外部新快照与专用测试卷；常规测试不执行任何 BCD 写入。
    #[cfg(windows)]
    #[test]
    #[ignore = "需要真实 PE、全新 VM 快照和独立注册夹具"]
    fn offline_registration_on_test_partition() {
        use crate::{capture_logged, windows_prepare as wp};
        use std::{env, path::PathBuf};
        assert_eq!(
            env::var("BR_WINRE_TEST_ACK").as_deref(),
            Ok("fresh-snapshot-test-volume-only")
        );
        let letter = env::var("BR_WINRE_TEST_TARGET")
            .unwrap()
            .chars()
            .next()
            .unwrap();
        let primary = env::var("BR_WINRE_TEST_PRIMARY")
            .unwrap()
            .chars()
            .next()
            .unwrap();
        assert_ne!(letter, primary);
        let target = PathBuf::from(format!("{letter}:\\"));
        assert_eq!(
            std::fs::read_to_string(target.join("BRRE-215-fixture.marker")).unwrap(),
            "offline-registration-fixture"
        );
        let expected = wp::volume_identity(letter).unwrap();
        assert!(
            expected
                .partition_guid
                .eq_ignore_ascii_case(&env::var("BR_WINRE_TEST_GUID").unwrap())
        );
        let efi = PathBuf::from(env::var("BR_WINRE_TEST_EFI").unwrap());
        let log = PathBuf::from(env::var("BR_WINRE_TEST_LOG").unwrap());
        capture_logged(
            "reg.exe",
            &["query", "HKLM\\SYSTEM\\CurrentControlSet\\Control\\MiniNT"],
            &log,
        )
        .unwrap();
        let before = capture_logged("bcdedit.exe", &["/enum", "all", "/v"], &log).unwrap();
        let manager = crate::parse_boot_manager_state(&before).unwrap();
        let primary_id = crate::pe_safety::target_loader(&before, primary).unwrap();
        let copy = capture_logged(
            "bcdedit.exe",
            &[
                "/copy",
                &primary_id,
                "/d",
                "BackupRestore registration fixture",
            ],
            &log,
        )
        .unwrap();
        let id = crate::bcd_identifier_from_line(&copy).unwrap().to_string();
        let result = (|| -> Result<(), TaskError> {
            let partition = format!("partition={letter}:");
            for key in ["device", "osdevice"] {
                capture_logged("bcdedit.exe", &["/set", &id, key, &partition], &log)?;
            }
            capture_logged("bcdedit.exe", &["/displayorder", &id, "/remove"], &log)?;
            // 夹具加载器不继承主系统恢复关联；只对新建对象操作。
            if field(
                object(&before.replace("\r\n", "\n"), &primary_id)?,
                "recoverysequence",
            )
            .is_ok()
            {
                capture_logged(
                    "bcdedit.exe",
                    &["/deletevalue", &id, "recoverysequence"],
                    &log,
                )?;
            }
            // 克隆项不能共享主系统的休眠恢复对象，否则 reagentc /enable
            // 会沿 resumeobject 修改主系统的恢复开关，污染测试基线。
            if field(
                object(&before.replace("\r\n", "\n"), &primary_id)?,
                "resumeobject",
            )
            .is_ok()
            {
                capture_logged("bcdedit.exe", &["/deletevalue", &id, "resumeobject"], &log)?;
            }
            fault_tests::preflight_failures(&target, &efi, &expected, &id, &log)?;
            repair(&target, &efi, &expected, &log)?;
            fault_tests::registered_failures(&target, &efi, &expected, &id, &primary_id, &log)?;
            repair(&target, &efi, &expected, &log)?;
            // 重现实际 C: 故障：注册保持启用，但 BCDBoot 新加载器丢失恢复关联。
            capture_logged(
                "bcdedit.exe",
                &["/deletevalue", &id, "recoverysequence"],
                &log,
            )?;
            capture_logged("bcdedit.exe", &["/set", &id, "recoveryenabled", "No"], &log)?;
            repair(&target, &efi, &expected, &log)?;
            Ok(())
        })();
        // 删除本次新建且指向测试卷的恢复对象及克隆加载器，保留原 BCD 对象。
        let after = capture_logged("bcdedit.exe", &["/enum", "all", "/v"], &log)
            .unwrap()
            .replace("\r\n", "\n");
        for block in after.split("\n\n") {
            let Some(created) = block.lines().find_map(crate::bcd_identifier_from_line) else {
                continue;
            };
            if before.contains(created) || created == id {
                continue;
            }
            let lower = block.to_ascii_lowercase();
            if lower.contains(&format!("ramdisk=[{}:]", letter.to_ascii_lowercase()))
                || field(block, "ramdisksdidevice").ok().as_deref()
                    == Some(&format!("partition={}:", letter.to_ascii_lowercase()))
            {
                capture_logged("bcdedit.exe", &["/delete", created], &log).unwrap();
            }
        }
        capture_logged("bcdedit.exe", &["/delete", &id], &log).unwrap();
        let final_bcd = capture_logged("bcdedit.exe", &["/enum", "all", "/v"], &log).unwrap();
        assert_eq!(crate::parse_boot_manager_state(&final_bcd), Some(manager));
        assert_eq!(final_bcd, before, "测试改变了原有启动对象，必须恢复原基线");
        result.unwrap();
    }
    fn bcd() -> String {
        format!(
            "identifier {OS}\ndevice partition=P:\nosdevice partition=P:\npath \\Windows\\system32\\winload.efi\nrecoverysequence {RE}\nrecoveryenabled Yes\n\nidentifier {RE}\ndevice ramdisk=[P:]\\Recovery\\WindowsRE\\Winre.wim,{RD}\nosdevice ramdisk=[P:]\\Recovery\\WindowsRE\\Winre.wim,{RD}\npath \\windows\\system32\\winload.efi\nwinpe Yes\n\nidentifier {RD}\nramdisksdidevice partition=P:\nramdisksdipath \\Recovery\\WindowsRE\\boot.sdi\n"
        )
    }
    /// D4：`reagentc /info` 的 WinRE 位置必须能解析出磁盘/分区，否则无法判断
    /// 「它是不是本机真实存在的独立恢复分区」。
    #[test]
    fn recovery_location_parses_both_spellings() {
        assert_eq!(
            parse_recovery_location(LOC),
            Some((1, 2)),
            "globalroot 形态"
        );
        assert_eq!(
            parse_recovery_location(r"\device\harddisk0\partition6\Recovery\WindowsRE"),
            Some((0, 6)),
            "设备路径形态"
        );
        assert_eq!(
            parse_recovery_location(r"\\?\GLOBALROOT\device\harddisk12\partition3\recovery\windowsre"),
            Some((12, 3)),
            "大写与多位数"
        );
        for bad in [
            "",
            "enabled",
            r"\device\harddisk0\partition0\recovery\windowsre",
            r"\device\harddisk0\recovery\windowsre",
            r"\device\harddiskx\partition2\recovery",
        ] {
            assert_eq!(parse_recovery_location(bad), None, "{bad}");
        }
    }

    /// D4：独立恢复分区上的恢复对象（设备路径写法）必须能通过校验，
    /// 指向别的分区则必须被拒。非 Windows 构建下设备号无法核对，一律拒绝。
    ///
    /// ★ 2026-10-27 改写：**不再硬编码 `harddisk0\\partition6`**。
    /// 那条断言只有在「本机真的存在 disk0 的 partition6」时才有意义——开发机有
    /// 独立恢复分区，GitHub 的 Windows runner 没有，于是 CI 第一次跑就红
    /// （`CreateFileW` 打不开不存在的卷 → 报错 → 「必须通过」的断言失败）。
    /// 现在用**本机系统卷的真实设备路径**构造规格：任何 Windows 机器上都成立，
    /// 而且比原来更强——它验的是「真设备 → 真分区号」这条链路，而不是一个
    /// 恰好对得上的常量。
    #[test]
    fn recovery_object_accepts_only_the_registered_partition() {
        // 盘符写法：同分区通过 / 别的盘符拒绝（跨平台部分）。
        let same = RecoveryPayload::at_location(LOC, 'P');
        assert!(verify_recovery_object(&bcd(), RE, &same).is_ok());
        assert!(verify_recovery_object(&bcd(), RE, &RecoveryPayload::at_location(LOC, 'Q')).is_err());

        #[cfg(not(windows))]
        {
            // 非 Windows：设备号无法核对，一律拒绝（不得凭设备路径放行）。
            let device_bcd = format!(
                "identifier {OS}\ndevice partition=P:\nosdevice partition=P:\npath \\Windows\\system32\\winload.efi\nrecoverysequence {RE}\nrecoveryenabled Yes\n\nidentifier {RE}\ndevice ramdisk=[\\Device\\HarddiskVolume6]\\Recovery\\WindowsRE\\Winre.wim,{RD}\nosdevice ramdisk=[\\Device\\HarddiskVolume6]\\Recovery\\WindowsRE\\Winre.wim,{RD}\npath \\windows\\system32\\winload.efi\nwinpe Yes\n\nidentifier {RD}\nramdisksdidevice partition=\\Device\\HarddiskVolume6\nramdisksdipath \\Recovery\\WindowsRE\\boot.sdi\n"
            );
            let independent = RecoveryPayload {
                location: r"\\?\globalroot\device\harddisk0\partition6\recovery\windowsre".into(),
                disk: 0,
                partition: 6,
                letter: None,
            };
            assert!(
                verify_recovery_object(&device_bcd, RE, &independent).is_err(),
                "非 Windows 不得凭设备路径放行"
            );
        }

        #[cfg(windows)]
        {
            // 用本机系统卷的真实设备路径：任何 Windows 机器上都存在。
            let system = std::env::var("SystemDrive").expect("SystemDrive 必须存在");
            let drive = system.chars().next().expect("SystemDrive 至少一字符");
            let live = crate::windows_prepare::volume_identity(drive).expect("系统卷身份可读");
            let device = crate::boot_entry::dos_device_of(drive).expect("NT 设备路径可查");
            let disk = live.disk_number.expect("系统卷应有盘号");
            let partition = live.partition_number.expect("系统卷应有分区号");
            let device_bcd = format!(
                "identifier {OS}\ndevice partition=P:\nosdevice partition=P:\npath \\Windows\\system32\\winload.efi\nrecoverysequence {RE}\nrecoveryenabled Yes\n\nidentifier {RE}\ndevice ramdisk=[{device}]\\Recovery\\WindowsRE\\Winre.wim,{RD}\nosdevice ramdisk=[{device}]\\Recovery\\WindowsRE\\Winre.wim,{RD}\npath \\windows\\system32\\winload.efi\nwinpe Yes\n\nidentifier {RD}\nramdisksdidevice partition={device}\nramdisksdipath \\Recovery\\WindowsRE\\boot.sdi\n"
            );
            let real = RecoveryPayload {
                location: format!(r"\\?\globalroot{device}\recovery\windowsre"),
                disk,
                partition,
                letter: None,
            };
            // 正向：真实设备 + 真实分区号 → 必须通过（这条现在在任何机器上都能验）。
            let outcome = verify_recovery_object(&device_bcd, RE, &real);
            assert!(outcome.is_ok(), "真实设备路径应当通过：{outcome:?}（device={device} disk={disk} part={partition}）");
            // 反向：同一台真实设备，但规格声称的分区号不对 → 必须拒（靠真链路而非常量）。
            let wrong = RecoveryPayload {
                location: real.location.clone(),
                disk,
                partition: partition.wrapping_add(1),
                letter: None,
            };
            assert!(
                verify_recovery_object(&device_bcd, RE, &wrong).is_err(),
                "设备路径指向真实分区、但规格声称的分区号不符时必须拒绝"
            );
            // 独立分区没有盘符时，盘符写法必须拒绝（不能拿目标盘符顶替）。
            assert!(verify_recovery_object(&bcd(), RE, &real).is_err());
        }
    }

    /// 【2026-10-19 实机缺陷修复】设备路径的两种大小写写法都必须路由到
    /// 「磁盘号 + 分区号」核对（macOS 上即平台存根报错），而不是在前置字符串
    /// 检查里就被判「无法识别」。本 crate 的 field() 把 BCD 字段值统一小写化，
    /// 而 bcdedit/bcdboot 写大写 `\Device\`——旧实现两处判据各只认一种写法，
    /// 实机 PE 内 recover 的 WinRE 重接因此失败。
    ///
    /// **只在非 Windows 上跑**（2026-10-22 补门控）：本测试的 assert 是
    /// 「报错文案含『仅在 Windows 上可用』」，而 Windows 上 `verify_partition_token`
    /// 会真的去核对设备号，`\Device\HarddiskVolume6` 这类输入可能直接返回 Ok →
    /// `unwrap_err()` panic。也就是说它从加入起在 Windows 上恒失败一条，
    /// 会让「单测全绿」这个验收口径失真（2026-10-22 在 VM 内跑 cli 全量测试时首次暴露）。
    #[cfg(not(windows))]
    #[test]
    fn partition_token_routing_is_case_insensitive() {
        let payload = RecoveryPayload {
            location: LOC.into(),
            disk: 0,
            partition: 6,
            letter: None,
        };
        for accepted in [
            r"\Device\HarddiskVolume6",
            r"\device\harddiskvolume6",
            r"\DEVICE\HARDDISKVOLUME6",
            r"\\.\HarddiskVolume6",
        ] {
            let error = verify_partition_token(accepted, &payload, "测试")
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("仅在 Windows 上可用"),
                "{accepted} 应路由到设备号核对，实际：{error}"
            );
        }
        for rejected in ["C", r"\Device", r"\DeviceHarddiskVolume6", "partition=C:", ""] {
            let error = verify_partition_token(rejected, &payload, "测试")
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("无法识别"),
                "{rejected} 应被判无法识别，实际：{error}"
            );
        }
    }

    #[test]
    fn info_requires_unique_known_status() {
        assert!(!parse_info("Windows RE 状态: Disabled").unwrap().enabled);
        let valid = format!(
            "Windows RE status: Enabled\nWindows RE location: {LOC}\nBCD identifier: {}",
            RE.trim_matches(['{', '}'])
        );
        assert_eq!(
            parse_info(&valid).unwrap(),
            Info {
                enabled: true,
                location: Some(LOC.into()),
                recovery_id: Some(RE.into())
            }
        );
        for invalid in [
            "成功",
            "Windows RE status: unknown",
            "Windows RE status: Enabled\nWindows RE status: Disabled",
            "Windows RE status: Enabled\nBCD identifier: malformed",
            "Windows RE status: Enabled",
        ] {
            assert!(parse_info(invalid).is_err());
        }
        assert!(
            parse_info(&format!(
                "{valid}\nBCD identifier: {}",
                RE.trim_matches(['{', '}'])
            ))
            .is_err()
        );
    }
    #[test]
    fn binding_requires_complete_exact_chain() {
        let info = Info {
            enabled: true,
            location: Some(LOC.into()),
            recovery_id: Some(RE.into()),
        };
        assert!(verify(&info, &bcd(), OS, 'P', LOC).is_ok());
        assert!(
            verify(
                &info,
                &bcd().replace("identifier", "标识符").replace('\n', "\r\n"),
                OS,
                'P',
                LOC
            )
            .is_ok()
        );
        for bad in [
            bcd().replace("recoveryenabled Yes", "recoveryenabled No"),
            bcd().replace("[P:]", "[C:]"),
            bcd().replace(
                "ramdisksdidevice partition=P:",
                "ramdisksdidevice partition=C:",
            ),
            bcd().replace("winpe Yes", "winpe No"),
            bcd().replace("boot.sdi", "other.sdi"),
            bcd().replace("recoverysequence", "description"),
            format!("{}\n\nidentifier {RD}\n", bcd()),
            bcd().replace(
                "recoveryenabled Yes",
                "recoveryenabled Yes\nrecoveryenabled Yes",
            ),
        ] {
            assert!(verify(&info, &bad, OS, 'P', LOC).is_err(), "{bad}");
        }
        assert!(
            verify(
                &Info {
                    enabled: false,
                    location: Some(LOC.into()),
                    recovery_id: Some(RE.into())
                },
                &bcd(),
                OS,
                'P',
                LOC
            )
            .is_err()
        );
        assert!(verify(&info, &bcd(), OS, 'P', "other").is_err());
        assert!(
            verify(
                &Info {
                    recovery_id: Some(OS.into()),
                    ..info
                },
                &bcd(),
                OS,
                'P',
                LOC
            )
            .is_err()
        );
    }
    struct Fake {
        calls: Vec<&'static str>,
        fail: &'static str,
        enabled: bool,
        false_success: bool,
        cloned: bool,
        skip: bool,
    }
    impl Fake {
        fn step(&mut self, name: &'static str) -> Result<(), TaskError> {
            self.calls.push(name);
            if self.fail == name {
                Err(crate::err("注入故障"))
            } else {
                Ok(())
            }
        }
    }
    impl Backend for Fake {
        fn preflight(&mut self) -> Result<Preflight, TaskError> {
            self.step("preflight")?;
            if self.skip {
                return Ok(Preflight::Skip("故障替身：组件缺失".into()));
            }
            Ok(Preflight::Proceed)
        }
        fn info(&mut self) -> Result<Info, TaskError> {
            self.step("info")?;
            Ok(Info {
                enabled: self.enabled,
                location: None,
                recovery_id: None,
            })
        }
        fn reset_clone(&mut self, _info: &Info) -> Result<bool, TaskError> {
            if !self.cloned {
                return Ok(false);
            }
            self.step("reset")?;
            self.enabled = self.fail == "ineffective-reset";
            self.cloned = false;
            Ok(true)
        }
        fn register(&mut self) -> Result<(), TaskError> {
            self.step("register")
        }
        fn enable(&mut self) -> Result<(), TaskError> {
            self.step("enable")?;
            self.enabled = !self.false_success;
            Ok(())
        }
        fn reconnect(&mut self, _info: &Info) -> Result<(), TaskError> {
            self.step("reconnect")
        }
        fn verify(&mut self, info: &Info) -> Result<(), TaskError> {
            self.step("verify")?;
            if info.enabled {
                Ok(())
            } else {
                Err(crate::err("仍未启用"))
            }
        }
    }
    #[test]
    fn registration_stops_on_each_failure_and_rejects_false_success() {
        for fail in ["preflight", "info", "register", "enable", "verify"] {
            let mut fake = Fake {
                calls: Vec::new(),
                fail,
                enabled: false,
                false_success: false,
                cloned: false,
                skip: false,
            };
            assert!(run(&mut fake).is_err());
            assert_eq!(fake.calls.last(), Some(&fail));
        }
        let mut fake = Fake {
            calls: Vec::new(),
            fail: "",
            enabled: false,
            false_success: true,
            cloned: false,
            skip: false,
        };
        assert!(run(&mut fake).is_err());
        let mut fake = Fake {
            enabled: true,
            fail: "reconnect",
            ..fake
        };
        assert!(run(&mut fake).is_err());
        assert_eq!(fake.calls.last(), Some(&"reconnect"));
    }
    /// 用户 2026-10-16 裁定（原 P5）：目标缺注册组件时降级跳过而非任务失败。
    /// 降级必须①报成功②带回跳过原因③不触碰任何注册写入步骤。
    #[test]
    fn registration_degrades_when_components_missing() {
        let mut fake = Fake {
            calls: Vec::new(),
            fail: "",
            enabled: false,
            false_success: false,
            cloned: false,
            skip: true,
        };
        assert_eq!(
            run(&mut fake).unwrap(),
            RegistrationOutcome::Skipped("故障替身：组件缺失".into())
        );
        assert_eq!(fake.calls, ["preflight"]);
    }
    #[test]
    fn registration_is_idempotent_and_always_rereads() {
        let mut fake = Fake {
            calls: Vec::new(),
            fail: "",
            enabled: false,
            false_success: false,
            cloned: false,
            skip: false,
        };
        run(&mut fake).unwrap();
        assert_eq!(
            fake.calls,
            ["preflight", "info", "register", "enable", "info", "verify"]
        );
        fake.calls.clear();
        run(&mut fake).unwrap();
        assert_eq!(
            fake.calls,
            ["preflight", "info", "reconnect", "info", "verify"]
        );
    }

    #[test]
    fn cloned_registration_must_reset_and_reread_before_enabling() {
        for fail in ["", "reset", "ineffective-reset"] {
            let mut fake = Fake {
                calls: Vec::new(),
                fail,
                enabled: true,
                false_success: false,
                cloned: true,
                skip: false,
            };
            let result = run(&mut fake);
            if fail.is_empty() {
                result.unwrap();
                assert_eq!(
                    fake.calls,
                    [
                        "preflight",
                        "info",
                        "reset",
                        "info",
                        "register",
                        "enable",
                        "info",
                        "verify"
                    ]
                );
            } else {
                assert!(result.is_err());
                assert!(!fake.calls.contains(&"register"));
                assert!(!fake.calls.contains(&"enable"));
                assert!(!fake.calls.contains(&"reconnect"));
            }
        }
    }
}
