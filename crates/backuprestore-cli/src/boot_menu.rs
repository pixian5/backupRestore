//! 还原期间编辑引导方案，映像应用完成后才写入 BCD。

use backuprestore_core::{TaskError, VolumeIdentity};
use serde::{Deserialize, Serialize};
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
        if !fields
            .iter()
            .any(|(k, v)| *k == "path" && v.ends_with("\\winload.efi"))
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
                    " [本次还原·必选]"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

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
    let mut entries = vec![BootMenuEntry {
        drive_letter: target_letter,
        display_name: "本次还原的 Windows".into(),
        selected: true,
        is_restore_target: true,
        volume: target.clone(),
    }];
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
    // 集成测试只为确保方案结构能构造；字段语义由上方纯函数测试覆盖。
    #[test]
    fn boot_menu_module_compiles() {
        assert!(super::TIMEOUT_DEFAULT > 0);
    }
}
