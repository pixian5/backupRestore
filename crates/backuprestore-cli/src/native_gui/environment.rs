//! 宿主环境探测、语言配置与文件浏览控制器
//!
//! 负责：
//! - 界面语言偏好与字号持久化与应用（, , ）；
//! - 进程提权状态探测与提权拉起（, ）；
//! - 系统驱动器探测、默认卷推断与环境刷新（, ）；
//! - 历史任务状态刷新与多字段展示（）；
//! - 备份镜像读取与文件/目录系统对话框浏览（, , ）；
//! - 测试自动化安装钩子（）。

use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use super::*;
// ---------------------------------------------------------------------------
// 界面语言偏好：记住用户上一次选的语言（`BackupRestore.settings.ini` 的
// `ui_language` 键）。
//
// 2026-10-25 补：此前语言切换只在进程内生效，**每次启动都回到中文**——右上角
// 选了 English，关掉再开又是中文，等于开关是"临时"的。用户没提，但这是半截
// 功能（同文件里 failure_threshold / dism_timeout_secs 都是持久化的）。
// ---------------------------------------------------------------------------
/// 键名单一来源在 `i18n`（CLI 侧 `init_from_env` 也读同一个键）。
const SETTINGS_UI_LANGUAGE_KEY: &str = crate::i18n::SETTINGS_UI_LANGUAGE_KEY;

pub(crate) fn settings_ini_path() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(crate::failure_policy::SETTINGS_FILE_NAME)))
}

/// 读界面语言偏好。文件缺失 / 读失败 / 值不认识 → 中文（GUI 必须能起来）。
pub(crate) fn load_ui_language_pref() -> Language {
    let Some(path) = settings_ini_path() else {
        return Language::Chinese;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Language::Chinese;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == SETTINGS_UI_LANGUAGE_KEY {
            return match value.trim().to_ascii_lowercase().as_str() {
                "en" | "english" => Language::English,
                _ => Language::Chinese,
            };
        }
    }
    Language::Chinese
}

/// 保存界面语言偏好（**合并写**）。
///
/// 不能整文件重写：`BackupRestore.settings.ini` 是共享文件，
/// `failure_policy` 与 `dism_timeout` 都往同一个文件写。整文件重写会把
/// 失败策略和 DISM 上限静默冲掉（同文件里那段注释记着这个教训）。
///
/// 注释处理：只剔除**自己**那一条注释再重写，其余注释原样保留——不能像别处
/// 那样把所有注释一起丢掉，否则每次保存都会顺手抹掉别人的说明。
pub(crate) fn save_ui_language_pref(language: Language) {
    let Some(path) = settings_ini_path() else {
        return;
    };
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut kept: Vec<String> = existing
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("# ui_language:") {
                return false;
            }
            if trimmed.starts_with('#') || trimmed.starts_with(';') {
                return true;
            }
            match trimmed.split_once('=') {
                Some((key, _)) => key.trim() != SETTINGS_UI_LANGUAGE_KEY,
                None => true,
            }
        })
        .map(|line| line.to_string())
        .collect();
    if kept.is_empty() {
        kept.push("# BackupRestore 程序设置（本文件由程序自动生成，可手工编辑）".to_string());
    }
    let value = match language {
        Language::Chinese => "zh",
        Language::English => "en",
    };
    kept.push(
        "# ui_language: 界面语言 zh=中文 en=English；在界面右上角语言栏切换时自动保存".to_string(),
    );
    kept.push(format!("{SETTINGS_UI_LANGUAGE_KEY}={value}"));
    let mut text = kept.join("\r\n");
    text.push_str("\r\n");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, text);
}

pub(crate) unsafe fn apply_language(state: &mut State) {
    let language = selected_language(state);
    // 界面语言同步给全局语言层：此后本进程写进日志的结论性文案也跟着走，
    // 不再"界面英文、日志中文"（此前日志无语言维度，切了界面也没用）。
    crate::i18n::set(match language {
        Language::Chinese => crate::pe_safety::MessageLanguage::Chinese,
        Language::English => crate::pe_safety::MessageLanguage::English,
    });
    set_operation_tabs(state, language);
    let desired = [
        None,
        selected_drive_letter(state, state.controls.source),
        selected_drive_letter(state, state.controls.target),
    ];
    set_drive_items(state, desired);
    set_wim_items(state);
    // 压缩下拉随语言重填（reset + 重填 + 保持原选择，无选择时默认「压缩」）。
    let compress_position = combo_selection(state.controls.compress);
    reset_combo(state.controls.compress);
    for label in compress_level_labels(language) {
        add_combo_item(state.controls.compress, label);
    }
    SendMessageW(
        state.controls.compress,
        CB_SETCURSEL,
        compress_position.unwrap_or(0),
        0,
    );
    for (id, key) in [
        (2001, "operation"),
        (2003, "source"),
        (2004, "image"),
        (2005, "target"),
        (2007, "index"),
        (2008, "menu"),
        (2014, "compress"),
        (2015, "index_name"),
        (2016, "keep"),
        (ID_VERIFY_HASH, "verify_hash"),
        (ID_LANGUAGE_LABEL, "language"),
    ] {
        set_child_text(state.root, id, ui_text(language, key));
    }
    for (id, key) in [
        (ID_REFRESH, "refresh"),
        (ID_READ_IMAGE, "read_image"),
        (ID_BROWSE_IMAGE, "browse"),
        (ID_PE_DIR_BROWSE, "browse"),
        (ID_CREATE_TASK, "create_task"),
        (ID_REFRESH_TASK, "refresh_task"),
        (ID_SETTINGS, "settings"),
        (ID_IMAGE_ADMIN, "image_admin"),
        (ID_VIEW_LOGS, "view_logs"),
        (ID_CANCEL_PENDING, "cancel_pending"),
    ] {
        set_child_text(state.root, id, ui_text(language, key));
    }
    // The default secondary boot entry is user-editable, but its initial
    // value must follow the selected language instead of leaking the English
    // placeholder into an otherwise Chinese page.
    let default_menu_name = if language == Language::Chinese {
        "Windows 备份"
    } else {
        "Windows Backup"
    };
    let existing_menu_name = get_text(state.controls.menu);
    if existing_menu_name.is_empty()
        || existing_menu_name == "Windows Backup"
        || existing_menu_name == "Windows 备份"
    {
        set_menu_name(state, default_menu_name);
    }
    set_drive_details(state);
    set_volume_labels(state);
    set_operation_visibility(state);
    layout_operation(state);
    set_operation_guidance(state);
    install_tooltips(state);
}

pub(crate) fn rust_cli_output(arguments: &[&str]) -> Result<String, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let output = Command::new(executable)
        .creation_flags(CREATE_NO_WINDOW)
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&output.stderr).into_owned();
    }
    if !output.status.success() {
        return Err(text.trim().to_string());
    }
    Ok(text.trim().to_string())
}

pub(crate) unsafe fn show_message(hwnd: Hwnd, text: &str, caption: &str, flags: u32) -> i32 {
    // PE 桌面自动点击模式：所有对话框自动接受（等效用户持续点"是"），
    // 避免在无输入注入通道的 PE 里阻塞自动化流程。
    if PE_AUTO_CLICK.load(std::sync::atomic::Ordering::SeqCst) {
        let style = flags & 0x0f;
        return if style == 0x04 /* MB_YESNO */ || style == 0x03
        /* MB_YESNOCANCEL */
        {
            IDYES
        } else {
            IDOK
        };
    }
    let text = wide(text);
    let caption = wide(caption);
    MessageBoxW(hwnd, text.as_ptr(), caption.as_ptr(), flags)
}


// 这两个模态框的控件 ID 留在父模块：`CONTROL_NAMES` 审计表（`WM_COMMAND` 入口
// 统一记 `GUI action: control=<名> id=<id>`）要按 id 反查名字；常量搬进子模块后
// 父模块就看不见了，只能复制一份——那正是拆分要避免的东西。
/// 顶栏「设置…」按钮的处理：弹窗 → 保存到设置文件 → 更新内存策略与 DISM 上限。
/// `EnumChildWindows` 的回调签名。

/// 字号刷新期间由回调读取：本次要应用到所有子控件的字体句柄。
static FONT_TO_APPLY: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// 给每个子控件发 `WM_SETFONT`（`1` = 立即重绘）。
unsafe extern "system" fn apply_font_proc(hwnd: Hwnd, _l_param: LParam) -> i32 {
    let font = FONT_TO_APPLY.load(std::sync::atomic::Ordering::SeqCst);
    if font != 0 {
        SendMessageW(hwnd, WM_SETFONT, font as usize, 1);
    }
    1
}

/// 用户在设置里改了界面字号之后调用：换掉主窗口所有子控件的字体，并重排。
///
/// ★ 只换字体**不够**：行距和「按行数算出来的控件高度」都随字号变
/// （`dpi::line_height` / `dpi::text_block_height`），不重排的话详情框、状态框
/// 会维持旧高度，大字号下最后一行又被切成半截。
pub(crate) unsafe fn apply_font_size(state: &mut State) {
    let font = gui_font();
    if font.is_null() {
        return;
    }
    FONT_TO_APPLY.store(font as isize, std::sync::atomic::Ordering::SeqCst);
    EnumChildWindows(state.root, apply_font_proc, 0);
    FONT_TO_APPLY.store(0, std::sync::atomic::Ordering::SeqCst);
    layout_operation(state);
}

pub(crate) unsafe fn open_settings(state: &mut State) {
    let language = selected_language(state);
    let Some(policy) =
        ask_settings_dialog(state.root, language, state.failure_policy, &state.executable_dir)
    else {
        append_gui_log(state, "settings dialog: cancelled (policy unchanged)");
        return;
    };
    state.failure_policy = policy;
    // DISM 等待上限、pause 最长等待与失败策略写在**同一个**文件里、由同一次
    // 保存触发。用哨兵区分"本次没保存"（取消/关窗）与"保存成 0"（= 不限制）。
    let saved_dism_secs = settings_dialog::SETTINGS_DISM_TIMEOUT_RESULT.load(std::sync::atomic::Ordering::SeqCst);
    let dism_secs = if saved_dism_secs == settings_dialog::SETTINGS_RESULT_UNSET {
        state.dism_timeout_secs
    } else {
        saved_dism_secs
    };
    state.dism_timeout_secs = dism_secs;
    let saved_pause_wait = settings_dialog::SETTINGS_PAUSE_WAIT_RESULT.load(std::sync::atomic::Ordering::SeqCst);
    let pause_wait_secs = if saved_pause_wait == settings_dialog::SETTINGS_RESULT_UNSET {
        state.failure_policy.pause_max_wait_secs
    } else {
        saved_pause_wait
    };
    let mut failures: Vec<String> = Vec::new();
    let policy_with_wait = crate::failure_policy::FailurePolicy {
        pause_max_wait_secs: pause_wait_secs,
        ..policy
    };
    state.failure_policy = policy_with_wait;
    if let Err(error) = policy_with_wait.save_to_dir(&state.executable_dir) {
        failures.push(format!("failure policy: {error}"));
    }
    if let Err(error) = crate::dism_timeout::save_to_dir(dism_secs, &state.executable_dir) {
        failures.push(format!("DISM timeout: {error}"));
    }
    // 界面字号：与其它设置同一个文件、同一次保存触发。用哨兵区分
    //「本次没保存」与「保存成某个值」（字号最小 10，0 不是合法值，但保持
    // 与其它 RESULT 一致的写法）。改完立即对当前窗口生效。
    let saved_font_size =
        settings_dialog::SETTINGS_FONT_SIZE_RESULT.load(std::sync::atomic::Ordering::SeqCst);
    if saved_font_size != settings_dialog::SETTINGS_RESULT_UNSET
        && crate::dpi::set_font_size(saved_font_size)
    {
        if let Err(error) = crate::dpi::save_font_size(&state.executable_dir, saved_font_size) {
            failures.push(format!("font size: {error}"));
        }
        apply_font_size(state);
    }
    if failures.is_empty() {
        append_gui_log(
            state,
            &format!(
                "settings saved: threshold={} action={} pause_max_wait_secs={} dism_timeout_secs={}",
                policy_with_wait.threshold,
                policy_with_wait.action.env_name(),
                pause_wait_secs,
                dism_secs
            ),
        );
        return;
    }
    // 写盘失败不能让内存里的设置与界面不一致：如实报错，但仍然生效本次会话
    //（下次启动会回到旧值——把这件事明确告诉用户，而不是静默丢掉）。
    let joined = failures.join("；");
    append_gui_log(
        state,
        &format!("settings save failed: {joined} (in-memory settings still applied)"),
    );
    show_message(
        state.root,
        &if language == Language::English {
            format!(
                "The settings are applied for this session, but could not be saved to the program directory: {joined}"
            )
        } else {
            format!("设置已在本次会话生效，但写入程序目录失败：{joined}")
        },
        if language == Language::English {
            "Settings not saved"
        } else {
            "设置未保存"
        },
        MB_OK | MB_ICONWARNING,
    );
}

pub(crate) unsafe fn is_elevated() -> bool {
    let mut token = null_mut();
    if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 || token.is_null() {
        return false;
    }
    let mut elevation = TokenElevation {
        token_is_elevated: 0,
    };
    let mut returned = 0;
    let result = GetTokenInformation(
        token,
        TOKEN_ELEVATION_CLASS,
        &mut elevation as *mut TokenElevation as *mut c_void,
        size_of::<TokenElevation>() as u32,
        &mut returned,
    );
    CloseHandle(token);
    result != 0 && elevation.token_is_elevated != 0
}

pub(crate) unsafe fn relaunch_elevated() -> Result<(), backuprestore_core::TaskError> {
    let executable = std::env::current_exe()
        .map_err(|error| crate::err(&format!("cannot resolve current executable: {error}")))?;
    let executable = wide(&executable.to_string_lossy());
    let verb = wide("runas");
    // Preserve launch options such as `--open-image` across UAC elevation.
    // ShellExecuteW does not inherit argv when its parameter pointer is null.
    let parameters = std::env::args()
        .skip(1)
        .map(|value| quote_argument(&value))
        .collect::<Vec<_>>()
        .join(" ");
    let parameters = wide(&parameters);
    let result = ShellExecuteW(
        null_mut(),
        verb.as_ptr(),
        executable.as_ptr(),
        if parameters.len() > 1 {
            parameters.as_ptr()
        } else {
            null()
        },
        null(),
        SW_MAXIMIZE,
    );
    if result <= 32 {
        return Err(crate::err(&format!(
            "elevated GUI launch failed with ShellExecute code {result}"
        )));
    }
    Ok(())
}

unsafe extern "system" fn close_previous_gui_window(hwnd: Hwnd, current_process: LParam) -> i32 {
    let mut process_id = 0;
    GetWindowThreadProcessId(hwnd, &mut process_id);
    if process_id == current_process as u32 {
        return 1;
    }

    let mut class_name = [0u16; 64];
    let written = GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32);
    if written > 0
        && String::from_utf16_lossy(&class_name[..written as usize]) == "BackupRestoreNativeGui"
    {
        // This is a cooperative close on the same interactive desktop, not a
        // process termination. It lets the old GUI release its package files.
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
    }
    1
}

pub(crate) unsafe fn close_previous_gui_windows() {
    EnumWindows(
        Some(close_previous_gui_window),
        GetCurrentProcessId() as LParam,
    );
    // Give normal WM_CLOSE handling a short chance to release an old package
    // before the latest instance proceeds to occupy the foreground.
    Sleep(200);
}

pub(crate) unsafe fn refresh_environment(state: &mut State) {
    let language = selected_language(state);
    append_gui_log(state, "GUI action started: refresh environment");
    let desired = [
        None,
        selected_drive_letter(state, state.controls.source),
        selected_drive_letter(state, state.controls.target),
    ];
    set_text(
        state.controls.status,
        if language == Language::English {
            "Refreshing Windows, WinRE and NTFS volume information…"
        } else {
            "正在刷新 Windows、WinRE 和 NTFS 卷信息…"
        },
    );
    state.drives = discover_drives();
    set_drive_items(state, desired);
    set_drive_details(state);
    // 卷列表就绪后按真实选中的源卷重算一次备份索引名（建窗时是按"源盘=系统盘"
    // 预填的，若实际选中了别的卷要换成带盘符的降级名）。用户手改过则不动。
    refresh_backup_index_name(state);
    // 刷新卷列表后，A（还原系统到 C 盘）的目标下拉被重建——重新锁定 %SystemDrive%，
    // 防止刷新后目标停留在刷新前选中的卷（下拉已禁用，但显示必须始终是系统盘）。
    if selected_operation(state) == "restore-existing" {
        let letter = active_system_letter();
        select_drive(state, state.controls.target, &letter);
    }
    let text = match rust_cli_output(&["inspect-environment"]) {
        Ok(output) => output,
        Err(error) => {
            append_gui_log(
                state,
                &format!("GUI action failed: refresh environment: {error}"),
            );
            set_text(state.controls.status, &format!("环境检查失败：{error}"));
            return;
        }
    };
    let report: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            append_gui_log(
                state,
                &format!("GUI action failed: parse environment report: {error}"),
            );
            set_text(state.controls.status, &format!("环境信息解析失败：{error}"));
            return;
        }
    };
    let system = json_text(&report, "windows");
    let architecture = json_text(&report, "architecture");
    let winre = report
        .get("winreAvailable")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let text = if language == Language::English {
        format!("Windows: {system}\nArchitecture: {architecture}\nWinRE available: {winre}")
    } else {
        format!(
            "Windows：{system}\n架构：{architecture}\n恢复环境可用：{}",
            if winre { "是" } else { "否" }
        )
    };
    let volume_details = if state.drives.is_empty() {
        if language == Language::English {
            "\n\nEligible volumes: none".to_string()
        } else {
            "\n\n可选择卷：无".to_string()
        }
    } else if language == Language::English {
        format!(
            "\n\nEligible volumes:\n{}",
            state
                .drives
                .iter()
                .map(|drive| drive_display(drive, language))
                .collect::<Vec<_>>()
                .join("\n")
        )
    } else {
        format!(
            "\n\n可选择卷：\n{}",
            state
                .drives
                .iter()
                .map(|drive| drive_display(drive, language))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    set_text(state.controls.status, &(text + &volume_details));
    append_gui_log(
        state,
        &format!(
            "GUI action completed: refresh environment; eligible_volumes={}",
            state.drives.len()
        ),
    );
}

pub(crate) unsafe fn refresh_task_status(state: &State) {
    let language = selected_language(state);
    append_gui_log(state, "GUI action started: refresh latest task status");
    set_text(
        state.controls.status,
        if language == Language::English {
            "Reading the latest task status…"
        } else {
            "正在读取最近任务状态…"
        },
    );
    // ① 先读 last-task.json：它给出任务身份与**日志文件在哪**。
    let mut lines: Vec<String> = Vec::new();
    let mut status_path: Option<PathBuf> = None;
    let english = language == Language::English;
    match std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("last-task.json")))
        .filter(|path| path.is_file())
        .and_then(|path| fs::read_to_string(path).ok())
    {
        Some(value) => match serde_json::from_str::<serde_json::Value>(&value) {
            Ok(report) => {
                let task_id = json_text(&report, "taskId");
                let operation = json_text(&report, "operation");
                let task_root = json_text(&report, "taskRoot");
                let prepare_log = json_text(&report, "prepareLog");
                let recovery_log = json_text(&report, "recoveryLog");
                // `statusJson` 是准备阶段就写好的绝对路径；老版本没有该字段时
                // 按任务目录兜底（status.json 恒在任务目录里）。
                let configured = json_text(&report, "statusJson");
                status_path = if !configured.is_empty() {
                    Some(PathBuf::from(configured))
                } else if task_root.is_empty() {
                    None
                } else {
                    Some(Path::new(&task_root).join("status.json"))
                };
                let sep = if english { ": " } else { "：" };
                for (key, value) in [
                    (status_field_label(language, "task_id"), task_id),
                    (status_field_label(language, "operation_field"), operation),
                    (status_field_label(language, "task_root"), task_root),
                    (status_field_label(language, "prepare_log"), prepare_log),
                    (status_field_label(language, "recovery_log"), recovery_log),
                ] {
                    lines.push(format!("{key}{sep}{value}"));
                }
            }
            Err(error) => lines.push(format!(
                "{}{error}",
                if english {
                    "Task record read failed: "
                } else {
                    "读取任务记录失败："
                }
            )),
        },
        None => lines.push(if english {
            "No recent task record found.".to_string()
        } else {
            "尚未找到最近任务记录。".to_string()
        }),
    }

    // ② 再读 status.json：这里才是「成功还是失败、为什么失败」。
    //    ★ 四种结局必须分开表述——「读不到」与「读出来是失败」是两件事，
    //    混成一句「状态不可用」会让排查无法定位是哪一侧坏了。
    let outcome = match status_path.as_deref() {
        Some(path) if path.is_file() => match fs::read_to_string(path) {
            Err(error) => StatusOutcome::Unreadable(error.to_string()),
            Ok(text) => match crate::status_receipt::parse_status(&text) {
                Ok(snapshot) => StatusOutcome::Ok(snapshot),
                Err(error) => StatusOutcome::Malformed(error),
            },
        },
        // 路径没配置、或文件还没生成（任务尚未进入恢复环境）。
        _ => StatusOutcome::Missing,
    };
    let (outcome_name, code_field, error_len) = match &outcome {
        StatusOutcome::Ok(status) => (
            "ok",
            status.error_code_text.clone().unwrap_or_default(),
            status.error.as_ref().map(String::len).unwrap_or(0),
        ),
        StatusOutcome::Missing => ("missing", String::new(), 0),
        StatusOutcome::Unreadable(_) => ("unreadable", String::new(), 0),
        StatusOutcome::Malformed(_) => ("malformed", String::new(), 0),
    };
    match &outcome {
        StatusOutcome::Ok(status) => {
            lines.extend(crate::status_receipt::render_status_lines(status, language))
        }
        other => lines.push(crate::status_receipt::render_outcome_line(other, language)),
    }
    set_text(state.controls.status, &lines.join("\n"));
    // 结构化日志行：自动化判据只认这三个字段，不解析中文文案。
    append_gui_log(
        state,
        &format!(
            "GUI action completed: refresh latest task status; status_outcome={} code={} error_len={} lines={}",
            outcome_name,
            if code_field.is_empty() { "none" } else { &code_field },
            error_len,
            lines.len()
        ),
    );
}

/// 「刷新任务状态」里那几个字段名（随界面语言切换）。
pub(crate) fn status_field_label(language: Language, key: &str) -> &'static str {
    match (language, key) {
        (Language::Chinese, "task_id") => "任务 ID",
        (Language::Chinese, "operation_field") => "操作",
        (Language::Chinese, "task_root") => "任务目录",
        (Language::Chinese, "prepare_log") => "准备日志",
        (Language::Chinese, "recovery_log") => "恢复日志",
        (_, "task_id") => "Task ID",
        (_, "operation_field") => "Operation",
        (_, "task_root") => "Task root",
        (_, "prepare_log") => "Prepare log",
        (_, "recovery_log") => "Recovery log",
        _ => "",
    }
}

pub(crate) fn discover_drives() -> Vec<DriveInfo> {
    let output = rust_cli_output(&["list-volumes"]).unwrap_or_default();
    parse_drive_infos(&output).unwrap_or_default()
}

pub(crate) fn suggested_drive_defaults(_drives: &[DriveInfo]) -> (String, String) {
    let system = std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".to_string())
        .trim()
        .trim_end_matches(':')
        .to_ascii_uppercase();
    (system, String::new())
}

pub(crate) fn same_partition_by_gui_identity(left: Option<&DriveInfo>, right: Option<&DriveInfo>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            if !left.volume_guid.is_empty() && !right.volume_guid.is_empty() {
                return left.volume_guid.eq_ignore_ascii_case(&right.volume_guid);
            }
            if !left.partition_type_guid.is_empty()
                && !right.partition_type_guid.is_empty()
                && left.disk_number.is_some()
                && left.partition_number.is_some()
                && left.disk_number == right.disk_number
                && left.partition_number == right.partition_number
            {
                return true;
            }
            left.letter.eq_ignore_ascii_case(&right.letter)
        }
        _ => false,
    }
}

pub(crate) unsafe fn read_image(state: &mut State) {
    let language = selected_language(state);
    let image_path = get_text(state.controls.image).trim().to_string();
    append_gui_log(
        state,
        &format!("GUI action started: read WIM metadata; image={image_path}"),
    );
    if let Err(error) = backuprestore_core::validate_absolute_path(&image_path) {
        let kind = if image_path.is_empty() { "empty" } else { "invalid" };
        append_gui_log(
            state,
            &format!(
                "GUI read WIM blocked: path kind={kind} detail={error}; guidance dialog opened"
            ),
        );
        set_text(
            state.controls.status,
            &if language == Language::English {
                if image_path.is_empty() {
                    "No image path entered. Choose [Choose image location] to pick a file, or [Cancel] to return.".to_string()
                } else {
                    "The image absolute path is invalid. Choose [Choose image location] to pick a file, or [Cancel] to return.".to_string()
                }
            } else if image_path.is_empty() {
                "尚未输入镜像绝对路径。请点击【选择镜像位置】选择文件，或点击【取消】返回。".to_string()
            } else {
                "镜像绝对路径无效。请点击【选择镜像位置】选择文件，或点击【取消】返回。".to_string()
            },
        );
        // 与「创建任务」同一套引导：空/非法路径弹提示框（非错误图标），
        // 提供【选择镜像位置】/【取消】；选浏览则回填后自动重读 WIM。
        let choice = ask_invalid_image_path(state.root, language, image_path.is_empty());
        append_gui_log(
            state,
            &format!("GUI read WIM guidance: user choice={choice} (1=browse 0=cancel)"),
        );
        if choice == 1 {
            browse_image(state);
            let updated = get_text(state.controls.image).trim().to_string();
            let valid = backuprestore_core::validate_absolute_path(&updated).is_ok();
            append_gui_log(
                state,
                &format!("GUI browse returned: updated={updated} valid={valid}"),
            );
            if valid {
                read_image(state);
            } else {
                SetFocus(state.controls.image);
            }
        }
        return;
    }
    let output = match rust_cli_output(&["wim-info", &image_path]) {
        Ok(value) => value,
        Err(error) => {
            append_gui_log(
                state,
                &format!("GUI action failed: read WIM metadata: {error}"),
            );
            state.wim_images.clear();
            set_wim_items(state);
            set_text(
                state.controls.status,
                &if language == Language::English {
                    format!("Could not read WIM metadata: {error}")
                } else {
                    format!("读取 WIM 详细信息失败：{error}")
                },
            );
            return;
        }
    };
    let report: serde_json::Value = match serde_json::from_str(&output) {
        Ok(value) => value,
        Err(error) => {
            append_gui_log(
                state,
                &format!("GUI action failed: parse WIM report: {error}"),
            );
            state.wim_images.clear();
            set_wim_items(state);
            set_text(state.controls.status, &format!("WIM 信息解析失败：{error}"));
            return;
        }
    };
    let images_json = report_images_value(&report);
    let images_text = match serde_json::to_string(&images_json) {
        Ok(value) => value,
        Err(error) => {
            append_gui_log(
                state,
                &format!("GUI action failed: serialize WIM index report: {error}"),
            );
            set_text(
                state.controls.status,
                &format!("WIM metadata serialization failed: {error}"),
            );
            return;
        }
    };
    match parse_wim_images(&images_text) {
        Ok(images) => {
            state.wim_images = images;
            set_wim_items(state);
            // S5：索引名预填菜单名文本框（用户手改过则不覆盖）。
            prefill_menu_name_from_image(state);
            let image = json_text(&report, "image");
            let sha256 = json_text(&report, "sha256");
            let metadata = json_text(&report, "metadataSha256");
            let minimum_target = json_text(&report, "minimumTarget");
            let details = if language == Language::English {
                format!(
                    "Image: {image}\nSHA-256: {sha256}\nMetadata SHA-256: {}\nMinimum target size: {}\nLoaded {} WIM image indexes. Select one from the dropdown.",
                    if metadata.is_empty() {
                        "not provided"
                    } else {
                        &metadata
                    },
                    if minimum_target.is_empty() {
                        "not provided"
                    } else {
                        &minimum_target
                    },
                    state.wim_images.len(),
                )
            } else {
                format!(
                    "镜像：{image}\nSHA-256：{sha256}\nmetadata SHA-256：{}\n最小目标容量：{}\n已读取 {} 个 WIM 索引，请从下拉框选择。",
                    if metadata.is_empty() {
                        "未提供"
                    } else {
                        &metadata
                    },
                    if minimum_target.is_empty() {
                        "未提供"
                    } else {
                        &minimum_target
                    },
                    state.wim_images.len(),
                )
            };
            set_text(state.controls.status, &details);
            append_gui_log(
                state,
                &format!(
                    "GUI action completed: read WIM metadata; indexes={}",
                    state.wim_images.len()
                ),
            );
        }
        Err(error) => {
            append_gui_log(
                state,
                &format!("GUI action failed: parse WIM indexes: {error}"),
            );
            state.wim_images.clear();
            set_wim_items(state);
            // ★ 按**字符**截断（H-GUI-1，2026-10-28 修）：这里原来是
            //   `&images_text[..4096]`，而 WIM 的 `ImageName` 几乎都是中文，
            //   4096 落在字符中间就 panic —— 而且这段代码本身就在「解析失败」的
            //   诊断路径上，于是「诊断代码成为崩溃源」。窗口过程 panic = 进程 abort。
            let diagnostic = crate::text_parsing::truncate_chars(&images_text, 4096);
            set_text(
                state.controls.status,
                &if language == Language::English {
                    format!("Could not parse WIM indexes: {error}\nRaw index JSON: {diagnostic}")
                } else {
                    format!("无法解析 WIM 索引：{error}\n原始索引 JSON：{diagnostic}")
                },
            );
        }
    }
}

pub(crate) unsafe fn browse_image(state: &mut State) {
    let language = selected_language(state);
    let operation = selected_operation(state);
    let mut buffer = vec![0_u16; 32768];
    let current = get_text(state.controls.image);
    let current_wide = wide(&current);
    let copy_len = current_wide.len().saturating_sub(1).min(buffer.len() - 1);
    buffer[..copy_len].copy_from_slice(&current_wide[..copy_len]);
    let filter = wide("WIM image (*.wim)\0*.wim\0All files (*.*)\0*.*\0\0");
    let title = wide(if operation == "backup" {
        if language == Language::English {
            "Choose backup image destination"
        } else {
            "选择备份镜像保存位置"
        }
    } else if language == Language::English {
        "Choose restore image"
    } else {
        "选择还原镜像"
    });
    let extension = wide("wim");
    let mut dialog = OpenFileNameW {
        l_struct_size: size_of::<OpenFileNameW>() as u32,
        hwnd_owner: state.root,
        h_instance: null_mut(),
        lpstr_filter: filter.as_ptr(),
        lpstr_custom_filter: null_mut(),
        n_max_cust_filter: 0,
        n_filter_index: 1,
        lpstr_file: buffer.as_mut_ptr(),
        n_max_file: buffer.len() as u32,
        lpstr_file_title: null_mut(),
        n_max_file_title: 0,
        lpstr_initial_dir: null(),
        lpstr_title: title.as_ptr(),
        flags: OFN_PATHMUSTEXIST
            | if operation == "backup" {
                0
            } else {
                OFN_FILEMUSTEXIST
            },
        n_file_offset: 0,
        n_file_extension: 0,
        lpstr_def_ext: extension.as_ptr(),
        l_cust_data: 0,
        lpfn_hook: null_mut(),
        lp_template_name: null(),
        pv_reserved: null_mut(),
        dw_reserved: 0,
        flags_ex: 0,
    };
    let accepted = if operation == "backup" {
        GetSaveFileNameW(&mut dialog)
    } else {
        GetOpenFileNameW(&mut dialog)
    };
    if accepted != 0 {
        let length = buffer.iter().position(|value| *value == 0).unwrap_or(0);
        let path = String::from_utf16_lossy(&buffer[..length]);
        set_text(state.controls.image, &path);
        append_gui_log(
            state,
            "GUI action completed: image path selected from file dialog",
        );
        let btn_task = GetDlgItem(state.root, ID_CREATE_TASK as i32);
        if backuprestore_core::validate_absolute_path(&path).is_ok() {
            set_text(
                btn_task,
                if language == Language::English {
                    "Create task"
                } else {
                    "创建任务"
                },
            );
        }
        if operation != "backup" && Path::new(&path).is_file() {
            read_image(state);
        }
    } else {
        append_gui_log(state, "GUI action cancelled: image file dialog");
    }
}

/// 「PE 目录路径」浏览按钮：弹出文件夹选择对话框（SHBrowseForFolderW，
/// 系统原生，ARM64 可用），选中后把完整路径回填到 ID_PE_DIR_EDIT。
pub(crate) unsafe fn browse_pe_dir(state: &State) {
    // BIF_NEWDIALOGSTYLE 要求调用线程先初始化 COM（OLE），否则对话框
    // 会立即失败返回；CoInitializeEx 返回 0(S_OK) 或 1(S_FALSE) 都算可用，
    // 0x80010106(RPC_E_CHANGED_MODE) 表示线程已是其他模式，跳过不配对。
    let co_init = CoInitializeEx(null(), COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    let com_ok = co_init == 0 || co_init == 1;
    let language = selected_language(state);
    let mut display_name = [0_u16; 260];
    let title = wide(if language == Language::English {
        "Choose PE folder"
    } else {
        "选择 PE 目录"
    });
    let info = BrowseInfoW {
        hwnd_owner: state.root,
        pidl_root: null(),
        display_name: display_name.as_mut_ptr(),
        title: title.as_ptr(),
        flags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        callback: None,
        l_param: 0,
        image: 0,
    };
    let pidl = SHBrowseForFolderW(&info);
    if com_ok {
        CoUninitialize();
    }
    if pidl.is_null() {
        append_gui_log(state, "GUI action cancelled: PE folder browse dialog");
        return;
    }
    let mut buffer = [0_u16; 32768];
    let ok = SHGetPathFromIDListW(pidl, buffer.as_mut_ptr());
    CoTaskMemFree(pidl);
    if ok == 0 {
        append_gui_log(state, "GUI action cancelled: PE folder browse dialog");
        return;
    }
    let length = buffer.iter().position(|value| *value == 0).unwrap_or(0);
    let path = String::from_utf16_lossy(&buffer[..length]);
    set_text(GetDlgItem(state.root, ID_PE_DIR_EDIT as i32), &path);
    append_gui_log(
        state,
        "GUI action completed: PE folder path selected from browse dialog",
    );
}

// 通用命令保留历史解释器语法；涉及用户输入的在线 DISM 走独立参数接口。
pub(crate) fn run_cmd_to_file(command: &str, out_file: Option<&Path>) -> CmdOutcome {
    run_cmd_to_file_timeout(command, out_file, 30000)
}
pub(crate) fn run_cmd_to_file_timeout(command: &str, out_file: Option<&Path>, timeout_ms: u32) -> CmdOutcome {
    // 历史调用方依赖覆盖文件；在线多阶段日志则由 run_program 追加。
    if let Some(path) = out_file
        && let Err(error) = std::fs::write(path, [])
    {
        return CmdOutcome::SpawnFailed(error.raw_os_error().unwrap_or(1) as u32);
    }
    crate::windows_command::run_shell(command, out_file, timeout_ms)
}

/// 安装 PE 入口：按启动方式单选分派到 RAM disk（目录，不占分区）
/// 或硬盘启动（独立分区）。两种模式可共存，启动项名称不同。
/// 测试钩子（开发/验收用）：启动时若存在 C:\br-test.json 则读取并自动
/// 设置安装 PE 参数；auto_install=true 时延迟触发安装（确认框自动接受）。
pub(crate) unsafe fn test_hook_auto_install(state: &mut State) {
    let path = "C:\\br-test.json";
    if !PathBuf::from(path).is_file() {
        return;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(value) => value,
        Err(_) => return,
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(_) => {
            append_gui_log(state, "test hook: br-test.json parse failed");
            return;
        }
    };
    append_gui_log(state, "test hook: config loaded");
    // 1. 操作模式：默认安装 PE；支持 "tab":"backup"/"restore"(A)/
    // "restore-data"(B)/"restore-other"或"secondary"(合并后的 C) 等
    let tab = json.get("tab").and_then(|v| v.as_str()).unwrap_or("pe");
    let op_index = match tab {
        "backup" => 0,
        "restore" => 1,
        "restore-data" => 2,
        "restore-other" => 3,
        // 2026-10-16 用户裁定：「还原系统到非 C 盘」与「新增第二系统」本就是同一个
        // 功能（只差文案），已合并为「新增非 C 盘第二系统」一个标签页，两个键都指 3。
        "secondary" => 3,
        _ => 4,
    };
    select_operation(state, op_index);
    // 1.5 B/C 入口的通道选择（"channel":"online"/"offline"，默认 online）：
    // 自动化验收离线链路时用它切到 offline。
    if let Some(channel) = json.get("channel").and_then(|v| v.as_str()) {
        let online = channel != "offline";
        append_gui_log(state, &format!("test hook: channel={channel}"));
        SendMessageW(
            GetDlgItem(state.root, ID_CHANNEL_ONLINE as i32),
            BM_SETCHECK,
            if online { BST_CHECKED } else { BST_UNCHECKED },
            0,
        );
        SendMessageW(
            GetDlgItem(state.root, ID_CHANNEL_OFFLINE as i32),
            BM_SETCHECK,
            if online { BST_UNCHECKED } else { BST_CHECKED },
            0,
        );
    }
    // 1.6 B 入口的「格式化目标分区」复选框（"format_target":false 取消勾选；
    // 不写默认勾选）。1a 阶段（手工格式化 + 只验还原链路）用它取消。
    if let Some(format_target) = json.get("format_target").and_then(|v| v.as_bool()) {
        append_gui_log(state, &format!("test hook: format_target={format_target}"));
        SendMessageW(
            GetDlgItem(state.root, ID_FORMAT_TARGET as i32),
            BM_SETCHECK,
            if format_target { BST_CHECKED } else { BST_UNCHECKED },
            0,
        );
    }
    // 2. 启动方式：ram / disk
    let mode_ram = json
        .get("mode")
        .and_then(|v| v.as_str())
        .map(|s| s == "ram")
        .unwrap_or(true);
    let ram_hwnd = GetDlgItem(state.root, ID_PE_MODE_RAM as i32);
    let disk_hwnd = GetDlgItem(state.root, ID_PE_MODE_DISK as i32);
    SendMessageW(
        ram_hwnd,
        BM_SETCHECK,
        if mode_ram { BST_CHECKED } else { BST_UNCHECKED },
        0,
    );
    SendMessageW(
        disk_hwnd,
        BM_SETCHECK,
        if mode_ram { BST_UNCHECKED } else { BST_CHECKED },
        0,
    );
    set_operation_visibility(state);
    layout_operation(state);
    set_volume_labels(state);
    // 3. 源卷（备份 tab 用 controls.source；PE 分支沿用 target_volume）
    if let Some(vol) = json.get("source_volume").and_then(|v| v.as_str()) {
        if let Some(pos) = state
            .drives
            .iter()
            .position(|d| d.letter.eq_ignore_ascii_case(vol))
        {
            SendMessageW(state.controls.source, CB_SETCURSEL, pos, 0);
            // 补发 CBN_SELCHANGE：CB_SETCURSEL 只是换选中项，不产生通知，
            // 父窗口的 WM_COMMAND 分支（set_drive_details + refresh_backup_index_name）
            // 不会跑——信息框会停驻在初始卷、备份索引名不随新源卷降级
            // （2026-10-18 实测：数据卷备份把系统版本名写进了 WIM，改不了）。
            // 与真实用户点选下拉等价。
            let notify = (CBN_SELCHANGE << 16) | ID_SOURCE;
            SendMessageW(state.root, WM_COMMAND, notify, state.controls.source as isize);
            append_gui_log(state, &format!("test hook: source={vol} pos={pos}"));
        } else {
            append_gui_log(state, &format!("test hook: source vol {vol} not found"));
        }
    }
    if let Some(vol) = json.get("target_volume").and_then(|v| v.as_str()) {
        let view = target_combo_indices(state);
        if let Some(pos) = drive_combo_position(view.as_deref(), state, vol) {
            SendMessageW(state.controls.target, CB_SETCURSEL, pos as usize, 0);
            // 同上：目标卷变更也要让父窗口刷新目标信息框等联动显示。
            let notify = (CBN_SELCHANGE << 16) | ID_TARGET;
            SendMessageW(state.root, WM_COMMAND, notify, state.controls.target as isize);
            append_gui_log(state, &format!("test hook: target={vol} pos={pos}"));
        } else {
            append_gui_log(state, &format!("test hook: target vol {vol} not found"));
        }
    }
    // 4. 目录 / 启动项名称 / 镜像路径
    if let Some(dir) = json.get("pe_dir").and_then(|v| v.as_str()) {
        set_text(GetDlgItem(state.root, ID_PE_DIR_EDIT as i32), dir);
    }
    if let Some(name) = json.get("pe_name").and_then(|v| v.as_str()) {
        set_text(GetDlgItem(state.root, ID_PE_NAME_EDIT as i32), name);
    }
    let image_value = json
        .get("image")
        .and_then(|v| v.as_str())
        .or_else(|| json.get("pe_image").and_then(|v| v.as_str()));
    if let Some(img) = image_value {
        set_text(state.controls.image, img);
    }
    append_gui_log(state, "test hook: fields set");
    // 5. 智能分流选择（0=取消 1=PE 2=RE）：设置后点「创建任务」时直接采用，
    //    跳过弹窗与确认框，便于在无法精确点击弹窗按钮的自动化环境里验证路由。
    state.test_drive_choice = json
        .get("system_drive_choice")
        .and_then(|v| v.as_i64())
        .filter(|v| *v >= 0 && *v <= 2)
        .map(|v| v as i32);
    if let Some(choice) = state.test_drive_choice {
        append_gui_log(state, &format!("test hook: system_drive_choice={choice}"));
    }
    // 6. 自动安装：跳过确认框，窗口显示后延迟触发
    if json
        .get("auto_install")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        TEST_AUTO_CONFIRM.store(true, std::sync::atomic::Ordering::SeqCst);
        PostMessageW(state.root, WM_APP_TEST_INSTALL, 0, 0);
        append_gui_log(state, "test hook: install posted");
    }
}
