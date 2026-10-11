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
/// 主界面常驻的日志尾部（固定显示最后 TAIL_LINES 行），不必点开弹窗即可判断是否卡住。
const ID_TAIL: i32 = 5;
/// 「详细日志」按钮，位于日志尾部右侧。
const ID_LOG_BUTTON: i32 = 6;
/// 详细日志弹窗内部的只读文本框。
const ID_LOG_EDIT: i32 = 7;
const SS_RIGHT: u32 = 0x0000_0002;
const TIMER_ID: usize = 1;
/// 详细日志弹窗的刷新定时器（与主窗口定时器区分）。
const LOG_TIMER_ID: usize = 2;
const WM_SIZE: u32 = 0x0005;
const WM_COMMAND: u32 = 0x0111;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const EM_SETSEL: u32 = 0x00B1;
const EM_SCROLLCARET: u32 = 0x00B7;
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SW_RESTORE: i32 = 9;
/// 主界面日志尾部固定显示的行数（用户要求 3 行）。
const TAIL_LINES: usize = 3;
/// 详细日志弹窗一次最多回看的日志字节数；超出部分丢弃最早内容。
const LOG_VIEW_BYTES: u64 = 512 * 1024;
/// 详细日志弹窗一次最多渲染的行数，避免超大日志把控件拖慢。
const LOG_VIEW_LINES: usize = 3000;
const WM_KEYDOWN: u32 = 0x0100;
/// F3：无论焦点在哪个控件都能打开详细日志弹窗。
/// WinRE 里鼠标并非总是可用，必须留一条纯键盘通路。
const VK_F3: usize = 0x72;
const VK_ESCAPE: usize = 0x1B;
/// 窗口类背景画刷：`COLOR_WINDOW + 1`。
///
/// 这个值必须给，不能留 `null`：类画刷为空时 Windows 不擦除客户区，
/// 控件之间的空白会残留上一次画在那里的任何像素——表现就是「窗口透视」，
/// 关掉详细日志弹窗后主界面空白处还留着弹窗的文字。
/// 写法与 `native_gui.rs` 既有窗口类一致（小整数当 hbrBackground 传，系统自动取画刷）。
const CLASS_BACKGROUND_BRUSH: Hwnd = 6usize as Hwnd;
const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
const FW_BOLD: i32 = 700;
const CLEARTYPE_QUALITY: u32 = 5;

/// 与窗口线程共享的进度状态：当前活动日志路径（主线程切换日志时更新）。
pub struct ProgressShared {
    pub log_path: Mutex<PathBuf>,
    pub log_offset: Mutex<u64>,
    pub window_up: AtomicBool,
    // 窗口尚未创建时收到关闭请求也要记住，避免快速失败留下孤立进度窗。
    close_requested: AtomicBool,
    window_thread: Mutex<Option<thread::JoinHandle<()>>>,
    /// 标记是否已对齐到 STEP 1/4 的开始时间
    pub start_time_aligned: AtomicBool,
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
    /// 缓存上一次从日志解析出的最新阶段名，保证无新日志写入时依然能正常显示
    pub cached_stage: Mutex<Option<String>>,
    /// 缓存上一次从日志解析出的当前阶段百分比
    pub cached_percent: Mutex<Option<u32>>,
    /// 缓存最近的详情日志行
    pub cached_details: Mutex<Vec<String>>,
    /// 详细日志弹窗句柄（窗口线程内创建，关闭只隐藏以便重复打开）。
    /// 与 `hwnd` 同样用 usize 存，避免裸指针不满足 Send。
    pub log_window: Mutex<Option<usize>>,
}

impl ProgressShared {
    /// 切换监控的日志文件路径，并指定起始偏移（例如 0，或者累积日志的追加点）。
    pub fn switch_log_path(&self, new_path: PathBuf, initial_offset: u64) {
        let mut path_guard = self.log_path.lock().unwrap();
        *path_guard = new_path;
        let mut offset_guard = self.log_offset.lock().unwrap();
        *offset_guard = initial_offset;
    }

    /// 将已用时间的计时起点重置为当前时刻（在写入 STEP 1/4 时对齐）。
    pub fn reset_start_time(&self) {
        self.start_time_aligned.store(true, Ordering::SeqCst);
        let mut guard = self.start_time.lock().unwrap();
        *guard = std::time::Instant::now();
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
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn MoveWindow(hwnd: Hwnd, x: i32, y: i32, width: i32, height: i32, repaint: i32) -> i32;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    fn IsWindowVisible(hwnd: Hwnd) -> i32;
    fn IsDialogMessageW(hwnd: Hwnd, message: *const Msg) -> i32;
    fn GetParent(hwnd: Hwnd) -> Hwnd;
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: i32) -> i32;
}

/// Win32 `RECT`：`GetClientRect` 用它回填客户区尺寸，驱动最大化后的控件重排。
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
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

/// 读取日志增量新行并更新缓存状态与步骤跟踪器。
fn read_log_delta(shared: &ProgressShared) {
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
        return; // 无新内容，保留已有缓存
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
    let mut new_details: Vec<String> = Vec::new();
    // DISM 进度条用 \r 原地刷新（重定向到文件时不带 \n），先把整段按
    // \r/\n 都拆成行再分类，否则整段会合并成一行、百分比永远取到第一个。
    let lines: Vec<&str> = buffer.split(['\n', '\r']).collect();
    for line in &lines {
        if crate::text_parsing::parse_step_marker(line).is_some() {
            // 新步骤不能继承上一条 DISM 命令的 100%。
            latest_percent = None;
            *shared.cached_percent.lock().unwrap() = None;
        }
        // 当日志中首次出现 STEP 1/ 步骤时，将计时起点精确重置为当前时刻，确保已用时间与步骤时间戳分秒对齐
        if line.contains("STEP 1/") && !shared.start_time_aligned.swap(true, Ordering::SeqCst) {
            let mut guard = shared.start_time.lock().unwrap();
            *guard = std::time::Instant::now();
        }
        let (stage, percent, detail) = classify_log_line(line);
        if let Some(value) = stage {
            latest_stage = Some(value);
        }
        if let Some(value) = percent {
            latest_percent = Some(value);
        }
        if let Some(value) = detail {
            new_details.push(value);
        }
    }
    if let Some(stage) = latest_stage {
        *shared.cached_stage.lock().unwrap() = Some(stage);
    }
    if let Some(percent) = latest_percent {
        *shared.cached_percent.lock().unwrap() = Some(percent);
    }
    if !new_details.is_empty() {
        let mut details = shared.cached_details.lock().unwrap();
        details.extend(new_details);
        while details.len() > TAIL_LINES {
            details.remove(0);
        }
    }
    // 编号步骤跨增量累积，并算出进度。
    let mut tracker = shared.steps.lock().unwrap();
    tracker.feed(&lines);
    if let Some(percent) = latest_percent {
        tracker.set_current_percent(Some(percent));
    }
}

/// 刷新进度窗口控件：包含时间跳动、呼吸小箭头动画与最新日志展示。
/// 关键改进：即使无新增日志字节，本函数也会每秒完整驱动时钟跳动与呼吸动画，绝不假死。
fn refresh_from_log(shared: &ProgressShared, hwnd: Hwnd) {
    // 1. 读取日志增量新行并更新缓存
    read_log_delta(shared);

    // 2. 提取最新状态（无论是否有新日志，都确保每秒驱动 UI 刷新与呼吸动画）
    let latest_stage = shared.cached_stage.lock().unwrap().clone();
    let latest_percent = *shared.cached_percent.lock().unwrap();
    let details = shared.cached_details.lock().unwrap().clone();
    let progress = shared.steps.lock().unwrap().snapshot();

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
        let pos = if latest_stage.as_deref() == Some("操作完成") {
            100
        } else if progress.current.is_some() {
            progress.current_percent.unwrap_or(0)
        } else {
            latest_percent.unwrap_or(0)
        };
        SendMessageW(GetDlgItem(hwnd, ID_BAR), PBM_SETPOS, pos as usize, 0);

        // 标题右侧增加一块显示已用时间/剩余时间、当前时间，靠右显示。
        // 关键：每秒钟调用一次，当前时间精确跳动！
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
        let time_text =
            format!("已用: {elapsed_str}  剩余: {remaining_str}\r\n当前时间: {now_str}");
        let time_wide = encode(&time_text);
        SetWindowTextW(GetDlgItem(hwnd, ID_TIME_INFO), time_wide.as_ptr());

        // 详情区预填 4 阶段骨架清单，再跟日志尾部。清单用 ✓/▶/· 标出
        // 已完成/进行中/未开始；正文每个已开始阶段后面附上时间戳（时分秒）[HH:MM:SS]。
        let online = shared
            .operation
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|s| s.starts_with("ONLINE_"));
        let is_backup = {
            let op_guard = shared.operation.lock().unwrap();
            op_guard
                .as_deref()
                .map(|s| {
                    s.eq_ignore_ascii_case("BACKUP") || s.eq_ignore_ascii_case("ONLINE_BACKUP")
                })
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
            (
                4,
                if online {
                    "发布副档/完成在线操作"
                } else {
                    "清理re启动项/配置"
                },
            ),
        ];

        let total_steps = progress.total.unwrap_or(4);
        let current = progress.current.unwrap_or(0);
        // 最后一步出现 100% 也不能代替终态落盘证据。
        let all_done = latest_stage.as_deref() == Some("操作完成");

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
                if elapsed.is_multiple_of(2) {
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
        if !body.is_empty() {
            let text = body.join("\r\n");
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            SetWindowTextW(GetDlgItem(hwnd, ID_DETAIL), wide.as_ptr());
        }

        // 日志尾部独立成区：固定显示最后 TAIL_LINES 行，不必点开弹窗就能看出
        // 日志是否还在推进（判断"是否卡住"的第一手依据）。不足 3 行时用空行占位，
        // 保证控件高度与排版稳定。
        let mut tail: Vec<String> = details
            .iter()
            .rev()
            .take(TAIL_LINES)
            .rev()
            .cloned()
            .collect();
        while tail.len() < TAIL_LINES {
            tail.insert(0, String::new());
        }
        let tail_text = tail.join("\r\n");
        let tail_handle = GetDlgItem(hwnd, ID_TAIL);
        SetWindowTextW(tail_handle, encode(&tail_text).as_ptr());
        // 开启自动换行后，3 条日志可能折成更多视觉行；滚到末尾保证最新一行可见。
        if !tail_handle.is_null() {
            let end = tail_text.encode_utf16().count();
            SendMessageW(tail_handle, EM_SETSEL, end, end as isize);
            SendMessageW(tail_handle, EM_SCROLLCARET, 0, 0);
        }
    }

    // 详细日志弹窗可见时同步刷新，保证弹窗内容跟着日志走。
    refresh_log_window(shared);
}

/// 读取当前活动日志的尾部原文（最多 `LOG_VIEW_BYTES` 字节、`LOG_VIEW_LINES` 行）。
///
/// 与主界面的分类摘要不同，这里返回**未经筛选的原始日志**（含 DISM 百分比刷新行），
/// 所以能看出进程到底停在哪一行。DISM 用 `\r` 原地刷新且重定向到文件时不带 `\n`，
/// 必须把 `\r` 也当换行，否则整段会挤成一行。
fn read_log_tail_text(shared: &ProgressShared) -> Option<String> {
    let path = shared.log_path.lock().unwrap().clone();
    let mut file = OpenOptions::new().read(true).open(&path).ok()?;
    let size = file.metadata().ok()?.len();
    let start = size.saturating_sub(LOG_VIEW_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).ok()?;
    let decoded = crate::text_parsing::decode_windows_bytes(&raw);
    let lines: Vec<&str> = decoded
        .split(['\r', '\n'])
        .filter(|line| !line.trim().is_empty())
        .collect();
    let shown = if lines.len() > LOG_VIEW_LINES {
        &lines[lines.len() - LOG_VIEW_LINES..]
    } else {
        &lines[..]
    };
    let mut text = String::new();
    if start > 0 || lines.len() > LOG_VIEW_LINES {
        text.push_str(&format!(
            "（已省略更早内容，仅显示最近 {} 行；完整日志文件：{}）\r\n\r\n",
            shown.len(),
            path.display()
        ));
    } else {
        text.push_str(&format!("（日志文件：{}）\r\n\r\n", path.display()));
    }
    text.push_str(&shown.join("\r\n"));
    Some(text)
}

/// 刷新详细日志弹窗内容；窗口不存在或已隐藏时直接跳过，避免无谓开销。
fn refresh_log_window(shared: &ProgressShared) {
    let handle = *shared.log_window.lock().unwrap();
    let Some(raw) = handle else { return };
    if raw == 0 {
        return;
    }
    let window = raw as Hwnd;
    unsafe {
        if IsWindowVisible(window) == 0 {
            return;
        }
        let Some(text) = read_log_tail_text(shared) else {
            return;
        };
        let edit = GetDlgItem(window, ID_LOG_EDIT);
        if edit.is_null() {
            return;
        }
        SetWindowTextW(edit, encode(&text).as_ptr());
        // 滚到末尾，让最新一行始终可见——"是否卡住"看的就是最后一行还在不在变。
        let end = text.encode_utf16().count();
        SendMessageW(edit, EM_SETSEL, end, end as isize);
        SendMessageW(edit, EM_SCROLLCARET, 0, 0);
    }
}

/// 按当前客户区尺寸重排全部控件。
///
/// 必须有这个函数，最大化/拖拽改变大小才真正可用：控件原先是硬编码坐标，
/// 窗口放大后内容仍挤在左上角。WM_CREATE 与 WM_SIZE 都调用它。
unsafe fn layout_controls(hwnd: Hwnd) {
    const MARGIN: i32 = 32;
    const BUTTON_WIDTH: i32 = 148;
    const GAP: i32 = 12;
    let mut rect = Rect::default();
    unsafe {
        if GetClientRect(hwnd, &mut rect) == 0 {
            return;
        }
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 2 * MARGIN || height <= 260 {
        return; // 最小化或尺寸异常时不动布局
    }
    let content = width - 2 * MARGIN;
    // 顶部：阶段标题占左侧 55%，时间信息靠右占其余宽度。
    let stage_width = (content * 55 / 100).max(240);
    let time_width = content - stage_width - GAP;
    // 日志尾部固定 3 行：按详情字体行高 26px 估算，加上边框留白。
    let tail_height = 26 * TAIL_LINES as i32 + 16;
    let bar_top = 92;
    let detail_top = 144;
    let tail_top = height - MARGIN - tail_height;
    let detail_height = (tail_top - GAP - detail_top).max(80);
    let tail_width = content - BUTTON_WIDTH - GAP;
    unsafe {
        MoveWindow(GetDlgItem(hwnd, ID_STAGE), MARGIN, 28, stage_width, 48, 1);
        MoveWindow(
            GetDlgItem(hwnd, ID_TIME_INFO),
            MARGIN + stage_width + GAP,
            28,
            time_width.max(160),
            48,
            1,
        );
        MoveWindow(GetDlgItem(hwnd, ID_BAR), MARGIN, bar_top, content, 36, 1);
        MoveWindow(
            GetDlgItem(hwnd, ID_DETAIL),
            MARGIN,
            detail_top,
            content,
            detail_height,
            1,
        );
        MoveWindow(
            GetDlgItem(hwnd, ID_TAIL),
            MARGIN,
            tail_top,
            tail_width.max(200),
            tail_height,
            1,
        );
        // 按钮与日志尾部同一行、贴在其右侧（用户指定位置）。
        MoveWindow(
            GetDlgItem(hwnd, ID_LOG_BUTTON),
            MARGIN + tail_width.max(200) + GAP,
            tail_top,
            BUTTON_WIDTH,
            tail_height,
            1,
        );
    }
}

/// 打开（或重新激活）详细日志弹窗。
///
/// 关闭只隐藏不销毁，这样反复开关不会重复建窗；弹窗自身带最大化按钮和
/// 双向滚动条，内容由 `refresh_log_window` 每秒跟随日志刷新。
unsafe fn open_log_window(main_hwnd: Hwnd, shared: &ProgressShared) {
    let existing = *shared.log_window.lock().unwrap();
    if let Some(raw) = existing
        && raw != 0
    {
        let window = raw as Hwnd;
        unsafe {
            ShowWindow(window, SW_RESTORE);
            ShowWindow(window, SW_SHOW);
            SetForegroundWindow(window);
        }
        refresh_log_window(shared);
        return;
    }
    let class_name = encode("BackupRestoreProgressLogClass");
    let class = WndClassExW {
        cb_size: std::mem::size_of::<WndClassExW>() as u32,
        style: 0,
        wnd_proc: Some(log_window_proc),
        cb_cls_extra: 0,
        cb_wnd_extra: 0,
        instance: unsafe { GetModuleHandleW(null()) },
        icon: null_mut(),
        cursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        brush: CLASS_BACKGROUND_BRUSH,
        menu_name: null(),
        class_name: class_name.as_ptr(),
        icon_sm: null_mut(),
    };
    static LOG_REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*LOG_REGISTERED.get_or_init(|| unsafe { RegisterClassExW(&class) } != 0) {
        return; // 注册失败时弹窗非关键，主界面仍有 3 行尾部可看
    }
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            encode("BackupRestore 详细日志").as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1000,
            720,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null_mut(),
        )
    };
    if window.is_null() {
        return;
    }
    unsafe {
        // 弹窗通过 GWL_USERDATA 记住主窗口，刷新时再从主窗口取共享状态，
        // 避免第二份 Arc 裸指针带来的生命周期问题。
        SetWindowLongPtrW(window, GWL_USERDATA, main_hwnd as isize);
    }
    *shared.log_window.lock().unwrap() = Some(window as usize);
    unsafe {
        ShowWindow(window, SW_SHOW);
        UpdateWindow(window);
    }
    refresh_log_window(shared);
}

/// 详细日志弹窗的窗口过程。关闭时只隐藏，绝不 `PostQuitMessage`——
/// 它与主进度窗口共用同一条消息循环，退出消息循环会连带关掉进度窗口。
unsafe extern "system" fn log_window_proc(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    match message {
        WM_CREATE => unsafe {
            let font = create_font(-17, false);
            let edit = CreateWindowExW(
                0,
                encode("Edit").as_ptr(),
                null(),
                // 自动换行：只保留纵向滚动条，长行折行显示而不是横向滚动。
                WS_CHILD
                    | WS_VISIBLE
                    | WS_BORDER
                    | WS_VSCROLL
                    | ES_MULTILINE
                    | ES_AUTOVSCROLL
                    | ES_READONLY,
                12,
                12,
                960,
                660,
                hwnd,
                ID_LOG_EDIT as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !edit.is_null() {
                SendMessageW(edit, WM_SETFONT, font as usize, 1);
            }
            SetTimer(hwnd, LOG_TIMER_ID, 1000, None);
            0
        },
        WM_SIZE => unsafe {
            let mut rect = Rect::default();
            if GetClientRect(hwnd, &mut rect) != 0 {
                let width = (rect.right - rect.left - 24).max(80);
                let height = (rect.bottom - rect.top - 24).max(80);
                MoveWindow(GetDlgItem(hwnd, ID_LOG_EDIT), 12, 12, width, height, 1);
            }
            0
        },
        WM_TIMER => unsafe {
            let main = GetWindowLongPtrW(hwnd, GWL_USERDATA) as Hwnd;
            if !main.is_null() {
                let shared_ptr = GetWindowLongPtrW(main, GWL_USERDATA) as *const ProgressShared;
                if !shared_ptr.is_null() {
                    refresh_log_window(&*shared_ptr);
                }
            }
            0
        },
        WM_CLOSE => unsafe {
            ShowWindow(hwnd, SW_HIDE);
            // 隐藏后主动让主进度窗口重绘：弹窗盖过的区域需要立刻擦除重画。
            let main = GetWindowLongPtrW(hwnd, GWL_USERDATA) as Hwnd;
            if !main.is_null() {
                InvalidateRect(main, null(), 1);
                UpdateWindow(main);
            }
            0
        },
        WM_DESTROY => unsafe {
            KillTimer(hwnd, LOG_TIMER_ID);
            0
        },
        _ => unsafe { DefWindowProcW(hwnd, message, w_param, l_param) },
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
            // 日志尾部：常驻显示最后 3 行原始日志，不点开弹窗也能判断是否卡住。
            let tail = CreateWindowExW(
                0,
                encode("Edit").as_ptr(),
                null(),
                // 自动换行（不加 ES_AUTOHSCROLL）：长日志行折到下一行而不是横向截断。
                // 折行会让 3 条日志占到 3 行以上，所以每次刷新后把控件滚到末尾
                // （见 refresh_from_log），保证最新一行始终可见。
                WS_CHILD | WS_VISIBLE | WS_BORDER | ES_MULTILINE | ES_READONLY,
                32,
                0,
                640,
                94,
                hwnd,
                ID_TAIL as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !tail.is_null() {
                SendMessageW(tail, WM_SETFONT, create_font(-16, false) as usize, 1);
            }
            // 「详细日志」按钮：位于日志尾部右侧（用户指定）。
            let button = CreateWindowExW(
                0,
                encode("Button").as_ptr(),
                encode("详细日志(F3)").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                148,
                94,
                hwnd,
                ID_LOG_BUTTON as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !button.is_null() {
                SendMessageW(button, WM_SETFONT, create_font(-19, true) as usize, 1);
            }
            layout_controls(hwnd);
            SetTimer(hwnd, TIMER_ID, 1000, None);
            0
        },
        // 最大化/还原/拖拽改变大小后重排控件，否则放大后内容仍挤在左上角。
        WM_SIZE => unsafe {
            layout_controls(hwnd);
            0
        },
        WM_COMMAND => unsafe {
            if (w_param & 0xFFFF) as i32 == ID_LOG_BUTTON {
                let shared_ptr = GetWindowLongPtrW(hwnd, GWL_USERDATA) as *const ProgressShared;
                if !shared_ptr.is_null() {
                    open_log_window(hwnd, &*shared_ptr);
                }
            }
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
            // 先销毁详细日志弹窗：它的 GWL_USERDATA 指向本窗口，主窗口销毁后
            // 再刷新会读到悬垂句柄。
            let shared_ptr = GetWindowLongPtrW(hwnd, GWL_USERDATA) as *const ProgressShared;
            if !shared_ptr.is_null() {
                let taken = (*shared_ptr).log_window.lock().unwrap().take();
                if let Some(raw) = taken
                    && raw != 0
                {
                    DestroyWindow(raw as Hwnd);
                }
            }
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
        cached_stage: Mutex::new(None),
        cached_percent: Mutex::new(None),
        cached_details: Mutex::new(Vec::new()),
        window_up: AtomicBool::new(false),
        close_requested: AtomicBool::new(false),
        window_thread: Mutex::new(None),
        start_time_aligned: AtomicBool::new(false),
        start_time: Mutex::new(std::time::Instant::now()),
        hwnd: Mutex::new(None),
        operation: Mutex::new(operation.map(|s| s.to_string())),
        log_window: Mutex::new(None),
    });
    let thread_shared = Arc::clone(&shared);
    let handle = thread::spawn(move || {
        // 窗口线程：创建 Win32 进度窗口并进入消息循环。
        unsafe { run_window(&thread_shared) };
    });
    *shared.window_thread.lock().unwrap() = Some(handle);
    shared
}

/// 等旧窗口彻底退出，再显示重试窗口，避免异步销毁抢走新窗口焦点。
pub fn close_and_wait(shared: &ProgressShared) {
    request_close(shared);
    let handle = shared.window_thread.lock().unwrap().take();
    if let Some(handle) = handle {
        let _ = handle.join();
    }
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
        brush: CLASS_BACKGROUND_BRUSH,
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
        static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*REGISTERED.get_or_init(|| RegisterClassExW(&class) != 0) {
            return; // 注册失败时进度窗口非关键，静默降级。
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
            // Msg 不是 Copy，按字段读取而不是整体复制。
            let msg_hwnd = (*msg.as_ptr()).hwnd;
            let msg_message = (*msg.as_ptr()).message;
            let msg_wparam = (*msg.as_ptr()).w_param;
            // 键盘通路：WinRE 下鼠标未必可用，所以 F3 全局打开详细日志、
            // Esc 关闭弹窗，都在消息循环里拦截——焦点落在哪个子控件上都有效。
            if msg_message == WM_KEYDOWN {
                if msg_wparam == VK_F3 {
                    open_log_window(hwnd, shared);
                    continue;
                }
                if msg_wparam == VK_ESCAPE {
                    let log = *shared.log_window.lock().unwrap();
                    if let Some(raw) = log
                        && raw != 0
                    {
                        let window = raw as Hwnd;
                        // 焦点在弹窗自身或其子控件上时，Esc 才收起弹窗。
                        if msg_hwnd == window || GetParent(msg_hwnd) == window {
                            ShowWindow(window, SW_HIDE);
                            // 弹窗让出屏幕后立即重绘主界面，不等下一次自然刷新。
                            InvalidateRect(hwnd, null(), 1);
                            UpdateWindow(hwnd);
                            continue;
                        }
                    }
                }
            }
            // IsDialogMessageW 提供 Tab/Shift+Tab/空格/回车的控件导航，
            // 否则带 WS_TABSTOP 的「详细日志」按钮在纯键盘环境下根本无法聚焦。
            let target = if msg_hwnd == hwnd {
                hwnd
            } else {
                let parent = GetParent(msg_hwnd);
                if parent.is_null() { msg_hwnd } else { parent }
            };
            if IsDialogMessageW(target, msg.as_ptr()) != 0 {
                continue;
            }
            TranslateMessage(msg.as_ptr());
            DispatchMessageW(msg.as_ptr());
        }
    }
    *shared.hwnd.lock().unwrap() = None;
    shared.window_up.store(false, Ordering::SeqCst);
    // 释放 GWL_USERDATA 持有的 Arc（窗口销毁后）
    let _ = unsafe { Arc::from_raw(raw as *const ProgressShared) };
}
