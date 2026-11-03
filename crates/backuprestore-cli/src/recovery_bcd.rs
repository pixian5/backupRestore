//! 恢复完成后的 BCD 引导项维护、默认项校验与第二系统引导菜单编排。
//!
//! 将引导管理器（Boot Manager）状态快照恢复、第二系统菜单项追加、
//! 孤儿休眠恢复对象（resumeobject）清理以及 BCD 引导有效性回读验证
//! 独立为自治模块，避免 main.rs 膨胀。

#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::path::Path;

use backuprestore_core::TaskError;
#[cfg(windows)]
use backuprestore_core::{Task, TaskStore};
use crate::error_code::{ErrCode, errc};
#[cfg(windows)]
use crate::process_runner::{capture_logged, run_logged};
#[cfg(windows)]
use crate::{append_log, err};

#[cfg(any(windows, test))]
pub(crate) fn bcd_identifier_from_line(line: &str) -> Option<&str> {
    let start = line.find('{')?;
    let end = line[start..].find('}')? + start + 1;
    let identifier = &line[start..end];
    (identifier.len() == 38
        && identifier.starts_with('{')
        && identifier.ends_with('}')
        && identifier[1..37]
            .chars()
            .enumerate()
            .all(|(index, character)| {
                matches!(index, 8 | 13 | 18 | 23)
                    .then_some(character == '-')
                    .unwrap_or_else(|| character.is_ascii_hexdigit())
            }))
    .then_some(identifier)
}

#[cfg(windows)]
pub(crate) fn set_secondary_boot_menu(
    efi_root: &Path,
    target_root: &Path,
    // None = 调用方没有命名意图（把完整 Windows 还原到另一个分区），此时只定位
    // loader 标识符给 preserve_primary_boot_manager 用，不改它的描述。
    menu_name: Option<&str>,
    log: &Path,
) -> Result<String, TaskError> {
    let store = efi_root.join("EFI\\Microsoft\\Boot\\BCD");
    if !store.exists() {
        return Err(err("BCD store is missing after BCDBoot"));
    }
    let store_arg = store.to_string_lossy().into_owned();
    let output = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let letter = target_root
        .to_string_lossy()
        .chars()
        .next()
        .ok_or_else(|| err("secondary target has no drive letter"))?
        .to_ascii_lowercase();
    let target_needle = format!("partition={letter}:");
    // Parse blank-line-separated BCD objects. The first GUID in a block is
    // the object identifier; later GUIDs belong to recovery/resume/inherit
    // references. Restrict matches to a Windows loader path so a
    // winresume.efi object cannot receive the secondary menu description.
    let normalized_output = output.replace("\r\n", "\n");
    let identifier = normalized_output
        .split("\n\n")
        .filter_map(|block| {
            let identifier = block.lines().find_map(bcd_identifier_from_line)?;
            let lower = block.to_ascii_lowercase();
            let is_loader = lower.contains("winload.efi");
            let targets_partition = lower.lines().any(|line| {
                let line = line.trim_start();
                (line.starts_with("device")
                    || line.starts_with("osdevice")
                    || line.starts_with("设备")
                    || line.starts_with("os 设备"))
                    && line.contains(&target_needle)
            });
            (is_loader && targets_partition).then_some(identifier.to_string())
        })
        .next()
        .ok_or_else(|| {
            err("BCDBoot created no Windows loader tied to the selected target partition")
        })?;
    let Some(menu_name) = menu_name else {
        append_log(
            log,
            &format!("Secondary Windows loader {identifier} kept its BCDBoot description"),
        )?;
        return Ok(identifier);
    };
    let description_args = [
        "/store",
        store_arg.as_str(),
        "/set",
        identifier.as_str(),
        "description",
        menu_name,
    ];
    run_logged("bcdedit.exe", &description_args, log)?;
    let verify = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    if !verify
        .to_ascii_lowercase()
        .contains(&menu_name.to_ascii_lowercase())
    {
        return Err(err("BCD menu name was not visible after update"));
    }
    append_log(
        log,
        &format!("Secondary Windows loader {identifier} named {menu_name}"),
    )?;
    Ok(identifier.clone())
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BcdBootManagerState {
    pub(crate) default: String,
    pub(crate) display_order: Vec<String>,
    /// Boot Manager 自身的 resumeobject——BCDBoot 改写 `{bootmgr}` 时会把它指向
    /// 新 loader 的休眠恢复程序，断电矩阵实测发现我们此前只恢复 default/displayorder，
    /// 该字段被默默换掉且无人校验。主系统 loader 自己的 resumeobject 不受影响。
    pub(crate) resume_object: Option<String>,
    /// 快照中每个 Windows loader 到其 resumeobject 的映射，用来在替换第二系统时
    /// 精确回收旧 loader 的孤儿 resume 对象；不按描述文字或“所有未引用对象”盲删。
    pub(crate) loader_resume_objects: Vec<(String, String)>,
}

/// Read the original Boot Manager state from the BCD snapshot captured while
/// preparing the task. BCDBoot documents that `/addlast` is ignored when `/s`
/// specifies a volume, and it can replace Boot Manager's default with the new
/// loader. The snapshot is therefore the source of truth across retries.
#[cfg(windows)]
pub(crate) fn boot_manager_state_from_task_snapshot(
    store: &TaskStore,
    task: &Task,
    log: &Path,
) -> Result<BcdBootManagerState, TaskError> {
    let task_dir = store.task_dir(&task.task_id)?;
    let text_snapshot = task_dir.join("bcd-before-bootmgr.txt");
    if text_snapshot.is_file() {
        let output = fs::read_to_string(&text_snapshot)?;
        return parse_boot_manager_state(&output)
            .ok_or_else(|| err("saved Boot Manager state is missing or invalid"));
    }
    let raw_snapshot = task_dir.join("bcd-before-raw");
    let snapshot = if raw_snapshot.is_file() {
        raw_snapshot
    } else {
        task_dir.join("bcd-before-export")
    };
    if !snapshot.is_file() {
        return Err(err(
            "task BCD snapshot is missing before secondary boot repair",
        ));
    }
    boot_manager_state_from_store(&snapshot, log)
}

#[cfg(windows)]
pub(crate) fn boot_manager_state_from_store(
    store: &Path,
    log: &Path,
) -> Result<BcdBootManagerState, TaskError> {
    let store_arg = store.to_string_lossy().into_owned();
    let output = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    parse_boot_manager_state(&output)
        .ok_or_else(|| err("existing Boot Manager state is missing or invalid"))
}

/// 确认默认启动项指向**本次还原出来的系统**；不对就当场改过去。
///
/// 2026-10-15 用户规则：还原完成后 default 一律指向新还原的系统。
#[cfg(windows)]
pub(crate) fn assert_boot_default_points_at_target(
    efi_root: &Path,
    target_letter: char,
    log: &Path,
) -> Result<(), TaskError> {
    let store = efi_root.join("EFI\\Microsoft\\Boot\\BCD");
    let store_arg = store.to_string_lossy().into_owned();
    // 目标卷的 loader：按分区找，不按"bcdboot 建了哪个"猜。
    let all = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let loader = crate::boot_menu::loader_identifier_for_partition(&all, target_letter)?
        .ok_or_else(|| errc(ErrCode::BcdbootLoaderMissing))?;
    // `/enum {bootmgr} /v`：/v 才输出 GUID 而非 {bootmgr}/{default} 别名。
    let read_default = |log: &Path| -> Result<String, TaskError> {
        let text = capture_logged(
            "bcdedit.exe",
            &["/store", &store_arg, "/enum", "{bootmgr}", "/v"],
            log,
        )?;
        parse_boot_manager_state(&text)
            .map(|state| state.default)
            .ok_or_else(|| errc(ErrCode::PostBcdbootBootMgrUnparsable))
    };
    let current = read_default(log)?;
    if current.eq_ignore_ascii_case(&loader) {
        append_log(log, &crate::i18n::msg_tpl("default_switched", &[loader.as_str()]))?;
        return Ok(());
    }
    append_log(
        log,
        &crate::i18n::msg_tpl(
            "default_switching",
            &[&format!("{current}").as_str(), loader.as_str()],
        ),
    )?;
    run_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/default", &loader],
        log,
    )?;
    let after = read_default(log)?;
    if !after.eq_ignore_ascii_case(&loader) {
        return Err(errc(ErrCode::DefaultSwitchFailed));
    }
    append_log(log, crate::i18n::msg("default_already_switched"))?;
    Ok(())
}

/// 保留仍存在的菜单项并追加第二系统；不复活 BCDBoot 删除的旧编号。
#[cfg(windows)]
pub(crate) fn preserve_primary_boot_manager(
    efi_root: &Path,
    previous: &BcdBootManagerState,
    secondary_identifier: &str,
    log: &Path,
) -> Result<(), TaskError> {
    let store = efi_root.join("EFI\\Microsoft\\Boot\\BCD");
    let store_arg = store.to_string_lossy().into_owned();
    let live = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let expected_order = secondary_boot_order(previous, secondary_identifier, &live)?;
    if let Some(resume_object) = &previous.resume_object {
        run_logged(
            "bcdedit.exe",
            &[
                "/store",
                &store_arg,
                "/set",
                "{bootmgr}",
                "resumeobject",
                resume_object,
            ],
            log,
        )?;
    } else if parse_boot_manager_state(&live)
        .and_then(|state| state.resume_object)
        .is_some()
    {
        run_logged(
            "bcdedit.exe",
            &[
                "/store",
                &store_arg,
                "/deletevalue",
                "{bootmgr}",
                "resumeobject",
            ],
            log,
        )?;
    }
    let mut display_args = vec![
        "/store".to_string(),
        store_arg.clone(),
        "/displayorder".to_string(),
    ];
    display_args.extend(expected_order.iter().cloned());
    let display_refs: Vec<&str> = display_args.iter().map(String::as_str).collect();
    run_logged("bcdedit.exe", &display_refs, log)?;
    let verified_text = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let verified = parse_boot_manager_state(&verified_text)
        .ok_or_else(|| err("Boot Manager state could not be read after preserving order"))?;
    let resume_object_matches = match (&verified.resume_object, &previous.resume_object) {
        (None, None) => true,
        (Some(actual), Some(expected)) => actual.eq_ignore_ascii_case(expected),
        _ => false,
    };
    if !resume_object_matches
        || verified.display_order.len() != expected_order.len()
        || verified
            .display_order
            .iter()
            .zip(&expected_order)
            .any(|(actual, expected)| !actual.eq_ignore_ascii_case(expected))
    {
        return Err(err("Secondary display order could not be preserved"));
    }
    cleanup_orphaned_secondary_resume_objects(previous, &verified_text, log)?;
    append_log(
        log,
        &format!(
            "Restored {} original display-order entries and appended secondary loader {secondary_identifier} (default left to the boot-menu plan)",
            expected_order.len().saturating_sub(1),
        ),
    )?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn cleanup_orphaned_secondary_resume_objects(
    previous: &BcdBootManagerState,
    live: &str,
    log: &Path,
) -> Result<(), TaskError> {
    let normalized_live = live.replace("\r\n", "\n");
    let live_blocks: Vec<&str> = normalized_live.split("\n\n").collect();
    let live_ids: std::collections::BTreeSet<String> = live_blocks
        .iter()
        .filter_map(|block| block.lines().find_map(bcd_identifier_from_line))
        .map(str::to_ascii_lowercase)
        .collect();
    let live_resume_refs: std::collections::BTreeSet<String> = live_blocks
        .iter()
        .flat_map(|block| block.lines())
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .to_ascii_lowercase()
                .starts_with("resumeobject")
                .then(|| bcd_value_from_line(trimmed).map(str::to_ascii_lowercase))
                .flatten()
        })
        .collect();
    for (loader, resume) in &previous.loader_resume_objects {
        if live_ids.contains(&loader.to_ascii_lowercase())
            || live_resume_refs.contains(&resume.to_ascii_lowercase())
        {
            continue;
        }
        let guid = crate::text_parsing::require_guid(resume, "remove orphan resume object")
            .map_err(|message| err(&message))?;
        match run_logged("bcdedit.exe", &["/delete", &guid, "/f"], log) {
            Ok(()) => append_log(
                log,
                &format!("Removed orphan resume object {guid} for replaced loader {loader}"),
            )?,
            Err(error) => append_log(
                log,
                &format!(
                    "Orphan resume object {guid} for replaced loader {loader} is already absent (deletion skipped): {error}"
                ),
            )?,
        }
    }
    Ok(())
}

#[cfg(any(windows, test))]
pub(crate) fn secondary_boot_order(
    previous: &BcdBootManagerState,
    secondary: &str,
    live: &str,
) -> Result<Vec<String>, TaskError> {
    let normalized = live.replace("\r\n", "\n");
    let objects: std::collections::BTreeSet<String> = normalized
        .split("\n\n")
        .filter_map(|block| block.lines().find_map(bcd_identifier_from_line))
        .map(str::to_ascii_lowercase)
        .collect();
    if !objects.contains(&previous.default.to_ascii_lowercase())
        || !objects.contains(&secondary.to_ascii_lowercase())
    {
        return Err(errc(ErrCode::LoaderMissingRefuseMenu));
    }
    let mut order: Vec<_> = previous
        .display_order
        .iter()
        .filter(|id| {
            objects.contains(&id.to_ascii_lowercase()) && !id.eq_ignore_ascii_case(secondary)
        })
        .cloned()
        .collect();
    if order.is_empty() {
        order.push(previous.default.clone());
    }
    order.push(secondary.to_owned());
    Ok(order)
}

/// Parse the minimal Boot Manager state required to preserve the existing loader.
#[cfg(any(windows, test))]
pub(crate) fn parse_boot_manager_state(output: &str) -> Option<BcdBootManagerState> {
    let output = output.replace("\r\n", "\n");
    let loader_resume_objects = output
        .split("\n\n")
        .filter_map(|block| {
            let lower = block.to_ascii_lowercase();
            if !lower.contains("\\windows\\system32\\winload.efi") {
                return None;
            }
            let loader = block.lines().find_map(bcd_identifier_from_line)?;
            let resume = block.lines().find_map(|line| {
                let trimmed = line.trim();
                trimmed
                    .to_ascii_lowercase()
                    .starts_with("resumeobject")
                    .then(|| bcd_value_from_line(trimmed).map(str::to_owned))
                    .flatten()
            })?;
            Some((loader.to_owned(), resume))
        })
        .collect();
    let boot_manager = output.split("\n\n").find(|block| {
        block
            .lines()
            .find_map(bcd_identifier_from_line)
            .is_some_and(|identifier| {
                identifier.eq_ignore_ascii_case("{9dea862c-5cdd-4e70-acc1-f32b344d4795}")
            })
    })?;
    let mut default = None;
    let mut resume_object = None;
    let mut display_order = Vec::new();
    let mut reading_display_order = false;
    for line in boot_manager.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("default") || trimmed.starts_with("默认") {
            default = bcd_value_from_line(trimmed).map(str::to_owned);
            reading_display_order = false;
            continue;
        }
        if lower.starts_with("resumeobject") {
            resume_object = bcd_value_from_line(trimmed).map(str::to_owned);
            reading_display_order = false;
            continue;
        }
        let key = trimmed.split_whitespace().next().unwrap_or("");
        if key.eq_ignore_ascii_case("displayorder") || key == "显示顺序" {
            reading_display_order = true;
        } else if !trimmed.starts_with('{') {
            reading_display_order = false;
        }
        if reading_display_order {
            if let Some(value) = bcd_value_from_line(trimmed) {
                display_order.push(value.to_owned());
            } else if !trimmed.is_empty() {
                reading_display_order = false;
            }
        }
    }
    Some(BcdBootManagerState {
        default: default?,
        display_order,
        resume_object,
        loader_resume_objects,
    })
}

#[cfg(any(windows, test))]
pub(crate) fn bcd_value_from_line(line: &str) -> Option<&str> {
    let start = line.find('{')?;
    let end = line[start..].find('}')? + start + 1;
    let value = &line[start..end];
    (value.len() >= 3
        && value[1..value.len() - 1]
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-'))
    .then_some(value)
}

#[cfg(windows)]
pub(crate) fn verify_bcd_target(efi_root: &Path, target_root: &Path, log: &Path) -> Result<(), TaskError> {
    let store = efi_root.join("EFI\\Microsoft\\Boot\\BCD");
    if !store.is_file() {
        return Err(err("BCD store is missing after BCDBoot"));
    }
    let target_letter = target_root
        .to_string_lossy()
        .chars()
        .next()
        .ok_or_else(|| err("restore target has no drive letter for BCD verification"))?
        .to_ascii_lowercase();
    let needle = format!("partition={target_letter}:");
    let store_arg = store.to_string_lossy().into_owned();
    let output = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let matched = output.lines().any(|line| {
        let lower = line.trim().to_ascii_lowercase();
        (lower.starts_with("device")
            || lower.starts_with("osdevice")
            || lower.starts_with("设备")
            || lower.starts_with("os 设备"))
            && lower.contains(&needle)
    });
    if !matched {
        return Err(err(&format!(
            "BCDBoot completed but no Windows loader points to the selected target {target_letter}:"
        )));
    }
    append_log(
        log,
        &format!("Verified BCD device/osdevice points to target {target_letter}:"),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secondary_menu_omits_deleted_objects_and_requires_primary() {
        let primary = "{11111111-2222-4333-8444-555555555555}";
        let removed = "{22222222-3333-4444-8555-666666666666}";
        let secondary = "{33333333-4444-4555-8666-777777777777}";
        let other = "{44444444-5555-4666-8777-888888888888}";
        let previous = BcdBootManagerState {
            default: primary.into(),
            display_order: vec![primary.into(), removed.into(), other.into()],
            resume_object: None,
            loader_resume_objects: Vec::new(),
        };
        let live = format!("identifier {primary}\n\n标识符 {secondary}\n\nidentifier {other}");
        assert_eq!(
            secondary_boot_order(&previous, secondary, &live).unwrap(),
            [primary, other, secondary]
        );
        assert!(
            secondary_boot_order(&previous, secondary, &format!("identifier {secondary}"))
                .is_err()
        );
        assert!(
            secondary_boot_order(&previous, secondary, &format!("identifier {primary}"))
                .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "需要 Windows 管理员（只读拒绝路径：BCD 存储不存在时必须拒绝且不写）"]
    fn secondary_menu_repair_rejections() {
        let tmp = std::env::temp_dir().join(format!(
            "br-menu-reject-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let log = tmp.join("menu-reject.log");
        let previous = BcdBootManagerState {
            default: "{aaaaaaaa-0000-0000-0000-000000000001}".to_string(),
            display_order: vec!["{aaaaaaaa-0000-0000-0000-000000000001}".to_string()],
            resume_object: None,
            loader_resume_objects: Vec::new(),
        };
        let secondary = "{aaaaaaaa-0000-0000-0000-000000000002}";
        let error = preserve_primary_boot_manager(&tmp, &previous, secondary, &log)
            .expect_err("BCD 存储不存在时必须拒绝");
        assert_eq!(error.code(), Some("BR-APL-009"), "外部命令失败的稳定码：{error}");
        assert!(
            error.to_string().contains("bcdedit"),
            "失败文案必须点名是哪个程序：{error}"
        );
        assert!(
            !tmp.join("EFI").exists(),
            "拒绝路径不得创建 EFI 目录：{}",
            tmp.join("EFI").display()
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "需要已完成的第二系统任务、EFI 挂载点及新核验快照；真实修改启动菜单"]
    fn secondary_menu_repair_native() {
        use std::path::PathBuf;
        let snapshot = std::env::var("BR_ACCEPTANCE_SNAPSHOT").expect("新快照");
        assert!(bcd_identifier_from_line(&snapshot).is_some());
        let root = std::env::var("BR_ACCEPTANCE_ROOT").expect("任务根目录");
        let id = std::env::var("BR_ACCEPTANCE_TASK").expect("任务ID");
        let efi = PathBuf::from(std::env::var("BR_ACCEPTANCE_EFI").expect("已挂载ESP"));
        let secondary = std::env::var("BR_ACCEPTANCE_SECONDARY").expect("已核验第二加载器");
        assert!(bcd_identifier_from_line(&secondary).is_some());
        let store = backuprestore_core::TaskStore::new(root);
        let task = store.load(&id).unwrap();
        assert_eq!(task.status, backuprestore_core::Stage::Success);
        assert_eq!(task.operation, backuprestore_core::Operation::CreateSecondary);
        let log = store
            .task_dir(&id)
            .unwrap()
            .join("menu-repair-acceptance.log");
        let previous = boot_manager_state_from_task_snapshot(&store, &task, &log).unwrap();
        preserve_primary_boot_manager(&efi, &previous, &secondary, &log).unwrap();
        println!("SECONDARY_MENU_REPAIR_VERIFIED");
    }

    #[test]
    fn bcd_identifier_parser_ignores_localized_field_labels() {
        assert_eq!(
            bcd_identifier_from_line(
                "\u{fffd}\u{fffd}\u{fffd} {3f66f61e-a1ce-11f1-9d29-f5ba5ed7cdd2}"
            ),
            Some("{3f66f61e-a1ce-11f1-9d29-f5ba5ed7cdd2}")
        );
        assert_eq!(bcd_identifier_from_line("device partition=H:"), None);
        assert_eq!(bcd_identifier_from_line("{not-a-bcd-guid}"), None);
    }

    #[test]
    fn secondary_loader_selection_ignores_resume_object() {
        let output = "Windows Boot Loader\nidentifier {11111111-2222-4333-8444-555555555555}\ndevice partition=Q:\nosdevice partition=Q:\npath \\Windows\\system32\\winload.efi\n\nWindows Resume Application\nidentifier {22222222-3333-4444-8555-666666666666}\ndevice partition=Q:\npath \\Windows\\system32\\winresume.efi";
        let selected = output
            .split("\n\n")
            .filter_map(|block| {
                let identifier = block.lines().find_map(bcd_identifier_from_line)?;
                let lower = block.to_ascii_lowercase();
                let targets_partition = lower.lines().any(|line| {
                    let line = line.trim_start();
                    (line.starts_with("device") || line.starts_with("osdevice"))
                        && line.contains("partition=q:")
                });
                (lower.contains("winload.efi") && targets_partition).then_some(identifier)
            })
            .next();
        assert_eq!(selected, Some("{11111111-2222-4333-8444-555555555555}"));
    }

    #[test]
    fn boot_manager_state_parser_accepts_guid_and_alias_only() {
        assert_eq!(
            parse_boot_manager_state(
                "Firmware Boot Manager\nidentifier {aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\ndisplayorder {ffffffff-1111-4222-8333-444444444444}\n\nWindows Boot Manager\nidentifier {9dea862c-5cdd-4e70-acc1-f32b344d4795}\ndefault {11111111-2222-4333-8444-555555555555}\ndisplayorder {11111111-2222-4333-8444-555555555555}"
            )
            .unwrap()
            .default,
            "{11111111-2222-4333-8444-555555555555}"
        );
        assert_eq!(
            parse_boot_manager_state(
                "Windows Boot Manager\nidentifier {9dea862c-5cdd-4e70-acc1-f32b344d4795}\n默认 {default}\n显示顺序 {default}"
            )
            .unwrap()
            .display_order,
            vec!["{default}"]
        );
        assert!(parse_boot_manager_state("default dangerous;value").is_none());
    }

    #[test]
    fn boot_manager_state_parser_preserves_resume_object_and_loader_mapping() {
        let state = parse_boot_manager_state(
            "Windows Boot Manager\nidentifier {9dea862c-5cdd-4e70-acc1-f32b344d4795}\ndefault {11111111-2222-4333-8444-555555555555}\nresumeobject {aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\ndisplayorder {11111111-2222-4333-8444-555555555555}\n\nWindows Boot Loader\nidentifier {11111111-2222-4333-8444-555555555555}\ndevice partition=C:\npath \\Windows\\system32\\winload.efi\nresumeobject {bbbbbbbb-2222-4333-8444-555555555555}",
        )
        .unwrap();
        assert_eq!(
            state.resume_object.as_deref(),
            Some("{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}")
        );
        assert_eq!(
            state.loader_resume_objects,
            vec![(
                "{11111111-2222-4333-8444-555555555555}".into(),
                "{bbbbbbbb-2222-4333-8444-555555555555}".into(),
            )]
        );
    }

    #[test]
    fn boot_menu_does_not_absorb_tools_or_later_guid_fields() {
        for field in ["displayorder", "显示顺序"] {
            let state = parse_boot_manager_state(&format!(
                "identifier {{9dea862c-5cdd-4e70-acc1-f32b344d4795}}\r\ndefault {{default}}\r\n{field} {{default}}\r\n                        {{current}}\r\ntoolsdisplayorder {{memdiag}}\r\n                        {{other}}\r\ntimeout 0"
            )).unwrap();
            assert_eq!(state.display_order, ["{default}", "{current}"]);
        }
    }
}
