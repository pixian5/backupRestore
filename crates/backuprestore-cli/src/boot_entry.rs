//! v1.7.11 新启动通道：**不碰系统注册 WinRE**，用「镜像卷上的载荷 WIM + 自建 BCD 条目」启动。
//!
//! 背景与实机依据：`docs/20260929-1300-pe-channel-poc-winre-wim-boots-from-image-volume.md`。
//! 旧通道要把注入写回 `X:\Recovery\WindowsRE\Winre.wim`（注册位），于是衍生出一整批机制：
//! 迁出（`reagentc /disable`→`/setreimage`→`/enable`）、`WINRE_HOME_*`、待回家标记与桌面收尾、
//! Plan D 捕获前翻回干净件、F1/F3 两道闸、以及与 Windows servicing 的竞态。
//! 新通道把载荷放到**镜像卷**（本来就可写、且不被还原格式化），注册位全程只是**只读**的资产来源，
//! 上面那批机制随之全部不需要。
//!
//! ## 实机踩过并已固化成代码约束的坑
//!
//! 1. **空 GUID 会让 `bcdedit` 静默作用于 `{default}`** —— 曾因此把真实 Windows 11 启动项的
//!    `device`/`osdevice` 改成了 ramdisk。所以本模块每一次 `/set`、`/delete` 之前都必须
//!    断言 GUID 非空且形如 `{8-4-4-4-12}`，宁可不建条目也绝不碰 `{default}`。
//! 2. **`/create /application OSLOADER` 建的条目吃不下 `device ramdisk=…`**；
//!    `/create {GUID} /device` 建的设备选项对象也不能作 `ramdisk=` 的目标（报「按规定设备无效」）。
//!    唯一可用组合是 **`/copy` 现成的 ramdisk 条目 + `/copy` 它的设备选项对象，再改字段**。
//! 3. `device`/`osdevice` 的值形态是 `ramdisk=[<路径>],{<设备选项对象>}`，**结尾没有 `]`**。
//! 4. `reagentc /info` 的「BCD 标识符」**不带花括号**，解析要两种形态都接受。
//! 5. 判退出码不能用 `… 2>&1 | Out-String` 之后读 `$LASTEXITCODE`（Rust 侧不存在该问题，
//!    但 PowerShell 脚手架会）——本模块一律用 `Command::output()` 的 `status.code()`。
//! 6. `reagentc /disable`+`/enable` 会**删掉并重建** WinRE BCD 条目，模板 GUID 每次都要现读。

use backuprestore_core::{TaskError, VolumeIdentity, read_json, write_json_atomic};
use std::fs;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::windows_prepare::ensure_volume_mounted;

/// 镜像卷上存放载荷 RE 的子目录名（不占卷根，避免与用户文件混在一起）。
use crate::boot_cleanup::RE_STAGING_DIR;
/// 任务目录里记录「本次自建启动项」的簿记文件名。
const ENTRY_RECORD: &str = "boot-entry.json";
/// 一次性 `bootsequence` 只设在 bootmgr 上；绝不写 default / displayorder / timeout。
const BOOTMGR: &str = "{bootmgr}";
/// 目标系统自建条目在 BCD 里的描述前缀（便于人工排查与清理）。
const ENTRY_DESCRIPTION: &str = "BackupRestore task RE";

/// 一条「自建启动项」的完整簿记：GUID、载荷位置、载荷哈希。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReBootEntry {
    /// 我们创建的 osloader 条目 GUID（带花括号）。
    pub loader_guid: String,
    /// 我们创建的设备选项对象 GUID（带花括号）。
    pub devopts_guid: String,
    /// 载荷 WIM 所在的卷身份（WinRE/桌面都用它按 GUID 解析盘符，绝不硬编码字母）。
    pub wim_volume: VolumeIdentity,
    /// 载荷 WIM 的绝对路径，形如 `F:\BackupRestoreRE\Winre.wim`。
    pub wim_path: String,
    /// 载荷 WIM 的 SHA-256（重武装前必须复核，防止被改动/截断）。
    pub wim_sha256: String,
    /// 记录的创建时间（RFC3339）。
    pub created: String,
}

impl ReBootEntry {
    fn record_path(task_dir: &Path) -> PathBuf {
        task_dir.join(ENTRY_RECORD)
    }

    /// 读取任务的启动项簿记；没有记录（旧任务 / 尚未建条目）返回 `None`。
    pub fn read(task_dir: &Path) -> Result<Option<Self>, TaskError> {
        let path = Self::record_path(task_dir);
        if !path.is_file() {
            return Ok(None);
        }
        read_json(path).map(Some)
    }

    pub(crate) fn write(&self, task_dir: &Path) -> Result<(), TaskError> {
        write_json_atomic(Self::record_path(task_dir), self)
    }
}

const BCDEDIT: &str = r"C:\Windows\System32\bcdedit.exe";

/// 只读操作（`/enum`、`/export` 等）：必须捕获输出，所以用管道。
#[cfg(windows)]
fn bcd(args: &[&str]) -> Result<String, TaskError> {
    let output = Command::new(BCDEDIT)
        .args(args)
        .stdin(Stdio::null())
        .creation_flags(crate::CREATE_NO_WINDOW)
        .output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(crate::err(&format!(
            "bcdedit {} failed with {}: {}",
            args.join(" "),
            output.status,
            text.trim()
        )));
    }
    Ok(text)
}

/// 写操作（`/set`、`/copy`、`/delete`、`/bootsequence`）：直接调 bcdedit，判退出码。
///
/// 这里一度绕过一圈「交给 `BackupRestore.exe bcd-set` 子进程代跑」，理由是以为
/// 「产品进程上下文会让 bcdedit 拒绝 ramdisk 设备」。2026-09-29 用 ProcMon 抓到真实
/// 命令行后证明那是误判：失败的从来不是进程，而是 `ramdisk=` 的值本身写畸形了
/// （见 `text_parsing::ramdisk_spec`）。绕路已全部拆掉，留这段注释防止再绕回去。
#[cfg(windows)]
fn bcd_write_once(args: &[&str]) -> Result<(), TaskError> {
    bcd(args).map(|_| ())
}

/// 写操作 + 重试（先试 `.status()` 形态；个别环境里无父控制台可继承时退回 `.output()`）。
#[cfg(windows)]
fn bcd_write(args: &[&str], log: &Path) -> Result<(), TaskError> {
    let mut last: Option<TaskError> = None;
    for attempt in 1..=3 {
        match bcd_write_once(args) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last = Some(error);
                if attempt < 3 {
                    crate::append_log(
                        log,
                        &format!(
                            "new boot channel: bcdedit {} attempt {attempt} failed; retrying",
                            args.join(" ")
                        ),
                    )?;
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| crate::err("bcdedit write failed")))
}

/// 建后即验：对象必须真的可枚举，否则后面所有 `/set` 都会落空。
#[cfg(windows)]
fn bcd_object_exists(guid: &str, log: &Path) -> Result<(), TaskError> {
    let guid = crate::text_parsing::require_guid(guid, "verify object")
        .map_err(|message| crate::err(&message))?;
    match bcd(&["/enum", &guid, "/v"]) {
        Ok(_) => Ok(()),
        Err(error) => {
            crate::append_log(
                log,
                &format!("new boot channel: object {guid} is not visible yet: {error}"),
            )?;
            Err(error)
        }
    }
}

/// BCD 标识符校验：要么是 GUID，要么是**源码常量 `{bootmgr}` 这个知名别名**。
///
/// 校验函数在 `text_parsing` 里，不在本模块：本模块整个是 `#[cfg(windows)]`，
/// 放在这里的单测在 macOS 上一次都不会编译——上次 `ramdisk=` 值畸形就是这么漏过去的。

/// `bcdedit /enum <guid> /v` 的某个字段值。
fn bcd_field(guid: &str, field: &str) -> Result<String, TaskError> {
    let guid = crate::text_parsing::require_identifier(guid, "enum")
        .map_err(|message| crate::err(&message))?;
    let text = bcd(&["/enum", &guid, "/v"])?;
    let prefix = format!("{field} ");
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            return Ok(rest.trim().to_string());
        }
    }
    Ok(String::new())
}

/// 从「当前注册的 WinRE 条目」取模板：`reagentc /info` 给 loader，`bcdedit /enum` 给设备选项对象。
///
/// 这两个 GUID **每次都要现读**：`reagentc /disable`+`/enable` 会删掉并重建 WinRE 条目
/// （实测 C: 的条目整个消失、换成新 GUID），硬编码必然失配。
#[cfg(windows)]
fn registered_winre_templates(log: &Path) -> Result<(String, String), TaskError> {
    let output = Command::new("reagentc.exe")
        .args(["/info"])
        .stdin(Stdio::null())
        .creation_flags(crate::CREATE_NO_WINDOW)
        .output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    crate::append_log(
        log,
        &format!("new boot channel: reagentc /info -> {}", text.trim()),
    )?;
    let loader = crate::text_parsing::parse_guid(&text).ok_or_else(|| {
        crate::err("reagentc /info did not report a WinRE BCD identifier; is WinRE enabled?")
    })?;
    let enum_text = bcd(&["/enum", &loader, "/v"])?;
    let devopts =
        crate::text_parsing::ramdisk_device_options_guid(&enum_text).ok_or_else(|| {
            crate::err("registered WinRE entry does not reference a device options object")
        })?;
    Ok((loader, devopts))
}

/// 备选模板：枚举所有 ramdisk osloader，挑第一个「设备选项对象确实存在」的。
///
/// 只用「当前注册的那一个」不够稳：`reagentc /disable`+`/enable` 会删掉并重建 WinRE 条目，
/// 实测出现过「`/info` 还报着旧标识符、但那个条目连同它的设备选项对象已经不存在」
/// 的情况（拿它当模板 → `/set device` 报「指定的设备无效」）。
#[cfg(windows)]
fn any_usable_winre_template(log: &Path) -> Result<(String, String), TaskError> {
    let text = bcd(&["/enum", "osloader", "/v"])?;
    let mut candidates: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("device ") || trimmed.starts_with("osdevice ") {
            if let Some(devopts) = crate::text_parsing::ramdisk_device_options_guid(trimmed) {
                // 找到该行所属的 osloader 标识符不好回溯，改为配对收集：先取 loader 再取 devopts。
                let _ = devopts;
            }
        }
    }
    // 简化且可靠：按块切分，每块取「标识符 + device 行的设备选项对象」。
    for block in text.split("\r\n\r\n") {
        let loader = block
            .lines()
            .find(|line| line.trim_start().starts_with("标识符"))
            .and_then(|line| crate::text_parsing::parse_guid(line));
        let devopts = block
            .lines()
            .find(|line| {
                let t = line.trim_start();
                (t.starts_with("device ") || t.starts_with("osdevice ")) && t.contains("ramdisk=")
            })
            .and_then(|line| crate::text_parsing::parse_guid(line));
        if let (Some(loader), Some(devopts)) = (loader, devopts) {
            candidates.push((loader, devopts));
        }
    }
    for (loader, devopts) in candidates {
        if bcd(&["/enum", &devopts, "/v"]).is_ok() && bcd(&["/enum", &loader, "/v"]).is_ok() {
            crate::append_log(
                log,
                &format!("new boot channel: usable WinRE template {loader} / {devopts}"),
            )?;
            return Ok((loader, devopts));
        }
    }
    Err(crate::err(
        "no usable WinRE entry found to use as a boot entry template",
    ))
}

/// 把载荷 WIM 与 boot.sdi 摆到镜像卷的 `BackupRestoreRE\` 下，并校验哈希。
#[cfg(windows)]
pub fn copy_into_staging(
    staged_wim: &Path,
    sdi_source: &Path,
    wim_volume: &VolumeIdentity,
    log: &Path,
) -> Result<(String, String), TaskError> {
    let letter = ensure_volume_mounted(wim_volume, 'R', log)?;
    let dir = PathBuf::from(format!(r"{letter}:\{RE_STAGING_DIR}"));
    fs::create_dir_all(&dir)?;
    let wim_path = dir.join("Winre.wim");
    let sdi_path = dir.join("boot.sdi");
    fs::copy(staged_wim, &wim_path)?;
    if sdi_source.is_file() {
        fs::copy(sdi_source, &sdi_path)?;
    } else {
        // 没有 boot.sdi 也能启动（实测 C: 的注册目录就没有它，WinRE 靠 winload 兜住）；
        // 缺它时设备选项里就不指 SDI，避免 bootmgr 找不到文件。
        crate::append_log(
            log,
            "new boot channel: source volume has no boot.sdi; SDI element left unset",
        )?;
    }
    let sha = backuprestore_core::sha256_file(&wim_path)?;
    let absolute = wim_path.to_string_lossy().into_owned();
    crate::append_log(
        log,
        &format!("new boot channel: payload staged at {absolute} sha256={sha}"),
    )?;
    Ok((absolute, sha))
}

/// 只建条目，**不武装**。返回时 `boot-entry.json` 已落盘、BCD 对象已就位，
/// 但 `bootsequence` 尚未设置——载荷 WIM 还没注入完成，不能让人这时重启进来。
///
/// **v1.7.14 起武装由调用方显式决定**（见 [`arm_one_shot`]）。此前端条目里就
/// 无条件 `/bootsequence`，于是 `prepare --no-reboot`（本意「只准备、别动启动状态」）
/// 也会改 bootmgr 的 bootsequence，下一次重启就直接进 PE 了。2026-09-29 实机抓到：
/// BCD 回显 `bootsequence {c0c8debb-…}` 正是当次 `--no-reboot` 新建的条目。
///
/// 任何一步失败都不留半成品（尽力回删已建对象）。
#[cfg(windows)]
pub fn create_entry(
    staged_wim: &Path,
    sdi_source: &Path,
    wim_volume: &VolumeIdentity,
    task_dir: &Path,
    log: &Path,
) -> Result<ReBootEntry, TaskError> {
    if let Some(existing) = ReBootEntry::read(task_dir)? {
        crate::append_log(
            log,
            "new boot channel: entry already recorded; reusing it instead of creating",
        )?;
        return Ok(existing);
    }

    let (wim_path, wim_sha256) = copy_into_staging(staged_wim, sdi_source, wim_volume, log)?;
    // 模板优先取「当前注册的 WinRE 条目」；它不可用（条目/设备选项对象已被 reagentc 删掉）
    // 时退回枚举所有 WinRE 条目，挑一个设备选项对象确实存在的。
    let (template_loader, template_devopts) = match registered_winre_templates(log) {
        Ok(templates) => templates,
        Err(error) => {
            crate::append_log(
                log,
                &format!(
                    "new boot channel: registered entry unusable as template ({error}); scanning all entries"
                ),
            )?;
            any_usable_winre_template(log)?
        }
    };

    // 1) 设备选项对象：copy 模板（/create /device 造的对象不能作 ramdisk= 目标）
    let devopts_raw = bcd(&[
        "/copy",
        &template_devopts,
        "/d",
        &format!("{ENTRY_DESCRIPTION} device options"),
    ])?;
    let devopts = crate::text_parsing::require_guid(
        &crate::text_parsing::parse_guid(&devopts_raw).unwrap_or_default(),
        "create device options",
    )
    .map_err(|message| crate::err(&message))?;
    crate::append_log(log, &format!("new boot channel: device options {devopts}"))?;
    // 2) osloader：copy 注册条目（/create /application OSLOADER 吃不下 ramdisk device）
    let loader_raw = bcd(&["/copy", &template_loader, "/d", ENTRY_DESCRIPTION])?;
    let loader = crate::text_parsing::require_guid(
        &crate::text_parsing::parse_guid(&loader_raw).unwrap_or_default(),
        "create loader",
    )
    .map_err(|message| crate::err(&message))?;
    crate::append_log(log, &format!("new boot channel: loader {loader}"))?;

    // 建后即验：两个对象都必须立即可枚举，否则后面的 /set 会落空。
    bcd_object_exists(&devopts, log)?;
    bcd_object_exists(&loader, log)?;
    let finish = || -> Result<(), TaskError> {
        let letter = ensure_volume_mounted(wim_volume, 'R', log)?;
        let spec = crate::text_parsing::ramdisk_spec(&wim_path, &devopts);
        bcd_write(
            &[
                "/set",
                &devopts,
                "ramdisksdidevice",
                &format!("partition={letter}:"),
            ],
            log,
        )?;
        let sdi = crate::text_parsing::staging_sdi_path(RE_STAGING_DIR);
        bcd_write(&["/set", &devopts, "ramdisksdipath", &sdi], log)?;
        if bcd_field(&devopts, "ramdisksdipath")? != sdi {
            return Err(crate::err("device options ramdisksdipath did not stick"));
        }
        bcd_write(&["/set", &loader, "device", &spec], log)?;
        bcd_write(&["/set", &loader, "osdevice", &spec], log)?;
        bcd_write(&["/set", &loader, "description", ENTRY_DESCRIPTION], log)?;
        Ok(())
    };

    // 整个「改字段」块带退避重试，纯粹为了扛住偶发的 BCD 存储占用。
    // 注意：这里的重试**不是**用来掩盖参数写错的——`ramdisk=` 值畸形时重试一万次也一样
    // 报「指定的设备无效」，2026-09-29 就是这么被误导了一整轮。
    let mut last_error: Option<TaskError> = None;
    for (attempt, delay_secs) in [1_u64, 2, 3, 4].into_iter().zip([2_u64, 6, 15, 30]) {
        if attempt > 1 {
            crate::append_log(
                log,
                &format!(
                    "new boot channel: entry field write failed; retry {attempt} after {delay_secs}s"
                ),
            )?;
            std::thread::sleep(std::time::Duration::from_secs(delay_secs));
        }
        match finish() {
            Ok(()) => {
                last_error = None;
                break;
            }
            Err(error) => {
                crate::append_log(
                    log,
                    &format!("new boot channel: attempt {attempt}: {error}"),
                )?;
                last_error = Some(error);
            }
        }
    }
    if let Some(error) = last_error {
        // 诊断：保留对象并记录 GUID，方便用别的进程上下文重试同一条命令
        crate::append_log(
            log,
            &format!("new boot channel: KEEP loader={loader} devopts={devopts}"),
        )?;
        return Err(error);
    }

    // 3) 回读校验：device/osdevice 必须正好等于我们的意图，否则不武装
    let expected = crate::text_parsing::ramdisk_spec(&wim_path, &devopts);
    let actual_device = bcd_field(&loader, "device")?;
    let actual_osdevice = bcd_field(&loader, "osdevice")?;
    crate::append_log(
        log,
        &format!("new boot channel: readback device={actual_device} osdevice={actual_osdevice}"),
    )?;
    if actual_device != expected || actual_osdevice != expected {
        let _ = bcd(&["/delete", &loader, "/f"]);
        let _ = bcd(&["/delete", &devopts, "/f"]);
        return Err(crate::err(&format!(
            "new boot entry readback mismatch: expected {expected}, got {actual_device} / {actual_osdevice}"
        )));
    }
    if bcd_field(&loader, "winpe")?.is_empty() {
        let _ = bcd(&["/delete", &loader, "/f"]);
        let _ = bcd(&["/delete", &devopts, "/f"]);
        return Err(crate::err("new boot entry is not marked winpe=yes"));
    }

    let entry = ReBootEntry {
        loader_guid: loader,
        devopts_guid: devopts,
        wim_volume: wim_volume.clone(),
        wim_path,
        wim_sha256,
        created: chrono::Utc::now().to_rfc3339(),
    };
    entry.write(task_dir)?;
    Ok(entry)
}

/// 武装一次性启动：只动 bootmgr 的 `bootsequence`，指向我们的 osloader。
///
/// **这里是人「可以重启进任务环境」的唯一开关**，所以必须由调用方在载荷注入、
/// 拷贝、簿记全部完成之后显式调用；`prepare --no-reboot` 时不该调它。
/// 校验失败即回删条目——绝不留一个「指向我们条目、却没真的挂上 bootmgr」的状态。
#[cfg(windows)]
pub fn arm_one_shot(entry: &ReBootEntry, log: &Path) -> Result<(), TaskError> {
    let loader = crate::text_parsing::require_guid(&entry.loader_guid, "arm loader")
        .map_err(|message| crate::err(&message))?;
    let devopts = crate::text_parsing::require_guid(&entry.devopts_guid, "arm device options")
        .map_err(|message| crate::err(&message))?;
    bcd(&["/bootsequence", &loader])?;
    let armed = bcd_field(BOOTMGR, "bootsequence")?;
    if armed != loader {
        let _ = bcd(&["/delete", &loader, "/f"]);
        let _ = bcd(&["/delete", &devopts, "/f"]);
        return Err(crate::err(&format!(
            "bootsequence is {armed}, expected {loader}; entry removed"
        )));
    }
    crate::append_log(
        log,
        &format!("new boot channel: one-shot bootsequence armed at {loader}"),
    )
}

/// 把簿记里的载荷哈希刷新成**镜像卷上那份 WIM 的当前实际值**。
///
/// 为什么需要：`create_entry` 按设计在 DISM 注入**之前**运行
/// （v1.7.11 顺序要点——建条目只需要"镜像卷上有一个合法 WIM"，干净的注册
/// WIM 副本就够），所以它记下的是**注入前**干净原件的哈希。注入之后载荷
/// WIM 被覆写成另一份，哈希随之改变，而薄记没跟着变。
///
/// 后果是致命的：断电续跑走 `rearm()`，它拿活载荷哈希比对这份过期记录，
/// 必然不符，于是以 "payload WIM hash differs from the prepared one;
/// refusing to re-arm" 放弃续跑，机器永久停在 75%
/// （2026-09-30 实机抓到，任务 61a4fa45，stage 卡在 image-applied）。
///
/// 幂等：哈希没变就什么都不做，不重复写文件。
pub(crate) fn refresh_payload_hash(
    entry: &mut ReBootEntry,
    task_dir: &Path,
    log: &Path,
) -> Result<(), TaskError> {
    let live = backuprestore_core::sha256_file(Path::new(&entry.wim_path))?;
    if live.eq_ignore_ascii_case(&entry.wim_sha256) {
        return Ok(());
    }
    crate::append_log(
        log,
        &format!(
            "new boot channel: payload hash is now {live} (recorded {}); refreshing the boot entry",
            entry.wim_sha256
        ),
    )?;
    entry.wim_sha256 = live;
    entry.write(task_dir)
}

/// 重武装：用于断电续跑（任务 resume 时重新把我们的条目设为一次性启动）。
///
/// 前置校验：条目仍存在、载荷 WIM 仍在且哈希与准备期一致。任一条不满足都**拒绝续跑**
/// —— 带着一个指向不存在/被改过的 WIM 的启动项重武装，只会让机器进不了任务环境。
#[cfg(windows)]
pub fn rearm(entry: &ReBootEntry, log: &Path) -> Result<(), TaskError> {
    let loader = crate::text_parsing::require_guid(&entry.loader_guid, "rearm loader")
        .map_err(|message| crate::err(&message))?;
    let devopts = crate::text_parsing::require_guid(&entry.devopts_guid, "rearm device options")
        .map_err(|message| crate::err(&message))?;
    let expected = crate::text_parsing::ramdisk_spec(&entry.wim_path, &devopts);
    let device = bcd_field(&loader, "device")?;
    if device != expected {
        return Err(crate::err(&format!(
            "recorded boot entry no longer points at our payload: {device}"
        )));
    }
    let wim = PathBuf::from(&entry.wim_path);
    if !wim.is_file() {
        return Err(crate::err(&format!(
            "payload WIM is missing at {}; cannot re-arm",
            entry.wim_path
        )));
    }
    let sha = backuprestore_core::sha256_file(&wim)?;
    if !sha.eq_ignore_ascii_case(&entry.wim_sha256) {
        return Err(crate::err(
            "payload WIM hash differs from the prepared one; refusing to re-arm",
        ));
    }
    bcd(&["/bootsequence", &loader])?;
    let armed = bcd_field(BOOTMGR, "bootsequence")?;
    if armed != loader {
        return Err(crate::err(&format!(
            "re-arm failed: bootsequence is {armed}, expected {loader}"
        )));
    }
    crate::append_log(
        log,
        "new boot channel: one-shot bootsequence re-armed for resumption",
    )?;
    Ok(())
}

/// 终态清理：清掉我们的一次性启动、删掉我们建的两个 BCD 对象、删掉镜像卷上的载荷 WIM。
///
/// 「先清 bootsequence 再删对象」顺序很重要：反过来的话，若中途失败，
/// 机器会带着一个指向不存在对象的一次性启动重启。
#[cfg(windows)]
pub fn disarm(entry: &ReBootEntry, log: &Path) -> Result<(), TaskError> {
    let loader = crate::text_parsing::require_guid(&entry.loader_guid, "disarm loader")
        .map_err(|message| crate::err(&message))?;
    let devopts = crate::text_parsing::require_guid(&entry.devopts_guid, "disarm device options")
        .map_err(|message| crate::err(&message))?;
    let armed = bcd_field(BOOTMGR, "bootsequence")?;
    if armed == loader {
        bcd(&["/deletevalue", BOOTMGR, "bootsequence"])?;
        crate::append_log(log, "new boot channel: one-shot bootsequence cleared")?;
    } else if !armed.is_empty() {
        crate::append_log(
            log,
            &format!("new boot channel: bootsequence belongs to {armed}; left untouched"),
        )?;
    }
    for guid in [&loader, &devopts] {
        if let Err(error) = bcd(&["/delete", guid, "/f"]) {
            // 已删除的对象再次清理可能报错；是否成功以枚举回读为准。
            crate::append_log(log, &format!("new boot channel: delete {guid}: {error}"))?;
        }
    }
    let remaining = bcd(&["/enum", "all", "/v"])?.to_ascii_lowercase();
    if remaining.contains(&loader.to_ascii_lowercase())
        || remaining.contains(&devopts.to_ascii_lowercase())
    {
        return Err(crate::err("RE BCD objects still exist after cleanup"));
    }
    crate::append_log(log, "new boot channel: BCD objects removed and verified")?;

    // 启动簿记仍保存桌面盘符，不能传给优先信任缓存盘符的 ensure_volume_mounted。
    // 先用卷 GUID 路径核验磁盘/分区身份，再仅删除此卷上的项目载荷。
    let root = crate::boot_cleanup::staging_volume_root(&entry.wim_volume)?;
    let actual = crate::windows_prepare::volume_identity_at_path(
        &root.to_string_lossy(),
        entry.wim_volume.volume_guid.clone(),
    )?;
    if !actual.same_partition(&entry.wim_volume)
        || actual.partition_offset != entry.wim_volume.partition_offset
        || actual.partition_size != entry.wim_volume.partition_size
    {
        return Err(crate::err(
            "RE staging volume identity mismatch; refusing cleanup",
        ));
    }
    crate::append_log(
        log,
        &format!(
            "new boot channel: cleanup volume={} recorded_letter={:?}",
            root.display(),
            entry.wim_volume.drive_letter
        ),
    )?;
    let dir = crate::boot_cleanup::remove_staging(&root)?;
    crate::append_log(
        log,
        &format!(
            "new boot channel: staging directory {} removed and verified",
            dir.display()
        ),
    )?;
    Ok(())
}
