// 「系统分区处理方式选择」与「无效镜像路径」两个模态框。
//
// 2026-10-26 从 `native_gui.rs` 整块搬出（拆分第二步第 4 块：评估文档里五个
// 自包含对话框的最后两个）。子模块而不是顶层模块：两个框都用父模块那一整套
// 私有 Win32 类型/常量/辅助函数（`wide`/`Hwnd`/`WParam` 等），做成顶层就得把
// 它们全提成 `pub(crate)`。
//
// 约定（与本目录另三个子模块一致）：**控件 ID 常量留在父模块**，因为
// `CONTROL_NAMES` 审计表与主窗口 `WM_COMMAND` 分发要用，搬走会变成两处副本；
// 本模块只 `use super::*` 取用，对外只暴露两个 `ask_*` 入口与一个
// `CHOICE_GENERIC_OFFLINE` 标志（调用方在弹框前置位，决定正文用哪套文案）。
//
// 两个框都是自绘模态循环（不依赖 comctl32 v6 TaskDialog）：
//   系统分区处理方式 → 返回 1=进入 PE、2=进入 Windows RE、0=取消；
//   无效镜像路径     → 返回 1=选择位置、0=取消。
// 两者都**故意不处理 WM_DESTROY**：`PostQuitMessage` 是线程级的，会给共享同一
// 消息队列的主窗口循环也投一个 `WM_QUIT`，历史上抓到过主窗口跟着退出
// （2026-09-30）。模态循环改为按 `IsWindow(dialog)` 判断退出。

use super::*;

// ===================== 系统分区处理方式选择（3 按钮模态） =====================
// 智能分流：备份/还原目标为「当前活动系统」时，不能在线执行，弹窗让用户选
// 进入 PE / 进入 Windows RE / 取消。自绘模态对话框（不依赖 comctl32 v6
// TaskDialog），返回 1=进入 PE、2=进入 Windows RE、0=取消。
/// 系统选择对话框的日志位置：与 GUI 主日志同目录，故障时一起看。
/// 2026-10-18 起统一走 `log_root::logs_dir()`——这里曾自己拼
/// `<exe>\logs`，与 `append_gui_log` 各写各的，路径说变就变。
fn system_choice_log_path() -> std::path::PathBuf {
    crate::log_root::logs_dir().join("gui.log")
}
/// 对话框文案语言：0=中文 1=English（模态期间单实例，用静态即可）。
static CHOICE_LANGUAGE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
// PE/RE 选择弹窗的正文模式：false=目标为当前系统盘（默认，历史文案）；
// true=用户在 B（还原数据到非 C 盘）显式选择离线通道——目标是数据卷，不涉及系统盘，
// 文案改为通用离线说明。调用方在 ask_system_drive_handler 前置位，弹窗创建时消费。
pub(super) static CHOICE_GENERIC_OFFLINE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" fn window_proc_system_choice(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    if message == WM_CREATE {
        let english = CHOICE_LANGUAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
        // B 入口离线通道用通用文案（目标是数据卷，不是系统盘）；消费后立即复位，
        // 下一次（如备份/还原系统盘）回到默认系统盘文案。
        let generic_offline = CHOICE_GENERIC_OFFLINE.swap(false, std::sync::atomic::Ordering::SeqCst);
        // ★ 2.6.87：删掉「用系统字体覆盖」的那两行（原来在这里取
        //   `GetStockObject(DEFAULT_GUI_FONT)` 再对每个控件 WM_SETFONT）。
        //   `create_control` 内部已经用 `gui_font()`（雅黑 + 用户选定字号）设过字体，
        //   这里再用系统字体覆盖回去，导致**这个弹窗的字号根本不跟随设置**、
        //   字体族也跟其它窗口不一致（中文看着像宋体、笔画发虚）。
        //   2026-10-30 截图确认：18 号设置下这个框仍是老样子。
        let body_text = if english {
            if generic_offline {
                "Offline channel selected: the target volume will be formatted and receive the image after rebooting into PE or Windows RE. Choose how to proceed:"
            } else {
                "The target volume is the currently running system. Backing up or restoring the system partition must run offline in PE or Windows RE. Choose how to proceed:"
            }
        } else if generic_offline {
            "已选择离线通道：重启进入 PE 或 Windows RE 后，目标卷将被格式化并写入镜像。请选择处理方式："
        } else {
            "目标分区是当前正在运行的系统。系统分区的备份与还原必须在 PE 或 Windows RE 中离线执行。请选择处理方式："
        };
        // 正文高度按当前字号实测：换字体后文案会折成更多行，写死 78 会把
        // 最后一行切掉（英文 18 号要 5 行 = 105 基准像素）。
        const MARGIN: i32 = 30;
        const CONTENT_W: i32 = 400;
        const BTN_H: i32 = 42;
        const BTN_GAP: i32 = 8;
        // +8：`DrawText` 的 DT_CALCRECT 高度与真实排版可能差 1~2 像素，
        // 正好贴住时最后一行的下缘（尤其降部，如「返」「y」）会被切。
        // 判据实测：不加时中文 18 号 slack=0、中文 14 号 slack=1。
        let body_height = measure_text_height(body_text, CONTENT_W) + 8;
        create_control(
            hwnd,
            "STATIC",
            body_text,
            0,
            MARGIN,
            24,
            CONTENT_W,
            body_height,
            ID_CHOICE_BODY,
        );
        let mut y = 24 + body_height + 16;
        create_control(
            hwnd,
            "BUTTON",
            if english {
                "Enter PE (recommended)"
            } else {
                "进入 PE（推荐）"
            },
            WS_TABSTOP | BS_DEFPUSHBUTTON,
            MARGIN,
            y,
            CONTENT_W,
            BTN_H,
            ID_CHOICE_PE,
        );
        y += BTN_H + BTN_GAP;
        create_control(
            hwnd,
            "BUTTON",
            if english {
                "Enter Windows RE"
            } else {
                "进入 Windows RE"
            },
            WS_TABSTOP,
            MARGIN,
            y,
            CONTENT_W,
            BTN_H,
            ID_CHOICE_RE,
        );
        y += BTN_H + BTN_GAP;
        create_control(
            hwnd,
            "BUTTON",
            if english { "Cancel" } else { "取消" },
            WS_TABSTOP,
            MARGIN,
            y,
            CONTENT_W,
            BTN_H,
            ID_CHOICE_CANCEL,
        );
        // 整窗尺寸按内容反推并居中。
        // ★ 原来窗口写死 460×320 且居中算法写成 `(屏幕宽 − 460)/2`——屏幕宽是
        //   物理像素、460 是基准像素，200% 下窗口实际 920 宽，于是整体偏右 230。
        let client_w = MARGIN * 2 + CONTENT_W;
        let client_h = y + BTN_H + 24;
        let mut frame = Rect {
            left: 0,
            top: 0,
            right: crate::dpi::s(client_w),
            bottom: crate::dpi::s(client_h),
        };
        AdjustWindowRectEx(&mut frame, WS_POPUP | WS_CAPTION | WS_SYSMENU, 0, 0);
        let window_w = frame.right - frame.left;
        let window_h = frame.bottom - frame.top;
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        MoveWindow(
            hwnd,
            ((screen_w - window_w) / 2).max(0),
            ((screen_h - window_h) / 2).max(0),
            window_w,
            window_h,
            1,
        );
        return 0;
    }
    if message == WM_KEYDOWN && w_param as u32 == VK_ESCAPE {
        // Esc = 取消（与点「取消」按钮相同）。
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut i32;
        if !state_ptr.is_null() {
            *state_ptr = 0;
            // 走 WM_CLOSE 异步销毁，理由同 WM_COMMAND 分支。
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
            return 0;
        }
    }
    if message == WM_CLOSE {
        // 真正的销毁点：这时候已经不在按钮的同步 SendMessage 链里了。
        DestroyWindow(hwnd);
        return 0;
    }
    if message == WM_COMMAND {
        let control_id = w_param & 0xffff;
            // GUI 操作审计：与 main 主窗口同一入口策略——先留痕再走业务分支。
            audit_control_action(control_id, (w_param >> 16) & 0xffff);
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut i32;
        if !state_ptr.is_null() {
            let result = match control_id {
                ID_CHOICE_PE => 1,
                ID_CHOICE_RE => 2,
                ID_CHOICE_CANCEL => 0,
                _ => -1,
            };
            if result >= 0 {
                // 记下用户到底点了哪个按钮：2026-09-30 用户反馈「进入 Windows RE
                // 点了没反应」，而日志里连一行都没有，无法判断是点击没送达还是
                // 后续步骤静默失败。这一行就是为了让下次一眼看出区别。
                // 走 `append_log` 而不是自己拼 OpenOptions：落点与 `gui.log`
                // 完全一致（该函数返回的就是它），还额外获得 PE/RE 里的持久化
                // 镜像（`session_log`）。此前这一行是手写的，换个地方就找不到。
                let _ = super::append_log(
                    &system_choice_log_path(),
                    &format!("system drive choice: button clicked, choice={result}"),
                );
                *state_ptr = result;
                // **不能在这里直接 DestroyWindow**：WM_COMMAND 是在按钮处理
                // BM_CLICK 时同步发过来的，销毁父窗口会连带销毁正在执行窗口过程的
                // 按钮自身，跨进程 SendMessage(BM_CLICK) 因此永久阻塞——
                // 2026-09-30 v1.8.13 实机卡死（提权 PowerShell 窗口挂住不返回）。
                // 改成投递 WM_CLOSE，等按钮的消息处理链退栈后再销毁。
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
                return 0;
            }
        }
    }
    // 这里**故意不处理 WM_DESTROY**：PostQuitMessage 是线程级的，会给共享同一个
    // 消息队列的主窗口循环也投一个 WM_QUIT。模态循环通常先把它取走，但那是竞态——
    // 2026-09-30 实机抓到过一次主窗口跟着退出（用户点完对话框整个程序消失）。
    // 模态循环改为按 IsWindow(dialog) 判断退出，不再依赖 WM_QUIT。
    // 同类先例见 show_system_info_window 里的注释。
    DefWindowProcW(hwnd, message, w_param, l_param)
}

/// 模态询问「进入 PE / 进入 RE / 取消」（3 按钮）。返回 1=PE 2=RE 0=取消。
pub(super) unsafe fn ask_system_drive_handler(hwnd: Hwnd, language: Language) -> i32 {
    let instance = GetModuleHandleW(null());
    let class_name = wide("BackupRestoreSystemDriveChoice");
    let class = WndClassExW {
        cb_size: size_of::<WndClassExW>() as u32,
        style: 0,
        wnd_proc: Some(window_proc_system_choice),
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
        // ERROR_CLASS_ALREADY_EXISTS=1410：类已注册，继续使用
        return 0;
    }
    CHOICE_LANGUAGE.store(
        if language == Language::English { 1 } else { 0 },
        std::sync::atomic::Ordering::SeqCst,
    );
    let caption = wide(if language == Language::English {
        "Recovery environment required"
    } else {
        "需要进入恢复环境处理"
    });
    // 屏幕居中（宽 460 高 320）：正文+3 按钮需要客户区约 254 高，
    // 总高 268 时客户区（≈238，扣标题栏）会把取消按钮裁掉，必须 ≥320。
    let screen_w = GetSystemMetrics(SM_CXSCREEN).max(640);
    let screen_h = GetSystemMetrics(SM_CYSCREEN).max(480);
    let dialog = CreateWindowExW(
        0,
        class_name.as_ptr(),
        caption.as_ptr(),
        WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
        // 这两个值只是占位：真正的尺寸与居中在 `WM_CREATE` 末尾按内容算完再
        // MoveWindow。这里必须也用 `dpi::s` 换算，否则 200% 下会先闪一下偏右的窗口。
        (screen_w - crate::dpi::s(460)) / 2,
        (screen_h - crate::dpi::s(320)) / 2,
        crate::dpi::s(460),
        crate::dpi::s(320),
        hwnd,
        null_mut(),
        instance,
        null_mut(),
    );
    if dialog.is_null() {
        return 0;
    }
    // 结果盒的裸指针留在**局部变量**里：循环退出后直接解引用读取。
    // 绝不能像早先那样事后再 GetWindowLongPtrW(dialog, …)——那时窗口已被
    // DestroyWindow 销毁，读它是未定义行为，实际稳定返回 0，
    // 于是用户的选择被吞掉（2026-09-30 实机：choice=2 却返回 0）。
    let result_box = Box::into_raw(Box::new(0i32));
    SetWindowLongPtrW(dialog, GWLP_USERDATA, result_box as isize);
    // 模态：禁用主窗口，进入对话框消息循环
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
    // 退出条件用 IsWindow(dialog)：WM_COMMAND / Esc 分支里的 DestroyWindow 是同步的，
    // 处理完消息回到这里窗口已不存在，循环必然结束；不依赖 WM_DESTROY 里的
    // PostQuitMessage（那会连带杀死主窗口的消息循环，见 window_proc_system_choice）。
    while IsWindow(dialog) != 0 && GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
        // 对话框式键盘导航（Tab 遍历 / 回车默认按钮 / Esc / 方向键切换单选）。
        if IsDialogMessageW(dialog, &message) == 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    // 窗口已销毁，但盒子还在：用局部裸指针读，不经过窗口句柄。
    let result = unsafe { *result_box };
    drop(unsafe { Box::from_raw(result_box) });
    // 恢复主窗口
    EnableWindow(hwnd, 1);
    SetForegroundWindow(hwnd);
    result
}

static INVALID_PATH_LANGUAGE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// 「镜像路径」提示框的触发原因：true=用户根本没输入（空串），false=输入了但不合法。
static INVALID_PATH_EMPTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" fn window_proc_invalid_path_choice(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    if message == WM_CREATE {
        let english = INVALID_PATH_LANGUAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
        let empty = INVALID_PATH_EMPTY.load(std::sync::atomic::Ordering::SeqCst);
        // ★ 2.6.87：同上，删掉用系统字体覆盖的两行——`create_control` 已经用
        //   `gui_font()` 设过字体，再覆盖回 `DEFAULT_GUI_FONT` 会让这个弹窗
        //   不跟随用户的「界面字号」，字体族也与其它窗口不一致。
        let body_text = match (english, empty) {
            (true, true) => "The image absolute path has not been entered yet.\n\nClick [Choose image location] to select a file, or [Cancel] to return.",
            (true, false) => "The image absolute path is invalid.\n\nClick [Choose image location] to select a file, or [Cancel] to return.",
            (false, true) => "尚未输入镜像绝对路径。\n\n请点击【选择镜像位置】选择文件，或点击【取消】返回。",
            (false, false) => "镜像绝对路径无效。\n\n请点击【选择镜像位置】选择文件，或点击【取消】返回。",
        };
        const MARGIN: i32 = 30;
        const CONTENT_W: i32 = 400;
        const BTN_H: i32 = 42;
        const BTN_GAP: i32 = 10;
        // +8：`DrawText` 的 DT_CALCRECT 高度与真实排版可能差 1~2 像素，
        // 正好贴住时最后一行的下缘（尤其降部，如「返」「y」）会被切。
        // 判据实测：不加时中文 18 号 slack=0、中文 14 号 slack=1。
        let body_height = measure_text_height(body_text, CONTENT_W) + 8;
        create_control(
            hwnd,
            "STATIC",
            body_text,
            0,
            MARGIN,
            24,
            CONTENT_W,
            body_height,
            0,
        );
        let mut y = 24 + body_height + 16;
        create_control(
            hwnd,
            "BUTTON",
            if english {
                "Choose image location"
            } else {
                "选择镜像位置"
            },
            WS_TABSTOP | BS_DEFPUSHBUTTON,
            MARGIN,
            y,
            CONTENT_W,
            BTN_H,
            ID_CHOICE_CHOOSE_LOCATION,
        );
        y += BTN_H + BTN_GAP;
        create_control(
            hwnd,
            "BUTTON",
            if english { "Cancel" } else { "取消" },
            WS_TABSTOP,
            MARGIN,
            y,
            CONTENT_W,
            BTN_H,
            ID_CHOICE_PATH_CANCEL,
        );
        // 整窗尺寸按内容反推并居中（原来写死 460×220，且居中混用了物理与基准像素）。
        let client_w = MARGIN * 2 + CONTENT_W;
        let client_h = y + BTN_H + 24;
        let mut frame = Rect {
            left: 0,
            top: 0,
            right: crate::dpi::s(client_w),
            bottom: crate::dpi::s(client_h),
        };
        AdjustWindowRectEx(&mut frame, WS_POPUP | WS_CAPTION | WS_SYSMENU, 0, 0);
        let window_w = frame.right - frame.left;
        let window_h = frame.bottom - frame.top;
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        MoveWindow(
            hwnd,
            ((screen_w - window_w) / 2).max(0),
            ((screen_h - window_h) / 2).max(0),
            window_w,
            window_h,
            1,
        );
        return 0;
    }
    if message == WM_KEYDOWN && w_param as u32 == VK_ESCAPE {
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut i32;
        if !state_ptr.is_null() {
            *state_ptr = 0;
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
            return 0;
        }
    }
    if message == WM_CLOSE {
        DestroyWindow(hwnd);
        return 0;
    }
    if message == WM_COMMAND {
        let control_id = w_param & 0xffff;
            // GUI 操作审计：与 main 主窗口同一入口策略——先留痕再走业务分支。
            audit_control_action(control_id, (w_param >> 16) & 0xffff);
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut i32;
        if !state_ptr.is_null() {
            let result = match control_id {
                ID_CHOICE_CHOOSE_LOCATION => 1,
                ID_CHOICE_PATH_CANCEL => 0,
                _ => -1,
            };
            if result >= 0 {
                *state_ptr = result;
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
                return 0;
            }
        }
    }
    DefWindowProcW(hwnd, message, w_param, l_param)
}

/// 镜像路径为空或不合法时弹出的模态对话框（不用错误图标报错，改为引导式提示），
/// 提供【选择镜像位置】与【取消】两个按钮。返回 1=选择镜像位置，0=取消。
/// `empty` = 用户根本没输入路径（文案「没有输入镜像路径」）；否则为「镜像路径无效」。
pub(super) unsafe fn ask_invalid_image_path(hwnd: Hwnd, language: Language, empty: bool) -> i32 {
    let instance = GetModuleHandleW(null());
    let class_name = wide("BackupRestoreInvalidPathChoice");
    let class = WndClassExW {
        cb_size: size_of::<WndClassExW>() as u32,
        style: 0,
        wnd_proc: Some(window_proc_invalid_path_choice),
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
        return 0;
    }
    INVALID_PATH_LANGUAGE.store(
        if language == Language::English { 1 } else { 0 },
        std::sync::atomic::Ordering::SeqCst,
    );
    INVALID_PATH_EMPTY.store(empty, std::sync::atomic::Ordering::SeqCst);
    let caption = wide(match (language, empty) {
        (Language::English, true) => "No image path entered",
        (Language::English, false) => "Invalid image path",
        (Language::Chinese, true) => "没有输入镜像路径",
        (Language::Chinese, false) => "镜像路径无效",
    });
    let screen_w = GetSystemMetrics(SM_CXSCREEN).max(640);
    let screen_h = GetSystemMetrics(SM_CYSCREEN).max(480);
    let dialog = CreateWindowExW(
        0,
        class_name.as_ptr(),
        caption.as_ptr(),
        WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
        (screen_w - crate::dpi::s(460)) / 2,
        (screen_h - crate::dpi::s(260)) / 2,
        crate::dpi::s(460),
        crate::dpi::s(260),
        hwnd,
        null_mut(),
        instance,
        null_mut(),
    );
    if dialog.is_null() {
        return 0;
    }
    let result_box = Box::into_raw(Box::new(0i32));
    SetWindowLongPtrW(dialog, GWLP_USERDATA, result_box as isize);
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
    let result = unsafe { *result_box };
    drop(unsafe { Box::from_raw(result_box) });
    EnableWindow(hwnd, 1);
    SetForegroundWindow(hwnd);
    result
}
