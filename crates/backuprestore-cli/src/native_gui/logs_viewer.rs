//! 「查看日志」窗口（2026-10-26 从 `native_gui.rs` 拆出）。
//!
//! 子模块而不是顶层模块：它要用父模块一整套私有 Win32 类型/常量/辅助函数
//! （`use super::*`），做成顶层就得把它们全提成 `pub(crate)`。
//!
//! 只有 `open_logs_viewer` 被父模块调用；其余函数、窗口过程与这几个静态量都是
//! 本窗口私有的。`ID_LOGS_*` 控件常量**留在父模块**——父模块的 `CONTROL_NAMES`
//! 审计表（控件 ID → 审计名）也要用它们。
use super::*;

// 本窗口私有：类名、语言、清单快照、日志根目录与显示上限。
/// 日志查看窗口的类名。
const LOGS_WINDOW_CLASS: &str = "BackupRestoreLogsWindow";
/// 对话框语言（0=中文 / 1=英文），与备份文件管理窗口同形态。
static LOGS_LANGUAGE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// 本次列出的日志文件（「用记事本打开」按选中下标取路径）。
static LOGS_FILES: std::sync::Mutex<Vec<crate::log_root::LogFileEntry>> =
    std::sync::Mutex::new(Vec::new());
/// 日志根目录：与写日志方共用 `log_root::logs_dir()`，读方绝不另算路径——
/// 两个进程各解析一次，策略确定性保证结果一致。
static LOGS_DIR: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);
/// 清单最多显示多少条。再多是噪音：排障要的是本轮日志，不是全部历史。
const LOGS_LIST_LIMIT: usize = 20;

/// 顶栏「查看日志」：报障第一步就是「日志给我」。此前只有完成对话框里的一行
/// 路径，用户得自己在资源管理器里翻「动作/会话」两层目录找本轮日志——这里直接
/// 列出最近修改的日志文件，本轮排最前。
pub(super) unsafe fn open_logs_viewer(state: &mut State) {
    let language = selected_language(state);
    // 读方与写方共用 `log_root::logs_dir()`：GUI 与写日志的进程各解析一次，
    // 策略是确定性的，因此两边永远同源，不会出现「界面指的地方没有日志」。
    let dir = crate::log_root::logs_dir().to_path_buf();
    append_gui_log(state, &format!("logs viewer: dir={}", dir.display()));
    ask_logs_dialog(state.root, language, dir);
}

/// 修改时间的可读文本。拿不到就如实说「未知」——排序在
/// `recent_log_files` 里已按 `Option` 处理（无时间排最后），这里只是显示，
/// 绝不能编一个时间冒充。
fn log_time_text(modified: Option<std::time::SystemTime>, english: bool) -> String {
    match modified {
        Some(time) => chrono::DateTime::<chrono::Local>::from(time)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        None => (if english { "unknown" } else { "未知" }).to_string(),
    }
}

/// 清单行的显示文本：`相对路径（大小，修改时间）`。
///
/// 用**相对**日志根目录的路径：任务日志都在 `<动作>/<会话>/` 子目录里，
/// 只显示文件名会让 `backup\session\task.log` 与 `restore\session\task.log`
/// 变成两个一模一样的 `task.log`，用户无从分辨是哪一轮。
unsafe fn logs_fill_list(hwnd: Hwnd, english: bool) {
    let root = LOGS_DIR
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_default();
    let files = crate::log_root::recent_log_files(&root, LOGS_LIST_LIMIT);
    let combo = GetDlgItem(hwnd, ID_LOGS_COMBO as i32);
    reset_combo(combo);
    for entry in &files {
        let relative = entry.path.strip_prefix(&root).unwrap_or(entry.path.as_path());
        let text = if english {
            format!(
                "{}  ({}, {})",
                relative.display(),
                crate::format_size(entry.size),
                log_time_text(entry.modified, english)
            )
        } else {
            format!(
                "{}（{}，{}）",
                relative.display(),
                crate::format_size(entry.size),
                log_time_text(entry.modified, english)
            )
        };
        add_combo_item(combo, &text);
    }
    if files.is_empty() {
        set_child_text(hwnd, ID_LOGS_DETAIL, "");
        set_child_text(
            hwnd,
            ID_LOGS_HINT,
            &if english {
                format!("No log files under {}", root.display())
            } else {
                format!("{} 下还没有日志文件。", root.display())
            },
        );
        if let Ok(mut slot) = LOGS_FILES.lock() {
            slot.clear();
        }
        return;
    }
    set_child_text(
        hwnd,
        ID_LOGS_HINT,
        &if english {
            format!("Log folder: {}", root.display())
        } else {
            format!("日志目录：{}", root.display())
        },
    );
    if let Ok(mut slot) = LOGS_FILES.lock() {
        *slot = files;
    }
    SendMessageW(combo, CB_SETCURSEL, 0, 0);
    logs_show_detail(hwnd, english);
}

/// 详情框：完整绝对路径 + 大小 + 修改时间（清单行为宽度做过裁剪，
/// 完整路径在这里给出，复制去报障时用得上）。
unsafe fn logs_show_detail(hwnd: Hwnd, english: bool) {
    let combo = GetDlgItem(hwnd, ID_LOGS_COMBO as i32);
    let selected = combo_selection(combo).unwrap_or(0);
    let Some(entry) = LOGS_FILES
        .lock()
        .ok()
        .and_then(|slot| slot.get(selected).cloned())
    else {
        set_child_text(hwnd, ID_LOGS_DETAIL, "");
        return;
    };
    set_child_text(
        hwnd,
        ID_LOGS_DETAIL,
        &if english {
            format!(
                "Path: {}\nSize: {}\nModified: {}",
                entry.path.display(),
                crate::format_size(entry.size),
                log_time_text(entry.modified, english)
            )
        } else {
            format!(
                "路径：{}\n大小：{}\n修改时间：{}",
                entry.path.display(),
                crate::format_size(entry.size),
                log_time_text(entry.modified, english)
            )
        },
    );
}

/// 用系统关联程序（记事本）打开选中日志。`ShellExecuteW` 返回值 <= 32 是失败码，
/// 必须检查——否则点了没反应却没有任何提示，正是「点了没反应」类报障的来源。
unsafe fn logs_open_selected(hwnd: Hwnd, english: bool) {
    let combo = GetDlgItem(hwnd, ID_LOGS_COMBO as i32);
    let selected = combo_selection(combo).unwrap_or(0);
    let Some(entry) = LOGS_FILES
        .lock()
        .ok()
        .and_then(|slot| slot.get(selected).cloned())
    else {
        return;
    };
    let path = wide(&entry.path.to_string_lossy());
    let verb = wide("open");
    let result = ShellExecuteW(
        null_mut(),
        verb.as_ptr(),
        path.as_ptr(),
        null_mut(),
        null_mut(),
        SW_SHOWNORMAL,
    );
    if result <= 32 {
        show_message(
            hwnd,
            &if english {
                format!("Cannot open the log file (code {result}).")
            } else {
                format!("无法打开日志文件（代码 {result}）。")
            },
            if english { "Open failed" } else { "打开失败" },
            MB_OK | MB_ICONWARNING,
        );
    }
}

/// 打开日志根目录：清单只显示最近 N 条，要看全部（或打包发给别人）就进目录。
unsafe fn logs_open_dir(hwnd: Hwnd, english: bool) {
    let Some(root) = LOGS_DIR.lock().ok().and_then(|slot| slot.clone()) else {
        return;
    };
    let path = wide(&root.to_string_lossy());
    let verb = wide("open");
    let result = ShellExecuteW(
        null_mut(),
        verb.as_ptr(),
        path.as_ptr(),
        null_mut(),
        null_mut(),
        SW_SHOWNORMAL,
    );
    if result <= 32 {
        show_message(
            hwnd,
            &if english {
                format!("Cannot open the log folder (code {result}).")
            } else {
                format!("无法打开日志目录（代码 {result}）。")
            },
            if english { "Open failed" } else { "打开失败" },
            MB_OK | MB_ICONWARNING,
        );
    }
}

/// 模态外壳：与 `ask_image_admin_dialog` 同一写法（自定义类 + 静态量传值 +
/// 嵌套消息循环 + `IsWindow` 退出条件）。本窗口**只读**——唯一的外部动作是
/// 打开文件或目录，都不改任何文件。
unsafe fn ask_logs_dialog(hwnd: Hwnd, language: Language, dir: std::path::PathBuf) {
    let english = language == Language::English;
    let instance = GetModuleHandleW(null());
    let class_name = wide(LOGS_WINDOW_CLASS);
    let class = WndClassExW {
        cb_size: size_of::<WndClassExW>() as u32,
        style: 0,
        wnd_proc: Some(window_proc_logs),
        cb_cls_extra: 0,
        cb_wnd_extra: 0,
        h_instance: instance,
        h_icon: null_mut(),
        h_cursor: null_mut(),
        h_brush: null_mut(),
        menu_name: null(),
        class_name: class_name.as_ptr(),
        h_icon_sm: null_mut(),
    };
    if RegisterClassExW(&class) == 0 && GetLastError() != 1410 {
        show_message(
            hwnd,
            if english {
                "Cannot register the log window class."
            } else {
                "无法注册日志查看窗口类。"
            },
            if english { "Open failed" } else { "打开失败" },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    LOGS_LANGUAGE.store(
        if english { 1 } else { 0 },
        std::sync::atomic::Ordering::SeqCst,
    );
    if let Ok(mut slot) = LOGS_DIR.lock() {
        *slot = Some(dir);
    }
    if let Ok(mut slot) = LOGS_FILES.lock() {
        slot.clear();
    }
    let caption = wide(if english { "Logs" } else { "查看日志" });
    let screen_w = GetSystemMetrics(SM_CXSCREEN).max(680);
    let screen_h = GetSystemMetrics(SM_CYSCREEN).max(560);
    let dialog = CreateWindowExW(
        0,
        class_name.as_ptr(),
        caption.as_ptr(),
        WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
        (screen_w - 640) / 2,
        (screen_h - 480) / 2,
        640,
        480,
        hwnd,
        null_mut(),
        instance,
        null_mut(),
    );
    if dialog.is_null() {
        show_message(
            hwnd,
            if english {
                "Cannot create the log window."
            } else {
                "无法创建日志查看窗口。"
            },
            if english { "Open failed" } else { "打开失败" },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    EnableWindow(hwnd, 0);
    ShowWindow(dialog, SW_SHOW);
    let mut message = Msg {
        hwnd: null_mut(),
        message: 0,
        w_param: 0,
        l_param: 0,
        time: 0,
        point: Point { x: 0, y: 0 },
    };
    while IsWindow(dialog) != 0 && GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
        if IsDialogMessageW(dialog, &message) == 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    EnableWindow(hwnd, 1);
    SetForegroundWindow(hwnd);
}

unsafe extern "system" fn window_proc_logs(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    let english = LOGS_LANGUAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
    if message == WM_CREATE {
        create_control(
            hwnd,
            "STATIC",
            if english { "Log file:" } else { "日志文件：" },
            0,
            16,
            50,
            90,
            20,
            0,
        );
        create_control(
            hwnd,
            "COMBOBOX",
            "",
            WS_TABSTOP | CBS_DROPDOWNLIST | WS_VSCROLL | WS_BORDER,
            106,
            46,
            500,
            300,
            ID_LOGS_COMBO,
        );
        create_control(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP | WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY | WS_VSCROLL,
            16,
            80,
            596,
            250,
            ID_LOGS_DETAIL,
        );
        create_control(
            hwnd,
            "BUTTON",
            if english { "Open in Notepad" } else { "用记事本打开" },
            WS_TABSTOP | BS_DEFPUSHBUTTON,
            16,
            346,
            160,
            28,
            ID_LOGS_OPEN_FILE,
        );
        create_control(
            hwnd,
            "BUTTON",
            if english { "Open folder" } else { "打开日志目录" },
            WS_TABSTOP,
            184,
            346,
            140,
            28,
            ID_LOGS_OPEN_DIR,
        );
        create_control(
            hwnd,
            "BUTTON",
            if english { "Refresh" } else { "刷新" },
            WS_TABSTOP,
            332,
            346,
            80,
            28,
            ID_LOGS_REFRESH,
        );
        create_control(
            hwnd,
            "BUTTON",
            if english { "Close" } else { "关闭" },
            WS_TABSTOP,
            420,
            346,
            80,
            28,
            ID_LOGS_CLOSE,
        );
        create_control(
            hwnd,
            "STATIC",
            if english { "Reading the log list..." } else { "正在读取日志列表…" },
            0,
            16,
            386,
            600,
            44,
            ID_LOGS_HINT,
        );
        logs_fill_list(hwnd, english);
        return 0;
    }
    if message == WM_KEYDOWN && w_param as u32 == VK_ESCAPE {
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
        return 0;
    }
    if message == WM_CLOSE {
        DestroyWindow(hwnd);
        return 0;
    }
    if message == WM_COMMAND {
        let control_id = w_param & 0xffff;
        // 与主窗口同一条纪律：审计放在消息入口，新控件天然全覆盖。
        audit_control_action(control_id, (w_param >> 16) & 0xffff);
        match control_id {
            ID_LOGS_CLOSE => {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
                return 0;
            }
            ID_LOGS_COMBO => {
                logs_show_detail(hwnd, english);
                return 0;
            }
            ID_LOGS_REFRESH => {
                logs_fill_list(hwnd, english);
                return 0;
            }
            ID_LOGS_OPEN_FILE => {
                logs_open_selected(hwnd, english);
                return 0;
            }
            ID_LOGS_OPEN_DIR => {
                logs_open_dir(hwnd, english);
                return 0;
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, message, w_param, l_param)
}
