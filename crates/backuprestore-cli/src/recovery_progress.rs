//! WinRE 恢复进度窗口。
//!
//! Recovery.exe 在 WinRE/PE 里运行时，用原生 Win32 窗口显示恢复进度
//! （阶段文本 + DISM 百分比进度条 + 日志尾部），替代无任何提示的 cmd 黑窗。
//! 窗口线程独立运行：只读当前活动 Recovery.log 的增量尾部解析阶段与百分比，
//! 不侵入恢复主流程；日志路径切换（早期日志→任务目录→镜像同目录）由
//! 主线程通过 `ProgressShared.log_path` 同步。
#![cfg(windows)]

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::native_gui::Msg;
use crate::text_parsing::classify_log_line;

type Hwnd = *mut c_void;
type WParam = usize;
type LParam = isize;
type LResult = isize;

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_BORDER: u32 = 0x0080_0000;
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_READONLY: u32 = 0x0800;
const WM_CREATE: u32 = 0x0001;
const WM_CLOSE: u32 = 0x0010;
const WM_DESTROY: u32 = 0x0002;
const WM_TIMER: u32 = 0x0113;
const WM_SETFONT: u32 = 0x0030;
const PBM_SETRANGE32: u32 = 0x0406;
const PBM_SETPOS: u32 = 0x0402;
const GWL_USERDATA: i32 = -21;
const DEFAULT_GUI_FONT: i32 = 17;
const ID_STAGE: i32 = 1;
const ID_BAR: i32 = 2;
const ID_DETAIL: i32 = 3;
const ID_TIME_INFO: i32 = 4;
const SS_RIGHT: u32 = 0x0000_0002;
const TIMER_ID: usize = 1;
const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
const FW_BOLD: i32 = 700;
const DEFAULT_CHARSET: u32 = 1;
const CLEARTYPE_QUALITY: u32 = 5;

/// 与窗口线程共享的进度状态：当前活动日志路径（主线程切换日志时更新）。
pub struct ProgressShared {
    pub log_path: Mutex<PathBuf>,
    pub log_offset: Mutex<u64>,
    pub window_up: AtomicBool,
    // 窗口尚未创建时收到关闭请求也要记住，避免快速失败留下孤立进度窗。
    close_requested: AtomicBool,
    /// 任务开始时间戳，用于计算已用时间与预估剩余时间。
    pub start_time: Mutex<std::time::Instant>,
    /// 进度窗口句柄（窗口线程创建后回填；主线程执行完请求关闭用 usize 存，
    /// 避免 *mut c_void 不满足跨线程 Send）。
    pub hwnd: Mutex<Option<usize>>,
    /// 编号步骤跟踪器。必须放这里而不是窗口线程的局部变量：
    /// 窗口每次只读到日志**增量**，而「一共几步、已完成几步」要跨增量累积，
    /// 否则两次刷新之间就会忘记自己走到哪。
    pub steps: Mutex<crate::text_parsing::StepTracker>,
    /// 操作类型（如 BACKUP / RESTORE_EXISTING），用于未打出步骤前预填骨架标题。
    pub operation: Mutex<Option<String>>,
}

impl ProgressShared {
    /// 切换监控的日志文件路径，并指定起始偏移（例如 0，或者累积日志的追加点）。
    pub fn switch_log_path(&self, new_path: PathBuf, initial_offset: u64) {
        let mut path_guard = self.log_path.lock().unwrap();
        *path_guard = new_path;
        let mut offset_guard = self.log_offset.lock().unwrap();
        *offset_guard = initial_offset;
    }
}

#[repr(C)]
struct WndClassExW {
    cb_size: u32,
    style: u32,
    wnd_proc: Option<unsafe extern "system" fn(Hwnd, u32, WParam, LParam) -> LResult>,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    instance: *mut c_void,
    icon: Hwnd,
    cursor: Hwnd,
    brush: Hwnd,
    menu_name: *const u16,
    class_name: *const u16,
    icon_sm: Hwnd,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: Hwnd,
        instance: *mut c_void,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, message: u32, w_param: WParam, l_param: LParam) -> LResult;
    fn GetMessageW(message: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn DispatchMessageW(message: *const Msg) -> LResult;
    fn TranslateMessage(message: *const Msg) -> i32;
    fn PostQuitMessage(exit_code: i32);
    fn PostMessageW(hwnd: Hwnd, message: u32, w_param: WParam, l_param: LParam) -> i32;
    fn SendMessageW(hwnd: Hwnd, message: u32, w_param: WParam, l_param: LParam) -> LResult;
    fn SetTimer(
        hwnd: Hwnd,
        id: usize,
        elapse: u32,
        timer_proc: Option<unsafe extern "system" fn(Hwnd, u32, WParam, u32)>,
    ) -> usize;
    fn KillTimer(hwnd: Hwnd, id: usize) -> i32;
    fn ShowWindow(hwnd: Hwnd, command: i32) -> i32;
    fn UpdateWindow(hwnd: Hwnd) -> i32;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn GetDlgItem(hwnd: Hwnd, id: i32) -> Hwnd;
    fn GetStockObject(index: i32) -> Hwnd;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn LoadCursorW(instance: *mut c_void, name: *const u16) -> Hwnd;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
}

#[link(name = "comctl32")]
unsafe extern "system" {
    fn InitCommonControlsEx(icc: *const InitCommonControlsEx) -> i32;
}

#[repr(C)]
struct InitCommonControlsEx {
    size: u32,
    flags: u32,
}

const ICC_PROGRESS_CLASS: u32 = 0x0000_0020;

const IDC_ARROW: *const u16 = 32512u16 as *const u16;

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike_out: u32,
        char_set: u32,
        output_precision: u32,
        clipping_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face_name: *const u16,
    ) -> Hwnd;
}

/// 创建大号中文字体（负高度 = 字符高度；微软雅黑优先，缺失时系统回退）。
const GB2312_CHARSET: u32 = 134;

/// 创建大号中文字体（负高度 = 字符高度；微软雅黑优先，GB2312 字符集确保 WinRE 缺失时正确回退到中文字体）。
fn create_font(height: i32, bold: bool) -> Hwnd {
    let face = encode("Microsoft YaHei");
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            if bold { FW_BOLD } else { 0 },
            0,
            0,
            0,
            GB2312_CHARSET,
            0,
            0,
            CLEARTYPE_QUALITY,
            0,
            face.as_ptr(),
        )
    }
}

/// 格式化秒数为 hh:mm:ss 或 mm:ss。
fn format_hms(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 读取日志增量新行并分类更新窗口控件。返回（阶段、百分比、详情）。
fn refresh_from_log(shared: &ProgressShared, hwnd: Hwnd) {
    let path = shared.log_path.lock().unwrap().clone();
    let mut offset = shared.log_offset.lock().unwrap();
    let mut file = match OpenOptions::new().read(true).open(&path) {
        Ok(file) => file,
        Err(_) => return, // 日志尚未出现，静默等待
    };
    let size = match file.metadata() {
        Ok(meta) => meta.len(),
        Err(_) => return,
    };
    if size < *offset {
        *offset = 0; // 日志被重建/替换
    }
    if size == *offset {
        return; // 无新内容
    }
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        *offset = 0;
        return;
    }
    let mut raw = Vec::new();
    if file.read_to_end(&mut raw).is_err() {
        return;
    }
    *offset = size;
    drop(file);
    // 中文 Windows 的 DISM 输出是 GBK 编码，不是合法 UTF-8。必须用有损解码，
    // 否则 read_to_string 一旦遇到非 UTF-8 字节就整体失败，窗口永远冻结在初始态。
    let buffer = crate::text_parsing::decode_windows_bytes(&raw);

    let mut latest_stage: Option<String> = None;
    let mut latest_percent: Option<u32> = None;
    let mut details: Vec<String> = Vec::new();
    // DISM 进度条用 \r 原地刷新（重定向到文件时不带 \n），先把整段按
    // \r/\n 都拆成行再分类，否则整段会合并成一行、百分比永远取到第一个。
    let lines: Vec<&str> = buffer.split(['\n', '\r']).collect();
    for line in &lines {
        let (stage, percent, detail) = classify_log_line(line);
        if let Some(value) = stage {
            latest_stage = Some(value);
        }
        if let Some(value) = percent {
            latest_percent = Some(value);
        }
        if let Some(value) = detail {
            details.push(value);
            if details.len() > 4 {
                details.remove(0);
            }
        }
    }
    // 编号步骤跨增量累积，并算出进度。
    let progress = {
        let mut tracker = shared.steps.lock().unwrap();
        tracker.feed(&lines);
        tracker.set_current_percent(latest_percent);
        tracker.snapshot()
    };
    unsafe {
        // 用户要求：阶段大标题保持精简（如 "1/4 挂载卷、校验"、"2/4 还原镜像（时间长）"），
        // 且绝对不能被 DISM 控制台的 "正在还原系统分区…" 过程日志覆盖。
        let current_step_title = if let Some(current_idx) = progress.current {
            progress
                .steps
                .iter()
                .find(|(idx, _, _)| *idx == current_idx)
                .map(|(_, name, _)| format!("{current_idx}/{} {name}", progress.total.unwrap_or(4)))
        } else {
            None
        };
        let final_stage = if latest_stage.as_deref() == Some("操作完成") {
            latest_stage.clone()
        } else {
            current_step_title.or_else(|| latest_stage.clone())
        };
        if let Some(stage) = final_stage {
            let wide: Vec<u16> = stage.encode_utf16().chain(std::iter::once(0)).collect();
            SetWindowTextW(GetDlgItem(hwnd, ID_STAGE), wide.as_ptr());
        }
        // 用户要求：进度条显示当前阶段进度而非总进度。
        // 当前阶段有百分比时显示当前阶段百分比（如 DISM 1%~100%），没有时显示 0。
        let pos = progress
            .current_percent
            .or(latest_percent)
            .unwrap_or(0);
        SendMessageW(GetDlgItem(hwnd, ID_BAR), PBM_SETPOS, pos as usize, 0);

        // 标题右侧增加一块显示已用时间/剩余时间、当前时间，靠右显示。
        let elapsed = shared.start_time.lock().unwrap().elapsed().as_secs();
        let elapsed_str = format_hms(elapsed);
        let remaining_str = if pos > 0 && pos < 100 {
            let total_est = elapsed * 100 / (pos as u64);
            let rem = total_est.saturating_sub(elapsed);
            format_hms(rem)
        } else {
            "--:--".to_string()
        };
        let now_str = chrono::Local::now().format("%H:%M:%S").to_string();
        let time_text = format!("已用: {elapsed_str}  剩余: {remaining_str}\r\n当前时间: {now_str}");
        let time_wide = encode(&time_text);
        SetWindowTextW(GetDlgItem(hwnd, ID_TIME_INFO), time_wide.as_ptr());

        // 详情区预填 4 阶段骨架清单，再跟日志尾部。清单用 ✓/▶/· 标出
        // 已完成/进行中/未开始；正文每个已开始阶段后面附上时间戳（时分秒）[HH:MM:SS]。
        let online = shared.operation.lock().unwrap().as_deref().is_some_and(|s| s.starts_with("ONLINE_"));
        let is_backup = {
            let op_guard = shared.operation.lock().unwrap();
            op_guard
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case("BACKUP") || s.eq_ignore_ascii_case("ONLINE_BACKUP"))
                .unwrap_or(false)
        };
        let step2_default = if is_backup {
            "捕获镜像（时间长）"
        } else {
            "还原镜像（时间长）"
        };
        let default_stages = [
            (1, "挂载卷、校验"),
            (2, step2_default),
            (3, "校验"),
            (4, if online { "发布副档/完成在线操作" } else { "清理re启动项/配置" }),
        ];

        let total_steps = progress.total.unwrap_or(4);
        let current = progress.current.unwrap_or(0);
        let all_done = latest_stage.as_deref() == Some("操作完成")
            || (current == total_steps && pos >= 100);

        let mut body: Vec<String> = Vec::new();
        for (idx, default_name) in default_stages {
            if idx > total_steps {
                continue;
            }
            let recorded = progress.steps.iter().find(|(i, _, _)| *i == idx);
            let name = recorded.map(|(_, n, _)| n.as_str()).unwrap_or(default_name);
            let ts = recorded.and_then(|(_, _, t)| t.as_deref());

            let mark = if all_done || idx < current {
                "✓"
            } else if idx == current {
                // 用户要求：当前正在执行阶段的小箭头亮 1 秒、消失 1 秒（每秒刷新时交替闪烁），
                // 消失时使用全角空格占位，保证排版平稳、文字不发生横向抖动。
                if elapsed % 2 == 0 {
                    "▶"
                } else {
                    "\u{3000}"
                }
            } else {
                "·"
            };
            let time_str = match ts {
                Some(t) => format!(" [{t}]"),
                None => String::new(),
            };
            body.push(format!("{mark} {idx}/{total_steps} {name}{time_str}"));
        }
        // 移除总进度，只保留当前进度
        if let Some(step) = latest_percent {
            body.push(format!("—— 当前进度 {step}% ——"));
        }
        body.push(String::new());
        body.extend(details.iter().cloned());
        if !body.is_empty() {
            let text = body.join("\r\n");
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            SetWindowTextW(GetDlgItem(hwnd, ID_DETAIL), wide.as_ptr());
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    match message {
        WM_CREATE => unsafe {
            // 大号字体：阶段标题加粗醒目，详情保持可读。
            let stage_font = create_font(-30, true);
            let detail_font = create_font(-20, false);
            let gui_font = GetStockObject(DEFAULT_GUI_FONT);
            // 阶段文本（大号加粗，左侧）
            let stage = CreateWindowExW(
                0,
                encode("Static").as_ptr(),
                encode("正在准备恢复环境…").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                32,
                28,
                440,
                48,
                hwnd,
                ID_STAGE as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            let _ = gui_font;
            if !stage.is_null() {
                SendMessageW(stage, WM_SETFONT, stage_font as usize, 1);
            }
            // 时间信息（靠右显示：已用/剩余/当前时间）
            let time_font = create_font(-18, false);
            let time_info = CreateWindowExW(
                0,
                encode("Static").as_ptr(),
                null(),
                WS_CHILD | WS_VISIBLE | SS_RIGHT,
                480,
                28,
                348,
                48,
                hwnd,
                ID_TIME_INFO as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !time_info.is_null() {
                SendMessageW(time_info, WM_SETFONT, time_font as usize, 1);
            }
            // 进度条
            let bar = CreateWindowExW(
                0,
                encode("msctls_progress32").as_ptr(),
                null(),
                WS_CHILD | WS_VISIBLE | WS_BORDER,
                32,
                92,
                796,
                36,
                hwnd,
                ID_BAR as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !bar.is_null() {
                SendMessageW(bar, PBM_SETRANGE32, 0, 100);
                SendMessageW(bar, PBM_SETPOS, 0, 0);
            }
            // 详情（多行只读，大区域）
            let detail = CreateWindowExW(
                0,
                encode("Edit").as_ptr(),
                null(),
                WS_CHILD | WS_VISIBLE | WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY,
                32,
                144,
                796,
                414,
                hwnd,
                ID_DETAIL as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !detail.is_null() {
                SendMessageW(detail, WM_SETFONT, detail_font as usize, 1);
            }
            SetTimer(hwnd, TIMER_ID, 1000, None);
            0
        },
        WM_TIMER => unsafe {
            let shared_ptr = GetWindowLongPtrW(hwnd, GWL_USERDATA) as *const ProgressShared;
            if !shared_ptr.is_null() {
                refresh_from_log(&*shared_ptr, hwnd);
            }
            0
        },
        WM_DESTROY => unsafe {
            KillTimer(hwnd, TIMER_ID);
            PostQuitMessage(0);
            0
        },
        _ => unsafe { DefWindowProcW(hwnd, message, w_param, l_param) },
    }
}

fn encode(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 启动恢复进度窗口线程，返回共享状态句柄（主线程用于切换日志路径）。
pub fn spawn(initial_log: PathBuf, operation: Option<&str>) -> Arc<ProgressShared> {
    let shared = Arc::new(ProgressShared {
        log_path: Mutex::new(initial_log),
        steps: Mutex::new(crate::text_parsing::StepTracker::new()),
        log_offset: Mutex::new(0),
        window_up: AtomicBool::new(false),
        close_requested: AtomicBool::new(false),
        start_time: Mutex::new(std::time::Instant::now()),
        hwnd: Mutex::new(None),
        operation: Mutex::new(operation.map(|s| s.to_string())),
    });
    let thread_shared = Arc::clone(&shared);
    let _ = thread::spawn(move || {
        // 窗口线程：创建 Win32 进度窗口并进入消息循环。
        unsafe { run_window(&thread_shared) };
    });
    shared
}

/// 请求关闭进度窗口（主线程操作执行完毕后调用，避免窗口残留在前台）。
pub fn request_close(shared: &ProgressShared) {
    shared.close_requested.store(true, Ordering::SeqCst);
    if let Some(hwnd) = *shared.hwnd.lock().unwrap()
        && hwnd != 0
    {
        unsafe {
            PostMessageW(hwnd as Hwnd, WM_CLOSE, 0, 0);
        }
    }
}

unsafe fn run_window(shared: &Arc<ProgressShared>) {
    let class_name = encode("BackupRestoreProgressClass");
    let class = WndClassExW {
        cb_size: std::mem::size_of::<WndClassExW>() as u32,
        style: 0,
        wnd_proc: Some(window_proc),
        cb_cls_extra: 0,
        cb_wnd_extra: 0,
        instance: unsafe { GetModuleHandleW(null()) },
        icon: null_mut(),
        cursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        brush: null_mut(),
        menu_name: null(),
        class_name: class_name.as_ptr(),
        icon_sm: null_mut(),
    };
    let icc = InitCommonControlsEx {
        size: std::mem::size_of::<InitCommonControlsEx>() as u32,
        flags: ICC_PROGRESS_CLASS,
    };
    unsafe {
        InitCommonControlsEx(&icc);
        if RegisterClassExW(&class) == 0 {
            return; // 类已注册或失败：进度窗口非关键，静默降级
        }
    }
    let title = encode("BackupRestore 恢复进度");
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            860,
            600,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null_mut(),
        )
    };
    if hwnd.is_null() {
        return;
    }
    // 窗口过程通过 GWL_USERDATA 拿共享状态，用于 WM_TIMER 读日志。
    let raw = Arc::into_raw(Arc::clone(shared)) as isize;
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_USERDATA, raw);
    }
    *shared.hwnd.lock().unwrap() = Some(hwnd as usize);
    shared.window_up.store(true, Ordering::SeqCst);
    unsafe {
        if shared.close_requested.load(Ordering::SeqCst) {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        } else {
            ShowWindow(hwnd, 1);
            UpdateWindow(hwnd);
        }
    }

    let mut msg = std::mem::MaybeUninit::<Msg>::uninit();
    loop {
        let result = unsafe { GetMessageW(msg.as_mut_ptr(), null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        unsafe {
            TranslateMessage(msg.as_ptr());
            DispatchMessageW(msg.as_ptr());
        }
    }
    *shared.hwnd.lock().unwrap() = None;
    shared.window_up.store(false, Ordering::SeqCst);
    // 释放 GWL_USERDATA 持有的 Arc（窗口销毁后）
    let _ = unsafe { Arc::from_raw(raw as *const ProgressShared) };
}
