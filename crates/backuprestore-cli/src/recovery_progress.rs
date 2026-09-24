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
const TIMER_ID: usize = 1;

/// 与窗口线程共享的进度状态：当前活动日志路径（主线程切换日志时更新）。
pub struct ProgressShared {
    pub log_path: Mutex<PathBuf>,
    pub log_offset: Mutex<u64>,
    pub window_up: AtomicBool,
    /// 进度窗口句柄（窗口线程创建后回填；主线程执行完请求关闭用 usize 存，
    /// 避免 *mut c_void 不满足跨线程 Send）。
    pub hwnd: Mutex<Option<usize>>,
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
    let mut buffer = String::new();
    if file.read_to_string(&mut buffer).is_err() {
        // 可能为 UTF-16 或正在写入，回退到读原始字节后按字节过滤。
        return;
    }
    *offset = size;
    drop(file);

    let mut latest_stage: Option<String> = None;
    let mut latest_percent: Option<u32> = None;
    let mut details: Vec<String> = Vec::new();
    // DISM 进度条用 \r 原地刷新（重定向到文件时不带 \n），先把整段按
    // \r/\n 都拆成行再分类，否则整段会合并成一行、百分比永远取到第一个。
    for line in buffer.split(['\n', '\r']) {
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
    unsafe {
        if let Some(stage) = latest_stage {
            let wide: Vec<u16> = stage.encode_utf16().chain(std::iter::once(0)).collect();
            SetWindowTextW(GetDlgItem(hwnd, ID_STAGE), wide.as_ptr());
        }
        if let Some(percent) = latest_percent {
            SendMessageW(GetDlgItem(hwnd, ID_BAR), PBM_SETPOS, percent as usize, 0);
        }
        if !details.is_empty() {
            let text = details.join("\r\n");
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
            // 阶段文本
            let stage = CreateWindowExW(
                0,
                encode("Static").as_ptr(),
                encode("正在准备恢复环境…").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                16,
                16,
                640,
                30,
                hwnd,
                null_mut(),
                GetModuleHandleW(null()),
                null_mut(),
            );
            let font = GetStockObject(DEFAULT_GUI_FONT);
            if !stage.is_null() {
                SendMessageW(stage, WM_SETFONT, font as usize, 1);
            }
            // 进度条
            let bar = CreateWindowExW(
                0,
                encode("msctls_progress32").as_ptr(),
                null(),
                WS_CHILD | WS_VISIBLE | WS_BORDER,
                16,
                56,
                640,
                26,
                hwnd,
                null_mut(),
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !bar.is_null() {
                SendMessageW(bar, PBM_SETRANGE32, 0, 100);
                SendMessageW(bar, PBM_SETPOS, 0, 0);
            }
            // 详情（多行只读）
            let detail = CreateWindowExW(
                0,
                encode("Edit").as_ptr(),
                null(),
                WS_CHILD | WS_VISIBLE | WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY,
                16,
                94,
                640,
                180,
                hwnd,
                null_mut(),
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !detail.is_null() {
                SendMessageW(detail, WM_SETFONT, font as usize, 1);
            }
            SetTimer(hwnd, TIMER_ID, 500, None);
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
pub fn spawn(initial_log: PathBuf) -> Arc<ProgressShared> {
    let shared = Arc::new(ProgressShared {
        log_path: Mutex::new(initial_log),
        log_offset: Mutex::new(0),
        window_up: AtomicBool::new(false),
        hwnd: Mutex::new(None),
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
            30,
            30,
            680,
            300,
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
        ShowWindow(hwnd, 1); // SW_SHOWNORMAL
        UpdateWindow(hwnd);
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
    // 释放 GWL_USERDATA 持有的 Arc（窗口销毁后）
    let _ = unsafe { Arc::from_raw(raw as *const ProgressShared) };
}
