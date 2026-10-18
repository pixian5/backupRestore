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
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::native_gui::Msg;
use crate::text_parsing::classify_log_line;

use backuprestore_core::TaskError;

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
/// 「引导修复」按钮：第二阶段开始后打开系统选择与排序窗口。
const ID_BOOT_MENU: i32 = 8;
/// 「打开命令提示符」按钮：RE 内现场运行命令修复（bcdedit/dism 等）。
/// 2026-10-14 用户需求：恢复环境出问题时可直接开 cmd，不必再从宿主侧逐键注入。
const ID_CMD_BUTTON: i32 = 9;
/// 「暂停/继续」按钮（ID 10）：用户按下后恢复链在下一个步骤边界挂起，
/// 再按一次继续。2026-10-16 用户需求——RE 里必须能停住程序现场处理。
const ID_PAUSE_BUTTON: i32 = 10;
/// 「BCD 回退」按钮（ID 11）：把 ESP 上的 BCD 恢复成任务准备期保存的快照
/// （`bcd-before-raw` / `bcd-before-export`），让用户在恢复链改坏引导后能一键退回。
/// 2026-10-16 用户需求——此前只能从宿主侧挂 PE 光盘手改，终端用户无法自救。
const ID_BCD_ROLLBACK_BUTTON: i32 = 11;
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
const EM_GETFIRSTVISIBLELINE: u32 = 0x00CE;
const EM_LINESCROLL: u32 = 0x00B6;
const SB_VERT: i32 = 1;
const SIF_ALL: u32 = 0x0017;
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
/// L：无论焦点在哪个控件都能打开详细日志弹窗。
/// WinRE 里鼠标并非总是可用，必须留一条纯键盘通路。
const VK_L: usize = 0x4C;
/// K：全局打开命令提示符（与 L 同机制；WinRE 下鼠标未必可用）。
/// 代价与 L 相同：焦点在文本控件里时无法输入字母 K——恢复窗口本身没有
/// 可编辑文本，只影响详细日志弹窗/引导修复编辑器内的输入，属可接受。
const VK_K: usize = 0x4B;
/// P：全局切换暂停/继续（与 L/K 同机制；WinRE 下鼠标未必可用）。
const VK_P: usize = 0x50;
/// B：全局触发 BCD 回退（与 L/K/P 同机制）。
///
/// 【2026-10-18 缺陷修复】「BCD 回退」按钮的文案从加上第一天起就写着
/// 「BCD 回退(B)」，但消息循环里从来没有 VK_B 分支——按 B 没有任何反应，
/// 属「文案承诺了快捷键却不实现」的缺陷。本常量与消息循环里的分支一并补上。
const VK_B: usize = 0x42;
/// M：全局打开引导修复编辑器（与 L/K/P/B 同机制；M = Menu）。
///
/// 【2026-10-18 用户要求「把这些按钮都加上快捷键」】此前五个按钮里只有
/// 引导修复没有对应键位。字母表占用：L 日志 / K 命令行 / P 暂停 /
/// B BCD 回退 / M 引导修复，互不冲突，也与引导修复编辑器自身的
/// Esc/回车/空格/F2/Shift+方向键不撞（见 boot_menu_window::handle_key）。
const VK_M: usize = 0x4D;
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

/// 暂停总上限（秒）。0 = 不限制（**默认**）。
///
/// 默认不限制是**有意的**：把默认设成某个固定秒数，会把「有人值守但看得久」
/// 的正常用法也中断，等于用一个误杀换掉一个挂死。真正的逃生通道是
/// **关闭进度窗口**——那 unmistakably 表示放弃（见 `pause_abandoned`）。
/// 这个上限只作为第二道保险，留给运维通过环境变量打开。
fn pause_total_timeout() -> u64 {
    // 【2026-10-17 修复】键名取自 `failure_policy` 的单一来源常量。此前这里是
    // 一枚裸字符串字面量，而全仓**没有任何写入方**设置它 → 永久 0 → 用户在设置
    // 里配的 `failure_pause_max_wait_secs` 对"暂停后没人来点继续"这一路不生效。
    // 写入方现位于 `recover_env`（把策略值灌进来）。
    std::env::var(crate::failure_policy::PAUSE_TIMEOUT_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

/// 与窗口线程共享的进度状态：当前活动日志路径（主线程切换日志时更新）。
/// 「BCD 回退」按钮的动作类型：返回 Ok(()) 表示已回退，Err(说明) 记录失败原因。
/// 抽成别名是为了避免 clippy 的 type_complexity 告警，也让字段声明更易读。
type BcdRollbackAction = Box<dyn Fn() -> Result<(), String> + Send>;

pub struct ProgressShared {
    pub log_path: Mutex<PathBuf>,
    pub log_offset: Mutex<u64>,
    pub window_up: AtomicBool,
    // 窗口尚未创建时收到关闭请求也要记住，避免快速失败留下孤立进度窗。
    close_requested: AtomicBool,
    window_thread: Mutex<Option<thread::JoinHandle<()>>>,
    /// 【2026-10-17 第二轮核查，卡死 5】窗口线程的「我结束了」信号。
    /// `JoinHandle` 没有带超时的 join：此前 `close_and_wait` 是裸 `join()`，
    /// 窗口线程一旦卡在 `refresh_from_log`（日志增量巨大时一次读全量）或
    /// BCD 回退闭包（持锁跑 bcdedit），恢复链就**永久阻塞在这里**，连失败重试
    /// 弹框都到不了 —— 上面所有逃生阀都在它下游。现在先等信号（带超时），
    /// 只有确认线程真的结束了才 join；超时就放弃 join 并如实记一行。
    window_done_tx: std::sync::mpsc::Sender<()>,
    window_done_rx: Mutex<std::sync::mpsc::Receiver<()>>,
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
    /// 第二阶段准备好的引导修复会话；按钮在会话存在后显示。
    pub boot_menu: Mutex<Option<Arc<crate::boot_menu::BootMenuSession>>>,
    /// 用户在恢复界面点了「暂停」时置 true。恢复链在每个步骤边界检查它，
    /// 为 true 则原地等待（不推进、不退出），让用户能停下来查看/处理。
    /// 用 AtomicBool 而不是 Mutex：检查非常频繁，且只需要一个布尔语义。
    pub paused: AtomicBool,
    /// 【2026-10-16 审查修订，卡死 4】暂停期间用户关掉了进度窗口。
    ///
    /// 为什么必须有：主进度窗口此前**没有 `WM_CLOSE` 分支**（落到
    /// `DefWindowProcW` → `DestroyWindow`），而 `WM_DESTROY` 只 `KillTimer` +
    /// 销毁日志弹窗 + `PostQuitMessage`，**从不通知恢复线程**。于是
    /// 「暂停 → 关窗口」之后：进程还活着、恢复线程在 `wait_if_paused` 里
    /// 无超时自旋、界面上什么都没有、也没有任何出口。杀进程之后
    /// `bootsequence` 仍武装，重启又回 RE 重来。
    ///
    /// 窗口过程在**用户发起**的 `WM_CLOSE`（区别于程序自己 `request_close`
    /// 发的那次）里置这个标志；`wait_if_paused` 看到它就返回 `Err` 中断恢复链，
    /// 由上层既有的失败处理（含 auto-escape）接管——机器因此能逃出去。
    pub pause_abandoned: AtomicBool,
    /// 「BCD 回退」按钮的动作。恢复主流程在拿到任务目录/ESP 身份后，把回退闭包
    /// 装到这里；按钮点击时调用它（回退需要 bcdedit + 卷挂载，只有主流程才有
    /// 这些上下文）。未装载时按钮点击只写一条日志。
    pub bcd_rollback: Mutex<Option<BcdRollbackAction>>,
}

impl ProgressShared {
    /// 切换监控的日志文件路径，并指定起始偏移（例如 0，或者累积日志的追加点）。
    pub fn switch_log_path(&self, new_path: PathBuf, initial_offset: u64) {
        let mut path_guard = self.log_path.lock().unwrap();
        *path_guard = new_path;
        let mut offset_guard = self.log_offset.lock().unwrap();
        *offset_guard = initial_offset;
    }

    /// 在第二阶段候选探测完成后发布引导修复会话，进度窗会显示按钮。
    pub fn set_boot_menu(&self, session: Arc<crate::boot_menu::BootMenuSession>) {
        *self.boot_menu.lock().unwrap() = Some(session);
    }

    /// 恢复链在**每个步骤边界**调用：若用户点了「暂停」，就在这里原地等待，
    /// 阻塞恢复推进，直到用户再点一次「继续」。
    ///
    /// 为什么放在恢复链里而不是窗口线程里：真正在干活的是恢复线程，只有让它
    /// 自己挂起才能"停住"；窗口线程只能改状态。等待期间**不关窗口、不退出**，
    /// 用户还能用「详细日志」「命令提示符」「引导修复」等按钮现场处理。
    ///
    /// 参数 `stage` 只用于写一条可读日志（"已暂停于 X"），不参与逻辑。
    ///
    /// 【2026-10-16 审查修订，卡死 4】返回 `Err` 的两种情况：
    /// ① 暂停期间用户关掉了进度窗口（`pause_abandoned`）—— unmistakably
    ///    表示放弃，必须中断而不是继续干一件用户已经看不见的事。
    /// ② 暂停总时长超过 `pause_total_timeout()`（默认 0 = 不限制）。
    /// 中断由上层既有的失败处理接管（保留现场；`auto-escape` 时会 disarm +
    /// 标终态 + 重启回 Windows），所以机器不会留在恢复环境里打转。
    pub fn wait_if_paused(&self, log: &Path, stage: &str) -> Result<(), TaskError> {
        if !self.paused.load(Ordering::SeqCst) {
            return Ok(());
        }
        let _ = crate::append_log(log, &format!("恢复已暂停（用户请求），停在：{stage}"));
        let limit_secs = pause_total_timeout();
        let deadline = (limit_secs > 0)
            .then(|| std::time::Instant::now() + std::time::Duration::from_secs(limit_secs));
        if limit_secs > 0 {
            let _ = crate::append_log(
                log,
                &format!("暂停总上限 {limit_secs} 秒；超时将中断恢复链并按失败处理"),
            );
        }
        // 每 200ms 醒来一次，纯自旋等待；暂停是人手操作，不需要更精细。
        while self.paused.load(Ordering::SeqCst) {
            if self.pause_abandoned.load(Ordering::SeqCst) {
                let _ = crate::append_log(
                    log,
                    "进度窗口已关闭且当时处于暂停：中断恢复链，按失败处理（现场保留）",
                );
                return Err(crate::err("进度窗口已关闭，恢复已中断"));
            }
            if let Some(deadline) = deadline
                && std::time::Instant::now() >= deadline
            {
                let _ = crate::append_log(
                    log,
                    &format!(
                        "暂停已达总上限 {limit_secs} 秒：中断恢复链，按失败处理（现场保留）"
                    ),
                );
                return Err(crate::err("暂停等待超时，恢复已中断"));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let _ = crate::append_log(log, "恢复已继续（用户请求）");
        Ok(())
    }

    /// 切换暂停状态，返回切换后的值（true=已暂停）。
    pub fn toggle_paused(&self) -> bool {
        let now = !self.paused.load(Ordering::SeqCst);
        self.paused.store(now, Ordering::SeqCst);
        now
    }

    /// 装载「BCD 回退」动作。恢复主流程在拿到任务目录与 ESP 身份后调用；
    /// 按钮点击时执行该闭包。重复调用会替换旧闭包。
    pub fn set_bcd_rollback(&self, action: BcdRollbackAction) {
        *self.bcd_rollback.lock().unwrap() = Some(action);
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
    fn SetFocus(hwnd: Hwnd) -> Hwnd;
    fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Hwnd;
    fn IsWindowVisible(hwnd: Hwnd) -> i32;
    fn IsDialogMessageW(hwnd: Hwnd, message: *const Msg) -> i32;
    fn GetParent(hwnd: Hwnd) -> Hwnd;
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: i32) -> i32;
    fn GetScrollInfo(hwnd: Hwnd, bar: i32, info: *mut ScrollInfo) -> i32;
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

/// Win32 垂直滚动条状态，用于判断详细日志当前是否已经在底部。
#[repr(C)]
#[derive(Default)]
struct ScrollInfo {
    cb_size: u32,
    f_mask: u32,
    n_min: i32,
    n_max: i32,
    n_page: u32,
    n_pos: i32,
    n_track_pos: i32,
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
        // 只有第二阶段已经完成候选系统探测后才开放引导修复；无会话时隐藏，避免用户误点。
        let boot_menu_ready = shared.boot_menu.lock().unwrap().is_some();
        ShowWindow(
            GetDlgItem(hwnd, ID_BOOT_MENU),
            if boot_menu_ready { SW_SHOW } else { SW_HIDE },
        );
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

/// 判断多行编辑框的垂直滚动条是否已在底部。
///
/// 只有用户没有向上翻阅时才允许刷新后自动滚到底；用户拖到中间/顶部时，
/// 每秒刷新也必须保持他们正在查看的历史位置。
unsafe fn log_edit_is_at_bottom(edit: Hwnd) -> bool {
    let mut info = ScrollInfo {
        cb_size: std::mem::size_of::<ScrollInfo>() as u32,
        f_mask: SIF_ALL,
        ..Default::default()
    };
    if unsafe { GetScrollInfo(edit, SB_VERT, &mut info) } == 0 {
        return true;
    }
    info.n_page == 0 || info.n_pos.saturating_add(info.n_page as i32) >= info.n_max
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
        // 先记录用户当前滚动位置。只有本来就在底部时才跟随新日志滚动；
        // 用户向上拖动查看历史后，刷新不能把他们强行拽回末尾。
        let was_at_bottom = log_edit_is_at_bottom(edit);
        let old_first_line = SendMessageW(edit, EM_GETFIRSTVISIBLELINE, 0, 0) as i32;
        SetWindowTextW(edit, encode(&text).as_ptr());
        if was_at_bottom {
            let end = text.encode_utf16().count();
            SendMessageW(edit, EM_SETSEL, end, end as isize);
            SendMessageW(edit, EM_SCROLLCARET, 0, 0);
        } else {
            // SetWindowText 可能重置首行，恢复到用户拖动前的首行。
            let new_first_line = SendMessageW(edit, EM_GETFIRSTVISIBLELINE, 0, 0) as i32;
            SendMessageW(
                edit,
                EM_LINESCROLL,
                0,
                (old_first_line - new_first_line) as isize,
            );
        }
    }
}

/// 布局：四边留白。
const MARGIN: i32 = 32;
/// 按钮宽度。
///
/// 【2026-10-18 用户要求「尺寸调小一些」】原为 120，有两个连带缺陷：
/// ① 默认窗宽（860）下 `tail_width` 只有 ~136px，代码用 `max(200)` 夹紧后
///    与按钮坐标的预算不一致，第五个按钮（BCD 回退）右缘冲出客户区；
///    ② 日志尾部被五个 120 宽的按钮挤到只剩 136px，3 行日志几乎没法看。
/// 取 108：`5*BUTTON_WIDTH + 5*GAP + TAIL_MIN_WIDTH + 2*MARGIN ≈ 824`，
/// 默认客户宽（≈852）下五个按钮恰好排满内容区，尾部仍留有 188px。
const BUTTON_WIDTH: i32 = 108;
/// 按钮高度：独立于日志尾部行高。原与 3 行日志等高（94px）又高又大；
/// 现在 32px，在日志尾部所在行内垂直居中。
const BUTTON_HEIGHT: i32 = 32;
/// 控件间距。
const GAP: i32 = 12;
/// 日志尾部最小宽度：内容区不够时先保按钮整行，尾部最多让到这。
const TAIL_MIN_WIDTH: i32 = 120;

/// 按当前客户区尺寸重排全部控件。
///
/// 必须有这个函数，最大化/拖拽改变大小才真正可用：控件原先是硬编码坐标，
/// 窗口放大后内容仍挤在左上角。WM_CREATE 与 WM_SIZE 都调用它。
unsafe fn layout_controls(hwnd: Hwnd) {
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
    // 按钮排现有五个（详细日志/引导修复/命令提示符/暂停/BCD 回退），
    // 日志尾部让出五份宽度（含尾部与第一个按钮之间的那份）。
    //
    // 【2026-10-18 缺陷修复】tail_width 只算一次、所有 MoveWindow 复用同一个
    // 值：此前绘制尾部时用 `max(200)` 夹紧、按钮坐标却用未夹紧的预算值，
    // 两者不一致会把最后一个按钮推出客户区之外（默认窗宽即复现）。
    let tail_width = (content - 5 * BUTTON_WIDTH - 5 * GAP).max(TAIL_MIN_WIDTH);
    // 按钮在日志尾部行内垂直居中；空间被极端压缩时 x 夹紧在内容区内，
    // 允许互相重叠也不允许冲出窗外（默认尺寸下不会触发，纯兜底）。
    let button_y = tail_top + (tail_height - BUTTON_HEIGHT) / 2;
    let clamp_x = |x: i32| x.clamp(MARGIN, (MARGIN + content - BUTTON_WIDTH).max(MARGIN));
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
            tail_width,
            tail_height,
            1,
        );
        // 按钮与日志尾部同一行、贴在其右侧（用户指定位置）。
        MoveWindow(
            GetDlgItem(hwnd, ID_LOG_BUTTON),
            clamp_x(MARGIN + tail_width + GAP),
            button_y,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            1,
        );
        // 引导修复按钮与详细日志按钮并排，避免挤压日志尾部。
        MoveWindow(
            GetDlgItem(hwnd, ID_BOOT_MENU),
            clamp_x(MARGIN + tail_width + GAP + BUTTON_WIDTH + GAP),
            button_y,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            1,
        );
        // 命令提示符按钮排在按钮行第三位，暂停按钮第四位。
        MoveWindow(
            GetDlgItem(hwnd, ID_CMD_BUTTON),
            clamp_x(MARGIN + tail_width + GAP + 2 * (BUTTON_WIDTH + GAP)),
            button_y,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            1,
        );
        MoveWindow(
            GetDlgItem(hwnd, ID_PAUSE_BUTTON),
            clamp_x(MARGIN + tail_width + GAP + 3 * (BUTTON_WIDTH + GAP)),
            button_y,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            1,
        );
        MoveWindow(
            GetDlgItem(hwnd, ID_BCD_ROLLBACK_BUTTON),
            clamp_x(MARGIN + tail_width + GAP + 4 * (BUTTON_WIDTH + GAP)),
            button_y,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            1,
        );
    }
}

/// 在恢复环境里打开一个交互式命令提示符（cmd.exe）。
///
/// 2026-10-14 用户需求：RE 内出问题（引导劫持/挂起操作等）时能现场运行
/// bcdedit/dism 等命令修复，不必再从宿主侧逐键注入。按钮与 K 快捷键都走这里。
/// 进程独立运行不等待（见 windows_command::spawn_detached_interactive，
/// 不挂作业对象、恢复流程不暂停）；失败只写日志。
unsafe fn open_cmd_prompt(shared: &ProgressShared) {
    if crate::windows_command::spawn_detached_interactive("cmd.exe") {
        let _ = crate::append_log(
            &*shared.log_path.lock().unwrap(),
            "已打开命令提示符（用户请求，恢复流程不受影响）",
        );
    } else {
        let _ = crate::append_log(
            &*shared.log_path.lock().unwrap(),
            "打开命令提示符失败：CreateProcessW 返回 0",
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
            // 把焦点交给日志编辑框，否则纯键盘环境下 PageUp/方向键无处可去。
            SetFocus(GetDlgItem(window, ID_LOG_EDIT));
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
        // 首次打开同样把焦点落到编辑框，保证键盘可以立刻翻阅日志。
        SetFocus(GetDlgItem(window, ID_LOG_EDIT));
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
                // WS_TABSTOP 必须给：WinRE 下可能没有鼠标，编辑框拿不到焦点就
                // 完全无法用 PageUp/方向键滚动日志，底部吸附也就永远用不上。
                WS_CHILD
                    | WS_VISIBLE
                    | WS_BORDER
                    | WS_VSCROLL
                    | WS_TABSTOP
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

/// 切换暂停/继续：改共享标志、翻转按钮文字、写日志。
///
/// WM_COMMAND（点按钮）与消息循环里的全局 P 键共用同一实现，避免两处逻辑漂移。
/// `via_key` 只用于日志措辞（点击 vs 按 P），文案与 2026-10-18 之前逐字一致。
unsafe fn toggle_pause(hwnd: Hwnd, shared: &ProgressShared, via_key: bool) {
    let paused = shared.toggle_paused();
    let label = encode(if paused { "继续(P)" } else { "暂停(P)" });
    unsafe {
        SetWindowTextW(GetDlgItem(hwnd, ID_PAUSE_BUTTON), label.as_ptr());
    }
    let message = match (paused, via_key) {
        (true, true) => "用户按 P 暂停：恢复将在下一个步骤边界挂起",
        (true, false) => "用户点击暂停：恢复将在下一个步骤边界挂起",
        (false, true) => "用户按 P 继续：恢复将立即推进",
        (false, false) => "用户点击继续：恢复将立即推进",
    };
    let _ = crate::append_log(&shared.log_path.lock().unwrap(), message);
}

/// 打开（或激活）引导修复编辑器。WM_COMMAND（点按钮）与全局 M 键共用。
///
/// 【2026-10-18 缺陷修复】已存在编辑器时只激活、不重开：`open_editor` 每次调用
/// 都会建一个新窗口，连点两次按钮（或按两次 M）会叠出多个编辑器，各自持一份
/// 状态，用户改哪一份都不直观。类名与 `boot_menu_window::open_editor` 注册的
/// 一致；窗口类按进程实例隔离，同进程只会命中自家窗口。
unsafe fn open_boot_menu_editor(hwnd: Hwnd, shared: &ProgressShared) {
    let session = shared.boot_menu.lock().unwrap().clone();
    // 会话未就绪（第二阶段候选探测完成前）时静默无操作，与点击行为一致。
    let Some(session) = session else { return };
    let class = encode("BackupRestoreBootMenuClass");
    let existing = unsafe { FindWindowW(class.as_ptr(), null()) };
    if !existing.is_null() {
        unsafe {
            SetForegroundWindow(existing);
        }
        return;
    }
    let editor = unsafe { crate::boot_menu::open_editor(hwnd, session) };
    if editor.is_null() {
        let _ = crate::append_log(
            &shared.log_path.lock().unwrap(),
            "引导修复编辑窗口创建失败",
        );
    }
}

/// 触发 BCD 回退：调用恢复主流程装载的闭包（需要任务目录与 ESP 身份）。
/// WM_COMMAND（点按钮）与全局 B 键共用。
unsafe fn trigger_bcd_rollback(shared: &ProgressShared, via_key: bool) {
    let guard = shared.bcd_rollback.lock().unwrap();
    let log_path = shared.log_path.lock().unwrap().clone();
    let actor = if via_key { "用户按 B 键" } else { "用户点击" };
    match guard.as_ref() {
        Some(action) => match action() {
            Ok(()) => {
                let _ = crate::append_log(
                    &log_path,
                    &format!("{actor} BCD 回退：已恢复准备期 BCD 快照"),
                );
            }
            Err(message) => {
                let _ =
                    crate::append_log(&log_path, &format!("{actor} BCD 回退失败：{message}"));
            }
        },
        None => {
            let _ = crate::append_log(
                &log_path,
                "BCD 回退尚不可用：恢复流程尚未装载任务目录/ESP 身份",
            );
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
            // 「详细日志(L)」按钮：位于日志尾部右侧（用户指定）。
            let button = CreateWindowExW(
                0,
                encode("Button").as_ptr(),
                encode("详细日志(L)").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                BUTTON_WIDTH,
                BUTTON_HEIGHT,
                hwnd,
                ID_LOG_BUTTON as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !button.is_null() {
                SendMessageW(button, WM_SETFONT, create_font(-14, true) as usize, 1);
            }
            // 引导修复(M) 按钮默认隐藏，主线程在 STEP 2 候选会话准备完成后打开。
            let boot_button = CreateWindowExW(
                0,
                encode("Button").as_ptr(),
                encode("引导修复(M)").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                BUTTON_WIDTH,
                BUTTON_HEIGHT,
                hwnd,
                ID_BOOT_MENU as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !boot_button.is_null() {
                SendMessageW(boot_button, WM_SETFONT, create_font(-14, true) as usize, 1);
                ShowWindow(boot_button, SW_HIDE);
            }
            // 「命令提示符(K)」按钮：常显（不依赖任务阶段）——恢复环境任何
            // 时刻都可能需要现场运行命令修复；K 为全局快捷键。
            // 【2026-10-18】文案由「打开命令提示符(K)」缩为「命令提示符(K)」：
            // 按钮按用户要求缩小后，原文案（7 个全角字）在 108px 宽里放不下，
            // 「打开」由点击动作本身表达，日志里仍写全称。
            let cmd_button = CreateWindowExW(
                0,
                encode("Button").as_ptr(),
                encode("命令提示符(K)").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                BUTTON_WIDTH,
                BUTTON_HEIGHT,
                hwnd,
                ID_CMD_BUTTON as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !cmd_button.is_null() {
                SendMessageW(cmd_button, WM_SETFONT, create_font(-14, true) as usize, 1);
            }
            // 「暂停(P)」按钮：常显。恢复链在步骤边界检查 paused 标志并挂起，
            // 让用户能停下来查看日志 / 用命令提示符处理，再按一次继续。
            let pause_button = CreateWindowExW(
                0,
                encode("Button").as_ptr(),
                encode("暂停(P)").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                BUTTON_WIDTH,
                BUTTON_HEIGHT,
                hwnd,
                ID_PAUSE_BUTTON as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !pause_button.is_null() {
                SendMessageW(pause_button, WM_SETFONT, create_font(-14, true) as usize, 1);
            }
            // 「BCD 回退(B)」按钮：常显。把 ESP 上的 BCD 恢复成准备期快照，
            // 让用户在恢复链改坏引导后能一键退回；B 为全局快捷键
            // （2026-10-18 补：此前文案带 (B) 但消息循环没有对应分支）。
            let rollback_button = CreateWindowExW(
                0,
                encode("Button").as_ptr(),
                encode("BCD 回退(B)").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                BUTTON_WIDTH,
                BUTTON_HEIGHT,
                hwnd,
                ID_BCD_ROLLBACK_BUTTON as usize as Hwnd,
                GetModuleHandleW(null()),
                null_mut(),
            );
            if !rollback_button.is_null() {
                SendMessageW(rollback_button, WM_SETFONT, create_font(-14, true) as usize, 1);
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
            let command_id = (w_param & 0xFFFF) as i32;
            let shared_ptr = GetWindowLongPtrW(hwnd, GWL_USERDATA) as *const ProgressShared;
            if !shared_ptr.is_null() {
                let shared = &*shared_ptr;
                if command_id == ID_LOG_BUTTON {
                    open_log_window(hwnd, shared);
                } else if command_id == ID_BOOT_MENU {
                    // 引导修复编辑器：与全局 M 键共用 open_boot_menu_editor。
                    open_boot_menu_editor(hwnd, shared);
                } else if command_id == ID_CMD_BUTTON {
                    // 现场命令提示符：恢复流程不暂停不受影响（cmd 独立进程）。
                    open_cmd_prompt(shared);
                } else if command_id == ID_PAUSE_BUTTON {
                    // 暂停/继续切换：与全局 P 键共用 toggle_pause。
                    toggle_pause(hwnd, shared, false);
                } else if command_id == ID_BCD_ROLLBACK_BUTTON {
                    // BCD 回退：与全局 B 键共用 trigger_bcd_rollback。
                    trigger_bcd_rollback(shared, false);
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
        WM_CLOSE => unsafe {
            // 【2026-10-16 审查修订，卡死 4】暂停中关闭窗口 = 用户放弃。
            //
            // 此前主进度窗口**没有** WM_CLOSE 分支：消息落到
            // `_ => DefWindowProcW`，窗口被销毁，而恢复线程还在
            // `wait_if_paused` 里无超时自旋——进程活着、没有任何 UI、
            // 没有超时、没有出口（`WM_DESTROY` 只做 KillTimer + 销毁日志
            // 弹窗 + PostQuitMessage，从不通知恢复线程）。
            //
            // 用 `close_requested` 区分「用户关窗」与「程序自己请求关闭」：
            // `request_close` 会先置位再投递 WM_CLOSE，那种情况不能算放弃
            // （操作已经做完，此刻多半也没在暂停）。
            //
            // 注意：置位后**必须仍调用 DefWindowProcW**，否则窗口不会被销毁，
            // 关窗就变成"窗口还在但恢复已中断"，比原来更糟。
            let shared_ptr = GetWindowLongPtrW(hwnd, GWL_USERDATA) as *const ProgressShared;
            if !shared_ptr.is_null() {
                let program_requested = (*shared_ptr).close_requested.load(Ordering::SeqCst);
                if !program_requested {
                    (*shared_ptr).pause_abandoned.store(true, Ordering::SeqCst);
                    if (*shared_ptr).paused.load(Ordering::SeqCst) {
                        let log_path = (*shared_ptr).log_path.lock().unwrap().clone();
                        let _ = crate::append_log(
                            &log_path,
                            "用户关闭进度窗口（当时处于暂停）：恢复链将中断并按失败处理，现场保留",
                        );
                    }
                }
            }
            DefWindowProcW(hwnd, message, w_param, l_param)
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
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
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
        window_done_tx: done_tx,
        window_done_rx: Mutex::new(done_rx),
        start_time_aligned: AtomicBool::new(false),
        start_time: Mutex::new(std::time::Instant::now()),
        hwnd: Mutex::new(None),
        operation: Mutex::new(operation.map(|s| s.to_string())),
        log_window: Mutex::new(None),
        boot_menu: Mutex::new(None),
        paused: AtomicBool::new(false),
        pause_abandoned: AtomicBool::new(false),
        bcd_rollback: Mutex::new(None),
    });
    let thread_shared = Arc::clone(&shared);
    let handle = thread::spawn(move || {
        // 窗口线程：创建 Win32 进度窗口并进入消息循环。
        unsafe { run_window(&thread_shared) };
        // 退出前发信号，让 `close_and_wait` 能区分「已结束」与「还卡着」。
        // 收不收得到无所谓（主线程可能已经超时放弃），故忽略返回值。
        let _ = thread_shared.window_done_tx.send(());
    });
    *shared.window_thread.lock().unwrap() = Some(handle);
    shared
}

/// 等旧窗口彻底退出，再显示重试窗口，避免异步销毁抢走新窗口焦点。
///
/// 【2026-10-17 第二轮核查，卡死 5】join **带超时**：等不到「窗口线程已结束」
/// 的信号就放弃 join（detach）并如实记一行，绝不把恢复链永久钉在这里 ——
/// 卡在 join 上时，失败重试弹框、自动逃出、BCD 回退**全都到不了**。
pub fn close_and_wait(shared: &ProgressShared) {
    /// 给窗口线程收尾的时间。正常关闭是毫秒级；这只是防止「永久阻塞」的闸门。
    const JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
    request_close(shared);
    let handle = shared.window_thread.lock().unwrap().take();
    let Some(handle) = handle else {
        return;
    };
    let finished = match shared.window_done_rx.lock() {
        Ok(receiver) => receiver.recv_timeout(JOIN_TIMEOUT).is_ok(),
        Err(_) => false,
    };
    if finished {
        let _ = handle.join();
        return;
    }
    // 超时：放弃 join（drop 即 detach），让恢复链继续往下走，并把这件事
    // 写进日志——事后看得到"进度窗没能正常退出"这个事实，而不是凭空猜测。
    let log = shared
        .log_path
        .lock()
        .map(|path| path.clone())
        .unwrap_or_default();
    if !log.as_os_str().is_empty() {
        let _ = crate::append_log(
            &log,
            &format!(
                "进度窗口线程 {} 秒内没有退出，`close_and_wait` 放弃等待并继续恢复链（窗口可能卡在日志刷新或 BCD 回退）",
                JOIN_TIMEOUT.as_secs()
            ),
        );
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
    // 标题按 operation 分叉（2026-10-15 用户裁定 A3）：在线备份与在线还原共用本窗口，
    // 但用户看到的标题写「恢复进度」会让备份用户以为正在还原。阶段骨架早在 487 行
    // 就按 is_backup 分叉，标题此前是漏网的一处。
    let operation = shared
        .operation
        .lock()
        .unwrap()
        .as_deref()
        .map(|s| {
            s.eq_ignore_ascii_case("BACKUP") || s.eq_ignore_ascii_case("ONLINE_BACKUP")
        })
        .unwrap_or(false);
    let title = encode(if operation {
        "BackupRestore 备份进度"
    } else {
        "BackupRestore 恢复进度"
    });
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
            // 键盘通路：WinRE 下鼠标未必可用，所以 L 打开详细日志、M 打开引导修复、
            // K 打开命令提示符、P 切换暂停/继续、B 触发 BCD 回退、Esc 收起日志
            // 弹窗，都在消息循环里拦截——焦点落在主窗口哪个子控件上都有效。
            if msg_message == WM_KEYDOWN {
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
                // 【2026-10-18 缺陷修复】字母快捷键只在「主进度窗口家族」内拦截。
                // 此前 L/K/P 是全局的：引导修复编辑器开着的时候，在改名输入框里
                // 输入 L/K/P 会误触对应功能——字母根本打不进去（全局键本为
                // 「WinRE 无鼠标」准备，不能以吃掉正常输入为代价）。编辑器与
                // 日志弹窗自有键盘处理（Esc/回车/空格/F2/Shift+方向键），
                // 焦点在它们身上时不该被上层窗口的快捷键拦截。
                // 主窗口家族 = 主窗口自身或其直接子控件（按钮/日志尾部/详情框）。
                let main_family = msg_hwnd == hwnd || GetParent(msg_hwnd) == hwnd;
                if main_family {
                    if msg_wparam == VK_L {
                        open_log_window(hwnd, shared);
                        continue;
                    }
                    if msg_wparam == VK_M {
                        open_boot_menu_editor(hwnd, shared);
                        continue;
                    }
                    if msg_wparam == VK_K {
                        open_cmd_prompt(&*shared);
                        continue;
                    }
                    if msg_wparam == VK_P {
                        toggle_pause(hwnd, shared, true);
                        continue;
                    }
                    if msg_wparam == VK_B {
                        trigger_bcd_rollback(shared, true);
                        continue;
                    }
                }
            }
            // 引导修复编辑器与进度窗共用消息循环；先拦截其快捷键，再交给对话框导航。
            // 消息可能发给编辑器本身，也可能发给其子控件，因此沿父链尝试。
            let mut key_target = msg_hwnd;
            while !key_target.is_null() {
                if crate::boot_menu::handle_key(key_target, &*msg.as_ptr()) {
                    break;
                }
                let parent = GetParent(key_target);
                if parent.is_null() || parent == hwnd {
                    break;
                }
                key_target = parent;
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
