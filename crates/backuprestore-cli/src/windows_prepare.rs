//! Normal-Windows preparation implemented in Rust.
//!
//! The desktop program does not delegate task creation or system preparation
//! to PowerShell.  This module owns the public `prepare`, `list-volumes`,
//! `inspect-environment` and `wim-info` commands; it uses only Rust file I/O
//! plus the Windows inbox command-line tools that perform the OS operations.

use backuprestore_core::{
    BootMode, DestinationSpec, ImageAcceptance, ImageSpec, Operation, PayloadManifest, TargetRole,
    TargetSpec, Task, TaskError, TaskStore, VolumeIdentity, VolumeRoles,
    sha256_file, validate_absolute_path, validate_volume_roles, write_json_atomic,
};
use chrono::Utc;
use serde_json::json;
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use crate::error_code::ErrCode;
pub(crate) use crate::image_admin::wim_full_windows_probe;
pub(crate) use crate::prepare_bcd::*;
pub(crate) use crate::prepare_plan::*;
pub(crate) use crate::volume_device::*;
use crate::{append_log, err, run_logged, winre_payload};

const RESERVED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// EFI 系统分区的分区类型 GUID。**判定 ESP 必须用它，不能用盘符**——盘符在不同
/// 环境里会重排（恢复环境内尤其如此），本项目此前把 `S:` 当作 ESP 只是约定，
/// `S:` 被别的卷占用或 ESP 挂到别的盘符时都会判错（`text_parsing.rs` 与
/// `native_gui.rs` 的注释都记录过这个靠不住的假设）。
pub(crate) const EFI_TYPE: &str = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}";
#[allow(dead_code)]
const RECOVERY_TYPE: &str = "{de94bba4-06d1-4d40-a16a-bfd50179d6ac}";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) fn prepare(arguments: Vec<String>) -> Result<(), TaskError> {
    let options = parse_prepare_options(arguments)?;
    let executable_dir = executable_dir()?;
    let workspace_drive = drive_from_path(&executable_dir)?;
    validate_prepare_options_safety(&options, Some(workspace_drive))?;
    let workspace = volume_identity(workspace_drive)?;
    let target = if matches!(
        options.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) {
        volume_identity(
            options
                .target_drive
                .ok_or_else(|| err("--target-drive is required for restore operations"))?,
        )?
    } else {
        workspace.clone()
    };
    if matches!(
        options.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) && workspace.same_partition(&target)
    {
        return Err(restore_workspace_target_error(
            workspace.drive_letter.unwrap_or(workspace_drive),
        ));
    }
    // Perform the no-overwrite guard before elevation. A direct CLI call must
    // fail locally without triggering UAC or touching any boot configuration.
    // 2026-10-18：launcher-errors.log 与 GUI 读取方同走 `log_root`——这条日志
    // 记录的恰是「连任务都没建成就失败了」的最早期故障，不能放在可能正是
    // 还原目标的程序目录里。
    require_administrator(&crate::log_root::logs_dir().join("launcher-errors.log"))?;
    prepare_task(&executable_dir, &workspace, target, options)
}

/// 列出当前 BCD 里所有分区 loader 的描述（S5 撞名预检，GUI 创建任务前调用）。
///
/// 刻意**不带 /store、不挂 ESP**：`bcdedit /enum` 是读操作，普通用户权限即可
/// 完成，而 GUI 常态未提权（package 自启副本）；提权 prepare 侧的消解发生在
/// 任务创建之后，用户看不到结果——这里让 GUI 在创建任务前就能预检并回显。
///
/// 失败不报错、输出空列表：调用方（GUI）按「无法预检」处理并记日志，
/// 真正的承重消解在 prepare 侧（挂 ESP 读 /store，必然可用）。
/// 输出 GBK 的本地化字段名用 `decode_windows_bytes` 解，中文菜单名才不会
/// 被 from_utf8_lossy 弄花（弄花等于撞名检查对中文系统全废）。
///
/// 【2026-10-16 审查修订，日志缺口 10】失败原因写进输出的 `error` 字段：
/// 此前失败与"真的没有启动项"都表现为**空列表**，调用方无法区分——
/// 撞名预检会静默失效，而用户看到的菜单名重名问题要到任务创建时才炸。
#[cfg(windows)]
pub(crate) fn list_boot_entries() -> Result<(), TaskError> {
    let output = std::process::Command::new("bcdedit.exe")
        .args(["/enum", "all", "/v"])
        .stdin(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let (descriptions, probe_error) = match &output {
        Ok(output) if output.status.success() => (
            crate::boot_menu::all_loader_descriptions(&crate::text_parsing::decode_windows_bytes(
                &output.stdout,
            )),
            None,
        ),
        Ok(output) => {
            // 退出码非零：把 stderr 尾部一起带出来，让 GUI 能记下真实原因
            //（BCD 存储不可访问 / 权限不足 / 系统无 UEFI 等）。
            let mut text = crate::text_parsing::decode_windows_bytes(&output.stdout);
            text.push_str(&crate::text_parsing::decode_windows_bytes(&output.stderr));
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
    let mut report = serde_json::json!({ "descriptions": descriptions });
    if let Some(error) = probe_error {
        report["error"] = serde_json::json!(error);
    }
    println!("{}", report);
    Ok(())
}

pub(crate) fn inspect_environment() -> Result<(), TaskError> {
    let output = capture("cmd.exe", &["/d", "/c", "ver"])?;
    // `ver` is localized and may use the active Windows code page.  The
    // status panel is UTF-8 Rust text, so retain only its invariant ASCII
    // version token instead of leaking mojibake such as `�本` into Chinese UI.
    let windows = windows_version_summary(&output);
    let architecture = native_architecture();
    // 同 `assert_environment`：`reagentc /info` 有上下文级失败模式（退出码 9），
    // 失败不代表 WinRE 不可用；这里只用于**显示**，失败时退回 BCD 判是否有 WinRE，
    // 绝不因为日志 ACL 问题就让整个「刷新环境」动作失败。
    let winre_available = match capture("reagentc.exe", &["/info"]) {
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
#[cfg(windows)]
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
        return Err(crate::errc(ErrCode::TaskStateInconsistent));
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
        match crate::boot_entry::ReBootEntry::read(&dir)? {
            Some(entry) => {
                crate::boot_entry::disarm(&entry, &log)?;
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
        return Err(crate::errc_with(
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
        let root_path = crate::boot_cleanup::staging_volume_root(volume)?;
        let actual =
            volume_identity_at_path(&root_path.to_string_lossy(), volume.volume_guid.clone())?;
        if !actual.same_partition(volume)
            || actual.partition_offset != volume.partition_offset
            || actual.partition_size != volume.partition_size
        {
            return Err(crate::errc(ErrCode::AbandonPayloadVolumeMismatch));
        }
        crate::boot_cleanup::remove_staging(&root_path, &task.task_id)?;
    }
    store.write_failure(&mut task, 2, "用户在创建新任务前明确放弃")?;
    store.cleanup_task_artifacts(&task.task_id)?;
    append_log(
        &log,
        "Abandoned task cleanup completed; evidence records retained",
    )?;
    Ok(())
}

fn prepare_task(
    executable_dir: &Path,
    workspace: &VolumeIdentity,
    target: VolumeIdentity,
    options: PrepareOptions,
) -> Result<(), TaskError> {
    // ★ 盘符时间线的起点必须在这里，早于任何身份探测：恢复卷的盘符是
    //   `recovery_identity()`（下面第 979 行那段之后）跑 `diskpart assign letter=`
    //   现挂的，晚一步开就正好漏掉要观测的那一次。设 `BACKUPRESTORE_LETTER_TRACE_MS`
    //   （如 200）才启采样线程；不设时只记「谁挂的、谁撤的」。
    crate::letter_trace::start(crate::log_root::logs_dir().join("drive-letter-trace.log"));
    assert_environment(options.source_drive)?;
    let source = volume_identity(options.source_drive)?;
    require_windows_volume(&source, "source")?;
    assert_bitlocker_off(options.source_drive)?;
    if workspace.is_reserved_partition() {
        return Err(err("program directory cannot be on EFI, MSR or Recovery"));
    }
    let (image_volume, image_path, image_relative) = if let Some(path) = &options.image_path {
        let (path, volume, relative) =
            image_path_info(path, options.operation != Operation::Backup)?;
        (volume, Some(path), relative)
    } else {
        (workspace.clone(), None, String::new())
    };
    if image_volume.is_reserved_partition() {
        return Err(err("image volume cannot be EFI, MSR or Recovery"));
    }
    assert_bitlocker_off(
        image_volume
            .drive_letter
            .ok_or_else(|| err("image volume has no drive letter"))?,
    )?;
    if matches!(
        options.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) {
        // Reject reserved restore targets explicitly before the BitLocker
        // probe. `manage-bde` cannot open EFI/MSR/Recovery partitions, so
        // without this early check a user selecting the EFI or Recovery
        // partition would see a confusing BitLocker error instead of the real
        // reason the target is ineligible.
        if target.is_reserved_partition() {
            return Err(restore_reserved_target_error());
        }
        assert_bitlocker_off(
            options
                .target_drive
                .ok_or_else(|| err("restore target drive is missing"))?,
        )?;
    }
    let recovery = recovery_identity()?;
    let efi = efi_identity(options.efi_drive)?;
    if workspace.same_partition(&efi) {
        return Err(err("program directory cannot be on EFI volume"));
    }
    if image_volume.same_partition(&efi) {
        return Err(err("image volume cannot be on EFI volume"));
    }
    // v1.7.11 新启动通道：注册 WinRE **全程只读**，因此
    //   - F1（还原目标 == 注册 WinRE 宿主）：格式化不再威胁「续跑启动源」（它在镜像卷上）；
    //   - F3（备份源 == 注册 WinRE 宿主）：载荷从不写注册位，镜像天然干净。
    // 两道冲突门与迁出闸门随之删除；仅保留 workspace/镜像卷不能落在注册 WinRE 分区
    // 这条基础规则（那是程序自身资产，与任务启动源无关）。
    let roles = VolumeRoles {
        workspace,
        image: &image_volume,
        source: Some(&source),
        target: matches!(
            options.operation,
            Operation::RestoreExisting | Operation::CreateSecondary
        )
        .then_some(&target),
    };
    if let Some(conflict) = validate_volume_roles(&roles, options.operation, &recovery).err() {
        return Err(err(conflict.message()));
    }
    // 准备日志先建好再跑输入校验：`validate_operation_inputs` 里的 DISM 预检
    // （镜像完整性探测）要往这里写 stdout/stderr。早先日志目录在校验**之后**
    // 才创建，那段失败就无处落盘——正是 2026-10-16 审查点名的日志缺口 1。
    // 2026-10-18：落点改走 `log_root`（用户要求：不放 PE、不放目标分区）。
    let bootstrap_log = crate::log_root::logs_dir().join("prepare-bootstrap.log");
    if let Some(parent) = bootstrap_log.parent() {
        fs::create_dir_all(parent)?;
    }
    let validation = validate_operation_inputs(
        &source,
        &target,
        &image_volume,
        image_path.as_deref(),
        &options,
        &bootstrap_log,
    )?;
    let image_acceptance = validation.acceptance;
    let store = TaskStore::new(executable_dir);
    // S5 新启动项撞名消解（docs/20261015-0725）：拟用的菜单名与现有任一启动项
    // 重名时追加 -YYYYMMDDHHMM。GUI 侧已按用户可见的方式消解过一次（幂等，
    // 已加后缀的名字不会二次加），但 CLI 直跑不过 GUI，必须在这里兜底——
    // 否则 validate 的「启动菜单出现同名项」会直接拒掉任务，而用户看到的
    // 名字是他自己起的，完全无法归因。读不到 BCD 不阻断：退化成不消解，
    // 由 validate 的同名守卫以明确错误拦住，也好过 silently 建出两个同名项。
    let options = {
        let mut options = options;
        if options.operation == Operation::CreateSecondary {
            let existing = crate::boot_menu::existing_loader_descriptions(&bootstrap_log);
            let resolved = crate::boot_menu::resolve_menu_name_collision(
                &options.boot_menu_name,
                &existing,
                chrono::Local::now(),
            );
            if resolved != options.boot_menu_name.trim() {
                append_log(
                    &bootstrap_log,
                    &crate::i18n::msg_tpl(
                        "wp_boot_menu_dup",
                        &[&options.boot_menu_name, &resolved],
                    ),
                )?;
            }
            options.boot_menu_name = resolved;
        }
        options
    };
    let cleanup =
        store.cleanup_terminal_tasks(backuprestore_core::DEFAULT_TERMINAL_TASK_RETENTION)?;
    if !cleanup.removed_task_ids.is_empty()
        || cleanup.skipped_nonterminal > 0
        || !cleanup.skipped_mounted.is_empty()
    {
        append_log(
            &bootstrap_log,
            &format!(
                "terminal task cleanup: removed={}, skipped_nonterminal={}, skipped_mounted={}",
                cleanup.removed_task_ids.len(),
                cleanup.skipped_nonterminal,
                cleanup.skipped_mounted.len()
            ),
        )?;
    }
    // 一个工作区共用同一份 EFI BCD 的 bootsequence，无法安全并发两个未终结的
    // 启动事务。载荷虽按 task ID 隔离，但那只防止文件互相覆盖，不解决 bootsequence
    // 争用；因此新任务只要发现任何未终结任务，都先停下交给用户选择继续或清理。
    // 这一步必须位于创建新 task.json、导出 BCD 和修改任何启动对象之前。
    let pending = store.incomplete_tasks()?;
    if !pending.is_empty() {
        let details = pending
            .iter()
            .map(|task| {
                let target = task
                    .target
                    .as_ref()
                    .map(|spec| spec.volume.partition_guid.as_str())
                    .unwrap_or("(无目标卷)");
                format!(
                    "id={} operation={:?} stage={:?} target={} created={}",
                    task.task_id, task.operation, task.status, target, task.created
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        append_log(
            &bootstrap_log,
            &format!("new preparation blocked by incomplete tasks: {details}"),
        )?;
        return Err(crate::errc_with(ErrCode::UnterminatedTaskExists, &[&details]));
    }
    ensure_workspace_capacity(workspace, &recovery)?;

    let boot_plan = match options.operation {
        Operation::CreateSecondary => BootMode::AddSecondary,
        _ => BootMode::ReturnExisting,
    };
    let mut task = Task::new(
        options.operation,
        backuprestore_core::BootPlan {
            mode: boot_plan,
            previous_bcd_sha256: None,
            menu_name: if options.operation == Operation::CreateSecondary {
                Some(options.boot_menu_name.clone())
            } else {
                None
            },
            // The one-time boot request is enabled only after the BCD
            // snapshot has been exported and hashed below. This lets us run
            // a complete side-effect-free task validation before any BCD or
            // WinRE mutation while still persisting a fully protected plan.
            boot_sequence_requested: false,
        },
    );
    task.source = Some(source.clone());
    task.workspace_volume = Some(workspace.clone());
    task.compress = options.compress.clone();
    match options.operation {
        Operation::Backup => {
            // 备份前镜像卷空间预检（2026-10-26）：在「决定重启去做」之前就挡掉
            // 明显装不下的目标，省得进恢复环境写了几十 GiB 才失败。★ 硬闸比例
            // 故意很低（源已用量的 5%），宁可放过也不误拒——WIM 压缩比随内容
            // 变化极大，按固定比例预估会拦掉真实可行的备份。
            if let Some(path) = image_path.as_ref() {
                ensure_backup_image_capacity(
                    options.source_drive,
                    path,
                    options.compress.as_deref().unwrap_or("fast"),
                )?;
            }
            task.destination = Some(DestinationSpec {
                volume: image_volume.clone(),
                absolute_path: image_path.clone(),
                relative_path: image_relative.clone(),
            });
            // 备份索引名与保留最近 N 个索引：写入任务供恢复执行时使用。
            task.image_name = options.image_name.clone();
            task.keep_indexes = options.keep_indexes;
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let Some(path) = image_path.as_ref() else {
                return Err(crate::errc(ErrCode::ImageFileMissing));
            };
            let file_size = fs::metadata(path)?.len();
            // 优先复用 sidecar metadata 里的权威 sha256；无 sidecar 镜像且未勾选 verify_hash 时填充合规占位，彻底避免重复读盘。
            // 例外：用户已显式接受"与副档不符的当前镜像"时，权威值改成**接受时读到的真实哈希**。
            // 否则任务里留着副档旧值 + verifyHash=true，重启后离线侧必然再次拒绝，
            // 用户的强制继续被静默撤销（历史缺陷）。
            let accepted_sha256 = image_acceptance
                .as_ref()
                .filter(|accepted| !accepted.accepted_sha256.is_empty())
                .map(|accepted| accepted.accepted_sha256.clone());
            let sha256 = if let Some(accepted) = accepted_sha256 {
                accepted
            } else if let Ok(meta) = crate::read_index_metadata(Path::new(path), options.wim_index)
            {
                meta.image_sha256
            } else if options.verify_hash {
                sha256_file(path)?
            } else {
                "0".repeat(64)
            };
            task.verify_hash = Some(options.verify_hash);
            task.image_acceptance = image_acceptance.clone();
            task.image = Some(ImageSpec {
                volume: image_volume.clone(),
                absolute_path: Some(path.clone()),
                relative_path: image_relative.clone(),
                sha256,
                size_bytes: file_size,
                index: options.wim_index,
                name: None,
            });
            task.target = Some(TargetSpec {
                volume: target.clone(),
                role: if options.operation == Operation::CreateSecondary {
                    TargetRole::NewWindows
                } else {
                    TargetRole::ExistingWindows
                },
                boot_menu_name: (options.operation == Operation::CreateSecondary)
                    .then(|| options.boot_menu_name.clone()),
                minimum_size_bytes: target.partition_size,
                // 目标性质在这里定案并落盘：RE 内 `%SystemDrive%` 是恢复环境的 RAM 盘 X:，
                // 探测不到用户的系统卷，所以离线侧只能读这个结论（见字段文档）。
                // 取不到活动系统身份时直接失败，不要退化成 false——那会让「重建唯一系统」
                // 误走第二系统收尾。
                is_active_system: target.same_partition(&active_system_volume_identity()?),
                // 格式化选项落盘（GUI 复选框 → prepare 参数 → RE 恢复链读取）
                format_target: options.format_target,
                // 「覆盖式应用到已含 Windows 卷」的路由标记（任务 #110）：
                // 由 validate 判定，RE 恢复链读取后走 wimlib 通道。
                overlay_no_acl: validation.overlay_no_acl,
            });
        }
        Operation::Probe => {}
    }

    // Validate the complete task before exporting BCD or touching WinRE. This
    // keeps malformed/reserved/undersized requests side-effect free even when
    // they arrived through the CLI instead of the GUI.
    task.validate()?;

    let task_dir = store.task_dir(&task.task_id)?;
    let prepare_log = task_dir.join("prepare.log");
    let bootstrap_bcd = executable_dir.join(format!(".backuprestore-{}.bcd", task.task_id));
    let bootstrap_arg = bootstrap_bcd.to_string_lossy().into_owned();
    run_logged("bcdedit.exe", &["/export", &bootstrap_arg], &bootstrap_log)?;
    task.boot_plan.previous_bcd_sha256 = Some(sha256_file(&bootstrap_bcd)?);
    task.boot_plan.boot_sequence_requested = true;
    task.validate()?;
    // 新启动通道：载荷放到**镜像卷**（re_staging），注册位只读，不污染系统卷。
    //
    // 2026-10-13 修正（docs/20261013-1936 对照实验 + 单变量矩阵 A/B/C）：
    // 1935 的「bootmgr 只解析系统卷盘符」结论不完整——真正的情况是
    // **盘符形式**的 ramdisk 规格只对系统卷生效，数据卷必须写
    // \Device\HarddiskVolumeN 设备路径形式（bcdedit 显示层两种都规范化成 [X:]，
    // 存储层不同，所以从回读看不出差别）。设备路径形式在数据卷上实测可引导
    // （GUID 目录 + 隐藏/系统属性 + 设备路径 = 成功；同结构换成盘符 = 回落）。
    // 修复落在 boot_entry（写规格时解析设备路径），载荷因此可以留在镜像卷：
    // 不污染 C:，单系统还原格式化目标卷也不会抹掉断电续跑的载荷。
    // 2.4.5 曾把载荷改到系统卷作权宜——那会污染 C: 且 restore-existing 断电后
    // 无法续跑，本轮回退。
    // ★ 字母会**逸出**：`boot_sdi` 把盘符拼进路径、存在结构体里，后面 prepare_payload
    //   还要按它读注册的 WinRE。显式延到进程结束（有边界：收尾统一撤销 + 崩溃兜底）。
    let (recovery_letter, lease_recovery) =
        ensure_volume_mounted_leased(&recovery, 'R', &prepare_log)?;
    // ★★ 复核结论（2026-10-28 深夜 + 2026-10-29 实机观测，见
    //    docs/20261029-0215-…-盘符观测-… 与 `letter_trace` 模块注释）：
    //    下面这行 `hold_for_process()` 在本机是**空转**，原因是：恢复卷本身就有盘符，
    //    `ensure_volume_mounted_leased` 一定走「复用既有盘符」分支 → `created=false`
    //    → 租约 `letter=None`。
    //
    //    2026-10-29 的实机时间线把「那个盘符哪来的」钉死了，答案和当初的推断不同：
    //      - 本机 WinRE 注册在 `harddisk0\partition4`，也就是 **OS 分区（C:）**，
    //        所以 `recovery_identity()` 解析出的恢复卷就是 C:，`identity_from_diskpart`
    //        在「复用已挂载分区」那一轮就命中 C:，**从不执行 `diskpart assign letter=`**；
    //      - 时间线实证：`REUSE C: disk 0 partition 4 (already mounted; no assignment)`，
    //        全程零挂载、零撤销。
    //    也就是说「准备期临时分配了一个 R:」在**本机这个布局下不存在**。它只在
    //    「WinRE 落在无盘符的专用恢复分区」时才会发生（本机分区 5 只有 840 MB，
    //    Win11 26100 的 reagentc 不接受，实测三次都坚持放回 partition4）。
    //
    //    ⚠️ 这**不是类型级恒真**，只是「:979 到 :1226 之间恢复卷一直有盘符」这个
    //       环境前提。所以这里只能放**调试期**断言——release 下编译掉，生产行为一字
    //       不变；一旦报警，说明前提变了、要去查 `identity_from_diskpart`，而不是来
    //       删这条断言。
    //    另外：`identity_from_diskpart` 里那条 `diskpart assign letter=` 确实**没有
    //    租约**（它写的是持久的 MountedDevices），在 OEM 布局上是真实的泄漏风险，
    //    只是本机走不到。要不要给它补租约，等有 OEM 布局的机器再说。
    debug_assert!(
        lease_recovery.owns_no_letter(),
        "recovery lease unexpectedly owns {recovery_letter}: the recovery volume lost its \
         drive letter between the workspace capacity check and the lease point"
    );
    lease_recovery.hold_for_process();
    let re_staging = ReStaging {
        volume: image_volume.clone(),
        boot_sdi: PathBuf::from(format!(r"{recovery_letter}:\Recovery\WindowsRE\boot.sdi")),
    };
    let result = prepare_payload(
        executable_dir,
        &store,
        &mut task,
        &recovery,
        &efi,
        &options,
        &prepare_log,
        &bootstrap_bcd,
        &re_staging,
    );
    let _ = fs::remove_file(&bootstrap_bcd);
    if let Err(error) = result {
        if task_dir.exists() {
            // 把稳定错误码一起落盘：Io/Json/InvalidTransition 的 Display 故意不带码，
            // 只有 error.code() 拿得到（界面「刷新任务状态」据此显示）。
            let _ = store.write_failure_coded(
                &mut task,
                error.code(),
                error.to_string(),
                Some(1),
            );
            let _ = append_log(&prepare_log, &format!("Preparation failed: {error}"));
        }
        return Err(error);
    }
    // recoveryLog 指向镜像同目录（如 E:\Recovery.log），与 WIM 并排便于查看；
    // GUI「刷新任务状态」据此显示。备份/还原都会把日志写到镜像同目录。
    let recovery_log_path = match options.image_path.as_ref() {
        Some(path) => Path::new(path)
            .parent()
            .map(|parent| parent.join("Recovery.log"))
            .unwrap_or_else(|| PathBuf::from("Recovery.log")),
        None => store.log_path(&task.task_id)?,
    };
    write_json_atomic(
        executable_dir.join("last-task.json"),
        &json!({
            "taskId": task.task_id,
            "operation": task.operation,
            "taskRoot": task_dir,
            "statusJson": store.status_path(&task.task_id)?,
            "recoveryLog": recovery_log_path,
            "prepareLog": prepare_log,
            "imagePath": image_path,
            "created": Utc::now(),
        }),
    )?;
    // `--json` 下用单行 JSON 取代两行 `KEY=VALUE`：判据只认字段，不再解析
    // 自然语言，也不必再区分「哪些命令有 TASK_ID 哪些没有」。
    // 非 json 模式保持旧的两行输出（既有脚本依赖它，不动）。
    // 详情上报给 `main` 统一输出（operation 由 main 从 `--operation` 参数推断，
    // 口径与 online 一致）。非 json 模式保持旧的两行输出。
    crate::cli_json::set_detail(
        Some(task.task_id.clone()),
        Some(prepare_log.display().to_string()),
    );
    if !crate::cli_json::enabled() {
        println!("TASK_ID={}", task.task_id);
        println!("TASK_ROOT={}", task_dir.display());
    }
    Ok(())
}

/// 新启动通道（v1.7.11）需要的「载荷 RE 暂存位置」信息。
///
/// * `volume` — 承载载荷 WIM 的卷（默认 = 镜像卷；本来就可写、且不被还原格式化）。
/// * `boot_sdi` — RAM 盘模板文件来源（注册卷上的 `boot.sdi`，可能不存在）。
pub struct ReStaging {
    pub volume: VolumeIdentity,
    pub boot_sdi: PathBuf,
}

// These values have different ownership and rollback roles. Keeping the
// parameters explicit makes preparation ordering and cleanup dependencies
// auditable at the call site.
#[allow(clippy::too_many_arguments)]
fn prepare_payload(
    executable_dir: &Path,
    store: &TaskStore,
    task: &mut Task,
    recovery: &VolumeIdentity,
    efi: &VolumeIdentity,
    options: &PrepareOptions,
    log: &Path,
    bootstrap_bcd: &Path,
    re_staging: &ReStaging,
) -> Result<(), TaskError> {
    let task_dir = store.task_dir(&task.task_id)?;
    let payload = task_dir.join("payload");
    let original = task_dir.join("original");
    let stage = task_dir.join("stage");
    let mount = task_dir.join("mount");
    store.create(task)?;
    fs::create_dir_all(&payload)?;
    fs::create_dir_all(&original)?;
    fs::create_dir_all(&stage)?;
    fs::create_dir_all(&mount)?;
    append_log(log, "Rust preparation started")?;

    fs::copy(bootstrap_bcd, task_dir.join("bcd-before-export"))?;
    let raw_bcd_hash = snapshot_raw_bcd(efi, &task_dir, log)?;
    // `bcdedit /export` is a logical export. Importing it may rewrite the
    // binary hive, so new tasks retain a byte-for-byte EFI store snapshot for
    // rollback while old tasks can still use the exported fallback.
    task.boot_plan.previous_bcd_sha256 = Some(raw_bcd_hash);
    task.validate()?;
    // `store.create` runs before the EFI snapshot is captured so the task
    // directory exists for the raw snapshot. Persist the updated boot-plan
    // hash before copying task.json into the WinRE payload; otherwise Recovery
    // would correctly reject a stale payload/task pair.
    write_json_atomic(store.task_path(&task.task_id)?, task)?;

    // 字母只在本函数内用于读注册的 WinRE 文件，函数结束即撤销。
    let (recovery_letter, _lease_recovery) = ensure_volume_mounted_leased(recovery, 'R', log)?;
    let registered_wim = PathBuf::from(format!(
        r"{}:\Recovery\WindowsRE\Winre.wim",
        recovery_letter
    ));
    if !registered_wim.is_file() {
        return Err(err("registered WinRE image is missing"));
    }
    fs::copy(&registered_wim, original.join("Winre.wim"))?;
    let original_hash = sha256_file(original.join("Winre.wim"))?;

    // 载荷 Recovery.exe 与主程序按契约同源；若同目录那份是旧件，
    // stage_static_payload 会改用正在运行的可执行文件并回一条告警——
    // 这条必须落进准备日志，否则"离线侧跑的是旧代码"只能事后从
    // 镜像元数据的 programVersion 里反推。
    if let Some(warning) = winre_payload::stage_static_payload(executable_dir, &payload)? {
        append_log(log, &format!("payload staging warning: {warning}"))?;
    }

    let env_path = payload.join("RecoveryTask.env");
    // 迁出任务：env 的 RECOVERY_* 直接指向 RE 暂存卷（先于注入烘焙进 WIM），
    write_recovery_env(&env_path, executable_dir, task, recovery, efi, options)?;
    fs::copy(store.task_path(&task.task_id)?, payload.join("task.json"))?;
    let recovery_hash = sha256_file(payload.join("Recovery.exe"))?;
    let task_hash = sha256_file(payload.join("task.json"))?;
    append_env(
        &env_path,
        &[("ORIGINAL_WINRE_SHA256", original_hash.as_str())],
    )?;

    let staged = stage.join("Winre.wim");
    fs::copy(&registered_wim, &staged)?;
    // ★ v1.7.11 顺序要点：**先建 BCD 条目，再做 DISM 注入**。
    // 实机结论：同一串 argv 下，bcdedit 在「本进程刚跑过 DISM」时校验镜像卷上的 WIM 会
    // 失败（报「指定的设备无效」，对象状态经 /enum 核验完全正确；换个进程立刻重试就成功）。
    // 而建条目只需要「镜像卷上有一个合法 WIM」——干净的注册 WIM 副本就够。
    // 一次性启动留到载荷注入并拷贝完成之后再武装（此时才允许人重启进来）。
    // ★ 问题 11：这一段（建 BCD 条目 → 载荷覆盖 → DISM 挂载/注入 → 卸载提交）
    // 失败时以前直接 `?` 返回，界面上是"准备失败"，机器上却可能留着本任务的
    // BCD 条目、镜像卷载荷，`boot-entry.json` 还指向它们，下一次重启进的不是
    // 用户期待的东西。现在整段收进闭包，任一失败都先做统一补偿再返回原错误——
    // 绝不为了"看起来干净"而把补偿失败盖成成功。
    let boot_entry = (|| -> Result<crate::boot_entry::ReBootEntry, TaskError> {
        // ★ 2026-10-12 实测修复：载荷与 RecoveryTask.env 必须**先**注入 staged，
        // 再建 BCD 条目 / 把 staged 复制到镜像卷暂存区（create_entry 与
        // copy_into_staging 都会把 staged 拷到 devopts 实际启动的暂存 WIM）。
        // 原顺序是「create_entry + copy_into_staging → DISM 注入」，于是 devopts
        // 启动的暂存 WIM 保留的是**注册位里的旧 env**——而 env 是任务相关的
        // （含 TASK_ID）。RE 用 winpeshl 拉起 Recovery.exe 读到的就是上一个任务的
        // env，表现为两种故障：① 注册位没 env 时 os error 2（文件不存在）；
        // ② 注册位有旧 env 时 "task is already terminal at stage Failed"（读到了
        // 上一个已终态的任务）。注入必须先于任何拷贝，否则 devopts 永远带着陈旧 env。
        // 风险注：DISM 在本进程跑过后紧接着 bcdedit 校验镜像卷 WIM 历史上偶发
        // 「指定的设备无效」；这里 DISM 针对的是 F: 上的 staged，提交后 sleep 5s
        // 再拷贝到 H: 并校验，实测未复现（若复现，把 copy_into_staging 与
        // create_entry 拆开、bcdedit 校验放最前即可）。
        let staged_arg = staged.to_string_lossy().into_owned();
        let mount_arg = mount.to_string_lossy().into_owned();
        run_logged(
            "dism.exe",
            &[
                "/Mount-Image",
                &format!("/ImageFile:{staged_arg}"),
                "/Index:1",
                &format!("/MountDir:{mount_arg}"),
            ],
            log,
        )?;
        let result = winre_payload::inject_winre_payload(&mount, &payload);
        if let Err(error) = result {
            let _ = run_logged(
                "dism.exe",
                &[
                    "/Unmount-Image",
                    &format!("/MountDir:{mount_arg}"),
                    "/Discard",
                ],
                log,
            );
            return Err(error);
        }
        run_logged(
            "dism.exe",
            &[
                "/Unmount-Image",
                &format!("/MountDir:{mount_arg}"),
                "/Commit",
            ],
            log,
        )?;
        thread::sleep(Duration::from_secs(5));

        let boot_entry = crate::boot_entry::create_entry(
            &staged,
            &re_staging.boot_sdi,
            &re_staging.volume,
            task_dir.as_path(),
            &task.task_id,
            log,
        )?;
        crate::boot_entry::copy_into_staging(
            &staged,
            &re_staging.boot_sdi,
            &re_staging.volume,
            &task.task_id,
            log,
        )?;
        Ok(boot_entry)
    })();
    if let Err(error) = &boot_entry {
        // 真正执行统一补偿：撤销本任务已建的启动项、载荷，并把 BCD 恢复到
        // 建条目之前导出的字节级快照。补偿本身失败时一并报出，不掩盖原错误。
        append_log(
            log,
            &format!(
                "[ERROR] preparation failed after boot entry was created ({error}); compensating"
            ),
        )?;
        if let Err(cleanup) = compensate_prepare_failure(task_dir.as_path(), efi, log) {
            append_log(
                log,
                &format!(
                    "[ERROR] compensation incomplete after preparation failure ({error}); {cleanup}"
                ),
            )?;
        }
        return Err(err(&format!(
            "preparation failed: {error}; boot entry and payload rolled back (see prepare.log)"
        )));
    }
    let mut boot_entry = boot_entry?;

    let staged_hash = sha256_file(&staged)?;
    let manifest = PayloadManifest {
        task_id: task.task_id.clone(),
        recovery_sha256: recovery_hash,
        task_sha256: task_hash,
        original_winre_sha256: original_hash.clone(),
        staged_winre_sha256: staged_hash.clone(),
        created_by_version: backuprestore_core::PROGRAM_VERSION.into(),
        recovery_task_env_sha256: Some(sha256_file(&env_path)?),
    };
    write_json_atomic(task_dir.join("manifest.json"), &manifest)?;
    write_status_env(&task_dir, task, "prepared")?;
    if options.no_reboot {
        append_log(
            log,
            "Task prepared with --no-reboot; registered WinRE unchanged",
        )?;
        return Ok(());
    }

    // v1.7.11 新启动通道：**不写注册位、不跑 reagentc /boottore**。
    // 注入后的载荷 WIM 覆盖到镜像卷的 BackupRestoreRE\，然后武装我们自建的那条
    // BCD 条目（条目本身在 DISM 之前就建好了，见上面的顺序要点）。
    // 注册位全程只是只读资产来源，因此迁出 / WINRE_HOME / 待回家收尾 / Plan D / F1·F3 全部不需要。
    // 依据：docs/20260929-1300-pe-channel-poc-winre-wim-boots-from-image-volume.md
    crate::boot_entry::copy_into_staging(
        &staged,
        &re_staging.boot_sdi,
        &re_staging.volume,
        &task.task_id,
        log,
    )?;
    // ★ 把簿记里的载荷哈希刷新成**注入后**的实际值。
    // create_entry 在 DISM 注入之前就跑（顺序要点见上），那时的
    // copy_into_staging 复制的是干净原件，记下的是它的哈希；注入后载荷
    // WIM 被覆写成另一份，哈希随之改变。不刷新的话，rearm() 在断电续跑时
    // 拿活载荷比对这份过期记录必然不符，整条续跑路被封死
    // （2026-09-30 实机：stage 卡在 image-applied/75，日志
    //  "payload WIM hash differs from the prepared one; refusing to re-arm"）。
    // ★ 问题 11（续）：`arm_one_shot` 之后同样要补偿。这一段失败比以前更危险——
    // 武装成功但状态落盘失败时，机器带着一个一次性启动请求重启，却没有任何
    // 持久状态说明这是谁设的，等于把人推进一个没人负责的任务环境。
    // 因此这里也必须统一撤销（清 bootsequence、删对象、删载荷、恢复 BCD 快照）。
    let armed = (|| -> Result<(), TaskError> {
        crate::boot_entry::refresh_payload_hash(&mut boot_entry, task_dir.as_path(), log)?;
        // 武装是「人可以重启了」的唯一开关，放在这一步：载荷已注入、已覆盖到镜像卷、
        // 簿记已落盘。v1.7.14 之前它藏在 create_entry 里无条件执行，导致
        // `--no-reboot` 也会改 bootmgr 的 bootsequence（实机证据见 boot_entry::arm_one_shot）。
        crate::boot_entry::arm_one_shot(&boot_entry, log)?;
        store.write_transition(task, backuprestore_core::Stage::BootRequested)?;
        write_status_env(&task_dir, task, "boot-requested")?;
        Ok(())
    })();
    if let Err(error) = armed {
        append_log(
            log,
            &format!("[ERROR] failed while arming the boot request ({error}); compensating"),
        )?;
        if let Err(cleanup) = compensate_prepare_failure(task_dir.as_path(), efi, log) {
            append_log(
                log,
                &format!(
                    "[ERROR] compensation incomplete after arming failure ({error}); {cleanup}"
                ),
            )?;
        }
        return Err(err(&format!(
            "preparation failed while arming the boot request: {error}; rolled back (see prepare.log)"
        )));
    }
    if options.test_fault.as_deref() == Some("power-loss-window") {
        append_log(
            log,
            "Development test fault: stopped after durable boot-requested state; no shutdown requested",
        )?;
        return Ok(());
    }
    // ★ 首启回落修复（2026-10-13 E2E 实测，见 flush_volume 注释）：武装成功后、
    // 请求重启前，把载荷卷与 ESP 的卷缓存显式落盘。失败与 shutdown 失败同样
    // 走统一补偿（此刻 bootsequence 已武装，绝不能留一个"可能读不到载荷"的
    // 一次性启动给下次开机）。
    let flush_before_reboot = (|| -> Result<(), TaskError> {
        // 租约：刷完缓存这两个盘符就没用了，紧接着是 `shutdown /r`；
        // 不留它们，避免下一次准备又撞上上一次留下的常驻盘符。
        let (staging, _lease_staging) =
            ensure_volume_mounted_leased(&boot_entry.wim_volume, 'R', log)?;
        flush_volume(staging, "载荷卷", log)?;
        let (esp_letter, _lease_esp) = ensure_volume_mounted_leased(&efi, 'S', log)?;
        flush_volume(esp_letter, "ESP/BCD", log)?;
        Ok(())
    })();
    if let Err(error) = flush_before_reboot {
        append_log(
            log,
            &format!("[ERROR] pre-reboot volume flush failed ({error}); compensating"),
        )?;
        if let Err(cleanup) = compensate_prepare_failure(task_dir.as_path(), efi, log) {
            append_log(
                log,
                &format!(
                    "[ERROR] compensation incomplete after flush failure ({error}); {cleanup}"
                ),
            )?;
        }
        return Err(err(&format!(
            "preparation failed at pre-reboot flush: {error}; rolled back (see prepare.log)"
        )));
    }
    // ★ 请求重启前先把本次进程占的临时盘符全部还回去：`mountvol` 的指派是**持久**记录，
    //   不还的话它们会跟着重启活下来，等分区哪天被删就变成幽灵盘符。
    release_process_leases(log);
    if let Err(error) = run_logged("shutdown.exe", &["/r", "/t", "0"], log) {
        // 重启请求失败：别把机器留在「下次开机进任务 RE，但没有人会来跑」的状态。
        // 走与其它准备失败点**同一个**补偿路径（清 bootsequence → 删对象 →
        // 删载荷 → 恢复 BCD 快照），避免两处清理逻辑各自漂移。
        append_log(
            log,
            &format!("[ERROR] shutdown request failed ({error}); compensating"),
        )?;
        if let Err(cleanup) = compensate_prepare_failure(task_dir.as_path(), efi, log) {
            append_log(
                log,
                &format!(
                    "[ERROR] compensation incomplete after shutdown failure ({error}); {cleanup}"
                ),
            )?;
        }
        return Err(err(&format!(
            "shutdown request failed: {error}; boot entry and payload rolled back (see prepare.log)"
        )));
    }
    Ok(())
}

/// ESP 上 BCD 存储的路径，**优先零盘符**。
///
/// 返回 `Some(..)` 表示用户在本次准备中显式接受了「与副档不符的当前镜像」，
/// 调用方必须把这份例外连同**接受时的真实哈希**一起写进任务，否则重启后
/// 离线侧会拿旧副档值再次拒绝（历史缺陷，见 [`ImageAcceptance`]）。
/// `validate_operation_inputs` 的产出（2026-10-22 任务 #110 扩展）。
pub(crate) struct ValidationOutcome {
    /// 用户显式接受了与副档不符的当前镜像（原返回值）。
    pub acceptance: Option<ImageAcceptance>,
    /// 命中「覆盖式应用（取消格式化）打到已含 Windows 的目标卷」组合。
    ///
    /// 2026-10-22 起该组合**不再硬拒**（用户裁定「换 wimlib」）：
    /// 准备期在此落盘 `overlay_no_acl=true`，恢复期（WinRE 链）据此走 wimlib
    /// 通道——DISM 该组合必然 `RestoreDirMetadata 0x80070005`（PE 内对照实验
    /// 钉死，见 docs/20261016-1413 §2），wimlib 自行获取 SeRestore/SeBackup
    /// 提权且该提权覆盖 SD 还原（2026-10-23 概念验证，docs/20261022-2308：
    /// tiny + 全量两级，镜像 SD 逐字节还原），故镜像目录权限完整还原、
    /// 个别受保护目录可能保留卷上现有权限（GUI 确认框明示）。
    /// GUI 侧有显式确认框明示该语义；CLI 直跑视为高级用户
    /// 显式选择 `--skip-format` 的意图。
    /// **名字沿袭历史**：`overlay_no_acl` 里的 `no_acl` 是 v2.6.25–v2.6.26
    /// 「跳过 SD 还原」时代的残留，当前语义 = 覆盖式应用（安全描述符随镜像
    /// 还原）；不改名是为了 task.json 序列化兼容。
    pub overlay_no_acl: bool,
}

fn validate_operation_inputs(
    source: &VolumeIdentity,
    target: &VolumeIdentity,
    image: &VolumeIdentity,
    image_path: Option<&str>,
    options: &PrepareOptions,
    log: &Path,
) -> Result<ValidationOutcome, TaskError> {
    match options.operation {
        Operation::Probe => Ok(ValidationOutcome {
            acceptance: None,
            overlay_no_acl: false,
        }),
        Operation::Backup => {
            if image.same_partition(source) {
                return Err(err("image volume must differ from backup source"));
            }
            // 盘符在这里**显式**取，不靠 400 行外那条为 BitLocker 写的
            // `assert_bitlocker_off(...ok_or_else...)` 顺带保证：那条守卫
            // 一旦被重构掉，这里就会退化成 panic。
            let source_letter = source
                .drive_letter
                .ok_or_else(|| err("backup source volume has no drive letter"))?;
            let image_letter = image
                .drive_letter
                .ok_or_else(|| err("image volume has no drive letter"))?;
            let required = source
                .partition_size
                .saturating_sub(volume_free_bytes(source_letter)?)
                .saturating_add(RESERVED_BYTES);
            // Appending is transactional: Recovery copies the current WIM to
            // a same-volume candidate before capture. Reserve that extra copy
            // here so a full destination cannot strand the original half-way
            // through an append attempt.
            let append_candidate_bytes = image_path
                .filter(|path| Path::new(path).is_file())
                .map(|path| fs::metadata(path).map(|metadata| metadata.len()))
                .transpose()?
                .unwrap_or(0);
            let required = required.saturating_add(append_candidate_bytes);
            if volume_free_bytes(image_letter)? < required {
                return Err(err("backup destination free space is insufficient"));
            }
            Ok(ValidationOutcome {
                acceptance: None,
                overlay_no_acl: false,
            })
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            // 「覆盖式应用到已含 Windows 卷」的路由标记：命中组合时置 true，
            // 由调用方写进 `TargetSpec.overlay_no_acl` 落盘（见
            // [`ValidationOutcome`] 文档）。未命中（含 Backup）恒 false。
            let mut overlay_no_acl = false;
            if !options.allow_destructive {
                return Err(err("restore requires --allow-destructive"));
            }
            if image.same_partition(target) {
                return Err(err("image volume must differ from restore target"));
            }
            if options.operation == Operation::RestoreExisting && !source.same_partition(target) {
                return Err(err("single-system restore target must equal source"));
            }
            if options.operation == Operation::CreateSecondary && source.same_partition(target) {
                return Err(err("second-system target must differ from source"));
            }
            // 目标是不是当前正在运行的那套系统。这个结论下面要用三次：P0 安全闸、
            // 是否需要完整 Windows 镜像（P2）、以及落进任务给 RE 读的 is_active_system。
            let target_is_active = target.same_partition(&active_system_volume_identity()?);
            // 【P0 安全闸】create-secondary 绝不能把目标指向当前正在运行的系统卷。
            //
            // 在这之前唯一挡着的是上面那条 `source != target`，而它可以用「源选别的卷」
            // 绕开：实测 `--operation create-secondary --source-drive T --target-drive C`
            // 会被接受，重启进 RE 后 diskpart 直接格式化正在运行的系统盘。GUI 同样能
            // 构造（源卷选 T:、目标选 C:），因为 GUI 的离线分流只覆盖
            // backup / restore-existing，create-secondary 不走 `%SystemDrive%` 检查。
            //
            // restore-existing **不加**这条：它的目标就是当前系统卷，属于合法场景
            // （重建唯一系统，GUI 会弹窗走 PE/RE 离线链）。
            if options.operation == Operation::CreateSecondary && target_is_active {
                return Err(err(
                    "create-secondary target must not be the active Windows volume",
                ));
            }
            // 【2026-10-18 缺陷修复 H1 第二层：prepare 侧同判据】
            // restore-existing **反过来**：它的目标必须是当前正在运行的系统卷
            // （重建唯一系统）。此前这里只断言 `source == target`，而 A 入口的
            // GUI 目标自动选中会静默失效（下拉停在上一个入口选中的盘），于是
            // 「源＝目标＝某个数据盘」也能一路通到「格式化该盘 + 灌镜像」。
            // GUI 已加同判据拦截，这里是**不依赖 GUI 的最终防线**（CLI 直通、
            // 测试钩子、GUI 绕过都挡在这里）。
            //
            // 前提：`prepare` 只在线上 Windows 会话里跑（GUI/CLI 建任务阶段），
            // 故 `%SystemDrive%` 就是那套正在运行的 Windows；RE 侧执行的是
            // recover/resume，不走本函数。
            if options.operation == Operation::RestoreExisting && !target_is_active {
                return Err(err(
                    "restore-existing target must be the active Windows volume",
                ));
            }
            let path = image_path.ok_or_else(|| err("restore image is missing"))?;
            if !Path::new(path).is_file() {
                return Err(err("restore image does not exist"));
            }
            let _ = capture(
                "dism.exe",
                &[
                    "/Get-WimInfo",
                    &format!("/WimFile:{path}"),
                    &format!("/Index:{}", options.wim_index),
                ],
            )?;
            // 擦除前确认镜像是完整 Windows（纯 PE 走这里会在格式化后才失败）。
            //
            // 【P2】但数据卷还原必须放行：数据卷镜像本来就没有 reagentc.exe / SYSTEM
            // hive，拿完整 Windows 的标准去卡它等于禁掉整个数据卷离线通道。判据用
            // **目标卷**当前有无 SYSTEM hive，与恢复期分叉 BCDBoot 的判据完全一致
            // （`main.rs` 的 data-volume 分支）：目标不是 Windows 卷 → 这次只铺数据、
            // 不碰引导，镜像缺 Windows 标记属正常。
            //
            // 本机此前是靠「挂载报 87 → 镜像名启发式放行」才通过的，换一台 DISM 挂载
            // 可用的机器，数据卷的离线还原会被硬拒（实测路径见 docs 待改清单 P2）。
            //
            // ⚠️ create-secondary 不在放行范围内：装第二系统必须是完整 Windows，
            // 否则产出一个进不了桌面的引导项。
            //
            // ⚠️ 活动系统卷**不做文件探测**：它的 SYSTEM hive 被系统独占，`probe_file`
            // 会 `File::open` 它并撞上共享违例（os error 32），那个错误不是 NotFound、
            // 会直接把准备阶段弄失败。而活动系统卷必然是 Windows 卷，无需探测。
            let target_is_windows = target_is_active
                || crate::pe_safety::target_is_windows_volume(&PathBuf::from(format!(
                    "{}:\\",
                    target.drive_letter.ok_or_else(|| crate::errc(ErrCode::TargetLetterMissing))?
                )))?;
            // 【准备期判定 · 2026-10-22 任务 #110：原「硬拒」已改「路由」】
            // 「系统卷 + 取消格式化（覆盖式应用）」对 DISM 是确定性失败组合
            // （根因见下）；2026-10-22 起不再挡在准备期，改为在此判定并落盘
            // `overlay_no_acl`，恢复期走 wimlib 通道。判定仍必须在
            // 准备期完成：RE 内探测不到宿主卷的 Windows 属性（`%SystemDrive%`
            // 是恢复环境自己的 RAM 盘 X:），与 `is_active_system` 同一理由。
            //
            // 精确根因（2026-10-16 PE 内对照实验钉死，见 docs/20261016-1413 §2 与
            // docs/20261016-1836）：DISM 覆盖式应用要把镜像里
            // `C:\Program Files\WindowsApps\...`（MSIX 应用包目录，owner =
            // NT SERVICE\TrustedInstaller，带 MSIX 专用 ACL）的目录元数据 /
            // Security Descriptor / owner 还原到卷上；该目录受 TrustedInstaller 保护，
            // 操作系统拒绝改写其 owner/ACL → `RestoreDirMetadata:(2418): Access is
            // denied` → `0x80070005` → DISM 立即整体放弃（exit 5）。
            // 实测旁证：`md` 在该目录下能建子目录、`rd` 能删（普通写入允许），
            // **只有"改写既有受保护目录元数据"这一步被拒**；格式化能过是因为受保护
            // 目录被清空，DISM 只需"新建"（允许）而非"改写"。
            //
            // 该约束与 RE/在线/PE 环境无关、与 DISM 版本无关：PE 里同镜像同样失败。
            // `--skip-format` 只对**数据卷**（无 Windows 的卷）有效（数据卷里没有
            // WindowsApps，2026-10-14 已在 T: 验证），对**已含 Windows 的系统卷**无效。
            //
            // 判据用「目标卷当前是否含 Windows」而非「是否活动系统卷」：任何既有
            // Windows 卷都带 TrustedInstaller 保护的目录，都会命中同一根因。
            //
            // 【2026-10-22 任务 #110：硬拒改路由】用户裁定「换 wimlib」
            // 后，本组合不再 `return Err`：置 `overlay_no_acl=true` 落进任务，
            // 恢复期走 wimlib 通道（wimlib 自行获取 SeRestore/SeBackup 提权，
            // RE 内可用；2026-10-23 概念验证 docs/20261022-2308 证明该提权
            // 同时覆盖 SD 还原——镜像目录权限完整还原，个别受保护目录可能
            // 保留卷上现有权限，GUI 确认框明示该语义）。
            // CLI 直跑 --skip-format 视为高级用户显式意图。
            if crate::pe_safety::overlay_restore_on_windows_volume(
                target_is_windows,
                options.format_target,
            ) {
                overlay_no_acl = true;
                append_log(
                    log,
                    crate::i18n::msg("wp_overlay_marked"),
                )?;
            }
            if options.operation == Operation::CreateSecondary || target_is_windows {
                // 降级放行必须留痕：否则事后无法判断「门禁到底跑没跑过」。
                // 2026-10-22 教训——参数错导致权威探测从未生效，而日志里一个字都没有。
                match wim_full_windows_probe(path, options.wim_index, log)? {
                    ProbeOutcome::FullWindows => append_log(
                        log,
                        crate::i18n::msg("wp_precheck_mounted_ok"),
                    )?,
                    ProbeOutcome::QuickPass(name) => append_log(
                        log,
                        &crate::i18n::msg_tpl("wp_precheck_fast_path", &[name.as_str()]),
                    )?,
                    ProbeOutcome::HeuristicPass => append_log(
                        log,
                        crate::i18n::msg("wp_precheck_heuristic"),
                    )?,
                }
            }
            // 配套备份 metadata（.index-N.metadata.json）存在时校验 WIM 完整性；
            // 优先比对物理文件大小，避免在提权准备阶段对 58GB 镜像再次耗时 2 分钟重复读盘计算哈希。
            // 缺失时视为第三方/PE WIM（如安装 WinRE/PE 为第二系统），跳过
            // 哈希校验与目标大小预检——DISM /Get-WimInfo 已确认 WIM 可读。
            let metadata = crate::read_index_metadata(Path::new(path), options.wim_index).ok();
            // 用户显式接受当前镜像时在这里产生例外记录；只有真正越过了副档判据才记，
            // 正常通过校验的镜像不写例外（避免例外退化成默认状态）。
            let mut acceptance: Option<ImageAcceptance> = None;
            if let Some(metadata) = &metadata {
                let file_size = fs::metadata(path)?.len();
                let size_conflict = metadata.image_size > 0 && file_size != metadata.image_size;
                if size_conflict && !options.force_restore_hash {
                    return Err(err("restore image size does not match metadata"));
                }
                // 用户要求：增加一个校验选项框，不勾选时仅对比大小，勾选时对比哈希
                if options.verify_hash {
                    let expected = &metadata.image_sha256;
                    // 这一次读盘是严格哈希模式本来就要付的代价；下面复用同一个值，
                    // 不额外再读一遍 58GB 镜像。
                    let actual = sha256_file(path)?;
                    let hash_conflict = !actual.eq_ignore_ascii_case(expected);
                    if hash_conflict && !options.force_restore_hash {
                        return Err(err("restore image hash does not match metadata"));
                    }
                    if hash_conflict || size_conflict {
                        // 记录"用户看过并接受的就是这份内容"。离线侧据此核验镜像
                        // 未再变化，旧副档只留痕。
                        acceptance = Some(ImageAcceptance {
                            accepted_sha256: actual,
                            accepted_size_bytes: file_size,
                            index: options.wim_index,
                            superseded_sidecar_sha256: Some(expected.clone()),
                            accepted_at: Utc::now(),
                        });
                    }
                } else if size_conflict {
                    // 仅对比大小模式：用户接受的是"当前这个大小"，不为此多读一次全盘哈希。
                    // 留空哈希表示本次例外不覆盖哈希判据，离线侧也只走大小校验。
                    acceptance = Some(ImageAcceptance {
                        accepted_sha256: String::new(),
                        accepted_size_bytes: file_size,
                        index: options.wim_index,
                        superseded_sidecar_sha256: Some(metadata.image_sha256.clone()),
                        accepted_at: Utc::now(),
                    });
                }
            }
            let minimum = metadata
                .as_ref()
                .map(|metadata| {
                    metadata
                        .required_target_size()
                        .max(metadata.source.partition_size)
                })
                .unwrap_or(0);
            if target.partition_size < minimum {
                return Err(err("restore target is too small"));
            }
            Ok(ValidationOutcome {
                acceptance,
                overlay_no_acl,
            })
        }
    }
}

fn image_path_info(
    value: &str,
    require_existing_file: bool,
) -> Result<(String, VolumeIdentity, String), TaskError> {
    validate_absolute_path(value)?;
    let normalized = value.replace('/', "\\");
    let drive = parse_drive(&normalized[..2])?;
    let root = format!(r"{}:\", drive);
    if !normalized[..3].eq_ignore_ascii_case(&root) {
        return Err(err("image path is not rooted on its drive"));
    }
    let relative = normalized[3..].to_string();
    backuprestore_core::validate_relative_path(&relative)?;
    let path = PathBuf::from(&normalized);
    if require_existing_file && !path.is_file() {
        return Err(err(&format!("image does not exist: {normalized}")));
    }
    if !require_existing_file && !path.parent().is_some_and(Path::is_dir) {
        return Err(err(&format!(
            "backup image parent directory does not exist: {normalized}"
        )));
    }
    Ok((normalized, volume_identity(drive)?, relative))
}

fn write_recovery_env(
    path: &Path,
    executable_dir: &Path,
    task: &Task,
    recovery: &VolumeIdentity,
    efi: &VolumeIdentity,
    options: &PrepareOptions,
) -> Result<(), TaskError> {
    let workspace = task
        .workspace_volume
        .as_ref()
        .ok_or_else(|| err("workspace is missing"))?;
    let source = task
        .source
        .as_ref()
        .ok_or_else(|| err("source is missing"))?;
    let task_root_rel = format!(
        "{}\\tasks\\{}",
        workspace_relative(executable_dir)?,
        task.task_id
    );
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    put_value(&mut values, "TASK_ID", task.task_id.clone());
    put_value(
        &mut values,
        "OPERATION",
        operation_name(task.operation).to_string(),
    );
    insert_identity(&mut values, "WORKSPACE", workspace);
    put_value(&mut values, "WORKSPACE_ROOT_REL", task_root_rel);
    insert_identity(&mut values, "RECOVERY", recovery);
    insert_identity(&mut values, "SOURCE", source);
    if options.test_fault.as_deref() == Some("identity-env-mismatch") {
        put_value(&mut values, "SOURCE_VOLUME_SERIAL", "FAULT-INJECTED".into());
    }
    if let Some(fault) = &options.test_fault {
        put_value(&mut values, "TEST_FAULT", fault.clone());
    }
    if let Some(source_drive) = source.drive_letter {
        let source_free = volume_free_bytes(source_drive)?;
        put_value(
            &mut values,
            "SOURCE_USED_BYTES",
            source
                .partition_size
                .saturating_sub(source_free)
                .to_string(),
        );
    }
    put_value(&mut values, "RESERVED_BYTES", RESERVED_BYTES.to_string());
    if task.operation == Operation::Backup {
        put_value(
            &mut values,
            "MINIMUM_TARGET_SIZE",
            source.partition_size.to_string(),
        );
    }
    if let Ok(computer) = env::var("COMPUTERNAME") {
        put_value(&mut values, "COMPUTERNAME", computer);
    }
    insert_identity(&mut values, "EFI", efi);
    if let Some(destination) = task.destination.as_ref() {
        insert_identity(&mut values, "IMAGE", &destination.volume);
        put_value(
            &mut values,
            "IMAGE_ABSOLUTE_PATH",
            destination.absolute_path.clone().unwrap_or_default(),
        );
        put_value(
            &mut values,
            "IMAGE_RELATIVE_PATH",
            destination.relative_path.clone(),
        );
    }
    if let Some(image) = task.image.as_ref() {
        insert_identity(&mut values, "IMAGE", &image.volume);
        put_value(
            &mut values,
            "IMAGE_ABSOLUTE_PATH",
            image.absolute_path.clone().unwrap_or_default(),
        );
        put_value(
            &mut values,
            "IMAGE_RELATIVE_PATH",
            image.relative_path.clone(),
        );
        put_value(&mut values, "IMAGE_SHA256", image.sha256.clone());
        put_value(&mut values, "WIM_INDEX", image.index.to_string());
    }
    if let Some(target) = task.target.as_ref() {
        insert_identity(&mut values, "TARGET", &target.volume);
        put_value(
            &mut values,
            "MINIMUM_TARGET_SIZE",
            target.minimum_size_bytes.to_string(),
        );
    }
    put_value(
        &mut values,
        "ALLOW_DESTRUCTIVE",
        if options.allow_destructive {
            "YES"
        } else {
            "NO"
        }
        .into(),
    );
    put_value(
        &mut values,
        "BOOT_MENU_NAME",
        options.boot_menu_name.clone(),
    );
    // 恢复环境失败处理策略 + 界面语言（2026-10-16 用户拍板做成设置项）。
    // 键名与取值都来自 `crate::failure_policy`，恢复环境读同一份常量。
    for (key, value) in options.failure_policy.env_pairs() {
        put_value(&mut values, key, value);
    }
    // DISM 等待上限（2026-10-16 审查补上）。只在非默认时写一行，避免给每个
    // 任务都多一行没有信息量的 0。
    if let Some((key, value)) = crate::dism_timeout::env_pair(options.dism_timeout_secs) {
        put_value(&mut values, key, value);
    }
    put_value(&mut values, "UI_LANGUAGE", options.ui_language.clone());
    put_value(&mut values, "TASK_CREATED", Utc::now().to_rfc3339());
    put_value(
        &mut values,
        "PROGRAM_VERSION",
        backuprestore_core::PROGRAM_VERSION.into(),
    );
    let mut text = String::new();
    for (key, value) in values {
        if value.contains(['\r', '\n', '=']) {
            return Err(err(&format!(
                "recovery environment value is invalid: {key}"
            )));
        }
        text.push_str(&format!("{key}={value}\r\n"));
    }
    fs::write(path, text)?;
    Ok(())
}

fn put_value(values: &mut BTreeMap<String, String>, key: &str, value: String) {
    values.insert(key.to_string(), value);
}

pub(crate) fn insert_identity(
    values: &mut BTreeMap<String, String>,
    prefix: &str,
    identity: &VolumeIdentity,
) {
    let insert = |suffix: &str, value: String, values: &mut BTreeMap<String, String>| {
        values.insert(format!("{prefix}_{suffix}"), value);
    };
    insert("VOLUME_GUID", identity.volume_guid.clone(), values);
    insert("DISK_GUID", identity.disk_guid.clone(), values);
    insert("PARTITION_GUID", identity.partition_guid.clone(), values);
    insert(
        "DISK_NUMBER",
        identity.disk_number.unwrap_or_default().to_string(),
        values,
    );
    insert(
        "PARTITION_NUMBER",
        identity.partition_number.unwrap_or_default().to_string(),
        values,
    );
    insert(
        "PARTITION_OFFSET",
        identity.partition_offset.to_string(),
        values,
    );
    insert(
        "PARTITION_SIZE",
        identity.partition_size.to_string(),
        values,
    );
    insert(
        "PARTITION_TYPE_GUID",
        identity.partition_type_guid.clone(),
        values,
    );
    insert("FILESYSTEM", identity.filesystem.clone(), values);
    insert("VOLUME_SERIAL", identity.volume_serial.clone(), values);
}

fn append_env(path: &Path, values: &[(&str, &str)]) -> Result<(), TaskError> {
    let mut file = OpenOptions::new().append(true).open(path)?;
    for (key, value) in values {
        writeln!(file, "{key}={value}")?;
    }
    file.sync_all()?;
    Ok(())
}

fn write_status_env(task_dir: &Path, task: &Task, stage: &str) -> Result<(), TaskError> {
    fs::write(
        task_dir.join("status.env"),
        format!(
            "task_id={}\r\nstage={stage}\r\nprogress={}\r\noperation={}\r\n",
            task.task_id,
            if stage == "boot-requested" { 2 } else { 0 },
            operation_name(task.operation),
        ),
    )?;
    Ok(())
}

fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::Probe => "probe",
        Operation::Backup => "backup",
        Operation::RestoreExisting => "restore-existing",
        Operation::CreateSecondary => "create-secondary",
    }
}

fn assert_environment(source_drive: char) -> Result<(), TaskError> {
    // 数据卷（无 SYSTEM hive）可在当前 Windows 在线备份/还原，无需恢复环境；
    // 仅系统卷（存在 SYSTEM hive）要求 WinRE 可用（备份/还原在 PE/RE 中执行）。
    if Path::new(&format!(
        r"{}:\Windows\System32\config\SYSTEM",
        source_drive
    ))
    .is_file()
    {
        // `reagentc /info` 有一个上下文级失败模式（退出码 9，DISM 写不了
        // `C:\Windows\Logs\DISM` 日志），**不代表 WinRE 真不可用**。所以：
        // /info 成功 → 按原判据（须报 GLOBALROOT 或 Recovery\WindowsRE）；
        // /info 失败 → 退回 BCD 能否推出 WinRE 卷来判"是否已注册"，判不出才拒绝。
        let winre_available = match capture("reagentc.exe", &["/info"]) {
            Ok(reagent) => reagent.contains("GLOBALROOT") || reagent.contains("Recovery\\WindowsRE"),
            Err(_) => recovery_identity_from_bcd().is_ok(),
        };
        if !winre_available {
            return Err(err("Windows RE is disabled or unavailable"));
        }
    }
    Ok(())
}

/// 【2026-10-17 第二轮核查，日志缺口 6】`fltmc` 的输出落盘。
///
/// 此前三个流全是 `Stdio::null()`，只按退出码判断 —— 提权失败时用户看到
/// 「需要管理员权限」，而日志里**看不出到底是没提权、还是 fltmc 本身不可用**
/// （后者在某些精简环境里会出现，此时真正的修法完全不同）。
fn require_administrator(log: &Path) -> Result<(), TaskError> {
    // 【2026-10-18 缺陷修复 M10】先跑真正的权限判据（`fltmc` 的退出码），
    // 日志写入单独容错。此前用 `capture_logged`：它第一行就是
    // `append_log(log, …)?`，一旦日志不可写（目录满 / ACL 限制 / `logs_dir`
    // 回退到不可写目录），错误会在 `fltmc` **之前**就返回，被这里一律翻成
    // 「administrator elevation is required」——用户反复提权仍然失败，
    // 真正的原因（日志写不了）被彻底带偏。
    // 用不写日志的 `capture`（非零退出即 Err），把「提权不够」与「日志写不了」
    // 两种失败分开：只有前者才报「需要管理员」。
    match capture("fltmc.exe", &[]) {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = append_log(log, &format!("administrator check failed: {error}"));
            Err(err("administrator elevation is required"))
        }
    }
}

fn require_windows_volume(identity: &VolumeIdentity, name: &str) -> Result<(), TaskError> {
    if identity.is_reserved_partition() || !identity.filesystem.eq_ignore_ascii_case("NTFS") {
        return Err(err(&format!("{name} must be a non-reserved NTFS volume")));
    }
    Ok(())
}

fn assert_bitlocker_off(letter: char) -> Result<(), TaskError> {
    // `-protectionaserrorlevel` has inconsistent exit-code semantics across
    // Windows releases. Parse the inbox status report conservatively instead:
    // only a clearly unencrypted (0.0%) volume is allowed. Any other result,
    // including an unreadable or partially encrypted report, is rejected.
    let output = capture("manage-bde.exe", &["-status", &format!("{letter}:")])?;
    if output.contains("0.0%") && !output.contains("100.0%") {
        Ok(())
    } else {
        Err(err(&format!(
            "BitLocker protection is enabled or volume state is unknown on {letter}:"
        )))
    }
}

fn ensure_workspace_capacity(
    workspace: &VolumeIdentity,
    recovery: &VolumeIdentity,
) -> Result<(), TaskError> {
    let workspace_letter = workspace
        .drive_letter
        .ok_or_else(|| err("workspace volume has no drive letter"))?;
    let recovery_letter = recovery
        .drive_letter
        .ok_or_else(|| err("Recovery volume has no drive letter"))?;
    let registered_wim = PathBuf::from(format!(
        r"{}:\Recovery\WindowsRE\Winre.wim",
        recovery_letter
    ));
    let wim_size = fs::metadata(&registered_wim)
        .map_err(|_| err("registered WinRE image is unavailable for workspace capacity check"))?
        .len();
    let required = wim_size.saturating_mul(2).saturating_add(256 * 1024 * 1024);
    let available = volume_free_bytes(workspace_letter)?;
    if available < required {
        return Err(err(&format!(
            "program directory volume has insufficient free space for WinRE staging: {available} < {required}"
        )));
    }
    Ok(())
}

/// 备份前镜像卷空间预检：镜像所在卷可用空间低于硬闸 → 带码拒绝。
///
/// 与在线通道（`online_operation::capture`）用的是同一对 core 纯函数，保证两条
/// 链阈值一致。读不到卷空间（卷不存在/权限）时**静默放过**：预检只是提前止损，
/// 不是权威判据，真正的权威是捕获过程本身与它的回滚。
fn ensure_backup_image_capacity(
    source_letter: char,
    image_path: &str,
    compress: &str,
) -> Result<(), TaskError> {
    let image_letter = match Path::new(image_path)
        .parent()
        .and_then(|parent| parent.to_string_lossy().chars().next())
        .filter(|c| c.is_ascii_uppercase())
    {
        Some(letter) => letter,
        // 非盘符路径（UNC 等）：无法判定卷，放过。
        None => return Ok(()),
    };
    let (source_total, source_free, available) = match (
        disk_free_space(source_letter),
        volume_free_bytes(image_letter),
    ) {
        (Ok((total, free)), Ok(available)) => (total, free, available),
        _ => return Ok(()),
    };
    let used = source_total.saturating_sub(source_free);
    // 判定与阈值都来自 core 的同一对纯函数（与在线通道共用），这里不写不等式——
    // 抄两遍就没法给「拦下」写判据（见 backup_space_shortfall 的注释）。
    if let Some(floor) = backuprestore_core::backup_space_shortfall(available, used) {
        return Err(crate::errc_with(
            ErrCode::BackupSpaceInsufficient,
            &[
                &crate::text_parsing::format_bytes(Some(available)),
                &crate::text_parsing::format_bytes(Some(floor)),
                &crate::text_parsing::format_bytes(Some(used)),
            ],
        ));
    }
    // 低于保守估计只做记录不做拦截（估计值本身对含大量媒体的数据卷可能偏低）。
    let estimated = backuprestore_core::estimate_backup_bytes(used, compress);
    if available < estimated {
        let dir = crate::log_root::logs_dir();
        {
            let _ = append_log(
                &dir.join("prepare.log"),
                &crate::i18n::msg_tpl(
                    "ol_backup_space_warn",
                    &[
                        &crate::text_parsing::format_bytes(Some(available)),
                        &crate::text_parsing::format_bytes(Some(estimated)),
                        &crate::text_parsing::format_bytes(Some(used)),
                    ],
                ),
            );
        }
    }
    Ok(())
}

fn executable_dir() -> Result<PathBuf, TaskError> {
    env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| err("cannot determine executable directory"))
}

fn drive_from_path(path: &Path) -> Result<char, TaskError> {
    let raw = path.to_string_lossy();
    let bytes = raw.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return Err(err("program directory must use a drive-root path"));
    }
    Ok((bytes[0] as char).to_ascii_uppercase())
}

fn workspace_relative(path: &Path) -> Result<String, TaskError> {
    let raw = path.to_string_lossy().replace('/', "\\");
    if raw.len() < 4 {
        return Err(err("program directory is invalid"));
    }
    let relative = raw[3..].trim_matches('\\');
    if relative.is_empty() {
        return Err(err("program directory cannot be a volume root"));
    }
    backuprestore_core::validate_relative_path(relative)?;
    Ok(relative.to_string())
}

fn capture(program: &str, args: &[&str]) -> Result<String, TaskError> {
    crate::process_runner::capture_output(program, args)
}

fn recovery_identity() -> Result<VolumeIdentity, TaskError> {
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
fn recovery_identity_from_bcd() -> Result<VolumeIdentity, TaskError> {
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

fn efi_identity(override_drive: Option<char>) -> Result<VolumeIdentity, TaskError> {
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

fn identity_from_diskpart(
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

fn identity_drive_candidates(preferred: char) -> impl Iterator<Item = char> {
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
fn mountvol_letter_shows_volume(letter: char) -> bool {
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

fn is_drive_letter_available(letter: char) -> bool {
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
fn mounted_letter_for_identity(identity: &VolumeIdentity) -> Option<char> {
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
fn mount_volume_guid(volume_guid: &str, preferred: char) -> Result<char, TaskError> {
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
    fn new(letter: char, created: bool, log: &Path) -> Self {
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
    /// ★ 这里**不能**加 `#[cfg(debug_assertions)]`：`debug_assert!` 展开成
    ///   `if cfg!(debug_assertions) { assert!(…) }`，条件表达式在 release 下
    ///   照样要过类型检查——加了 cfg 就会 `E0599 method not found`（当场踩到）。
    pub(crate) fn owns_no_letter(&self) -> bool {
        self.letter.is_none()
    }

    /// 显式声明：这个盘符要活到**本进程结束**。
    ///
    /// 给「字母确实会逸出」的调用点用——写进持久化方案、被拼进返回的路径字符串、
    /// 存进结构体字段。这些地方没法让租约跟着作用域走，但也不能就这么放开不管。
    ///
    /// ★ 这是**有边界的**逃生口，不是「不管了」：
    ///   - 字母进进程级集合，`main()` 收尾统一 `mountvol /D`；
    ///   - 进程被强杀/崩溃 → 磁盘上的租约登记（带 PID）由**下次启动**按「拥有者已死」
    ///     精确清理，不会误伤并存实例。
    ///   ★ 它是可 grep 的：`grep hold_for_process` 就能列出全部「字母会逸出」的地方。
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
//
// ★ 为什么还要这一层（2026-10-27/28 两轮事故的结论）：
//   作用域租约解决「正常路径不忘撤」；但进程被 `taskkill /F`、崩溃、断电时，
//   Drop 不会跑，盘符就永久留下。所以**分配时**在磁盘上登记一行 `<盘符> <PID>`，
//   正常撤销时划掉；**下次启动**只清理「登记还在但拥有者进程已死」的那些。
//   用 PID 而不是时间戳，是为了不误伤并存的另一个实例（GUI 会派生 CLI 去准备）。

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
///
/// 判活走 `OpenProcess` + `GetExitCodeProcess`：拿不到句柄（进程不存在）或退出码
/// 不是 `STILL_ACTIVE` 都算已死。**并存的活实例绝不会被误清**。
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

/// 该 PID 是否仍是活着的进程（委托给 `windows_command`：那里已经声明了同一批
/// Win32 符号，重复声明会触发 `redeclared with a different signature`）。
fn process_still_alive(pid: u32) -> bool {
    crate::windows_command::process_is_alive(pid)
}

/// 挂载一个卷并把盘符交给调用方，**随租约返回**：本次若新分配了盘符，
/// 租约离开作用域就 `mountvol /D` 撤销（或 `hold_for_process()` 延到进程结束）。
///
/// ★ 这是全库**唯一**的挂载入口（2026-10-28 结构收敛）。原来还有一个不返回租约的
///   `ensure_volume_mounted`，只要它公开，「挂了不管」就永远可能被写出来——本机
///   6 条幽灵盘符、测试夹具每次留一条，都是这么来的。约定靠人记不住，改成编译器记。
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
    // Prefer reusing a mount point that already exists. The registered WinRE
    // very often lives on the OS partition, which Windows already exposes, so
    // this is both the common case and the cheapest one.
    if let Some(letter) = mounted_letter_for_identity(identity) {
        return Ok((letter, false));
    }
    // Attach the volume by its GUID path. This is the only strategy that works
    // for identities without numeric disk/partition coordinates (imported or
    // legacy tasks) and for hidden recovery partitions DiskPart refuses to
    // expose by number.
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
    // diskpart `assign letter=` 同样是持久写入，所以这里也算「本次新分配」。
    let letter = mounted
        .drive_letter
        .ok_or_else(|| err("mounted volume has no drive letter"))?;
    Ok((letter, true))
}

fn extract_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    text.split_once(marker).map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_options_reject_missing_image_or_invalid_drive() {
        assert!(
            parse_prepare_options(vec![
                "--operation".into(),
                "probe".into(),
                "--source-drive".into(),
                "C".into(),
            ])
            .is_ok()
        );
        assert!(
            parse_prepare_options(vec![
                "--operation".into(),
                "restore-existing".into(),
                "--source-drive".into(),
                "C".into(),
            ])
            .is_err()
        );
        assert!(
            parse_prepare_options(vec![
                "--operation".into(),
                "probe".into(),
                "--source-drive".into(),
                "CC".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn prepare_fault_options_require_efi_only_for_efi_faults() {
        let base = vec![
            "--operation".into(),
            "probe".into(),
            "--source-drive".into(),
            "P".into(),
        ];
        let mut power_loss = base.clone();
        power_loss.extend(["--test-fault".into(), "power-loss-window".into()]);
        assert!(
            parse_prepare_options(power_loss).is_ok(),
            "power-loss-window uses the real system EFI by default"
        );

        for fault in ["identity-env-mismatch", "bcdboot-failure"] {
            let mut missing_efi = base.clone();
            missing_efi.extend(["--test-fault".into(), fault.into()]);
            assert!(
                parse_prepare_options(missing_efi).is_err(),
                "{fault} must require an explicit development EFI"
            );
        }
        for fault in [
            "power-loss-target-erased",
            "power-loss-image-applied",
            "power-loss-boot-repaired",
        ] {
            let mut system_efi_fault = base.clone();
            system_efi_fault.extend(["--test-fault".into(), fault.into()]);
            assert!(
                parse_prepare_options(system_efi_fault).is_ok(),
                "{fault} must use the current system EFI by default"
            );
        }
    }
}

