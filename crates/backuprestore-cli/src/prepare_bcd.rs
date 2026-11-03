// Windows 专用模块——非 Windows 编译时仅作类型检查，调用方被 cfg 门控。
#![cfg_attr(not(windows), allow(dead_code))]
//! 准备期 BCD 快照与引导回滚补偿器（Prepare BCD Snapshot & Compensation）
//!
//! 负责：
//! - ESP 分区与免盘符 BCD store verbatim 路径定位；
//! - 多 ESP 候选活动引导指纹比对与引导 ESP 自动鉴别；
//! - 准备前 BCD 字节级 raw 快照与逻辑导出；
//! - 准备阶段各关键步骤（创建条目/载荷注入/一次性启动武装/缓存刷写/关机请求）失败时的统一回滚补偿；
//! - 恢复环境（WinRE/PE）一键 BCD 快照回退。

use std::fs;
use std::path::{Path, PathBuf};

use backuprestore_core::{sha256_file, verify_sha256, TaskError, VolumeIdentity};
use crate::error_code::ErrCode;
use crate::volume_device::volume_identity_at_path;
use crate::windows_prepare::{ensure_volume_mounted_leased, MountLease, EFI_TYPE};
use crate::{append_log, capture_logged, err, run_logged};

/// ESP 上 BCD 存储的路径，**优先零盘符**。
///
/// 方案 A（v1.8.2）的核心入口。返回 `(路径, 本次是否为此挂了盘符)`：
/// 第二个元素是 `Some(letter)` 时，调用方用完必须把盘符卸掉，
/// 否则「临时盘符」会变成新的污染源——那正是 S 盘反复出现的原因。
///
/// 为什么能零盘符：verbatim 卷路径下 `CreateFileW` / `CopyFileExW` /
/// `bcdedit` / `dism` 全都接受。2026-09-29 实机验证（`docs/20260929-2014-S盘自动打开问题定位与修复方案.md` 第六·补节）：
///   - 读：两条路径 copy 出的 BCD 副本 SHA-256 相同
///   - 写：`set {bootmgr} default` 后 `/enum` 输出 `fc /b` 逐字节无差异
///   - 写：`/set` `/create` `/delete` 三个动词均成功
///   - 枚举：卷路径可直接列出 ESP 全部文件
///
/// 卷 GUID 缺失（导入的旧任务、DiskPart 探测失败）时才退回挂盘符。
pub(crate) fn efi_bcd_store_path(
    efi: &VolumeIdentity,
    preferred: char,
    log: &Path,
) -> Result<(PathBuf, Option<MountLease>), TaskError> {
    if let Some(volume) = efi.volume_path() {
        let mut path = PathBuf::from(volume);
        path.push("EFI");
        path.push("Microsoft");
        path.push("Boot");
        path.push("BCD");
        append_log(
            log,
            &format!("plan A: verbatim BCD store path = {}", path.display()),
        )?;
        return Ok((path, None));
    }
    let (letter, lease) = ensure_volume_mounted_leased(efi, preferred, log)?;
    append_log(
        log,
        &format!("plan A: volume GUID absent; fell back to drive letter {letter}:"),
    )?;
    Ok((
        PathBuf::from(format!(r"{letter}:\EFI\Microsoft\Boot\BCD")),
        Some(lease),
    ))
}

/// 镜像完整性预检（2.4.2 引入，2.4.3 恢复）：还原类操作要求的"完整 Windows"标记探测。
///
/// 原生枚举包括已挂载卷；验证 EFI 类型及 BCD，多个候选或探测错误均拒绝猜测。
pub(crate) fn esp_identity_without_drive_letter() -> Result<VolumeIdentity, TaskError> {
    let mut matches: Vec<VolumeIdentity> = Vec::new();
    let mut last_error: Option<TaskError> = None;
    for volume_path in native_volume_paths()? {
        // verbatim 路径形如 \\?\Volume{GUID}\，卷 GUID 取中段。
        let Some(guid) = volume_path
            .trim()
            .strip_prefix(&crate::text_parsing::esp_volume_prefix())
            .and_then(|rest| rest.strip_suffix('\\'))
        else {
            continue;
        };
        match volume_identity_at_path(&volume_path, guid.to_string()) {
            Ok(identity) => {
                if !identity.partition_type_guid.eq_ignore_ascii_case(EFI_TYPE) {
                    continue;
                }
                // 未知错误不能当成不存在后随意挑另一个 ESP。
                if crate::pe_safety::probe_file_metadata(
                    std::path::Path::new(&volume_path),
                    &["EFI", "Microsoft", "Boot", "BCD"],
                )? {
                    matches.push(identity);
                }
            }
            Err(error) => last_error = Some(error),
        }
    }
    match matches.len() {
        0 => Err(last_error.unwrap_or_else(|| crate::errc(ErrCode::NoBcdEfiPartition))),
        1 => Ok(matches.remove(0)),
        _ => {
            // 多 ESP 自动识别（2.4.2）：不再一律拒绝。判据是**活动 BCD 指纹**——
            // `bcdedit`（不带 /store）枚举的就是本次引导实际使用的 BCD；把每个
            // 候选 ESP 的 {bootmgr} 状态（default/displayorder/resumeobject）与它
            // 比对，**恰好一个**全等即认定该 ESP 是引导 ESP，安全选用。
            // 指纹不匹配的 ESP 属于别的系统/磁盘，写它会把引导改到错误的链上；
            // 指纹全等的候选不止一个（克隆盘）时仍然拒绝猜测——保护不变。
            if let Some(active) = active_esp_by_store_fingerprint(&matches) {
                return Ok(active.identity);
            }
            let list = matches
                .iter()
                .map(|identity| {
                    format!(
                        "disk {} partition {}",
                        identity.disk_number.unwrap_or_default(),
                        identity.partition_number.unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            Err(crate::errc_with(ErrCode::MultipleEspAmbiguous, &[&list]))
        }
    }
}

/// 按活动 BCD 指纹在多个 ESP 候选里挑出引导 ESP。
///
/// 返回 `None` 表示无法唯一确定（活动指纹读不到 / 无匹配 / 多于一个匹配）。
fn active_esp_by_store_fingerprint(candidates: &[VolumeIdentity]) -> Option<ActiveEspPick> {
    let active = boot_manager_fingerprint(None)?;
    let mut hit: Option<ActiveEspPick> = None;
    for identity in candidates {
        // volume_guid 字段存的是完整路径（\\?\Volume{GUID}\），先剥成裸 {GUID} 再拼。
        let Some(guid) = crate::text_parsing::bare_volume_guid(&identity.volume_guid) else {
            continue;
        };
        let store = format!(r"\\?\Volume{guid}\EFI\Microsoft\Boot\BCD");
        let Some(fingerprint) = boot_manager_fingerprint(Some(&store)) else {
            continue;
        };
        if fingerprint != active {
            continue;
        }
        if hit.is_some() {
            // 克隆盘：两个 ESP 的 BCD 内容一致，仍拒绝猜测。
            return None;
        }
        hit = Some(ActiveEspPick {
            identity: identity.clone(),
            fingerprint,
        });
    }
    hit
}

/// 读取一个 BCD store 的 {bootmgr} 指纹；`store=None` 时读活动系统 store。
/// 指纹取 (default, display_order, resume_object) 的小写归一元组——这三个字段
/// 足以区分"本次引导链"与"别的系统的 BCD"，且复用既有解析器
/// （`parse_boot_manager_state` 已有本地化与别名相关单测）。
fn boot_manager_fingerprint(store: Option<&str>) -> Option<(String, Vec<String>, String)> {
    let mut arguments: Vec<String> = Vec::new();
    if let Some(store) = store {
        arguments.push("/store".into());
        arguments.push(store.to_string());
    }
    arguments.extend(["/enum".into(), "{bootmgr}".into(), "/v".into()]);
    let output = run_bcdedit_quiet(&arguments).ok()?;
    let state = crate::parse_boot_manager_state(&output)?;
    Some((
        state.default.to_ascii_lowercase(),
        state
            .display_order
            .iter()
            .map(|id| id.to_ascii_lowercase())
            .collect(),
        state.resume_object.unwrap_or_default().to_ascii_lowercase(),
    ))
}

/// 无日志副作用的 bcdedit 调用（ESP 识别属探测路径，不该污染任务日志）。
fn run_bcdedit_quiet(arguments: &[String]) -> Result<String, TaskError> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("bcdedit.exe")
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(crate::errc_with(
            ErrCode::ProgramFailed,
            &[
                "bcdedit.exe",
                &format!("{}", output.status),
                crate::text_parsing::tail_of_text(&text, crate::text_parsing::LOG_TAIL_MAX_BYTES),
            ],
        ));
    }
    Ok(text)
}

/// `active_esp_by_store_fingerprint` 的命中结果（持有克隆，避免借用纠缠）。
struct ActiveEspPick {
    identity: VolumeIdentity,
    #[allow(dead_code)]
    fingerprint: (String, Vec<String>, String),
}

/// 直接枚举卷 GUID，不解析 mountvol 的本地化提示，不分配盘符。
pub(crate) fn native_volume_paths() -> Result<Vec<String>, TaskError> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FindFirstVolumeW(name: *mut u16, size: u32) -> *mut std::ffi::c_void;
        fn FindNextVolumeW(handle: *mut std::ffi::c_void, name: *mut u16, size: u32) -> i32;
        fn FindVolumeClose(handle: *mut std::ffi::c_void) -> i32;
    }
    let mut buffer = vec![0u16; 1024];
    let handle = unsafe { FindFirstVolumeW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if handle as isize == -1 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut paths = Vec::new();
    loop {
        let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        paths.push(String::from_utf16_lossy(&buffer[..len]));
        if unsafe { FindNextVolumeW(handle, buffer.as_mut_ptr(), buffer.len() as u32) } == 0 {
            let error = std::io::Error::last_os_error();
            unsafe {
                FindVolumeClose(handle);
            }
            if error.raw_os_error() != Some(18) {
                return Err(error.into());
            }
            break;
        }
    }
    Ok(paths)
}

/// 从稳定卷 GUID 回读现场身份；调用方另行检查编号、几何及卷序列号。
pub(crate) fn refresh_identity(expected: &VolumeIdentity) -> Result<VolumeIdentity, TaskError> {
    let root = expected
        .volume_path()
        .ok_or_else(|| crate::errc(ErrCode::StableGuidMissing))?;
    let mut actual = volume_identity_at_path(&root, expected.volume_guid.clone())?;
    actual.drive_letter = expected.drive_letter;
    Ok(actual)
}

pub(crate) fn snapshot_raw_bcd(
    efi: &VolumeIdentity,
    task_dir: &Path,
    log: &Path,
) -> Result<String, TaskError> {
    // 方案 A（v1.8.2）：优先 verbatim 卷路径，不分配盘符。
    // 真挂了盘符时租约活到本函数结束，不再需要手写 `mountvol /D` 收尾。
    let (source, _lease_efi) = efi_bcd_store_path(efi, 'S', log)?;
    let snapshot = task_dir.join("bcd-before-raw");
    let result = (|| {
        if !source.is_file() {
            return Err(err("EFI BCD store is missing while creating raw snapshot"));
        }
        let source_arg = source.to_string_lossy().into_owned();
        let boot_manager = capture_logged(
            "bcdedit.exe",
            &["/store", &source_arg, "/enum", "all", "/v"],
            log,
        )
        .or_else(|_| capture_logged("bcdedit.exe", &["/enum", "all", "/v"], log))?;
        fs::write(task_dir.join("bcd-before-bootmgr.txt"), boot_manager)?;
        match fs::copy(&source, &snapshot) {
            Ok(_) => {
                let hash = sha256_file(&snapshot)?;
                verify_sha256(&source, &hash)?;
                append_log(log, "Captured byte-for-byte EFI BCD snapshot")?;
                Ok(hash)
            }
            Err(copy_error) if copy_error.raw_os_error() == Some(32) => {
                let export = task_dir.join("bcd-before-export");
                if !export.is_file() {
                    return Err(copy_error.into());
                }
                let hash = sha256_file(&export)?;
                append_log(
                    log,
                    "Active EFI BCD is locked; using bcdedit logical export for rollback",
                )?;
                Ok(hash)
            }
            Err(error) => Err(error.into()),
        }
    })();
    result
}

/// 准备失败的**统一补偿**：撤销本任务已产生的全部副作用。
///
/// 1. **先清一次性启动请求**：反向保证机器不会重启进任务 RE；
/// 2. **删 BCD 对象**：只删本任务记录在案的那两个 GUID，绝不用描述字符串匹配；
/// 3. **删镜像卷上的载荷**：按任务 ID 定位，只删本任务那一份；
/// 4. **恢复 BCD 快照**：`create_entry` 之前导出过字节级快照，此时才回到基线。
pub(crate) fn compensate_prepare_failure(
    task_dir: &Path,
    efi: &VolumeIdentity,
    log: &Path,
) -> Result<(), TaskError> {
    let mut problems: Vec<String> = Vec::new();
    let record = crate::boot_entry::ReBootEntry::read(task_dir)?;
    let residue = crate::boot_record::ResidueRecord::read(task_dir)?;
    if record.is_none() && residue.is_none() {
        append_log(
            log,
            "preparation compensation: no boot entry recorded; nothing to disarm",
        )?;
    }
    // 正式簿记：走完整 disarm（清启动请求 → 删对象 → 删本任务载荷）。
    if let Some(entry) = record.as_ref()
        && let Err(error) = crate::boot_entry::disarm(entry, log)
    {
        problems.push(format!("boot entry disarm: {error}"));
    }
    // 半成品簿记：只删那两个 BCD 对象，不去核验启动链或删载荷。
    if let Some(entry) = residue.as_ref() {
        if let Err(error) =
            crate::boot_entry::remove_entry_objects(&entry.loader_guid, &entry.devopts_guid, log)
        {
            problems.push(format!("residue objects removal: {error}"));
        }
        match crate::boot_record::ResidueRecord::clear(task_dir) {
            Ok(()) => append_log(log, "preparation compensation: residue record cleared")?,
            Err(error) => problems.push(format!("residue record cleanup: {error}")),
        }
    }
    if let Err(error) = rollback_boot_request(task_dir, efi, log) {
        problems.push(format!("BCD snapshot restore: {error}"));
    }
    if problems.is_empty() {
        append_log(
            log,
            "preparation compensation: all task-owned side effects undone and verified",
        )?;
        Ok(())
    } else {
        Err(crate::errc_with(
            ErrCode::PreparationCompensationIncomplete,
            &[&problems.join("; ")],
        ))
    }
}

/// 把 ESP 上的 BCD 恢复成任务准备期保存的快照。
pub(crate) fn rollback_bcd_snapshot(
    task_dir: &Path,
    efi: &VolumeIdentity,
    log: &Path,
) -> Result<(), TaskError> {
    rollback_boot_request(task_dir, efi, log)
}

fn rollback_boot_request(
    task_dir: &Path,
    efi: &VolumeIdentity,
    log: &Path,
) -> Result<(), TaskError> {
    let raw_snapshot = task_dir.join("bcd-before-raw");
    if raw_snapshot.is_file() {
        let (store, _lease_efi) = efi_bcd_store_path(efi, 'S', log)?;
        let result = (|| {
            let expected = sha256_file(&raw_snapshot)?;
            match fs::copy(&raw_snapshot, &store) {
                Ok(_) => {
                    verify_sha256(&store, &expected)?;
                    append_log(log, "Restored byte-for-byte EFI BCD snapshot")?;
                }
                Err(error) if error.raw_os_error() == Some(32) => {
                    let detail = format!(
                        "ESP 上的 BCD 正被系统占用（错误 32，共享冲突），无法按字节回退。\
                         请重启进入恢复环境（WinRE/PE）后重试；在线会话下也可用\
                         「引导修复(M)」手工调整启动项"
                    );
                    append_log(log, &format!("EFI BCD raw restore blocked: {detail}"))?;
                    return Err(err(&detail));
                }
                Err(error) => return Err(error.into()),
            }
            Ok(())
        })();
        return result;
    }
    let snapshot = task_dir.join("bcd-before-export");
    if !snapshot.is_file() {
        return Err(err(
            "BCD snapshot is missing while rolling back preparation",
        ));
    }
    let snapshot_arg = snapshot.to_string_lossy().into_owned();
    run_logged("bcdedit.exe", &["/import", &snapshot_arg], log)?;
    append_log(
        log,
        "Restored logical BCD snapshot after preparation failure",
    )?;
    Ok(())
}
