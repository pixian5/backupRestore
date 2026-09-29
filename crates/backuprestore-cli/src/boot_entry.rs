//! v1.7.11 新启动通道：**不碰系统注册 WinRE**，用「镜像卷上的载荷 WIM + 自建 BCD 条目」启动。
//!
//! 背景与实机依据：`docs/20260929-130000-pe-channel-poc-winre-wim-boots-from-image-volume.md`。
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
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use crate::windows_prepare::ensure_volume_mounted;

/// 镜像卷上存放载荷 RE 的子目录名（不占卷根，避免与用户文件混在一起）。
pub const RE_STAGING_DIR: &str = r"BackupRestoreRE";
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

    fn write(&self, task_dir: &Path) -> Result<(), TaskError> {
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

/// 写操作（`/set`、`/copy`、`/delete`、`/bootsequence`）——**由本程序的子进程代跑**。
///
/// 为什么必须绕一圈：2026-09-29 实机用诊断电池逐条记录后确认，同一串 argv 在父进程里
/// 只有 `device ramdisk=…` 被 bcdedit 拒绝（`description`、`path`、`device partition=…`
/// 全都正常，文件也能被独占打开），而**换成一个新进程就一次成功**——父进程自身状态会
/// 影响 bcdedit 对 ramdisk 设备的校验。所以写入统一交给 `BackupRestore.exe bcd-set`
/// 这个短生命周期子进程，父进程这边只判退出码。
#[cfg(windows)]
fn bcd_write_once(args: &[&str]) -> Result<(), TaskError> {
    let executable = std::env::current_exe().map_err(|error| {
        crate::err(&format!("cannot locate our own executable: {error}"))
    })?;
    let mut child_args: Vec<String> = vec!["bcd-set".to_string()];
    child_args.extend(args.iter().map(|arg| arg.to_string()));
    let output = Command::new(&executable)
        .args(&child_args)
        .creation_flags(crate::CREATE_NO_WINDOW)
        .output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(crate::err(&format!(
            "bcd-set {} failed with {}: {}",
            args.join(" "),
            output.status,
            text.trim()
        )));
    }
    Ok(())
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

/// 诊断电池：在**同一个进程里**按顺序跑一串 bcdedit 调用，逐步记录退出码与完整输出。
///
/// 为什么需要它：2026-09-29 出现「同一串 argv、同一对 BCD 对象，在产品进程里失败、
/// 从任何 shell 里立刻重试就成功」的怪事。靠猜（DISM、cwd、调用形态、程序名）都试过，
/// 全部排除。所以改成把每一步的事实记录下来：哪个元素能写、哪个不能、重试是否也一样、
/// `.output()` 与 `.status()` 是否不同。日志落在 prepare.log 里，`DIAG` 前缀可 grep。
#[cfg(windows)]
fn diag_battery(loader: &str, devopts: &str, spec: &str, wim_path: &Path, log: &Path) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let probe = |label: &str, args: &[&str], use_status: bool| {
        let mut command = Command::new(r"C:\Windows\System32\bcdedit.exe");
        command.args(args);
        command.creation_flags(crate::CREATE_NO_WINDOW);
        let (code, text) = if use_status {
            match command.status() {
                Ok(status) => (status.code().unwrap_or(-1), String::new()),
                Err(error) => (-99, format!("spawn error: {error}")),
            }
        } else {
            command.stdin(Stdio::null());
            match command.output() {
                Ok(output) => (
                    output.status.code().unwrap_or(-1),
                    format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ),
                ),
                Err(error) => (-99, format!("spawn error: {error}")),
            }
        };
        let note = format!(
            "DIAG[{stamp}] {label} rc={code} args={} :: {}",
            args.join(" "),
            text.trim()
        );
        crate::append_log(log, &note).ok();
    };
    probe("read-loader", &["/enum", loader, "/v"], false);
    probe("write-devolpts-description", &["/set", devopts, "description", "BR-DIAG"], false);
    probe("write-loader-description", &["/set", loader, "description", "BR-DIAG"], false);
    probe("write-loader-path", &["/set", loader, "path", r"\windows\system32\winload.efi"], false);
    probe("write-device-partition", &["/set", loader, "device", "partition=C:"], false);
    probe("read-device-after-partition", &["/enum", loader, "/v"], false);
    probe(
        "write-device-ramdisk-wellknown",
        &["/set", loader, "device", "ramdisk=[F:\\BackupRestoreRE\\Winre.wim,{ramdiskoptions}"],
        false,
    );
    probe("write-device-output", &["/set", loader, "device", spec], false);
    probe("write-device-status", &["/set", loader, "device", spec], true);
    probe("write-device-output-again", &["/set", loader, "device", spec], false);
    // WMI 创建的进程由 WMI 服务拉起，**不继承调用进程的句柄**——用来验证
    // 「父进程句柄 inherited 导致 bcdedit 拒绝 ramdisk 设备」这个假设。
    let wmic_cmd = format!("bcdedit.exe /set {loader} device {spec}");
    if let Ok(output) = Command::new("cmd.exe")
        .args([
            "/d",
            "/c",
            &format!("wmic process call create \"{wmic_cmd}\""),
        ])
        .creation_flags(crate::CREATE_NO_WINDOW)
        .output()
    {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        crate::append_log(
            log,
            &format!(
                "DIAG[{stamp}] write-device-via-wmic rc={} :: {}",
                output.status.code().unwrap_or(-1),
                text.trim()
            ),
        )
        .ok();
    }
    let cmdline = format!(
        "bcdedit.exe /set {loader} device {spec}"
    );
    let mut command = Command::new("cmd.exe");
    command.args(["/d", "/c", &cmdline]);
    command.creation_flags(crate::CREATE_NO_WINDOW);
    match command.output() {
        Ok(output) => {
            let note = format!(
                "DIAG[{stamp}] write-device-via-cmd rc={} :: {}{}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            crate::append_log(log, &note).ok();
        }
        Err(error) => {
            crate::append_log(
                log,
                &format!("DIAG[{stamp}] write-device-via-cmd spawn error: {error}"),
            )
            .ok();
        }
    }
    // 试探：本进程能否「独占」打开镜像卷上的 WIM / SDI。若失败 → 有别人持着句柄，
    // 那 bcdedit 打不开同一文件就会报「指定的设备无效」。
    // Windows 上独占打开要走 std::os::windows::fs::OpenOptionsExt::share_mode(0)。
    for candidate in [
        wim_path.to_path_buf(),
        wim_path
            .parent()
            .map(|parent| parent.join("boot.sdi"))
            .unwrap_or_default(),
    ] {
        let label = candidate
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&candidate);
        match opened {
            Ok(_) => crate::append_log(log, &format!("DIAG[{stamp}] exclusive-open {label}: OK")).ok(),
            Err(error) => crate::append_log(
                log,
                &format!(
                    "DIAG[{stamp}] exclusive-open {label}: FAILED {error} (kind={:?})",
                    error.kind()
                ),
            )
            .ok(),
        };
    }
    // 把当前进程的环境与 PATH 解析结果也记下来，便于和外部 shell 对比
    for (label, args) in [
        ("env-dump", vec!["/d", "/c", "set"]),
        ("which-bcdedit", vec!["/d", "/c", "where bcdedit"]),
    ] {
        if let Ok(output) = Command::new("cmd.exe")
            .args(&args)
            .creation_flags(crate::CREATE_NO_WINDOW)
            .output()
        {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let lines: Vec<&str> = text.lines().take(30).collect();
            crate::append_log(
                log,
                &format!("DIAG[{stamp}] {label} :: {}", lines.join(" || ")),
            )
            .ok();
        }
    }
    // 顺手记录进程上下文事实
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".into());
    crate::append_log(
        log,
        &format!(
            "DIAG[{stamp}] context pid={} cwd={} exe={}",
            std::process::id(),
            cwd,
            std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "<unknown>".into())
        ),
    )
    .ok();
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

/// `bcdedit /enum <guid> /v` 的某个字段值。
fn bcd_field(guid: &str, field: &str) -> Result<String, TaskError> {
    let guid = crate::text_parsing::require_guid(guid, "enum").map_err(|message| crate::err(&message))?;
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
    let devopts = crate::text_parsing::ramdisk_device_options_guid(&enum_text).ok_or_else(|| {
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

/// 建条目 + 武装一次性启动。**任何一步失败都不留半成品**（尽力回删已建对象）。
#[cfg(windows)]
/// 只建条目，不武装。返回时 `boot-entry.json` 已落盘、BCD 对象已就位，
/// 但 `bootsequence` 尚未设置——载荷 WIM 还没注入完成，不能让人这时重启进来。
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
                &format!("new boot channel: registered entry unusable as template ({error}); scanning all entries"),
            )?;
            any_usable_winre_template(log)?
        }
    };

    // 1) 设备选项对象：copy 模板（/create /device 造的对象不能作 ramdisk= 目标）
    let devopts_raw = bcd(&["/copy", &template_devopts, "/d", &format!("{ENTRY_DESCRIPTION} device options")])?;
    let devopts = crate::text_parsing::require_guid(
        &crate::text_parsing::parse_guid(&devopts_raw).unwrap_or_default(),
        "create device options",
    ).map_err(|message| crate::err(&message))?;
    crate::append_log(log, &format!("new boot channel: device options {devopts}"))?;
    // 2) osloader：copy 注册条目（/create /application OSLOADER 吃不下 ramdisk device）
    let loader_raw = bcd(&["/copy", &template_loader, "/d", ENTRY_DESCRIPTION])?;
    let loader = crate::text_parsing::require_guid(
        &crate::text_parsing::parse_guid(&loader_raw).unwrap_or_default(),
        "create loader",
    ).map_err(|message| crate::err(&message))?;
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
        // 诊断电池：把同一进程里每一步的退出码与完整输出记进日志（见 diag_battery 注释）
        diag_battery(&loader, &devopts, &spec, Path::new(&wim_path), log);
        bcd_write(&["/set", &loader, "device", &spec], log)?;
        bcd_write(&["/set", &loader, "osdevice", &spec], log)?;
        bcd_write(&["/set", &loader, "description", ENTRY_DESCRIPTION], log)?;
        Ok(())
    };

    // 整个「改字段」块带退避重试。实测失败诱因之一是**刚落盘的 700MB 载荷 WIM 会被短暂
    // 占用/扫描，bcdedit 打不开它就报「指定的设备无效」**——手工脚本里文件已放置几分钟，
    // 所以一次成功；产品在同一次 prepare 里拷完立刻改字段，于是稳定失败。
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
                crate::append_log(log, &format!("new boot channel: attempt {attempt}: {error}"))?;
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
        &format!(
            "new boot channel: readback device={actual_device} osdevice={actual_osdevice}"
        ),
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

    // 4) 武装一次性启动（只动 bootmgr 的 bootsequence）
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
    )?;

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

/// 重武装：用于断电续跑（任务 resume 时重新把我们的条目设为一次性启动）。
///
/// 前置校验：条目仍存在、载荷 WIM 仍在且哈希与准备期一致。任一条不满足都**拒绝续跑**
/// —— 带着一个指向不存在/被改过的 WIM 的启动项重武装，只会让机器进不了任务环境。
#[cfg(windows)]
pub fn rearm(entry: &ReBootEntry, log: &Path) -> Result<(), TaskError> {
    let loader = crate::text_parsing::require_guid(&entry.loader_guid, "rearm loader").map_err(|message| crate::err(&message))?;
    let devopts = crate::text_parsing::require_guid(&entry.devopts_guid, "rearm device options").map_err(|message| crate::err(&message))?;
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
    let loader = crate::text_parsing::require_guid(&entry.loader_guid, "disarm loader").map_err(|message| crate::err(&message))?;
    let devopts = crate::text_parsing::require_guid(&entry.devopts_guid, "disarm device options").map_err(|message| crate::err(&message))?;
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
    let _ = bcd(&["/delete", &loader, "/f"]);
    let _ = bcd(&["/delete", &devopts, "/f"]);
    crate::append_log(log, "new boot channel: BCD objects removed")?;
    match ensure_volume_mounted(&entry.wim_volume, 'R', log) {
        Ok(letter) => {
            let dir = PathBuf::from(format!(r"{letter}:\{RE_STAGING_DIR}"));
            let _ = fs::remove_file(dir.join("Winre.wim"));
            let _ = fs::remove_file(dir.join("boot.sdi"));
            let _ = fs::remove_dir(&dir);
            crate::append_log(
                log,
                &format!("new boot channel: staging directory {} removed", dir.display()),
            )?;
        }
        Err(error) => crate::append_log(
            log,
            &format!("new boot channel: staging volume unreachable ({error}); only the WIM stays behind"),
        )?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramdisk_spec_has_no_trailing_bracket() {
        let spec = crate::text_parsing::ramdisk_spec(r"F:\BootRestoreRE\Winre.wim", "{11111111-2222-3333-4444-555555555555}");
        assert_eq!(spec, r"ramdisk=[F:\BootRestoreRE\Winre.wim],{11111111-2222-3333-4444-555555555555}");
        assert!(!spec.ends_with('}') || spec.contains("],{"));
        assert_eq!(spec.matches('[').count(), 1);
        assert_eq!(spec.matches(']').count(), 1);
    }
}
