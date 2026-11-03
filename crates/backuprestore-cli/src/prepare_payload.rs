//! WinRE 载荷注入、暂存与一次性引导武装子系统 (prepare_payload.rs)
//!
//! 承载准备阶段核心的载荷组装与 UEFI 启动武装：
//! 1. `ReStaging`: 暂存卷与 boot.sdi 路径模型；
//! 2. `prepare_payload`: 建立任务工作目录，导出 BCD 字节快照，挂载并注入 WinRE 载荷
//!    （静态工具链、RecoveryTask.env、task.json），创建/复制镜像卷暂存 BCD 引导项，
//!    生成 manifest.json，武装一次性启动并触发系统重启；
//! 3. 任何步骤失败均触发统一的准备故障补偿（`compensate_prepare_failure`）。

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use backuprestore_core::{
    PayloadManifest, Task, TaskError, TaskStore, VolumeIdentity, write_json_atomic,
};

use crate::prepare_bcd::{compensate_prepare_failure, snapshot_raw_bcd};
use crate::prepare_env::{append_env, write_recovery_env, write_status_env};
use crate::prepare_plan::PrepareOptions;
use crate::volume_device::flush_volume;
use crate::windows_prepare::{ensure_volume_mounted_leased, release_process_leases};
use crate::{append_log, err, run_logged, sha256_file, winre_payload};

pub struct ReStaging {
    pub volume: VolumeIdentity,
    pub boot_sdi: PathBuf,
}

// These values have different ownership and rollback roles. Keeping the
// parameters explicit makes preparation ordering and cleanup dependencies
// auditable at the call site.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_payload(
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
        let (esp_letter, _lease_esp) = ensure_volume_mounted_leased(efi, 'S', log)?;
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
