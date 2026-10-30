//! 「程序设置」模态对话框（`native_gui` 的子模块）。
//!
//! ## 为什么是子模块，而不是顶层模块
//!
//! 这个对话框要用父模块里**一整套私有 Win32 层**：`type Hwnd/WParam/LParam/LResult`、
//! `extern "system"` 的 FFI 声明、`WS_*`/`WM_*`/`BS_*`/`ES_*` 常量、
//! `wide` / `create_control` / `set_child_text` / `get_text` / `show_message` /
//! `audit_control_action` 这些辅助函数。做成**顶层**模块就得把它们全部提成
//! `pub(crate)`（几十处，私有边界随之失去意义）；做成**子**模块后一句
//! `use super::*` 全部拿到（子模块可见父模块的私有项），**父模块一个字都不用改**。
//!
//! 唯一要暴露给父模块的只有三样：`ask_settings_dialog` 本身，以及两个「本次存了什么」
//! 的原子量（父的 `open_settings` 据此写回 `State` 并落盘）——都用 `pub(super)`。
//!
//! ## 语义边界（搬迁时刻意保持原样）
//!
//! 窗口**只负责把值读出来/写回去**：文案、阈值范围与校验全部来自
//! `crate::failure_policy` 与 `crate::dism_timeout`（单一来源）。别在这里复制第二份
//! 文案或常量——P3 之前踩过「GUI 里抄一份硬编码文案」的坑。
//!
//! 布局尺寸是**固定**的（窗口不可缩放），因此没有主窗口那种 `layout_operation`。

use super::*;

// ===================== 程序设置窗口（「恢复环境失败处理」） =====================
//
// 2026-10-16 用户拍板：把「恢复环境连续失败 N 次后如何处理」做成用户可设置的
// 选项——次数用文本框（默认 3），处理方式二选一：
//   ① 自动清理并重启回正常 Windows（默认，= 原有行为）
//   ② 暂停在恢复环境，由用户按按钮手动执行同一套清理
//
// 设置保存在程序目录的 `BackupRestore.settings.ini`；创建任务时随 prepare
// 参数落盘进 `RecoveryTask.env`，恢复环境按它执行。**判据与文案的单一来源是
// `crate::failure_policy`**：这个窗口只负责把值读出来/写回去，自己不拼文案，
// 免得重演「GUI 里复制第二份硬编码文案」的老问题（P3 前科）。

/// 设置窗口的界面语言（0=中文 1=英文）。窗口过程在 `WM_CREATE` 里读。
static SETTINGS_LANGUAGE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// 设置窗口打开时的初始失败次数。
static SETTINGS_INITIAL_THRESHOLD: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(crate::failure_policy::DEFAULT_THRESHOLD);
/// 设置窗口打开时的初始行为（0=auto-escape 1=pause）。
static SETTINGS_INITIAL_ACTION: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// 设置窗口打开时的初始 DISM 等待上限（秒；0=不限制）。
static SETTINGS_INITIAL_DISM_TIMEOUT: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(crate::dism_timeout::DEFAULT_SECS);
/// 设置窗口打开时的初始 `pause` 最长等待秒数（0=不限制）。
static SETTINGS_INITIAL_PAUSE_WAIT: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(crate::failure_policy::DEFAULT_PAUSE_MAX_WAIT_SECS);
/// 用户点「保存」且校验通过时写入新策略；`None` = 取消或直接关窗。
static SETTINGS_RESULT: std::sync::Mutex<Option<crate::failure_policy::FailurePolicy>> =
    std::sync::Mutex::new(None);
/// 用户点「保存」且校验通过时写入新的 DISM 等待上限（秒）。
pub(super) static SETTINGS_DISM_TIMEOUT_RESULT: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(u32::MAX);
/// 用户点「保存」且校验通过时写入新的 `pause` 最长等待秒数。
pub(super) static SETTINGS_PAUSE_WAIT_RESULT: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(u32::MAX);
/// 设置窗口打开时的初始界面字号（像素，96dpi 基准）。
static SETTINGS_INITIAL_FONT_SIZE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(crate::dpi::DEFAULT_FONT_SIZE);
/// 用户点「保存」时写入的新界面字号；`SETTINGS_RESULT_UNSET` = 本次没保存。
///
/// 与主窗口其它 RESULT 一样用哨兵而不是 `Option`：0 在这里**不是**合法值
/// （字号最小 10），但保持与其它 RESULT 一致的写法更不容易看错。
pub(super) static SETTINGS_FONT_SIZE_RESULT: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(u32::MAX);
/// 两个 `*_RESULT` 的哨兵：`u32::MAX` = 本次没保存（取消/关窗）。
/// 用它而不是 `Option`：这些 static 没有外部所有权，哨兵比加锁更省事，
/// 且 0 是一个**合法**值（= 不限制），不能拿 0 当"未保存"。
pub(super) const SETTINGS_RESULT_UNSET: u32 = u32::MAX;


/// 设置窗口的窗口过程。控件布局是**固定**的（窗口不可缩放），
/// 因此不像主窗口那样需要 `layout_operation`。
unsafe extern "system" fn window_proc_settings(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    use crate::failure_policy::FailureAction;
    if message == WM_CREATE {
        let english = SETTINGS_LANGUAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
        let language = if english {
            Language::English
        } else {
            Language::Chinese
        };
        let ml = message_language(language);
        let auto = FailureAction::AutoEscape;
        let pause = FailureAction::PauseInRecovery;
        // ★ 2.6.83：布局从「每块写死坐标与高度」改成「从上往下按实测高度流水排」。
        //   旧写法的高度（40/56/44）是按 14 号字折两行/三行算的；用户把字号调到
        //   16/18 后同一段文案折行数变多，多出来的行画到控件矩形外面、被下一个
        //   控件的背景盖掉半截（2026-10-30 实测）。为什么只能实测见父模块
        //   `measure_text` 的注释。
        let left = 24;
        let content_width = 500;
        let indent_left = 46;
        let indent_width = 480;
        let line = crate::dpi::line_height();
        let mut y = 20;

        // ── 第一行：连续失败次数 ────────────────────────────────────────
        let threshold_label = if english {
            "After the recovery environment fails"
        } else {
            "恢复环境连续失败"
        };
        let threshold_suffix = if english { "times:" } else { "次后：" };
        let threshold_label_width = measure_text_width(threshold_label).max(80);
        create_control(
            hwnd,
            "STATIC",
            threshold_label,
            0,
            left,
            y,
            threshold_label_width,
            line,
            0,
        );
        let threshold_edit_x = left + threshold_label_width + 12;
        create_control(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL | ES_NUMBER,
            threshold_edit_x,
            y - 2,
            56,
            line + 7,
            ID_SETTINGS_THRESHOLD_EDIT,
        );
        create_control(
            hwnd,
            "STATIC",
            threshold_suffix,
            0,
            threshold_edit_x + 66,
            y,
            160,
            line,
            0,
        );
        y += line + 10;
        let threshold_hint = if english {
            "Enter 1-20 (default 3)."
        } else {
            "可填 1–20，默认 3。"
        };
        let height = measure_text_height(threshold_hint, content_width);
        create_control(
            hwnd,
            "STATIC",
            threshold_hint,
            0,
            left,
            y,
            content_width,
            height,
            ID_SETTINGS_HINT,
        );
        y += height + 12;
        // ── 第二行：两种处理方式（标签 + 说明各自来自 failure_policy，单一来源）──
        let auto_label = auto.label(ml);
        create_control(
            hwnd,
            "BUTTON",
            auto_label,
            WS_TABSTOP | BS_AUTORADIOBUTTON | WS_GROUP,
            left,
            y,
            content_width,
            line + 4,
            ID_SETTINGS_ACTION_AUTO,
        );
        y += line + 8;
        let auto_detail = auto.detail(ml);
        let height = measure_text_height(auto_detail, indent_width);
        create_control(
            hwnd,
            "STATIC",
            auto_detail,
            0,
            indent_left,
            y,
            indent_width,
            height,
            ID_SETTINGS_DETAIL_AUTO,
        );
        y += height + 14;
        let pause_label = pause.label(ml);
        create_control(
            hwnd,
            "BUTTON",
            pause_label,
            WS_TABSTOP | BS_AUTORADIOBUTTON,
            left,
            y,
            content_width,
            line + 4,
            ID_SETTINGS_ACTION_PAUSE,
        );
        y += line + 8;
        let pause_detail = pause.detail(ml);
        let height = measure_text_height(pause_detail, indent_width);
        create_control(
            hwnd,
            "STATIC",
            pause_detail,
            0,
            indent_left,
            y,
            indent_width,
            height,
            ID_SETTINGS_DETAIL_PAUSE,
        );
        y += height + 16;
        // ── 第三行：DISM 等待上限（秒）。0 = 不限制 = 历史行为 ──────────
        let dism_label = if english {
            "Maximum wait for DISM"
        } else {
            "DISM 最长等待"
        };
        let dism_suffix = if english {
            "seconds (0 = unlimited)"
        } else {
            "秒（0=不限制）"
        };
        let dism_label_width = measure_text_width(dism_label).max(80);
        create_control(
            hwnd,
            "STATIC",
            dism_label,
            0,
            left,
            y,
            dism_label_width,
            line,
            0,
        );
        let dism_edit_x = left + dism_label_width + 12;
        create_control(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL | ES_NUMBER,
            dism_edit_x,
            y - 2,
            56,
            line + 7,
            ID_SETTINGS_DISM_TIMEOUT_EDIT,
        );
        create_control(
            hwnd,
            "STATIC",
            dism_suffix,
            0,
            dism_edit_x + 66,
            y,
            220,
            line,
            0,
        );
        y += line + 12;
        let dism_hint = crate::dism_timeout::hint(ml);
        let height = measure_text_height(dism_hint, content_width);
        create_control(
            hwnd,
            "STATIC",
            dism_hint,
            0,
            left,
            y,
            content_width,
            height,
            ID_SETTINGS_DISM_TIMEOUT_HINT,
        );
        y += height + 16;
        // ── 第四行：`pause` 策略的最长等待秒数（只对「暂停」生效）────────
        let pause_wait_label = if english {
            "Maximum wait when paused in the recovery environment"
        } else {
            "暂停在恢复环境时最长等待"
        };
        let pause_wait_suffix = if english {
            "seconds (0 = wait indefinitely)"
        } else {
            "秒（0=一直等）"
        };
        // 上限 280：英文文案在 18 号字下比这还长，放开就会把后缀挤到窗口外。
        // 中文 11 个字在 18 号下约 200，正常用不到上限。
        let pause_wait_label_width = measure_text_width(pause_wait_label).clamp(80, 280);
        create_control(
            hwnd,
            "STATIC",
            pause_wait_label,
            0,
            left,
            y,
            pause_wait_label_width,
            line,
            0,
        );
        let pause_wait_edit_x = left + pause_wait_label_width + 12;
        create_control(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL | ES_NUMBER,
            pause_wait_edit_x,
            y - 2,
            56,
            line + 7,
            ID_SETTINGS_PAUSE_WAIT_EDIT,
        );
        create_control(
            hwnd,
            "STATIC",
            pause_wait_suffix,
            0,
            pause_wait_edit_x + 66,
            y,
            220,
            line,
            0,
        );
        y += line + 12;
        let pause_wait_hint = if english {
            "Only applies to the \"pause\" option above. After the limit expires the program runs the same cleanup as \"Yes\" and reboots to Windows. Leave 0 to wait indefinitely (the previous behaviour)."
        } else {
            "只对上面选「暂停在恢复环境」时生效。超时后程序执行与「是」完全相同的清理并重启回正常 Windows。填 0 = 一直等（与旧版一致）。"
        };
        let height = measure_text_height(pause_wait_hint, content_width);
        create_control(
            hwnd,
            "STATIC",
            pause_wait_hint,
            0,
            left,
            y,
            content_width,
            height,
            0,
        );
        y += height + 16;
        // ── 保存位置说明（让用户知道改的是哪个文件、对哪些任务生效）──────
        let saved_note = if english {
            "Saved to BackupRestore.settings.ini next to the program. It applies to tasks created afterwards; existing tasks keep the setting they were created with."
        } else {
            "设置保存在程序目录的 BackupRestore.settings.ini，对之后创建的任务生效；已有任务沿用创建时的设置。"
        };
        let height = measure_text_height(saved_note, content_width);
        create_control(
            hwnd,
            "STATIC",
            saved_note,
            0,
            left,
            y,
            content_width,
            height,
            0,
        );
        y += height + 18;
        // ── 2.6.81：界面字号（用户自选）──────────────────────────────
        let font_hint = if english {
            "Interface font size:"
        } else {
            "界面字号："
        };
        let font_hint_width = measure_text_width(font_hint).max(80);
        create_control(
            hwnd,
            "STATIC",
            font_hint,
            0,
            left,
            y,
            font_hint_width,
            line,
            ID_SETTINGS_FONT_SIZE_HINT,
        );
        // 下拉列表的「高度」参数是**展开后列表**的总高度（不是闭合时的高度，
        // 闭合高度由字体决定）。旧值写死 24，比一行还矮，展开后一条都看不到。
        let font_combo_height =
            (line + 6) * (crate::dpi::FONT_SIZE_CHOICES.len() as i32 + 1);
        let font_combo = create_control(
            hwnd,
            "COMBOBOX",
            "",
            WS_TABSTOP | CBS_DROPDOWNLIST,
            left + font_hint_width + 12,
            y - 4,
            180,
            font_combo_height,
            ID_SETTINGS_FONT_SIZE_COMBO,
        );
        y += line + 20;
        for size in crate::dpi::FONT_SIZE_CHOICES {
            let label = match size {
                12 => {
                    if english {
                        "12 (small)"
                    } else {
                        "12（小）"
                    }
                }
                14 => {
                    if english {
                        "14 (standard)"
                    } else {
                        "14（标准）"
                    }
                }
                16 => {
                    if english {
                        "16 (large)"
                    } else {
                        "16（大）"
                    }
                }
                _ => {
                    if english {
                        "18 (extra large)"
                    } else {
                        "18（特大）"
                    }
                }
            };
            add_combo_item(font_combo, label);
        }
        // ── 按钮行 ─────────────────────────────────────────────────
        y += 10;
        create_control(
            hwnd,
            "BUTTON",
            if english { "Cancel" } else { "取消" },
            WS_TABSTOP,
            330,
            y,
            96,
            30,
            ID_SETTINGS_CANCEL,
        );
        create_control(
            hwnd,
            "BUTTON",
            if english { "Save" } else { "保存" },
            WS_TABSTOP | BS_DEFPUSHBUTTON,
            434,
            y,
            96,
            30,
            ID_SETTINGS_SAVE,
        );
        // ── 按实测内容定窗口尺寸并居中 ────────────────────────────────
        // ★ 2.6.82 只把窗口尺寸跟着 DPI 放大，高度仍写死 590（按 14 号字算的），
        //   字号一大内容就顶出窗口底部。现在客户区高度直接取「最后一个控件底缘 +
        //   边距」，再用 AdjustWindowRectEx 由客户区反推整窗尺寸（含标题栏）。
        //   窗口还没 ShowWindow，这里 MoveWindow 不会闪。
        let client_h = y + 30 + 18;
        let frame_style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
        let mut frame = Rect {
            left: 0,
            top: 0,
            right: crate::dpi::s(560),
            bottom: crate::dpi::s(client_h),
        };
        AdjustWindowRectEx(&mut frame, frame_style, 0, 0);
        let window_w = frame.right - frame.left;
        let window_h = frame.bottom - frame.top;
        let screen_w = GetSystemMetrics(SM_CXSCREEN).max(640);
        let screen_h = GetSystemMetrics(SM_CYSCREEN).max(480);
        MoveWindow(
            hwnd,
            ((screen_w - window_w) / 2).max(0),
            ((screen_h - window_h) / 2).max(0),
            window_w,
            window_h,
            1,
        );
        // 初值：次数 + 选中的行为 + DISM 等待上限 + pause 最长等待。
        set_child_text(
            hwnd,
            ID_SETTINGS_THRESHOLD_EDIT,
            &SETTINGS_INITIAL_THRESHOLD
                .load(std::sync::atomic::Ordering::SeqCst)
                .to_string(),
        );
        set_child_text(
            hwnd,
            ID_SETTINGS_DISM_TIMEOUT_EDIT,
            &SETTINGS_INITIAL_DISM_TIMEOUT
                .load(std::sync::atomic::Ordering::SeqCst)
                .to_string(),
        );
        set_child_text(
            hwnd,
            ID_SETTINGS_PAUSE_WAIT_EDIT,
            &SETTINGS_INITIAL_PAUSE_WAIT
                .load(std::sync::atomic::Ordering::SeqCst)
                .to_string(),
        );
        let checked = if SETTINGS_INITIAL_ACTION.load(std::sync::atomic::Ordering::SeqCst) == 1 {
            ID_SETTINGS_ACTION_PAUSE
        } else {
            ID_SETTINGS_ACTION_AUTO
        };
        SendMessageW(
            GetDlgItem(hwnd, checked as i32),
            BM_SETCHECK,
            BST_CHECKED,
            0,
        );
        // 字号下拉框初值：按值找索引（`FONT_SIZE_CHOICES` 是档位的单一来源）。
        let initial_font_size = SETTINGS_INITIAL_FONT_SIZE.load(std::sync::atomic::Ordering::SeqCst);
        let font_index = crate::dpi::FONT_SIZE_CHOICES
            .iter()
            .position(|&s| s == initial_font_size)
            .unwrap_or(1);
        SendMessageW(
            GetDlgItem(hwnd, ID_SETTINGS_FONT_SIZE_COMBO as i32),
            CB_SETCURSEL,
            font_index as usize,
            0,
        );
        SetFocus(GetDlgItem(hwnd, ID_SETTINGS_THRESHOLD_EDIT as i32));
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
            // GUI 操作审计：与 main 主窗口同一入口策略——先留痕再走业务分支。
            audit_control_action(control_id, (w_param >> 16) & 0xffff);
        match control_id {
            ID_SETTINGS_CANCEL => {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
                return 0;
            }
            ID_SETTINGS_SAVE => {
                let english = SETTINGS_LANGUAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
                let language = if english {
                    Language::English
                } else {
                    Language::Chinese
                };
                let ml = message_language(language);
                let raw = get_text(GetDlgItem(hwnd, ID_SETTINGS_THRESHOLD_EDIT as i32));
                // 校验复用 `failure_policy` 的解析：错误文案与 CLI 侧同一来源。
                let Ok(threshold) =
                    crate::failure_policy::FailurePolicy::parse_threshold(&raw, ml)
                else {
                    show_message(
                        hwnd,
                        if english {
                            "Enter a whole number between 1 and 20."
                        } else {
                            "失败次数必须是 1 到 20 之间的整数。"
                        },
                        if english {
                            "Invalid value"
                        } else {
                            "取值无效"
                        },
                        MB_OK | MB_ICONWARNING,
                    );
                    SetFocus(GetDlgItem(hwnd, ID_SETTINGS_THRESHOLD_EDIT as i32));
                    return 0;
                };
                let action = if IsDlgButtonChecked(hwnd, ID_SETTINGS_ACTION_PAUSE as i32) != 0 {
                    FailureAction::PauseInRecovery
                } else {
                    FailureAction::AutoEscape
                };
                // DISM 等待上限：校验复用 `dism_timeout` 的解析（同一来源）。
                let dism_raw = get_text(GetDlgItem(hwnd, ID_SETTINGS_DISM_TIMEOUT_EDIT as i32));
                let Ok(dism_secs) = crate::dism_timeout::parse_secs(&dism_raw) else {
                    show_message(
                        hwnd,
                        if english {
                            "Enter a whole number of seconds between 0 and 86400 (0 = unlimited)."
                        } else {
                            "请输入 0 到 86400 之间的整数秒（0 = 不限制）。"
                        },
                        if english {
                            "Invalid value"
                        } else {
                            "取值无效"
                        },
                        MB_OK | MB_ICONWARNING,
                    );
                    SetFocus(GetDlgItem(hwnd, ID_SETTINGS_DISM_TIMEOUT_EDIT as i32));
                    return 0;
                };
                // `pause` 最长等待秒数：校验复用 `failure_policy` 的解析（同一来源）。
                let pause_wait_raw =
                    get_text(GetDlgItem(hwnd, ID_SETTINGS_PAUSE_WAIT_EDIT as i32));
                let Ok(pause_wait_secs) =
                    crate::failure_policy::parse_pause_max_wait_text(&pause_wait_raw, ml)
                else {
                    show_message(
                        hwnd,
                        if english {
                            "Enter a whole number of seconds between 0 and 86400 (0 = wait indefinitely)."
                        } else {
                            "请输入 0 到 86400 之间的整数秒（0 = 一直等）。"
                        },
                        if english {
                            "Invalid value"
                        } else {
                            "取值无效"
                        },
                        MB_OK | MB_ICONWARNING,
                    );
                    SetFocus(GetDlgItem(hwnd, ID_SETTINGS_PAUSE_WAIT_EDIT as i32));
                    return 0;
                };
                SETTINGS_DISM_TIMEOUT_RESULT.store(dism_secs, std::sync::atomic::Ordering::SeqCst);
                SETTINGS_PAUSE_WAIT_RESULT
                    .store(pause_wait_secs, std::sync::atomic::Ordering::SeqCst);
                // 界面字号：按**下拉框索引**取值而不是解析文本——文本带本地化后缀
                // （「14（标准）」/「14 (standard)」），解析它会把两种语言都写死，
                // 而索引与语言无关（`FONT_SIZE_CHOICES` 是单一来源）。
                let font_combo = GetDlgItem(hwnd, ID_SETTINGS_FONT_SIZE_COMBO as i32);
                let font_index = SendMessageW(font_combo, CB_GETCURSEL, 0, 0);
                let font_size = if font_index >= 0 {
                    crate::dpi::FONT_SIZE_CHOICES
                        .get(font_index as usize)
                        .copied()
                        .unwrap_or(crate::dpi::DEFAULT_FONT_SIZE)
                } else {
                    crate::dpi::DEFAULT_FONT_SIZE
                };
                SETTINGS_FONT_SIZE_RESULT.store(font_size, std::sync::atomic::Ordering::SeqCst);
                if let Ok(policy) = crate::failure_policy::FailurePolicy::new(threshold, action)
                    && let Ok(mut slot) = SETTINGS_RESULT.lock()
                {
                    *slot = Some(policy);
                }
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
                return 0;
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, message, w_param, l_param)
}

/// 打开「程序设置」模态窗口，返回用户保存的新策略（取消/关窗返回 `None`）。
///
/// 调用方负责持久化（写 `BackupRestore.settings.ini`）与更新内存里的策略——
/// 窗口过程只做「取值 + 校验」，不碰磁盘，便于单测与排查。
pub(super) unsafe fn ask_settings_dialog(
    hwnd: Hwnd,
    language: Language,
    current: crate::failure_policy::FailurePolicy,
    executable_dir: &std::path::Path,
) -> Option<crate::failure_policy::FailurePolicy> {
    use crate::failure_policy::FailureAction;
    let instance = GetModuleHandleW(null());
    let class_name = wide(SETTINGS_WINDOW_CLASS);
    let class = WndClassExW {
        cb_size: size_of::<WndClassExW>() as u32,
        style: 0,
        wnd_proc: Some(window_proc_settings),
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
        return None;
    }
    SETTINGS_LANGUAGE.store(
        if language == Language::English { 1 } else { 0 },
        std::sync::atomic::Ordering::SeqCst,
    );
    SETTINGS_INITIAL_THRESHOLD.store(current.threshold, std::sync::atomic::Ordering::SeqCst);
    SETTINGS_INITIAL_ACTION.store(
        if current.action == FailureAction::PauseInRecovery {
            1
        } else {
            0
        },
        std::sync::atomic::Ordering::SeqCst,
    );
    SETTINGS_INITIAL_DISM_TIMEOUT.store(
        crate::dism_timeout::load_from_dir(executable_dir),
        std::sync::atomic::Ordering::SeqCst,
    );
    SETTINGS_INITIAL_PAUSE_WAIT.store(
        current.pause_max_wait_secs,
        std::sync::atomic::Ordering::SeqCst,
    );
    // 哨兵：本次没保存。0 是合法值（= 不限制），不能拿 0 当"未保存"。
    SETTINGS_DISM_TIMEOUT_RESULT.store(SETTINGS_RESULT_UNSET, std::sync::atomic::Ordering::SeqCst);
    SETTINGS_PAUSE_WAIT_RESULT.store(SETTINGS_RESULT_UNSET, std::sync::atomic::Ordering::SeqCst);
    SETTINGS_FONT_SIZE_RESULT.store(SETTINGS_RESULT_UNSET, std::sync::atomic::Ordering::SeqCst);
    SETTINGS_INITIAL_FONT_SIZE.store(crate::dpi::font_size(), std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut slot) = SETTINGS_RESULT.lock() {
        *slot = None;
    }
    let caption = wide(if language == Language::English {
        "Settings"
    } else {
        "程序设置"
    });
    // ★ 2.6.83：这里的尺寸只是**初始占位**，真正的尺寸由 `WM_CREATE` 末尾按
    //   实测内容高度算出来再 MoveWindow 居中（见 `window_proc_settings`）。
    //   历史：项目此前在 460×268 上裁掉过「取消」按钮；2026-10-16 加 DISM 等待
    //   上限与 pause 最长等待两行 → 400 → 480 → 560；2.6.81 加界面字号一行 → 590。
    //   这一串数字每次加行都要手算，且字号一变就失效，所以 2.6.83 彻底改成实测。
    let screen_w = GetSystemMetrics(SM_CXSCREEN).max(640);
    let screen_h = GetSystemMetrics(SM_CYSCREEN).max(590);
    let dialog = CreateWindowExW(
        0,
        class_name.as_ptr(),
        caption.as_ptr(),
        WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
        // ★ 2.6.81：窗口尺寸必须和内部控件**一起**按 DPI 缩放。之前只缩放了控件
        //   （它们走父模块的 `create_control`），窗口本身没缩放，200% 下控件被画到
        //   窗口外面，右侧与底部内容全被切掉（用户 2026-10-30 截图反馈）。
        (screen_w - crate::dpi::s(560)) / 2,
        (screen_h - crate::dpi::s(590)) / 2,
        crate::dpi::s(560),
        crate::dpi::s(590),
        hwnd,
        null_mut(),
        instance,
        null_mut(),
    );
    if dialog.is_null() {
        return None;
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
    // 退出条件用 `IsWindow(dialog)`：`WM_COMMAND` 里的 `DestroyWindow` 是同步的，
    // 处理完消息回到这里窗口已不存在，循环必然结束（与系统盘选择弹窗同一写法）。
    while IsWindow(dialog) != 0 && GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
        if IsDialogMessageW(dialog, &message) == 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    EnableWindow(hwnd, 1);
    SetForegroundWindow(hwnd);
    SETTINGS_RESULT.lock().ok().and_then(|mut slot| slot.take())
}
