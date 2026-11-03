//! 环境探测、启动项呈现与任务废弃收口模块 (environment_admin.rs)
//!
//! 承载原本在 `windows_prepare.rs` 中的顶层 CLI 诊断与收口命令：
//! 1. `list_boot_entries`: 枚举 BCD 启动菜单加载器条目并以 JSON 格式输出；
//! 2. `inspect_environment`: 探测操作系统版本、CPU 架构、WinRE 注册状态与驱动器列表；
//! 3. `abandon_task`: 显式放弃或收口一个准备中/挂起/终态任务，解除一次性引导锁定并清理暂存载荷。

#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use backuprestore_core::{TaskError, TaskStore};
use serde_json::json;

use crate::boot_cleanup::{remove_staging, staging_volume_root};
use crate::boot_entry::{disarm, ReBootEntry};
use crate::boot_menu::all_loader_descriptions;
use crate::error_code::ErrCode;
use crate::prepare_bcd::compensate_prepare_failure;
use crate::prepare_plan::windows_version_summary;
use crate::prepare_validation::require_administrator;
use crate::process_runner::capture_output;
use crate::text_parsing::decode_windows_bytes;
use crate::volume_device::{discover_drives, native_architecture, volume_identity_at_path};
use crate::windows_prepare::{efi_identity, recovery_identity_from_bcd};
use crate::{append_log, errc, errc_with};

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// 枚举当前系统 BCD 中的全部启动项描述并输出结构化 JSON。
pub(crate) fn list_boot_entries() -> Result<(), TaskError> {
    let output = Command::new("bcdedit.exe")
        .args(["/enum", "all", "/v"])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let (descriptions, probe_error) = match &output {
        Ok(output) if output.status.success() => (
            all_loader_descriptions(&decode_windows_bytes(&output.stdout)),
            None,
        ),
        Ok(output) => {
            // 退出码非零：把 stderr 尾部一起带出来，让 GUI 能记下真实原因
            //（BCD 存储不可访问 / 权限不足 / 系统无 UEFI 等）。
            let mut text = decode_windows_bytes(&output.stdout);
            text.push_str(&decode_windows_bytes(&output.stderr));
            let tail: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect();
            let start = tail.len().saturating_sub(4);
            (
                Vec::new(),
                Some(format!(
                    "bcdedit /enum exited with {}: {}",
                    output.status,
                    tail[start..].join(" | ")
                )),
            )
        }
        Err(error) => (Vec::new(), Some(format!("bcdedit could not be started: {error}"))),
    };
    let mut report = json!({ "descriptions": descriptions });
    if let Some(error) = probe_error {
        report["error"] = json!(error);
    }
    println!("{}", report);
    Ok(())
}

/// 探测当前运行环境（Windows 版本、架构、WinRE 可用性与驱动器列表）并以 JSON 输出。
pub(crate) fn inspect_environment() -> Result<(), TaskError> {
    let output = capture_output("cmd.exe", &["/d", "/c", "ver"])?;
    // `ver` is localized and may use the active Windows code page.  The
    // status panel is UTF-8 Rust text, so retain only its invariant ASCII
    // version token instead of leaking mojibake such as `本` into Chinese UI.
    let windows = windows_version_summary(&output);
    let architecture = native_architecture();
    // 同 `assert_environment`：`reagentc /info` 有上下文级失败模式（退出码 9），
    // 失败不代表 WinRE 不可用；这里只用于**显示**，失败时退回 BCD 判是否有 WinRE，
    // 绝不因为日志 ACL 问题就让整个「刷新环境」动作失败。
    let winre_available = match capture_output("reagentc.exe", &["/info"]) {
        Ok(reagent) => reagent.contains("GLOBALROOT") || reagent.contains("Recovery\\WindowsRE"),
        Err(_) => recovery_identity_from_bcd().is_ok(),
    };
    println!(
        "{}",
        serde_json::to_string(&json!({
            "windows": windows,
            "architecture": architecture,
            "winreAvailable": winre_available,
            "volumes": discover_drives()?,
        }))?
    );
    Ok(())
}

/// 显式放弃或收口一个任务。
///
/// 两条路径：
/// - **未写入目标卷**（Prepared/BootRequested/RecoveryStarted/Preflight）：走完整准备期补偿
///   （清启动请求 → 删对象 → 删载荷 → 恢复 BCD 快照）。
/// - **已终结**（Failed，2026-10-16 用户拍板新增）：走终态收口——只清本任务的引导对象与载荷、
///   删掉大工作目录，**保留**证据记录。擦除后失败的任务以前既不能续也不能清（永久悬置）。
///
/// 仍拒绝的情形：**非终态且已越过破坏性边界**（TargetErased 之后但未标终态）——那说明恢复链
/// 还在进行中，需要先继续原任务或人工恢复，不能从清理入口直接抹掉。
pub(crate) fn abandon_task(root: String, task_id: String) -> Result<(), TaskError> {
    // 先拿到日志路径再查提权：这样「不是管理员」这件事本身也能落进任务日志，
    // 而不是只留一句弹框文案。
    let log = TaskStore::new(root.clone()).log_path(&task_id)?;
    require_administrator(&log)?;
    let store = TaskStore::new(root);
    let mut task = store.load(&task_id)?;
    let dir = store.task_dir(&task_id)?;
    let status: backuprestore_core::StatusRecord =
        backuprestore_core::read_json(store.status_path(&task_id)?)?;
    if status.task_id != task.task_id
        || status.stage != task.status
        || status.operation != task.operation
    {
        return Err(errc(ErrCode::TaskStateInconsistent));
    }
    // 【终态收口，2026-10-16 用户拍板】已终结（Failed）的任务允许收口：
    // 只清本任务的引导对象与载荷、删掉大工作目录，**保留** task.json / status.json /
    // manifest.json / 日志等证据记录，供人工诊断。
    //
    // 为什么需要这条入口：任务在擦除后失败会停在 TargetErased；恢复环境的自动逃出
    // 现在会把它标成 Failed（见 `main.rs::auto_escape_to_windows`），但在此之前
    // （以及任何人工介入的场景）用户既不能续跑、也不能清理——这正是 2026-10-16
    // 实机暴露的「非终态任务永久悬置」。收口时**必须同时清 bootsequence**
    // （`disarm` 已含，且只认本任务的 GUID），否则等于把「下次启动又进恢复环境」
    // 的锁原样留给下一次启动。
    if task.status == backuprestore_core::Stage::Failed {
        append_log(
            &log,
            &format!(
                "Collecting terminal (Failed) task; operation={:?} — disarm boot objects, \
                 remove payload, keep evidence",
                task.operation
            ),
        )?;
        match ReBootEntry::read(&dir)? {
            Some(entry) => {
                disarm(&entry, &log)?;
                append_log(&log, "terminal cleanup: one-shot boot entry disarmed")?;
            }
            None => append_log(
                &log,
                "terminal cleanup: no boot entry record on disk; nothing to disarm",
            )?,
        }
        store.cleanup_task_artifacts(&task.task_id)?;
        append_log(
            &log,
            "Terminal (Failed) task collected; evidence records retained",
        )?;
        return Ok(());
    }
    if !task.status.can_abandon_before_target_write() {
        return Err(errc_with(
            ErrCode::TaskAlreadyAdvanced,
            &[&format!("{:?}", task.status)],
        ));
    }
    append_log(
        &log,
        &format!(
            "User explicitly abandoned task before target writes; operation={:?} stage={:?}",
            task.operation, task.status
        ),
    )?;
    let efi = efi_identity(None)?;
    // 【2026-10-18 缺陷修复：ef71fccc 三按钮死锁（续）】补偿失败**不再中断收口**。
    // 能走到这里的任务都在破坏性边界之前（目标卷还没动过），补偿只是尽量把准备期
    // 对 BCD 的改动还原。ESP 上的 BCD 被系统占用（错误 32，BFSVC 持锁）时还原不了，
    // 此时中断整个放弃流程 = 用户又被卡住——和刚修好的阶段闸门是同一个死锁，
    // 只是换了个位置复发。改为记 ERROR 后继续：BCD 保持现状（可用「引导修复(M)」
    // 或 bcdedit 手工处理），任务照样标终态、一次性启动项照样撤销。
    if let Err(cleanup) = compensate_prepare_failure(&dir, &efi, &log) {
        append_log(
            &log,
            &format!(
                "[ERROR] abandon: BCD compensation incomplete ({cleanup}); continuing to close the task"
            ),
        )?;
    }
    // create_entry can copy the WIM to staging before its final boot-entry
    // record is durable. Re-check by stable volume identity and remove only
    // this task's directory, even if no ReBootEntry record survived.
    let staging_volume = task.image.as_ref().map(|image| &image.volume).or_else(|| {
        task.destination
            .as_ref()
            .map(|destination| &destination.volume)
    });
    if let Some(volume) = staging_volume {
        let root_path = staging_volume_root(volume)?;
        let actual =
            volume_identity_at_path(&root_path.to_string_lossy(), volume.volume_guid.clone())?;
        if !actual.same_partition(volume)
            || actual.partition_offset != volume.partition_offset
            || actual.partition_size != volume.partition_size
        {
            return Err(errc(ErrCode::AbandonPayloadVolumeMismatch));
        }
        remove_staging(&root_path, &task.task_id)?;
    }
    store.write_failure(&mut task, 2, "用户在创建新任务前明确放弃")?;
    store.cleanup_task_artifacts(&task.task_id)?;
    append_log(
        &log,
        "Abandoned task cleanup completed; evidence records retained",
    )?;
    Ok(())
}
