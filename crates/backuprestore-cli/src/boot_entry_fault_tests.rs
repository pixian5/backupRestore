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
    Ok(())
}
