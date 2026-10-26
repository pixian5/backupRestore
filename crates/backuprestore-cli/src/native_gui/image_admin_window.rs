//! 「备份文件管理…」窗口（2026-10-26 从 `native_gui.rs` 拆出）。
//!
//! 子模块而不是顶层模块：它要用父模块一整套私有 Win32 类型/常量/辅助函数
//! （`use super::*`），做成顶层就得把它们全提成 `pub(crate)`。
//!
//! 只有 `open_image_admin` 被父模块调用；其余函数、窗口过程、worker 完成消息与
//! 那几个静态量都是本窗口私有的。`ID_IMAGE_ADMIN*` 控件常量**留在父模块** ——
//! 父模块的 `CONTROL_NAMES` 审计表与顶栏布局也要用它们。
use super::*;

// 本窗口私有：类名、两条 worker 完成消息、删除闸门与 worker 结果槽。
/// 备份文件管理窗口的类名。
const IMAGE_ADMIN_WINDOW_CLASS: &str = "BackupRestoreImageAdminWindow";
/// worker → 对话框的完成消息（自定义段 0x8003/0x8004，避开既有 0x8001/0x8002）。
const WM_APP_IMAGE_LIST_DONE: u32 = 0x8003;
const WM_APP_IMAGE_DELETE_DONE: u32 = 0x8004;
/// 删除进行中标志：与 `ONLINE_BUSY` 同形态但分开——两件互不相干的事不该互相挡住。
static IMAGE_ADMIN_BUSY: backuprestore_core::operation_safety::OperationGate =
    backuprestore_core::operation_safety::OperationGate::new();
/// 列表 worker 的结果。对话框打开期间只跑一轮，不会并发写。
static IMAGE_ADMIN_LIST_RESULT: std::sync::Mutex<Option<Vec<crate::image_admin::ImageSummary>>> =
    std::sync::Mutex::new(None);
/// 列表 worker 的失败原因（目录不存在 / DISM 失败等），非空时对话框弹窗告知。
static IMAGE_ADMIN_LIST_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// 删除 worker 的结果：`(删除数, 保留数, 镜像路径)`。
static IMAGE_ADMIN_DELETE_RESULT: std::sync::Mutex<Option<Result<(u32, u32, String), String>>> =
    std::sync::Mutex::new(None);
static IMAGE_ADMIN_LANGUAGE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// 本次对话框管理的目录（列表 worker 与「打开所在文件夹」共用）。
static IMAGE_ADMIN_DIR: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

// ── 备份文件管理（P1-c）───────────────────────────────

/// 顶栏「备份文件管理…」的处理：弹模态窗管理 `H:\BRImages`（缺省）下的镜像。
pub(super) unsafe fn open_image_admin(state: &mut State) {
    let language = selected_language(state);
    let dir = std::path::PathBuf::from(crate::default_image_dir());
    append_gui_log(state, &format!("image admin: dir={}", dir.display()));
    ask_image_admin_dialog(state.root, language, dir);
}

/// 模态外壳：与 `ask_settings_dialog` 同一写法（自定义类 + 静态量传值 +
/// 嵌套消息循环 + `IsWindow` 退出条件）。本对话框无「保存」语义——所有操作
/// （删除）在窗口内即时执行并各自确认。
unsafe fn ask_image_admin_dialog(hwnd: Hwnd, language: Language, dir: std::path::PathBuf) {
    let english = language == Language::English;
    let instance = GetModuleHandleW(null());
    let class_name = wide(IMAGE_ADMIN_WINDOW_CLASS);
    let class = WndClassExW {
        cb_size: size_of::<WndClassExW>() as u32,
        style: 0,
        wnd_proc: Some(window_proc_image_admin),
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
                "Cannot register the backup file window class."
            } else {
                "无法注册备份文件管理窗口类。"
            },
            if english { "Open failed" } else { "打开失败" },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    IMAGE_ADMIN_LANGUAGE.store(
        if english { 1 } else { 0 },
        std::sync::atomic::Ordering::SeqCst,
    );
    if let Ok(mut slot) = IMAGE_ADMIN_DIR.lock() {
        *slot = Some(dir);
    }
    if let Ok(mut slot) = IMAGE_ADMIN_LIST_RESULT.lock() {
        *slot = None;
    }
    if let Ok(mut slot) = IMAGE_ADMIN_LIST_ERROR.lock() {
        *slot = None;
    }
    if let Ok(mut slot) = IMAGE_ADMIN_DELETE_RESULT.lock() {
        *slot = None;
    }
    let caption = wide(if english {
        "Backup files"
    } else {
        "备份文件管理"
    });
    let screen_w = GetSystemMetrics(SM_CXSCREEN).max(680);
    let screen_h = GetSystemMetrics(SM_CYSCREEN).max(560);
    let dialog = CreateWindowExW(
        0,
        class_name.as_ptr(),
        caption.as_ptr(),
        WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
        (screen_w - 640) / 2,
        (screen_h - 560) / 2,
        640,
        560,
        hwnd,
        null_mut(),
        instance,
        null_mut(),
    );
    if dialog.is_null() {
        show_message(
            hwnd,
            if english {
                "Cannot create the backup file window."
            } else {
                "无法创建备份文件管理窗口。"
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

/// 列表 worker：`list_images` 每个镜像要跑一次 DISM，放后台线程，
/// 完成后 `WM_APP_IMAGE_LIST_DONE` 回对话框（与在线任务同一形态）。
unsafe fn spawn_image_list_worker(dialog: Hwnd) {
    let Some(dir) = IMAGE_ADMIN_DIR
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
    else {
        return;
    };
    let root = dialog as usize;
    let spawn = std::thread::Builder::new()
        .name("image-admin-list".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(|| {
                let (log_dir, _note) =
                    crate::online_log::resolve_dir("image-admin").map_err(|e| e.to_string())?;
                crate::image_admin::list_images(&dir, &log_dir.join("admin.log"))
                    .map_err(|e| e.to_string())
            })
            .unwrap_or_else(|payload| Err(crate::panic_payload_text(payload.as_ref())));
            match result {
                Ok(images) => {
                    if let Ok(mut slot) = IMAGE_ADMIN_LIST_RESULT.lock() {
                        *slot = Some(images);
                    }
                }
                Err(error) => {
                    if let Ok(mut slot) = IMAGE_ADMIN_LIST_ERROR.lock() {
                        *slot = Some(error);
                    }
                    if let Ok(mut slot) = IMAGE_ADMIN_LIST_RESULT.lock() {
                        *slot = Some(Vec::new());
                    }
                }
            }
            unsafe { PostMessageW(root as Hwnd, WM_APP_IMAGE_LIST_DONE, 0, 0) };
        });
    if spawn.is_err() {
        // 线程起不来：同步告知，不留「永久加载中」。
        if let Ok(mut slot) = IMAGE_ADMIN_LIST_ERROR.lock() {
            *slot = Some("无法启动列表线程".to_string());
        }
    }
}

/// 选中镜像的索引清单 + 「保留最近 N 个」默认值回填。
unsafe fn image_admin_show_detail(
    hwnd: Hwnd,
    images: &[crate::image_admin::ImageSummary],
) {
    let combo = GetDlgItem(hwnd, ID_IMAGE_ADMIN_COMBO as i32);
    let selected = combo_selection(combo).unwrap_or(0);
    let Some(image) = images.get(selected) else {
        set_child_text(hwnd, ID_IMAGE_ADMIN_DETAIL, "");
        return;
    };
    let mut text = String::new();
    text += &format!("{}\n", image.path.display());
    text += &format!(
        "文件大小 {}｜{} 个索引\n",
        crate::format_size(image.file_size),
        image.indexes.len()
    );
    text += "----------------------------------------\n";
    for index in &image.indexes {
        let entry = image.entries.get(index);
        let name = entry
            .and_then(|e| e.name.clone())
            .unwrap_or_else(|| "(无副档)".to_string());
        let created = entry
            .and_then(|e| e.created.clone())
            .unwrap_or_else(|| "-".to_string());
        let source = entry
            .and_then(|e| e.source.clone())
            .unwrap_or_else(|| "-".to_string());
        let size = entry
            .and_then(|e| e.image_size)
            .map(crate::format_size)
            .unwrap_or_else(|| "-".to_string());
        text += &format!("索引 {index:<3} {name:<22} {created}  {size:<10} {source}\n");
    }
    set_child_text(hwnd, ID_IMAGE_ADMIN_DETAIL, &text);
    // 默认「保留最近 N 个」= 当前索引数：默认动作是不删。
    set_child_text(
        hwnd,
        ID_IMAGE_ADMIN_KEEP_EDIT,
        &image.indexes.len().to_string(),
    );
}

/// 「打开所在文件夹」：资源管理器打开选中镜像所在目录。
unsafe fn image_admin_open_folder(hwnd: Hwnd) {
    let english = IMAGE_ADMIN_LANGUAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
    let Some(images) = IMAGE_ADMIN_LIST_RESULT
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
    else {
        return;
    };
    let combo = GetDlgItem(hwnd, ID_IMAGE_ADMIN_COMBO as i32);
    let selected = combo_selection(combo).unwrap_or(0);
    let Some(image) = images.get(selected) else {
        return;
    };
    let Some(parent) = image.path.parent() else {
        return;
    };
    let parent = wide(&parent.to_string_lossy());
    let verb = wide("open");
    let result = ShellExecuteW(
        null_mut(),
        verb.as_ptr(),
        parent.as_ptr(),
        null_mut(),
        null_mut(),
        SW_SHOWNORMAL,
    );
    if result <= 32 {
        let message = if english {
            format!("Cannot open the folder (code {result}).")
        } else {
            format!("无法打开所在文件夹（代码 {result}）。")
        };
        show_message(
            hwnd,
            &message,
            if english { "Open failed" } else { "打开失败" },
            MB_OK | MB_ICONWARNING,
        );
    }
}

/// 「删除旧索引…」：钳制 → 无删则告知 → 原生确认框列出将删的索引 → worker 执行。
unsafe fn image_admin_delete_flow(hwnd: Hwnd) {
    let english = IMAGE_ADMIN_LANGUAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
    let Some(images) = IMAGE_ADMIN_LIST_RESULT
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
    else {
        return;
    };
    let combo = GetDlgItem(hwnd, ID_IMAGE_ADMIN_COMBO as i32);
    let selected = combo_selection(combo).unwrap_or(0);
    let Some(image) = images.get(selected) else {
        return;
    };
    let total = image.indexes.len() as u32;
    let raw = get_text(GetDlgItem(hwnd, ID_IMAGE_ADMIN_KEEP_EDIT as i32));
    let keep = crate::image_admin::clamp_keep(raw.trim().parse::<u32>().unwrap_or(1), total);
    let removed = total - keep;
    if removed == 0 {
        show_message(
            hwnd,
            if english {
                "There are no old indexes to delete (the keep count already equals the index count)."
            } else {
                "没有需要删除的旧索引（保留数已等于当前索引数）。"
            },
            if english { "Nothing to delete" } else { "无需删除" },
            MB_OK | MB_ICONINFORMATION,
        );
        return;
    }
    // 破坏性确认：把将删的索引（号 + 名称）逐行列出来，不让用户猜。
    let mut doomed = String::new();
    for index in 1..=removed {
        let name = image
            .entries
            .get(&index)
            .and_then(|e| e.name.clone())
            .unwrap_or_default();
        doomed += &format!("  {index}  {name}\n");
    }
    let answer = show_message(
        hwnd,
        &if english {
            format!(
                "Delete the {removed} oldest index(es) of\n{}\nkeeping the latest {keep}:\n{doomed}\nThis cannot be undone. Delete?",
                image.path.display()
            )
        } else {
            format!(
                "将删除\n{}\n最旧的 {removed} 个索引，只保留最新 {keep} 个：\n{doomed}\n此操作不可撤销。确认删除？",
                image.path.display()
            )
        },
        if english {
            "Delete old indexes?"
        } else {
            "删除旧索引？"
        },
        MB_YESNO | MB_ICONWARNING,
    );
    if answer != IDYES {
        return;
    }
    if !IMAGE_ADMIN_BUSY.try_start() {
        show_message(
            hwnd,
            if english {
                "A delete operation is already running; wait for it to finish."
            } else {
                "已有删除操作正在进行，请等待完成。"
            },
            if english { "Busy" } else { "操作进行中" },
            MB_OK | MB_ICONWARNING,
        );
        return;
    }
    let path = image.path.clone();
    let root = hwnd as usize;
    let spawn = std::thread::Builder::new()
        .name("image-admin-delete".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(|| {
                let (log_dir, _note) =
                    crate::online_log::resolve_dir("image-admin").map_err(|e| e.to_string())?;
                crate::image_admin::delete_oldest_indexes(&path, keep, &log_dir.join("admin.log"))
                    .map(|(removed, kept)| (removed, kept, path.display().to_string()))
                    .map_err(|e| e.to_string())
            })
            .unwrap_or_else(|payload| Err(crate::panic_payload_text(payload.as_ref())));
            if let Ok(mut slot) = IMAGE_ADMIN_DELETE_RESULT.lock() {
                *slot = Some(result);
            }
            unsafe { PostMessageW(root as Hwnd, WM_APP_IMAGE_DELETE_DONE, 0, 0) };
        });
    if spawn.is_err() {
        IMAGE_ADMIN_BUSY.finish();
        show_message(
            hwnd,
            if english {
                "Cannot start the delete thread."
            } else {
                "无法启动删除线程。"
            },
            if english { "Start failed" } else { "启动失败" },
            MB_OK | MB_ICONERROR,
        );
    }
}

/// 备份文件管理对话框的消息处理。
unsafe extern "system" fn window_proc_image_admin(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    let english = IMAGE_ADMIN_LANGUAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
    if message == WM_CREATE {
        create_control(
            hwnd,
            "STATIC",
            if english { "WIM image:" } else { "镜像：" },
            0,
            16,
            14,
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
            10,
            500,
            300,
            ID_IMAGE_ADMIN_COMBO,
        );
        create_control(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP | WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY | WS_VSCROLL,
            16,
            44,
            596,
            330,
            ID_IMAGE_ADMIN_DETAIL,
        );
        create_control(
            hwnd,
            "STATIC",
            if english {
                "Keep latest"
            } else {
                "保留最近"
            },
            0,
            16,
            388,
            80,
            20,
            0,
        );
        create_control(
            hwnd,
            "EDIT",
            "1",
            WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL | ES_NUMBER,
            96,
            384,
            48,
            24,
            ID_IMAGE_ADMIN_KEEP_EDIT,
        );
        create_control(
            hwnd,
            "STATIC",
            if english {
                "indexes:"
            } else {
                "个索引："
            },
            0,
            150,
            388,
            70,
            20,
            0,
        );
        create_control(
            hwnd,
            "BUTTON",
            if english { "Refresh" } else { "刷新" },
            WS_TABSTOP,
            16,
            420,
            80,
            28,
            ID_IMAGE_ADMIN_REFRESH,
        );
        create_control(
            hwnd,
            "BUTTON",
            if english {
                "Delete old indexes..."
            } else {
                "删除旧索引…"
            },
            WS_TABSTOP | BS_DEFPUSHBUTTON,
            104,
            420,
            160,
            28,
            ID_IMAGE_ADMIN_DELETE,
        );
        create_control(
            hwnd,
            "BUTTON",
            if english {
                "Open folder"
            } else {
                "打开所在文件夹"
            },
            WS_TABSTOP,
            272,
            420,
            140,
            28,
            ID_IMAGE_ADMIN_OPEN_FOLDER,
        );
        create_control(
            hwnd,
            "BUTTON",
            if english { "Close" } else { "关闭" },
            WS_TABSTOP,
            420,
            420,
            80,
            28,
            ID_IMAGE_ADMIN_CLOSE,
        );
        create_control(
            hwnd,
            "STATIC",
            if english {
                "Reading the image list..."
            } else {
                "正在读取镜像列表…"
            },
            0,
            16,
            458,
            600,
            64,
            ID_IMAGE_ADMIN_HINT,
        );
        spawn_image_list_worker(hwnd);
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
    if message == WM_APP_IMAGE_LIST_DONE {
        let error = IMAGE_ADMIN_LIST_ERROR
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        let images = IMAGE_ADMIN_LIST_RESULT
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .unwrap_or_default();
        let combo = GetDlgItem(hwnd, ID_IMAGE_ADMIN_COMBO as i32);
        reset_combo(combo);
        for image in &images {
            let name = image
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| image.path.display().to_string());
            add_combo_item(combo, &name);
        }
        if images.is_empty() {
            let dir = IMAGE_ADMIN_DIR
                .lock()
                .ok()
                .and_then(|slot| slot.clone())
                .unwrap_or_default();
            set_child_text(hwnd, ID_IMAGE_ADMIN_DETAIL, "");
            set_child_text(
                hwnd,
                ID_IMAGE_ADMIN_HINT,
                &if let Some(error) = error {
                    if english {
                        format!("Cannot read the image list: {error}")
                    } else {
                        format!("无法读取镜像列表：{error}")
                    }
                } else if english {
                    format!("No WIM images in {}", dir.display())
                } else {
                    format!("{} 下没有 WIM 镜像。", dir.display())
                },
            );
        } else {
            let dir = IMAGE_ADMIN_DIR
                .lock()
                .ok()
                .and_then(|slot| slot.clone())
                .unwrap_or_default();
            set_child_text(
                hwnd,
                ID_IMAGE_ADMIN_HINT,
                &if english {
                    format!("Managing {} (select an image to see its indexes)", dir.display())
                } else {
                    format!("正在管理 {}（选中镜像查看索引）", dir.display())
                },
            );
            SendMessageW(combo, CB_SETCURSEL, 0, 0);
            image_admin_show_detail(hwnd, &images);
        }
        if let Ok(mut slot) = IMAGE_ADMIN_LIST_RESULT.lock() {
            *slot = Some(images);
        }
        return 0;
    }
    if message == WM_APP_IMAGE_DELETE_DONE {
        IMAGE_ADMIN_BUSY.finish();
        let result = IMAGE_ADMIN_DELETE_RESULT
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        match result {
            Some(Ok((removed, kept, path))) => {
                show_message(
                    hwnd,
                    &if english {
                        format!("Deleted {removed} old index(es); {kept} kept:\n{path}")
                    } else {
                        format!("已删除 {removed} 个旧索引，保留最新 {kept} 个：\n{path}")
                    },
                    if english { "Deleted" } else { "删除完成" },
                    MB_OK | MB_ICONINFORMATION,
                );
                spawn_image_list_worker(hwnd);
            }
            Some(Err(error)) => {
                show_message(
                    hwnd,
                    &if english {
                        format!("Delete failed: {error}")
                    } else {
                        format!("删除失败：{error}")
                    },
                    if english { "Delete failed" } else { "删除失败" },
                    MB_OK | MB_ICONERROR,
                );
            }
            None => {}
        }
        return 0;
    }
    if message == WM_COMMAND {
        let control_id = w_param & 0xffff;
        audit_control_action(control_id, (w_param >> 16) & 0xffff);
        match control_id {
            ID_IMAGE_ADMIN_CLOSE => {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
                return 0;
            }
            ID_IMAGE_ADMIN_COMBO => {
                if let Some(images) = IMAGE_ADMIN_LIST_RESULT
                    .lock()
                    .ok()
                    .and_then(|slot| slot.clone())
                {
                    image_admin_show_detail(hwnd, &images);
                }
                return 0;
            }
            ID_IMAGE_ADMIN_REFRESH => {
                spawn_image_list_worker(hwnd);
                return 0;
            }
            ID_IMAGE_ADMIN_OPEN_FOLDER => {
                image_admin_open_folder(hwnd);
                return 0;
            }
            ID_IMAGE_ADMIN_DELETE => {
                image_admin_delete_flow(hwnd);
                return 0;
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, message, w_param, l_param)
}
