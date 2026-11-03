//! 卷挂载、临时盘符分配与生命周期租约（MountLease）管理。
//!
//! 本模块拥有系统 EFI/WinRE 分区定位、卷 GUID 与 Diskpart 盘符挂载、
//! 以及 RAII 作用域租约（`MountLease`）与崩溃兜底清扫机制。

use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use backuprestore_core::{TaskError, VolumeIdentity};

use crate::error_code::ErrCode;
use crate::prepare_bcd::esp_identity_without_drive_letter;
use crate::volume_device::volume_identity;
use crate::{append_log, err};

/// EFI 系统分区的分区类型 GUID。**判定 ESP 必须用它，不能用盘符**——盘符在不同
/// 环境里会重排（恢复环境内尤其如此），本项目此前把 `S:` 当作 ESP 只是约定，
/// `S:` 被别的卷占用或 ESP 挂到别的盘符时都会判错（`text_parsing.rs` 与
/// `native_gui.rs` 的注释都记录过这个靠不住的假设）。
pub(crate) const EFI_TYPE: &str = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}";
#[allow(dead_code)]
pub(crate) const RECOVERY_TYPE: &str = "{de94bba4-06d1-4d40-a16a-bfd50179d6ac}";
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn capture(program: &str, args: &[&str]) -> Result<String, TaskError> {
    crate::process_runner::capture_output(program, args)
}

pub(crate) fn recovery_identity() -> Result<VolumeIdentity, TaskError> {
    // 首选 `reagentc /info`。但它有一个**上下文级**的已知失败模式：退出码 9
    // （`WinReGetWimVersion failed to initialize DISM`），根因是 `C:\Windows\Logs\DISM`
    // 目录的 ACL 不给 Administrators 写权限、DISM 写不了自己的日志（2026-10-16
    // 实测钉死，见 `docs/20261016-1413` §1）。该失败**不代表 WinRE 真的坏了**，
    // 却会让整个准备链在 314ms 内 hard-fail（用户 2026-10-16 裁定：这种情况记警告
    // 继续，别在这里把流程掐死——真有问题是后面的 WIM 挂载/注入会自己报错）。
    //
    // 因此这里改为两级：
    //   ①`reagentc /info` 成功 → 按报告的分区身份继续（原逻辑）。
    //   ②失败 → 退回从**BCD 的 `recoverysequence` → ramdisk 卷路径**推导 WinRE 所在卷。
    let output = match capture("reagentc.exe", &["/info"]) {
        Ok(output) => output,
        Err(info_error) => {
            return recovery_identity_from_bcd()
                .map_err(|bcd_error| {
                    crate::errc_with(
                        ErrCode::WinreIdentityProbeFailed,
                        &[&info_error.to_string(), &bcd_error.to_string()],
                    )
                });
        }
    };
    let lower = output.to_ascii_lowercase();
    let disk = extract_after(&lower, "harddisk")
        .and_then(|value| value.split('\\').next()?.parse().ok())
        .ok_or_else(|| err("WinRE disk identity is missing"))?;
    let partition = extract_after(&lower, "partition")
        .and_then(|value| value.split('\\').next()?.parse().ok())
        .ok_or_else(|| err("WinRE partition identity is missing"))?;
    let identity = identity_from_diskpart(disk, partition, 'R')?;
    // Windows 官方支持两种 WinRE 注册形态：GPT Recovery 分区（de94bba4-…）或
    // OS 分区的 \Recovery\WindowsRE（reagentc /enable 在多数现代系统上会把
    // WinRE 复制回 OS 分区，无法稳定留在 Recovery 分区）。两种形态的 BCD
    // ramdisk 引导行为一致（已在 Parallels ARM64 实机验证可正常引导进 WinRE）。
    // 这里不再强制要求 Recovery 分区类型：identity 仅用于“工作区/镜像卷不得
    // 与恢复分区同卷”的安全检查，按 reagentc 报告的真实分区继续即可；
    // 若 WinRE 文件缺失，后续 WIM 挂载/注入会自行报错。
    Ok(identity)
}

/// `reagentc /info` 失败时的兜底：从 BCD 里已注册的 WinRE 启动对象反推其所处卷。
///
/// 路径：`bcdedit /enum {bootmgr}` 找到当前系统加载器 → 其 `recoverysequence`
/// 指向的恢复对象 → 该对象的 `device`/`osdevice` 是
/// `ramdisk=[X:]\Recovery\WindowsRE\Winre.wim,{设备选项}` → 取出 `[X:]` 卷 → 用
/// `volume_identity` 拿卷身份（盘符在这里只是**读**的入口，identity 里绝不记它）。
///
/// 只在 `/info` 失败时调用，属**尽力而为**：任何一步取不到就如实报错，
/// 不制造假身份（`recovery` 身份下游还用于定位 boot.sdi 与写 RECOVERY 角色，
/// 给错比给不出更危险）。
pub(crate) fn recovery_identity_from_bcd() -> Result<VolumeIdentity, TaskError> {
    // 当前系统加载器 = `{bootmgr}.default`；`/v` 才输出真实 GUID。
    let bootmgr = capture(
        "bcdedit.exe",
        &["/enum", "{bootmgr}", "/v"],
    )?;
    let default_loader = extract_after(&bootmgr.to_ascii_lowercase(), "default")
        .and_then(|value| value.lines().next())
        .and_then(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .ok_or_else(|| crate::errc(ErrCode::BcdNoDefaultLoader))?;
    // 该加载器的 recoverysequence（WinRE 恢复对象的 GUID）。
    let os_block = capture("bcdedit.exe", &["/enum", &default_loader, "/v"])?;
    let recovery_guid = extract_after(&os_block.to_ascii_lowercase(), "recoverysequence")
        .and_then(|value| value.lines().next())
        .and_then(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .ok_or_else(|| crate::errc(ErrCode::BcdNoRecoverySequence))?;
    // 恢复对象的 device：`ramdisk=[X:]\Recovery\...\Winre.wim,{…}`。
    let recovery_block = capture("bcdedit.exe", &["/enum", &recovery_guid, "/v"])?;
    let device = extract_after(&recovery_block, "device")
        .and_then(|value| value.lines().next())
        .map(|line| line.trim().to_ascii_lowercase())
        .ok_or_else(|| crate::errc(ErrCode::BcdRecoveryNoDevice))?;
    // 从 `ramdisk=[x:]...` 里取卷标识（单字母盘符）。
    let letter = device
        .split_once("ramdisk=[")
        .and_then(|(_, rest)| rest.split_once(':'))
        .and_then(|(letter, _)| letter.chars().next())
        .filter(|c| c.is_ascii_alphanumeric())
        .ok_or_else(|| crate::errc(ErrCode::BcdRecoveryRamdiskUnrecognized))?;
    volume_identity(letter.to_ascii_uppercase())
}

pub(crate) fn efi_identity(override_drive: Option<char>) -> Result<VolumeIdentity, TaskError> {
    if let Some(letter) = override_drive {
        let identity = volume_identity(letter)?;
        if !identity.partition_type_guid.eq_ignore_ascii_case(EFI_TYPE) {
            return Err(err("specified EFI drive is not a GPT EFI system partition"));
        }
        return Ok(identity);
    }
    // 方案 A（v1.8.2）：**先试零盘符**。
    // 在「未挂载的隐藏卷」里找分区类型 GUID 为 EFI 且真有 Boot/BCD 的那一个，
    // 全程不分配盘符。拿不到再退回下面这套 mountvol 循环——那是保底，不是首选。
    if let Ok(identity) = esp_identity_without_drive_letter() {
        return Ok(identity);
    }
    // The system EFI partition is normally hidden and has no drive letter.
    // More importantly, a development/test EFI can already be mounted (for
    // example as E:). It must never win merely because it is visible first:
    // `mountvol /S` is Windows' authoritative way to expose the EFI that the
    // current system actually booted from. Only if that lookup genuinely
    // fails do we fall back to an already-mounted EFI for diagnostics.
    let mut last_error = None;
    for letter in identity_drive_candidates('Z') {
        if !is_drive_letter_available(letter) {
            continue;
        }
        // ★ 2026-09-29 实机测量：**`mountvol X: /S` 的退出码不可信**。
        // 提权会话里逐盘符实测（`.test-artifacts/elev-channel/mvwhy.txt`）：
        //   Z: EXIT=1 → 但随后 `/L` 显示 ESP 已经挂在 Z: 上（成功却报失败）
        //   Y: EXIT=0 → `/L` 显示的仍是同一个卷（ESP 已挂，重复 `/S` 无事可做）
        // 原来的代码信退出码，于是把成功的那一次当失败跳过去、换下一个盘符
        // 再挂一次同一个卷——这正是 S:/Z: 盘符反复出现、窗口"不可访问"的机制来源。
        // 现在改成：发起 `/S` 之后**用 `/L` 确认真的挂上了**，只看结果不看退出码。
        let _ = Command::new("mountvol.exe")
            .args([format!("{letter}:"), "/S".to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        if !mountvol_letter_shows_volume(letter) {
            last_error = Some(err(&format!(
                "mountvol /S did not expose a volume at {letter}:; not the boot ESP?"
            )));
            continue;
        }
        // 时间线：ESP 这次挂载是**临时**的（下面立刻 `/D` 撤掉），而且要躲开
        // 「`/S` 退出码骗人」那类噪声——所以这里记的是「`/L` 已确认挂上」这个事实。
        crate::letter_trace::note(&format!(
            "ASSIGN {letter}: via=mountvol /S (temporary ESP inspection; released immediately)"
        ));
        let identity = volume_identity(letter);
        let _ = Command::new("mountvol.exe")
            .args([format!("{letter}:"), "/D".to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        crate::letter_trace::note(&format!("RELEASE {letter}: via=mountvol /D (ESP inspection done)"));
        match identity {
            Ok(mut identity) if identity.partition_type_guid.eq_ignore_ascii_case(EFI_TYPE) => {
                // The letter was only a temporary mount used to inspect the
                // hidden system ESP. Clear it before returning: later code
                // must call ensure_volume_mounted again rather than treating
                // an already-removed drive letter as authoritative.
                identity.drive_letter = None;
                return Ok(identity);
            }
            Ok(identity) => {
                last_error = Some(err(&format!(
                    "mountvol /S exposed disk {} partition {} but it is not an EFI partition",
                    identity.disk_number.unwrap_or_default(),
                    identity.partition_number.unwrap_or_default(),
                )));
            }
            Err(error) => last_error = Some(error),
        }
    }
    // Never fall back to an arbitrary mounted EFI here. A development EFI
    // may be visible as E: while the current system EFI is hidden; selecting
    // it would write the wrong BCD and make the task appear successful while
    // leaving the actual Boot Manager unchanged.
    Err(last_error.unwrap_or_else(|| err("system GPT EFI partition was not found")))
}

pub(crate) fn identity_from_diskpart(
    disk: u32,
    partition: u32,
    preferred: char,
) -> Result<VolumeIdentity, TaskError> {
    // First reuse an already-mounted partition.  A fixed preferred letter
    // is only a hint; it may be occupied by an unrelated volume in WinRE.
    for letter in 'C'..='Z' {
        let Ok(identity) = volume_identity(letter) else {
            continue;
        };
        if identity.disk_number == Some(disk) && identity.partition_number == Some(partition) {
            // 时间线：这次**没有**新分配盘符，用的是既有的。写下来是为了让
            // 「谁挂的」这条链完整——不然事后只看到 `REUSE`，会以为盘符本来就是它的。
            crate::letter_trace::note(&format!(
                "REUSE {letter}: disk {disk} partition {partition} (already mounted; no assignment)"
            ));
            return Ok(identity);
        }
    }

    let mut last_error = None;
    for letter in identity_drive_candidates(preferred) {
        if !is_drive_letter_available(letter) {
            continue;
        }
        let script = format!(
            "select disk {disk}\r\nselect partition {partition}\r\nassign letter={letter}\r\n"
        );
        let temp = env::temp_dir().join(format!(
            "BackupRestore-identity-{disk}-{partition}-{letter}.txt"
        ));
        fs::write(&temp, script)?;
        let result = capture("diskpart.exe", &["/s", &temp.to_string_lossy()]);
        let _ = fs::remove_file(&temp);
        if let Err(error) = result {
            last_error = Some(error);
            continue;
        }
        match volume_identity(letter) {
            Ok(identity)
                if identity.disk_number == Some(disk)
                    && identity.partition_number == Some(partition) =>
            {
                // ★ 这就是那条「无租约的持久挂载」：`diskpart assign letter=` 写的是
                //   `HKLM\SYSTEM\MountedDevices`，撤不撤由调用方负责，而这里**没有**
                //   租约。时间线把它钉死——谁、哪个盘符、哪块盘的哪个分区、什么时候。
                crate::letter_trace::note(&format!(
                    "ASSIGN {letter}: disk {disk} partition {partition} via=diskpart \
                     (persistent MountedDevices write; no lease covers it)"
                ));
                return Ok(identity);
            }
            Ok(identity) => {
                last_error = Some(err(&format!(
                    "DiskPart assigned {letter}: to disk {} partition {}, expected disk {disk} partition {partition}",
                    identity.disk_number.unwrap_or_default(),
                    identity.partition_number.unwrap_or_default(),
                )));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        err(&format!(
            "unable to mount disk {disk} partition {partition} on an available drive letter"
        ))
    }))
}

pub(crate) fn identity_drive_candidates(preferred: char) -> impl Iterator<Item = char> {
    std::iter::once(preferred)
        .chain('C'..='Z')
        .filter(|letter| *letter != 'X')
        .collect::<Vec<_>>()
        .into_iter()
        .fold(Vec::new(), |mut letters, letter| {
            if !letters.contains(&letter) {
                letters.push(letter);
            }
            letters
        })
        .into_iter()
}

/// `mountvol X: /L` 是否报告该盘符上**有**卷。
///
/// 这是挂载成功的唯一可靠判据：`/S` 的退出码在实机上会骗人（见
/// `efi_identity` 里 2026-09-29 的测量记录），`/L` 的结果不会。
#[cfg(windows)]
pub(crate) fn mountvol_letter_shows_volume(letter: char) -> bool {
    let Ok(output) = Command::new("mountvol.exe")
        .args([format!("{letter}:"), "/L".to_string()])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return false;
    };
    // 判据本体在 text_parsing（macOS 也编译、有单测），这里只是跑一下 mountvol。
    crate::text_parsing::mountvol_listing_has_volume(&String::from_utf8_lossy(&output.stdout))
}

pub(crate) fn is_drive_letter_available(letter: char) -> bool {
    let Ok(output) = Command::new("mountvol.exe")
        .args([format!("{letter}:"), "/L".to_string()])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        // `mountvol X: /L` exits with code 1 when the letter has no mount
        // point (the normal free-letter case). Assignment below still has to
        // succeed and is followed by full identity verification.
        return true;
    }
    !String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .any(|line| !line.is_empty())
}

/// Find a drive letter that Windows already exposes for `identity`.
///
/// Matching on the disk/partition GUIDs (and the volume GUID when known) is
/// what makes the resume path independent from numeric coordinates: drive
/// letters are reassigned between Windows and WinRE, but the GUIDs are stable.
pub(crate) fn mounted_letter_for_identity(identity: &VolumeIdentity) -> Option<char> {
    let volume_guid = identity.volume_guid.trim();
    for letter in 'C'..='Z' {
        if !Path::new(&format!(r"{}:\", letter)).is_dir() {
            continue;
        }
        let Ok(actual) = volume_identity(letter) else {
            continue;
        };
        if actual.same_partition(identity) {
            return Some(letter);
        }
        if !volume_guid.is_empty() && actual.volume_guid.eq_ignore_ascii_case(volume_guid) {
            return Some(letter);
        }
    }
    None
}

/// Attach `\\?\Volume{...}` to a free drive letter with `mountvol`.
pub(crate) fn mount_volume_guid(volume_guid: &str, preferred: char) -> Result<char, TaskError> {
    let trimmed = volume_guid.trim().trim_end_matches('\\').to_string();
    if trimmed.is_empty() {
        return Err(err("volume GUID is empty"));
    }
    // `mountvol <letter>: <name>` 要求卷名为 `\\?\Volume{GUID}\` 形式。
    // 但 `VolumeIdentity.volume_guid` 在不同来源下格式不一：从盘符读回的是
    // `\\?\Volume{GUID}\`，而 `esp_identity_without_drive_letter` 走 verbatim
    // 路径得到的是裸 `{GUID}`。后者直接喂给 mountvol 会被当成非法名称而失败，
    // 进而落到 diskpart 兜底、在首选盘符被占用时彻底报错（见 2026-10-12 在线还原
    // 首跑失败）。这里统一规整为 mountvol 期望的形式，保证 GUID 挂载路径可用。
    let upper = trimmed.to_uppercase();
    let mount_arg = if upper.starts_with("\\\\?\\VOLUME{") {
        format!("{trimmed}\\")
    } else if trimmed.starts_with('{') {
        format!("\\\\?\\Volume{trimmed}\\")
    } else {
        format!("{trimmed}\\")
    };
    let mount_target = mount_arg.trim_end_matches('\\').to_string();
    let mut last_error = None;
    for letter in identity_drive_candidates(preferred) {
        if !is_drive_letter_available(letter) {
            continue;
        }
        let output = match Command::new("mountvol.exe")
            .args([format!("{letter}:"), mount_arg.clone()])
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            Ok(output) => output,
            Err(error) => {
                last_error = Some(err(&format!("mountvol {letter}: failed to start: {error}")));
                continue;
            }
        };
        if !output.status.success() {
            last_error = Some(err(&format!(
                "mountvol {letter}: exited with {}",
                output.status
            )));
            continue;
        }
        // Assignment succeeding is not proof: verify the letter really points
        // at the volume we asked for before handing it back.
        match volume_identity(letter) {
            Ok(actual)
                if actual
                    .volume_guid
                    .trim()
                    .trim_end_matches('\\')
                    .eq_ignore_ascii_case(&mount_target) =>
            {
                crate::letter_trace::note(&format!(
                    "ASSIGN {letter}: via=mountvol (volume GUID {mount_target}; leased, will be released)"
                ));
                return Ok(letter);
            }
            Ok(actual) => {
                last_error = Some(err(&format!(
                    "mountvol assigned {letter}: to {}, expected {mount_target}",
                    actual.volume_guid
                )));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| err("unable to mount the volume GUID on an available drive letter")))
}

/// 撤销一次临时挂载（`mountvol <letter>: /D`）。
///
/// 失败只记日志：撤不掉最坏是留一个盘符，不能反过来把正在跑的任务打断。
pub(crate) fn release_mounted_letter(letter: char, log: &Path) {
    let output = Command::new("mountvol.exe")
        .args([format!("{letter}:"), "/D".to_string()])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let outcome = match &output {
        Ok(run) if run.status.success() => "released".to_string(),
        Ok(run) => format!("mountvol /D exited with {}", run.status),
        Err(error) => format!("mountvol /D failed to start: {error}"),
    };
    // 时间线收口：这是「谁撤的」。撤失败也要记——留一个盘符正是要追的幽灵。
    crate::letter_trace::note(&format!("RELEASE {letter}: via=mountvol /D ({outcome})"));
    let _ = append_log(
        log,
        &format!("released temporary drive letter {letter}: ({outcome})"),
    );
}

/// 「本次调用亲手挂上去」的盘符，离开作用域自动撤销。
///
/// ★ 为什么必须存在（2026-10-27 实机事故）：`mountvol <letter>: <卷>` 与 diskpart
///   `assign letter=` 写的都是 **持久**记录（`HKLM\SYSTEM\MountedDevices`）。用完不撤，
///   盘符就永久留在系统里；等那个分区后来被删掉，记录还在，Windows 会为这个并不存在
///   的卷反复弹「V:\\ 无法访问。设备未就绪。」——本机一次性查出 6 个这样的幽灵
///   （B/G/I/Q/V/Z，`\DosDevices\*` 指向的分区 GUID 在所有磁盘上都已不存在）。
///
/// 只有**新分配**的盘符才进租约：卷本来就有盘符时（`mounted_letter_for_identity`
/// 或 `identity.drive_letter` 命中）撤掉会把用户正在用的盘符弄丢。
pub(crate) struct MountLease {
    letter: Option<char>,
    log: PathBuf,
}

impl MountLease {
    pub(crate) fn new(letter: char, created: bool, log: &Path) -> Self {
        let lease = Self {
            letter: created.then_some(letter),
            log: log.to_path_buf(),
        };
        if let Some(letter) = lease.letter {
            register_lease(letter, &lease.log);
            crate::letter_trace::note(&format!(
                "LEASE {letter}: created by this process (released on drop or at process exit)"
            ));
        }
        lease
    }

    /// 仅调试断言用：本租约是否为空。
    ///
    /// 空 = 本次**没有**新分配盘符（复用了既有盘符），`hold_for_process()` 与
    /// `Drop` 都不会动盘符。
    pub(crate) fn owns_no_letter(&self) -> bool {
        self.letter.is_none()
    }

    /// 显式声明：这个盘符要活到**本进程结束**。
    ///
    /// 给「字母确实会逸出」的调用点用——写进持久化方案、被拼进返回的路径字符串、
    /// 存进结构体字段。这些地方没法让租约跟着作用域走，但也不能就这么放开不管。
    pub(crate) fn hold_for_process(mut self) {
        if let Some(letter) = self.letter.take() {
            let mut held = process_leases()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !held.contains(&letter) {
                held.push(letter);
            }
            let _ = append_log(
                &self.log,
                &format!("drive letter {letter}: held for the rest of this process"),
            );
            crate::letter_trace::note(&format!(
                "HOLD {letter}: kept until process exit (released by release_process_leases)"
            ));
        }
    }
}

impl Drop for MountLease {
    fn drop(&mut self) {
        if let Some(letter) = self.letter {
            unregister_lease(letter);
            release_mounted_letter(letter, &self.log);
        }
    }
}

// ---------- 进程级租约集合 + 磁盘登记（崩溃兜底）----------

static PROCESS_LEASES: Mutex<Vec<char>> = Mutex::new(Vec::new());

fn process_leases() -> &'static Mutex<Vec<char>> {
    &PROCESS_LEASES
}

fn lease_registry_path() -> Option<PathBuf> {
    let dir = crate::log_root::logs_dir();
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("mount-leases.txt"))
}

/// 登记一行 `<letter> <pid>`（尽力而为：登记失败不影响挂载本身）。
fn register_lease(letter: char, log: &Path) {
    let Some(path) = lease_registry_path() else {
        return;
    };
    let line = format!("{} {}\n", letter, std::process::id());
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            if let Err(error) = file.write_all(line.as_bytes()) {
                let _ = append_log(log, &format!("lease registry append failed: {error}"));
            }
        }
        Err(error) => {
            let _ = append_log(log, &format!("lease registry open failed: {error}"));
        }
    }
}

/// 划掉本进程登记的那一行（正常撤销时调用）。
fn unregister_lease(letter: char) {
    let Some(path) = lease_registry_path() else {
        return;
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let me = std::process::id().to_string();
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| {
            let mut parts = line.split_whitespace();
            !(parts.next() == Some(&letter.to_string()) && parts.next() == Some(me.as_str()))
        })
        .collect();
    let _ = fs::write(&path, format!("{}\n", kept.join("\n")));
}

/// 进程收尾：把 `hold_for_process()` 留下的盘符全部撤销。
pub(crate) fn release_process_leases(log: &Path) {
    let drained: Vec<char> = {
        let mut held = process_leases()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *held)
    };
    for letter in drained {
        unregister_lease(letter);
        release_mounted_letter(letter, log);
    }
}

/// 启动时的崩溃兜底：只清理「登记还在、但登记它的进程已经不在了」的盘符。
pub(crate) fn release_dead_owner_leases(log: &Path) {
    let Some(path) = lease_registry_path() else {
        return;
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let mut alive_lines: Vec<String> = Vec::new();
    let mut released_any = false;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(letter_text), Some(pid_text)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some(letter) = letter_text.chars().next() else {
            continue;
        };
        let Ok(pid) = pid_text.parse::<u32>() else {
            continue;
        };
        if pid == std::process::id() || process_still_alive(pid) {
            alive_lines.push(line.to_string());
            continue;
        }
        release_mounted_letter(letter, log);
        released_any = true;
    }
    if released_any {
        let _ = append_log(
            log,
            "startup sweep: released drive letters whose owner process was gone",
        );
    }
    let _ = fs::write(&path, format!("{}\n", alive_lines.join("\n")));
}

/// 该 PID 是否仍是活着的进程。
fn process_still_alive(pid: u32) -> bool {
    crate::windows_command::process_is_alive(pid)
}

/// 挂载一个卷并把盘符交给调用方，**随租约返回**：本次若新分配了盘符，
/// 租约离开作用域就 `mountvol /D` 撤销（或 `hold_for_process()` 延到进程结束）。
pub(crate) fn ensure_volume_mounted_leased(
    identity: &VolumeIdentity,
    preferred: char,
    log: &Path,
) -> Result<(char, MountLease), TaskError> {
    let (letter, created) = ensure_volume_mounted_tracked(identity, preferred, log)?;
    Ok((letter, MountLease::new(letter, created, log)))
}

/// 返回 `(盘符, 本次是否新分配)`。
fn ensure_volume_mounted_tracked(
    identity: &VolumeIdentity,
    preferred: char,
    _log: &Path,
) -> Result<(char, bool), TaskError> {
    if let Some(letter) = identity.drive_letter {
        // 保存的盘符只是提示，跨启动后可能已经属于另一分区。
        if let Ok(actual) = volume_identity(letter)
            && identity.same_partition(&actual)
            && identity.partition_offset == actual.partition_offset
            && identity.partition_size == actual.partition_size
        {
            return Ok((letter, false));
        }
    }
    // Prefer reusing a mount point that already exists.
    if let Some(letter) = mounted_letter_for_identity(identity) {
        return Ok((letter, false));
    }
    // Attach the volume by its GUID path.
    if !identity.volume_guid.trim().is_empty()
        && let Ok(letter) = mount_volume_guid(&identity.volume_guid, preferred)
    {
        return Ok((letter, true));
    }
    let disk = identity.disk_number.ok_or_else(|| {
        err("volume has no disk number and could not be matched by GUID or an existing mount point")
    })?;
    let partition = identity
        .partition_number
        .ok_or_else(|| err("volume has no partition number"))?;
    let mounted = identity_from_diskpart(disk, partition, preferred)?;
    if !mounted.same_partition(identity)
        || !mounted
            .volume_guid
            .eq_ignore_ascii_case(&identity.volume_guid)
        || mounted.partition_offset != identity.partition_offset
        || mounted.partition_size != identity.partition_size
        || !mounted
            .partition_type_guid
            .eq_ignore_ascii_case(&identity.partition_type_guid)
        || !mounted
            .filesystem
            .eq_ignore_ascii_case(&identity.filesystem)
    {
        return Err(err("mounted volume identity does not match task"));
    }
    let letter = mounted
        .drive_letter
        .ok_or_else(|| err("mounted volume has no drive letter"))?;
    Ok((letter, true))
}

pub(crate) fn extract_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    text.split_once(marker).map(|(_, value)| value)
}
