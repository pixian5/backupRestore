//! 还原期间编辑引导方案，映像应用完成后才写入 BCD。

use backuprestore_core::{TaskError, VolumeIdentity};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::path::{Path, PathBuf};

pub(crate) const TIMEOUT_MAX: u32 = 999;
pub(crate) const TIMEOUT_DEFAULT: u32 = 10;
pub(crate) const NAME_MAX_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BootMenuEntry {
    pub drive_letter: char,
    pub display_name: String,
    pub selected: bool,
    pub is_restore_target: bool,
    pub volume: VolumeIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BootMenuPlan {
    pub entries: Vec<BootMenuEntry>,
    pub timeout_seconds: u32,
}

impl BootMenuPlan {
    pub(crate) fn toggle(&mut self, index: usize) -> bool {
        let Some(entry) = self.entries.get_mut(index) else {
            return false;
        };
        if entry.is_restore_target {
            return false;
        }
        entry.selected = !entry.selected;
        true
    }
    pub(crate) fn move_up(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.entries.len() {
            return false;
        }
        self.entries.swap(index - 1, index);
        true
    }
    pub(crate) fn move_down(&mut self, index: usize) -> bool {
        if index >= self.entries.len().saturating_sub(1) {
            return false;
        }
        self.entries.swap(index, index + 1);
        true
    }
    /// 本次还原目标在菜单里的下标。默认顺序下它在最后，但用户可上移，
    /// 所以调用方一律用本方法定位，不要硬编码 0。
    pub(crate) fn restore_target_index(&self) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.is_restore_target)
    }
    pub(crate) fn rename(&mut self, index: usize, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty()
            || trimmed.chars().any(char::is_control)
            || index >= self.entries.len()
        {
            return false;
        }
        self.entries[index].display_name = trimmed.chars().take(NAME_MAX_CHARS).collect();
        true
    }
    pub(crate) fn validate(&self) -> Result<(), TaskError> {
        if self.entries.iter().filter(|e| e.is_restore_target).count() != 1
            || self
                .entries
                .iter()
                .any(|e| e.is_restore_target && !e.selected)
        {
            return Err(crate::err("本次还原系统必须且只能出现一次，不能取消"));
        }
        if self.timeout_seconds > TIMEOUT_MAX {
            return Err(crate::err("启动菜单等待时间超出范围"));
        }
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.display_name.trim().is_empty()
                || entry.display_name.chars().any(char::is_control)
                || entry.display_name.chars().count() > NAME_MAX_CHARS
                || !entry.volume.is_complete()
            {
                return Err(crate::err("引导方案名称或卷身份无效"));
            }
            if self.entries[..i]
                .iter()
                .any(|e| e.volume.same_partition(&entry.volume))
            {
                return Err(crate::err("引导方案重复系统分区"));
            }
        }
        Ok(())
    }
}

/// 对象必须唯一；同卷存在多个加载器时拒绝按枚举顺序猜测。
pub(crate) fn loader_identifier_for_partition(
    output: &str,
    letter: char,
) -> Result<Option<String>, TaskError> {
    let needle = format!("partition={}: ", letter.to_ascii_lowercase());
    let needle = needle.trim_end();
    let mut found = Vec::new();
    for block in output.replace("\r\n", "\n").split("\n\n") {
        let lower = block.to_ascii_lowercase();
        let fields: Vec<_> = lower
            .lines()
            .filter_map(|line| {
                let (key, value) = line.trim().split_once(char::is_whitespace)?;
                Some((key, value.trim()))
            })
            .collect();
        // 中文本地化的 bcdedit 用「路径」而不是 path；只认英文会在中文机器上
        // 漏掉全部加载器，引导修复因此永远找不到候选（实机前由单测拦住）。
        if !fields
            .iter()
            .any(|(k, v)| matches!(*k, "path" | "路径") && v.ends_with("\\winload.efi"))
        {
            continue;
        }
        if !fields
            .iter()
            .any(|(k, v)| matches!(*k, "device" | "osdevice" | "设备") && *v == needle)
        {
            continue;
        }
        if let Some(id) = block.lines().find_map(crate::bcd_identifier_from_line) {
            found.push(id.to_string());
        }
    }
    if found.len() > 1 {
        return Err(crate::err("同一系统分区有多个引导项，拒绝猜测"));
    }
    Ok(found.pop())
}

pub(crate) fn render_entries(plan: &BootMenuPlan, cursor: usize) -> String {
    plan.entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            format!(
                "{} [{}] {}. {} ({}:){}",
                if i == cursor { ">" } else { " " },
                if e.selected { "√" } else { " " },
                i + 1,
                e.display_name,
                e.drive_letter,
                if e.is_restore_target {
                    " [当前正在还原中的系统·Step 4生效]"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// 落盘的引导方案：带上 task_id 防止拿上一个任务的候选来改本次还原。
#[cfg(windows)]
#[derive(Debug, Serialize, Deserialize)]
struct SavedPlan {
    task_id: String,
    plan: BootMenuPlan,
}

#[cfg(windows)]
pub(crate) struct BootMenuSession {
    state: std::sync::Mutex<SessionState>,
    path: PathBuf,
    task_id: String,
    log: PathBuf,
}
#[cfg(windows)]
struct SessionState {
    plan: BootMenuPlan,
    editable: bool,
    error: Option<String>,
}

#[cfg(windows)]
impl BootMenuSession {
    pub(crate) fn prepare(
        target: &VolumeIdentity,
        task_id: &str,
        dir: &Path,
        log: &Path,
    ) -> Result<std::sync::Arc<Self>, TaskError> {
        let path = dir.join("boot-menu-plan.json");
        let mut plan = if path.try_exists()? {
            let saved: SavedPlan = backuprestore_core::read_json(&path)?;
            if saved.task_id != task_id
                || !saved
                    .plan
                    .entries
                    .iter()
                    .any(|e| e.is_restore_target && e.volume.same_partition(target))
            {
                return Err(crate::err("引导方案不属于当前还原任务"));
            }
            saved.plan
        } else {
            discover(target, log)?
        };
        for entry in &mut plan.entries {
            let letter = crate::windows_prepare::ensure_volume_mounted(
                &entry.volume,
                entry.drive_letter,
                log,
            )?;
            let actual = crate::windows_prepare::volume_identity(letter)?;
            entry.volume = crate::pe_safety::rebind_after_boot(&entry.volume, &actual, false)?;
            entry.drive_letter = letter;
        }
        plan.validate()?;
        backuprestore_core::write_json_atomic(
            &path,
            &SavedPlan {
                task_id: task_id.into(),
                plan: plan.clone(),
            },
        )?;
        crate::append_log(
            log,
            &format!(
                "引导修复方案已准备：{} 个系统；本次目标必选；无倒计时",
                plan.entries.len()
            ),
        )?;
        Ok(std::sync::Arc::new(Self {
            state: std::sync::Mutex::new(SessionState {
                plan,
                editable: true,
                error: None,
            }),
            path,
            task_id: task_id.into(),
            log: log.into(),
        }))
    }
    pub(crate) fn snapshot(&self) -> (BootMenuPlan, bool) {
        let state = self.state.lock().unwrap();
        (state.plan.clone(), state.editable)
    }
    pub(crate) fn edit(&self, change: impl FnOnce(&mut BootMenuPlan)) {
        let mut state = self.state.lock().unwrap();
        if !state.editable {
            return;
        }
        let mut next = state.plan.clone();
        change(&mut next);
        let result = next.validate().and_then(|_| {
            backuprestore_core::write_json_atomic(
                &self.path,
                &SavedPlan {
                    task_id: self.task_id.clone(),
                    plan: next.clone(),
                },
            )
        });
        match result {
            Ok(()) => {
                state.plan = next;
                let _ = crate::append_log(&self.log, "引导修复选择已保存，恢复继续运行");
            }
            Err(error) => {
                state.error = Some(error.to_string());
                let _ = crate::append_log(&self.log, &format!("引导修复方案保存失败：{error}"));
            }
        }
    }
    pub(crate) fn freeze(&self) -> Result<BootMenuPlan, TaskError> {
        let mut state = self.state.lock().unwrap();
        state.editable = false;
        if let Some(error) = &state.error {
            return Err(crate::err(error));
        }
        state.plan.validate()?;
        Ok(state.plan.clone())
    }
}

#[cfg(windows)]
fn discover(target: &VolumeIdentity, log: &Path) -> Result<BootMenuPlan, TaskError> {
    let target_letter = target
        .drive_letter
        .ok_or_else(|| crate::err("还原目标盘符缺失"))?;
    // 默认顺序沿用既有约定：原有系统在前，本次还原的目标追加在后。
    // （早先版本把目标放在首位，导致启动菜单第一项变成新系统、而默认启动项仍是
    // 主系统，两者不一致会让用户困惑；现在默认与 run10 验收过的行为一致，
    // 用户想置前可用「上移」自己调整。）
    let mut entries = Vec::new();
    let paths = crate::windows_prepare::native_volume_paths()?;
    for path in paths {
        let identity = match crate::windows_prepare::volume_identity_at_path(&path, path.clone()) {
            Ok(identity) => identity,
            Err(error) => {
                crate::append_log(log, &format!("引导候选卷身份不可读：{path}；{error}"))?;
                continue;
            }
        };
        if identity.is_reserved_partition()
            || identity.same_partition(target)
            || identity.drive_letter == Some('X')
        {
            continue;
        }
        let root = Path::new(&path);
        if !crate::pe_safety::target_is_windows_volume(root)?
            || !crate::pe_safety::probe_file(root, &["Windows", "System32", "winload.efi"])?
        {
            continue;
        }
        // 常规系统必须含离线恢复组件；硬盘版 PE 保留现有入口，不重新登记。
        if !crate::pe_safety::probe_file(root, &["Windows", "System32", "reagentc.exe"])? {
            crate::append_log(
                log,
                &format!("卷缺少常规 Windows 恢复组件，保留现有入口：{path}"),
            )?;
            continue;
        }
        let letter = crate::windows_prepare::ensure_volume_mounted(&identity, 'U', log)?;
        let actual = crate::windows_prepare::volume_identity(letter)?;
        let volume = crate::pe_safety::rebind_after_boot(&identity, &actual, false)?;
        entries.push(BootMenuEntry {
            drive_letter: letter,
            display_name: format!("Windows ({letter}:)"),
            selected: true,
            is_restore_target: false,
            volume,
        });
    }
    // 还原目标放在最后：保持「原有系统在前」的默认顺序，且它是必选项不能取消。
    entries.push(BootMenuEntry {
        drive_letter: target_letter,
        display_name: "本次还原的 Windows".into(),
        selected: true,
        is_restore_target: true,
        volume: target.clone(),
    });
    Ok(BootMenuPlan {
        entries,
        timeout_seconds: TIMEOUT_DEFAULT,
    })
}

#[cfg(windows)]
pub(crate) fn apply(
    plan: &BootMenuPlan,
    esp: &Path,
    log: &Path,
    resume: Option<&crate::boot_entry::ReBootEntry>,
) -> Result<(), TaskError> {
    plan.validate()?;
    // 盘符是临时参数、不是身份。从方案准备好到真正写入 BCD 之间，目标卷可能被
    // 重新挂到别的盘符（在线路径 STEP 3.5 就挂到 T:），所以这里必须按稳定 GUID
    // 重新取当前盘符。用 prepare 时记的旧盘符去找 loader，会一个都找不到——
    // 这正是 2026-10-12 在线路径接入后第四步必败的根因。
    let mut plan = plan.clone();
    for entry in &mut plan.entries {
        let letter =
            crate::windows_prepare::ensure_volume_mounted(&entry.volume, entry.drive_letter, log)?;
        let actual = crate::windows_prepare::volume_identity(letter)?;
        entry.volume = crate::pe_safety::rebind_after_boot(&entry.volume, &actual, false)?;
        entry.drive_letter = letter;
    }
    let store = esp
        .join("EFI\\Microsoft\\Boot\\BCD")
        .to_string_lossy()
        .into_owned();
    let mut output = crate::capture_logged(
        "bcdedit.exe",
        &["/store", &store, "/enum", "all", "/v"],
        log,
    )?;
    let previous = crate::parse_boot_manager_state(&output)
        .ok_or_else(|| crate::err("引导修复前启动管理器无效"))?;
    for entry in plan.entries.iter().filter(|e| e.selected) {
        let actual = crate::windows_prepare::volume_identity(entry.drive_letter)?;
        crate::pe_safety::verify_binding(&entry.volume, &actual, false)?;
        if !crate::pe_safety::target_is_windows_volume(&PathBuf::from(format!(
            "{}:\\",
            entry.drive_letter
        )))? {
            return Err(crate::err("引导修复时所选系统已不存在"));
        }
        if loader_identifier_for_partition(&output, entry.drive_letter)?.is_none() {
            if let Some(re) = resume {
                crate::boot_entry::rearm(re, log)?;
            }
            crate::run_logged(
                "bcdboot.exe",
                &[
                    &format!("{}:\\Windows", entry.drive_letter),
                    "/s",
                    &esp.to_string_lossy(),
                    "/f",
                    "UEFI",
                    "/addlast",
                ],
                log,
            )?;
            if let Some(re) = resume {
                crate::boot_entry::rearm(re, log)?;
            }
            output = crate::capture_logged(
                "bcdedit.exe",
                &["/store", &store, "/enum", "all", "/v"],
                log,
            )?;
        }
    }
    let mut selected = Vec::new();
    let mut candidates = Vec::new();
    let mut target_id = None;
    for entry in &plan.entries {
        let id = loader_identifier_for_partition(&output, entry.drive_letter)?;
        if let Some(id) = &id {
            candidates.push(id.clone());
        }
        if !entry.selected {
            continue;
        }
        let id = id.ok_or_else(|| crate::err("引导修复后找不到系统加载器"))?;
        crate::run_logged(
            "bcdedit.exe",
            &[
                "/store",
                &store,
                "/set",
                &id,
                "description",
                &entry.display_name,
            ],
            log,
        )?;
        if entry.is_restore_target {
            target_id = Some(id.clone());
        }
        selected.push(id);
    }
    // 不在常规 Windows 候选中的入口（包括硬盘版 PE）保留在原菜单中。
    for id in &previous.display_order {
        if !candidates.contains(id) && output.contains(id) && !selected.contains(id) {
            selected.push(id.clone());
        }
    }
    let default = if selected.contains(&previous.default) {
        previous.default
    } else {
        target_id.ok_or_else(|| crate::err("缺少必选目标加载器"))?
    };
    crate::run_logged(
        "bcdedit.exe",
        &["/store", &store, "/default", &default],
        log,
    )?;
    if let Some(id) = &previous.resume_object {
        crate::run_logged(
            "bcdedit.exe",
            &["/store", &store, "/set", "{bootmgr}", "resumeobject", id],
            log,
        )?;
    }
    let mut args = vec!["/store", store.as_str(), "/displayorder"];
    args.extend(selected.iter().map(String::as_str));
    crate::run_logged("bcdedit.exe", &args, log)?;
    crate::run_logged(
        "bcdedit.exe",
        &[
            "/store",
            &store,
            "/timeout",
            &plan.timeout_seconds.to_string(),
        ],
        log,
    )?;
    let verify = crate::capture_logged(
        "bcdedit.exe",
        &["/store", &store, "/enum", "all", "/v"],
        log,
    )?;
    let actual = crate::parse_boot_manager_state(&verify)
        .ok_or_else(|| crate::err("无法回读引导修复结果"))?;
    let timeout = verify.lines().any(|line| {
        line.split_whitespace().collect::<Vec<_>>()
            == ["timeout", &plan.timeout_seconds.to_string()]
    });
    if actual.display_order != selected
        || actual.default != default
        || actual.resume_object != previous.resume_object
        || !timeout
    {
        return Err(crate::err(
            "引导修复顺序、默认项、休眠引用或显示时间回读不一致",
        ));
    }
    for entry in plan.entries.iter().filter(|e| e.selected) {
        let id = loader_identifier_for_partition(&verify, entry.drive_letter)?
            .ok_or_else(|| crate::err("回读缺少所选系统"))?;
        let object =
            crate::capture_logged("bcdedit.exe", &["/store", &store, "/enum", &id, "/v"], log)?;
        if !object.lines().any(|line| {
            line.trim()
                .strip_prefix("description")
                .is_some_and(|v| v.trim() == entry.display_name)
        }) {
            return Err(crate::err("引导菜单名称回读不一致"));
        }
    }
    crate::append_log(
        log,
        &format!(
            "引导修复完成：菜单 {} 项，显示 {} 秒；现有 PE 入口保留",
            selected.len(),
            plan.timeout_seconds
        ),
    )
}

#[cfg(windows)]
#[path = "boot_menu_window.rs"]
mod window;
#[cfg(windows)]
pub(crate) use window::{handle_key, open_editor};

#[cfg(test)]
mod tests {
    use super::*;
    use backuprestore_core::VolumeIdentity;

    /// 构造完整卷身份（`is_complete` 要求全部稳定字段都在）。
    fn identity(disk: u8, partition: u8, letter: char) -> VolumeIdentity {
        VolumeIdentity {
            disk_guid: format!("{{dg-{disk}}}"),
            partition_guid: format!("{{pg-{partition}}}"),
            volume_guid: format!("{{vg-{disk}-{partition}}}"),
            partition_type_guid: "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}".into(),
            disk_number: Some(u32::from(disk)),
            partition_number: Some(u32::from(partition)),
            partition_offset: u64::from(partition) * 1_000_000,
            partition_size: 500_000_000_000,
            filesystem: "NTFS".into(),
            volume_serial: format!("{disk:08x}{partition:08x}"),
            drive_letter: Some(letter),
        }
    }

    /// 默认顺序与 `discover()` 一致：原有系统在前，本次还原的目标在最后。
    fn plan() -> BootMenuPlan {
        BootMenuPlan {
            entries: vec![
                BootMenuEntry {
                    drive_letter: 'C',
                    display_name: "Windows (C:)".into(),
                    selected: true,
                    is_restore_target: false,
                    volume: identity(0, 1, 'C'),
                },
                BootMenuEntry {
                    drive_letter: 'E',
                    display_name: "本次还原的 Windows".into(),
                    selected: true,
                    is_restore_target: true,
                    volume: identity(2, 5, 'E'),
                },
            ],
            timeout_seconds: TIMEOUT_DEFAULT,
        }
    }

    #[test]
    fn restore_target_index_finds_the_mandatory_entry() {
        // 默认顺序里目标在最后；用户上移后仍必须定位正确，故禁止硬编码下标 0。
        let p = plan();
        assert_eq!(p.restore_target_index(), Some(1));
        let mut moved = plan();
        assert!(moved.move_up(1), "目标可被上移");
        assert_eq!(moved.restore_target_index(), Some(0), "上移后下标要跟着变");
        let mut empty = plan();
        empty.entries.clear();
        assert_eq!(empty.restore_target_index(), None);
    }

    #[test]
    fn restore_target_cannot_be_unselected() {
        let mut p = plan();
        assert!(!p.toggle(1), "还原目标是必选项，勾选必须被拒绝");
        assert!(p.entries[1].selected, "必选项状态不能被改变");
        assert!(p.toggle(0), "其它系统可以取消");
        assert!(!p.entries[0].selected);
        assert!(p.toggle(0));
        assert!(p.entries[0].selected, "再次勾选应恢复选中");
        assert!(!p.toggle(9), "越界下标不得 panic");
    }

    #[test]
    fn ordering_moves_entries_within_bounds() {
        let mut p = plan();
        assert!(!p.move_up(0), "首项上移应无效");
        assert!(p.move_down(0), "首项下移应生效");
        assert_eq!(p.entries[0].drive_letter, 'E', "下移后首项应变成原第二项");
        assert!(p.move_up(1));
        assert_eq!(p.entries[0].drive_letter, 'C', "上移后应还原");
        assert!(!p.move_down(1), "末项下移应无效");
        assert!(!p.move_down(9), "越界下标不得 panic");
    }

    #[test]
    fn rename_trims_and_rejects_unusable_names() {
        let mut p = plan();
        assert!(p.rename(1, "  第二系统  "));
        assert_eq!(p.entries[1].display_name, "第二系统", "首尾空白应被去掉");
        assert!(!p.rename(1, "   "), "纯空白名称必须拒绝");
        assert!(!p.rename(1, "带\x07控制字符"), "控制字符必须拒绝");
        assert!(!p.rename(9, "越界"), "越界下标不得 panic");
        let long = "长".repeat(NAME_MAX_CHARS + 5);
        assert!(p.rename(1, &long));
        assert_eq!(
            p.entries[1].display_name.chars().count(),
            NAME_MAX_CHARS,
            "超长名称应截断"
        );
    }

    #[test]
    fn validate_requires_exactly_one_selected_target() {
        let mut p = plan();
        p.validate().expect("默认方案应通过");
        p.entries.remove(1);
        p.validate().expect_err("缺少还原目标必须拒绝");
        let mut p = plan();
        p.entries[1].selected = false;
        p.validate().expect_err("还原目标被取消必须拒绝");
        let mut p = plan();
        p.entries[0].is_restore_target = true;
        p.validate().expect_err("两个必选项必须拒绝");
    }

    #[test]
    fn validate_rejects_incomplete_identity_and_duplicates() {
        let mut p = plan();
        p.entries[1].volume.volume_guid = String::new();
        p.validate().expect_err("卷身份不完整必须拒绝");
        let mut p = plan();
        p.entries[1].volume.disk_guid = p.entries[0].volume.disk_guid.clone();
        p.entries[1].volume.partition_guid = p.entries[0].volume.partition_guid.clone();
        p.validate().expect_err("重复系统分区必须拒绝");
        let mut p = plan();
        p.entries[1].display_name = String::new();
        p.validate().expect_err("空名称必须拒绝");
        let mut p = plan();
        p.timeout_seconds = TIMEOUT_MAX + 1;
        p.validate().expect_err("菜单时间超上限必须拒绝");
        let mut p = plan();
        p.timeout_seconds = TIMEOUT_MAX;
        p.validate().expect("恰好上限应通过");
    }

    #[test]
    fn loader_identifier_finds_the_unique_loader() {
        // 两个对象块：只有 C 分区的那个同时满足 winload.efi + 设备字段。
        let output = "\
Windows Boot Manager\r\nidentifier {bootmgr}\r\n\r\nWindows Boot Loader\r\nidentifier {11111111-2222-3333-4444-555555555555}\r\npath \\Windows\\system32\\winload.efi\r\ndevice partition=C:\r\nosdevice partition=C:\r\n\r\n";
        assert_eq!(
            loader_identifier_for_partition(output, 'c')
                .unwrap()
                .as_deref(),
            Some("{11111111-2222-3333-4444-555555555555}")
        );
        assert_eq!(loader_identifier_for_partition(output, 'd').unwrap(), None);
    }

    #[test]
    fn loader_identifier_accepts_localized_device_labels() {
        // 中文 Windows 的 bcdedit 用「设备」「操作系统设备」标签，字段名解析不能因此失效。
        let output = "Windows Boot Loader\r\n标识符 {aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}\r\n路径 \\Windows\\system32\\winload.efi\r\n设备 partition=C:\r\n操作系统设备 partition=C:\r\n\r\n";
        assert_eq!(
            loader_identifier_for_partition(output, 'C')
                .unwrap()
                .as_deref(),
            Some("{aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}")
        );
    }

    #[test]
    fn loader_identifier_refuses_to_guess_between_two_loaders() {
        // 同卷两个加载器：按枚举顺序猜会把用户引导到错误系统，必须报错。
        let output = "Windows Boot Loader\r\nidentifier {11111111-1111-1111-1111-111111111111}\r\npath \\Windows\\system32\\winload.efi\r\ndevice partition=C:\r\nosdevice partition=C:\r\n\r\nWindows Boot Loader\r\nidentifier {22222222-2222-2222-2222-222222222222}\r\npath \\Windows\\system32\\winload.efi\r\ndevice partition=C:\r\nosdevice partition=C:\r\n\r\n";
        assert!(loader_identifier_for_partition(output, 'c').is_err());
    }

    #[test]
    fn loader_identifier_ignores_ramdisk_and_incomplete_objects() {
        // ramdisk 选项与 resume 对象也带 winload 路径特征，但设备字段不是分区加载器。
        let output = "Windows Boot Loader\r\nidentifier {ramdisk}\r\npath \\Windows\\system32\\winload.efi\r\nramdiskdevice partition=E:\r\n\r\nWindows Boot Loader\r\nidentifier {no-osdevice}\r\npath \\Windows\\system32\\winload.efi\r\ndevice partition=C:\r\n\r\n";
        assert_eq!(loader_identifier_for_partition(output, 'e').unwrap(), None);
        assert_eq!(loader_identifier_for_partition(output, 'c').unwrap(), None);
    }

    #[test]
    fn render_entries_marks_cursor_target_and_selection() {
        // 光标落在下标 1，即默认顺序里的还原目标（原有系统在前）。
        let rendered = render_entries(&plan(), 1);
        let lines: Vec<&str> = rendered.split("\r\n").collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with(' '), "非焦点行用空格占位保持排版稳定");
        assert!(lines[0].contains("[√]"), "选中项显示勾选");
        assert!(lines[0].contains("(C:)"), "首行是原有系统");
        assert!(!lines[0].contains("必选"), "非目标行不得标注必选");
        assert!(lines[1].starts_with('>'), "光标行以 > 指示");
        assert!(
            lines[1].contains("[当前正在还原中的系统·Step 4生效]"),
            "还原目标必须标注"
        );
        assert!(
            lines[1].ends_with("[当前正在还原中的系统·Step 4生效]"),
            "必选标注在行尾"
        );
        assert!(lines[1].contains("(E:)"), "目标行带盘符");
    }
}
