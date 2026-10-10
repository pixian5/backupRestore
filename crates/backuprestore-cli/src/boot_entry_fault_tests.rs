//! 真实活动 BCD 的续跑拒绝与补偿失败测试，只接受已核验快照的专用夹具。
use super::*;

#[test]
#[ignore = "修改测试虚拟机启动对象；需要 BR_ACCEPTANCE_ROOT 和 BR_ACCEPTANCE_SNAPSHOT"]
fn missing_payload_conflict_and_cleanup_retry() -> Result<(), TaskError> {
    let proof = std::env::var("BR_ACCEPTANCE_SNAPSHOT").map_err(|_| crate::err("缺少新快照"))?;
    crate::text_parsing::require_guid(&proof, "acceptance snapshot").map_err(|e| crate::err(&e))?;
    let root =
        PathBuf::from(std::env::var("BR_ACCEPTANCE_ROOT").map_err(|_| crate::err("缺少夹具目录"))?);
    let store = backuprestore_core::TaskStore::new(&root);
    let ids: Vec<_> = fs::read_dir(root.join("tasks"))?.collect::<Result<_, _>>()?;
    if ids.len() != 1 {
        return Err(crate::err("只允许一个已完成测试任务"));
    }
    let task = store.load(&ids[0].file_name().to_string_lossy())?;
    if task.status != backuprestore_core::Stage::Success {
        return Err(crate::err("测试任务必须先成功"));
    }
    let original = store.task_dir(&task.task_id)?;
    let task_id = task.task_id.clone();
    let dir = root.join("boot-fault-fixture");
    fs::create_dir(&dir)?;
    let log = dir.join("boot-fault.log");
    let volume = task
        .workspace_volume
        .ok_or_else(|| crate::err("缺少工作卷"))?;
    // 前置条件仍按整个暂存父目录判断"卷上没有任何恢复载荷"；载荷按任务隔离后，
    // 本夹具自己的那一份落在 BackupRestoreRE\<任务ID>\ 下。
    let staging_root = crate::boot_cleanup::staging_volume_root(&volume)?;
    if staging_root
        .join(crate::boot_cleanup::RE_STAGING_DIR)
        .try_exists()?
    {
        return Err(crate::err("存在恢复载荷，禁止覆盖"));
    }
    let staging = crate::boot_cleanup::staging_task_path(&staging_root, &task_id)?;
    let baseline = bcd(&["/enum", "all", "/v"])?;
    if !bcd_field(BOOTMGR, "bootsequence")?.is_empty() {
        return Err(crate::err("已有启动请求，拒绝测试"));
    }
    let sdi = PathBuf::from(std::env::var("SystemDrive").map_err(|_| crate::err("缺少系统盘"))?)
        .join("Recovery\\WindowsRE\\boot.sdi");
    let entry = create_entry(
        &original.join("stage/Winre.wim"),
        &sdi,
        &volume,
        &dir,
        &task_id,
        &log,
    )?;
    let wim = PathBuf::from(&entry.wim_path);
    let saved = wim.with_extension("wim.acceptance-backup");
    let before = bcd(&["/enum", "all", "/v"])?;
    fs::rename(&wim, &saved)?;
    let missing = rearm(&entry, &log);
    fs::rename(&saved, &wim)?;
    assert!(missing.is_err());
    assert_eq!(before, bcd(&["/enum", "all", "/v"])?);
    println!("FAULT_PASS missing-payload: BCD unchanged; original retained");
    fs::rename(&wim, &saved)?;
    fs::write(&wim, b"corrupted payload")?;
    let damaged = rearm(&entry, &log);
    fs::remove_file(&wim)?;
    fs::rename(&saved, &wim)?;
    assert!(damaged.is_err());
    assert_eq!(before, bcd(&["/enum", "all", "/v"])?);
    println!("FAULT_PASS corrupt-payload: BCD unchanged; original retained");
    bcd(&["/bootsequence", "{current}"])?;
    let foreign = bcd_field(BOOTMGR, "bootsequence")?;
    let conflict = rearm(&entry, &log);
    assert!(conflict.is_err());
    assert_eq!(foreign, bcd_field(BOOTMGR, "bootsequence")?);
    bcd(&["/deletevalue", BOOTMGR, "bootsequence"])?;
    println!("FAULT_PASS boot-request-conflict: foreign request preserved");
    rearm(&entry, &log)?;
    // 模拟补偿删除被未知文件阻挡：必须报错、保留未知内容，并支持再次补偿。
    fs::write(staging.join("keep.acceptance"), b"unknown content")?;
    assert!(disarm(&entry, &log).is_err());
    assert_eq!(
        fs::read(staging.join("keep.acceptance"))?,
        b"unknown content"
    );
    fs::remove_file(staging.join("keep.acceptance"))?;
    disarm(&entry, &log)?;
    assert_eq!(baseline, bcd(&["/enum", "all", "/v"])?);
    println!("FAULT_PASS cleanup-compensation-failure: retry succeeded; baseline BCD restored");

    // 问题 11：字段写入失败时必须留下半成品簿记，且统一补偿能据此清掉那两个对象。
    // 此前这两个 GUID 只出现在日志文本里，没有任何代码认得它们——界面上是
    // "准备失败"，机器上却静静留着两个 device/osdevice 从未写成功的 BCD 对象。
    //
    // ★ 夹具自 2.2.1 以来的缺陷（本轮实机首次执行才暴露）：前段成功的 create_entry
    // 在同一目录留下了 boot-entry.json，末段再次调用会命中"复用已有条目"分支
    // 直接返回 Ok，entry-fields 注入点根本不执行。必须先清掉簿记与本任务载荷，
    // 让 create_entry 走全新创建路径。
    fs::remove_file(dir.join(ENTRY_RECORD))?;
    let staging_root2 = crate::boot_cleanup::staging_volume_root(&volume)?;
    crate::boot_cleanup::remove_staging(&staging_root2, &task_id)?;
    crate::recovery_fault::configure(Some("acceptance:entry-fields:error"), &dir);
    let kept = create_entry(
        &original.join("stage/Winre.wim"),
        &sdi,
        &volume,
        &dir,
        &task_id,
        &log,
    );
    assert!(kept.is_err(), "字段写入注入错误必须让建条目失败");
    let residue = crate::boot_record::ResidueRecord::read(&dir)?
        .ok_or_else(|| crate::err("失败后必须留下半成品簿记，否则没人认得那两个对象"))?;
    assert_eq!(residue.task_id, task_id);
    assert!(crate::text_parsing::is_guid(&residue.loader_guid));
    assert!(crate::text_parsing::is_guid(&residue.devopts_guid));
    // 注入点位于 /copy 之后、字段改写（finish()）之前：KEEP 保留的对象是从注册
    // WinRE 模板整体复制来的，**继承模板的全部字段**（device 仍指向 C: 注册位）。
    // "从未写成功"的正确证据是：字段仍指向模板路径，而不是本任务载荷路径
    // （H:\BackupRestoreRE\<任务ID>\）——若 finish() 真跑过，device 一定会被
    // 改写成指向我们按任务隔离的载荷 WIM。
    let loader_block = bcd(&["/enum", &residue.loader_guid, "/v"])?;
    let devopts_block = bcd(&["/enum", &residue.devopts_guid, "/v"])?;
    // bcdedit 的字段名随系统语言本地化（中文系统输出"标识符"而非 identifier，
    // 2026-10-11 SYSTEM 通道实机确认）。存在性判断只能用语言无关的 GUID 本身。
    assert!(
        loader_block.contains(&residue.loader_guid),
        "保留的诊断对象 loader 应仍存在"
    );
    let staging_marker = crate::boot_cleanup::staging_relative_dir(&task_id)?;
    assert!(
        !loader_block
            .to_ascii_lowercase()
            .contains(&staging_marker.to_ascii_lowercase()),
        "半成品 loader 的 device 不得指向本任务载荷（说明 finish() 从未写入成功）"
    );
    assert!(
        !devopts_block
            .to_ascii_lowercase()
            .contains(&staging_marker.to_ascii_lowercase()),
        "半成品 devopts 的 ramdisksdipath 不得指向本任务载荷目录"
    );
    println!("FAULT_PASS entry-field-failure: residue recorded with exact GUIDs");
    // 复现"统一补偿"收尾：清注入配置，按残留簿记的精确 GUID 删掉 KEEP 的两个
    // 诊断对象，再回基线。真实路径里这由 compensate_prepare_failure 完成；
    // 夹具内直接用同一 remove_entry_objects，保持安全边界一致。
    crate::recovery_fault::configure(None, &dir);
    remove_entry_objects(&residue.loader_guid, &residue.devopts_guid, &log)?;
    crate::boot_record::ResidueRecord::clear(&dir)?;
    assert_eq!(baseline, bcd(&["/enum", "all", "/v"])?);
    println!("FAULT_PASS entry-field-compensation: residue objects removed; baseline restored");
    Ok(())
}
