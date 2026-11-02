//! 「任务与证据」窗口（2026-11-02 从 `native_gui.rs` 拆出）。
//!
//! 独立模态窗口：列出 `<运行目录>\tasks` 下全部任务，可看详情、清工作树、开目录。
//! **解析 / 排序 / 文案全部在 `crate::task_admin`**（无条件编译、本机可单测）；
//! 这里只负责控件与消息。不要把解析写回本文件——`cfg(windows)` 里的逻辑在 mac 上
//! 是零信息（编译器不打开），本项目的 GUI 缺陷多半就是这么漏掉的。

use super::*;

const TASK_ADMIN_CLASS: &str = "BackupRestoreTaskAdmin";
const ID_TASK_LABEL: usize = 1400;
const ID_TASK_COMBO: usize = 1401;
const ID_TASK_DETAIL: usize = 1402;
const ID_TASK_REFRESH: usize = 1403;
const ID_TASK_CLEANUP: usize = 1404;
const ID_TASK_OPEN_DIR: usize = 1405;
const ID_TASK_CLOSE: usize = 1406;
const ID_TASK_HINT: usize = 1407;

/// 下拉框的一项：正常记录或坏记录。
/// ★ **槽位顺序必须与下拉框项一一对应**——错位会造成「选中第 3 项却清理第 2 个任务」，
/// 而这种错位在界面上完全看不出来。
enum TaskSlot {
    Row(crate::task_admin::TaskRow),
    Bad {
        dir: String,
        defect: crate::task_admin::RowDefect,
    },
}

struct TaskAdminModel {
    slots: Vec<TaskSlot>,
    language: Language,
    root: PathBuf,
}

static TASK_ADMIN_MODEL: std::sync::Mutex<Option<TaskAdminModel>> = std::sync::Mutex::new(None);

fn lock_task_admin() -> std::sync::MutexGuard<'static, Option<TaskAdminModel>> {
    // 窗口过程里 panic 一次就会把锁 poison；此时用 into_inner 继续，
    // 总比让整个任务窗口失效好（数据本身没被破坏）。
    TASK_ADMIN_MODEL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 枚举任务目录并解析。坏记录**照样返回**：用户要能看见它、照路径去人工处理，
/// 而不是整块列表因为一个坏目录打不开。
fn load_task_slots(root: &Path) -> Vec<TaskSlot> {
    let mut rows = Vec::new();
    let mut broken = Vec::new();
    if let Ok(entries) = fs::read_dir(root.join("tasks")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir = path.to_string_lossy().into_owned();
            let task_json = fs::read_to_string(path.join("task.json")).ok();
            let status_json = fs::read_to_string(path.join("status.json")).ok();
            match (task_json, status_json) {
                (Some(task), Some(status)) => {
                    match crate::task_admin::parse_row(&task, &status, &dir) {
                        Ok(row) => rows.push(row),
                        Err(defect) => broken.push(TaskSlot::Bad { dir, defect }),
                    }
                }
                // 缺文件也要说清缺哪一个：task.json 与 status.json 的修法不同。
                (None, _) => broken.push(TaskSlot::Bad {
                    dir,
                    defect: crate::task_admin::RowDefect::TaskMalformed("读不出 task.json".into()),
                }),
                (_, None) => broken.push(TaskSlot::Bad {
                    dir,
                    defect: crate::task_admin::RowDefect::StatusMalformed(
                        "读不出 status.json".into(),
                    ),
                }),
            }
        }
    }
    crate::task_admin::sort_rows(&mut rows);
    let mut slots: Vec<TaskSlot> = rows.into_iter().map(TaskSlot::Row).collect();
    // 坏记录排在正常记录之后：它们是「需要人工处理」的少数派，不该置顶。
    slots.extend(broken);
    slots
}

unsafe fn task_admin_control(hwnd: Hwnd, id: usize) -> Option<Hwnd> {
    let handle = GetDlgItem(hwnd, id as i32);
    if handle.is_null() {
        None
    } else {
        Some(handle)
    }
}

/// 按当前模型重填下拉框、标签与详情。
///
/// ★ 先算好全部文字并**释放锁**，再碰控件：持锁调 `SendMessageW` 会在控件
/// 回调里重入同一把锁时死锁（`CBN_SELCHANGE` 就是被 `CB_SETCURSEL` 触发的）。
unsafe fn task_admin_reload(hwnd: Hwnd) {
    let (labels, count, language) = {
        let guard = lock_task_admin();
        let Some(model) = guard.as_ref() else {
            return;
        };
        let labels = model
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| match slot {
                TaskSlot::Row(row) => crate::task_admin::combo_label(index, row, model.language),
                TaskSlot::Bad { dir, .. } => {
                    crate::task_admin::defect_combo_label(index, dir, model.language)
                }
            })
            .collect::<Vec<_>>();
        (labels, model.slots.len(), model.language)
    };

    if let Some(combo) = task_admin_control(hwnd, ID_TASK_COMBO) {
        SendMessageW(combo, CB_RESETCONTENT, 0, 0);
        for label in &labels {
            let text = wide(label);
            SendMessageW(combo, CB_ADDSTRING, 0, text.as_ptr() as isize);
        }
        if count > 0 {
            SendMessageW(combo, CB_SETCURSEL, 0, 0);
        }
    }
    if let Some(label) = task_admin_control(hwnd, ID_TASK_LABEL) {
        // ★ 2.6.68 修缺陷：零任务态原来写「任务：0 个（尚无任务记录）」，几何判据实测
        //   文字宽 **156 px** 而标签框只有 **80 px** → 被裁掉一半。这个状态只在
        //   「全新安装 / 清空任务」时出现，平时跑判据永远测不到（当时 tasks 里有 50 条）。
        //   改法：标签只报数字（最宽「任务：50 个」= 66 px，稳），说明挪到 696 px 的提示条。
        let text = match (language, count) {
            (Language::Chinese, total) => format!("任务：{total} 个"),
            (Language::English, total) => format!("Tasks: {total}"),
        };
        set_text(label, &text);
        // ★ 2.7.3：标签宽度按**实测文本**算。
        //   原来写死 80 基准像素：14 号字下「任务：50 个」= 66 px 装得下，但 18 号字下
        //   「任务：4 个」实测 88 px、控件只有 80 → 右边被切掉 8 px。
        //   2026-10-31 由 GUI 判据在「任务与证据」窗口抓到，是四个辅助窗口
        //   **首次纳入矩阵**后报出的第一个真缺陷（此前只量主窗口，永远看不见它）。
        //   做法：标签按实测加宽，下拉框跟着右移、**右边界保持 708 不变**
        //   （详情框/提示条都是 x=12 宽 696 → 右边界 708），窗口尺寸不变。
        let label_w = fit_width(&text, 80);
        let combo_x = 12 + label_w + 4;
        let combo_w = (708 - combo_x).max(320);
        task_admin_place(label, 12, 14, label_w, 20);
        if let Some(combo) = task_admin_control(hwnd, ID_TASK_COMBO) {
            task_admin_place(combo, combo_x, 10, combo_w, 240);
        }
        if count == 0 {
            task_admin_set_hint(
                hwnd,
                match language {
                    Language::Chinese => "尚无任务记录：先在主窗口创建任务，再回到这里查看。",
                    Language::English => {
                        "No tasks yet: create one in the main window, then come back."
                    }
                },
            );
        }
    }
    task_admin_show_detail(hwnd);
}

/// 把当前选中项画进详情框，并按「能否清理」调整按钮可用状态。
unsafe fn task_admin_show_detail(hwnd: Hwnd) {
    let index = match task_admin_control(hwnd, ID_TASK_COMBO) {
        Some(combo) => {
            let selection = SendMessageW(combo, CB_GETCURSEL, 0, 0);
            if selection >= 0 {
                selection as usize
            } else {
                usize::MAX
            }
        }
        None => usize::MAX,
    };
    let (text, allow_cleanup, dir) = {
        let guard = lock_task_admin();
        let Some(model) = guard.as_ref() else {
            return;
        };
        match model.slots.get(index) {
            Some(TaskSlot::Row(row)) => (
                crate::task_admin::detail_lines(row, model.language).join("\r\n"),
                // 未完成任务禁止清理：core 会返 BR-CORE-059，界面先拦一道，
                // 让用户看到「为什么按钮是灰的」而不是点下去才被拒。
                !row.is_incomplete(),
                row.dir.clone(),
            ),
            Some(TaskSlot::Bad { dir, defect }) => (
                crate::task_admin::defect_lines(defect, dir, model.language).join("\r\n"),
                false,
                dir.clone(),
            ),
            None => (
                match model.language {
                    Language::Chinese => "（没有可显示的任务）".to_string(),
                    Language::English => "(no task to show)".to_string(),
                },
                false,
                String::new(),
            ),
        }
    };
    if let Some(detail) = task_admin_control(hwnd, ID_TASK_DETAIL) {
        set_text(detail, &text);
    }
    if let Some(cleanup) = task_admin_control(hwnd, ID_TASK_CLEANUP) {
        EnableWindow(cleanup, i32::from(allow_cleanup));
    }
    if let Some(open) = task_admin_control(hwnd, ID_TASK_OPEN_DIR) {
        EnableWindow(open, i32::from(!dir.is_empty()));
    }
}

unsafe fn task_admin_set_hint(hwnd: Hwnd, text: &str) {
    if let Some(hint) = task_admin_control(hwnd, ID_TASK_HINT) {
        set_text(hint, text);
    }
}

/// 清理当前任务的工作树。
///
/// ★ 只调 `cleanup_task_artifacts`：它按白名单只删 `mount/stage/original/payload`，
///   **保留** task/status/manifest/log/BCD 备份。证据是产品承诺，不在这里删。
unsafe fn task_admin_cleanup(hwnd: Hwnd) {
    let selection = match task_admin_control(hwnd, ID_TASK_COMBO) {
        Some(combo) => SendMessageW(combo, CB_GETCURSEL, 0, 0),
        None => return,
    };
    if selection < 0 {
        return;
    }
    let (task_id, language, root) = {
        let guard = lock_task_admin();
        let Some(model) = guard.as_ref() else {
            return;
        };
        match model.slots.get(selection as usize) {
            Some(TaskSlot::Row(row)) if !row.is_incomplete() => {
                (row.id.clone(), model.language, model.root.clone())
            }
            _ => return,
        }
    };
    let (prompt, caption) = match language {
        Language::Chinese => (
            format!(
                "将删除任务的工作树目录（mount / stage / original / payload）。\n\n\
                 任务 {task_id}\n\n\
                 任务记录、状态、清单、日志与 BCD 备份都会保留，供事后诊断。\n\n\
                 继续？"
            ),
            "清理任务工作树",
        ),
        Language::English => (
            format!(
                "Delete the working tree of task {task_id}\n\
                 (mount / stage / original / payload).\n\n\
                 Task, status, manifest, logs and BCD backups are kept. Continue?"
            ),
            "Clean task working tree",
        ),
    };
    if show_message(hwnd, &prompt, caption, MB_YESNO | MB_ICONWARNING) != IDYES {
        return;
    }
    let store = backuprestore_core::TaskStore::new(&root);
    let hint = match store.cleanup_task_artifacts(&task_id) {
        Ok(true) => match language {
            Language::Chinese => "已清理该任务的工作树；任务记录与日志保留。".to_string(),
            Language::English => "Working tree removed; records and logs kept.".to_string(),
        },
        // `false` = 本来就没有工作树（已清理过，或该操作不产生工作树）。
        // 这不是失败，但也不能说成「清理成功」——两者在用户眼里是同一种结果、不同的原因。
        Ok(false) => match language {
            Language::Chinese => "该任务没有残留工作树，无需清理。".to_string(),
            Language::English => "No working tree to remove for this task.".to_string(),
        },
        // 失败一律带稳定错误码：`Io`/`Json` 两类文案本身没有 [BR-…] 前缀。
        Err(error) => crate::failure_receipt::with_code(&error.to_string(), error.code()),
    };
    task_admin_set_hint(hwnd, &hint);
    task_admin_reload(hwnd);
}

unsafe fn task_admin_open_dir(hwnd: Hwnd) {
    let selection = match task_admin_control(hwnd, ID_TASK_COMBO) {
        Some(combo) => SendMessageW(combo, CB_GETCURSEL, 0, 0),
        None => return,
    };
    if selection < 0 {
        return;
    }
    let dir = {
        let guard = lock_task_admin();
        let Some(model) = guard.as_ref() else {
            return;
        };
        match model.slots.get(selection as usize) {
            Some(TaskSlot::Row(row)) => row.dir.clone(),
            Some(TaskSlot::Bad { dir, .. }) => dir.clone(),
            None => return,
        }
    };
    if dir.is_empty() {
        return;
    }
    let verb = wide("open");
    let file = wide(&dir);
    ShellExecuteW(
        hwnd,
        verb.as_ptr(),
        file.as_ptr(),
        null(),
        null(),
        SW_SHOWNORMAL,
    );
}

/// 固定尺寸布局（不设 `WS_THICKFRAME`）：窗口不可缩放，布局一次算清，
/// 省掉「缩放后控件重叠」这类只能靠截图才发现的问题。
/// 把控件挪到「基准像素」位置并设定大小。
///
/// 与 `create_control` 共用同一套缩放（内部乘 DPI 系数）：布局代码一律写基准像素，
/// 遇到 125%/200% 才不会出现「一处乘了系数、一处没乘」的错位。
unsafe fn task_admin_place(hwnd: Hwnd, x: i32, y: i32, width: i32, height: i32) {
    MoveWindow(
        hwnd,
        crate::dpi::s(x),
        crate::dpi::s(y),
        crate::dpi::s(width),
        crate::dpi::s(height),
        1,
    );
}

unsafe fn task_admin_create_controls(hwnd: Hwnd, language: Language) {
    let chinese = language == Language::Chinese;
    create_control(hwnd, "STATIC", "", 0, 12, 14, 80, 20, ID_TASK_LABEL);
    // 下拉高度 = 展开后的列表高度（不是折叠后的高度）。
    create_control(
        hwnd,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST | WS_VSCROLL | WS_TABSTOP,
        96,
        10,
        612,
        240,
        ID_TASK_COMBO,
    );
    create_control(
        hwnd,
        "EDIT",
        "",
        WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY | WS_VSCROLL | WS_TABSTOP,
        12,
        44,
        696,
        336,
        ID_TASK_DETAIL,
    );
    // ★ 2.7.4：这一排按钮原来写死 100/150/130/66 + 固定 x（238/346/504/642）。
    //   和同窗口那个标签（2.7.3 修）是同一类隐患：宽度不随文本/字号变，全靠「恰好还够」。
    //   18 号中文下「打开任务目录」实测 156 px > 写死的 130 px，之所以没被切，是因为
    //   按钮框比控件框宽一点点——换个更长的词就破。改成：先按实测文本算宽，再从右边界
    //   往左排，右边界仍是 708（与详情框右边缘对齐），窗口尺寸不用变。
    let buttons = [
        (ID_TASK_REFRESH, if chinese { "刷新列表" } else { "Refresh" }, 100),
        (ID_TASK_CLEANUP, if chinese { "清理工作树" } else { "Clean tree" }, 150),
        (ID_TASK_OPEN_DIR, if chinese { "打开任务目录" } else { "Open dir" }, 130),
        (ID_TASK_CLOSE, if chinese { "关闭" } else { "Close" }, 66),
    ];
    const BUTTON_GAP: i32 = 8;
    const BUTTON_RIGHT: i32 = 708;
    let mut widths = [0i32; 4];
    let mut xs = [0i32; 4];
    let mut cursor = BUTTON_RIGHT;
    for index in (0..buttons.len()).rev() {
        let width = fit_width(buttons[index].1, buttons[index].2);
        widths[index] = width;
        cursor -= width;
        xs[index] = cursor;
        cursor -= BUTTON_GAP;
    }
    // 按原顺序创建：WS_TABSTOP 的键盘遍历顺序 = 创建顺序，倒着建会把 Tab 顺序反掉。
    for index in 0..buttons.len() {
        create_control(
            hwnd,
            "BUTTON",
            buttons[index].1,
            WS_TABSTOP,
            xs[index],
            392,
            widths[index],
            28,
            buttons[index].0,
        );
    }
    create_control(hwnd, "STATIC", "", 0, 12, 428, 696, 20, ID_TASK_HINT);
    // 统一字体：WinRE 未必带 Consolas，且回退字体缺中文；stock GUI 字体随镜像本地化。
    let font = gui_font();
    if !font.is_null() {
        for id in [
            ID_TASK_LABEL,
            ID_TASK_COMBO,
            ID_TASK_DETAIL,
            ID_TASK_REFRESH,
            ID_TASK_CLEANUP,
            ID_TASK_OPEN_DIR,
            ID_TASK_CLOSE,
            ID_TASK_HINT,
        ] {
            if let Some(handle) = task_admin_control(hwnd, id) {
                SendMessageW(handle, WM_SETFONT, font as WParam, 1);
            }
        }
    }
    if let Some(detail) = task_admin_control(hwnd, ID_TASK_DETAIL) {
        // 0 = 不限长度（多行 EDIT 的默认上限会截断长路径）。
        SendMessageW(detail, EM_SETLIMITTEXT, 0, 0);
    }
}

unsafe extern "system" fn task_admin_window_proc(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    match message {
        WM_COMMAND => {
            let id = (w_param & 0xffff) as usize;
            let code = ((w_param >> 16) & 0xffff) as usize;
            match id {
                ID_TASK_COMBO if code == CBN_SELCHANGE => task_admin_show_detail(hwnd),
                ID_TASK_REFRESH => {
                    task_admin_set_hint(hwnd, "");
                    task_admin_reload(hwnd);
                }
                ID_TASK_CLEANUP => task_admin_cleanup(hwnd),
                ID_TASK_OPEN_DIR => task_admin_open_dir(hwnd),
                ID_TASK_CLOSE => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        // 刻意不处理 WM_DESTROY：本窗口与主窗口同线程，PostQuitMessage 会把整个
        // 程序的消息循环打掉（「软硬件信息」窗口踩过同一个坑）。
        _ => DefWindowProcW(hwnd, message, w_param, l_param),
    }
}

/// 打开「任务与证据」窗口（模态，父窗口禁用）。
pub(super) unsafe fn show_task_admin(parent: Hwnd, language: Language, root: &Path) {
    let instance = GetModuleHandleW(null());
    if instance.is_null() {
        return;
    }
    let class_name = wide(TASK_ADMIN_CLASS);
    let class = WndClassExW {
        cb_size: size_of::<WndClassExW>() as u32,
        style: 0,
        wnd_proc: Some(task_admin_window_proc),
        cb_cls_extra: 0,
        cb_wnd_extra: 0,
        h_instance: instance,
        h_icon: null_mut(),
        h_cursor: null_mut(),
        h_brush: CreateSolidBrush(PE_BACKGROUND),
        menu_name: null(),
        class_name: class_name.as_ptr(),
        h_icon_sm: null_mut(),
    };
    // 第二次点开会重复注册同一类名，返回 0 不是错误。
    let _ = RegisterClassExW(&class);

    {
        let mut model = lock_task_admin();
        *model = Some(TaskAdminModel {
            slots: load_task_slots(root),
            language,
            root: root.to_path_buf(),
        });
    }

    // ★ 2.6.67 修缺陷：CreateWindowExW 的 width/height 是**整窗**尺寸（含边框与
    //   标题栏），不是客户区——上一版注释把这点写反了，于是客户区实测只有
    //   704×441，而控件是按 720×480 摆的：下拉框/详情框/关闭按钮/提示条右边界
    //   到 708 > 704，被裁掉 4 px；提示条 y 到 448 > 441，纵向也被裁 7 px。
    //   这类缺陷肉眼在小屏上很难分辨（差 4 px），是几何判据抓出来的。
    //   正解：先用 AdjustWindowRectEx 由目标客户区反推整窗尺寸。
    // ★ 2.6.79：目标客户区先按 DPI 缩放，再由 AdjustWindowRectEx 反推整窗尺寸。
    //   边框与标题栏的厚度本身也随 DPI 变，交给 AdjustWindowRectEx 处理即可。
    //   100% 下 s() 是恒等，与改动前完全一致。
    let client_w = crate::dpi::s(720);
    let client_h = crate::dpi::s(480);
    let style = WS_CAPTION | WS_SYSMENU | WS_VISIBLE;
    let mut frame = Rect {
        left: 0,
        top: 0,
        right: client_w,
        bottom: client_h,
    };
    unsafe {
        AdjustWindowRectEx(&mut frame, style, 0, 0);
    }
    let width = frame.right - frame.left;
    let height = frame.bottom - frame.top;
    let screen_width = GetSystemMetrics(SM_CXSCREEN).max(800);
    let screen_height = GetSystemMetrics(SM_CYSCREEN).max(600);
    let title = wide(match language {
        Language::Chinese => "任务与证据 - BackupRestore",
        Language::English => "Tasks & evidence - BackupRestore",
    });
    let window = CreateWindowExW(
        0,
        class_name.as_ptr(),
        title.as_ptr(),
        style,
        ((screen_width - width) / 2).max(0),
        ((screen_height - height) / 2).max(0),
        width,
        height,
        parent,
        null_mut(),
        instance,
        null_mut(),
    );
    if window.is_null() {
        let mut model = lock_task_admin();
        *model = None;
        return;
    }
    task_admin_create_controls(window, language);
    task_admin_reload(window);
    if let Some(combo) = task_admin_control(window, ID_TASK_COMBO) {
        SetFocus(combo);
    }

    // 手写模态循环，与「软硬件信息」窗口同一套：PeekMessageW 轮询 + 每轮查
    // IsWindow；WM_QUIT 属于桌面，必须还回去（否则关这个窗口会连带退出程序）。
    EnableWindow(parent, 0);
    let mut message: Msg = std::mem::zeroed();
    loop {
        if PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) == 0 {
            if IsWindow(window) == 0 {
                break;
            }
            Sleep(20);
            continue;
        }
        if message.message == WM_QUIT {
            PostQuitMessage(0);
            break;
        }
        TranslateMessage(&message);
        DispatchMessageW(&message);
        if IsWindow(window) == 0 {
            break;
        }
    }
    EnableWindow(parent, 1);
    SetForegroundWindow(parent);
    // 清掉模型：任务目录随时在变，下次打开必须重新枚举，不能拿旧快照糊弄。
    {
        let mut model = lock_task_admin();
        *model = None;
    }
}
