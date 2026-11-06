//! 恢复运行宿主引擎（Recovery Engine）。
//!
//! 承载 WinRE / PE 环境下的主恢复执行循环、状态机转移、
//! DISM / wimlib 还原驱动、引导项清理与终态收敛。

use std::collections::BTreeMap;
#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::Arc;

use backuprestore_core::{Stage, Task, TaskError, TaskStore};
#[cfg(windows)]
use backuprestore_core::{
    BootMode, Operation, PayloadManifest, StatusRecord,
    read_json, verify_image_file,
};

use crate::append_log;
use crate::err;
#[allow(unused_imports)]
use crate::error_code::{ErrCode, errc, errc_with};
#[cfg(windows)]
use crate::process_runner::{run_logged, run_logged_timeout};
#[cfg(windows)]
use crate::recovery_volume::*;
#[cfg(windows)]
use crate::recovery_bcd::*;
#[cfg(windows)]
use crate::text_parsing::MountedVolumes;
#[cfg(windows)]
use crate::{
    boot_entry, pe_safety, recovery_fault, recovery_progress, resume_safety,
    secondary_registry, windows_prepare, winre_registration,
};
#[cfg(windows)]
use crate::pending_task::stage_resumable_after_interruption;
#[cfg(windows)]
use crate::read_index_metadata;

pub(crate) use crate::recovery_interactive::*;
#[allow(unused_imports)]
pub(crate) use crate::recovery_backup::*;

#[cfg(not(windows))]
pub(crate) fn recover_env(_path: String) -> Result<(), TaskError> {
    Err(err(
        "recover-env is only available on Windows/WinRE; use recover --dry-run on this host",
    ))
}

#[cfg(windows)]
pub(crate) fn recover_env(path: String) -> Result<(), TaskError> {
    use backuprestore_core::{validate_payload_files, validate_task_id};

    // === 持久早期日志 ===
    // 【2026-10-18 缺陷修复 L4】此前硬编码 `C:\…` 与 `C:\WinRE-PoC\…`：
    // WinRE 里 C: 常常正是**离线 Windows = 还原目标卷**，这些「跨重启可回读」
    // 的日志会在随后的格式化中被一起抹掉，与 v2.6.15 定下的「日志根不落在
    // PE、不落在目标卷」（`log_root::logs_dir()`）自相矛盾。现在统一走日志根。
    let persistent_logs = [crate::log_root::logs_dir().join("recovery-early.log")];
    let meta_log = |line: &str| {
        use std::io::Write;
        for p in &persistent_logs {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
            {
                let _ = writeln!(f, "{line}");
            }
        }
    };
    meta_log(&format!("=== recover-env START path='{path}' ==="));

    let values = match read_env_file(&path) {
        Ok(v) => v,
        Err(e) => {
            meta_log(&format!("read_env_file FAILED: {e}"));
            return Err(e);
        }
    };
    let task_id = env_required(&values, "TASK_ID")?;
    validate_task_id(&task_id)?;
    // 【2026-10-16 审查修订，卡死 2/3/6】把任务级的 DISM 等待上限灌进进程环境变量，
    // 让本进程内所有 DISM 调用（`pe_operation::command` 的 long 分支、
    // `online_operation::dism`、`native_gui` 的 PE 安装）都能读到。
    // 恢复环境是**无人值守**的：只能靠任务自带的值，不能指望谁记得设环境变量。
    // 值为 0（默认/未设置）时同样显式设一次，避免父进程遗留的旧值干扰。
    {
        let secs = crate::dism_timeout::from_values(&values);
        // set_var 在 2024 edition 里是 unsafe（多线程并发修改环境不安全）；
        // 此刻还在单线程入口，且必须在任何子进程派生之前完成。
        unsafe { std::env::set_var(crate::dism_timeout::ENV_VAR, secs.to_string()) };
        meta_log(&format!("DISM wait limit: {secs} sec (0 = unlimited)"));
    }
    // 【2026-10-17 修复】暂停总上限同样灌进进程环境变量。此前
    // `recovery_progress::wait_if_paused` 读的 `PAUSE_TIMEOUT_ENV_VAR` 全仓没有
    // 写入方 → 永久 0（不限制），用户在设置里配的 `failure_pause_max_wait_secs`
    // 对「暂停后无人点继续」这一路完全不生效；只有失败弹框那条路
    // （`MessageBoxTimeoutW`）用到了它。这里让两条路**同源**。
    {
        let policy = crate::failure_policy::FailurePolicy::from_values(&values);
        let secs = policy.pause_timeout_env_value();
        // 0 也要显式写：避免父进程遗留的旧值干扰（与 DISM 同一个理由）。
        unsafe { std::env::set_var(crate::failure_policy::PAUSE_TIMEOUT_ENV_VAR, secs.to_string()) };
        meta_log(&format!("pause total limit: {secs} sec (0 = unlimited)"));
    }
    let workspace_root_rel = env_required(&values, "WORKSPACE_ROOT_REL")?;
    let store_rel = split_workspace_root_rel(&workspace_root_rel, &task_id)?;
    meta_log(&format!(
        "TASK_ID={task_id} workspace_root_rel={workspace_root_rel}"
    ));
    // Until the workspace volume is mounted only an ephemeral WinRE path is
    // available.  Switch to the task directory immediately after mounting;
    // all task/recovery logs that survive WinRE are stored there.
    let mut early_log = PathBuf::from(r"X:\BackupRestore-Recovery-early.log");
    let operation_hint = values.get("OPERATION").map(|s| s.as_str());
    // WinRE 恢复进度窗口（失败静默降级，非关键）：GUI 显示阶段/DISM 进度/日志尾部。
    let progress = recovery_progress::spawn(early_log.clone(), operation_hint);
    // 每次重试拥有自己的窗口，任何提前返回都关闭旧窗口。
    struct ProgressGuard(Arc<recovery_progress::ProgressShared>);
    impl Drop for ProgressGuard {
        fn drop(&mut self) {
            recovery_progress::close_and_wait(&self.0);
        }
    }
    let _progress_guard = ProgressGuard(Arc::clone(&progress));
    // One registry for every role mounted in this WinRE session, so two roles
    // that resolve to the same volume share its letter instead of stealing it
    // from each other.
    let mut mounts = MountedVolumes::new();
    let task_letter =
        match mount_env_volume(&mut mounts, &values, "WORKSPACE", 'T', &early_log, false) {
            Ok(letter) => {
                meta_log(&format!("mount WORKSPACE OK -> {letter}:"));
                letter
            }
            Err(e) => {
                meta_log(&format!("mount WORKSPACE FAILED: {e}"));
                return Err(e);
            }
        };
    let store = TaskStore::new(PathBuf::from(format!(r"{}:\{store_rel}", task_letter)));
    let task_dir = store.task_dir(&task_id)?;
    early_log = task_dir.join("Recovery-early.log");
    // RAM 盘初始化日志也落到任务目录，便于跨重启核验设备初始化。
    if let Ok(system_root) = std::env::var("SystemRoot") {
        for (source, name) in [
            ("Temp/BackupRestore-init.log", "WinPE-init.log"),
            ("System32/wpeinit.log", "wpeinit.log"),
        ] {
            let source = PathBuf::from(&system_root).join(source);
            if source.is_file() {
                fs::copy(source, task_dir.join(name))?;
            }
        }
    }
    // 早期日志切到任务目录后，进度窗口跟随新日志并重置读取偏移。
    progress.switch_log_path(early_log.clone(), 0);
    let mut task = store.load(&task_id)?;
    meta_log(&format!(
        "task loaded status={:?} operation={:?}",
        task.status, task.operation
    ));
    if task.status == Stage::RecoveryComplete {
        // 清理中断后的重入仅清理，不依赖已删除的载荷文件，也绝不再次格式化。
        recovery_fault::configure(values.get("TEST_FAULT").map(String::as_str), &task_dir);
        complete_recovery_cleanup(&store, &mut task, &early_log)?;
        return run_logged("wpeutil.exe", &["reboot"], &early_log);
    }
    if task.status == Stage::Success {
        return run_logged("wpeutil.exe", &["reboot"], &early_log);
    }
    if task.status == Stage::Failed {
        return Err(err(&format!(
            "task is already terminal at stage {:?}; refusing to run it again",
            task.status
        )));
    }
    // 这里曾经在入口立即把注册 WinRE 覆写回干净原件（commits fcb48a1），已于
    // 2026-09-28 回退：那样会让整个会话的注册位都不是注入件，而断电续跑要求重启后
    // bootmgr 加载的注册位必须仍是注入件（含 winpeshl → 自动拉起 Recovery.exe），
    // 否则重启会落进微软原版 WinRE、任务不会自动继续。
    //
    // 注册位只有一处，却要同时扮演「载荷宿主」与「捕获纯净」两个互斥角色，且不能用
    // 第二宿主位置绕开（WinRE 强校验启动路径 == ReAgent 注册位置），因此只能按时间
    // 分片：任务存活期保持注入件，只在 DISM 捕获前那一刻翻成干净原件
    // （见 `restore_clean_winre_before_capture`），任务终结时再还原一次。
    // 完整设计与场景矩阵：docs/20260928-0745-registered-winre-policy-and-resume-design.md
    // 主日志默认在任务目录；挂载镜像卷后（见下）切换到镜像同目录 Recovery.log，
    // 方便用户在 WIM 旁直接查看。workspace_log 保留给 GUI 状态报告读取。
    let workspace_log = store.log_path(&task_id)?;
    let mut log = workspace_log.clone();
    let mut efi_root = None;
    // WinRE cleanup is part of the recovery contract.  Defer the terminal
    // success write until the original registered WinRE image has been
    // restored and verified below.
    let result = (|| -> Result<(), TaskError> {
        let persisted_status: StatusRecord = read_json(store.status_path(&task_id)?)?;
        if !persisted_status.task_id.eq_ignore_ascii_case(&task.task_id) {
            return Err(err("status.json task_id does not match task.json"));
        }
        if persisted_status.operation != task.operation {
            return Err(err("status.json operation does not match task.json"));
        }
        // The normal-host preparation records boot-requested in status.json
        // without rewriting the payload task.json. Accept exactly that one-step
        // ahead state; any other divergence indicates a torn or tampered task.
        if persisted_status.stage != task.status
            && !(task.status == Stage::Prepared && persisted_status.stage == Stage::BootRequested)
        {
            return Err(err("status.json stage does not match task.json"));
        }
        verify_task_identity_env(&values, &task)?;
        let manifest: PayloadManifest = read_json(task_dir.join("manifest.json"))?;
        if !manifest.task_id.eq_ignore_ascii_case(&task.task_id) {
            return Err(err("payload manifest task_id does not match task.json"));
        }
        let recovery_exe = task_dir.join("payload").join("Recovery.exe");
        let task_json = task_dir.join("payload").join("task.json");
        let recovery_task_env = task_dir.join("payload").join("RecoveryTask.env");
        let original = task_dir.join("original").join("Winre.wim");
        let staged = task_dir.join("stage").join("Winre.wim");
        if !recovery_exe.is_file() {
            return Err(err("Recovery.exe is required for every WinRE operation"));
        }
        verify_running_recovery_binary(&manifest)?;
        validate_payload_files(
            &manifest,
            &recovery_exe,
            &task_json,
            &recovery_task_env,
            &original,
            &staged,
        )?;
        // The payload task is the immutable copy staged before the one-time
        // boot request. Compare every field except the durable stage: after a
        // real interruption the workspace task is legitimately at
        // TargetErased/ImageApplied/BootRepaired while the immutable payload
        // remains Prepared.
        let payload_task: Task = read_json(&task_json)?;
        payload_task.validate()?;
        let mut comparable_payload = payload_task;
        let mut comparable_workspace = task.clone();
        // Drive letters are intentionally ephemeral.  WinRE remounts the
        // same GUID-identified volumes using its own letters (for example
        // P:/Q: become G:/H:).  Comparing those letters made every
        // interrupted task fail before it could resume, despite all stable
        // partition identities being identical.
        clear_task_drive_letters(&mut comparable_payload);
        clear_task_drive_letters(&mut comparable_workspace);
        comparable_payload.status = comparable_workspace.status;
        if comparable_payload != comparable_workspace {
            return Err(err(
                "payload task does not match the workspace task prepared for this recovery",
            ));
        }

        recovery_fault::configure(values.get("TEST_FAULT").map(String::as_str), &task_dir);
        // 先完成任务和不可变载荷校验，再判断哪些角色确实受本任务格式化影响。
        let recovery_letter = mount_env_volume(
            &mut mounts,
            &values,
            "RECOVERY",
            'R',
            &early_log,
            resume_safety::role_was_erased(&task, &values, "RECOVERY"),
        )?;
        meta_log(&format!("mount RECOVERY OK (r={recovery_letter})"));
        let efi_letter = if task.operation != Operation::Probe {
            let reformatted_ok = resume_safety::role_was_erased(&task, &values, "SOURCE");
            let source_letter = mount_env_volume(
                &mut mounts,
                &values,
                "SOURCE",
                'S',
                &early_log,
                reformatted_ok,
            )?;
            let image_letter =
                mount_env_volume(&mut mounts, &values, "IMAGE", 'I', &early_log, false)?;
            if let Some(source) = task.source.as_mut() {
                source.drive_letter = Some(source_letter);
            }
            if let Some(image) = task.image.as_mut() {
                image.volume.drive_letter = Some(image_letter);
            }
            // 日志放到镜像同目录（如 E:\Recovery.log），与 WIM 并排便于查看；
            // 写失败则回退任务目录日志，不影响恢复流程。
            // 备份任务用 destination 记录镜像，还原任务用 image。
            let image_relative = match &task.destination {
                Some(spec) => Some(spec.relative_path.clone()),
                None => task.image.as_ref().map(|spec| spec.relative_path.clone()),
            };
            if let Some(relative) = image_relative {
                let parent = Path::new(&relative.replace('\\', "/"))
                    .parent()
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_default();
                let candidate = if parent.is_empty() || parent == "/" {
                    PathBuf::from(format!("{image_letter}:\\Recovery.log"))
                } else {
                    PathBuf::from(format!(
                        "{image_letter}:\\{}\\Recovery.log",
                        parent.trim_matches('/')
                    ))
                };
                // 候选日志若是新文件，把任务目录已有的早期日志一并复制过去，
                // 保证镜像旁的 Recovery.log 内容完整。
                if !candidate.exists() && workspace_log.exists() {
                    let _ = fs::copy(&workspace_log, &candidate);
                }
                let candidate_offset = fs::metadata(&candidate).map(|m| m.len()).unwrap_or(0);
                match append_log(&candidate, "Recovery log continued in image directory") {
                    Ok(_) => {
                        log = candidate;
                        // 主日志切到镜像同目录（如 E:\Recovery.log）后，进度窗口跟随追加点。
                        progress.switch_log_path(log.clone(), candidate_offset);
                    }
                    Err(error) => {
                        let _ = append_log(
                            &log,
                            &format!(
                                "image-directory log unavailable ({error}); keeping workspace log"
                            ),
                        );
                    }
                }
            }
            if let Some(destination) = task.destination.as_mut() {
                destination.volume.drive_letter = Some(image_letter);
            }
            if task.target.is_some() {
                let target_letter = mount_env_volume(
                    &mut mounts,
                    &values,
                    "TARGET",
                    'W',
                    &early_log,
                    matches!(
                        task.status,
                        Stage::TargetErased | Stage::ImageApplied | Stage::BootRepaired
                    ),
                )?;
                // WinRE may reuse E: for an image/data volume; keep EFI on a
                // late temporary letter to avoid mount collisions.
                let efi_letter =
                    mount_env_volume(&mut mounts, &values, "EFI", 'Z', &early_log, false)?;
                if let Some(target) = task.target.as_mut() {
                    target.volume.drive_letter = Some(target_letter);
                }
                Some(efi_letter)
            } else {
                None
            }
        } else if let Some(source) = task.source.as_mut() {
            // Probe tasks are allowed to keep their task files on the source
            // partition. Reuse the verified workspace mount when both names
            // identify the same partition.
            let workspace_volume = task
                .workspace_volume
                .as_ref()
                .ok_or_else(|| err("probe task is missing workspace volume identity"))?;
            if !workspace_volume.same_partition(source) {
                source.drive_letter = Some(mount_env_volume(
                    &mut mounts,
                    &values,
                    "SOURCE",
                    'S',
                    &early_log,
                    false,
                )?);
            } else {
                source.drive_letter = Some(task_letter);
                append_log(
                    &early_log,
                    &format!("Probe source is the task partition; reusing {task_letter}:"),
                )?;
            }
            None
        } else {
            None
        };
        efi_root = efi_letter.map(|letter| PathBuf::from(format!(r"{}:\", letter)));
        append_log(
            &log,
            &format!(
                "Recovery.exe started from env task={} operation={:?}",
                task_id, task.operation
            ),
        )?;
        meta_log(&format!(
            "reaching recover_windows (operation={:?})",
            task.operation
        ));
        recover_windows(
            &store,
            &mut task,
            &log,
            efi_root.as_deref(),
            Some(&values),
            false,
            Some(&progress),
        )
    })();
    if let Err(error) = result {
        // 【2026-10-23 实机发现】用户按过 B 回退时，续跑链校验必然失败（BCD 被整份
        // 换回准备期快照，任务自建的 loader/device options 随之消失），但此前现场
        // 只剩一句 `invalid task: 续跑启动链字段不符：device`，看不出是用户自己按了 B。
        // 这里把因果补进日志与错误文案——**只改说明，不改行为**：失败仍按原路径
        // 收口（保留现场、可重试），避免掩盖真正的缺陷。
        let error = if crate::recovery_progress::bcd_rolled_back()
            && error.to_string().contains("续跑启动链")
        {
            let _ = append_log(
                &log,
                crate::i18n::msg("rollback_bcd_noted"),
            );
            match annotate_chain_failure(&error.to_string(), true) {
                Some(text) => crate::err(&text),
                None => error,
            }
        } else {
            error
        };
        // C: 可能已经被擦除；回滚旧 BCD 不会还原系统文件，并可能移除续跑对象。
        // 保留当前阶段与外部载荷，停止本次执行，故障解除后从同一阶段重试。
        let persisted = store.write_recovery_error_coded(&task, error.code(), error.to_string());
        let _ = append_log(
            &log,
            &format!(
                "Recovery retry required at {:?}: {error}; boot entry and payload retained",
                task.status
            ),
        );
        if let Err(state_error) = persisted {
            let _ = append_log(
                &log,
                &format!("Unable to persist recovery error: {state_error}"),
            );
        }
        return Err(error);
    }
    store.write_transition(&mut task, Stage::RecoveryComplete)?;
    complete_recovery_cleanup(&store, &mut task, &log)?;
    meta_log("FINAL: success, wpeutil reboot");
    run_logged("wpeutil.exe", &["reboot"], &log)
}

/// 续跑链校验失败时，若本次运行回退过 BCD，就把因果补进错误文案。
///
/// 抽成纯函数便于单测：真实的回退标记是进程级静态量（`bcd_rolled_back`），
/// 单测里造不出来，也不该为了让测试能过就把标记做成可变全局。
///
/// 只有**同时**满足「本次回退过」与「错误是续跑链校验」才改写——其它失败
/// （镜像损坏、载荷缺失等）必须保持原文，不能被这句说明糊住。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn annotate_chain_failure(error: &str, rolled_back: bool) -> Option<String> {
    if rolled_back && error.contains("续跑启动链") {
        // `TaskError` 的 Display 自带 `invalid task: ` 前缀，而调用方还要再包一层
        // `crate::err`。不剥掉就会在对话框里出现
        // 「invalid task: invalid task: 续跑启动链…」（2026-10-23 实机撞见）。
        let body = error.strip_prefix("invalid task: ").unwrap_or(error);
        Some(format!(
            "{body}（本次运行中用户按 B 回退过 BCD：BCD 已还原为任务准备期快照，\
             续跑链校验失败属预期）"
        ))
    } else {
        None
    }
}

/// 还原终态自检（2026-10-26）：把「目标卷序列号前后 + Windows 内核文件」写成一行
/// ASCII 结论。它取代人工「放哨兵文件 + 记序列号」：`serial_before` 与
/// `serial_after` 不同即证明目标卷被重写过。
///
/// ★ 自检是**记录**，失败绝不能把一次成功的还原判成失败，所以调用处一律忽略
/// 返回值；盘符或卷身份取不到时字段记 `unknown`，宁可少记也不改判定。
#[cfg(windows)]
fn append_restore_selfcheck(log: &Path, task: &backuprestore_core::Task) {
    let Some(target) = task.target.as_ref() else {
        return;
    };
    let Some(letter) = target.volume.drive_letter else {
        return;
    };
    let after = crate::windows_prepare::volume_identity(letter)
        .map(|identity| identity.volume_serial)
        .unwrap_or_else(|_| "unknown".to_string());
    let ntoskrnl = PathBuf::from(format!(r"{letter}:\Windows\System32\ntoskrnl.exe")).is_file();
    let _ = append_log(
        log,
        &crate::text_parsing::restore_selfcheck_line(
            letter,
            &target.volume.volume_serial,
            &after,
            ntoskrnl,
        ),
    );
}

/// 该阶段证明磁盘、引导、恢复注册均已完成；重复执行不会重做磁盘操作。
#[cfg(windows)]
pub(crate) fn complete_recovery_cleanup(
    store: &TaskStore,
    task: &mut Task,
    log: &Path,
) -> Result<(), TaskError> {
    if task.status != Stage::RecoveryComplete {
        return Err(errc(ErrCode::TaskNotRecovered));
    }
    append_log(log, crate::i18n::msg("step_cleanup"))?;
    let env = store
        .task_dir(&task.task_id)?
        .join("payload/RecoveryTask.env");
    if env.is_file() {
        let values = read_env_file(env)?;
        recovery_fault::configure(
            values.get("TEST_FAULT").map(String::as_str),
            &store.task_dir(&task.task_id)?,
        );
    }
    let result = (|| {
        finalize_boot_entry_after_task(&store.task_dir(&task.task_id)?, log)?;
        // 终态自检：备份任务没有 target，函数自身会直接返回，不用在这里分叉。
        append_restore_selfcheck(log, task);
        recovery_fault::checkpoint("success-before", log)?;
        store.write_transition(task, Stage::Success)?;
        recovery_fault::checkpoint("success-after", log)?;
        append_log(log, "Boot entry cleaned; task marked successful")
    })();
    if let Err(error) = &result {
        // 【2026-10-16 审查修订，日志缺口 4】写盘失败不用 `?`：那会用写盘错误
        // 替换掉 `result` 里的原始恢复错误向上传播（`result` 仍会在函数末尾
        // 返回）。两个失败各自记一行，事后能区分"清理失败"与"连状态都没记上"。
        if task.status == Stage::RecoveryComplete {
            if let Err(state_error) =
                store.write_recovery_error(task, format!("恢复已完成，待重试清理：{error}"))
            {
                let _ = append_log(
                    log,
                    &crate::i18n::msg_tpl(
                        "recovery_done_status_write",
                        &[&error.to_string(), &state_error.to_string()],
                    ),
                );
            }
        }
        if let Err(log_error) =
            append_log(log, &format!("Recovery completed; cleanup pending: {error}"))
        {
            let _ = append_log(
                log,
                &crate::i18n::msg_tpl("recovery_done_log_write", &[&error.to_string(), &log_error.to_string()]),
            );
        }
    }
    result
}

/// 显式重试既有任务：先核验外部启动链；已恢复的任务只补做清理。
#[cfg(windows)]
pub(crate) fn resume_task(root: String, id: String) -> Result<(), TaskError> {
    let store = TaskStore::new(root);
    let mut task = store.load(&id)?;
    let dir = store.task_dir(&id)?;
    let log = store.log_path(&id)?;
    if task.status == Stage::RecoveryComplete {
        return complete_recovery_cleanup(&store, &mut task, &log);
    }
    let explicitly_prepared = task.status == Stage::Prepared;
    if !explicitly_prepared && !stage_resumable_after_interruption(task.status) {
        return Err(errc(ErrCode::TaskNotResumable));
    }
    if explicitly_prepared {
        // `prepare --no-reboot` leaves a complete task/payload and BCD template
        // but intentionally does not arm it. Only an explicit user resume may
        // turn that prepared task into a one-shot recovery boot.
        store.write_transition(&mut task, Stage::BootRequested)?;
    }
    let entry = boot_entry::ReBootEntry::read(&dir)?.ok_or_else(|| errc(ErrCode::TaskMissingLaunchRecord))?;
    boot_entry::rearm(&entry, &log)?;
    append_log(
        &log,
        "Explicit resume: validated existing task and boot payload",
    )?;
    run_logged("shutdown.exe", &["/r", "/t", "0"], &log)
}

/// 终态清理：清一次性启动、删自建 BCD 对象、删镜像卷上的载荷 WIM。
/// 幂等；没有记录（旧通道任务）时直接返回，不报错。
#[cfg(windows)]
fn finalize_boot_entry_after_task(task_dir: &Path, log: &Path) -> Result<(), TaskError> {
    let Some(entry) = boot_entry::ReBootEntry::read(task_dir)? else {
        append_log(
            log,
            "Boot entry cleanup: no recorded boot entry; nothing to clean",
        )?;
        return Ok(());
    };
    boot_entry::disarm(&entry, log)
}

#[cfg(any(windows, test))]
#[allow(dead_code)]
pub(crate) fn clear_task_drive_letters(task: &mut Task) {
    if let Some(source) = task.source.as_mut() {
        source.drive_letter = None;
    }
    if let Some(workspace) = task.workspace_volume.as_mut() {
        workspace.drive_letter = None;
    }
    if let Some(image) = task.image.as_mut() {
        image.volume.drive_letter = None;
    }
    if let Some(destination) = task.destination.as_mut() {
        destination.volume.drive_letter = None;
    }
    if let Some(target) = task.target.as_mut() {
        target.volume.drive_letter = None;
    }
}

/// `winpeshl.ini` starts this GUI-subsystem Rust binary directly.  Verify the
/// actual executable loaded from the task WinRE image as well as the immutable
/// workspace payload copy before touching any disk.  Checking only the copy in
/// the workspace would not prove that WinRE launched the staged Rust payload.
#[cfg(windows)]
fn verify_running_recovery_binary(manifest: &PayloadManifest) -> Result<(), TaskError> {
    let running = env::current_exe()?;
    backuprestore_core::verify_sha256(&running, &manifest.recovery_sha256).map_err(|error| {
        err(&format!(
            "running WinRE Recovery.exe does not match the prepared payload: {error}"
        ))
    })
}


#[cfg(not(windows))]
pub(crate) fn recover_windows(
    _store: &TaskStore,
    _task: &mut Task,
    _log: &Path,
    _efi_root: Option<&Path>,
    _metadata_context: Option<&BTreeMap<String, String>>,
    _finalize_success: bool,
    _progress: Option<&()>,
) -> Result<(), TaskError> {
    Err(err(
        "real Recovery execution is only available on Windows/WinRE",
    ))
}

/// 离线还原是否需要准备「引导修复会话」（STEP 1 预应用判据）。
///
/// 必须与另外两处同源，否则判据漂移会让会话该建没建、收尾却按 Windows 走，
/// 在 STEP 4 炸「Windows 系统卷缺少引导修复方案」：
/// 1. 收尾路由：灌完镜像后探测 `Windows\System32\config\SYSTEM` 是否存在；
/// 2. `resolve_efi_root_for_recovery`：决定要不要提前挂 ESP；
/// 3. 本函数：决定要不要建会话（第二阶段「引导修复」按钮也挂在它上面）。
///
/// create-secondary 的常态是「prepare 刚格式化完的空卷」：灌镜像前探测必然
/// false，所以必须认 `role=NewWindows`。2026-10-19 实机撞见（06fc5e8 引入时
/// 即只认前者；run10 能过只因那次目标卷已装着 Windows）。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn recovery_needs_boot_menu_session(
    target_is_windows: bool,
    role: backuprestore_core::TargetRole,
) -> bool {
    target_is_windows || role == backuprestore_core::TargetRole::NewWindows
}

#[cfg(windows)]
pub(crate) fn recover_windows(
    store: &TaskStore,
    task: &mut Task,
    log: &Path,
    efi_root: Option<&Path>,
    metadata_context: Option<&BTreeMap<String, String>>,
    finalize_success: bool,
    progress: Option<&Arc<crate::recovery_progress::ProgressShared>>,
) -> Result<(), TaskError> {
    use backuprestore_core::TargetRole;
    if task.status == Stage::Prepared {
        store.write_transition(task, Stage::RecoveryStarted)?;
    }
    if task.status == Stage::BootRequested {
        store.write_transition(task, Stage::RecoveryStarted)?;
    }
    if task.status == Stage::RecoveryStarted {
        store.write_transition(task, Stage::Preflight)?;
    }
    let image_path = task
        .image
        .as_ref()
        .map(|image| resolve_volume_path(&image.volume, &image.relative_path, log))
        .transpose()?;
    if let (Some(path), Some(image)) = (&image_path, task.image.as_ref()) {
        let verify_hash = task.verify_hash.unwrap_or(false);
        if verify_hash {
            verify_image_file(path, image)?;
        } else {
            // 快速校验：仅核对文件存在性和物理大小，避免数分钟无谓读盘
            let size = fs::metadata(path)
                .map_err(|e| err(&format!("failed to read image metadata: {e}")))?
                .len();
            if image.size_bytes != 0 && size != image.size_bytes {
                return Err(err(&format!(
                    "image size mismatch: expected {}, got {size}",
                    image.size_bytes
                )));
            }
        }
        if matches!(
            task.operation,
            Operation::RestoreExisting | Operation::CreateSecondary
        ) {
            // 配套备份 metadata 存在时校验 WIM 哈希与目标分区大小；缺失时
            // 视为第三方/PE WIM（如安装 WinRE/PE 为第二系统），跳过该校验。
            if let Ok(metadata) = read_index_metadata(path, image.index) {
                if verify_hash && !metadata.image_sha256.eq_ignore_ascii_case(&image.sha256) {
                    // 用户在准备阶段显式接受过这份镜像时，旧副档不再充当判据。
                    // 镜像在用户确认之后是否又被改动，已由上面的 verify_image_file
                    // 按「接受时的真实哈希与大小」核验；这里只确认例外确实绑定当前
                    // 索引与内容，不是别的镜像或别的索引留下的通用放行。
                    let covered = task.image_acceptance.as_ref().is_some_and(|accepted| {
                        accepted.covers(image.index, &image.sha256, image.size_bytes)
                    });
                    if !covered {
                        return Err(err("image hash does not match backup metadata"));
                    }
                    append_log(
                        log,
                        &format!(
                            "image hash differs from sidecar {} but the user accepted {} at prepare time; sidecar is no longer the source-of-truth for this task",
                            metadata.image_sha256, image.sha256
                        ),
                    )?;
                }
                let target = task.target.as_ref().ok_or_else(|| err("missing target"))?;
                let required = metadata
                    .required_target_size()
                    .max(metadata.source.partition_size);
                if required == 0 || target.volume.partition_size < required {
                    return Err(err(&format!(
                        "target partition is too small: {} < {}",
                        target.volume.partition_size, required
                    )));
                }
            }
        }
        run_logged(
            "dism.exe",
            &["/Get-WimInfo", &format!("/WimFile:{}", path.display())],
            log,
        )?;
    }
    match task.operation {
        Operation::Probe => {
            append_log(log, "Probe task validated; no disk operation requested")?;
            if task.status == Stage::RecoveryStarted {
                store.write_transition(task, Stage::Preflight)?;
            }
            if task.status == Stage::Preflight && finalize_success {
                store.write_transition(task, Stage::Success)?;
            }
        }
        Operation::Backup => {
            execute_recovery_backup(
                store,
                task,
                log,
                metadata_context,
                progress,
                finalize_success,
            )?;
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let mut target = task.target.clone().ok_or_else(|| err("missing target"))?;
            // v1.7.11 新通道：格式化目标卷不再威胁"续跑启动源"（它在镜像卷上），
            // 因此 F1 执行层二次闸整体取消。目标卷自带的 Winre.wim 会被镜像里的
            // 干净原版替换，这与 Ghost 类工具语义一致（注册侧处置见设计文档 §3.3）。
            let image_index = task.image.as_ref().map(|image| image.index).unwrap_or(1);
            if target.role == TargetRole::NewWindows
                && task.boot_plan.mode != BootMode::AddSecondary
            {
                return Err(err("secondary target requires add-secondary boot plan"));
            }
            let image_path = image_path.ok_or_else(|| err("missing image"))?;
            // 目标卷同样按身份解析：PE 里记录盘符可能已被别的卷占用，而
            // `diskpart select disk/partition` 用的磁盘编号是稳定的，所以只要
            // 拿到"这个卷现在叫什么字母"就能安全格式化/应用。
            let target_letter = resolve_volume_letter(&target.volume, log)?;
            let target_root = PathBuf::from(format!("{target_letter}:\\"));
            target.volume.drive_letter = Some(target_letter);
            let live = windows_prepare::volume_identity(target_letter)?;
            let previous = (target.volume.disk_number, target.volume.partition_number);
            target.volume = pe_safety::rebind_after_boot(
                &target.volume,
                &live,
                !matches!(
                    task.status,
                    Stage::TargetErased | Stage::ImageApplied | Stage::BootRepaired
                ),
            )?;
            append_log(
                log,
                &crate::i18n::msg_tpl(
                    "restore_identity_rebound",
                    &[
                        &format!("{previous:?}"),
                        &format!(
                            "{:?}",
                            (target.volume.disk_number, target.volume.partition_number)
                        ),
                    ],
                ),
            )?;
            // 收尾按「目标性质」路由，不按「操作类型」：目标是另一套 Windows 时，
            // create-secondary 和 restore-existing 需要的收尾是同一套（保主系统
            // default + 修盘符绑定 + 按第二系统注册 WinRE）。判据来自准备阶段落盘的
            // is_active_system——RE 内 `%SystemDrive%` 是 RAM 盘 X:，探测不到用户系统卷。
            // 注：default 归属不再由此判据分支——v2.6.4 起只有一条规则
            // （一律指向本次还原出来的系统），固化在 boot_menu::decide_default。
            let target_is_secondary = !target.is_active_system;
            // 第二阶段按钮需要在目标身份确认后尽早出现；方案落盘并不修改 BCD，
            // 实际写入延后到映像应用完成后的引导修复阶段。
            // 判据必须与收尾路由（下方「灌完镜像后探测 SYSTEM hive」那扇门）和
            // `resolve_efi_root_for_recovery` 完全一致：目标**当前**是 Windows 卷，
            // 或本次要把完整系统装进一个空卷（`role=NewWindows`，create-secondary
            // 的常态——prepare 刚格式化完目标，灌镜像前探测必然 false）。
            // 只认前者会让 C 入口离线流在 STEP 4 炸「Windows 系统卷缺少引导修复
            // 方案」：会话没建，收尾却按 Windows 走（2026-10-19 实机撞见，
            // 06fc5e8 引入时即如此；run10 能过只因那次目标已装着 Windows）。
            let boot_menu_session = if recovery_needs_boot_menu_session(
                pe_safety::target_is_windows_volume(&target_root)?,
                target.role.clone(),
            ) {
                // 优先用任务里记录的第二系统菜单名（用户在准备阶段自己起的）；
                // 没有它才由 BootMenuSession 按镜像副档版本自动起名。
                let preferred_name = target.boot_menu_name.clone();
                let session = crate::boot_menu::BootMenuSession::prepare(
                    &target.volume,
                    &task.task_id,
                    &store.task_dir(&task.task_id)?,
                    log,
                    preferred_name.as_deref(),
                )?;
                // default 归属（docs/20261015-0725 S1，2026-10-15 用户修订）：
                // 无论目标是活动系统盘（重建唯一系统）还是另一个分区（新增第二
                // 系统），还原完成后 default 一律指向本次还原出来的系统——用户
                // 要的就是"还原完开机进新系统"。该规则已固化在
                // `boot_menu::decide_default` 里，apply 只执行不推断。
                if let Some(p) = progress {
                    p.set_boot_menu(Arc::clone(&session));
                    // 装载「BCD 回退」动作：点击时把 ESP 上的 BCD 恢复成任务准备期
                    // 保存的快照（bcd-before-raw / bcd-before-export）。ESP 身份在
                    // 点击时才解析（此刻还没到 STEP 3 的 find_efi_root），
                    // 用免盘符探测避免抢盘符。
                    let rollback_task_dir = store.task_dir(&task.task_id)?;
                    let rollback_log = log.to_path_buf();
                    p.set_bcd_rollback(Arc::new(move || {
                        let efi = windows_prepare::esp_identity_without_drive_letter()
                            .map_err(|error| error.to_string())?;
                        windows_prepare::rollback_bcd_snapshot(
                            &rollback_task_dir,
                            &efi,
                            &rollback_log,
                        )
                        .map_err(|error| error.to_string())
                    }));
                }
                append_log(log, crate::i18n::msg("boot_candidates_ready"))?;
                Some(session)
            } else {
                append_log(log, crate::i18n::msg("skip_boot_candidates_not_windows"))?;
                None
            };
            append_log(log, crate::i18n::msg("step_mount"))?;
            if let Some(p) = progress {
                p.reset_start_time();
            }
            // 一次性启动已经被固件消费。完整系统还原必须在擦除前重新武装，
            // 使真正断电后的下一次开机仍能从外部分区进入同一任务。
            let resume_entry = if metadata_context.is_some() {
                let entry = boot_entry::ReBootEntry::read(&store.task_dir(&task.task_id)?)?
                    .ok_or_else(|| errc(ErrCode::RestoreMissingResumeBoot))?;
                if entry.wim_volume.same_partition(&target.volume) {
                    return Err(errc(ErrCode::ResumePayloadOnEraseTarget));
                }
                if !task
                    .image
                    .as_ref()
                    .is_some_and(|image| image.volume.same_partition(&entry.wim_volume))
                {
                    return Err(errc(ErrCode::ResumePayloadVolumeMismatch));
                }
                boot_entry::rearm(&entry, log)?;
                Some(entry)
            } else {
                None
            };
            // TargetErased is deliberately persisted before formatting.  If
            // power fails after that write, formatting and applying the WIM
            // are safe to repeat on the explicitly selected target.  Older
            // task versions persisted ImageApplied before DISM completed, so
            // resume from ImageApplied also re-runs Apply-Image defensively.
            if matches!(task.status, Stage::Preflight | Stage::TargetErased) {
                let entering_target_erased = task.status == Stage::Preflight;
                if task.status == Stage::Preflight {
                    store.write_transition(task, Stage::TargetErased)?;
                }
                // 格式化选项（TargetSpec.format_target，GUI 复选框落盘）：取消勾选时
                // 覆盖式应用——跳过 diskpart 格式化与格式化后校验（不格式化就没有
                // 「序列号已变」的校验对象），直接进入镜像应用。断电续跑从
                // TargetErased 重入时同样按此选项跳过，行为一致。
                if target.format_target {
                    format_target_partition(store, task, &target.volume, log)?;
                    if !target_root.is_dir() {
                        return Err(err(
                            "restore target is unavailable after DiskPart format operation",
                        ));
                    }
                    verify_partition_identity_after_format(&target.volume, log)?;
                } else {
                    append_log(
                        log,
                        crate::i18n::msg("overlay_skip_format"),
                    )?;
                    if !target_root.is_dir() {
                        return Err(err(
                            "restore target is unavailable for overlay apply (no format requested)",
                        ));
                    }
                }
                if entering_target_erased {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-target-erased",
                        Stage::TargetErased,
                        log,
                    );
                }
            }
            if matches!(task.status, Stage::TargetErased | Stage::ImageApplied) {
                pe_safety::verify_binding(
                    &target.volume,
                    &windows_prepare::volume_identity(target_letter)?,
                    false,
                )?;
                append_log(log, crate::i18n::msg("step_apply"))?;
                let entering_image_applied = task.status == Stage::TargetErased;
                // 应用镜像前是最后一个「还没动目标」的安全暂停点：用户可在这里
                // 停下确认镜像/目标，再决定是否继续。
                if let Some(p) = progress {
                    // Err = 暂停期间窗口被关 / 暂停超时：中断恢复链并按失败处理
                    //（现场保留；auto-escape 时会 disarm + 标终态 + 重启回 Windows）。
                    p.wait_if_paused(log, "STEP 2/4 还原镜像")?;
                }
                recovery_fault::checkpoint("apply", log)?;
                // 【2026-10-22 任务 #110】「覆盖式应用（取消格式化）打到已含
                // Windows 的卷」按准备期落盘的 `overlay_no_acl` 路由执行通道：
                // DISM 会在还原 `Program Files\WindowsApps` 等 TrustedInstaller
                // 保护目录的 SD/owner 时被拒（RestoreDirMetadata 0x80070005 →
                // exit 5，PE 内对照实验钉死，见 docs/20261016-1413 §2）；
                // wimlib 自行获取 SeRestore/SeBackup 提权（POC E5：禁用提权的
                // 父进程下仍 exit 0），且该提权覆盖 SD 还原（2026-10-23 概念
                // 验证 docs/20261022-2308：tiny + 全量两级，镜像 SD 逐字节
                // 还原），RE 上下文同样可用。判据、用户确认与二进制定位
                // 均在准备期/GUI/载荷注入完成，此处只读标记决定走哪条路。
                let overlay_no_acl = task
                    .target
                    .as_ref()
                    .is_some_and(|spec| spec.overlay_no_acl);
                if overlay_no_acl {
                    // RE 内 current_exe（Recovery.exe）就在注入后的 System32，
                    // wimlib 二进制由载荷注入链放在同一目录（winre_payload）。
                    let executable_dir = std::env::current_exe()
                        .map_err(|error| {
                            errc_with(ErrCode::ProgramDirUnresolvable, &[&error.to_string()])
                        })?
                        .parent()
                        .ok_or_else(|| errc(ErrCode::ProgramDirResolveFailed))?
                        .to_path_buf();
                    crate::wimlib_apply::apply_overlay(
                        &executable_dir,
                        &image_path,
                        image_index,
                        &target_root,
                        log,
                        crate::dism_timeout::wait_ms(),
                    )?;
                } else if let Err(error) = run_logged_timeout(
                    "dism.exe",
                    &[
                        "/English",
                        "/Apply-Image",
                        &format!("/ImageFile:{}", image_path.display()),
                        &format!("/Index:{}", image_index),
                        &format!("/ApplyDir:{}", target_root.display()),
                    ],
                    log,
                    // 设置里的 DISM 等待上限（默认 u32::MAX = 不限制，零回归）。
                    // 以前这里写死无限等待，配置项形同虚设。
                    crate::dism_timeout::wait_ms(),
                ) {
                    // 【D1 2026-10-24】DISM apply 失败 → 按判据自动降级 wimlib
                    // 重试一次（设计 docs/20261024-1636；判据/分类/文案单一来源
                    // = failure_policy，编排 = wimlib_apply）。四个 apply 落点
                    // （RE 主链 / PE / 在线数据卷分支 / 在线第二系统分支）共用
                    // 同一个编排函数，避免「PE 降级了、RE 没降级」的分叉。
                    crate::wimlib_apply::fallback_after_dism_failure(
                        &error,
                        log,
                        &image_path,
                        image_index,
                        &target_root,
                    )?;
                }
                if task.status == Stage::TargetErased {
                    store.write_transition(task, Stage::ImageApplied)?;
                }
                if entering_image_applied {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-image-applied",
                        Stage::ImageApplied,
                        log,
                    );
                }
            }
            if matches!(task.status, Stage::ImageApplied | Stage::BootRepaired)
                && target_root
                    .join("Windows")
                    .join("System32\\config\\SYSTEM")
                    .is_file()
            {
                append_log(log, crate::i18n::msg("step_verify"))?;
                let efi = find_efi_root(efi_root)?;
                let entering_boot_repaired = task.status == Stage::ImageApplied;
                // 修引导前暂停点：BCD 尚未被改动，用户在此停下可安全放弃。
                if let Some(p) = progress {
                    p.wait_if_paused(log, "STEP 3/4 校验（引导修复前）")?;
                }
                // BootRepaired is recorded immediately before BCDBoot so a
                // failure is eligible for BCD rollback.  A retry from that
                // stage simply re-runs BCDBoot and validates its output.
                if task.status == Stage::ImageApplied {
                    store.write_transition(task, Stage::BootRepaired)?;
                }
                if entering_boot_repaired {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-boot-repaired",
                        Stage::BootRepaired,
                        log,
                    );
                }
                if metadata_context
                    .and_then(|values| env_optional(values, "TEST_FAULT"))
                    .as_deref()
                    == Some("bcdboot-failure")
                {
                    return Err(err("development test fault: BCDBoot failure injected"));
                }
                pe_safety::verify_binding(
                    &target.volume,
                    &windows_prepare::volume_identity(target_letter)?,
                    false,
                )?;
                let windows_root = target_root.join("Windows");
                // target_is_secondary 已在引导方案准备处按同一判据声明（default 策略共用），
                // 这里不再重复声明，避免两处判据漂移。
                // Capture the primary BCD state from the byte-for-byte task
                // snapshot before BCDBoot.  It remains available even if a
                // power failure occurs after BCDBoot changes the live store.
                let previous_boot_manager = if target_is_secondary {
                    secondary_registry::repair(&target_root, target_letter, &target.volume, log)?;
                    Some(boot_manager_state_from_task_snapshot(store, task, log)?)
                } else {
                    None
                };
                let mut bcd_args = vec![
                    windows_root.to_string_lossy().into_owned(),
                    "/s".to_string(),
                    efi.to_string_lossy().into_owned(),
                    "/f".to_string(),
                    "UEFI".to_string(),
                    "/v".to_string(),
                ];
                if target_is_secondary {
                    // 2026-10-15 用户规则修订：**不再加 /d**。此前加 /d 是为了让
                    // default 留在原主系统；现在规则反过来——还原完成后 default 就
                    // 该指向本次还原出来的系统，而 bcdboot 的默认行为正是如此。
                    // 旧注释里"失败后用户被拖进半成品系统"的顾虑实际不成立：
                    // 这里是镜像应用完成之后才跑 bcdboot，那一刻目标卷已是完整
                    // Windows，后续失败（WinRE 注册等）不会让它启动不了。
                    // /addlast 只管固件条目的添加位置，与 default 无关，保留——
                    // 菜单顺序最终仍由 boot_menu::apply 按用户方案写定。
                    bcd_args.push("/addlast".to_string());
                    // 顺带给新条目一个可读名字。最终命名仍由引导修复
                    // （boot_menu::apply，最后一个写 BCD 的步骤）按用户意图覆盖；
                    // 但那一步之前若失败，菜单里留下的就是 BCDBoot 的默认描述
                    // （实测为 "Windows PreInstallation Environment"），用户分不清
                    // 这是哪一项。先写上任务里的名字，失败态也能辨认。
                    if let Some(name) = target.boot_menu_name.as_deref() {
                        let name = name.trim();
                        if !name.is_empty() {
                            bcd_args.push("/description".to_string());
                            bcd_args.push(name.to_string());
                        }
                    }
                }
                let bcd_refs: Vec<&str> = bcd_args.iter().map(String::as_str).collect();
                if let Some(entry) = &resume_entry {
                    boot_entry::rearm(entry, log)?;
                }
                recovery_fault::checkpoint("bcdboot-before", log)?;
                run_logged("bcdboot.exe", &bcd_refs, log)?;
                recovery_fault::checkpoint("bcdboot-after", log)?;
                // 显式记录收尾路由与 default 意图（G4）：A 场景（重建唯一系统）
                // 从此留下「default 指向新 loader」的日志，不再只靠 BCD 现状反推。
                append_log(
                    log,
                    if target_is_secondary {
                        crate::i18n::msg("route_secondary")
                    } else {
                        crate::i18n::msg("route_unique")
                    },
                )?;
                // bcdboot 不带 /d 时本就会把 default 指向新 loader，但 /s 自定义
                // ESP 场景下 BFSVC 的行为不完全可控，所以立刻回读一次确认，不对
                // 就当场改。放在这个位置是因为此刻环境已知良好（bcdboot 刚成功、
                // ESP 仍挂载），比等流程失败后在未知状态下补救可靠得多。
                assert_boot_default_points_at_target(&efi, target_letter, log)?;
                if let Some(entry) = &resume_entry {
                    boot_entry::rearm(entry, log)?;
                }
                let boot_manager = efi.join("EFI\\Microsoft\\Boot\\bootmgfw.efi");
                if !boot_manager.exists() {
                    return Err(err("BCDBoot reported success but bootmgfw.efi is missing"));
                }
                verify_bcd_target(&efi, &target_root, log)?;
                // 【2026-10-28 用户裁定】注册只有两态：成功，或带码失败
                // （缺恢复环境组件 → `BR-RE-047`）。原来的「降级跳过」会让任务报成功
                // 却没有恢复环境，已删除。
                if target_is_secondary {
                    winre_registration::repair_secondary(&target_root, &efi, &target.volume, log)?;
                } else {
                    winre_registration::repair(&target_root, &efi, &target.volume, log)?;
                }
                if target_is_secondary {
                    // 菜单名只有「新增第二系统」才由用户指定；把完整 Windows 镜像还原到
                    // 另一个分区时没有这个意图，此时只定位 loader、不改它的描述。
                    // 但 preserve_primary_boot_manager 两种情况都要跑——保住主系统的
                    // 默认启动项与菜单顺序是安全行为，不依赖菜单名。
                    let menu_name = target
                        .boot_menu_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty());
                    let identifier =
                        set_secondary_boot_menu(&efi, &target_root, menu_name, log)?;
                    if let Some(ref prev_bm) = previous_boot_manager {
                        preserve_primary_boot_manager(
                            &efi,
                            prev_bm,
                            &identifier,
                            log,
                        )?;
                    } else {
                        return Err(crate::errc(ErrCode::BootManagerUnknownContinue));
                    }
                }
                // 引导修复必须是**最后**一个写 BCD 的步骤：用户在弹窗里选的名字、
                // 顺序、勾选与菜单秒数是最终意图。早先放在 bcdboot 之前时，
                // 后面的 set_secondary_boot_menu 会用 task.boot_menu_name 覆盖掉改名，
                // preserve_primary_boot_manager 又会按快照顺序重写 displayorder，
                // 把用户的排序和取消勾选一起丢掉——与弹窗里
                // "本列表就是还原完成后的最终启动菜单顺序"的承诺直接矛盾。
                // 目标系统是必选项；其它系统按勾选处理，既有 PE 入口由 apply 保留。
                let boot_menu_plan = boot_menu_session
                    .as_ref()
                    .ok_or_else(|| errc(ErrCode::WindowsVolumeMissingPlan))?
                    .freeze()?;
                crate::boot_menu::apply(&boot_menu_plan, &efi, log, resume_entry.as_ref())?;
                if finalize_success {
                    append_restore_selfcheck(log, task);
                    append_log(log, crate::i18n::msg("step_cleanup"))?;
                    store.write_transition(task, Stage::Success)?;
                }
            } else if matches!(task.status, Stage::ImageApplied | Stage::BootRepaired) {
                // 数据卷还原：目标卷无 SYSTEM hive（非 Windows 系统），跳过 BCDBoot 启动修复。
                append_log(log, crate::i18n::msg("step_verify"))?;
                append_log(
                    log,
                    "data-volume restore: target has no SYSTEM hive; skipping BCDBoot",
                )?;
                if task.status == Stage::ImageApplied {
                    store.write_transition(task, Stage::BootRepaired)?;
                }
                if finalize_success {
                    append_restore_selfcheck(log, task);
                    append_log(log, crate::i18n::msg("step_cleanup"))?;
                    store.write_transition(task, Stage::Success)?;
                }
            } else {
                return Err(err(&format!(
                    "restore cannot resume from stage {:?}",
                    task.status
                )));
            }
        }
    }
    append_log(log, "Recovery completed")?;
    Ok(())
}

/// Development-only fault injection for a power loss after a durable recovery
/// stage.  Reboot immediately without writing `failed` or cleaning WinRE, so
/// a subsequent normal-Windows GUI launch must detect and resume the task.
#[cfg(windows)]
fn interrupt_after_stage_if_requested(
    metadata_context: Option<&BTreeMap<String, String>>,
    fault: &str,
    stage: Stage,
    log: &Path,
) {
    if metadata_context
        .and_then(|values| env_optional(values, "TEST_FAULT"))
        .as_deref()
        != Some(fault)
    {
        return;
    }
    // A simulated power-loss task is deliberately booted again by the normal
    // GUI. Persist a one-shot marker before rebooting so the same injected
    // fault cannot fire forever when the resumed stage is re-entered.
    let marker = log.with_file_name(format!(".fault-{fault}.triggered"));
    if marker.is_file() {
        // ★ 被陈旧标记抑制时**必须大声说出来**（2026-10-28）。
        //   标记是「一次性」语义，挂在任务目录里防止重启后无限重放同一故障。
        //   但目录会被复用（验收夹具就是固定 task id），于是上一轮的标记会把
        //   这一轮的注入吃掉 —— 用例照样绿，其实什么都没验。
        //   静默地不注入是本项目最贵的坑之一（「报告通过但什么都没验」），
        //   所以这里留一条**可被 grep 的痕迹**：injection=suppressed_by_stale_marker。
        let _ = append_log(
            log,
            &format!(
                "TEST_FAULT={fault} injection=suppressed_by_stale_marker marker={}; \
                 an earlier run left this marker, so NO fault was injected this time",
                marker.display()
            ),
        );
        return;
    }
    if fs::write(&marker, format!("stage={stage:?}\n")).is_err() {
        let _ = append_log(
            log,
            "Development test fault marker could not be persisted; refusing injection",
        );
        return;
    }
    let _ = append_log(
        log,
        &format!(
            "Development test fault: simulating power loss after durable stage {stage:?}; rebooting without cleanup"
        ),
    );
    let _ = run_logged("wpeutil.exe", &["reboot"], log);
    std::process::exit(0);
}


#[derive(Default)]
pub(crate) struct RecoverOptions {
    pub(crate) dry_run: bool,
    pub(crate) efi_root: Option<PathBuf>,
}

pub(crate) fn parse_recover_options(arguments: Vec<String>) -> Result<RecoverOptions, TaskError> {
    let mut options = RecoverOptions::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--dry-run" => options.dry_run = true,
            "--efi-root" => {
                let root = arguments
                    .next()
                    .ok_or_else(|| err("--efi-root requires a mounted EFI root path"))?;
                if root.trim().is_empty() {
                    return Err(err("--efi-root cannot be empty"));
                }
                options.efi_root = Some(PathBuf::from(root));
            }
            _ => return Err(err(&format!("unknown recover option: {argument}"))),
        }
    }
    Ok(options)
}

pub(crate) fn recover(root: String, id: String, options: RecoverOptions) -> Result<(), TaskError> {
    let store = TaskStore::new(root);
    let task_path = store
        .task_path(&id)
        .map_err(|error| errc_with(ErrCode::TaskIdInvalid, &[&id, &error.to_string()]))?;
    if !task_path.is_file() {
        return Err(errc_with(
            ErrCode::TaskFileMissing,
            &[&task_path.display().to_string()],
        ));
    }
    let mut task = store.load(&id)?;
    let log = store.log_path(&id)?;
    append_log(
        &log,
        &format!(
            "Recovery started operation={:?} task={}",
            task.operation, task.task_id
        ),
    )?;
    if task.status == Stage::Success {
        append_log(&log, "Task already successful; refusing to run twice")?;
        return Ok(());
    }
    if task.status == Stage::Failed {
        return Err(err(
            "task is failed; create a new task after reviewing recovery.log",
        ));
    }
    if options.dry_run {
        println!(
            "dry-run: task={} operation={:?} stage={:?}",
            task.task_id, task.operation, task.status
        );
        if let Some(image) = &task.image {
            println!(
                "image={} sha256={}",
                image
                    .absolute_path
                    .as_deref()
                    .unwrap_or(&image.relative_path),
                image.sha256
            );
        }
        if let Some(target) = &task.target {
            println!(
                "target partition={} role={:?}",
                target.volume.partition_guid, target.role
            );
        }
        append_log(&log, "Dry-run completed; no disk operation performed")?;
        return Ok(());
    }
    if !cfg!(windows) {
        return Err(err(
            "real Recovery execution is only available on Windows/WinRE; use --dry-run on this host",
        ));
    }
    if let Some(efi_root) = options.efi_root.as_deref() {
        if !efi_root.is_dir() {
            return Err(err("--efi-root must be an existing mounted EFI directory"));
        }
        append_log(
            &log,
            &format!(
                "Using explicit EFI root for direct recovery: {}",
                efi_root.display()
            ),
        )?;
    }
    #[cfg(windows)]
    let metadata_values: Option<BTreeMap<String, String>> = {
        let dir = store.task_dir(&id)?;
        let env = dir.join("payload/RecoveryTask.env");
        let values = if env.is_file() {
            read_env_file(env)?
        } else {
            BTreeMap::new()
        };
        crate::recovery_fault::configure(values.get("TEST_FAULT").map(String::as_str), &dir);
        if task.status == Stage::RecoveryComplete {
            return complete_recovery_cleanup(&store, &mut task, &log);
        }
        Some(values)
    };
    #[cfg(not(windows))]
    let metadata_values: Option<BTreeMap<String, String>> = None;
    #[cfg(windows)]
    let efi_root: Option<PathBuf> = match options.efi_root.clone() {
        Some(root) => Some(root),
        None => resolve_efi_root_for_recovery(&task, &log, metadata_values.as_ref())?,
    };
    #[cfg(not(windows))]
    let efi_root: Option<PathBuf> = options.efi_root.clone();
    let result = recover_windows(
        &store,
        &mut task,
        &log,
        efi_root.as_deref(),
        metadata_values.as_ref(),
        false,
        None,
    );
    if let Err(error) = &result {
        if let Err(log_error) = append_log(
            &log,
            &crate::i18n::msg_tpl("recovery_failed", &[&error.to_string()]),
        ) {
            let _ = append_log(
                &log,
                &crate::i18n::msg_tpl("recovery_failed_log_write", &[&error.to_string(), &log_error.to_string()]),
            );
        }
        if let Err(write_error) =
            store.write_recovery_error_coded(&task, error.code(), error.to_string())
        {
            let _ = append_log(
                &log,
                &crate::i18n::msg_tpl(
                    "recovery_failed_status_write",
                    &[&error.to_string(), &write_error.to_string()],
                ),
            );
        }
    }
    #[cfg(windows)]
    if result.is_ok() {
        store.write_transition(&mut task, Stage::RecoveryComplete)?;
        complete_recovery_cleanup(&store, &mut task, &log)?;
    }
    result
}

pub(crate) fn cli_recover(mut args: impl Iterator<Item = String>) -> Result<(), TaskError> {
    let root = args.next().ok_or_else(|| err("task root is required"))?;
    let id = args.next().ok_or_else(|| err("task id is required"))?;
    let options = parse_recover_options(args.collect())?;
    recover(root, id, options)
}

/// 解析卷内工作区相对路径；从最后一个 tasks 分隔符切分，允许上层目录也叫 tasks。
#[cfg(any(windows, test))]
pub(crate) fn split_workspace_root_rel(value: &str, task_id: &str) -> Result<String, TaskError> {
    backuprestore_core::validate_relative_path(value)?;
    let normalized = value.replace('/', "\\");
    let (store_rel, relative_id) = normalized
        .rsplit_once("\\tasks\\")
        .ok_or_else(|| err("WORKSPACE_ROOT_REL must contain \\tasks\\"))?;
    if store_rel.is_empty() {
        return Err(err("WORKSPACE_ROOT_REL has an empty workspace path"));
    }
    if relative_id != task_id {
        return Err(err("WORKSPACE_ROOT_REL task id does not match TASK_ID"));
    }
    Ok(store_rel.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use backuprestore_core::TargetRole;

    #[test]
    fn recovery_boot_menu_gate_matches_finalize_routing() {
        assert!(recovery_needs_boot_menu_session(
            false,
            TargetRole::NewWindows
        ));
        assert!(recovery_needs_boot_menu_session(
            true,
            TargetRole::ExistingWindows
        ));
        assert!(!recovery_needs_boot_menu_session(
            false,
            TargetRole::ExistingWindows
        ));
        assert!(recovery_needs_boot_menu_session(
            true,
            TargetRole::NewWindows
        ));
    }

    #[test]
    fn recover_options_accept_explicit_efi_root() {
        let options =
            parse_recover_options(vec!["--dry-run".into(), "--efi-root".into(), "E:\\".into()])
                .expect("options should parse");
        assert!(options.dry_run);
        assert_eq!(options.efi_root.unwrap().to_string_lossy(), "E:\\");
    }

    #[test]
    fn recover_options_reject_unknown_or_incomplete_flags() {
        assert!(parse_recover_options(vec!["--unknown".into()]).is_err());
        assert!(parse_recover_options(vec!["--efi-root".into()]).is_err());
    }

    /// 【2026-10-23 实机发现】按 B 回退后续跑链校验失败，文案要说明因果。
    #[test]
    fn chain_failure_note_only_for_bcd_rollback_sessions() {
        let chain = "invalid task: 续跑启动链字段不符：device";
        // 回退过 + 续跑链错 → 补说明，且原文不丢
        let note = annotate_chain_failure(chain, true).expect("应补说明");
        assert!(note.contains("续跑启动链字段不符：device"), "原文不能丢：{note}");
        assert!(note.contains("按 B 回退过 BCD"), "要点明是用户按了 B：{note}");
        assert!(
            !note.starts_with("invalid task: "),
            "前缀必须剥掉：{note}"
        );
        // 没回退过 → 不改写
        assert_eq!(annotate_chain_failure(chain, false), None);
        // 回退过但错误不是续跑链 → 保持原文，不能被说明糊住
        assert_eq!(
            annotate_chain_failure("payload WIM is missing", true),
            None
        );
    }

    #[test]
    fn workspace_path_allows_directory_named_tasks() {
        let task_id = "11111111-1111-4111-8111-111111111111";
        assert_eq!(
            split_workspace_root_rel(&format!("tasks\\tasks\\{task_id}"), task_id).unwrap(),
            "tasks"
        );
        assert_eq!(
            split_workspace_root_rel(
                &format!("Tools\\tasks\\BackupRestore\\tasks\\{task_id}"),
                task_id
            )
            .unwrap(),
            "Tools\\tasks\\BackupRestore"
        );
        assert!(
            split_workspace_root_rel("..\\tasks\\11111111-1111-4111-8111-111111111111", task_id)
                .is_err()
        );
        assert!(
            split_workspace_root_rel(
                "tasks\\tasks\\22222222-2222-4222-8222-222222222222",
                task_id
            )
            .is_err()
        );
    }
}


