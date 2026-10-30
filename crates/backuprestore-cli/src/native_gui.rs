//! Rust-native Win32 front end.
//!
//! The GUI deliberately uses only Win32 APIs exposed by the Windows SDK. This
//! keeps the offline build self-contained and avoids pulling a GUI framework or
//! another network dependency into the recovery package. Destructive work is
//! performed by the elevated Rust preparation command;
//! this module owns the window, fields, validation, confirmation and status.

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::fs;
use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{null, null_mut};

use crate::append_log;
use crate::text_parsing::{
    WimImageInfo, compression_from_ui_index, decode_bcdedit_bytes, first_braced_guid, format_bytes,
    json_text, parse_wim_images, quote_argument,
};
use crate::error_code::ErrCode;
use crate::status_receipt::StatusOutcome;
use backuprestore_core::PROGRAM_VERSION;

/// 「程序设置」模态对话框（2026-10-26 从本文件拆出，见子模块头部注释）。
///
/// 子模块而不是顶层模块：它要用这里一整套私有 Win32 类型/常量/辅助函数，
/// 做成顶层就得把它们全提成 `pub(crate)`。
mod settings_dialog;
use settings_dialog::ask_settings_dialog;

type Handle = *mut c_void;
type HInstance = Handle;
type HIcon = Handle;
type HCursor = Handle;
type HBrush = Handle;
type HMenu = Handle;
type Hwnd = Handle;
type WParam = usize;
type LParam = isize;
type LResult = isize;

const WM_CREATE: u32 = 0x0001;
const WM_COMMAND: u32 = 0x0111;
const WM_DESTROY: u32 = 0x0002;
const WM_CLOSE: u32 = 0x0010;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf0000;
const WS_POPUP: u32 = 0x80000000;
const WS_EX_TOPMOST: u32 = 0x00000008;
const TTS_ALWAYSTIP: u32 = 0x0001;
const TTS_NOPREFIX: u32 = 0x0002;
const TTS_BALLOON: u32 = 0x0040;
const WS_VISIBLE: u32 = 0x10000000;
const WS_CHILD: u32 = 0x40000000;
const WS_BORDER: u32 = 0x00800000;
const WS_TABSTOP: u32 = 0x00010000;
const WS_VSCROLL: u32 = 0x00200000;
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_READONLY: u32 = 0x0800;
const CBS_DROPDOWNLIST: u32 = 0x0003;
const BS_AUTORADIOBUTTON: u32 = 0x0009;
const BS_PUSHLIKE: u32 = 0x1000;
const WS_GROUP: u32 = 0x00020000;
const CB_ADDSTRING: u32 = 0x0143;
const CB_RESETCONTENT: u32 = 0x014b;
const CB_GETCOUNT: u32 = 0x0146;
const CB_SETCURSEL: u32 = 0x014e;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETDROPPEDWIDTH: u32 = 0x0160;
const BM_SETCHECK: u32 = 0x00f1;
const BST_UNCHECKED: usize = 0;
const BST_CHECKED: usize = 1;
const CBN_SELCHANGE: usize = 1;
/// 编辑框内容变化通知。注意 SetWindowText 也会触发它——判定「用户手改」
/// 必须对照 `menu_name_auto`（见 set_menu_name）。
const EN_CHANGE: usize = 0x0300;
const WM_SETFONT: u32 = 0x0030;
const WM_SIZE: u32 = 0x0005;
const WM_KEYDOWN: u32 = 0x0100;
// 虚拟键码（用于快捷键）：Ctrl+B/R/P/O、F5、回车、Esc。
const VK_RETURN: u32 = 0x0D;
const VK_ESCAPE: u32 = 0x1B;
const VK_CONTROL: u32 = 0x11;
const VK_B: u32 = 0x42;
const VK_R: u32 = 0x52;
const VK_P: u32 = 0x50;
const VK_O: u32 = 0x4F;
const VK_F5: u32 = 0x74;
// 默认按钮样式：无焦点时按回车也会触发该按钮。
const BS_DEFPUSHBUTTON: u32 = 0x00000001;
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SW_SHOWNORMAL: i32 = 1;
const SW_MAXIMIZE: i32 = 3;
const MB_OK: u32 = 0x00000000;
const MB_ICONERROR: u32 = 0x00000010;
const MB_YESNO: u32 = 0x00000004;
const MB_YESNOCANCEL: u32 = 0x00000003;
const MB_ICONWARNING: u32 = 0x00000030;
const MB_ICONINFORMATION: u32 = 0x00000040;
const MB_ICONQUESTION: u32 = 0x00000020;
/// 默认按钮放在第二个（「否」）。用于「发现镜像有问题」这类警告：让人看见，
/// 但别让一个回车就把坏镜像装上去。
const MB_DEFBUTTON2: u32 = 0x00000100;
/// 让消息框在最前台（恢复环境没有外壳帮它抢焦点；待续跑询问框也需要）。
const MB_SETFOREGROUND: u32 = 0x00010000;
const IDYES: i32 = 6;
const IDNO: i32 = 7;
const IDOK: i32 = 1;
const WM_APP_TEST_INSTALL: u32 = 0x8001;
/// 在线备份/还原后台线程完成通知（结果在 ONLINE_RESULT 全局读）。
const WM_APP_ONLINE_DONE: u32 = 0x8002;
/// 测试钩子：自动安装时跳过确认框（验收/自动化测试用）。
static TEST_AUTO_CONFIRM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// PE 桌面「自动点击」模式：存在 S:\pe-click.txt 时置位。PE 桌面启动后
/// 自动向主窗口投递对应按钮的 WM_COMMAND（与真实鼠标点击走完全相同的
/// 分发路径），所有确认框自动接受（等效用户点"是"），执行完成后自动
/// 恢复 BCD default、清除 bootsequence 并重启回 Windows。
static PE_AUTO_CLICK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const ID_REFRESH: usize = 1001;
const ID_READ_IMAGE: usize = 1002;
const ID_CREATE_TASK: usize = 1003;
const ID_REFRESH_TASK: usize = 1004;
const ID_LANGUAGE: usize = 1005;
const ID_BROWSE_IMAGE: usize = 1006;
/// 顶栏「设置…」按钮：打开程序设置窗口（当前只有「恢复环境失败处理」一项）。
const ID_SETTINGS: usize = 1007;
/// 顶栏「取消待重启任务」按钮：把「上次没跑完、每次启动都想重启机器」的任务
/// 交回给用户处置（继续 / 放弃）。此前 GUI 完全没有这个出口（2026-10-17
/// 第二轮核查，逃生 A/D）。
const ID_CANCEL_PENDING: usize = 1008;
/// 设置窗口内的控件（独立 ID 段，避免与主窗口控件串号）。
const ID_SETTINGS_THRESHOLD_EDIT: usize = 3100;
const ID_SETTINGS_ACTION_AUTO: usize = 3101;
const ID_SETTINGS_ACTION_PAUSE: usize = 3102;
const ID_SETTINGS_SAVE: usize = 3103;
const ID_SETTINGS_CANCEL: usize = 3104;
const ID_SETTINGS_HINT: usize = 3105;
const ID_SETTINGS_DETAIL_AUTO: usize = 3106;
const ID_SETTINGS_DETAIL_PAUSE: usize = 3107;
/// DISM 等待上限（秒）文本框与其说明。
const ID_SETTINGS_DISM_TIMEOUT_EDIT: usize = 3108;
const ID_SETTINGS_DISM_TIMEOUT_HINT: usize = 3109;
/// `pause` 策略的最长等待秒数文本框。
const ID_SETTINGS_PAUSE_WAIT_EDIT: usize = 3110;
/// 界面字号下拉框与其说明（2.6.81 起用户可在设置里自选字号）。
const ID_SETTINGS_FONT_SIZE_COMBO: usize = 3111;
const ID_SETTINGS_FONT_SIZE_HINT: usize = 3112;
/// 设置窗口的类名（`RegisterClassExW` 用）。
const SETTINGS_WINDOW_CLASS: &str = "BackupRestoreSettingsWindow";
/// 顶栏「备份文件管理…」按钮（P1-c：需求 §3 的「管理备份文件」入口）。
const ID_IMAGE_ADMIN: usize = 1009;
/// 备份文件管理窗口的控件（独立 ID 段 3200+，避免与设置窗口/主窗口串号）。
const ID_IMAGE_ADMIN_COMBO: usize = 3200;
const ID_IMAGE_ADMIN_DETAIL: usize = 3201;
const ID_IMAGE_ADMIN_KEEP_EDIT: usize = 3202;
const ID_IMAGE_ADMIN_REFRESH: usize = 3203;
const ID_IMAGE_ADMIN_DELETE: usize = 3204;
const ID_IMAGE_ADMIN_OPEN_FOLDER: usize = 3205;
const ID_IMAGE_ADMIN_CLOSE: usize = 3206;
const ID_IMAGE_ADMIN_HINT: usize = 3207;
/// 顶栏「查看日志」按钮（P1-a：报障第一步「日志给我」不该让用户自己翻目录）。
const ID_VIEW_LOGS: usize = 1010;
/// 日志查看窗口的控件（独立 ID 段 3300+）。
const ID_LOGS_COMBO: usize = 3300;
const ID_LOGS_DETAIL: usize = 3301;
const ID_LOGS_OPEN_FILE: usize = 3302;
const ID_LOGS_OPEN_DIR: usize = 3303;
const ID_LOGS_REFRESH: usize = 3304;
const ID_LOGS_CLOSE: usize = 3305;
const ID_LOGS_HINT: usize = 3306;
const ID_OPERATION_BACKUP: usize = 1101;
const ID_OPERATION_RESTORE: usize = 1102;
const ID_OPERATION_PE: usize = 1104;
// 三入口拆分（2026-10-14 拍板）：「单系统还原」拆成 A/B/C 三个入口，
// tab 总数 4 → 6。B=还原数据到非 C 盘，C=还原系统到非 C 盘。
// 2026-10-26 用户要求移除「探测」入口（ID 1107）：tab 总数 6 → 5。
// 探测**能力仍在命令行**（`prepare --operation probe`），只是 GUI 不再提供入口。
const ID_OPERATION_RESTORE_DATA: usize = 1105;
const ID_OPERATION_RESTORE_OTHER: usize = 1106;
const ID_SOURCE: usize = 1202;
const ID_IMAGE: usize = 1203;
const ID_TARGET: usize = 1204;
const ID_INDEX: usize = 1206;
const ID_MENU: usize = 1207;
const ID_COMPRESS: usize = 1208;
const ID_INDEX_NAME_EDIT: usize = 1209;
const ID_KEEP_EDIT: usize = 1210;
const ID_VERIFY_HASH: usize = 1211;
// B/C 入口的执行通道单选（在线=不重启 / 离线=重启进 PE/RE），默认均在线。
const ID_CHANNEL_ONLINE: usize = 1212;
const ID_CHANNEL_OFFLINE: usize = 1213;
// B（还原数据到非 C 盘）的「格式化目标分区」复选框（2026-10-14 用户拍板：
// B 的正确语义是格式化后还原，默认勾选；不勾=覆盖式应用、保留卷上多余文件。
// 离线通道在 RE 内恒格式化，不受此开关影响——文案里写明）。
const ID_FORMAT_TARGET: usize = 1214;
const BS_AUTOCHECKBOX: u32 = 0x00000003;
const BM_GETCHECK: u32 = 0x00F0;
const ID_STATUS: usize = 1300;
const ID_LANGUAGE_LABEL: usize = 2009;
const ID_SOURCE_DETAILS: usize = 2012;
const ID_TARGET_DETAILS: usize = 2013;
const OFN_PATHMUSTEXIST: u32 = 0x00000800;
const OFN_FILEMUSTEXIST: u32 = 0x00001000;
const CREATE_NO_WINDOW: u32 = 0x08000000;
// 2.6.87：`DEFAULT_GUI_FONT` 常量已删除。原来两个「选择」弹窗用它取系统字体
// 再覆盖 `create_control` 设好的雅黑，导致那两个弹窗的字号不跟随设置。
// 现在全仓统一走 `gui_font()`，不要再引入系统字体。
// Edit control messages: EM_SETLIMITTEXT with a zero limit on a multiline EDIT
// lifts the default 32,767 character cap, which silently truncated the tail of
// the information report (the 按钮边界 section never reached the screen).
const EM_SETLIMITTEXT: u32 = 0x00C5;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_ELEVATION_CLASS: u32 = 20;
const TTM_ADDTOOLW: u32 = 0x0432;
const TTM_GETTOOLCOUNT: u32 = 0x040d;
const TTF_IDISHWND: u32 = 0x0001;
const TTF_SUBCLASS: u32 = 0x0010;
const ICC_WIN95_CLASSES: u32 = 0x000000ff;

// PE desktop mode window messages, metrics and control identifiers.
const WM_TIMER: u32 = 0x0113;
const WM_CTLCOLORSTATIC: u32 = 0x0138;
const WM_CTLCOLORBTN: u32 = 0x0135;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const EWX_REBOOT: u32 = 0x00000002;
const SS_CENTER: u32 = 0x0001;
const ID_PE_BACKUP: usize = 1401;
const ID_PE_RESTORE: usize = 1402;
const ID_PE_SECONDARY: usize = 1403;
const ID_PE_CMD: usize = 1404;
const ID_PE_EXIT: usize = 1405;
const ID_PE_MAIN_GUI: usize = 1406;
// 「重启」已合并进「返回 Windows」（返回 = 修复 BCD + 重启回 Windows），
// 原 ID_PE_REBOOT=1406 改为「打开完整程序」按钮。
// 「软硬件信息」按钮与其只读信息窗口的文本框
const ID_PE_SYSINFO: usize = 1419;
const ID_PE_SYSINFO_TEXT: usize = 1420;
const ID_PE_TITLE: usize = 1407;
const ID_PE_VERSION: usize = 1408;
const ID_PE_CLOCK: usize = 1409;
// 主窗口「安装 PE」tab 的"重启进入 PE"按钮（PE 桌面无独立重启按钮，
// 「重启」已合并进「返回 Windows」）
const ID_PE_REBOOT_MAIN: usize = 1410;
// 主窗口「安装 PE」tab 的"创建桌面快捷方式"按钮
const ID_PE_SHORTCUT: usize = 1411;
// 「安装 PE」tab 启动方式单选（RAM disk / 硬盘启动）与 PE 目录名输入
const ID_PE_MODE_RAM: usize = 1412;
const ID_PE_MODE_DISK: usize = 1413;
const ID_PE_DIR_LABEL: usize = 1414;
const ID_PE_DIR_EDIT: usize = 1415;
// 「PE 目录路径」旁的浏览按钮：弹文件夹选择对话框回填完整路径
const ID_PE_DIR_BROWSE: usize = 1418;
const ID_PE_NAME_LABEL: usize = 1416;
const ID_PE_NAME_EDIT: usize = 1417;
// PE 桌面"备份/还原/第二系统"任务对话框控件
const ID_PE_DLG_LABEL1: usize = 1456;
const ID_PE_DLG_LABEL2: usize = 1457;
const ID_PE_DLG_LABEL3: usize = 1458;
const ID_PE_DLG_WARN: usize = 1459;
const ID_PE_DLG_COMBO: usize = 1451;
const ID_PE_DLG_EDIT: usize = 1452;
const ID_PE_DLG_NAME: usize = 1453;
const ID_PE_DLG_OK: usize = 1454;
const ID_PE_DLG_CANCEL: usize = 1455;
const WS_CAPTION: u32 = 0x00c00000;
const WS_SYSMENU: u32 = 0x00080000;
// 软硬件信息窗口：可调整大小的边框与横向滚动条（长行如 ipconfig 输出不折行）
const WS_THICKFRAME: u32 = 0x00040000;
const WS_HSCROLL: u32 = 0x00100000;
const PM_REMOVE: u32 = 0x0001;
const WM_QUIT: u32 = 0x0012;
const ES_AUTOHSCROLL: u32 = 0x0080;
/// 只允许输入数字（设置窗口的「失败次数」文本框；保存时仍会再校验一次范围）。
const ES_NUMBER: u32 = 0x2000;
const PE_TIMER_ID: usize = 1;
// COLORREF values are 0x00BBGGRR.
const PE_BACKGROUND: u32 = 0x00553a2b; // RGB(43, 58, 85), deep blue-grey.
const PE_TITLE_TEXT: u32 = 0x00e8e8ea; // near-white.

#[repr(C)]
pub(crate) struct Point {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// `SIZE`：`GetTextExtentPoint32W` 的输出（宽、高，单位是 DC 的逻辑像素）。
#[repr(C)]
struct TextSize {
    cx: i32,
    cy: i32,
}

#[repr(C)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[repr(C)]
struct OpenFileNameW {
    l_struct_size: u32,
    hwnd_owner: Hwnd,
    h_instance: HInstance,
    lpstr_filter: *const u16,
    lpstr_custom_filter: *mut u16,
    n_max_cust_filter: u32,
    n_filter_index: u32,
    lpstr_file: *mut u16,
    n_max_file: u32,
    lpstr_file_title: *mut u16,
    n_max_file_title: u32,
    lpstr_initial_dir: *const u16,
    lpstr_title: *const u16,
    flags: u32,
    n_file_offset: u16,
    n_file_extension: u16,
    lpstr_def_ext: *const u16,
    l_cust_data: isize,
    lpfn_hook: *mut c_void,
    lp_template_name: *const u16,
    pv_reserved: *mut c_void,
    dw_reserved: u32,
    flags_ex: u32,
}

/// 手工 mirror C 的 `TOOLINFOW`（x64，总 64 字节）。
///
/// **不要加 `reserved` 字段**：2026-10-18 实机抓到「tooltips installed:
/// added=34 tool_count=0」——此前版本末尾多一个 usize reserved 使结构变成
/// 72 字节，comctl32 按 cbSize 校验后拒绝每一次 TTM_ADDTOOLW，34 个控件
/// 全部注册失败（而应用层毫无知觉，只因没人悬停看过一眼）。
#[repr(C)]
struct ToolInfoW {
    cb_size: u32,
    u_flags: u32,
    hwnd: Hwnd,
    u_id: usize,
    rect: Rect,
    hinst: HInstance,
    lpsz_text: *const u16,
    l_param: isize,
}

#[repr(C)]
struct InitCommonControlsEx {
    size: u32,
    classes: u32,
}

#[repr(C)]
pub(crate) struct Msg {
    pub(crate) hwnd: Hwnd,
    pub(crate) message: u32,
    pub(crate) w_param: WParam,
    pub(crate) l_param: LParam,
    pub(crate) time: u32,
    pub(crate) point: Point,
}

#[repr(C)]
struct WndClassExW {
    cb_size: u32,
    style: u32,
    wnd_proc: Option<unsafe extern "system" fn(Hwnd, u32, WParam, LParam) -> LResult>,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: HInstance,
    h_icon: HIcon,
    h_cursor: HCursor,
    h_brush: HBrush,
    menu_name: *const u16,
    class_name: *const u16,
    h_icon_sm: HIcon,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(
        callback: Option<unsafe extern "system" fn(Hwnd, LParam) -> i32>,
        l_param: LParam,
    ) -> i32;
    fn EnableWindow(window: Hwnd, enable: i32) -> i32;
    fn SetForegroundWindow(window: Hwnd) -> i32;
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
        menu: HMenu,
        instance: HInstance,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, message: u32, w_param: WParam, l_param: LParam) -> LResult;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn DispatchMessageW(message: *const Msg) -> LResult;
    fn GetMessageW(message: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    fn GetDlgItem(hwnd: Hwnd, id: i32) -> Hwnd;
    fn IsDlgButtonChecked(hwnd: Hwnd, id: i32) -> u32;
    fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max_count: i32) -> i32;
    fn GetClassNameW(hwnd: Hwnd, class_name: *mut u16, max_count: i32) -> i32;
    fn GetWindowThreadProcessId(hwnd: Hwnd, process_id: *mut u32) -> u32;
    fn MessageBoxW(hwnd: Hwnd, text: *const u16, caption: *const u16, flags: u32) -> i32;
    fn PostQuitMessage(exit_code: i32);
    fn PostMessageW(hwnd: Hwnd, message: u32, w_param: WParam, l_param: LParam) -> i32;
    fn SendMessageW(hwnd: Hwnd, message: u32, w_param: WParam, l_param: LParam) -> LResult;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn SetFocus(hwnd: Hwnd) -> Hwnd;
    fn ShowWindow(hwnd: Hwnd, command: i32) -> i32;
    fn MoveWindow(hwnd: Hwnd, x: i32, y: i32, width: i32, height: i32, repaint: i32) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    // 由「目标客户区」反推「整窗尺寸」：CreateWindowExW 的 width/height 是含
    // 边框与标题栏的整窗尺寸，直接传客户区尺寸会让客户区小一圈（2.6.67 的缺陷）。
    fn AdjustWindowRectEx(rect: *mut Rect, style: u32, menu: i32, ex_style: u32) -> i32;
    fn TranslateMessage(message: *const Msg) -> i32;
    fn PeekMessageW(message: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
    fn IsWindow(hwnd: Hwnd) -> i32;
    // 对话框式键盘导航：让普通窗口也能用 Tab 遍历焦点、方向键切换
    // 单选按钮、回车触发默认按钮、Esc 关闭、Alt+助记键。
    fn IsDialogMessageW(hwnd: Hwnd, message: *const Msg) -> i32;
    // 2.6.81：用户在设置里改字号后，要遍历主窗口所有子控件重设字体。
    // 回调返回 0 表示停止枚举，这里始终返回 1（继续）。
    fn EnumChildWindows(hwnd: Hwnd, callback: EnumChildProc, l_param: LParam) -> i32;
    // 即时物理键盘状态：prlctl 宿主导入的修饰键（如 Ctrl）在消息队列里可能有
    // 时序延迟，GetKeyState 偶发读不到；GetAsyncKeyState 反映当前真实状态，更可靠。
    fn GetAsyncKeyState(key: i32) -> i16;
}

// 2.6.87：这里原来的 `GetStockObject` 声明已删除（唯一的两处调用是那两个
// 「选择」弹窗拿系统字体覆盖雅黑，现已改为统一走 `gui_font()`）。

#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(file_name: *mut OpenFileNameW) -> i32;
    fn GetSaveFileNameW(file_name: *mut OpenFileNameW) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> HInstance;
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn CloseHandle(handle: Handle) -> i32;
    fn Sleep(milliseconds: u32);
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, desired_access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        information_class: u32,
        information: *mut c_void,
        information_length: u32,
        return_length: *mut u32,
    ) -> i32;
    /// 读注册表值。用来取当前系统版本（备份索引名默认值）——GUI 里不能为三个
    /// 字符串去 spawn reg.exe（会闪控制台），直接读 HKLM 即可。
    fn RegGetValueW(
        key: Handle,
        sub_key: *const u16,
        value: *const u16,
        flags: u32,
        value_type: *mut u32,
        data: *mut c_void,
        data_size: *mut u32,
    ) -> i32;
}

const HKEY_LOCAL_MACHINE: Handle = 0x8000_0002usize as Handle;
const RRF_RT_REG_SZ: u32 = 0x0000_0002;
const RRF_RT_REG_DWORD: u32 = 0x0000_0010;
/// Windows 版本信息的标准位置。
const CURRENT_VERSION_KEY: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";

#[repr(C)]
struct BrowseInfoW {
    hwnd_owner: Hwnd,
    pidl_root: *const c_void,
    display_name: *mut u16,
    title: *const u16,
    flags: u32,
    callback: Option<unsafe extern "system" fn(Hwnd, u32, isize, isize) -> i32>,
    l_param: isize,
    image: i32,
}

const BIF_RETURNONLYFSDIRS: u32 = 0x00000001;
const BIF_NEWDIALOGSTYLE: u32 = 0x00000040;
const CSIDL_DESKTOPDIRECTORY: i32 = 0x0010;
const SHGFP_TYPE_CURRENT: u32 = 0;
const SEE_MASK_NOCLOSEPROCESS: u32 = 0x00000040;

#[repr(C)]
struct ShellExecuteInfoW {
    cb_size: u32,
    f_mask: u32,
    hwnd: Hwnd,
    lp_verb: *const u16,
    lp_file: *const u16,
    lp_parameters: *const u16,
    lp_directory: *const u16,
    n_show: i32,
    h_inst_app: HInstance,
    lp_id_list: *mut c_void,
    lp_class: *const u16,
    hkey_class: Handle,
    dw_hot_key: u32,
    h_icon: Handle,
    h_process: Handle,
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn ShellExecuteW(
        hwnd: Hwnd,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> isize;
    fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
    fn SHBrowseForFolderW(info: *const BrowseInfoW) -> *mut c_void;
    fn SHGetPathFromIDListW(pidl: *const c_void, path: *mut u16) -> i32;
    fn SHGetFolderPathW(hwnd: Hwnd, csidl: i32, token: Handle, flags: u32, path: *mut u16) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoTaskMemFree(pv: *mut c_void);
    fn CoInitializeEx(reserved: *const c_void, co_init: u32) -> i32;
    fn CoUninitialize();
}

const COINIT_APARTMENTTHREADED: u32 = 0x2;
const COINIT_DISABLE_OLE1DDE: u32 = 0x4;

#[link(name = "comctl32")]
unsafe extern "system" {
    fn InitCommonControlsEx(init: *const InitCommonControlsEx) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
    fn ExitWindowsEx(flags: u32, reserved: u32) -> i32;
    fn SetTimer(
        hwnd: Hwnd,
        id: usize,
        elapsed: u32,
        timer_proc: Option<unsafe extern "system" fn(Hwnd, u32, usize, u32)>,
    ) -> usize;
    fn KillTimer(hwnd: Hwnd, id: usize) -> i32;
    fn SetTextColor(hdc: Handle, color: u32) -> u32;
    fn SetBkMode(hdc: Handle, mode: i32) -> i32;
    // ★ 2.6.83：文字块尺寸**实测**用。用户可自选 12/14/16/18 四种字号，
    //   同一段文字在不同字号下折行数不同（14 号两行、18 号三行），写死高度
    //   必然错位，只能让 GDI 按当前字体真正算一遍。
    fn GetDC(hwnd: Hwnd) -> Handle;
    fn ReleaseDC(hwnd: Hwnd, hdc: Handle) -> i32;
    fn DrawTextW(
        hdc: Handle,
        text: *const u16,
        count: i32,
        rect: *mut Rect,
        format: u32,
    ) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLocalTime(time: *mut SystemTime);
    fn FindFirstVolumeW(volume_name: *mut u16, size: u32) -> Handle;
    fn FindNextVolumeW(handle: Handle, volume_name: *mut u16, size: u32) -> i32;
    fn FindVolumeClose(handle: Handle) -> i32;
    fn GetVolumeInformationW(
        root_path: *const u16,
        volume_name: *mut u16,
        volume_name_size: u32,
        serial: *mut u32,
        max_component_length: *mut u32,
        flags: *mut u32,
        fs_name: *mut u16,
        fs_name_size: u32,
    ) -> i32;
    fn SetVolumeMountPointW(mount_point: *const u16, volume: *const u16) -> i32;
    fn DeleteVolumeMountPointW(mount_point: *const u16) -> i32;
    fn GetFileAttributesW(name: *const u16) -> u32;
    fn GetLogicalDrives() -> u32;
    fn GetDiskFreeSpaceExW(
        directory: *const u16,
        free_bytes_available: *mut u64,
        total_bytes: *mut u64,
        total_free_bytes: *mut u64,
    ) -> i32;
    fn GetLastError() -> u32;
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: Handle,
    ) -> Handle;
    fn WriteFile(
        file: Handle,
        buffer: *const c_void,
        bytes_to_write: u32,
        bytes_written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn SelectObject(hdc: Handle, object: Handle) -> Handle;
    fn GetTextExtentPoint32W(
        hdc: Handle,
        text: *const u16,
        count: i32,
        size: *mut TextSize,
    ) -> i32;
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strikeout: u32,
        charset: u32,
        output_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face_name: *const u16,
    ) -> Handle;
    // 强制同步重绘：SetWindowTextW 只做异步失效，快速切换 tab 时旧文本会
    // 残留在控件区域，这里立即失效并更新窗口，保证像素层与文本层一致。
    fn InvalidateRect(hwnd: Handle, rect: *const Rect, erase: i32) -> i32;
    fn UpdateWindow(hwnd: Handle) -> i32;
}

const GWLP_USERDATA: i32 = -21;

#[repr(C)]
struct TokenElevation {
    token_is_elevated: u32,
}

struct Controls {
    language: Hwnd,
    /// 五个操作入口 tab（备份 / 还原系统到 C 盘 / 还原数据到非 C 盘 /
    /// 新增第二系统 / 安装 PE），BS_AUTORADIOBUTTON + BS_PUSHLIKE 单选组。
    operation_tabs: [Hwnd; 5],
    source: Hwnd,
    image: Hwnd,
    target: Hwnd,
    index: Hwnd,
    menu: Hwnd,
    compress: Hwnd,
    /// 备份索引名输入框（默认程序启动时间，可修改）。
    index_name: Hwnd,
    /// 保留最近 N 个索引输入框（0=不清理）。
    keep: Hwnd,
    source_details: Hwnd,
    target_details: Hwnd,
    /// 还原校验选项框：勾选时对比哈希，不勾选仅对比大小。
    verify_hash: Hwnd,
    status: Hwnd,
}

struct State {
    root: Hwnd,
    tooltip: Hwnd,
    controls: Controls,
    executable_dir: PathBuf,
    wim_images: Vec<WimImageInfo>,
    drives: Vec<DriveInfo>,
    operation_index: usize,
    /// 测试钩子：智能分流时直接用该选择（0=取消 1=PE 2=RE），跳过弹窗。
    test_drive_choice: Option<i32>,
    /// 「启动菜单名称」文本框是否被用户手动改过（S5）。改过之后 WIM 索引名
    /// 预填不再覆盖它——用户意图优先于自动预填。
    menu_name_touched: bool,
    /// 程序最后一次自动写进菜单名文本框的值。WM_COMMAND 的 EN_CHANGE 拿到
    /// 与它不同的文本才判定「用户手改」：SetWindowText 自己也会触发 EN_CHANGE，
    /// 没有这个对照就会把程序自己的预填误判成用户输入。
    menu_name_auto: Option<String>,
    /// 备份索引名是否被用户手改过：改过就不再随源卷变化重算（同菜单名）。
    index_name_touched: bool,
    /// 程序最后一次自动写进索引名文本框的值（判定"手改"的对照）。
    index_name_auto: Option<String>,
    /// 恢复环境失败处理策略（设置窗口读写；创建任务时随参数落盘进
    /// `RecoveryTask.env`）。启动时从程序目录的 `BackupRestore.settings.ini` 读。
    failure_policy: crate::failure_policy::FailurePolicy,
    /// DISM 等待上限（秒；0 = 不限制 = 历史行为）。来源 = 设置窗口
    /// `BackupRestore.settings.ini` 的 `dism_timeout_secs`。创建任务时随
    /// `--dism-timeout-secs` 传给 prepare，落盘进 `RecoveryTask.env`。
    dism_timeout_secs: u32,
}

/// State for the PE recovery-desktop window. The operation the technician
/// picks is carried out through `PE_EXIT_TAB` (an atomic) so the window can be
/// destroyed safely inside WM_COMMAND before the message loop finishes.
struct PeDesktopState {
    clock: Hwnd,
    fonts: [Handle; 3],
    /// PE 恢复桌面按钮的气泡提示（tooltip）窗口句柄。
    tooltip: Hwnd,
}

/// GUI diagnostics land in the dedicated log root (see `crate::log_root`),
/// never beside the executable: 2026-10-18 用户要求「日志放到一个不是 PE、
/// 不是目标分区的一个地方」，而程序目录可能在系统盘/还原目标上。A failed
/// diagnostic write must not alter a safety check or a pending preparation
/// operation.
///
/// `state` 保留在签名里（调用点遍布全文件）；当前实现不再需要它——
/// 日志根是进程级单例，与窗口状态无关。
fn append_gui_log(_state: &State, message: &str) {
    let _ = super::append_log(&crate::log_root::logs_dir().join("gui.log"), message);
}

/// 控件 ID → 可读名称。用于 GUI 操作审计日志：
/// 出问题时能从 gui.log 一眼看出「用户点了哪个控件」，而不是只有数字 ID。
/// 由 `const ID_*` 生成，新增控件时请同步补一行。
const CONTROL_NAMES: &[(usize, &str)] = &[
    (ID_REFRESH, "ID_REFRESH"),
    (ID_READ_IMAGE, "ID_READ_IMAGE"),
    (ID_CREATE_TASK, "ID_CREATE_TASK"),
    (ID_REFRESH_TASK, "ID_REFRESH_TASK"),
    (ID_LANGUAGE, "ID_LANGUAGE"),
    (ID_BROWSE_IMAGE, "ID_BROWSE_IMAGE"),
    (ID_SETTINGS, "ID_SETTINGS"),
    (ID_CANCEL_PENDING, "ID_CANCEL_PENDING"),
    (ID_IMAGE_ADMIN, "ID_IMAGE_ADMIN"),
    (ID_VIEW_LOGS, "ID_VIEW_LOGS"),
    (ID_LOGS_COMBO, "ID_LOGS_COMBO"),
    (ID_LOGS_DETAIL, "ID_LOGS_DETAIL"),
    (ID_LOGS_OPEN_FILE, "ID_LOGS_OPEN_FILE"),
    (ID_LOGS_OPEN_DIR, "ID_LOGS_OPEN_DIR"),
    (ID_LOGS_REFRESH, "ID_LOGS_REFRESH"),
    (ID_LOGS_CLOSE, "ID_LOGS_CLOSE"),
    (ID_OPERATION_BACKUP, "ID_OPERATION_BACKUP"),
    (ID_OPERATION_RESTORE, "ID_OPERATION_RESTORE"),
    (ID_OPERATION_PE, "ID_OPERATION_PE"),
    (ID_OPERATION_RESTORE_DATA, "ID_OPERATION_RESTORE_DATA"),
    (ID_OPERATION_RESTORE_OTHER, "ID_OPERATION_RESTORE_OTHER"),
    (ID_SOURCE, "ID_SOURCE"),
    (ID_IMAGE, "ID_IMAGE"),
    (ID_TARGET, "ID_TARGET"),
    (ID_INDEX, "ID_INDEX"),
    (ID_MENU, "ID_MENU"),
    (ID_COMPRESS, "ID_COMPRESS"),
    (ID_INDEX_NAME_EDIT, "ID_INDEX_NAME_EDIT"),
    (ID_KEEP_EDIT, "ID_KEEP_EDIT"),
    (ID_VERIFY_HASH, "ID_VERIFY_HASH"),
    (ID_CHANNEL_ONLINE, "ID_CHANNEL_ONLINE"),
    (ID_CHANNEL_OFFLINE, "ID_CHANNEL_OFFLINE"),
    (ID_FORMAT_TARGET, "ID_FORMAT_TARGET"),
    (ID_STATUS, "ID_STATUS"),
    (ID_PE_BACKUP, "ID_PE_BACKUP"),
    (ID_PE_RESTORE, "ID_PE_RESTORE"),
    (ID_PE_SECONDARY, "ID_PE_SECONDARY"),
    (ID_PE_CMD, "ID_PE_CMD"),
    (ID_PE_EXIT, "ID_PE_EXIT"),
    (ID_PE_MAIN_GUI, "ID_PE_MAIN_GUI"),
    (ID_PE_TITLE, "ID_PE_TITLE"),
    (ID_PE_VERSION, "ID_PE_VERSION"),
    (ID_PE_CLOCK, "ID_PE_CLOCK"),
    (ID_PE_REBOOT_MAIN, "ID_PE_REBOOT_MAIN"),
    (ID_PE_SHORTCUT, "ID_PE_SHORTCUT"),
    (ID_PE_MODE_RAM, "ID_PE_MODE_RAM"),
    (ID_PE_MODE_DISK, "ID_PE_MODE_DISK"),
    (ID_PE_DIR_LABEL, "ID_PE_DIR_LABEL"),
    (ID_PE_DIR_EDIT, "ID_PE_DIR_EDIT"),
    (ID_PE_NAME_LABEL, "ID_PE_NAME_LABEL"),
    (ID_PE_NAME_EDIT, "ID_PE_NAME_EDIT"),
    (ID_PE_DIR_BROWSE, "ID_PE_DIR_BROWSE"),
    (ID_PE_SYSINFO, "ID_PE_SYSINFO"),
    (ID_PE_SYSINFO_TEXT, "ID_PE_SYSINFO_TEXT"),
    (ID_PE_DLG_COMBO, "ID_PE_DLG_COMBO"),
    (ID_PE_DLG_EDIT, "ID_PE_DLG_EDIT"),
    (ID_PE_DLG_NAME, "ID_PE_DLG_NAME"),
    (ID_PE_DLG_OK, "ID_PE_DLG_OK"),
    (ID_PE_DLG_CANCEL, "ID_PE_DLG_CANCEL"),
    (ID_PE_DLG_LABEL1, "ID_PE_DLG_LABEL1"),
    (ID_PE_DLG_LABEL2, "ID_PE_DLG_LABEL2"),
    (ID_PE_DLG_LABEL3, "ID_PE_DLG_LABEL3"),
    (ID_PE_DLG_WARN, "ID_PE_DLG_WARN"),
    (ID_CHOICE_BODY, "ID_CHOICE_BODY"),
    (ID_CHOICE_PE, "ID_CHOICE_PE"),
    (ID_CHOICE_RE, "ID_CHOICE_RE"),
    (ID_CHOICE_CANCEL, "ID_CHOICE_CANCEL"),
    (ID_LANGUAGE_LABEL, "ID_LANGUAGE_LABEL"),
    (ID_SOURCE_DETAILS, "ID_SOURCE_DETAILS"),
    (ID_TARGET_DETAILS, "ID_TARGET_DETAILS"),
    (ID_CHOICE_CHOOSE_LOCATION, "ID_CHOICE_CHOOSE_LOCATION"),
    (ID_CHOICE_PATH_CANCEL, "ID_CHOICE_PATH_CANCEL"),
    (ID_SETTINGS_THRESHOLD_EDIT, "ID_SETTINGS_THRESHOLD_EDIT"),
    (ID_SETTINGS_ACTION_AUTO, "ID_SETTINGS_ACTION_AUTO"),
    (ID_SETTINGS_ACTION_PAUSE, "ID_SETTINGS_ACTION_PAUSE"),
    (ID_SETTINGS_SAVE, "ID_SETTINGS_SAVE"),
    (ID_SETTINGS_CANCEL, "ID_SETTINGS_CANCEL"),
    (ID_SETTINGS_HINT, "ID_SETTINGS_HINT"),
    (ID_SETTINGS_DETAIL_AUTO, "ID_SETTINGS_DETAIL_AUTO"),
    (ID_SETTINGS_DETAIL_PAUSE, "ID_SETTINGS_DETAIL_PAUSE"),
    (ID_SETTINGS_DISM_TIMEOUT_EDIT, "ID_SETTINGS_DISM_TIMEOUT_EDIT"),
    (ID_SETTINGS_DISM_TIMEOUT_HINT, "ID_SETTINGS_DISM_TIMEOUT_HINT"),
    (ID_SETTINGS_PAUSE_WAIT_EDIT, "ID_SETTINGS_PAUSE_WAIT_EDIT"),
];

/// GUI 的 `Language` → `pe_safety` 的 `MessageLanguage`（判据/文案的语言维度）。
///
/// 【2026-10-26】拆设置对话框时它一度跟着搬进子模块，但主窗口的 PE 安装对话框
/// 也在用 → 留在父模块（它是通用语言映射，不是设置专属）。
/// GUI 的 `Language` → `pe_safety` 的 `MessageLanguage`（判据/文案的语言维度）。
fn message_language(language: Language) -> crate::pe_safety::MessageLanguage {
    match language {
        Language::Chinese => crate::pe_safety::MessageLanguage::Chinese,
        Language::English => crate::pe_safety::MessageLanguage::English,
    }
}

/// 控件 ID → 可读名称；未知 ID 返回 `"?"`（查名字绝不能让日志失败）。
fn control_name(control_id: usize) -> &'static str {
    for (value, name) in CONTROL_NAMES {
        if *value == control_id {
            return name;
        }
    }
    "?"
}

/// GUI 操作审计：**每一次**用户操作在进业务分支之前先留一行痕。
///
/// 【为什么放在 WM_COMMAND 入口，而不是逐个分支补】逐个补会漏、也会随新功能
/// 腐化；放在入口则"所有操作"天然全覆盖，新加控件只要出现在 `CONTROL_NAMES`
/// 里就有名字（没有也只是显示 `?`，日志本身不会丢）。
///
/// 各分支原有的语义日志（如 `GUI action completed: ...`）**保留不动**，两者互补：
///   · 入口这行 = 用户当时**做了什么**（一定在，不看业务是否成功）
///   · 分支那行 = 那件事**做成了没有**（业务自己决定写什么）
///
/// 2026-09-30 的教训：用户报「点了没反应」，而日志里连一行都没有，分不清是
/// 点击根本没送达、还是送达后后续步骤静默失败——有这一行就能一眼区分。
/// 真正落盘的那一行。**不依赖 `State`**——对话框（RE 选择、无效路径选择、
/// 设置窗口）没有 `State` 可传，但它们同样需要留痕，所以取 `State` 之外的
/// 写法，让所有窗口过程都能调用同一个函数。
fn audit_control_action(control_id: usize, notification: usize) {
    let _ = super::append_log(
        &crate::log_root::logs_dir().join("gui.log"),
        &format!(
            "GUI action: control={} id={} notification=0x{:x}",
            control_name(control_id),
            control_id,
            notification
        ),
    );
}

unsafe fn install_tooltips(state: &mut State) {
    if !state.tooltip.is_null() {
        DestroyWindow(state.tooltip);
        state.tooltip = null_mut();
    }
    let tooltip_class = wide("tooltips_class32");
    let tooltip = CreateWindowExW(
        WS_EX_TOPMOST,
        tooltip_class.as_ptr(),
        null(),
        WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX | TTS_BALLOON,
        0,
        0,
        0,
        0,
        state.root,
        null_mut(),
        null_mut(),
        null_mut(),
    );
    if tooltip.is_null() {
        append_gui_log(state, "tooltip create failed (window null)");
        return;
    }
    let language = selected_language(state);
    let controls = [
        (state.controls.operation_tabs[0], "backup"),
        (state.controls.operation_tabs[1], "restore_a"),
        (state.controls.operation_tabs[2], "restore_b"),
        (state.controls.operation_tabs[3], "secondary"),
        (state.controls.operation_tabs[4], "pe"),
        (
            GetDlgItem(state.root, ID_CHANNEL_ONLINE as i32),
            "channel_online",
        ),
        (
            GetDlgItem(state.root, ID_CHANNEL_OFFLINE as i32),
            "channel_offline",
        ),
        (
            GetDlgItem(state.root, ID_FORMAT_TARGET as i32),
            "format_target",
        ),
        (state.controls.source, "source"),
        (state.controls.target, "target"),
        (state.controls.source_details, "source_details"),
        (state.controls.target_details, "target_details"),
        (state.controls.status, "status"),
        (state.controls.image, "image"),
        (state.controls.index, "index"),
        (state.controls.menu, "menu"),
        (state.controls.compress, "compress"),
        (state.controls.index_name, "index_name"),
        (state.controls.keep, "keep"),
        (state.controls.verify_hash, "verify_hash"),
        (GetDlgItem(state.root, ID_REFRESH as i32), "refresh"),
        (GetDlgItem(state.root, ID_READ_IMAGE as i32), "read_image"),
        (GetDlgItem(state.root, ID_CREATE_TASK as i32), "create_task"),
        (
            GetDlgItem(state.root, ID_REFRESH_TASK as i32),
            "refresh_task",
        ),
        (GetDlgItem(state.root, ID_BROWSE_IMAGE as i32), "browse"),
        (
            GetDlgItem(state.root, ID_PE_DIR_BROWSE as i32),
            "pe_dir_browse",
        ),
        (
            GetDlgItem(state.root, ID_PE_REBOOT_MAIN as i32),
            "pe_reboot_main",
        ),
        (GetDlgItem(state.root, ID_PE_SHORTCUT as i32), "pe_shortcut"),
        (GetDlgItem(state.root, ID_PE_MODE_RAM as i32), "pe_mode_ram"),
        (
            GetDlgItem(state.root, ID_PE_MODE_DISK as i32),
            "pe_mode_disk",
        ),
        (GetDlgItem(state.root, ID_PE_DIR_EDIT as i32), "pe_dir_edit"),
        (GetDlgItem(state.root, ID_LANGUAGE as i32), "language"),
        (GetDlgItem(state.root, ID_SETTINGS as i32), "settings"),
        (
            GetDlgItem(state.root, ID_CANCEL_PENDING as i32),
            "cancel_pending",
        ),
        (GetDlgItem(state.root, ID_VIEW_LOGS as i32), "view_logs"),
    ];
    let mut added = 0usize;
    let mut missing = 0usize;
    for (control, key) in controls {
        if control.is_null() {
            missing += 1;
            continue;
        }
        let text = tooltip_text(language, key);
        add_tooltip(tooltip, state.root, control, text);
        added += 1;
    }
    let tool_count = SendMessageW(tooltip, TTM_GETTOOLCOUNT, 0, 0) as usize;
    append_gui_log(
        state,
        &format!(
            "tooltips installed: added={added} missing={missing} tool_count={tool_count} balloon=true"
        ),
    );
    // Keep the tooltip handle alive by storing it in the state object. The
    // control owns the subclass hooks and automatically tracks child bounds.
    state.tooltip = tooltip;
}


// 文案表已抽到 `gui_text`（2026-10-26）：那是图形界面里唯一能在非 Windows 主机上
// 单测的部分。这里重导出，调用点一行不用改（沿用 `pe_safety` 的既有套路）。
pub(crate) use crate::gui_text::{tooltip_text, ui_text, Language};


#[derive(Clone, Debug)]
struct DriveInfo {
    letter: String,
    label: String,
    filesystem: String,
    size_bytes: Option<u64>,
    free_bytes: Option<u64>,
    volume_guid: String,
    disk_number: Option<u32>,
    partition_number: Option<u32>,
    partition_type_guid: String,
    has_windows_installation: bool,
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 固定 9pt（96dpi 基准）的界面字体。
///
/// 为什么不用 `GetStockObject(DEFAULT_GUI_FONT)`：进程声明 DPI 感知后，系统
/// 字体随 DPI 变大（200% 下约 18px），而所有布局坐标仍按 96dpi 写死，文字会
/// 溢出、按钮互压（2026-10-30 实测：顶栏按钮与入口按钮重叠）。布局缩放是下一
/// 步；在那之前先把字体钉在 96dpi 基准上，保证 100% 与 200% 下布局一致。
unsafe fn gui_font() -> Handle {
    // ★ 2.6.80：宋体（SimSun）→ 微软雅黑（Microsoft YaHei）。
    //   原因：用户反馈「字体看着不是特别清楚」。宋体是点阵/旧式字体，在 14px 下
    //   笔画发虚，而且**没有真正的粗体字形**——把它请求成 700 字重时视觉上几乎没变化
    //   （实测加粗前后模型判断均为「笔画较细」）。雅黑是矢量字体、自带 Bold，
    //   同样字号下更清晰饱满。字符集仍按 GB2312（134），与原来一致。
    let face = wide("Microsoft YaHei");
    // ★ 2.6.79：字号改为随 DPI 缩放（200% 下 14→28）。
    // ★ 2.6.80：基准从 12 提到 14（约 10.5pt）。12px 宋体在中文界面下确实偏小
    //   （用户 2026-10-30 反馈「仍然有点小」）。★ **改基准字号必须同步改下面
    //   那几个按「行距 × 行数」算出来的高度**（`details_height` / `status_height`
    //   与 ID_SOURCE_DETAILS / ID_TARGET_DETAILS / ID_STATUS 的创建高度），
    //   否则多行文本的最后一行会被切成半截（2026-10-26 踩过，见 `details_height` 注释）。
    // ★ 2.6.80：字重 400 → 700（粗体）。用户反馈「字体看着不是特别清楚」。
    //   注意：宋体本身没有真正的粗体字形，Windows 会对笔画做合成加粗，
    //   若加粗后反而发糊，下一步应改为换字体族（微软雅黑）而不是继续加粗。
    // ★ 2.6.88：字重 700 → 400（**加粗改回常规**）。用户 2026-10-30 明确要求
    //   「把字体都改成微软雅黑不加粗」。字体族保持微软雅黑不变，只去掉加粗。
    //   注意：字体一变，所有「按实测文字宽度算出来的控件宽度」都会跟着变
    //   （常规体比粗体窄），窗口/标签会略微收窄——这是预期的，不需要补偿；
    //   外部判据（`.test-artifacts/dpi-2679/*.ps1`）里重建字体的字重也必须
    //   同步改成 400，否则会拿粗体去量常规体的文字，误报「被切」。
    CreateFontW(
        // ★ 2.6.81：字号改为读「用户在设置里选的值」，不再写死。
        -crate::dpi::s(crate::dpi::font_size() as i32),
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        134,
        0,
        0,
        0,
        0,
        face.as_ptr(),
    )
}

/// 用当前界面字体**实测**一段文字在给定宽度下需要占多大。
///
/// 返回 `(宽, 高)`，单位是 96dpi 基准像素（与写死的布局坐标同一空间）；
/// `wrap = true` 时按 `width96` 折行算多行高度，`false` 时按单行算宽度。
///
/// ## 为什么必须实测，不能按「行距 × 行数」估
///
/// 2026-10-30 用户反馈「设置界面过小」时修完窗口尺寸后暴露出更底层的问题：
/// 设置对话框里每块说明文字的高度与下一个控件的 y 全是写死的（40/56/44），
/// 那是按 14 号字折两行算出来的。2.6.81 允许用户把字号调到 16/18 之后，
/// 同一段文字在 18 号下折成三行，第三行画到控件矩形**外面**，又被下一个
/// 控件的背景盖掉上半截——截图里「并重启回正常 Windows」只剩半行字，且压在
/// 下面那个单选按钮上。折几行由「字体 × 宽度 × 文案」三者共同决定（文案还随
/// 界面语言变），所以只能让 GDI 用同一套字体真算一遍。
///
/// 用屏幕 DC（`GetDC(NULL)`）而不是窗口 DC：这样窗口还没创建也能量，
/// 布局可以在创建控件之前就把高度算准。
pub(crate) unsafe fn measure_text(
    text: &str,
    width96: i32,
    wrap: bool,
) -> (i32, i32) {
    // DrawTextW 的格式位（winuser.h）：只在本函数用到，就近定义。
    const DT_CALCRECT: u32 = 0x0400;
    const DT_WORDBREAK: u32 = 0x0010;
    const DT_SINGLELINE: u32 = 0x0020;
    const DT_NOPREFIX: u32 = 0x0800;
    let font = gui_font();
    let hdc = GetDC(null_mut());
    if hdc.is_null() || font.is_null() {
        // 量不出来就退回「一行高度」，至少不会算出负数或 0。
        return (width96, crate::dpi::line_height());
    }
    let previous = SelectObject(hdc, font);
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: crate::dpi::s(width96),
        bottom: 0,
    };
    let format = DT_CALCRECT
        | DT_NOPREFIX
        | if wrap { DT_WORDBREAK } else { DT_SINGLELINE };
    let text = wide(text);
    DrawTextW(hdc, text.as_ptr(), -1, &mut rect, format);
    SelectObject(hdc, previous);
    // 屏幕 DC 用完必须归还：GetDC 每次都会给一个新的 DC，不还就漏（每开一次
    // 对话框要量十几段文字，漏 DC 会一路累积到 GDI 对象上限）。
    ReleaseDC(null_mut(), hdc);
    // 字体是本函数自己新建的，量完就删；不删的话每开一次对话框漏十几个 GDI 对象。
    DeleteObject(font);
    let width = crate::dpi::unscale(rect.right - rect.left);
    let height = crate::dpi::unscale(rect.bottom - rect.top).max(crate::dpi::line_height());
    (width.max(0), height)
}

/// 单行文字实测宽度（96dpi 基准像素）。用于「标签 + 输入框 + 后缀」这种
/// 同行排布：标签宽度随文案和字号变，写死就会把输入框压住或者把后缀挤出去。
///
/// ## 为什么不用 `DrawTextW(DT_CALCRECT)` 量宽度
///
/// 2026-10-30 实测（200% 缩放、微软雅黑粗体）：`DT_CALCRECT` 报的宽度**比真实排版窄**，
/// 而且窄多少**随字符串变**——强的窄 8 个基准像素（「还原数据到非 C 盘」报 298、实排 316），
/// 弱的窄 20（「Interface font size:」报 170、实排 190）。先试着补一个固定 14，
/// 结果仍然有两处各差 3~6 像素被判据抓到——固定补偿这条路走不通。
///
/// 所以宽度改用 `GetTextExtentPoint32W`：它就是排版时用来取字符串尺寸的那个接口，
/// 配同一个字体、同一块屏幕 DC，量出来的值直接就是绘制宽度。
///
/// 高度仍然走 `measure_text`（`DT_CALCRECT|DT_WORDBREAK`）：那个方向没观察到偏差，
/// 而且折行只有 `DrawText` 会算。
pub(crate) unsafe fn measure_text_width(text: &str) -> i32 {
    /// ClearType 取整可能差 1~2 像素，留一点余量，避免控件边框压住最后一个字。
    const SAFETY: i32 = 4;
    let font = gui_font();
    let hdc = GetDC(null_mut());
    if hdc.is_null() || font.is_null() {
        return SAFETY;
    }
    let previous = SelectObject(hdc, font);
    let wide = wide(text);
    let mut size = TextSize { cx: 0, cy: 0 };
    let ok = GetTextExtentPoint32W(hdc, wide.as_ptr(), wide.len() as i32 - 1, &mut size) != 0;
    SelectObject(hdc, previous);
    ReleaseDC(null_mut(), hdc);
    DeleteObject(font);
    if !ok {
        return SAFETY;
    }
    crate::dpi::unscale(size.cx).max(0) + SAFETY
}

/// 多行文字实测高度（96dpi 基准像素），按 `width96` 折行。
pub(crate) unsafe fn measure_text_height(text: &str, width96: i32) -> i32 {
    measure_text(text, width96, true).1
}

// Win32 control creation mirrors CreateWindowExW's explicit rectangle and id.
#[allow(clippy::too_many_arguments)]
unsafe fn create_control(
    parent: Hwnd,
    class: &str,
    text: &str,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: usize,
) -> Hwnd {
    let class = wide(class);
    let text = wide(text);
    let handle = CreateWindowExW(
        0,
        class.as_ptr(),
        text.as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        // ★ 2.6.79：所有控件坐标统一按 DPI 系数缩放。这里是 71 个控件的唯一
        //   创建入口，在一处乘系数即可覆盖全部静态布局；动态重排走
        //   `layout_operation` 的 `reposition`（同样已缩放）。
        crate::dpi::s(x),
        crate::dpi::s(y),
        crate::dpi::s(width),
        crate::dpi::s(height),
        parent,
        id as HMenu,
        null_mut(),
        null_mut(),
    );
    let font = gui_font();
    if !handle.is_null() && !font.is_null() {
        SendMessageW(handle, WM_SETFONT, font as usize, 1);
    }
    handle
}

unsafe fn add_tooltip(tooltip: Hwnd, parent: Hwnd, control: Hwnd, text: &str) {
    if tooltip.is_null() || control.is_null() {
        return;
    }
    // Tooltip controls retain the text pointer, so deliberately leak these
    // tiny immutable UTF-16 strings for the lifetime of the GUI process.
    let text = Box::leak(wide(text).into_boxed_slice());
    let info = ToolInfoW {
        cb_size: size_of::<ToolInfoW>() as u32,
        u_flags: TTF_IDISHWND | TTF_SUBCLASS,
        hwnd: parent,
        u_id: control as usize,
        rect: Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        hinst: null_mut(),
        lpsz_text: text.as_ptr(),
        l_param: 0,
    };
    SendMessageW(tooltip, TTM_ADDTOOLW, 0, &info as *const ToolInfoW as isize);
}

unsafe fn set_text(hwnd: Hwnd, value: &str) {
    // Win32 EDIT controls require CRLF for explicit line breaks; the
    // normalization itself lives in `text_parsing` so it is covered by the
    // offline tests. Keeping the conversion here makes guidance/details boxes
    // render logical lines consistently instead of depending on wrapping.
    let value = wide(&crate::text_parsing::normalize_edit_newlines(value));
    SetWindowTextW(hwnd, value.as_ptr());
    // 强制立即重绘（历史 bug 根因修复）：SetWindowTextW 只把控件区域标为
    // 异步失效，快速连续切换 tab 时 WM_PAINT 会被合并/延迟，旧文本会残留
    // 在控件区域直到下一次重绘——表现为「PE tab 镜像标签行叠出上一 tab
    // 的 status 文本」。这里立即失效并同步更新，保证文本层与像素层一致。
    InvalidateRect(hwnd, null(), 1);
    UpdateWindow(hwnd);
}

unsafe fn get_text(hwnd: Hwnd) -> String {
    // GetWindowTextW is the supported cross-control API for retrieving text
    // from EDIT/COMBOBOX controls.  Some Windows builds return zero for a
    // cross-thread WM_GETTEXTLENGTH even though the control visibly contains
    // text; relying on that message made a filled image path look empty to
    // the Rust GUI.  Read the exact length first, then use the reported count
    // so embedded NULs cannot leak into validation.
    let reported_length = GetWindowTextLengthW(hwnd);
    // A few native controls report zero from GetWindowTextLengthW while
    // still returning their text from GetWindowTextW. Use a bounded fallback
    // buffer in that case instead of treating the field as empty.
    let capacity = if reported_length > 0 {
        reported_length as usize + 1
    } else {
        32 * 1024
    };
    let mut buffer = vec![0u16; capacity];
    let mut written = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    // GetWindowTextW is not guaranteed to marshal EDIT text across every
    // Windows integrity/thread boundary. If it reports no characters, retry
    // with the control messages themselves and the same bounded buffer. This
    // keeps real user-entered paths readable without an unbounded allocation.
    if written == 0 {
        let message_length = SendMessageW(hwnd, 0x000e, 0, 0).max(0) as usize;
        let message_capacity = message_length.saturating_add(1).clamp(1, 32 * 1024);
        if message_capacity != buffer.len() {
            buffer.resize(message_capacity, 0);
        }
        written = SendMessageW(hwnd, 0x000d, buffer.len(), buffer.as_mut_ptr() as isize) as i32;
    }
    let written = written.max(0) as usize;
    String::from_utf16_lossy(&buffer[..written.min(buffer.len())])
}

unsafe fn add_combo_item(hwnd: Hwnd, value: &str) {
    let value = wide(value);
    SendMessageW(hwnd, CB_ADDSTRING, 0, value.as_ptr() as isize);
}

unsafe fn reset_combo(hwnd: Hwnd) {
    SendMessageW(hwnd, CB_RESETCONTENT, 0, 0);
}

unsafe fn combo_index(hwnd: Hwnd) -> usize {
    let index = SendMessageW(hwnd, CB_GETCURSEL, 0, 0);
    if index < 0 { 0 } else { index as usize }
}

unsafe fn combo_selection(hwnd: Hwnd) -> Option<usize> {
    let index = SendMessageW(hwnd, CB_GETCURSEL, 0, 0);
    if index < 0 {
        None
    } else {
        Some(index as usize)
    }
}

unsafe fn selected_language(state: &State) -> Language {
    if combo_index(state.controls.language) == 1 {
        Language::English
    } else {
        Language::Chinese
    }
}

unsafe fn set_child_text(parent: Hwnd, id: usize, value: &str) {
    let child = GetDlgItem(parent, id as i32);
    if !child.is_null() {
        set_text(child, value);
    }
}

unsafe fn selected_operation(state: &State) -> &'static str {
    match state.operation_index {
        0 => "backup",
        // 三入口拆分：1=A 还原系统到 C 盘（沿用 restore-existing），
        // 2=B 还原数据到非 C 盘，3=C 还原系统到非 C 盘。
        1 => "restore-existing",
        2 => "restore-data",
        3 => "create-secondary",
        4 => "install-pe-entry",
        _ => "backup",
    }
}

/// GUI 入口键 → CLI `prepare --operation` 值。
/// B 复用 restore-existing（prepare 链要求 source==target，RE 内数据卷分支不碰引导）；
/// C 复用 create-secondary（目标≠活动系统、需菜单名、NewWindows 收尾）——
/// 2.4.9 起收尾按「目标性质」路由，operation 只表达语义，这里仅做入口归一。
fn cli_operation(gui: &str) -> &'static str {
    match gui {
        "restore-data" => "restore-existing",
        "create-secondary" => "create-secondary",
        "backup" => "backup",
        "restore-existing" => "restore-existing",
        "install-pe-entry" => "install-pe-entry",
        "probe" => "probe",
        _ => "backup",
    }
}

/// 是否「镜像还原类」入口（A/B/C + 新增第二系统）：

/// 共用 WIM 索引必选、破坏性确认、副档校验、--allow-destructive 等闸门。
fn is_image_restore_entry(gui: &str) -> bool {
    matches!(
        gui,
        "restore-existing" | "restore-data" | "create-secondary"
    )
}

/// 「格式化目标分区」复选框的入口集合：A/B/C 三个还原入口。
/// set_operation_visibility 的可见性列表与 layout_operation 的重排条件
/// **必须共用本函数**：2026-10-17 修复的 C 页遮挡 bug 即因
/// v2.6.4 合并入口时可见性列表把 restore-system-other 改名 create-secondary、
/// 而 layout 的排除条件 `!= "create-secondary"` 漏改——复选框显示却不参与
/// 排版，停在后创建控件的下层被按钮行/详情框盖住。单一来源杜绝再次漂移。
fn is_format_target_entry(gui: &str) -> bool {
    matches!(
        gui,
        "restore-existing" | "restore-data" | "create-secondary"
    )
}

/// 当前活动系统盘符（`%SystemDrive%`，无冒号大写）。
/// GUI 侧只用于锁定 A 的目标、B/C 的目标预检——真正的卷身份比对在
/// prepare 的 P0 安全闸里做，这里不做安全承诺。
fn active_system_letter() -> String {
    std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".to_string())
        .trim_end_matches('\\')
        .trim_end_matches(':')
        .to_ascii_uppercase()
}

/// 当前活动系统盘符（`%SystemDrive%`，大写字母），**取不到就返回 `None`**。
///
/// 与 `active_system_letter` 的区别：那个在环境变量缺失时退回 `"C"`（只为界面
/// 显示一个盘符，错了无害）；这个用于**安全判定**，取不到时必须如实说"不知道"，
/// 绝不能退回字母 C——写死 C 会把"系统盘不是 C"的机器漏掉、把"C 是数据盘"的
/// 机器误拦，正是 `install_pe_harddisk` 里那处 `drive_char == 'C'` 的老问题。
fn system_drive_letter() -> Option<char> {
    std::env::var("SystemDrive")
        .ok()
        .and_then(|value| {
            value
                .trim_end_matches('\\')
                .trim_end_matches(':')
                .chars()
                .next()
        })
        .map(|letter| letter.to_ascii_uppercase())
}

/// 该卷是不是 EFI 系统分区（装引导文件的那个小分区）。
///
/// 判据是分区类型 GUID `EFI_TYPE`，不是盘符。
fn is_efi_system_partition(drive: Option<&DriveInfo>) -> bool {
    drive
        .map(|drive| {
            drive
                .partition_type_guid
                .eq_ignore_ascii_case(crate::windows_prepare::EFI_TYPE)
        })
        .unwrap_or(false)
}

/// 在运行本程序的那个卷上？程序所在卷不能被当成 PE 安装目标——装进去会覆盖
/// 正在运行的程序自身。
fn is_program_drive(drive_char: char, executable_dir: &std::path::Path) -> bool {
    match executable_dir.to_string_lossy().chars().next() {
        Some(letter) => letter.eq_ignore_ascii_case(&drive_char),
        // 拿不到程序路径时不猜：既不能误判成"是"，也不能假设成 C。
        None => false,
    }
}

unsafe fn set_operation_tabs(state: &State, language: Language) {
    for (index, (button, key)) in state
        .controls
        .operation_tabs
        .iter()
        .zip([
            "backup", "restore_a", "restore_b", "secondary", "pe",
        ])
        .enumerate()
    {
        set_text(*button, ui_text(language, key));
        append_gui_log(
            state,
            &format!("PE dev: tab[{index}] key={key} hwnd={button:?} text_set"),
        );
        SendMessageW(
            *button,
            BM_SETCHECK,
            if index == state.operation_index {
                BST_CHECKED
            } else {
                BST_UNCHECKED
            },
            0,
        );
    }
}

/// 「操作模式」那一排入口按钮（五个「按下去像页签」的单选按钮）的横向排布。
///
/// ## 为什么必须实测文字宽度，不能写死
///
/// ★ 2.6.83 修缺陷。这五个按钮的宽度是**创建时写死**的
/// （56 / 120 / 120 / 150 / 72，x 从 90 起每隔 3px 排下去），那是按 14 号字
/// 的中文文案估出来的。用户能在设置里把字号调到 16 / 18 之后，三个问题同时出现：
///
/// 1. 文字整体变宽，「新增非 C 盘第二系统」这种 9 字长文案直接顶出按钮矩形，
///    相邻两个按钮的字贴在一起，看上去像一串挤在一块的标签；
/// 2. 左边「操作模式」这个标签只有 62px 宽，18 号字下四个汉字要 70 多像素，
///    末字被切掉；
/// 3. 英文界面同理，而且更严重——「Add second Windows system off C:」
///    比任何中文文案都长。
///
/// 所以这里改成：**让 GDI 按当前字体把每段文案真正量一遍**（`measure_text_width`），
/// 加左右内边距当按钮宽度，再依次往右排。文案随语言切换、字号随用户选择变，
/// 量出来的宽度自然跟着变，不需要再维护一张「哪个语言多宽」的表。
///
/// 这一行的高度（32）不随字号变：18 号字行距是 21，装进 32 仍有余量，
/// 下面的控件 y 坐标链（`layout_operation` 里从 y=100 起）也就不必跟着改。
unsafe fn layout_operation_tabs(state: &State) {
    /// 文字左右各留的空白：按钮边框 + 一点视觉呼吸。太窄会让字贴着边框。
    const TAB_SIDE_PADDING: i32 = 9;
    // 注：`measure_text_width` 走的是 `GetTextExtentPoint32W`，量出来的就是
    // 真实绘制宽度（另加 4 px 防 ClearType 取整），这里只加视觉内边距。
    /// 相邻两个按钮之间的空隙。
    const TAB_GAP: i32 = 3;
    /// 最小宽度：「备份」这种两字文案也至少要有这么宽，否则看着不像按钮。
    const TAB_MIN_WIDTH: i32 = 56;
    const TOP: i32 = 60;
    const HEIGHT: i32 = 32;
    /// 「操作模式」标签与第一个按钮之间的空隙。
    const LABEL_GAP: i32 = 8;

    let move_to = |hwnd: Hwnd, x: i32, width: i32| {
        if !hwnd.is_null() {
            MoveWindow(
                hwnd,
                crate::dpi::s(x),
                crate::dpi::s(TOP),
                crate::dpi::s(width),
                crate::dpi::s(HEIGHT),
                1,
            );
        }
    };

    let mut x = 20;
    // 先排「操作模式」标签：它的宽度也按当前文案实测（中文 4 字 / 英文 "Operation"
    // 差别很大），不再固定 62。控件 id 2001 是 `WM_CREATE` 里创建的那个 STATIC。
    let label = GetDlgItem(state.root, 2001);
    if !label.is_null() {
        let text = get_text(label);
        let width = (measure_text_width(&text) + TAB_SIDE_PADDING * 2).max(40);
        move_to(label, x, width);
        x += width + LABEL_GAP;
    }
    for button in state.controls.operation_tabs.iter() {
        let text = get_text(*button);
        let width = (measure_text_width(&text) + TAB_SIDE_PADDING * 2).max(TAB_MIN_WIDTH);
        move_to(*button, x, width);
        x += width + TAB_GAP;
    }
    // 只在右边缘真的变了才记日志：`layout_operation` 每次 WM_SIZE 都会跑，
    // 无条件下日志会在拖拽窗口时把 GUI 日志刷满。
    let right_edge = x - TAB_GAP;
    static LAST_EDGE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
    if LAST_EDGE.swap(right_edge, std::sync::atomic::Ordering::SeqCst) != right_edge {
        append_gui_log(
            state,
            &format!("layout: operation tabs right edge={right_edge} (96dpi base)"),
        );
    }
}

fn operation_display(language: Language, operation: &str) -> &'static str {
    match (language, operation) {
        (Language::Chinese, "probe") => "探测（仅检查）",
        (Language::Chinese, "backup") => "备份",
        (Language::Chinese, "restore-existing") => "还原系统到 C 盘",
        (Language::Chinese, "restore-data") => "还原数据到非 C 盘",
        (Language::Chinese, "create-secondary") => "新增非 C 盘第二系统",
        (Language::Chinese, "install-pe-entry") => "安装 PE",
        (Language::English, "probe") => "Inspect",
        (Language::English, "backup") => "Backup",
        (Language::English, "restore-existing") => "Restore system to C:",
        (Language::English, "restore-data") => "Restore data to non-C:",
        (Language::English, "create-secondary") => "Add second system (non-C:)",
        (Language::English, "install-pe-entry") => "Install PE",
        _ => "",
    }
}

fn operation_hint_key(operation: &str) -> &'static str {
    match operation {
        "backup" => "backup_hint",
        "restore-existing" => "restore_hint",
        "restore-data" => "restore_b_hint",
        "create-secondary" => "secondary_hint",
        "install-pe-entry" => "pe_hint",
        _ => "backup_hint",
    }
}

unsafe fn set_operation_guidance(state: &State) {
    let language = selected_language(state);
    set_text(
        state.controls.status,
        ui_text(language, operation_hint_key(selected_operation(state))),
    );
}

unsafe fn set_operation_visibility(state: &State) {
    let operation = selected_operation(state);
    let show_image = operation != "probe";
    let show_target = matches!(
        operation,
        "restore-existing" | "restore-data" | "create-secondary"
            | "install-pe-entry"
    );
    // A/B/C/新增第二系统都要手选 WIM 索引 + 副档校验复选框。
    let show_index = is_image_restore_entry(operation);
    // 源卷行只在「备份」出现（选备份来源）。还原类一律隐藏：A/B 源=目标、
    // C 源恒为当前系统盘（active_system_letter()，创建任务处读取，不给用户
    // 改的机会），显示一个改不了的卷只会误导（v2.6.13 起 C 页移除源卷行）。
    let show_source = operation == "backup";
    // WIM 压缩方式（fast/none）只对备份首次创建有意义，仅「备份」tab 显示。
    let show_compress = operation == "backup";
    // 「第二系统名称」在「新增第二系统」与 C（他盘系统还原，离线 prepare 必填）
    // 显示；「安装 PE」tab 有自己的「PE 启动项名称」输入框，若此处也显示 menu，
    // 其布局位置（field_x+500, secondary_y）会恰好覆盖右下角「创建快捷方式」按钮。
    let show_menu = matches!(operation, "create-secondary");
    // B/C 入口显示「在线/离线」通道单选；A 必离线（无选择），备份走智能分流。
    let show_channel = matches!(operation, "restore-data" | "create-secondary");
    let set_visible = |hwnd: Hwnd, visible: bool| {
        ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
    };
    let set_child_visible = |id: i32, visible: bool| {
        set_visible(GetDlgItem(state.root, id), visible);
    };

    set_visible(state.controls.source, show_source);
    set_visible(state.controls.source_details, show_source);
    set_child_visible(2003, show_source);

    set_visible(state.controls.image, show_image);
    set_child_visible(ID_BROWSE_IMAGE as i32, show_image);
    set_child_visible(2004, show_image);

    set_visible(state.controls.index, show_index);
    set_visible(state.controls.verify_hash, show_index);
    set_child_visible(2007, show_index);
    set_visible(state.controls.menu, show_menu);
    set_child_visible(2008, show_menu);
    set_visible(state.controls.compress, show_compress);
    set_child_visible(2014, show_compress);
    // 备份索引名与保留最近 N 个：仅「备份」tab 显示（压缩率行下方新行）。
    set_visible(state.controls.index_name, show_compress);
    set_visible(state.controls.keep, show_compress);
    set_child_visible(2015, show_compress);
    set_child_visible(2016, show_compress);

    set_visible(state.controls.target, show_target);
    set_visible(state.controls.target_details, show_target);
    set_child_visible(2005, show_target);
    // A（还原系统到 C 盘）：目标锁定为当前系统盘，禁用下拉防误选；
    // 界面仍显示实际盘符（select_operation 会自动选中 %SystemDrive%）。
    EnableWindow(
        state.controls.target,
        if operation == "restore-existing" { 0 } else { 1 },
    );
    // B/C 的通道单选；「格式化目标分区」复选框在三个还原入口（A/B/C）都显示
    //（2026-10-14 用户拍板：所有还原都要有格式化选项，在线离线全通道生效）。
    set_child_visible(ID_CHANNEL_ONLINE as i32, show_channel);
    set_child_visible(ID_CHANNEL_OFFLINE as i32, show_channel);
    set_child_visible(ID_FORMAT_TARGET as i32, is_format_target_entry(operation));
    // 「重启进入 PE」「创建快捷方式」只在 安装 PE tab 显示
    set_child_visible(ID_PE_REBOOT_MAIN as i32, operation == "install-pe-entry");
    set_child_visible(ID_PE_SHORTCUT as i32, operation == "install-pe-entry");
    // 启动方式单选与目录行只在「安装 PE」tab 显示；硬盘启动模式下隐藏目录路径行
    let pe_mode_ram = IsDlgButtonChecked(state.root, ID_PE_MODE_RAM as i32) != 0;
    let pe_visible = operation == "install-pe-entry";
    append_gui_log(
        state,
        &format!("PE visibility: op={operation} pe_visible={pe_visible} ram_checked={pe_mode_ram}"),
    );
    set_child_visible(ID_PE_MODE_RAM as i32, pe_visible);
    set_child_visible(ID_PE_MODE_DISK as i32, pe_visible);
    // 「PE 启动项名称」输入框两种模式通用，在「安装 PE」tab 始终显示
    set_child_visible(ID_PE_NAME_LABEL as i32, pe_visible);
    set_child_visible(ID_PE_NAME_EDIT as i32, pe_visible);
    // PE 目录路径仅 RAM disk 模式需要（WIM 复制目录），硬盘启动模式隐藏
    set_child_visible(ID_PE_DIR_LABEL as i32, pe_visible && pe_mode_ram);
    set_child_visible(ID_PE_DIR_EDIT as i32, pe_visible && pe_mode_ram);
    set_child_visible(ID_PE_DIR_BROWSE as i32, pe_visible && pe_mode_ram);
}

/// 顶栏操作按钮（查看日志 / 备份文件管理… / 取消待重启任务 / 设置… / 语言）
/// 独立成一行，放在客户区顶部这条原本空着的窄带里。
///
/// ★ 2026-10-26 修缺陷：此前这 5 个控件与「操作模式」的入口行**同在 y=60**，
/// 两组横向总宽约 90+650(入口) + 4 按钮+语言约 750 = 远超任何窗口宽度，于是
/// 顶栏按钮被画在入口按钮上面。实机 GetWindowRect 量到 **6 对重叠**，最严重的是
/// 「新增非 C 盘第二系统」被「查看日志」盖掉 107×24 px（占该按钮宽度 71%）。
/// 当时的注释写着「最小窗口 1020 宽下两者不重叠」，而它只对「入口排到 ~741、
/// 语言栏从右边算起」成立——没算后来加进来的四个顶栏按钮。教训：两排控件共用
/// 一个 y 时，必须按**最坏宽度**（最小窗口 + 最长语言）核算，不能只看各自的起点。
const TOP_BAR_Y: i32 = 18;

unsafe fn layout_operation(state: &State) {
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 1020,
        bottom: 760,
    };
    GetClientRect(state.root, &mut rect);
    // ★ 2.6.79：把实际客户区换算回 96dpi 基准空间，下面这一大段重排常量
    //   （右对齐的「客户宽 − 20 − 总宽」之类）一概不动，仍按 96dpi 书写；
    //   最终由 `reposition` 统一乘系数输出物理像素。
    //   为什么必须先换算：客户区读出来已经是放大后的值，若在物理量上再整体乘系数，
    //   右对齐会算出「(2040−20−241)×2」这种翻倍的错误位置。
    //   100% 下 `unscale` 是恒等，结果与改动前逐像素相同。
    let client_width = crate::dpi::unscale(rect.right - rect.left).max(1020);
    // 「操作模式」那一排页签的横向排布：宽度按当前字号/语言实测，见函数注释。
    // 放在这里而不是只在语言切换时调用，是因为**改字号**也要重排
    // （`apply_font_size` 也是走 `layout_operation`）。
    layout_operation_tabs(state);
    let field_x = 180;
    let field_width = (client_width - field_x - 24).max(700);
    // ★ 高度按「**最坏行数 × 行距**」算，不能按某一页的行数算。
    // ★ 2.6.80：基准字号 12→14，行距随之 **14 px → 17 px**（tmHeight 14 + tmExternalLeading 3）。
    //   原行距 = SimSun 9pt 的 tmHeight 12 + tmExternalLeading 2 = 14 px（2026-10-26 实测：
    //   按 tmHeight 12 算会得出「9 行只要 108 px、装得下」，而屏幕上最后一行被切成半截）。
    // 最坏情况 = 「还原数据到非 C 盘」页的目标卷详情 **10 行**（该页设备详情有一行
    // 954 px > 可用 950 px，软换行多出一行）→ 需 140 px；EM_GETRECT 实测文本区比控件
    // 高少 4 px（上下边框各 2）→ 148 留 8 px 余量。其余页 9 行（=126 px）都在其内。
    // ★ 往 `drive_details` 加字段就要同步这里，否则又是一次「最后一行看不到」。
    // ★ 2.6.81：按**当前字号**算（最坏 10 行 + 8 余量），不再写死 178。
    //   字号可在设置里改，行距随之变，这里必须跟着变，否则最后一行被切。
    let details_height = crate::dpi::text_block_height(10, 8);
    let row_gap = 8;
    // 行可见性：源卷行仅「备份」显示（A/B/C 隐藏——C 源恒为当前系统盘，
    // v2.6.13 起从 C 页移除），通道单选行仅 B/C 显示。隐藏的行不占位，
    // 目标行相应上移。
    let layout_op = selected_operation(state);
    let source_visible = layout_op == "backup";
    let target_visible = matches!(
        layout_op,
        "restore-existing" | "restore-data" | "create-secondary"
            | "install-pe-entry"
    );
    let image_visible = layout_op != "probe";
    let index_visible = is_image_restore_entry(layout_op);
    let channel_visible = matches!(layout_op, "restore-data" | "create-secondary");
    let source_combo_y = 100;
    let source_details_y = source_combo_y + 36;
    let target_combo_y = if source_visible {
        source_details_y + details_height + row_gap
    } else {
        source_combo_y
    };
    let target_details_y = target_combo_y + 36;
    // 通道单选行：目标卷详情框下方；三个还原入口再加一行「格式化目标分区」复选框
    let channel_y = target_details_y + details_height + 8;
    let show_format_target = is_format_target_entry(layout_op);
    let format_y = channel_y + 26;
    let last_details_y = if show_format_target {
        format_y + 24
    } else if channel_visible {
        channel_y + 24
    } else if target_visible {
        target_details_y + details_height
    } else {
        source_details_y + details_height
    };
    // 「安装 PE」tab 的启动方式单选占三行（约 90px：单选行 + 启动项名行 + 目录行），后续 status 下移
    let pe_extra_y = if selected_operation(state) == "install-pe-entry" {
        90
    } else {
        0
    };
    // 状态提示框的高度：9 行文字 + 边框与余量，随字号变。
    // ★ 必须在这里先算出来：下面每一行的 y 都要让过它。
    //   2.6.80 把基准字号 12→14 之后这个高度从 148 涨到 178，但 `image_y` 仍按
    //   「status_y + 168」排（168 是「148 + 20 空隙」的老值），于是状态框把
    //   「镜像绝对路径」那一行的顶部压住了 10 个基准像素（200% 下 20 物理像素）。
    //   2026-10-30 用户截图指出「镜像绝对路径这一行顶部被稍微遮挡了」。
    //   教训与 2.6.83 同源：**高度一改，靠它算出来的每一个 y 都得重新审视**。
    let status_height = crate::dpi::text_block_height(9, 25);
    let status_y = last_details_y + 16 + pe_extra_y;
    // 状态框与下一行之间的空隙。原来是 20（= 168 − 148）；现在按实测高度算，
    // 8 已经够呼吸，也少往下推一些，免得整窗底部贴到屏幕边。
    let status_gap = 8;
    let image_y = status_y + status_height + status_gap;
    let secondary_y = image_y + 34;
    // C（还原系统到非 C 盘）与「新增第二系统」的 verify_hash 复选框在
    // secondary_y+32（底缘 +56）——按钮行必须让过它，留 8px 间隙。
    // （2.5.2 及此前按钮行在 +40，与复选框重叠 16px，「严格对比哈希」被挡住。）
    let buttons_y = if index_visible {
        if matches!(
            selected_operation(state),
            "create-secondary"
        ) {
            secondary_y + 64
        } else {
            secondary_y + 40
        }
    } else if selected_operation(state) == "backup" {
        // 备份时 secondary_y 行独占「压缩率」下拉，下一行是「索引名 + 保留最近 N 个」，
        // 按钮行再下移 30px 避免遮挡。
        secondary_y + 40 + 30
    } else if image_visible {
        image_y + 40
    } else {
        // 「探测」页没有镜像行，按钮直接跟在状态框下面——同样必须让过实测高度。
        status_y + status_height + status_gap
    };

    let reposition = |hwnd: Hwnd, x: i32, y: i32, width: i32, height: i32| {
        if !hwnd.is_null() {
            // ★ 2.6.79：入参都是 96dpi 基准值（含由 `client_width` 推出的结果），
            //   这里统一换算成物理像素。这一处覆盖本函数全部动态重排，
            //   所以 530 行常量不需要逐个改写。
            MoveWindow(
                hwnd,
                crate::dpi::s(x),
                crate::dpi::s(y),
                crate::dpi::s(width),
                crate::dpi::s(height),
                1,
            );
        }
    };
    // 语言栏靠右显示：标签(55) + 间距(6) + combo(180)，combo 右缘 = 客户宽 - 20。
    // 创建时的静态坐标只对最小宽 1020 成立，这里按实际客户宽动态右对齐
    //（覆盖最大化/还原/拖拽改尺寸等所有窗口状态）。
    let lang_combo_w = 180;
    let lang_total = 55 + 6 + lang_combo_w;
    reposition(
        GetDlgItem(state.root, ID_LANGUAGE_LABEL as i32),
        client_width - 20 - lang_total,
        TOP_BAR_Y + 7,
        55,
        18,
    );
    reposition(
        state.controls.language,
        client_width - 20 - lang_combo_w,
        TOP_BAR_Y,
        lang_combo_w,
        300,
    );
    // 「设置…」按钮紧靠语言栏左侧、同排右对齐（客户宽变化时一起跟随）。
    let settings_w = 90;
    let settings_x = client_width - 20 - lang_total - 8 - settings_w;
    reposition(
        GetDlgItem(state.root, ID_SETTINGS as i32),
        settings_x,
        TOP_BAR_Y,
        settings_w,
        24,
    );
    // 「取消待重启任务」在「设置…」左侧，同排右对齐。
    // 2026-10-17 教训：这个按钮最初漏了这条重排，停留在创建坐标 760，把语言
    // 标签和 combo 压掉半个 —— 顶栏三个右对齐控件必须一起进 layout_operation。
    let cancel_w = 150;
    let cancel_x = settings_x - 8 - cancel_w;
    reposition(
        GetDlgItem(state.root, ID_CANCEL_PENDING as i32),
        cancel_x,
        TOP_BAR_Y,
        cancel_w,
        24,
    );
    // 「备份文件管理…」在「取消待重启任务」左侧，同排右对齐（同一链条，
    // 漏了重排就会压在邻接控件上——2026-10-17 的教训对每个新顶栏按钮都成立）。
    let image_admin_w = 140;
    let image_admin_x = cancel_x - 8 - image_admin_w;
    reposition(
        GetDlgItem(state.root, ID_IMAGE_ADMIN as i32),
        image_admin_x,
        TOP_BAR_Y,
        image_admin_w,
        24,
    );
    // 「查看日志」在同一链条的最左端（同一个教训：新顶栏按钮必须进这条重排链）。
    let view_logs_w = 120;
    reposition(
        GetDlgItem(state.root, ID_VIEW_LOGS as i32),
        image_admin_x - 8 - view_logs_w,
        TOP_BAR_Y,
        view_logs_w,
        24,
    );
    reposition(
        state.controls.source,
        field_x,
        source_combo_y,
        field_width,
        220,
    );
    reposition(
        state.controls.source_details,
        field_x,
        source_details_y,
        field_width,
        details_height,
    );
    reposition(
        state.controls.target,
        field_x,
        target_combo_y,
        field_width,
        220,
    );
    reposition(
        state.controls.target_details,
        field_x,
        target_details_y,
        field_width,
        details_height,
    );
    // B/C 入口的通道单选行：在线（左）/ 离线（右），文案随语言刷新，
    // 双保险处理「两个单选都未选中」的异常态（同 PE 模式单选的做法）。
    if channel_visible {
        let online_hwnd = GetDlgItem(state.root, ID_CHANNEL_ONLINE as i32);
        let offline_hwnd = GetDlgItem(state.root, ID_CHANNEL_OFFLINE as i32);
        let online_checked = IsDlgButtonChecked(state.root, ID_CHANNEL_ONLINE as i32) != 0;
        let offline_checked = IsDlgButtonChecked(state.root, ID_CHANNEL_OFFLINE as i32) != 0;
        if !online_checked && !offline_checked {
            SendMessageW(online_hwnd, BM_SETCHECK, BST_CHECKED, 0);
            append_gui_log(
                state,
                "channel layout: both radios unchecked -> reset to online",
            );
        }
        let language = selected_language(state);
        set_text(
            online_hwnd,
            if language == Language::English {
                "Online (no reboot)"
            } else {
                "在线执行（不重启）"
            },
        );
        set_text(
            offline_hwnd,
            if language == Language::English {
                "Offline (reboot into PE/RE)"
            } else {
                "离线执行（重启进 PE/RE）"
            },
        );
        reposition(online_hwnd, field_x, channel_y, 300, 22);
        reposition(offline_hwnd, field_x + 320, channel_y, 360, 22);
    }
    // 三个还原入口的「格式化目标分区」复选框：通道行下方一行（A 无通道行时
    // 仍按同一相对位置排布），文案随语言刷新。
    if show_format_target {
        let format_hwnd = GetDlgItem(state.root, ID_FORMAT_TARGET as i32);
        set_text(
            format_hwnd,
            if selected_language(state) == Language::English {
                "Format target volume (erase, then write the image)"
            } else {
                "格式化目标分区（清空目标卷后写入镜像）"
            },
        );
        let format_row_y = if channel_visible { format_y } else { channel_y };
        reposition(format_hwnd, field_x, format_row_y, 460, 22);
    }
    // ★ 同样按「最坏行数 × 行距」算，并且**以实测可用高度反推**：
    // 2.6.80：行距 17 px，提示文案最长 9 行（restore 页）→ 153 px；实测该控件
    // EM_GETRECT 的文本区比控件高度少 14 px（178 → availH 164），故 178 之后余量 11 px。
    // 更长的内容（PE 页）靠 WS_VSCROLL 滚动——**宁可滚，也不要切半行**。
    // 此前 64 px 只够 4.2 行，第 5 行被切成半截。
    // 9 行 + 25（文本区比控件高少 14px，另留 11px 余量）；同样随字号变化。
    // 高度在函数上半段就已经算好了（`status_height`）——下面几行的 y 都靠它定位，
    // 所以不能只在这儿算。
    reposition(state.controls.status, 20, status_y, client_width - 40, status_height);

    // 「安装 PE」tab：行1 RAM disk 单选 + PE 目录路径（完整路径，同排）；
    // 行2 硬盘启动单选；行3 PE 启动项名称（两种模式通用）
    if selected_operation(state) == "install-pe-entry" {
        let mode_y = last_details_y + 18;
        let pe_ram_checked = {
            let ram_checked = IsDlgButtonChecked(state.root, ID_PE_MODE_RAM as i32) != 0;
            let disk_checked = IsDlgButtonChecked(state.root, ID_PE_MODE_DISK as i32) != 0;
            if !ram_checked && !disk_checked {
                // 双保险：任何路径（切换 tab 等）导致两个单选都未选中时，
                // 恢复默认选中 RAM disk，避免目录行被隐藏、模式语义丢失。
                SendMessageW(
                    GetDlgItem(state.root, ID_PE_MODE_RAM as i32),
                    BM_SETCHECK,
                    BST_CHECKED,
                    0,
                );
                append_gui_log(
                    state,
                    "PE layout: both radios unchecked -> reset to RAM disk",
                );
                true
            } else {
                ram_checked
            }
        };
        append_gui_log(
            state,
            &format!(
                "PE layout: last_details_y={last_details_y} mode_y={mode_y} ram_checked={pe_ram_checked}"
            ),
        );
        set_text(
            GetDlgItem(state.root, ID_PE_MODE_RAM as i32),
            if selected_language(state) == Language::English {
                "RAM disk (no partition)"
            } else {
                "RAM disk（不占分区）"
            },
        );
        set_text(
            GetDlgItem(state.root, ID_PE_MODE_DISK as i32),
            if selected_language(state) == Language::English {
                "Hard disk boot (own partition)"
            } else {
                "硬盘启动（独立分区）"
            },
        );
        set_text(
            GetDlgItem(state.root, ID_PE_DIR_LABEL as i32),
            if selected_language(state) == Language::English {
                "PE folder path"
            } else {
                "PE 目录路径"
            },
        );
        reposition(
            GetDlgItem(state.root, ID_PE_MODE_RAM as i32),
            field_x,
            mode_y,
            260,
            22,
        );
        // 行2：硬盘启动单选
        reposition(
            GetDlgItem(state.root, ID_PE_MODE_DISK as i32),
            field_x,
            mode_y + 28,
            280,
            22,
        );
        // 「PE 目录路径」输入框：填完整路径（如 C:\BackupRestorePE，盘符可
        // 任意合法目录，不限于 C）。WIM 复制到该路径的 sources\boot.wim。
        // 为空时按语言+当前目标卷填默认完整路径；已有内容（含完整路径）原样保留。
        let dir_edit_hwnd = GetDlgItem(state.root, ID_PE_DIR_EDIT as i32);
        let dir_text = get_text(dir_edit_hwnd);
        let dir_trimmed = dir_text.trim();
        if dir_trimmed.is_empty() {
            // 程序所在盘符：取到就排除它，取不到（程序路径异常）就不排除任何卷，
            // 绝不退回字母 C——那会把一台系统盘恰好是 C 的机器上所有其它卷都排除掉。
            let exe_drive = state
                .executable_dir
                .to_string_lossy()
                .chars()
                .next()
                .map(|letter| letter.to_ascii_uppercase());
            let default_drive = state
                .drives
                .iter()
                .map(|d| d.letter.to_ascii_uppercase())
                .find(|letter| {
                    letter
                        .chars()
                        .next()
                        .map(|c| Some(c) != exe_drive)
                        .unwrap_or(true)
                })
                .unwrap_or_else(|| {
                    std::env::var("SystemDrive")
                        .unwrap_or_else(|_| "C:".to_string())
                        .trim()
                        .trim_end_matches(':')
                        .to_ascii_uppercase()
                });
            set_text(dir_edit_hwnd, &format!("{default_drive}:\\BackupRestorePE"));
        }
        reposition(
            GetDlgItem(state.root, ID_PE_DIR_LABEL as i32),
            field_x + 270,
            mode_y,
            110,
            22,
        );
        reposition(
            GetDlgItem(state.root, ID_PE_DIR_EDIT as i32),
            field_x + 380,
            mode_y,
            200,
            24,
        );
        // 浏览按钮紧贴目录输入框右侧（输入框 380..580，按钮 586..666）
        // ★ 2.6.80：按钮宽 60 → 80，给 14px 微软雅黑 Bold 的「浏览…」留出余量。
        reposition(
            GetDlgItem(state.root, ID_PE_DIR_BROWSE as i32),
            field_x + 586,
            mode_y,
            80,
            24,
        );
        // 行3：「PE 启动项名称」（两种模式通用，始终显示）
        set_text(
            GetDlgItem(state.root, ID_PE_NAME_LABEL as i32),
            if selected_language(state) == Language::English {
                "Boot entry name"
            } else {
                "PE 启动项名称"
            },
        );
        // 输入框为空时按语言+当前模式填默认名；若仍是另一模式的默认名
        // （用户未自定义），切换模式时跟随更新（用户自定义后保留）。
        let name_edit_hwnd = GetDlgItem(state.root, ID_PE_NAME_EDIT as i32);
        let default_name = pe_entry_description(selected_language(state), pe_ram_checked);
        let current_name = get_text(name_edit_hwnd).trim().to_string();
        if current_name.is_empty() {
            set_text(name_edit_hwnd, &default_name);
        } else {
            let other_default = pe_entry_description(selected_language(state), !pe_ram_checked);
            if current_name == other_default {
                set_text(name_edit_hwnd, &default_name);
            }
        }
        reposition(
            GetDlgItem(state.root, ID_PE_NAME_LABEL as i32),
            field_x,
            mode_y + 56,
            130,
            22,
        );
        reposition(
            GetDlgItem(state.root, ID_PE_NAME_EDIT as i32),
            field_x + 140,
            mode_y + 54,
            300,
            24,
        );
        // 硬盘模式下隐藏 PE 目录路径（完整路径）行，仅 RAM disk 显示
        let dir_label_hwnd = GetDlgItem(state.root, ID_PE_DIR_LABEL as i32);
        let label_visible = if pe_ram_checked { SW_SHOW } else { SW_HIDE };
        let edit_visible = if pe_ram_checked { SW_SHOW } else { SW_HIDE };
        ShowWindow(dir_label_hwnd, label_visible);
        ShowWindow(dir_edit_hwnd, edit_visible);
        ShowWindow(
            GetDlgItem(state.root, ID_PE_DIR_BROWSE as i32),
            edit_visible,
        );
    }

    // ★ 2.6.80：基准字号 12→14 且改用微软雅黑 Bold 之后，浏览按钮的文字变宽，
    //   原来「间距 10 + 按钮宽 80」在视觉上贴到输入框右端（用户反馈重叠）。
    //   这里把间距加到 14、按钮宽加到 106，并相应缩短输入框，右缘仍与
    //   `field_x + field_width` 对齐（10+80=90 与 14+106=120 的差额由输入框吸收）。
    let image_width = (field_width - 120).max(370);
    reposition(state.controls.image, field_x, image_y, image_width, 24);
    reposition(
        GetDlgItem(state.root, ID_BROWSE_IMAGE as i32),
        field_x + image_width + 14,
        image_y,
        106,
        24,
    );
    // 「单系统还原」与「新增第二系统」tab 的 WIM 索引与校验复选框布局：
    // C（还原系统到非 C 盘）与「新增第二系统」一样带菜单名输入框，用同一布局。
    let (index_width, verify_hash_x, verify_hash_y) =
        if matches!(
            selected_operation(state),
            "create-secondary"
        ) {
            (380, field_x, secondary_y + 32)
        } else {
            (
                field_width - 320,
                field_x + field_width - 300,
                secondary_y + 2,
            )
        };
    reposition(state.controls.index, field_x, secondary_y, index_width, 220);
    reposition(
        state.controls.verify_hash,
        verify_hash_x,
        verify_hash_y,
        300,
        24,
    );
    reposition(
        state.controls.menu,
        field_x + 500,
        secondary_y,
        (field_width - 500).max(300),
        24,
    );
    // 「备份」tab 的压缩率下拉：WIM 索引/第二系统名称行（secondary_y）在
    // 备份模式下控件均隐藏，压缩率独占该行：标签在左侧，下拉框在右侧。
    reposition(GetDlgItem(state.root, 2014), 20, secondary_y + 2, 150, 24);
    reposition(state.controls.compress, field_x, secondary_y, 280, 220);

    reposition(
        GetDlgItem(state.root, 2003),
        20,
        source_combo_y + 2,
        150,
        40,
    );
    reposition(
        GetDlgItem(state.root, 2005),
        20,
        target_combo_y + 2,
        150,
        40,
    );
    reposition(GetDlgItem(state.root, 2004), 20, image_y + 2, 150, 24);
    reposition(GetDlgItem(state.root, 2007), 20, secondary_y + 2, 150, 24);
    reposition(
        GetDlgItem(state.root, 2008),
        field_x + 500 - 110,
        secondary_y + 2,
        100,
        24,
    );
    // 备份 tab 第二行：索引名（左）+ 保留最近 N 个（右），位于压缩率行下方。
    let index_name_y = secondary_y + 32;
    reposition(GetDlgItem(state.root, 2015), 20, index_name_y, 130, 24);
    reposition(
        state.controls.index_name,
        field_x,
        index_name_y - 2,
        280,
        24,
    );
    reposition(
        GetDlgItem(state.root, 2016),
        field_x + 320,
        index_name_y,
        100,
        24,
    );
    reposition(state.controls.keep, field_x + 430, index_name_y - 2, 80, 24);
    for (id, x) in [
        (ID_REFRESH, 20),
        (ID_READ_IMAGE, 150),
        (ID_CREATE_TASK, 280),
        (ID_REFRESH_TASK, 410),
    ] {
        reposition(
            GetDlgItem(state.root, id as i32),
            x,
            buttons_y,
            if id == ID_REFRESH_TASK { 140 } else { 120 },
            28,
        );
    }
    // 「安装 PE」tab 专属按钮（重启进入 PE / 创建快捷方式）：
    // 创建时使用固定坐标（560/710, 630），这里与通用按钮行同 y 对齐，
    // 避免窗口/布局变化时与右侧控件错位或遮挡。
    if selected_operation(state) == "install-pe-entry" {
        reposition(
            GetDlgItem(state.root, ID_PE_REBOOT_MAIN as i32),
            560,
            buttons_y,
            140,
            28,
        );
        reposition(
            GetDlgItem(state.root, ID_PE_SHORTCUT as i32),
            710,
            buttons_y,
            140,
            28,
        );
    }
}

unsafe fn set_volume_labels(state: &State) {
    let language = selected_language(state);
    let operation = selected_operation(state);
    // A 的目标标签带实际盘符（%SystemDrive%），动态拼接；源卷行在 A/B/C 隐藏，
    // 标签文案仅作回退占位。Keep the labels short enough for the narrow role
    // column. The detail boxes below carry the complete safety explanation.
    let (source, target): (String, String) = match (language, operation) {
        (Language::Chinese, "backup") => {
            ("源卷（备份来源）".into(), "目标卷（镜像位置）".into())
        }
        (Language::Chinese, "restore-existing") => (
            "源卷（不使用）".into(),
            format!("目标卷（当前系统盘 {}:）", active_system_letter()),
        ),
        (Language::Chinese, "restore-data") => {
            ("源卷（不使用）".into(), "目标卷（数据卷）".into())
        }
        (Language::Chinese, "create-secondary") => (
            "源卷（不使用）".into(),
            "目标卷（第二系统位置）".into(),
        ),
        (Language::Chinese, "install-pe-entry") => {
            ("源卷（不使用）".into(), "目标卷（PE 安装位置）".into())
        }
        (Language::Chinese, _) => ("源卷（检查对象）".into(), "目标卷（不使用）".into()),
        (Language::English, "backup") => ("Source (backup)".into(), "Target (image)".into()),
        (Language::English, "restore-existing") => (
            "Source (unused)".into(),
            format!("Target (system {}:)", active_system_letter()),
        ),
        (Language::English, "restore-data") => {
            ("Source (unused)".into(), "Target (data)".into())
        }
        (Language::English, "create-secondary") => (
            "Source (unused)".into(),
            "Target (second system)".into(),
        ),
        (Language::English, "install-pe-entry") => {
            ("Source (unused)".into(), "Target (PE install)".into())
        }
        (Language::English, _) => ("Source (inspect)".into(), "Target (unused)".into()),
    };
    set_child_text(state.root, 2003, &source);
    set_child_text(state.root, 2005, &target);
}

unsafe fn select_operation(state: &mut State, index: usize) {
    // 【2026-10-18 缺陷修复 M2】两个「当前选中项」都必须在**改 operation_index
    // 之前**读取。combo 里的项是**旧入口**填充的，而 `selected_drive_letter`
    // 按**当前** operation_index 决定用不用过滤视图来解释索引——顺序一颠倒，
    // 就是用新视图去读旧内容，选中项被静默移位（从 C 切到 A 会把用户选的
    // F: 读成 E:）。源卷同理：此前重填时 source 传 `None`，等于把备份页的
    // 源卷静默重置为第 0 项，切走再切回可能备份到错误的分区。
    let keep_target = selected_drive_letter(state, state.controls.target);
    let keep_source = selected_drive_letter(state, state.controls.source);
    state.operation_index = index.min(4);
    set_operation_tabs(state, selected_language(state));
    let operation = selected_operation(state);
    if operation == "restore-existing" {
        // A（还原系统到 C 盘）：目标锁定 %SystemDrive%，自动选中（下拉已禁用）。
        // 旧版「源卷选择复制到目标」逻辑随源卷行隐藏一并移除。
        let letter = active_system_letter();
        select_drive(state, state.controls.target, &letter);
    }
    if matches!(operation, "restore-data" | "create-secondary") {
        // B/C：每次进入入口时通道复位为「在线」默认——不跨入口保留上次的
        // 离线选择，防止用户在 C 选了离线后切到 B 忘改而意外重启格式化。
        SendMessageW(
            GetDlgItem(state.root, ID_CHANNEL_ONLINE as i32),
            BM_SETCHECK,
            BST_CHECKED,
            0,
        );
        SendMessageW(
            GetDlgItem(state.root, ID_CHANNEL_OFFLINE as i32),
            BM_SETCHECK,
            BST_UNCHECKED,
            0,
        );
    }
    // C 入口目标 combo 排除当前系统盘；离开 C 回别处时恢复全量（A 页要能
    // 选中系统盘）。combo 内容只在 set_drive_items 填充时定死、tab 切换
    // 本身不重填——这里按「当前入口期望项数 vs 实际项数」校验，不符即带
    // 选中重填（v2.6.13；少了这步，从备份 tab 进 C 会残留全量枚举，系统盘
    // 又出现在下拉里）。
    if target_combo_expected_count(state)
        != SendMessageW(state.controls.target, CB_GETCOUNT, 0, 0) as usize
    {
        append_gui_log(
            state,
            &format!(
                "target combo refilled: op={operation} keep={} keep_source={}",
                keep_target.clone().unwrap_or_else(|| "<none>".into()),
                keep_source.clone().unwrap_or_else(|| "<none>".into())
            ),
        );
        // 源卷选中项一并传下去（M2）：不传就是「静默重置为第 0 项」。
        set_drive_items(state, [None, keep_source, keep_target]);
    }
    if matches!(operation, "restore-data" | "create-secondary") {
        // 目标若还停在初始默认的系统盘（B/C 都禁止的目标），自动跳到第一个
        // 合法卷：B=非系统且非 Windows 卷；C=非系统卷。找不到就保持原样，
        // 由「创建任务」时的目标闸门给出明确报错（不做静默猜测）。
        let system = active_system_letter();
        if let Some(current) = selected_drive_letter(state, state.controls.target)
            && current.eq_ignore_ascii_case(&system)
            && let Some(candidate) = state.drives.iter().find(|drive| {
                !drive.letter.eq_ignore_ascii_case(&system)
                    && (operation == "create-secondary" || !drive.has_windows_installation)
            })
        {
            append_gui_log(
                state,
                &format!(
                    "target default moved off system drive: {} -> {}",
                    current, candidate.letter
                ),
            );
            select_drive(state, state.controls.target, &candidate.letter);
        }
    }
    set_operation_visibility(state);
    set_volume_labels(state);
    layout_operation(state);
    set_operation_guidance(state);
    set_drive_details(state);
    // S5：切到 C/第二系统 tab 时，若菜单名还是自动态就用当前索引的 WIM 名预填。
    prefill_menu_name_from_image(state);
    // 整窗强制重绘（历史 bug 双保险）：layout_operation 会对十余个控件做
    // MoveWindow，快速连续切换 tab 时重绘风暴容易让个别控件区域残留上一
    // tab 的文本（表现为「PE tab 镜像标签行叠出上一 tab 的 status 文本」）。
    // 在全部布局与文案就绪后同步重绘整窗，彻底擦除残留像素。
    InvalidateRect(state.root, null(), 1);
    UpdateWindow(state.root);
}

/// C 入口（新增非 C 盘第二系统）目标卷枚举视图：排除当前系统盘。
///
/// combo 索引与 `state.drives` 索引的默认映射是恒等的；过滤开启后所有
/// CB_GETCURSEL/CB_SETCURSEL 都必须经本视图换算，否则 combo 索引会错位到
/// 别的卷——「选了 E: 实际写到 F:」比不过滤更危险。返回 None = 不过滤
/// （恒等映射，其余入口保持原行为）。
unsafe fn target_combo_indices(state: &State) -> Option<Vec<usize>> {
    if selected_operation(state) != "create-secondary" {
        return None;
    }
    let system = active_system_letter();
    Some(
        state
            .drives
            .iter()
            .enumerate()
            .filter(|(_, drive)| !drive.letter.eq_ignore_ascii_case(&system))
            .map(|(index, _)| index)
            .collect(),
    )
}

/// 目标 combo 在当前入口下的期望项数（与 set_drive_items 填充结果对称）：
/// 入口视图空/无卷时下拉里是 1 个占位提示项，否则等于视图大小。
unsafe fn target_combo_expected_count(state: &State) -> usize {
    match target_combo_indices(state) {
        Some(view) if !view.is_empty() => view.len(),
        Some(_) => 1,
        None if state.drives.is_empty() => 1,
        None => state.drives.len(),
    }
}

/// 盘符 → target combo 项索引（经过滤视图换算；view=None 直查 drives）。
fn drive_combo_position(view: Option<&[usize]>, state: &State, letter: &str) -> Option<usize> {
    match view {
        Some(view) => view
            .iter()
            .copied()
            .position(|index| state.drives[index].letter.eq_ignore_ascii_case(letter)),
        None => state
            .drives
            .iter()
            .position(|drive| drive.letter.eq_ignore_ascii_case(letter)),
    }
}

unsafe fn selected_drive_letter(state: &State, control: Hwnd) -> Option<String> {
    let index = combo_selection(control)?;
    let drive_index = if control == state.controls.target {
        match target_combo_indices(state) {
            Some(view) => *view.get(index)?,
            None => index,
        }
    } else {
        index
    };
    state.drives.get(drive_index).map(|drive| drive.letter.clone())
}

fn drive_display(drive: &DriveInfo, language: Language) -> String {
    let label = if drive.label.is_empty() {
        if language == Language::English {
            "no label"
        } else {
            "无卷标"
        }
    } else {
        &drive.label
    };
    let disk = drive
        .disk_number
        .map(|number| number.to_string())
        .unwrap_or_else(|| "?".to_string());
    let partition = drive
        .partition_number
        .map(|number| number.to_string())
        .unwrap_or_else(|| "?".to_string());
    let windows_marker = if drive.has_windows_installation {
        " | Windows"
    } else {
        ""
    };
    if language == Language::English {
        format!(
            "{}: | {} | {} | total {} | free {} | disk {}/partition {}{}",
            drive.letter,
            drive.filesystem,
            label,
            format_bytes(drive.size_bytes),
            format_bytes(drive.free_bytes),
            disk,
            partition,
            windows_marker,
        )
    } else {
        format!(
            "{}: | {} | 卷标 {} | 总容量 {} | 可用 {} | 磁盘 {}/分区 {}{}",
            drive.letter,
            drive.filesystem,
            label,
            format_bytes(drive.size_bytes),
            format_bytes(drive.free_bytes),
            disk,
            partition,
            windows_marker,
        )
    }
}

fn drive_details(drive: &DriveInfo, language: Language) -> String {
    let disk = drive
        .disk_number
        .map(|number| number.to_string())
        .unwrap_or_else(|| "?".to_string());
    let partition = drive
        .partition_number
        .map(|number| number.to_string())
        .unwrap_or_else(|| "?".to_string());
    if language == Language::English {
        format!(
            "{}: {}\nFile system: {}\r\nWindows installation: {}\r\nTotal: {} | Free: {}\r\nDisk/partition: {}/{}\r\nPartition type: {}\r\nVolume GUID: {}",
            drive.letter,
            if drive.label.is_empty() {
                "no label"
            } else {
                &drive.label
            },
            drive.filesystem,
            if drive.has_windows_installation {
                "yes"
            } else {
                "no"
            },
            format_bytes(drive.size_bytes),
            format_bytes(drive.free_bytes),
            disk,
            partition,
            drive.partition_type_guid,
            drive.volume_guid,
        )
    } else {
        format!(
            "{}: {}\n文件系统：{}\r\nWindows 安装：{}\r\n总容量：{} | 可用：{}\r\n磁盘/分区：{}/{}\r\n分区类型：{}\r\n卷 GUID：{}",
            drive.letter,
            if drive.label.is_empty() {
                "无卷标"
            } else {
                &drive.label
            },
            drive.filesystem,
            if drive.has_windows_installation {
                "是"
            } else {
                "否"
            },
            format_bytes(drive.size_bytes),
            format_bytes(drive.free_bytes),
            disk,
            partition,
            drive.partition_type_guid,
            drive.volume_guid,
        )
    }
}

fn drive_role_description(role: &str, operation: &str, language: Language) -> &'static str {
    match (language, role, operation) {
        (Language::Chinese, "source", "backup") => {
            "源卷用途：备份时从这里捕获 Windows 分区；镜像不能保存到这个卷。"
        }
        (Language::Chinese, "source", "restore-existing") => {
            "源卷用途：当前要替换的 Windows 分区；单系统还原会强制目标卷与它相同。"
        }
        (Language::Chinese, "source", _) => {
            "源卷用途：探测时只核验当前 Windows 分区身份；不会备份、还原或写入。"
        }
        (Language::Chinese, "target", "restore-existing") => {
            "目标卷用途：锁定为当前正在运行的系统盘；离线格式化后写回镜像，重建唯一系统。"
        }
        (Language::Chinese, "target", "restore-data") => {
            "目标卷用途：接收数据卷镜像。默认勾选「格式化目标分区」=清空后写入（卷序列号改变）；取消勾选=覆盖式应用（多余文件保留）。不能选当前系统盘或已有 Windows 的分区。"
        }
        (Language::Chinese, "target", "create-secondary") => {
            "目标卷用途：将被格式化并写入完整 Windows 作为第二系统，新增启动菜单项，完成后默认启动项切换为这台新系统（原系统仍可选）。下拉列表已排除当前系统盘（C:）。"
        }
        (Language::Chinese, "target", "backup") => {
            "目标卷用途：备份模式不会写入此卷；镜像保存位置由“镜像绝对路径”决定。"
        }
        (Language::Chinese, "target", _) => {
            "目标卷用途：探测模式只核验身份；不会备份、还原、格式化或写入。"
        }
        (Language::English, "source", "backup") => {
            "Purpose: backup captures the Windows partition here; the image cannot be stored on this volume."
        }
        (Language::English, "source", "restore-existing") => {
            "Purpose: current Windows partition to replace; single-system restore forces target to match it."
        }
        (Language::English, "source", _) => {
            "Purpose: probe only checks the current Windows volume identity; it never writes the volume."
        }
        (Language::English, "target", "restore-existing") => {
            "Purpose: locked to the running system drive; formatted offline and rewritten, rebuilding the only Windows."
        }
        (Language::English, "target", "restore-data") => {
            "Purpose: receives the data-volume image. 'Format target volume' checked (default) = erase then write (serial changes); unchecked = overlay apply. Never the system drive or an existing Windows partition."
        }
        (Language::English, "target", "create-secondary") => {
            "Purpose: formatted and written as a second Windows with a new boot entry; after the restore the default boot entry switches to this new system (the previous system stays selectable). The dropdown already excludes the current system drive (C:)."
        }
        (Language::English, "target", "backup") => {
            "Purpose: backup does not write this volume; image storage comes from Image absolute path."
        }
        (Language::English, "target", _) => {
            "Purpose: probe only checks identity; it never backs up, restores, formats or writes this volume."
        }
        _ => "",
    }
}

unsafe fn set_drive_details(state: &State) {
    let language = selected_language(state);
    let selected = [
        (
            "source",
            selected_drive_letter(state, state.controls.source),
        ),
        (
            "target",
            selected_drive_letter(state, state.controls.target),
        ),
    ];
    let detail_controls = [state.controls.source_details, state.controls.target_details];
    let operation = selected_operation(state);
    for (role, letter) in selected {
        let index = match role {
            "source" => 0,
            _ => 1,
        };
        let text = letter
            .and_then(|letter| state.drives.iter().find(|item| item.letter == letter))
            .map(|drive| {
                format!(
                    "{}\n\n{}",
                    drive_role_description(role, operation, language),
                    drive_details(drive, language)
                )
            })
            .unwrap_or_else(|| {
                if language == Language::English {
                    format!(
                        "{}\n\nNo volume selected.",
                        drive_role_description(role, operation, language)
                    )
                } else {
                    format!(
                        "{}\n\n未选择卷。",
                        drive_role_description(role, operation, language)
                    )
                }
            });
        set_text(detail_controls[index], &text);
    }
}

unsafe fn set_drive_items(state: &State, desired: [Option<String>; 3]) {
    // 源卷：全量枚举（备份入口选备份来源；还原类不读它的选择）。
    reset_combo(state.controls.source);
    if state.drives.is_empty() {
        add_combo_item(
            state.controls.source,
            if selected_language(state) == Language::English {
                "No eligible mounted volume"
            } else {
                "没有可选择的已挂载卷"
            },
        );
        SendMessageW(state.controls.source, CB_SETCURSEL, 0, 0);
    } else {
        for drive in &state.drives {
            add_combo_item(
                state.controls.source,
                &drive_display(drive, selected_language(state)),
            );
        }
    }
    // 目标卷：C 入口排除当前系统盘（combo 索引经 target_combo_indices 换算）。
    let target_view = target_combo_indices(state);
    let target_positions: Vec<usize> = match &target_view {
        Some(view) if !view.is_empty() => view.clone(),
        // 唯一卷恰是当前系统盘：明确告知排除规则，不留静默空 combo。
        Some(_) => Vec::new(),
        None => (0..state.drives.len()).collect(),
    };
    reset_combo(state.controls.target);
    if state.drives.is_empty() {
        add_combo_item(
            state.controls.target,
            if selected_language(state) == Language::English {
                "No eligible mounted volume"
            } else {
                "没有可选择的已挂载卷"
            },
        );
        SendMessageW(state.controls.target, CB_SETCURSEL, 0, 0);
    } else if target_positions.is_empty() {
        add_combo_item(
            state.controls.target,
            if selected_language(state) == Language::English {
                "No eligible target (current system drive excluded)"
            } else {
                "没有可选目标卷（已排除当前系统盘）"
            },
        );
        SendMessageW(state.controls.target, CB_SETCURSEL, 0, 0);
    } else {
        for &drive_index in &target_positions {
            add_combo_item(
                state.controls.target,
                &drive_display(&state.drives[drive_index], selected_language(state)),
            );
        }
    }
    for (control, wanted, view) in [
        (state.controls.source, desired[1].clone(), None),
        (state.controls.target, desired[2].clone(), target_view.as_deref()),
    ] {
        let position = wanted
            .as_deref()
            .and_then(|letter| drive_combo_position(view, state, letter))
            .unwrap_or(0);
        SendMessageW(control, CB_SETCURSEL, position as usize, 0);
    }
}

unsafe fn select_drive(state: &State, control: Hwnd, letter: &str) {
    // 【2026-10-18 缺陷修复 H1】此前目标分支写作
    // `target_combo_indices(state).as_deref().and_then(|view| …)`：视图为 `None`
    // （除 C 入口外的**所有**入口，含 A）时 `and_then` 直接得到 `None`，
    // `CB_SETCURSEL` 从不发出。A 入口「自动选中当前系统盘」的意图因此落空——
    // 下拉停在上一个入口选中的盘，界面却写着「目标卷（当前系统盘 C:）」，
    // **显示卷 ≠ 操作卷**（prepare 只要求 source==target 即放行 → 格式化错盘）。
    // 现在与 `drive_combo_position` 完全同源：视图在就按视图换算（视图里找不到
    // 就保持不动，绝不回退全量索引），视图为 None 就用全量索引。
    let position = if control == state.controls.target {
        let view = target_combo_indices(state);
        drive_combo_position(view.as_deref(), state, letter)
    } else {
        drive_combo_position(None, state, letter)
    };
    if let Some(position) = position {
        SendMessageW(control, CB_SETCURSEL, position as usize, 0);
    }
}

unsafe fn selected_wim_index(state: &State) -> Option<u32> {
    if let Some(image) = state.wim_images.get(combo_index(state.controls.index)) {
        return Some(image.index);
    }
    get_text(state.controls.index)
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
}

fn parse_drive_infos(output: &str) -> Result<Vec<DriveInfo>, String> {
    let value: serde_json::Value = serde_json::from_str(output)
        .map_err(|error| format!("volume metadata JSON parse failed: {error}"))?;
    let items = if let Some(items) = value.as_array() {
        items.clone()
    } else if value.get("letter").is_some() {
        vec![value]
    } else {
        return Err("volume metadata did not return a volume object or array".to_string());
    };
    let mut drives = Vec::with_capacity(items.len());
    for item in items {
        let letter = json_text(&item, "letter")
            .trim()
            .trim_end_matches(':')
            .to_ascii_uppercase();
        if letter.len() != 1 || !letter.as_bytes()[0].is_ascii_alphabetic() {
            continue;
        }
        drives.push(DriveInfo {
            letter,
            label: json_text(&item, "label"),
            filesystem: json_text(&item, "filesystem"),
            size_bytes: json_text(&item, "sizeBytes").parse::<u64>().ok(),
            free_bytes: json_text(&item, "freeBytes").parse::<u64>().ok(),
            volume_guid: json_text(&item, "volumeGuid"),
            disk_number: json_text(&item, "diskNumber").parse::<u32>().ok(),
            partition_number: json_text(&item, "partitionNumber").parse::<u32>().ok(),
            partition_type_guid: json_text(&item, "partitionTypeGuid"),
            has_windows_installation: json_text(&item, "hasWindowsInstallation")
                .eq_ignore_ascii_case("true"),
        });
    }
    drives.sort_by(|left, right| left.letter.cmp(&right.letter));
    drives.dedup_by(|left, right| left.letter == right.letter);
    if drives.is_empty() {
        return Err("no eligible mounted volumes were returned".to_string());
    }
    Ok(drives)
}

fn report_images_value(report: &serde_json::Value) -> serde_json::Value {
    report
        .get("images")
        .cloned()
        .or_else(|| {
            if report.get("ImageIndex").is_some() || report.get("imageIndex").is_some() {
                Some(report.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()))
}

fn wim_display(image: &WimImageInfo, language: Language) -> String {
    let name = if image.name.is_empty() {
        if language == Language::English {
            "(unnamed)"
        } else {
            "（未命名）"
        }
    } else {
        &image.name
    };
    let description = if image.description.is_empty() {
        if language == Language::English {
            "no description"
        } else {
            "无描述"
        }
    } else {
        &image.description
    };
    let version = if image.version.is_empty() {
        "?"
    } else {
        &image.version
    };
    let architecture = if image.architecture.is_empty() {
        "?"
    } else {
        &image.architecture
    };
    let edition = if image.edition.is_empty() {
        "?"
    } else {
        &image.edition
    };
    let install_type = if image.installation_type.is_empty() {
        "?"
    } else {
        &image.installation_type
    };
    if language == Language::English {
        format!(
            "Index {} | {} | {} | Version {} | Arch {} | Edition {} | Install {} | Size {}",
            image.index,
            name,
            description,
            version,
            architecture,
            edition,
            install_type,
            format_bytes(image.size_bytes),
        )
    } else {
        format!(
            "索引 {} | {} | {} | 版本 {} | 架构 {} | 版本类型 {} | 大小 {} | 安装类型 {}",
            image.index,
            name,
            description,
            version,
            architecture,
            edition,
            format_bytes(image.size_bytes),
            install_type,
        )
    }
}

unsafe fn set_wim_items(state: &State) {
    let language = selected_language(state);
    let selected = selected_wim_index(state);
    reset_combo(state.controls.index);
    if state.wim_images.is_empty() {
        add_combo_item(
            state.controls.index,
            if language == Language::English {
                "No image metadata loaded (click Read image)"
            } else {
                "尚未读取镜像信息（请点击“读取镜像”）"
            },
        );
        SendMessageW(state.controls.index, CB_SETCURSEL, 0, 0);
        return;
    }
    let mut selected_position = 0usize;
    for (position, image) in state.wim_images.iter().enumerate() {
        if selected == Some(image.index) {
            selected_position = position;
        }
        add_combo_item(state.controls.index, &wim_display(image, language));
    }
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 1020,
        bottom: 760,
    };
    GetClientRect(state.root, &mut rect);
    // The open list must expose each index's full metadata, even when the
    // selected one-line control shares a row with a secondary-system name.
    // ★ 2.6.79：客户区换算回 96dpi 基准再减常量，最后整体缩放成物理像素。
    let dropdown_width =
        crate::dpi::s((crate::dpi::unscale(rect.right - rect.left) - 204).max(440)) as usize;
    SendMessageW(state.controls.index, CB_SETDROPPEDWIDTH, dropdown_width, 0);
    SendMessageW(state.controls.index, CB_SETCURSEL, selected_position, 0);
}

/// S5：把选中 WIM 索引的名字预填到「启动菜单名称」文本框。
///
/// 名字取自 DISM 的 ImageName——我们自己的备份是索引名（如「2026-10-14
/// 21:40」），第三方 WIM 是「Windows 11 Pro」之类。只在用户没有手动改过时
/// 覆盖（`menu_name_touched`）：用户意图优先于自动预填。
unsafe fn prefill_menu_name_from_image(state: &mut State) {
    if state.menu_name_touched {
        return;
    }
    if !matches!(
        selected_operation(state),
        "create-secondary"
    ) {
        return;
    }
    // 先取owned名字再写文本框：image 的借用不能跨过 set_menu_name 的 &mut。
    let name = state
        .wim_images
        .get(combo_index(state.controls.index))
        .map(|image| image.name.trim().to_string());
    let Some(name) = name else {
        return;
    };
    if name.is_empty() {
        return;
    }
    set_menu_name(state, &name);
}

/// 程序侧写菜单名（区别于用户输入）：同步记录自动值，WM_COMMAND 的
/// EN_CHANGE 据此区分「程序预填」与「用户手改」——SetWindowText 自己也会
/// 触发 EN_CHANGE，没有这个对照就会把程序自己的预填误判成用户输入。
///
/// 顺序要紧：必须先写 `menu_name_auto` 再 `set_text`。EDIT 的 WM_SETTEXT
/// 会同步（SendMessage）向父窗发 EN_CHANGE，处理函数在同一调用栈上重入；
/// 若照「先写文本后记自动值」的直觉顺序，重入时 auto 还是上一个值（启动
/// 首次甚至是 None），当前文本已变 → 差值非空 → `menu_name_touched` 被
/// 永久置 true，此后读取镜像/切索引的预填全部被跳过（2026-10-15 实机
/// 复现：菜单框永远停留在语言默认值，S5 预填整个失效）。
unsafe fn set_menu_name(state: &mut State, value: &str) {
    state.menu_name_auto = Some(value.to_string());
    set_text(state.controls.menu, value);
}

/// 读 HKLM 下的 REG_SZ 值。读不到或类型不符返回 `None`——版本信息只是给索引名
/// 起个好名字，缺了要有降级方案，绝不能因此阻断建窗。
unsafe fn reg_read_string(sub_key: &str, value: &str) -> Option<String> {
    let key = wide(sub_key);
    let name = wide(value);
    let mut buffer = vec![0u16; 512];
    let mut size = (buffer.len() * size_of::<u16>()) as u32;
    let status = RegGetValueW(
        HKEY_LOCAL_MACHINE,
        key.as_ptr(),
        name.as_ptr(),
        RRF_RT_REG_SZ,
        null_mut(),
        buffer.as_mut_ptr().cast(),
        &mut size,
    );
    if status != 0 {
        return None;
    }
    // 缓冲区未被填满的部分是 0，去掉尾部 NUL 再 trim。
    let text = String::from_utf16_lossy(&buffer)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

/// 读 HKLM 下的 REG_DWORD 值（`UBR` 补丁级别就是 DWORD）。
unsafe fn reg_read_dword(sub_key: &str, value: &str) -> Option<u32> {
    let key = wide(sub_key);
    let name = wide(value);
    let mut data: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let status = RegGetValueW(
        HKEY_LOCAL_MACHINE,
        key.as_ptr(),
        name.as_ptr(),
        RRF_RT_REG_DWORD,
        null_mut(),
        (&mut data as *mut u32).cast(),
        &mut size,
    );
    (status == 0).then_some(data)
}

/// `ProductName` 去掉尾部 edition 词：`Windows 11 Pro` → `Windows 11`，
/// `Windows Server 2022 Datacenter` → `Windows Server 2022`。
/// 词数不足三个时不拆（避免把 `Windows 11` 砍成 `Windows`）。
fn trim_edition(product: &str) -> String {
    let words: Vec<&str> = product.split_whitespace().collect();
    if words.len() >= 3 {
        words[..words.len() - 1].join(" ")
    } else {
        product.trim().to_string()
    }
}

/// 当前系统的版本串，如 `Windows 11 26200.9550`。
///
/// ⚠️ 主版本**不能只信 `ProductName`**：实测本机是 Windows 11 25H2（build
/// 26200.9550、DisplayVersion 25H2），注册表 `ProductName` 却仍写着
/// `Windows 10 Pro`——跨版本升级/预览通道常见。所以桌面版按 build 号判定
/// （≥22000 = Windows 11，≥10240 = Windows 10），这也是通行做法；
/// Server 分支不受影响（ProductName 里的 "Server" 优先，且它的 build 号
/// 与桌面版不同源，不能套桌面版阈值）。
unsafe fn current_windows_version_tag() -> Option<String> {
    let product = reg_read_string(CURRENT_VERSION_KEY, "ProductName")?;
    let build = reg_read_string(CURRENT_VERSION_KEY, "CurrentBuildNumber");
    let build_number = build
        .as_deref()
        .and_then(|value| value.trim().parse::<u32>().ok());
    let ubr = reg_read_dword(CURRENT_VERSION_KEY, "UBR");
    let family = if product.to_ascii_lowercase().contains("server") {
        trim_edition(&product)
    } else {
        match build_number {
            Some(number) if number >= 22000 => "Windows 11".to_string(),
            Some(number) if number >= 10240 => "Windows 10".to_string(),
            // build 读不到才退回 ProductName：宁可信息旧一点，也不要凭空造。
            _ => trim_edition(&product),
        }
    };
    let version = match (build, ubr) {
        (Some(build), Some(ubr)) => format!("{build}.{ubr}"),
        (Some(build), None) => build,
        // 连 build 都读不到就只给产品名，不再拼半个版本号。
        (None, _) => return Some(family),
    };
    Some(format!("{family} {version}"))
}

/// 备份索引名默认值：`系统版本 + 年月日`，例 `Windows 11 26200.9550 20261015`
/// （2026-10-15 用户需求）。
///
/// **只在源卷就是当前系统盘时才带版本**：别的卷的版本要挂离线 hive 才读得到，
/// 代价高；而把当前系统的版本安在别的卷头上是误导——索引名写进 WIM 就改不了。
/// 那种情况退化成带盘符的名字，信息少但不假。
unsafe fn default_backup_index_name(source_is_system: bool, letter: Option<&str>) -> String {
    let mut now = SystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    GetLocalTime(&mut now);
    let date = format!("{:04}{:02}{:02}", now.year, now.month, now.day);
    if !source_is_system {
        return match letter {
            Some(letter) => format!("Windows ({letter}:) {date}"),
            None => format!("Windows {date}"),
        };
    }
    match current_windows_version_tag() {
        Some(tag) => format!("{tag} {date}"),
        None => format!("Windows {date}"),
    }
}

/// 程序侧写备份索引名：同步记录自动值，EN_CHANGE 据此区分「程序预填」与
/// 「用户手改」——理由与顺序要求完全同 `set_menu_name`（先记 auto 再写文本）。
unsafe fn set_index_name(state: &mut State, value: &str) {
    state.index_name_auto = Some(value.to_string());
    set_text(state.controls.index_name, value);
}

/// 源卷确定后重算备份索引名；用户手改过就再也不动它。
unsafe fn refresh_backup_index_name(state: &mut State) {
    if state.index_name_touched {
        return;
    }
    let letter = selected_drive_letter(state, state.controls.source);
    // `%SystemDrive%` 取不到时 `system` 是 None，此时如实判定为"不是系统卷"
    // 而不是退回字母 C——备份索引名的默认值会因此降级成带盘符的形式
    // （`Windows (F:) 日期`），宁可少写也不要写错。
    let system = system_drive_letter();
    let source_is_system = letter
        .as_deref()
        .and_then(|letter| letter.chars().next())
        .and_then(|letter| system.map(|system| letter.to_ascii_uppercase() == system))
        .unwrap_or(false);
    let value = default_backup_index_name(source_is_system, letter.as_deref());
    if state.index_name_auto.as_deref() == Some(value.as_str()) {
        return;
    }
    set_index_name(state, &value);
}

/// S5：读当前 BCD 里所有启动项名字（撞名预检的数据源）。
///
/// 走 CLI 子命令 `list-boot-entries`：`bcdedit /enum` 是读操作，普通权限即可，
/// 而 GUI 常态未提权（package 自启副本）。读不到返回空表——按「无法预检」
/// 处理并记日志，承重消解在提权后的 prepare 侧（docs/20261015-0725）。
fn existing_boot_entry_names(state: &State) -> Vec<String> {
    match rust_cli_output(&["list-boot-entries"]) {
        Ok(output) => {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&output) else {
                append_gui_log(state, "boot entries JSON parse failed; precheck skipped");
                return Vec::new();
            };
            // 【2026-10-16 审查修订，日志缺口 10】`bcdedit /enum` 失败时 CLI 会在
            // JSON 里带一个 `error` 字段。此前失败与"真的没有启动项"都表现为空
            // 列表，撞名预检会**静默失效**——名字重名要到任务创建时才炸。
            if let Some(error) = value.get("error").and_then(|item| item.as_str()) {
                append_gui_log(
                    state,
                    &format!("boot entries probe failed; collision precheck skipped: {error}"),
                );
            }
            value
                .get("descriptions")
                .and_then(|items| items.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        }
        Err(error) => {
            append_gui_log(
                state,
                &format!("boot entries unavailable; collision precheck skipped: {error}"),
            );
            Vec::new()
        }
    }
}

/// 压缩方式下拉显示文本。只保留「压缩/不压缩」两项，不再暴露 max/LZX。
/// 索引 0/1 固定对应 DISM 术语 fast/none（映射见 text_parsing）。
fn compress_level_labels(language: Language) -> [&'static str; 2] {
    if language == Language::Chinese {
        ["压缩", "不压缩"]
    } else {
        ["Compression", "No compression"]
    }
}

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

fn settings_ini_path() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(crate::failure_policy::SETTINGS_FILE_NAME)))
}

/// 读界面语言偏好。文件缺失 / 读失败 / 值不认识 → 中文（GUI 必须能起来）。
fn load_ui_language_pref() -> Language {
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
fn save_ui_language_pref(language: Language) {
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

unsafe fn apply_language(state: &mut State) {
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

fn rust_cli_output(arguments: &[&str]) -> Result<String, String> {
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

unsafe fn show_message(hwnd: Hwnd, text: &str, caption: &str, flags: u32) -> i32 {
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
const ID_CHOICE_BODY: usize = 2000;
const ID_CHOICE_PE: usize = 2001;
const ID_CHOICE_RE: usize = 2002;
const ID_CHOICE_CANCEL: usize = 2003;
const ID_CHOICE_CHOOSE_LOCATION: usize = 3001;
const ID_CHOICE_PATH_CANCEL: usize = 3002;


// 「备份文件管理…」窗口（2026-10-26 拆出，见子模块头部注释）。
mod choice_dialogs;
use choice_dialogs::{ask_invalid_image_path, ask_system_drive_handler, CHOICE_GENERIC_OFFLINE};

mod image_admin_window;
use image_admin_window::open_image_admin;

// 「查看日志」窗口（2026-10-26 拆出，见子模块头部注释）。
// 子模块而不是顶层模块：它要用这里一整套私有 Win32 类型/常量/辅助函数，
// 做成顶层就得把它们全提成 `pub(crate)`。
mod logs_viewer;
use logs_viewer::open_logs_viewer;

/// 顶栏「设置…」按钮的处理：弹窗 → 保存到设置文件 → 更新内存策略与 DISM 上限。
/// `EnumChildWindows` 的回调签名。
type EnumChildProc = unsafe extern "system" fn(Hwnd, LParam) -> i32;

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
unsafe fn apply_font_size(state: &mut State) {
    let font = gui_font();
    if font.is_null() {
        return;
    }
    FONT_TO_APPLY.store(font as isize, std::sync::atomic::Ordering::SeqCst);
    EnumChildWindows(state.root, apply_font_proc, 0);
    FONT_TO_APPLY.store(0, std::sync::atomic::Ordering::SeqCst);
    layout_operation(state);
}

unsafe fn open_settings(state: &mut State) {
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

unsafe fn is_elevated() -> bool {
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

unsafe fn relaunch_elevated() -> Result<(), super::TaskError> {
    let executable = std::env::current_exe()
        .map_err(|error| super::err(&format!("cannot resolve current executable: {error}")))?;
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
        return Err(super::err(&format!(
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

unsafe fn close_previous_gui_windows() {
    EnumWindows(
        Some(close_previous_gui_window),
        GetCurrentProcessId() as LParam,
    );
    // Give normal WM_CLOSE handling a short chance to release an old package
    // before the latest instance proceeds to occupy the foreground.
    Sleep(200);
}

unsafe fn refresh_environment(state: &mut State) {
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

unsafe fn refresh_task_status(state: &State) {
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
fn status_field_label(language: Language, key: &str) -> &'static str {
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

fn discover_drives() -> Vec<DriveInfo> {
    let output = rust_cli_output(&["list-volumes"]).unwrap_or_default();
    parse_drive_infos(&output).unwrap_or_default()
}

fn suggested_drive_defaults(_drives: &[DriveInfo]) -> (String, String) {
    let system = std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".to_string())
        .trim()
        .trim_end_matches(':')
        .to_ascii_uppercase();
    (system, String::new())
}

fn same_partition_by_gui_identity(left: Option<&DriveInfo>, right: Option<&DriveInfo>) -> bool {
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

unsafe fn read_image(state: &mut State) {
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

unsafe fn browse_image(state: &mut State) {
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
unsafe fn browse_pe_dir(state: &State) {
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
use crate::windows_command::{CmdOutcome, GetExitCodeProcess, WaitForSingleObject};
fn run_cmd_to_file(command: &str, out_file: Option<&Path>) -> CmdOutcome {
    run_cmd_to_file_timeout(command, out_file, 30000)
}
fn run_cmd_to_file_timeout(command: &str, out_file: Option<&Path>, timeout_ms: u32) -> CmdOutcome {
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
unsafe fn test_hook_auto_install(state: &mut State) {
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

unsafe fn install_pe_entry(state: &State) {
    let mode_ram = IsDlgButtonChecked(state.root, ID_PE_MODE_RAM as i32) != 0;
    if mode_ram {
        install_pe_ramdisk(state);
    } else {
        install_pe_harddisk(state);
    }
}

/// 启动项名称：按程序当前语言 + 启动模式（避免中英混合）。
fn pe_entry_description(language: Language, mode_ram: bool) -> String {
    if mode_ram {
        if language == Language::Chinese {
            "Windows PE (BackupRestore) 内存启动".to_string()
        } else {
            "Windows PE (BackupRestore) RAM".to_string()
        }
    } else if language == Language::Chinese {
        "Windows PE (BackupRestore) 硬盘启动".to_string()
    } else {
        "Windows PE (BackupRestore) Disk".to_string()
    }
}

/// 解析「PE 目录路径」输入框的完整路径（如 C:\BackupRestorePE 或 D:\a\PE）。
/// 返回 (盘符字符, 相对根目录路径，如 "\BackupRestorePE" / "\a\PE")。
/// 非法（缺盘符、根目录、含非法字符）返回 None。
fn split_pe_dir_path(raw: &str) -> Option<(char, String)> {
    let path = raw.trim().trim_end_matches('\\').trim_end_matches('/');
    if path.len() < 3 {
        return None;
    }
    let bytes = path.as_bytes();
    let drive = bytes[0].to_ascii_uppercase();
    if !drive.is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return None;
    }
    if path[3..].is_empty() {
        // 根目录（如 C:\），不允许：会把 PE 文件散落到盘根
        return None;
    }
    for ch in path[3..].chars() {
        if matches!(ch, '*' | '?' | '<' | '>' | '|' | '"') {
            return None;
        }
    }
    let rel = format!("\\{}", &path[3..]);
    Some((drive as char, rel))
}

/// RAM disk 模式：把 boot.wim 复制到 `{卷}:\{目录}`（不占独立分区），
/// bootmgr 通过 {ramdiskoptions} + boot.sdi 从内存启动。
/// 装 PE 之前先看一眼**这个镜像自己带的启动配置**对不对——**只警告，不拦**
/// （2026-10-29 用户拍板：`只警告，允许继续装`）。
///
/// 背景（实机踩过）：产品装 PE 是纯铺设器——格式化 + `dism /Apply-Image` + 建
/// BCD 项，**一个字节都不改镜像里的文件**。而「PE 启动后自动弹出恢复桌面」靠的是
/// 镜像自带的 `Windows\System32\winpeshl.ini`。若那份配置指向不存在的程序，装完
/// 重启就是「黑一下又回 Windows」，19 秒一轮无限循环，而且**从外面看不出原因**
/// （要进 PE 分区数日志条数才判得出）。机器一秒能查的事，不该让人查几小时。
///
/// 返回 `true` = 继续安装，`false` = 用户选择先不装。
///
/// ★ 探测本身失败（缺 wimlib、镜像正被占用、wimlib 报错）**一律不拦**：这是附加
/// 保护，不是安装的前置条件；把它做成硬门禁会让「装 PE」在缺 wimlib 的机器上
/// 整体不可用，而那种机器本来也能正常装 PE。
unsafe fn confirm_pe_image_shell(state: &State, image_path: &str) -> bool {
    let language = selected_language(state);
    let work_dir = state.executable_dir.join("_pe-shell-check");
    let log = crate::log_root::logs_dir().join("pe-shell-check.log");
    let probe = crate::wimlib_apply::probe_pe_image_shell(
        &state.executable_dir,
        Path::new(image_path),
        &work_dir,
        &log,
    );
    let _ = std::fs::remove_dir_all(&work_dir);
    let (shell, recovery_exe_present) = match probe {
        Ok(facts) => facts,
        Err(error) => {
            append_gui_log(
                state,
                &format!("PE image shell check skipped (probe failed): {error}"),
            );
            return true;
        }
    };
    let Some(problem) = crate::text_parsing::judge_pe_shell(&crate::text_parsing::PeShellFacts {
        shell: shell.as_deref(),
        recovery_exe_present,
    }) else {
        append_gui_log(state, "PE image shell check passed");
        return true;
    };
    append_gui_log(
        state,
        &format!(
            "PE image shell check: {} (image={image_path} log={})",
            problem.describe(),
            log.display()
        ),
    );
    let detail = match &problem {
        crate::text_parsing::PeShellProblem::Missing => {
            if language == Language::English {
                "The image has no Windows\\System32\\winpeshl.ini, so WinPE would stop at a bare command prompt."
                    .to_string()
            } else {
                "镜像里没有 Windows\\System32\\winpeshl.ini 这个文件，PE 启动后只会停在一个黑命令行窗口。"
                    .to_string()
            }
        }
        crate::text_parsing::PeShellProblem::Mismatch { bytes, actual } => {
            if language == Language::English {
                format!(
                    "That file is {bytes} bytes and is not the expected shell (expected 61 bytes: one line, Recovery.exe,--pe-desktop). It reads: {actual}"
                )
            } else {
                format!(
                    "这个文件是 {bytes} 字节，内容不是产品期望的那一份（期望 61 字节、只有一行 Recovery.exe,--pe-desktop）。它的实际内容是：{actual}"
                )
            }
        }
        crate::text_parsing::PeShellProblem::RecoveryExeMissing => {
            if language == Language::English {
                "The shell file is correct, but the Recovery.exe it points at is not inside this image."
                    .to_string()
            } else {
                "启动配置写对了，但它指向的 Recovery.exe 不在这个镜像里。".to_string()
            }
        }
    };
    if TEST_AUTO_CONFIRM.load(std::sync::atomic::Ordering::SeqCst) {
        append_gui_log(
            state,
            "test hook: PE image shell warning auto-continued",
        );
        return true;
    }
    let message = if language == Language::English {
        format!(
            "This PE image's startup configuration is not what the product expects.\n\n{detail}\n\nInstalling it anyway will most likely give you a screen that flashes black and returns to Windows, over and over, every ~19 seconds. The product does not modify the image, so the install itself would still succeed.\n\nContinue installing anyway?"
        )
    } else {
        format!(
            "这个 PE 镜像的启动配置不对。\n\n{detail}\n\n照这样装下去，重启选 PE 之后大概率会「黑一下又回到 Windows」，而且反复循环（约 19 秒一轮）。安装动作本身会成功——产品只是把镜像原样铺到分区，不改里面的任何文件。\n\n仍然继续安装吗？"
        )
    };
    let answer = show_message(
        state.root,
        &message,
        if language == Language::English {
            "PE image self-check"
        } else {
            "PE 镜像自检"
        },
        MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
    );
    if answer != IDYES {
        append_gui_log(
            state,
            "GUI action cancelled: PE image shell warning declined by user",
        );
        return false;
    }
    append_gui_log(
        state,
        "PE image shell warning acknowledged; continuing by user choice",
    );
    true
}

unsafe fn install_pe_ramdisk(state: &State) {
    let language = selected_language(state);
    let image_path = get_text(state.controls.image).trim().to_string();
    if !PathBuf::from(&image_path).is_file() {
        append_gui_log(state, "GUI action blocked: PE WIM file does not exist");
        show_message(
            state.root,
            &if language == Language::English {
                format!("PE WIM file not found: {image_path}")
            } else {
                format!("PE 镜像文件不存在：{image_path}")
            },
            if language == Language::English {
                "Validation failed"
            } else {
                "参数校验失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 读「PE 目录路径」：完整路径（如 C:\BackupRestorePE，盘符任意合法目录）。
    // RAM disk 模式把 boot.wim 复制到该路径的 sources\boot.wim，不占独立分区。
    let dir_path = {
        let raw = get_text(GetDlgItem(state.root, ID_PE_DIR_EDIT as i32))
            .trim()
            .to_string();
        if raw.trim().is_empty() {
            // 布局层已填默认值，这里防御性兜底
            let system = std::env::var("SystemDrive")
                .unwrap_or_else(|_| "C:".to_string())
                .trim()
                .trim_end_matches(':')
                .to_ascii_uppercase();
            format!("{system}:\\BackupRestorePE")
        } else {
            raw
        }
    };
    let (target_char, rel_path) = match split_pe_dir_path(&dir_path) {
        Some(value) => value,
        None => {
            append_gui_log(state, "GUI action blocked: invalid PE directory path");
            show_message(
                state.root,
                &if language == Language::English {
                    format!(
                        "Invalid PE directory path \"{dir_path}\". Enter a full path like C:\\BackupRestorePE (any drive letter is allowed); it must not be a drive root."
                    )
                } else {
                    format!(
                        "PE 目录路径不合法：\"{dir_path}\"。请填写完整路径（如 C:\\BackupRestorePE，盘符可以是任意合法目录），不能是盘符根目录。"
                    )
                },
                if language == Language::English {
                    "Validation failed"
                } else {
                    "参数校验失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    };
    // 启动项名称：读共用的「PE 启动项名称」输入框，空则按语言+模式用默认名
    let entry_name = {
        let typed = get_text(GetDlgItem(state.root, ID_PE_NAME_EDIT as i32))
            .trim()
            .to_string();
        if typed.is_empty() {
            pe_entry_description(language, true)
        } else {
            typed
        }
    };
    if target_char
        == state
            .executable_dir
            .to_string_lossy()
            .chars()
            .next()
            .unwrap_or(' ')
    {
        append_gui_log(
            state,
            "GUI action blocked: target volume is the program volume",
        );
        show_message(
            state.root,
            if language == Language::English {
                "The target volume must differ from the volume that runs this program."
            } else {
                "目标卷不能与运行本程序的卷相同。"
            },
            if language == Language::English {
                "Validation failed"
            } else {
                "参数校验失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 装之前先看镜像自带的启动配置（只警告，不拦）。放在破坏性确认**之前**：
    // 能改主意的时候给信息，比格式化完再讲有用。
    if !confirm_pe_image_shell(state, &image_path) {
        return;
    }
    // Destructive-ish confirmation: overwrites <target>:\<dir>\sources\boot.wim
    // and modifies the boot configuration. Nothing runs before this confirmation.
    let answer = if TEST_AUTO_CONFIRM.load(std::sync::atomic::Ordering::SeqCst) {
        append_gui_log(state, "test hook: RAM disk confirmation auto-accepted");
        IDYES
    } else {
        show_message(
            state.root,
            &if language == Language::English {
                format!(
                    "Install PE (RAM disk) to {dir_path}?\n\n- Copy the PE WIM to {dir_path}\\sources\\boot.wim\n- Ensure {dir_path}\\boot\\boot.sdi\n- Add boot entry \"{entry_name}\" to the boot menu\n\nNo extra partition is used. The current Windows default boot is NOT changed. Continue?"
                )
            } else {
                format!(
                    "以 RAM disk 方式安装 PE 到 {dir_path}？\n\n- 复制 PE 镜像到 {dir_path}\\sources\\boot.wim\n- 确保 {dir_path}\\boot\\boot.sdi 存在\n- 在启动菜单新增「{entry_name}」启动项\n\n不占用独立分区。不修改当前 Windows 默认启动。是否继续？"
                )
            },
            if language == Language::English {
                "Install PE"
            } else {
                "安装 PE"
            },
            MB_YESNO | MB_ICONWARNING,
        )
    };
    if answer != IDYES {
        append_gui_log(
            state,
            "GUI action cancelled: PE install confirmation declined",
        );
        return;
    }
    append_gui_log(
        state,
        &format!("PE RAM disk install started: wim={image_path} target={dir_path}"),
    );
    let target_root = format!("{dir_path}\\");
    // 1. Copy the PE WIM into <target>:\<dir>\sources\boot.wim, preserving an
    //    existing file as .stock on first install.
    let sources_dir = format!("{target_root}sources");
    if !PathBuf::from(&sources_dir).is_dir() {
        let _ = log_fs(
            state,
            &format!("创建 PE 源目录 {sources_dir}"),
            std::fs::create_dir_all(&sources_dir),
        );
    }
    let wim_dest = PathBuf::from(format!("{sources_dir}\\boot.wim"));
    if wim_dest.is_file() {
        let stock = PathBuf::from(format!("{sources_dir}\\boot.wim.stock"));
        if !stock.is_file() {
            let _ = log_fs(
                state,
                &format!("备份既有 boot.wim 到 {}", stock.display()),
                std::fs::copy(&wim_dest, &stock),
            );
            append_gui_log(
                state,
                "PE install: existing boot.wim kept as boot.wim.stock",
            );
        }
    }
    if let Err(error) = std::fs::copy(&image_path, &wim_dest) {
        append_gui_log(state, &format!("PE install failed: copy boot.wim: {error}"));
        show_message(
            state.root,
            &format!(
                "{}: {error}",
                if language == Language::English {
                    "Failed to copy the PE WIM"
                } else {
                    "复制 PE 镜像失败"
                }
            ),
            if language == Language::English {
                "PE install failed"
            } else {
                "安装失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 2. Ensure <target>:\boot\boot.sdi exists (ramdisk SDI template).
    let boot_dir = format!("{target_root}boot");
    let sdi_dest = PathBuf::from(format!("{boot_dir}\\boot.sdi"));
    if !sdi_dest.is_file() {
        let _ = log_fs(
            state,
            &format!("创建 PE 引导目录 {boot_dir}"),
            std::fs::create_dir_all(&boot_dir),
        );
        let mut sdi_source: Option<PathBuf> = None;
        for candidate in [
            "C:\\WinPE_arm64\\media\\boot\\boot.sdi",
            "C:\\Program Files (x86)\\Windows Kits\\10\\Assessment and Deployment Kit\\Windows Preinstallation Environment\\arm64\\media\\boot\\boot.sdi",
        ] {
            if PathBuf::from(candidate).is_file() {
                sdi_source = Some(PathBuf::from(candidate));
                break;
            }
        }
        if let Some(src) = sdi_source
            && let Err(error) = std::fs::copy(&src, &sdi_dest)
        {
            append_gui_log(state, &format!("PE install failed: copy boot.sdi: {error}"));
        }
        if !sdi_dest.is_file() {
            append_gui_log(
                state,
                "PE install blocked: <target>:\\<dir>\\boot\\boot.sdi is missing and no ADK copy was found",
            );
            show_message(
                state.root,
                &if language == Language::English {
                    format!(
                        "{dir_path}\\boot\\boot.sdi is missing and no ADK copy could be found. Place the Windows PE boot.sdi on the target volume and try again."
                    )
                } else {
                    format!(
                        "{dir_path}\\boot\\boot.sdi 不存在，且未找到 ADK 副本。请将 WinPE 的 boot.sdi 放到目标卷后重试。"
                    )
                },
                if language == Language::English {
                    "PE install failed"
                } else {
                    "安装失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    }
    // 3. BCD: reuse the system {ramdiskoptions} entry (bcdedit on this
    //    platform rejects /create /application ramdisk), repoint it at the
    //    target volume/folder, then create the PE osloader entry and append
    //    it to the display order.
    let guid_out = state.executable_dir.join("_pe-bcd-guid.txt");
    let guid_out = guid_out.to_string_lossy().to_string();
    // 【2026-10-16 审查修订，日志缺口 8】bcdedit 的输出目录先建好：下面 GUID
    // 解析失败与每个 /set 步骤失败都要把原始输出落到这里，而不是进 NUL。
    let bcd_out_dir = crate::log_root::logs_dir().to_path_buf();
    let _ = log_fs(
        state,
        &format!("创建日志输出目录 {}", &bcd_out_dir.display()),
        std::fs::create_dir_all(&bcd_out_dir),
    );
    let _ = std::fs::remove_file(&guid_out);
    let ram_guid_path = "{ramdiskoptions}".to_string();
    let mut steps = vec![
        format!("bcdedit.exe /set {ram_guid_path} ramdisksdidevice partition={target_char}:"),
        format!("bcdedit.exe /set {ram_guid_path} ramdisksdipath {rel_path}\\boot\\boot.sdi"),
    ];
    let os_guid = {
        let _ = std::fs::remove_file(&guid_out);
        let code = run_cmd_to_file(
            &format!("bcdedit.exe /create /d \"{entry_name}\" /application osloader"),
            Some(std::path::Path::new(&guid_out)),
        );
        if !code.is_success() {
            append_gui_log(
                state,
                &format!("PE install failed: bcdedit create osloader code={code}"),
            );
            show_message(
                state.root,
                &if language == Language::English {
                    format!("bcdedit failed to create the PE boot entry (code {code}).")
                } else {
                    format!("bcdedit 创建 PE 启动项失败（退出码 {code}）。")
                },
                if language == Language::English {
                    "PE install failed"
                } else {
                    "安装失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
        match extract_bcd_guid(&guid_out) {
            Some(guid) => guid,
            None => {
                // 【2026-10-16 审查修订，日志缺口 8】把 bcdedit 的原始输出落盘：
                // 此前解析失败只弹一句"无法读取 GUID"，输出本身没进任何地方，
                // 事后无从判断是 bcdedit 没输出、还是输出格式变了（后者才是
                // 真正需要修的地方）。
                let raw = std::fs::read(&guid_out)
                    .map(|bytes| decode_bcdedit_bytes(&bytes))
                    .unwrap_or_default();
                let _ = log_fs(
                    state,
                    "留存 bcdedit 原始输出",
                    std::fs::copy(&guid_out, bcd_out_dir.join("pe-install-bcdedit-create.out")),
                );
                append_gui_log(
                    state,
                    &format!("PE install failed: could not parse GUID from bcdedit output: {raw}"),
                );
                show_message(
                    state.root,
                    if language == Language::English {
                        "Could not read the created PE boot entry GUID."
                    } else {
                        "无法读取新建的 PE 启动项 GUID。"
                    },
                    if language == Language::English {
                        "PE install failed"
                    } else {
                        "安装失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        }
    };
    let os_guid_path = format!("{{{}}}", os_guid);
    let ramdisk_device =
        format!("ramdisk=[{target_char}:]{rel_path}\\sources\\boot.wim,{ram_guid_path}");
    steps.push(format!(
        "bcdedit.exe /set {os_guid_path} device {ramdisk_device}"
    ));
    steps.push(format!(
        "bcdedit.exe /set {os_guid_path} osdevice {ramdisk_device}"
    ));
    steps.push(format!("bcdedit.exe /set {os_guid_path} winpe yes"));
    steps.push(format!("bcdedit.exe /set {os_guid_path} detecthal yes"));
    steps.push(format!(
        "bcdedit.exe /set {os_guid_path} systemroot \\windows"
    ));
    steps.push(format!("bcdedit.exe /set {os_guid_path} nx OptIn"));
    steps.push(format!(
        "bcdedit.exe /set {os_guid_path} description \"{entry_name}\""
    ));
    steps.push(format!("bcdedit.exe /displayorder {os_guid_path} /addlast"));
    let mut failed_step: Option<String> = None;
    // 【2026-10-16 审查修订，日志缺口 8】每个 bcdedit 步骤的输出落到独立文件：
    // 此前 `run_cmd_to_file(step, None)` 把三流全部送进 NUL，失败时只剩一个
    // 退出码，而 bcdedit 的真实原因（"拒绝访问"/"找不到元素"/GUID 拼错）
    // 恰恰写在输出里。文件放在程序目录，与 `_pe-bcd-guid.txt` 同一处。
    let bcd_out_dir = crate::log_root::logs_dir().to_path_buf();
    let _ = log_fs(
        state,
        &format!("创建日志输出目录 {}", &bcd_out_dir.display()),
        std::fs::create_dir_all(&bcd_out_dir),
    );
    for (index, step) in steps.iter().enumerate() {
        let out_file = bcd_out_dir.join(format!("pe-install-bcdedit-{index}.out"));
        let _ = std::fs::remove_file(&out_file);
        let code = run_cmd_to_file(step, Some(&out_file));
        let detail = std::fs::read(&out_file)
            .map(|bytes| decode_bcdedit_bytes(&bytes))
            .unwrap_or_default();
        let detail = detail.trim();
        append_gui_log(
            state,
            &format!(
                "PE install bcdedit: {step} -> {code}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    crate::i18n::msg_tpl("pe_output_suffix", &[&detail])
                }
            ),
        );
        if !code.is_success() {
            failed_step = Some(if detail.is_empty() {
                step.clone()
            } else {
                format!("{step}\n{detail}")
            });
            break;
        }
    }
    if let Some(step) = failed_step {
        show_message(
            state.root,
            &format!(
                "{}: {}",
                if language == Language::English {
                    "bcdedit step failed"
                } else {
                    "bcdedit 步骤失败"
                },
                step
            ),
            if language == Language::English {
                "PE install failed"
            } else {
                "安装失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let _ = std::fs::remove_file(&guid_out);
    // 记录 PE 启动项 GUID（裸 GUID），供「重启进入 PE」按钮设置 bootsequence 使用。
    let guid_file = state.executable_dir.join("pe-entry-guid.txt");
    let _ = std::fs::write(&guid_file, os_guid.trim());
    append_gui_log(
        state,
        &format!(
            "GUI action completed: PE installed (RAM disk wim -> {dir_path}\\sources\\boot.wim, BCD entry {os_guid_path})"
        ),
    );
    show_message(
        state.root,
        &if language == Language::English {
            format!(
                "PE (RAM disk) installed to {dir_path}.\n\nThe boot menu now has \"{entry_name}\". Reboot and choose it from the menu to enter the recovery desktop. The current Windows default boot is unchanged."
            )
        } else {
            format!(
                "PE（RAM disk）已安装到 {dir_path}。\n\n启动菜单已新增「{entry_name}」。重启后从菜单选择即可进入恢复桌面。当前 Windows 默认启动未改动。"
            )
        },
        if language == Language::English {
            "Install complete"
        } else {
            "安装完成"
        },
        MB_OK,
    );
}

/// 硬盘启动模式：把 boot.wim Apply 到目标独立分区（先格式化），
/// BCD 建 osloader 条目并带 winpe/detecthal 标志，直接从分区引导 PE。
unsafe fn install_pe_harddisk(state: &State) {
    let language = selected_language(state);
    let image_path = get_text(state.controls.image).trim().to_string();
    if !PathBuf::from(&image_path).is_file() {
        append_gui_log(state, "GUI action blocked: PE WIM file does not exist");
        show_message(
            state.root,
            &if language == Language::English {
                format!("PE WIM file not found: {image_path}")
            } else {
                format!("PE 镜像文件不存在：{image_path}")
            },
            if language == Language::English {
                "Validation failed"
            } else {
                "参数校验失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let target_drive = match selected_drive_letter(state, state.controls.target) {
        Some(value) => value,
        None => {
            append_gui_log(state, "GUI action blocked: no target partition selected");
            show_message(
                state.root,
                if language == Language::English {
                    "Select the target partition for the PE install."
                } else {
                    "请先选择 PE 安装的目标分区。"
                },
                if language == Language::English {
                    "Validation failed"
                } else {
                    "参数校验失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    };
    let drive_char = match target_drive.chars().next() {
        Some(letter) => letter.to_ascii_uppercase(),
        // 取不到目标盘符就拒绝，不退回字母 C——那会凭空造出一个"合法目标"。
        None => {
            append_gui_log(state, "GUI action blocked: target drive letter unavailable");
            show_message(
                state.root,
                if language == Language::English {
                    "Could not determine the target drive letter."
                } else {
                    "无法确定目标分区的盘符。"
                },
                if language == Language::English {
                    "Validation failed"
                } else {
                    "参数校验失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    };
    // 目标分区安全校验：非系统卷、非程序所在卷、非 ESP、可用空间 >= 2GB。
    //
    // 2026-10-16：系统盘改按 `%SystemDrive%` 判、ESP 改按分区类型 GUID 判，
    // 不再写死 `'C'` / `'S'`。此前那两种写法在「系统盘不是 C」「ESP 不在 S」
    // 的机器上拦不住真正的危险目标，在「C 或 S 其实是数据盘」的机器上又会误拦。
    let target_info = state
        .drives
        .iter()
        .find(|drive| drive.letter.eq_ignore_ascii_case(&drive_char.to_string()));
    let mut is_system_volume = false;
    let mut free_bytes: u64 = 0;
    if let Some(drive) = target_info {
        is_system_volume = drive.has_windows_installation;
        free_bytes = drive.free_bytes.unwrap_or(0);
    }
    let on_system_drive = system_drive_letter() == Some(drive_char);
    let on_esp = is_efi_system_partition(target_info);
    let on_program_drive = is_program_drive(drive_char, &state.executable_dir);
    if on_system_drive || on_esp || on_program_drive {
        append_gui_log(
            state,
            &format!(
                "GUI action blocked: target {drive_char}: is system={on_system_drive} esp={on_esp} program={on_program_drive}"
            ),
        );
        show_message(
            state.root,
            if language == Language::English {
                "The target partition cannot be the system drive, the ESP or the volume that runs this program."
            } else {
                "目标分区不能是系统盘、ESP 或运行本程序的卷。"
            },
            if language == Language::English {
                "Validation failed"
            } else {
                "参数校验失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    if is_system_volume {
        append_gui_log(
            state,
            "GUI action blocked: target partition contains a Windows installation",
        );
        show_message(
            state.root,
            if language == Language::English {
                "The target partition contains a Windows installation. Choose an empty or dedicated partition."
            } else {
                "目标分区包含 Windows 系统，请选择空分区或专用分区。"
            },
            if language == Language::English {
                "Validation failed"
            } else {
                "参数校验失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    if free_bytes < 2 * 1024 * 1024 * 1024 {
        append_gui_log(
            state,
            "GUI action blocked: target partition free space < 2GB",
        );
        show_message(
            state.root,
            &if language == Language::English {
                format!(
                    "Target partition {drive_char}: free space is below 2 GB ({}).",
                    format_bytes(Some(free_bytes))
                )
            } else {
                format!(
                    "目标分区 {drive_char}: 可用空间不足 2GB（{}）。",
                    format_bytes(Some(free_bytes))
                )
            },
            if language == Language::English {
                "Validation failed"
            } else {
                "参数校验失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 启动项名称：读共用的「PE 启动项名称」输入框，空则按语言+模式用默认名
    let entry_name = {
        let typed = get_text(GetDlgItem(state.root, ID_PE_NAME_EDIT as i32))
            .trim()
            .to_string();
        if typed.is_empty() {
            pe_entry_description(language, false)
        } else {
            typed
        }
    };
    // 装之前先看镜像自带的启动配置（只警告，不拦）。放在格式化确认之前。
    if !confirm_pe_image_shell(state, &image_path) {
        return;
    }
    // 破坏性确认：目标分区将被格式化 + 写入 PE 系统
    let answer = if TEST_AUTO_CONFIRM.load(std::sync::atomic::Ordering::SeqCst) {
        append_gui_log(state, "test hook: hard disk confirmation auto-accepted");
        IDYES
    } else {
        show_message(
            state.root,
            &if language == Language::English {
                format!(
                    "Install PE (hard disk boot) to partition {drive_char}:?\n\n- The partition WILL BE FORMATTED (all data on it is lost)\n- Apply the PE WIM to {drive_char}:\\\n- Add boot entry \"{entry_name}\" to the boot menu\n\nThe current Windows default boot is NOT changed. Continue?"
                )
            } else {
                format!(
                    "以硬盘启动方式安装 PE 到分区 {drive_char}:？\n\n- 该分区将被格式化（数据全部丢失！）\n- 将 PE 镜像展开到 {drive_char}:\\\n- 在启动菜单新增「{entry_name}」启动项\n\n不修改当前 Windows 默认启动。是否继续？"
                )
            },
            if language == Language::English {
                "Install PE"
            } else {
                "安装 PE"
            },
            MB_YESNO | MB_ICONWARNING,
        )
    };
    if answer != IDYES {
        append_gui_log(
            state,
            "GUI action cancelled: PE hard disk install confirmation declined",
        );
        return;
    }
    append_gui_log(
        state,
        &format!("PE hard disk install started: wim={image_path} partition={drive_char}:"),
    );
    // 不能仅依赖 diskpart 退出码：按物理分区选择、保存输出，并核对新文件系统序列号。
    let format_log = crate::log_root::logs_dir().join("pe-format.log");
    let format_result = (|| -> Result<(), backuprestore_core::TaskError> {
        let before = crate::windows_prepare::volume_identity(drive_char)?;
        if before.volume_serial.is_empty() {
            return Err(crate::err(
                "cannot verify format without original volume serial",
            ));
        }
        let script = crate::diskpart_format_script(before.disk_number, before.partition_number)?;
        let script_file = state.executable_dir.join("_pe-format.txt");
        std::fs::write(&script_file, &script)?;
        crate::append_log(
            &format_log,
            &crate::i18n::msg_tpl("pe_fmt_before", &[&format!("{before:?}"), script.as_str()]),
        )?;
        let result = crate::windows_command::run_program(
            "diskpart.exe",
            &["/s".into(), script_file.to_string_lossy().into_owned()],
            Some(&format_log),
            300000,
        );
        crate::append_log(&format_log, &crate::i18n::msg_tpl("pe_diskpart_result", &[&format!("{result}")]))?;
        if !result.is_success() {
            return Err(crate::err(&format!("diskpart failed: {result}")));
        }
        let after = crate::windows_prepare::volume_identity(drive_char)?;
        crate::append_log(&format_log, &crate::i18n::msg_tpl("pe_fmt_after", &[&format!("{after:?}")]))?;
        backuprestore_core::operation_safety::verify_formatted_volume(&before, &after)?;
        if !Path::new(&format!("{drive_char}:\\")).is_dir() {
            return Err(crate::err("formatted volume root inaccessible"));
        }
        std::fs::remove_file(script_file)?;
        crate::append_log(
            &format_log,
            crate::i18n::msg("pe_fmt_verified"),
        )
    })();
    if let Err(error) = format_result {
        append_gui_log(
            state,
            &format!(
                "PE format failed; refusing apply: {error}; log={}",
                format_log.display()
            ),
        );
        show_message(
            state.root,
            &format!(
                "无法确认格式化成功，已停止展开镜像。\n{error}\n日志：{}",
                format_log.display()
            ),
            "安装失败",
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 2. Apply PE WIM（索引 1 = WinPE）到分区
    // 【2026-10-16 审查修订，卡死 3】等待上限可配置（`dism_timeout`），默认仍
    // INFINITE。此前硬编码 INFINITE，且这条调用链从 `WM_COMMAND` **同步**执行
    // （`4367 install_pe_entry` → `4779 install_pe_harddisk`，不开线程）：
    // DISM 挂死则主窗口消息循环被阻塞，整窗「未响应」、无进度、无取消，
    // 只能任务管理器杀进程。
    let apply_wait_ms = crate::dism_timeout::wait_ms();
    let apply_code = run_cmd_to_file_timeout(
        &format!(
            "dism /Apply-Image /ImageFile:\"{image_path}\" /Index:1 /ApplyDir:{drive_char}:\\"
        ),
        None,
        apply_wait_ms,
    );
    if !apply_code.is_success() {
        append_gui_log(
            state,
            &format!("PE hard disk install failed: dism apply code={apply_code}"),
        );
        show_message(
            state.root,
            &if language == Language::English {
                format!("Failed to apply the PE WIM to {drive_char}:\\ (code {apply_code}).")
            } else {
                format!("将 PE 镜像展开到 {drive_char}:\\ 失败（退出码 {apply_code}）。")
            },
            if language == Language::English {
                "PE install failed"
            } else {
                "安装失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 3. BCD：建 osloader 条目（winpe/detecthal 标志齐全），加到显示顺序
    // 【2026-10-16 审查修订，日志缺口 8】bcdedit 的输出目录先建好：下面
    // GUID 解析失败与每个 /set 步骤失败都要把原始输出落到这里，而不是进 NUL。
    let bcd_out_dir = crate::log_root::logs_dir().to_path_buf();
    let _ = log_fs(
        state,
        &format!("创建日志输出目录 {}", &bcd_out_dir.display()),
        std::fs::create_dir_all(&bcd_out_dir),
    );
    let guid_out = state
        .executable_dir
        .join("_pe-bcd-guid.txt")
        .to_string_lossy()
        .to_string();
    let _ = std::fs::remove_file(&guid_out);
    let os_guid = {
        let code = run_cmd_to_file(
            &format!("bcdedit.exe /create /d \"{entry_name}\" /application osloader"),
            Some(std::path::Path::new(&guid_out)),
        );
        if !code.is_success() {
            append_gui_log(
                state,
                &format!("PE hard disk install failed: bcdedit create code={code}"),
            );
            show_message(
                state.root,
                &if language == Language::English {
                    format!("bcdedit failed to create the PE boot entry (code {code}).")
                } else {
                    format!("bcdedit 创建 PE 启动项失败（退出码 {code}）。")
                },
                if language == Language::English {
                    "PE install failed"
                } else {
                    "安装失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
        match extract_bcd_guid(&guid_out) {
            Some(guid) => guid,
            None => {
                // 【2026-10-16 审查修订，日志缺口 8】同上：原始输出落盘 + 写进
                // GUI 日志，事后能区分"bcdedit 没输出"与"输出格式变了"。
                let raw = std::fs::read(&guid_out)
                    .map(|bytes| decode_bcdedit_bytes(&bytes))
                    .unwrap_or_default();
                let _ = log_fs(
                    state,
                    "留存 bcdedit 原始输出",
                    std::fs::copy(&guid_out, bcd_out_dir.join("pe-hdd-bcdedit-create.out")),
                );
                append_gui_log(
                    state,
                    &format!(
                        "PE hard disk install failed: could not parse GUID from bcdedit output: {raw}"
                    ),
                );
                show_message(
                    state.root,
                    if language == Language::English {
                        "Could not read the created PE boot entry GUID."
                    } else {
                        "无法读取新建的 PE 启动项 GUID。"
                    },
                    if language == Language::English {
                        "PE install failed"
                    } else {
                        "安装失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        }
    };
    let os_guid_path = format!("{{{}}}", os_guid);
    let steps = vec![
        format!("bcdedit.exe /set {os_guid_path} device partition={drive_char}:"),
        format!("bcdedit.exe /set {os_guid_path} osdevice partition={drive_char}:"),
        format!("bcdedit.exe /set {os_guid_path} path \\Windows\\system32\\winload.efi"),
        format!("bcdedit.exe /set {os_guid_path} systemroot \\Windows"),
        format!("bcdedit.exe /set {os_guid_path} winpe yes"),
        format!("bcdedit.exe /set {os_guid_path} detecthal yes"),
        format!("bcdedit.exe /set {os_guid_path} nx OptIn"),
        format!("bcdedit.exe /set {os_guid_path} description \"{entry_name}\""),
        format!("bcdedit.exe /displayorder {os_guid_path} /addlast"),
    ];
    let mut failed_step: Option<String> = None;
    // 【2026-10-16 审查修订，日志缺口 8】bcdedit 的输出落盘而不是进 NUL：
    // 此前 `run_cmd_to_file(step, None)` 把三流全送 NUL，失败只剩退出码，
    // 而 bcdedit 的真实原因（拒绝访问 / 找不到元素 / GUID 拼错）写在输出里。
    let bcd_out_dir = crate::log_root::logs_dir().to_path_buf();
    let _ = log_fs(
        state,
        &format!("创建日志输出目录 {}", &bcd_out_dir.display()),
        std::fs::create_dir_all(&bcd_out_dir),
    );
    for (index, step) in steps.iter().enumerate() {
        let out_file = bcd_out_dir.join(format!("pe-hdd-bcdedit-{index}.out"));
        let _ = std::fs::remove_file(&out_file);
        let code = run_cmd_to_file(step, Some(&out_file));
        let detail = std::fs::read(&out_file)
            .map(|bytes| decode_bcdedit_bytes(&bytes))
            .unwrap_or_default();
        let detail = detail.trim();
        append_gui_log(
            state,
            &format!(
                "PE hard disk bcdedit: {step} -> {code}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    crate::i18n::msg_tpl("pe_output_suffix", &[&detail])
                }
            ),
        );
        if !code.is_success() {
            failed_step = Some(if detail.is_empty() {
                step.clone()
            } else {
                format!("{step}\n{detail}")
            });
            break;
        }
    }
    if let Some(step) = failed_step {
        show_message(
            state.root,
            &format!(
                "{}: {}",
                if language == Language::English {
                    "bcdedit step failed"
                } else {
                    "bcdedit 步骤失败"
                },
                step
            ),
            if language == Language::English {
                "PE install failed"
            } else {
                "安装失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let _ = std::fs::remove_file(&guid_out);
    // 记录 PE 启动项 GUID，供「重启进入 PE」按钮使用
    let guid_file = state.executable_dir.join("pe-entry-guid.txt");
    let _ = std::fs::write(&guid_file, os_guid.trim());
    append_gui_log(
        state,
        &format!(
            "GUI action completed: PE installed (hard disk -> {drive_char}:\\, BCD entry {os_guid_path})"
        ),
    );
    show_message(
        state.root,
        &if language == Language::English {
            format!(
                "PE (hard disk boot) installed to partition {drive_char}:.\n\nThe boot menu now has \"{entry_name}\". Reboot and choose it from the menu to enter the recovery desktop. The current Windows default boot is unchanged."
            )
        } else {
            format!(
                "PE（硬盘启动）已安装到分区 {drive_char}:。\n\n启动菜单已新增「{entry_name}」。重启后从菜单选择即可进入恢复桌面。当前 Windows 默认启动未改动。"
            )
        },
        if language == Language::English {
            "Install complete"
        } else {
            "安装完成"
        },
        MB_OK,
    );
}

/// Parse the first `{xxxxxxxx-....}` GUID from a bcdedit output file and
/// return it WITHOUT the surrounding braces (callers wrap with `{}` as
/// needed). Decoding and scanning live in `text_parsing` so both are covered
/// by the offline test suite.
fn extract_bcd_guid(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    first_braced_guid(&decode_bcdedit_bytes(&bytes))
}

/// Get the current user's shell desktop, including known-folder redirection.
unsafe fn current_desktop_dir() -> Option<PathBuf> {
    let mut path = [0_u16; 260];
    if SHGetFolderPathW(
        null_mut(),
        CSIDL_DESKTOPDIRECTORY,
        null_mut(),
        SHGFP_TYPE_CURRENT,
        path.as_mut_ptr(),
    ) != 0
    {
        return None;
    }
    let length = path.iter().position(|value| *value == 0)?;
    (length > 0).then(|| PathBuf::from(String::from_utf16_lossy(&path[..length])))
}

/// Windows opens file URLs from a .url shell shortcut without a script host.
fn file_url(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let mut encoded = String::new();
    for byte in normalized.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~' | b'/' | b':')
        {
            encoded.push(*byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    format!("file:///{encoded}")
}

/// 「创建快捷方式」：在用户桌面创建指向 BackupRestore.exe 本身的 Windows
/// Internet shortcut（.url）。它由 Shell 直接解析，不依赖 PowerShell、WScript 或
/// 控制台窗口；双击会打开主 GUI。
unsafe fn pe_create_shortcut(state: &State) {
    let language = selected_language(state);
    let executable =
        std::env::current_exe().unwrap_or_else(|_| state.executable_dir.join("BackupRestore.exe"));
    let mut directories = Vec::new();
    if let Some(desktop) = current_desktop_dir() {
        directories.push(desktop);
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        directories.push(PathBuf::from(profile).join("Desktop"));
    }
    // Parallels can redirect the user desktop to the shared macOS desktop.
    directories.push(PathBuf::from(r"C:\Mac\Home\Desktop"));
    directories.retain(|directory| directory.is_dir());
    directories.sort();
    directories.dedup();

    let contents = format!(
        "[InternetShortcut]\r\nURL={}\r\nIconFile={}\r\nIconIndex=0\r\n",
        file_url(&executable),
        executable.display(),
    );
    let mut created = Vec::new();
    let mut failures = Vec::new();
    for directory in directories {
        let shortcut = directory.join("BackupRestore.url");
        match fs::write(&shortcut, &contents) {
            Ok(()) => created.push(shortcut),
            Err(error) => failures.push(format!("{}: {error}", shortcut.display())),
        }
    }
    append_gui_log(
        state,
        &format!(
            "PE create shortcut: created=[{}] failures=[{}]",
            created
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("; "),
            failures.join("; ")
        ),
    );
    if !created.is_empty() {
        show_message(
            state.root,
            if language == Language::English {
                "Desktop shortcut \"BackupRestore\" created. Double-click it to open the program."
            } else {
                "已创建桌面快捷方式「BackupRestore」。双击即可打开程序。"
            },
            if language == Language::English {
                "Shortcut created"
            } else {
                "快捷方式已创建"
            },
            MB_OK | MB_ICONINFORMATION,
        );
    } else {
        show_message(
            state.root,
            &if language == Language::English {
                "Failed to create a desktop shortcut. No desktop directory was writable."
                    .to_string()
            } else {
                "创建桌面快捷方式失败：没有可写入的桌面目录。".to_string()
            },
            if language == Language::English {
                "Failed"
            } else {
                "创建失败"
            },
            MB_OK | MB_ICONERROR,
        );
    }
}

/// 「改引导 + 立刻重启」这类动作的统一执行入口。
///
/// 为什么要有它（2026-10-17 第二轮核查，日志缺口 3）：三处设置
/// `bootsequence` 的入口此前都只留「命令 + 退出码」两行（快捷方式那条连这两行
/// 都没有），而 **bcdedit 的真实输出**（元素未找到 / 拒绝访问 / 参数无效）
/// 一个字都没留下 —— 偏偏这几处的后果是**马上重启这台机器**。
///
/// 日志路径统一取 `log_root::logs_dir()`（2026-10-18 用户要求：不放 PE、
/// 不放目标分区）——`state` 只用于决定日志行要不要同步进 GUI 状态栏。
unsafe fn run_boot_command_logged(state: Option<&State>, label: &str, command: &str) -> CmdOutcome {
    let logs = crate::log_root::logs_dir();
    // 这两个 helper 可能在没有 `State` 的入口里跑（桌面快捷方式 / PE 桌面），
    // 建目录失败就由随后的写日志失败如实暴露，不在这里假装记一笔。
    let _ = std::fs::create_dir_all(logs);
    let out = logs.join("boot-sequence.log");
    let code = run_cmd_to_file(command, Some(&out));
    let message = format!(
        "{label}: {command} -> exit code={code}（bcdedit 完整输出：{}）",
        out.display()
    );
    match state {
        Some(state) => append_gui_log(state, &message),
        None => {
            // PE/RE 里这一行会由 `session_log` 自动镜像到持久化卷。
            let _ = super::append_log(&logs.join("gui.log"), &message);
        }
    }
    code
}

/// PE 桌面里的重启：先 `wpeutil`（PE 原生），失败再 `shutdown.exe`，
/// 两条都失败**必须**告诉用户。
///
/// 旧代码是 `let _ = run_cmd_to_file("wpeutil.exe reboot", None);`：
/// 没日志、没兜底、失败后 PE 桌面毫无反应，用户面对一台卡住的机器不知道
/// 发生了什么（2026-10-17 第二轮核查，日志缺口 4）。
unsafe fn pe_reboot_with_fallback() {
    let logs = crate::log_root::logs_dir();
    // 这两个 helper 可能在没有 `State` 的入口里跑（桌面快捷方式 / PE 桌面），
    // 建目录失败就由随后的写日志失败如实暴露，不在这里假装记一笔。
    let _ = std::fs::create_dir_all(logs);
    let out = logs.join("pe-reboot.log");
    let first = run_cmd_to_file("wpeutil.exe reboot", Some(&out));
    let _ = super::append_log(
        &logs.join("gui.log"),
        &format!("pe_reboot: wpeutil.exe reboot -> exit code={first}"),
    );
    if first.is_success() {
        return;
    }
    let second = run_cmd_to_file("shutdown.exe /r /t 0 /f", None);
    let _ = super::append_log(
        &logs.join("gui.log"),
        &format!("pe_reboot: shutdown.exe fallback -> exit code={second}"),
    );
    if second.is_success() {
        return;
    }
    message_box_standalone(
        "自动重启失败：wpeutil 与 shutdown.exe 都失败了。\n\
         请在命令提示符里手动执行 wpeutil reboot，或长按电源键重启。\n\n\
         Auto-reboot failed: both wpeutil and shutdown.exe failed. \
         Run 'wpeutil reboot' manually or power-cycle the machine.",
        "重启失败 / Reboot failed",
        MB_OK | MB_ICONERROR,
    );
}

/// 记下「原本被 `let _ =` 吞掉」的文件系统操作。
///
/// 为什么要有它（2026-10-17 第二轮核查，日志缺口 5）：PE 安装期间多处
/// `fs::copy` / `create_dir_all` 的失败被直接丢弃，故障要延后到后面的步骤才炸，
/// 而日志里看不出「到底是哪一步没拷成」。
unsafe fn log_fs<T>(state: &State, what: &str, result: std::io::Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            append_gui_log(state, &crate::i18n::msg_tpl("op_failed", &[what, &error.to_string()]));
            None
        }
    }
}

/// 设置一次性 PE 启动项并重启，进入恢复桌面等待用户选择操作。
unsafe fn pe_reboot_to_pe(state: &State) {
    let language = selected_language(state);
    let guid_file = state.executable_dir.join("pe-entry-guid.txt");
    let guid = std::fs::read_to_string(&guid_file)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let Some(guid) = guid else {
        show_message(
            state.root,
            if language == Language::English {
                "No PE entry installed yet. Open the Install PE tab, choose a target volume and a PE WIM, then click Create Task first."
            } else {
                "尚未安装 PE。请先在「安装 PE」页选择目标卷和 PE 镜像，点击「创建任务」完成安装。"
            },
            if language == Language::English {
                "PE entry missing"
            } else {
                "未安装 PE"
            },
            MB_OK | MB_ICONINFORMATION,
        );
        return;
    };
    // 单纯进入恢复桌面，无需写入可执行文本任务。
    let command = format!("bcdedit.exe /set {{bootmgr}} bootsequence {{{guid}}}");
    let code = run_boot_command_logged(Some(state), "PE reboot to PE", &command);
    if code.is_success() {
        // 配置成功：立即自动重启进入 PE（无需用户手动重启）
        append_gui_log(state, "PE reboot to PE: bootsequence set, auto reboot now");
        if ExitWindowsEx(EWX_REBOOT, 0) == 0 {
            // ExitWindowsEx 失败（缺关机权限等）时回退 shutdown.exe /r /t 0
            let fallback = run_cmd_to_file("shutdown.exe /r /t 0 /f", None);
            append_gui_log(
                state,
                &format!(
                    "PE reboot to PE: ExitWindowsEx failed, shutdown.exe fallback code={fallback}"
                ),
            );
            if !fallback.is_success() {
                // 两条路都失败：提示用户手动重启
                show_message(
                    state.root,
                    &if language == Language::English {
                        "PE boot sequence set, but auto-reboot failed (ExitWindowsEx and shutdown.exe both failed). Please restart manually.".to_string()
                    } else {
                        "已设置 PE 一次性启动项，但自动重启失败（ExitWindowsEx 与 shutdown.exe 均失败），请手动重启进入 PE。".to_string()
                    },
                    if language == Language::English {
                        "Auto-reboot failed"
                    } else {
                        "自动重启失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
            }
        }
    } else {
        show_message(
            state.root,
            &if language == Language::English {
                format!(
                    "Failed to set PE boot sequence (bcdedit code {code}). Run as administrator."
                )
            } else {
                format!("配置失败（bcdedit 退出码 {code}）。请确认以管理员身份运行。")
            },
            if language == Language::English {
                "Failed"
            } else {
                "设置失败"
            },
            MB_OK | MB_ICONERROR,
        );
    }
}

/// 桌面快捷方式入口（`--pe-reboot`）：无主窗口，设置 bootsequence 指向
/// 已安装的 PE，用无父窗口消息框提示结果。非提升进程先提权重启。
pub unsafe fn pe_reboot_standalone() -> Result<(), super::TaskError> {
    if !is_elevated() {
        relaunch_elevated()?;
        return Ok(());
    }
    let executable_dir = std::env::current_exe()
        .map_err(|error| super::err(&format!("cannot resolve current executable: {error}")))?
        .parent()
        .map(|parent| parent.to_path_buf())
        .unwrap_or_default();
    let guid_file = executable_dir.join("pe-entry-guid.txt");
    let guid = std::fs::read_to_string(&guid_file)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let Some(guid) = guid else {
        let text = wide(
            "尚未安装 PE。请先打开 BackupRestore，在「安装 PE」页安装后再使用本快捷方式。\n\nPE is not installed yet. Install it from the Install PE tab first.",
        );
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            wide("未安装 PE").as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
        return Ok(());
    };
    let command = format!("bcdedit.exe /set {{bootmgr}} bootsequence {{{guid}}}");
    // 这个入口**没有主窗口**（桌面快捷方式）：此前连一行日志都没有，
    // 进程退出后完全看不出这台机器被设置过一次性启动项。
    let code = run_boot_command_logged(None, "pe-reboot standalone", &command);
    if code.is_success() {
        let text = wide(
            "已设置一次性启动项。重启后自动进入 PE 恢复桌面；PE 启动时会自动清除该启动项，之后重启正常回到 Windows。\n\nBoot sequence set. Reboot to enter the PE recovery desktop automatically.",
        );
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            wide("设置成功").as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    } else {
        let text = wide(&format!(
            "设置 bootsequence 失败（bcdedit 退出码 {code}）。请确认以管理员身份运行。\n\nFailed to set boot sequence (exit code {code})."
        ));
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            wide("设置失败").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
    Ok(())
}

/// 读取最近一次 launcher-errors.log 里的错误信息（用于 prepare 异常退出感知）。
/// 与写方（`main::record_launch_error` / `windows_prepare` 的 UAC 前置）
/// 同源：都走 `log_root::logs_dir()`。写的是 CLI 进程、读的是 GUI 进程，
/// 两边各解析一次——策略确定才拿得到同一份文件，否则 GUI 读到空气。
fn read_latest_launcher_error() -> Option<String> {
    let log_path = crate::log_root::logs_dir().join("launcher-errors.log");
    let content = std::fs::read_to_string(log_path).ok()?;
    content
        .lines()
        .rev()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(|s| s.to_string())
}

/// 返回 `Some(IDYES)` 表示用户明确要求清理未完成任务；返回 `Some(IDNO)` 或
/// `None` 表示不能/不应继续。只读扫描失败也阻断，不猜测当前是否安全。
unsafe fn incomplete_task_choice(
    state: &State,
    language: Language,
) -> Option<(i32, backuprestore_core::Task)> {
    let store = backuprestore_core::TaskStore::new(&state.executable_dir);
    let tasks = match store.incomplete_tasks() {
        Ok(tasks) => tasks,
        Err(error) => {
            append_gui_log(state, &format!("incomplete task scan failed: {error}"));
            show_message(
                state.root,
                &format!("无法检查已有未完成任务：{error}"),
                if language == Language::English {
                    "Cannot inspect tasks"
                } else {
                    "无法检查任务"
                },
                MB_OK | MB_ICONERROR,
            );
            return None;
        }
    };
    if tasks.is_empty() {
        return None;
    }
    let details = tasks
        .iter()
        .map(|task| {
            let target = task
                .target
                .as_ref()
                .map(|spec| spec.volume.partition_guid.as_str())
                .unwrap_or("(无目标卷)");
            format!(
                "{} / {:?} / {:?} / target={target}",
                task.task_id, task.operation, task.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    append_gui_log(
        state,
        &format!(
            "incomplete task scan: count={} details={details}",
            tasks.len()
        ),
    );
    let text = if language == Language::English {
        format!(
            "An unfinished recovery task already exists:\n\n{details}\n\nYes = resume the oldest task, No = abandon and clean it (only before target writes), Cancel = do not create a new task."
        )
    } else {
        format!(
            "检测到尚未终结的任务：\n\n{details}\n\n选择“是”继续最早的旧任务；选择“否”放弃并清理（仅限未写入目标卷的任务）；选择“取消”不创建新任务。"
        )
    };
    let title = if language == Language::English {
        "Unfinished task exists"
    } else {
        "存在未完成任务"
    };
    let task = tasks.into_iter().next().expect("tasks 非空");
    Some((
        show_message(
            state.root,
            &format!("{text}\n\n是=继续最早的旧任务；否=放弃并清理；取消=不创建新任务。"),
            title,
            MB_YESNOCANCEL | MB_ICONWARNING,
        ),
        task,
    ))
}

/// 清理用户明确放弃的未完成任务：只删除任务目录里的恢复工作树，并将任务标记为
/// Failed 保留诊断记录。若任务已经进入破坏性阶段，拒绝 GUI 直接清理——这种任务
/// 必须先走恢复/续跑，避免把“目标已擦除但还没应用镜像”的材料删掉。
unsafe fn cleanup_incomplete_tasks_from_gui(
    state: &State,
    language: Language,
) -> Result<(), String> {
    let store = backuprestore_core::TaskStore::new(&state.executable_dir);
    let tasks = store
        .incomplete_tasks()
        .map_err(|error| error.to_string())?;
    // 先完整预检，避免多个任务中后面的危险任务导致前面的任务已经被部分清理。
    if let Some(task) = tasks
        .iter()
        .find(|task| !task.status.can_abandon_before_target_write())
    {
        return Err(format!(
            "任务 {} 已进入 {:?} 阶段，可能已修改目标卷；不能从新任务对话框直接清理。请先从恢复入口续跑或人工处理。",
            task.task_id, task.status
        ));
    }
    for task in tasks {
        let dir = store
            .task_dir(&task.task_id)
            .map_err(|error| error.to_string())?;
        let log = dir.join("recovery.log");
        append_log(
            &log,
            &format!(
                "User explicitly abandoned task from new-task dialog; operation={:?} stage={:?}",
                task.operation, task.status
            ),
        )
        .map_err(|error| error.to_string())?;
        let mut failed = task.clone();
        store
            .write_failure(&mut failed, 2, "用户在创建新任务前明确放弃旧任务")
            .map_err(|error| error.to_string())?;
        store
            .cleanup_task_artifacts(&failed.task_id)
            .map_err(|error| error.to_string())?;
        append_gui_log(
            state,
            &format!(
                "incomplete task abandoned and artifacts cleaned: {}",
                failed.task_id
            ),
        );
    }
    let _ = language;
    Ok(())
}

/// 启动一个需要管理员权限的旧任务续跑；`resume` 会验证载荷、重武装一次性
/// 启动项，然后把同一任务送回 WinRE。这里只允许用户明确选择了某个 task ID。
unsafe fn launch_explicit_resume(
    state: &State,
    task: &backuprestore_core::Task,
    language: Language,
) -> bool {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            append_gui_log(state, &format!("resume launch failed: {error}"));
            return false;
        }
    };
    let args = [
        "resume".to_string(),
        state.executable_dir.to_string_lossy().into_owned(),
        task.task_id.clone(),
    ];
    let params = args
        .iter()
        .map(|argument| crate::text_parsing::quote_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let executable_wide = wide(&executable.to_string_lossy());
    let verb = wide("runas");
    let params_wide = wide(&params);
    let mut info = ShellExecuteInfoW {
        cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
        f_mask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: state.root,
        lp_verb: verb.as_ptr(),
        lp_file: executable_wide.as_ptr(),
        lp_parameters: params_wide.as_ptr(),
        lp_directory: null(),
        n_show: SW_SHOWNORMAL,
        h_inst_app: null_mut(),
        lp_id_list: null_mut(),
        lp_class: null(),
        hkey_class: null_mut(),
        dw_hot_key: 0,
        h_icon: null_mut(),
        h_process: null_mut(),
    };
    if ShellExecuteExW(&mut info) == 0 {
        append_gui_log(
            state,
            &format!("resume launch failed: Windows error {}", GetLastError()),
        );
        return false;
    }
    if !info.h_process.is_null() {
        let wait = WaitForSingleObject(info.h_process, 1500);
        if wait == 0 {
            let mut code = 0_u32;
            GetExitCodeProcess(info.h_process, &mut code);
            CloseHandle(info.h_process);
            if code != 0 {
                append_gui_log(state, &format!("resume launch exited early: code={code}"));
                return false;
            }
        } else {
            CloseHandle(info.h_process);
        }
    }
    append_gui_log(
        state,
        &format!("explicit resume launched: task={}", task.task_id),
    );
    let text = if language == Language::English {
        "The unfinished task was sent to Windows RE for explicit resumption."
    } else {
        "已将旧任务送入 Windows RE，继续执行原任务。"
    };
    show_message(
        state.root,
        text,
        if language == Language::English {
            "Resume task"
        } else {
            "继续旧任务"
        },
        MB_OK | MB_ICONINFORMATION,
    );
    true
}

unsafe fn create_task(state: &mut State) {
    if ONLINE_BUSY.is_busy() {
        show_message(
            state.root,
            "已有在线任务正在执行，请等待完成。",
            "任务运行中",
            MB_OK | MB_ICONWARNING,
        );
        return;
    }
    let language = selected_language(state);
    let operation = selected_operation(state).to_string();
    append_gui_log(
        state,
        &format!("GUI action started: create task; operation={operation}"),
    );
    // The PE recovery install is a direct in-process operation (copy WIM +
    // BCD entry), not a WinRE recovery task, so handle it before any of the
    // restore-specific validation below.
    if operation == "install-pe-entry" {
        install_pe_entry(state);
        return;
    }
    // 在任何破坏性确认、镜像校验、提权或 prepare 之前检查未终结任务。
    // 载荷目录按任务 ID 隔离只能防文件覆盖；同一工作区的 EFI bootsequence
    // 仍是单资源。发现旧任务时让用户选择：继续原任务 / 清理并放弃旧任务 / 取消。
    // “继续”只继续最早的任务；“清理”必须经过用户明确确认，不能静默删恢复材料。
    if let Some((choice, task)) = incomplete_task_choice(state, language) {
        if choice == IDYES {
            if launch_explicit_resume(state, &task, language) {
                return;
            }
            return;
        }
        if choice != IDNO {
            append_gui_log(state, "GUI action cancelled: incomplete task remains");
            return;
        }
        if let Err(error) = cleanup_incomplete_tasks_from_gui(state, language) {
            append_gui_log(
                state,
                &format!("GUI action blocked: incomplete task cleanup failed: {error}"),
            );
            show_message(
                state.root,
                &error,
                if language == Language::English {
                    "Cleanup failed"
                } else {
                    "清理失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    }
    let btn_task = GetDlgItem(state.root, ID_CREATE_TASK as i32);
    let current_btn_text = get_text(btn_task);
    if current_btn_text == "选择镜像位置" || current_btn_text == "Choose image location" {
        browse_image(state);
        let updated = get_text(state.controls.image).trim().to_string();
        if backuprestore_core::validate_absolute_path(&updated).is_ok() {
            set_text(
                btn_task,
                if language == Language::English {
                    "Create task"
                } else {
                    "创建任务"
                },
            );
            if is_image_restore_entry(&operation) && Path::new(&updated).is_file() {
                read_image(state);
            }
        }
        return;
    }
    let image_path = get_text(state.controls.image).trim().to_string();
    if operation != "probe"
        && let Err(error) = backuprestore_core::validate_absolute_path(&image_path)
    {
        let kind = if image_path.is_empty() { "empty" } else { "invalid" };
        append_gui_log(
            state,
            &format!(
                "GUI image path check: kind={kind} detail={error}; guidance dialog opened"
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
        // 空路径与非法路径分别用不同文案引导，均提供【选择镜像位置】/【取消】
        // （用户 2026-10-18 要求：不要弹错误图标报错，改成提示 + 浏览 + 取消）。
        let choice = ask_invalid_image_path(state.root, language, image_path.is_empty());
        append_gui_log(
            state,
            &format!("GUI image path guidance: user choice={choice} (1=browse 0=cancel)"),
        );
        if choice == 1 {
            // 用户点击【选择镜像位置】：立即弹出文件浏览窗口
            browse_image(state);
            let updated = get_text(state.controls.image).trim().to_string();
            let valid = backuprestore_core::validate_absolute_path(&updated).is_ok();
            append_gui_log(
                state,
                &format!("GUI browse returned: updated={updated} valid={valid}"),
            );
            if valid {
                set_text(
                    btn_task,
                    if language == Language::English {
                        "Create task"
                    } else {
                        "创建任务"
                    },
                );
                if is_image_restore_entry(&operation) && Path::new(&updated).is_file() {
                    read_image(state);
                }
            }
            SetFocus(state.controls.image);
        } else {
            // 用户点击【取消】：将主界面【创建任务】按钮文本修改为【选择镜像位置】
            set_text(
                btn_task,
                if language == Language::English {
                    "Choose image location"
                } else {
                    "选择镜像位置"
                },
            );
            SetFocus(state.controls.image);
        }
        return;
    }
    // 路径合法时确保按钮文本为【创建任务】
    set_text(
        btn_task,
        if language == Language::English {
            "Create task"
        } else {
            "创建任务"
        },
    );
    let index = selected_wim_index(state);
    let index = if is_image_restore_entry(&operation) && index.is_none() {
        if Path::new(&image_path).is_file() {
            read_image(state);
        }
        let recheck = selected_wim_index(state);
        match recheck {
            Some(value) => value.to_string(),
            None => {
                append_gui_log(
                    state,
                    "GUI action blocked: restore requested without a selected WIM index",
                );
                show_message(
                    state.root,
                    if language == Language::English {
                        "Read the WIM first, then select a valid image index from the dropdown."
                    } else {
                        "请先读取 WIM，再从下拉框选择有效的镜像索引。"
                    },
                    if language == Language::English {
                        "Validation failed"
                    } else {
                        "参数校验失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        }
    } else {
        index.unwrap_or(1).to_string()
    };
    let selected_or_error = |control: Hwnd, label: &str| {
        selected_drive_letter(state, control)
            .ok_or_else(|| format!("{label}必须从下拉框选择一个可用卷。"))
    };
    let workspace_drive = state
        .executable_dir
        .to_string_lossy()
        .chars()
        .next()
        .map(|value| value.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    // 三入口拆分：先取目标卷（A 锁定系统盘 / B 数据卷 / C 第二系统位置），
    // 源卷按入口归一——A/B 的 prepare 链要求 source==target（restore-existing），
    // C 的语义是「保留当前系统、装另一个分区」（create-secondary 的 source）。
    // 源卷下拉只在备份/新增第二系统显示，A/B/C 不再读它的选择。
    // 探测页不显示目标卷行（`set_operation_visibility` 隐藏了它），下拉里根本
    // 不存在「用户的选择」。探测不写入任何卷，这个值只是交给 prepare 的参数，
    // 取不到就退回当前系统盘——不能让一个用不到的字段挡住环境自检。
    let target_drive = if operation == "probe" {
        selected_drive_letter(state, state.controls.target).unwrap_or_else(active_system_letter)
    } else {
        match selected_or_error(
            state.controls.target,
            if language == Language::English {
                "Restore target"
            } else {
                "目标卷"
            },
        ) {
            Ok(value) => value,
            Err(error) => {
                append_gui_log(state, &format!("GUI action blocked: {error}"));
                show_message(
                    state.root,
                    &error,
                    if language == Language::English {
                        "Validation failed"
                    } else {
                        "参数校验失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        }
    };
    // 【2026-10-18 缺陷修复 H1 第二层：A 入口目标硬闸】
    // 光修好「自动选中」不够——H1 的成因正是 UI 自动选中**静默失效**而下游没有
    // 任何一道断言兜住：A 页文案写死「目标卷（当前系统盘 X:）」、下拉被禁用，
    // prepare 侧又只要求 `source == target` 就放行。于是只要选中项不是系统盘，
    // 整条链就会格式化那个盘并灌入镜像。这里按入口性质断言到底：
    // restore-existing 的目标必须就是当前运行的系统盘，否则直接拦下。
    if operation == "restore-existing"
        && !target_drive.eq_ignore_ascii_case(&active_system_letter())
    {
        let message = if language == Language::English {
            format!(
                "The 'Restore C:' entry must target the currently running system drive ({}:), \
                 but {} is selected. Refusing to continue — nothing was changed. \
                 (Pick the 'Restore C:' entry only when you intend to overwrite the running system.)",
                active_system_letter(),
                target_drive
            )
        } else {
            format!(
                "「还原系统到 C 盘」入口的目标必须是当前正在运行的系统盘（{}:），\n\
                 而当前选中的是 {}。已拒绝执行，未做任何更改。",
                active_system_letter(),
                target_drive
            )
        };
        append_gui_log(
            state,
            &format!(
                "GUI action blocked: restore-existing target {} != active system drive {}",
                target_drive,
                active_system_letter()
            ),
        );
        show_message(
            state.root,
            &message,
            if language == Language::English {
                "Restore blocked"
            } else {
                "还原已停止"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let source_drive = match operation.as_str() {
        "restore-existing" | "restore-data" => target_drive.clone(),
        "create-secondary" => active_system_letter(),
        // 探测页同样不显示源卷行，与上面目标卷同理退回当前系统盘。
        "probe" => {
            selected_drive_letter(state, state.controls.source).unwrap_or_else(active_system_letter)
        }
        _ => match selected_or_error(
            state.controls.source,
            if language == Language::English {
                "Source volume"
            } else {
                "源卷"
            },
        ) {
            Ok(value) => value,
            Err(error) => {
                append_gui_log(state, &format!("GUI action blocked: {error}"));
                show_message(
                    state.root,
                    &error,
                    if language == Language::English {
                        "Validation failed"
                    } else {
                        "参数校验失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        },
    };
    // Keep this as the first restore-specific guard: no image parsing,
    // confirmation, elevation or preparation is reached when the program
    // directory would be overwritten.
    let workspace_identity = state
        .drives
        .iter()
        .find(|drive| drive.letter.eq_ignore_ascii_case(&workspace_drive));
    let target_identity = state
        .drives
        .iter()
        .find(|drive| drive.letter.eq_ignore_ascii_case(&target_drive));
    if is_image_restore_entry(&operation)
        && same_partition_by_gui_identity(workspace_identity, target_identity)
    {
        append_gui_log(
            state,
            "GUI action blocked: program workspace volume matches restore target",
        );
        show_message(
            state.root,
            if language == Language::English {
                "Cannot start restore: the program directory is on the same volume as the restore target. Move the entire BackupRestore folder to another volume and run it again. No task, WinRE, boot configuration or reboot was requested."
            } else {
                "无法开始还原：程序目录位于目标分区所在卷。请手动将整个 BackupRestore 文件夹移动到其他分区后重新运行。未创建任务，未修改 WinRE，未修改启动配置，未请求重启。"
            },
            if language == Language::English {
                "Restore blocked"
            } else {
                "还原已停止"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // ===== 三入口拆分后的 GUI 侧目标闸门（prepare 的 P0 安全闸仍是最终防线）：
    // B/C 的目标不能是当前系统盘——B 是数据还原、C 是第二系统，覆盖运行中的
    // 系统盘分别属于 A 入口和明确的误操作。
    if matches!(operation.as_str(), "restore-data" | "create-secondary")
        && target_drive.eq_ignore_ascii_case(&active_system_letter())
    {
        let message = if language == Language::English {
            format!(
                "The selected target is the currently running system drive ({}:). Restoring data or a second system onto it is not allowed; use the 'Restore C:' entry to restore the running system. Nothing was changed.",
                active_system_letter()
            )
        } else {
            format!(
                "所选目标是当前正在运行的系统盘（{}:）。还原数据到非 C 盘与还原系统到非 C 盘不允许指向它；要还原当前系统请使用「还原系统到 C 盘」入口。未做任何更改。",
                active_system_letter()
            )
        };
        append_gui_log(
            state,
            "GUI action blocked: restore-data/create-secondary target is the active system drive",
        );
        show_message(
            state.root,
            &message,
            if language == Language::English {
                "Restore blocked"
            } else {
                "还原已停止"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // B（还原数据到非 C 盘）的目标不能是已有 Windows 的分区：数据通道不修复引导，
    // 把数据镜像盖在 Windows 分区上只会得到一个坏系统。指引改用 C/新增第二系统。
    if operation == "restore-data"
        && let Some(target_info) = state
            .drives
            .iter()
            .find(|drive| drive.letter.eq_ignore_ascii_case(&target_drive))
        && target_info.has_windows_installation
    {
        append_gui_log(
            state,
            "GUI action blocked: restore-data target already contains a Windows installation",
        );
        show_message(
            state.root,
            if language == Language::English {
                "The selected target already contains a Windows installation. The data-restore channel does not repair boot configuration; use 'Restore other' or 'Second system' to restore a system. Nothing was changed."
            } else {
                "所选目标分区上已有 Windows 安装。还原数据到非 C 盘的通道不处理引导；要还原系统请改用「还原系统到非 C 盘」或「新增第二系统」。未做任何更改。"
            },
            if language == Language::English {
                "Restore blocked"
            } else {
                "还原已停止"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 【准备期硬拒，GUI 早期版】「系统卷 + 取消格式化（覆盖式应用）」是确定性失败组合。
    // A 入口（还原系统到 C 盘）目标必然是带 Windows 的系统卷；此时取消勾选「格式化
    // 目标分区」= 让 DISM 去改写 C: 上 `Program Files\WindowsApps\...`（MSIX 包目录，
    // owner=TrustedInstaller）的目录元数据，必然被拒（RestoreDirMetadata
    // 0x80070005）→ RE 内 exit 5。
    // 权威闸门在 prepare 的 `validate_operation_inputs`（离线）与
    // `restore_to_windows_volume`（在线），这里只做「不让用户白跑一趟提权+重启」的前置拦截。
    // 详见 docs/20261016-1413 §2、docs/20261016-1836。
    //
    // 【2026-10-16 接手复核修正（R3 文案分叉）】此前本处**自己复制了一份等价条件与
    // 中英双语硬编码文案**，与 `pe_safety` 的判据并存 —— 判据能对、文案会漂
    // （项目 P3 前科）。彼时改为只调用同一个纯函数（判据 + 旧硬拒文案单一来源）。
    // 【2026-10-22 #110】硬拒改确认框后，旧硬拒文案退役（发布件里从未输出），
    // `pe_safety::overlay_restore_on_windows_volume` 收敛为纯谓词；本处确认框
    // 文案语义是「告知 wimlib 通道代价并请示」，与旧硬拒文案的「指导用户改参数」
    // 不同，有意只此一处（无分叉对象）。
    if is_image_restore_entry(&operation) {
        let format_target_checked = IsDlgButtonChecked(state.root, ID_FORMAT_TARGET as i32) != 0;
        let target_has_windows = state
            .drives
            .iter()
            .find(|drive| drive.letter.eq_ignore_ascii_case(&target_drive))
            .is_some_and(|drive| drive.has_windows_installation);
        if crate::pe_safety::overlay_restore_on_windows_volume(
            target_has_windows,
            format_target_checked,
        ) {
            // 【2026-10-22 任务 #110：硬拒改确认】命中「已含 Windows + 取消
            // 格式化」不再直接拦截：DISM 该组合必然 RestoreDirMetadata
            // 0x80070005（docs/20261016-1413 §2），用户裁定「换 wimlib」
            // 后改走 wimlib 通道（自行获取 SeRestore/SeBackup 提权，POC 见
            // docs/20261022-1427）。【2026-10-23 产品化】概念验证
            // （docs/20261022-2308）证明该提权同时覆盖安全描述符还原 →
            // 镜像目录权限（ACL/owner）完整还原，个别受保护目录可能保留卷上
            // 现有权限。确认框明示该语义；确认后放行进入正常任务流——
            // prepare/在线链内部按同一判据路由，已无硬拒点；取消则维持原
            // 拦截行为（什么都不做）。
            append_gui_log(
                state,
                "GUI overlay restore onto a volume containing Windows: asking the user to confirm the wimlib route",
            );
            let notice = if language == Language::English {
                "The target volume already contains Windows. Overlay apply without formatting will use the wimlib channel: directory permissions (ACL/owner) from the image are restored in full; a few protected directories may keep their existing permissions.\n\nContinue?"
            } else {
                "目标分区已含 Windows。取消格式化的覆盖式应用将走 wimlib 通道：镜像中的目录权限（属主与访问控制列表）将完整还原，个别受保护目录可能保留卷上现有权限。\n\n是否继续？"
            };
            let answer = show_message(
                state.root,
                notice,
                if language == Language::English {
                    "Confirm overlay restore (wimlib channel)"
                } else {
                    "确认覆盖式还原（wimlib 通道）"
                },
                MB_YESNO | MB_ICONWARNING,
            );
            if answer != IDYES {
                append_gui_log(
                    state,
                    "GUI action blocked: overlay restore (wimlib route) declined by the user",
                );
                return;
            }
            append_gui_log(
                state,
                "GUI overlay restore: the user confirmed the wimlib route",
            );
        }
    }
    // C（还原系统到非 C 盘）走 create-secondary 语义，离线 prepare 要求启动菜单名必填；
    // 在线通道同样用它给新启动项命名。空名在这里拦下，避免提权后才发现。
    if operation == "create-secondary" && get_text(state.controls.menu).trim().is_empty() {
        append_gui_log(state, "GUI action blocked: create-secondary requires a boot menu name");
        show_message(
            state.root,
            if language == Language::English {
                "Enter the boot menu name for the new system (shown in the Windows boot menu) before creating the task."
            } else {
                "请先填写启动菜单名称（新系统在 Windows 启动菜单中的显示名）再创建任务。"
            },
            if language == Language::English {
                "Validation failed"
            } else {
                "参数校验失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let backup_will_append = operation == "backup" && PathBuf::from(&image_path).is_file();
    let image_drive = image_path
        .chars()
        .next()
        .map(|value| value.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    if operation != "probe" && image_drive == source_drive {
        append_gui_log(
            state,
            "GUI action blocked: image volume matches source volume",
        );
        show_message(
            state.root,
            if language == Language::English {
                "Image volume must differ from the Windows source volume."
            } else {
                "镜像卷不能与 Windows 源卷相同。"
            },
            if language == Language::English {
                "Validation failed"
            } else {
                "参数校验失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // S5：新启动项撞名消解。与现有任一启动菜单项重名时追加 -YYYYMMDDHHMM，
    // 回写文本框并告知——用户可继续手改后再创建。数据源是未提权的
    // bcdedit /enum（list-boot-entries 子命令）；读不到就跳过，提权后的
    // prepare 侧还有一道同样的消解兜底（docs/20261015-0725）。
    if matches!(
        operation.as_str(),
        "create-secondary"
    ) {
        let typed = get_text(state.controls.menu).trim().to_string();
        let existing = existing_boot_entry_names(state);
        let resolved =
            crate::boot_menu::resolve_menu_name_collision(&typed, &existing, chrono::Local::now());
        if resolved != typed {
            append_gui_log(
                state,
                &format!("boot menu name collision resolved: {typed} -> {resolved}"),
            );
            set_menu_name(state, &resolved);
            // 消解结果已是既定名：标上手改，避免随后切索引又被 WIM 名覆盖。
            state.menu_name_touched = true;
            show_message(
                state.root,
                &if language == Language::English {
                    format!(
                        "The boot menu name \"{typed}\" is already in use by an existing boot entry. It has been changed to \"{resolved}\". You can edit it further before creating the task."
                    )
                } else {
                    format!(
                        "启动菜单名称「{typed}」与现有启动项重名，已自动改为「{resolved}」。可继续手动修改后再创建任务。"
                    )
                },
                if language == Language::English {
                    "Boot menu name adjusted"
                } else {
                    "启动菜单名称已调整"
                },
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }
    let operation_label = operation_display(language, &operation);
    // C/第二系统入口把最终菜单名写进破坏性确认摘要：消解后的名字必须让用户
    // 在点「是」之前看见，而不是建完任务才发现启动项叫了别的名字。
    let menu_summary = if matches!(
        operation.as_str(),
        "create-secondary"
    ) {
        let menu = get_text(state.controls.menu).trim().to_string();
        if language == Language::English {
            format!("\nBoot menu name: {menu}")
        } else {
            format!("\n启动菜单名：{menu}")
        }
    } else {
        String::new()
    };
    let (target_line, warning) =
        crate::failure_policy::destructive_confirm_parts(
            &operation,
            message_language(language),
            &target_drive,
        );
    let summary = if language == Language::English {
        format!(
            "Mode: {operation_label}\nSource volume: {source_drive}\nImage: {image_path}{target_line}\nWIM index: {index}{menu_summary}",
        )
    } else {
        format!(
            "模式：{operation_label}\n源卷：{source_drive}\n镜像绝对路径：{image_path}{target_line}\nWIM 索引：{index}{menu_summary}",
        )
    };
    // 测试钩子（--test-hook + auto_install）下自动接受确认框，与 PE 安装确认同策略：
    // 自动化验收环境无法精确点击弹窗按钮，卡在这里等于流程永远走不下去。
    // 探测入口不弹这道确认：它承诺「不写盘、不重启」，弹「破坏性确认」与自身
    // 语义直接冲突，还会让首次使用者以为自检有风险——而该入口的存在意义恰恰是
    // 让人放心地先自检一遍（需求 §7/§8）。备份与三个还原入口的行为一字不动。
    let destructive_answer = if operation == "probe" {
        append_gui_log(
            state,
            "probe: destructive confirmation skipped (no disk write, no reboot)",
        );
        IDYES
    } else if TEST_AUTO_CONFIRM.load(std::sync::atomic::Ordering::SeqCst) {
        append_gui_log(state, "test hook: destructive confirmation auto-accepted");
        IDYES
    } else {
        show_message(
            state.root,
            &format!("{summary}\n\n{warning}"),
            if language == Language::English {
                "Destructive confirmation"
            } else {
                "破坏性确认"
            },
            MB_YESNO | MB_ICONWARNING,
        )
    };
    if is_image_restore_entry(&operation) && destructive_answer != IDYES {
        append_gui_log(
            state,
            &format!("GUI action cancelled: destructive confirmation; operation={operation}"),
        );
        set_text(
            state.controls.status,
            if language == Language::English {
                "The destructive task was cancelled."
            } else {
                "用户取消了破坏性任务。"
            },
        );
        return;
    }
    // ===== 还原前档案（.index-N.metadata.json）检查：缺失或哈希不匹配时
    // 弹窗提示，用户确认后仍可还原（传 --force-restore-hash 跳过校验）=====
    // 档案丢失不阻止还原：用户可能移动/删除了 wim 旁的档案文件。
    let mut force_restore_hash = false;
    let mut verify_hash_enabled = false;
    if is_image_restore_entry(&operation) && PathBuf::from(&image_path).is_file() {
        let verify_hash = unsafe {
            SendMessageW(state.controls.verify_hash, BM_GETCHECK, 0, 0) == BST_CHECKED as isize
        };
        verify_hash_enabled = verify_hash;
        let index_number = index.parse::<u32>().unwrap_or(1);
        let metadata_check = crate::read_index_metadata(Path::new(&image_path), index_number);
        let hash_ok = match &metadata_check {
            Ok(metadata) => {
                // 优化：优先比对文件物理大小；如果用户勾选了“严格对比哈希”，才流式计算大文件 SHA-256
                // 彻底消除默认情况下在主 UI 消息循环线程同步流式读取 58GB 导致窗口未响应（卡死）1~2 分钟的问题。
                if let Ok(file_meta) = std::fs::metadata(&image_path) {
                    if metadata.image_size > 0 && file_meta.len() != metadata.image_size {
                        false
                    } else if verify_hash {
                        backuprestore_core::sha256_file(&image_path)
                            .map(|actual| actual.eq_ignore_ascii_case(&metadata.image_sha256))
                            .unwrap_or(false)
                    } else {
                        true
                    }
                } else {
                    false
                }
            }
            Err(_) => false,
        };
        let (prompt, title) = if metadata_check.is_err() {
            (
                (
                    if language == Language::English {
                        "No backup metadata file was found next to this image (it may have been moved or deleted).\n\nSkipping the integrity check and continuing may restore a wrong or damaged image.\n\nContinue anyway?"
                    } else {
                        "未在此镜像旁找到备份档案文件（可能被移动或删除）。\n\n跳过完整性校验直接还原，可能还原到错误或损坏的镜像。\n\n是否仍要继续还原？"
                    }
                )
                .to_string(),
                (
                    if language == Language::English {
                        "Backup metadata missing"
                    } else {
                        "备份档案缺失"
                    }
                )
                .to_string(),
            )
        } else if !hash_ok {
            (
                (
                    if language == Language::English {
                        "The backup metadata does not match this image file (the image may have been modified or damaged).\n\nContinue anyway?"
                    } else {
                        "备份档案与镜像不匹配（镜像可能被修改或损坏）。\n\n是否仍要还原？"
                    }
                )
                .to_string(),
                (
                    if language == Language::English {
                        "Image hash mismatch"
                    } else {
                        "镜像校验不匹配"
                    }
                )
                .to_string(),
            )
        } else {
            (String::new(), String::new())
        };
        // 测试钩子下同样自动接受（否则档案缺失弹窗会卡住自动验收）。
        let metadata_answer = if TEST_AUTO_CONFIRM.load(std::sync::atomic::Ordering::SeqCst) {
            append_gui_log(state, "test hook: metadata confirmation auto-accepted");
            IDYES
        } else if prompt.is_empty() {
            IDYES
        } else {
            show_message(state.root, &prompt, &title, MB_YESNO | MB_ICONWARNING)
        };
        if !prompt.is_empty() && metadata_answer != IDYES
        {
            append_gui_log(
                state,
                &format!("restore cancelled: metadata check declined; operation={operation}"),
            );
            return;
        }
        force_restore_hash = metadata_check.is_err() || !hash_ok;
        // ===== A（还原系统到 C 盘）容量预检加强：目标锁定为系统盘、无法换盘纠错，
        // 容量不足必须在重启前拦下（进 PE 后才发现等于白重启一次）。
        // required_target_size = max(备份时源分区大小, 已用字节+保留量)，
        // 与目标分区总大小比对（还原会先格式化，比较对象是总容量而非空闲）。
        if operation == "restore-existing"
            && let Ok(metadata) = &metadata_check
            && let Some(target_size) = state
                .drives
                .iter()
                .find(|drive| drive.letter.eq_ignore_ascii_case(&target_drive))
                .and_then(|drive| drive.size_bytes)
            && metadata.required_target_size() > target_size
        {
            let required_gb = metadata.required_target_size() / 1_073_741_824;
            let actual_gb = target_size / 1_073_741_824;
            append_gui_log(
                state,
                &format!(
                    "GUI action blocked: restore target too small; required={} bytes actual={target_size} bytes",
                    metadata.required_target_size()
                ),
            );
            show_message(
                state.root,
                &if language == Language::English {
                    format!(
                        "The system drive ({target_drive}:) is smaller than this image requires (needs about {required_gb} GiB, the partition is {actual_gb} GiB). The target is locked to the system drive and cannot be changed; use a smaller image or a larger disk. Nothing was changed."
                    )
                } else {
                    format!(
                        "系统盘（{target_drive}:）容量小于该镜像的要求（约需 {required_gb} GiB，当前分区 {actual_gb} GiB）。目标锁定为系统盘无法更换；请换用更小的镜像或更大的磁盘。未做任何更改。"
                    )
                },
                if language == Language::English {
                    "Restore blocked"
                } else {
                    "还原已停止"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    }
    // ===== 智能分流：备份/还原目标为「当前活动系统」→ 弹窗选 PE/RE/取消 =====
    // 判断用「当前活动系统」（%SystemDrive%），不是「任何含 Windows 的卷」：
    // 双系统时另一个 Windows 卷并未运行，可直接在线备份/还原。
    let compress = if operation == "backup" {
        // 两项下拉映射为 DISM 术语：压缩=fast、不压缩=none。
        match compression_from_ui_index(combo_selection(state.controls.compress)) {
            Ok(level) => level.to_string(),
            Err(error) => {
                append_gui_log(state, &format!("invalid compression selection: {error}"));
                return;
            }
        }
    } else {
        String::new()
    };
    // 备份索引名（默认程序启动时间，用户可改）与保留最近 N 个索引（0=不清理）。
    let image_name = if operation == "backup" {
        get_text(state.controls.index_name).trim().to_string()
    } else {
        String::new()
    };
    let keep_indexes = if operation == "backup" {
        get_text(state.controls.keep)
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|value| *value > 0)
    } else {
        None
    };
    if matches!(operation.as_str(), "backup" | "restore-existing") {
        let system_upper = std::env::var("SystemDrive")
            .unwrap_or_else(|_| "C:".to_string())
            .trim_end_matches('\\')
            .trim_end_matches(':')
            .to_ascii_uppercase();
        // 待处理分区：备份=被捕获的源卷（镜像保存卷只是存放位置，不影响是否离线）；
        // 还原=被覆盖的目标卷。
        let affected = if operation == "backup" {
            &source_drive
        } else {
            &target_drive
        };
        let affected_upper = affected.trim_end_matches(':').to_ascii_uppercase();
        // A（还原系统到 C 盘）目标锁定系统盘、必离线；备份仍按「是否当前活动系统」分流。
        let needs_offline = if operation == "backup" {
            affected_upper == system_upper
        } else {
            true
        };
        if needs_offline {
            // 当前活动系统：必须离线处理，弹窗让用户选 PE / RE / 取消。
            // 测试钩子配置了 system_drive_choice 时直接采用（跳过弹窗）。
            let choice = match state.test_drive_choice {
                Some(c) => c,
                None => ask_system_drive_handler(state.root, language),
            };
            append_gui_log(
                state,
                &format!("system drive choice returned: {choice} (0=cancel 1=PE 2=RE)"),
            );
            match choice {
                1 => {
                    append_gui_log(
                        state,
                        "system drive operation: user chose PE (schedule PE task)",
                    );
                    schedule_pe_task(
                        state,
                        &operation,
                        &source_drive,
                        &target_drive,
                        &image_path,
                        &index,
                        &compress,
                    );
                    return;
                }
                2 => {
                    // 进入 Windows RE：先弹确认框（明确告知后续动作），
                    // 确认后再走管理员 prepare → WinRE 任务链，避免"只关弹窗无反应"。
                    // 测试钩子指定了 choice 时跳过确认框（自动化环境无法精确点击）。
                    if state.test_drive_choice.is_none() {
                        // 文案按实际操作区分：此前写死「备份」，还原/新增第二系统时
                        // 也在说"执行备份"，用户会以为点错了。
                        // 操作名走 operation_display 的显示名，不能直接用内部键：2026-09-30
                        // 实机确认框显示成「将创建backup任务」，中文界面里混进英文内部键。
                        let operation_text = operation_display(language, &operation);
                        let (body, caption) = if language == Language::English {
                            (
                                format!(
                                    "A {operation_text} task will be created and prepared with administrator rights, then the system will restart into the recovery environment to run it. Continue?"
                                ),
                                "Enter the recovery environment".to_string(),
                            )
                        } else {
                            (
                                format!(
                                    "将创建「{operation_text}」任务并以管理员权限准备，准备完成后系统会重启进入恢复环境执行。是否继续？"
                                ),
                                "进入恢复环境".to_string(),
                            )
                        };
                        let confirm =
                            show_message(state.root, &body, &caption, MB_YESNO | MB_ICONQUESTION);
                        append_gui_log(
                            state,
                            &format!(
                                "system drive operation: Windows RE confirm result={confirm} (6=yes)"
                            ),
                        );
                        if confirm != IDYES {
                            append_gui_log(
                                state,
                                "system drive operation: Windows RE confirm declined",
                            );
                            return;
                        }
                    }
                    append_gui_log(
                        state,
                        "system drive operation: user chose Windows RE (prepare chain)",
                    );
                }
                _ => {
                    append_gui_log(state, "system drive operation cancelled by user");
                    return;
                }
            }
        } else {
            // 非当前活动系统（数据盘 / 未运行的第二系统）：在线直接执行，不重启
            append_gui_log(
                state,
                &format!("online operation: affected={affected_upper} system={system_upper}"),
            );
            run_online_operation(
                state,
                &operation,
                &source_drive,
                &target_drive,
                &image_path,
                &index,
                &compress,
                &image_name,
                keep_indexes,
                false,
            );
            return;
        }
    }
    // ===== B/C 入口按「通道单选」分流（默认在线；离线 = 重启进 PE/RE）=====
    if matches!(operation.as_str(), "restore-data" | "create-secondary") {
        let online = IsDlgButtonChecked(state.root, ID_CHANNEL_ONLINE as i32) != 0;
        append_gui_log(
            state,
            &format!("channel split: op={operation} online={online}"),
        );
        if online {
            // 在线：按「格式化目标分区」选项决定是否清空目标卷（默认勾选）——
            // B=数据卷覆盖/格式化应用；C=格式化+应用+修复引导（online_operation
            // 的 create-secondary 分支，同样尊重格式化选项）。
            // C 的启动菜单名通过 image_name 传给在线通道——restore_to_windows_volume
            // 会优先用它命名新启动项（用户命名 > 镜像版本名自动推断）。
            let online_image_name = if operation == "create-secondary" {
                get_text(state.controls.menu).trim().to_string()
            } else {
                String::new()
            };
            let format_target =
                IsDlgButtonChecked(state.root, ID_FORMAT_TARGET as i32) != 0;
            append_gui_log(
                state,
                &format!("online params: format_target={format_target}"),
            );
            run_online_operation(
                state,
                cli_operation(&operation),
                &source_drive,
                &target_drive,
                &image_path,
                &index,
                &compress,
                &online_image_name,
                keep_indexes,
                format_target,
            );
            return;
        }
        // 离线 B：目标是数据卷（不是系统盘），PE/RE 弹窗用通用文案；
        // PE=排队 PE 任务，RE=确认后走提权 prepare 链。
        if operation == "restore-data" {
            CHOICE_GENERIC_OFFLINE.store(true, std::sync::atomic::Ordering::SeqCst);
            let choice = match state.test_drive_choice {
                Some(c) => c,
                None => ask_system_drive_handler(state.root, language),
            };
            // 弹窗在 WM_CREATE 里已 swap(false) 消费；测试钩子跳过弹窗时手动复位，
            // 防止标志泄漏到下一次系统盘弹窗（文案错成通用版）。
            CHOICE_GENERIC_OFFLINE.store(false, std::sync::atomic::Ordering::SeqCst);
            append_gui_log(
                state,
                &format!("offline data restore choice: {choice} (0=cancel 1=PE 2=RE)"),
            );
            match choice {
                1 => {
                    append_gui_log(
                        state,
                        "offline data restore: user chose PE (schedule PE task)",
                    );
                    schedule_pe_task(
                        state,
                        cli_operation(&operation),
                        &source_drive,
                        &target_drive,
                        &image_path,
                        &index,
                        &compress,
                    );
                    return;
                }
                2 => {
                    // 进入 Windows RE：与 A/备份相同的确认框机制
                    if state.test_drive_choice.is_none() {
                        let operation_text = operation_display(language, &operation);
                        let (body, caption) = if language == Language::English {
                            (
                                format!(
                                    "A {operation_text} task will be created and prepared with administrator rights, then the system will restart into the recovery environment to run it. Continue?"
                                ),
                                "Enter the recovery environment".to_string(),
                            )
                        } else {
                            (
                                format!(
                                    "将创建「{operation_text}」任务并以管理员权限准备，准备完成后系统会重启进入恢复环境执行。是否继续？"
                                ),
                                "进入恢复环境".to_string(),
                            )
                        };
                        let confirm =
                            show_message(state.root, &body, &caption, MB_YESNO | MB_ICONQUESTION);
                        append_gui_log(
                            state,
                            &format!(
                                "offline data restore: Windows RE confirm result={confirm} (6=yes)"
                            ),
                        );
                        if confirm != IDYES {
                            append_gui_log(
                                state,
                                "offline data restore: Windows RE confirm declined",
                            );
                            return;
                        }
                    }
                    append_gui_log(
                        state,
                        "offline data restore: user chose Windows RE (prepare chain)",
                    );
                }
                _ => {
                    append_gui_log(state, "offline data restore cancelled by user");
                    return;
                }
            }
        }
        // 离线 C（还原系统到非 C 盘）：与「新增第二系统」完全一致的离线链——
        // destructive 确认已在上方完成，直接落到下面的提权 prepare。
    }
    let executable =
        std::env::current_exe().unwrap_or_else(|_| state.executable_dir.join("BackupRestore.exe"));
    let mut arguments = vec![
        "prepare".to_string(),
        // GUI 入口归一成 CLI operation：B→restore-existing、C→create-secondary
        //（2.4.9 起收尾按目标性质路由，CLI 侧无需感知三入口细分）。
        "--operation".to_string(),
        cli_operation(&operation).to_string(),
        "--source-drive".to_string(),
        source_drive.clone(),
        "--target-drive".to_string(),
        target_drive.clone(),
        "--boot-menu-name".to_string(),
        get_text(state.controls.menu),
    ];
    if operation != "probe" {
        arguments.extend([
            "--image-path".to_string(),
            image_path,
            "--wim-index".to_string(),
            index,
        ]);
    }
    if operation == "backup" {
        // 压缩下拉：值即 fast/none（DISM 术语，语言无关，已在上方分流处解析）。
        arguments.extend(["--compress".to_string(), compress]);
        // 备份索引名（用户输入，默认程序启动时间）与保留最近 N 个索引。
        arguments.extend(["--image-name".to_string(), image_name]);
        if let Some(keep) = keep_indexes {
            arguments.extend(["--keep-indexes".to_string(), keep.to_string()]);
        }
    }
    if is_image_restore_entry(&operation) {
        arguments.push("--allow-destructive".to_string());
        // 档案缺失/哈希不匹配且用户已确认 → 跳过哈希校验。
        if force_restore_hash {
            arguments.push("--force-restore-hash".to_string());
        }
        if verify_hash_enabled {
            arguments.push("--verify-hash".to_string());
        }
        // 格式化选项（离线链路）：三个还原入口取消勾选「格式化目标分区」时，
        // prepare 落盘 format_target=false，RE 恢复链做覆盖式应用（跳过 diskpart）。
        // 默认勾选不传参（=格式化，历史行为）。
        if matches!(
            operation.as_str(),
            "restore-existing" | "restore-data" | "create-secondary"
        ) && IsDlgButtonChecked(state.root, ID_FORMAT_TARGET as i32) == 0
        {
            append_gui_log(state, "prepare args: --skip-format (overlay restore)");
            arguments.push("--skip-format".to_string());
        }
    }
    if operation == "probe" {
        arguments.push("--no-reboot".to_string());
    }
    // 恢复环境失败处理策略 + 界面语言（2026-10-16 用户拍板做成设置项）。
    // 显式传给 prepare，由它落盘进 `RecoveryTask.env`——这样「任务创建时用的是
    // 哪套策略」被固定在任务里，事后改设置不会改变已有任务的行为。
    arguments.extend([
        "--fail-threshold".to_string(),
        state.failure_policy.threshold.to_string(),
        "--fail-action".to_string(),
        state.failure_policy.action.env_name().to_string(),
        // `pause` 策略的最长等待秒数（0 = 一直等 = 旧版行为）。
        "--fail-pause-max-wait-secs".to_string(),
        state.failure_policy.pause_max_wait_secs.to_string(),
        // DISM 等待上限（2026-10-16 审查补上）：显式传给 prepare，由它落盘进
        // `RecoveryTask.env`，恢复环境在入口灌进进程环境变量。0 = 不限制。
        "--dism-timeout-secs".to_string(),
        state.dism_timeout_secs.to_string(),
        "--ui-language".to_string(),
        if language == Language::English {
            "en".to_string()
        } else {
            "zh".to_string()
        },
    ]);
    let executable_wide = wide(&executable.to_string_lossy());
    let params = arguments
        .iter()
        .map(|argument| quote_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let runas = wide("runas");
    let params = wide(&params);
    append_gui_log(
        state,
        &format!("elevated prepare: launching (visible) operation={operation} params={params:?}"),
    );
    let mut exec_info = ShellExecuteInfoW {
        cb_size: size_of::<ShellExecuteInfoW>() as u32,
        f_mask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: state.root,
        lp_verb: runas.as_ptr(),
        lp_file: executable_wide.as_ptr(),
        lp_parameters: params.as_ptr(),
        lp_directory: null(),
        n_show: SW_SHOWNORMAL,
        h_inst_app: null_mut(),
        lp_id_list: null_mut(),
        lp_class: null(),
        hkey_class: null_mut(),
        dw_hot_key: 0,
        h_icon: null_mut(),
        h_process: null_mut(),
    };
    let ok = ShellExecuteExW(&mut exec_info);
    if ok == 0 {
        let err = GetLastError();
        append_gui_log(
            state,
            &format!(
                "GUI action failed: elevated prepare launch; operation={operation}; ShellExecuteExW failed err={err}"
            ),
        );
        let err_msg = if language == Language::English {
            format!(
                "Could not start the elevated preparation script (Windows error {err}). No task, WinRE or reboot was requested."
            )
        } else {
            format!(
                "无法启动管理员准备脚本（Windows 错误码 {err}）。未创建任务、未修改 WinRE、未请求重启。"
            )
        };
        set_text(state.controls.status, &err_msg);
        show_message(
            state.root,
            &err_msg,
            if language == Language::English {
                "Preparation launch failed"
            } else {
                "准备启动失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    append_gui_log(
        state,
        &format!(
            "elevated prepare: ShellExecuteExW succeeded; h_process={:?}",
            exec_info.h_process
        ),
    );
    if !exec_info.h_process.is_null() {
        // 等待 1500 毫秒，检测 prepare 进程是否因参数校验或安全规则立即异常退出。
        // 如果 1500ms 内退出，说明前置校验失败，必须立即读取 launcher-errors.log 弹窗报错；
        // 如果 1500ms 后仍在运行，说明校验通过，正在正常执行 DISM 挂载与 BCD 准备。
        let wait_result = WaitForSingleObject(exec_info.h_process, 1500);
        if wait_result == 0 {
            let mut exit_code: u32 = 0;
            GetExitCodeProcess(exec_info.h_process, &mut exit_code);
            CloseHandle(exec_info.h_process);
            if exit_code != 0 {
                let last_error = read_latest_launcher_error().unwrap_or_else(|| {
                    if language == Language::English {
                        format!("Preparation process terminated abnormally with exit code {exit_code}")
                    } else {
                        format!("管理员准备进程异常退出，退出码：{exit_code}")
                    }
                });
                append_gui_log(
                    state,
                    &format!(
                        "elevated prepare failed early: exit_code={exit_code} error={last_error}"
                    ),
                );
                let display_err = if language == Language::English {
                    format!("Preparation failed (exit code {exit_code}):\n\n{last_error}")
                } else {
                    format!("准备失败（退出码 {exit_code}）：\n\n{last_error}")
                };
                set_text(state.controls.status, &display_err);
                show_message(
                    state.root,
                    &display_err,
                    if language == Language::English {
                        "Preparation Failed"
                    } else {
                        "任务准备失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        } else {
            CloseHandle(exec_info.h_process);
        }
    }
    append_gui_log(
        state,
        &format!(
            "GUI action delegated: elevated prepare launched; operation={operation}; source={source_drive}; target={target_drive}"
        ),
    );
    set_text(
        state.controls.status,
        if operation == "probe" {
            if language == Language::English {
                "Probe preparation started with -NoReboot. Check status.json and logs; no backup, restore or reboot will run."
            } else {
                "已启动探测准备流程（不重启）。请查看任务状态和日志；不会备份、还原或格式化。"
            }
        } else if backup_will_append {
            if language == Language::English {
                "Elevated preparation started. The existing WIM will receive a new index; existing indexes remain unchanged until the append candidate is verified."
            } else {
                "已启动管理员准备流程。现有 WIM 将追加一个新索引；候选镜像验证完成前，原有索引不会被修改。"
            }
        } else if language == Language::English {
            "Elevated preparation started. Check status.json, prepare.log and Recovery.log; this is not recovery success."
        } else {
            "已启动管理员准备脚本。请在任务结果中查看 status.json、prepare.log 和 Recovery.log；这不是恢复成功证明。"
        },
    );
}

// ===================== 智能分流：进入 PE / 在线直接执行 =====================

/// 将绑定稳定卷身份和镜像索引的请求持久写入 ESP，成功后才安排一次性 PE 启动。
unsafe fn schedule_pe_task(
    state: &State,
    operation: &str,
    source_drive: &str,
    target_drive: &str,
    image_path: &str,
    _index: &str,
    _compress: &str,
) {
    let language = selected_language(state);
    let guid_file = state.executable_dir.join("pe-entry-guid.txt");
    let guid = std::fs::read_to_string(&guid_file)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let Some(guid) = guid else {
        show_message(
            state.root,
            if language == Language::English {
                "No PE entry installed yet. Open the Install PE tab, install the PE first, then retry."
            } else {
                "尚未安装 PE。请先在「安装 PE」页完成 PE 安装，再重试。"
            },
            if language == Language::English {
                "PE entry missing"
            } else {
                "未安装 PE"
            },
            MB_OK | MB_ICONINFORMATION,
        );
        return;
    };
    let request = (|| -> Result<(), backuprestore_core::TaskError> {
        let mode = if operation == "backup" {
            "backup"
        } else {
            "restore"
        };
        let drive = if mode == "backup" {
            source_drive
        } else {
            target_drive
        };
        let index = _index
            .parse::<u32>()
            .map_err(|_| crate::errc(ErrCode::ImageIndexInvalid))?;
        // overlay_no_acl=false：PE 三入口（backup/restore/secondary）恒格式化，
        // 不存在「覆盖式应用到已含 Windows 卷」组合（任务 #110）。
        let request = crate::pe_operation::new_request(mode, drive, image_path, index, None, "", false)?;
        crate::pe_operation::enqueue(&request)
    })();
    let write_ok = request.is_ok();
    if let Err(error) = request {
        show_message(
            state.root,
            &error.to_string(),
            "任务写入失败，未修改启动项",
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let command = format!("bcdedit.exe /set {{bootmgr}} bootsequence {{{guid}}}");
    let code = run_boot_command_logged(Some(state), "schedule_pe_task", &command);
    if code.is_success() && write_ok {
        append_gui_log(state, "schedule_pe_task: bootsequence set, auto reboot now");
        if ExitWindowsEx(EWX_REBOOT, 0) == 0 {
            let fallback = run_cmd_to_file("shutdown.exe /r /t 0 /f", None);
            append_gui_log(
                state,
                &format!("schedule_pe_task: shutdown.exe fallback code={fallback}"),
            );
            if !fallback.is_success() {
                show_message(
                    state.root,
                    if language == Language::English {
                        "PE task configured and boot sequence set, but auto-reboot failed. Please restart manually."
                    } else {
                        "已写入 PE 任务配置并设置一次性启动项，但自动重启失败，请手动重启进入 PE。"
                    },
                    if language == Language::English {
                        "Auto-reboot failed"
                    } else {
                        "自动重启失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
            }
        }
    } else {
        show_message(
            state.root,
            &if language == Language::English {
                format!(
                    "Failed to configure PE task (bcdedit code {code}, config write {write_ok}). Run as administrator."
                )
            } else {
                format!(
                    "配置失败（bcdedit 退出码 {code}，配置写入 {write_ok}）。请确认以管理员身份运行。"
                )
            },
            if language == Language::English {
                "Failed"
            } else {
                "设置失败"
            },
            MB_OK | MB_ICONERROR,
        );
    }
}

use crate::online_operation::{OnlineOpParams, OnlineResult};
static ONLINE_RESULT: std::sync::Mutex<Option<OnlineResult>> = std::sync::Mutex::new(None);
static ONLINE_BUSY: backuprestore_core::operation_safety::OperationGate =
    backuprestore_core::operation_safety::OperationGate::new();

#[allow(clippy::too_many_arguments)]
unsafe fn run_online_operation(
    state: &State,
    operation: &str,
    source_drive: &str,
    target_drive: &str,
    image_path: &str,
    index: &str,
    compress: &str,
    image_name: &str,
    keep_indexes: Option<u32>,
    format_target: bool,
) {
    if !ONLINE_BUSY.try_start() {
        show_message(
            state.root,
            "已有在线任务正在执行或结果尚未处理，请等待完成。",
            "任务运行中",
            MB_OK | MB_ICONWARNING,
        );
        return;
    }
    let params = OnlineOpParams {
        operation: operation.into(),
        source_drive: source_drive.into(),
        target_drive: target_drive.into(),
        image_path: image_path.into(),
        index: index.into(),
        compress: compress.into(),
        image_name: image_name.into(),
        keep_indexes,
        format_target,
    };
    let root = state.root as usize;
    EnableWindow(GetDlgItem(state.root, ID_CREATE_TASK as i32), 0);
    // 忙标志一直保持到主窗口取走结果；旧结果不会被新任务清空。
    let spawn = std::thread::Builder::new()
        .name("online-operation".into())
        .spawn(move || {
            // payload 一并带进结果：2026-10-16 审查发现原实现是
            // `unwrap_or_else(|_| …)`，把 panic 载荷（断言消息、文件行号）整个
            // 丢掉，只留一句「线程异常退出」。windows subsystem 没有控制台，
            // 而 `install_panic_logging` 装的 hook 也已把同一份内容写进
            // `<exe>\logs\panic.log`，两边互为兜底。
            let result = std::panic::catch_unwind(|| crate::online_operation::execute(&params))
                .unwrap_or_else(|payload| {
                    let detail = crate::panic_payload_text(payload.as_ref());
                    OnlineResult {
                        success: false,
                        detail: format!(
                            "在线执行线程异常退出（{detail}）。完整调用栈见 \
                             <程序目录>\\logs\\panic.log；请检查本轮日志和候选目录；不能判定为成功。"
                        ),
                        // panic 发生时连 `execute` 的第一行都没走完，没有本轮日志。
                        log_path: None,
                        // panic 不是业务失败，没有稳定码（归因看 panic.log）。
                        error_code: None,
                    }
                });
            *ONLINE_RESULT.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            unsafe {
                PostMessageW(root as Hwnd, WM_APP_ONLINE_DONE, 0, 0);
            }
        });
    if let Err(error) = spawn {
        ONLINE_BUSY.finish();
        EnableWindow(GetDlgItem(state.root, ID_CREATE_TASK as i32), 1);
        show_message(
            state.root,
            &format!("无法启动在线线程：{error}"),
            "启动失败",
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 消息投递失败时由定时器兜底；结果存在则处理一次，不通过文本猜测是否成功。
    SetTimer(state.root, 9002, 1000, None);
    set_text(
        state.controls.status,
        "在线任务执行中；请勿关闭程序。进度窗口显示当前阶段，完成后报告结果。",
    );
    append_gui_log(
        state,
        &format!("online started: op={operation} source={source_drive} target={target_drive}"),
    );
}

unsafe extern "system" fn window_proc(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    if message == WM_CREATE {
        let executable_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|value| value.to_path_buf()))
            .unwrap_or_default();
        let drives = discover_drives();
        // B/C 入口的「在线/离线」通道单选：创建坐标只是占位，
        // layout_operation 会按目标卷详情框下方重新定位并刷新文案；
        // 默认选中「在线」，select_operation 每次进入 B/C 时复位。
        // 访问统一走 GetDlgItem(ID_CHANNEL_ONLINE/OFFLINE)，不进 Controls。
        let _channel_online = create_control(
            hwnd,
            "BUTTON",
            "在线执行（不重启）",
            WS_TABSTOP | BS_AUTORADIOBUTTON | WS_GROUP,
            180,
            620,
            300,
            22,
            ID_CHANNEL_ONLINE,
        );
        let _channel_offline = create_control(
            hwnd,
            "BUTTON",
            "离线执行（重启进 PE/RE）",
            WS_TABSTOP | BS_AUTORADIOBUTTON,
            500,
            620,
            360,
            22,
            ID_CHANNEL_OFFLINE,
        );
        SendMessageW(_channel_online, BM_SETCHECK, BST_CHECKED, 0);
        // B 入口的「格式化目标分区」复选框：默认勾选（B 的正确语义=清空后写入）。
        // 坐标占位，layout_operation 定位；文案随语言刷新。
        let _format_target = create_control(
            hwnd,
            "BUTTON",
            "格式化目标分区（清空目标卷后写入镜像）",
            BS_AUTOCHECKBOX | WS_TABSTOP,
            180,
            650,
            420,
            22,
            ID_FORMAT_TARGET,
        );
        SendMessageW(_format_target, BM_SETCHECK, BST_CHECKED, 0);
        let (system_drive, _image_drive) = suggested_drive_defaults(&drives);
        // The image field is intentionally blank until the user chooses an
        // absolute path; no fixed BackupRestore folder is assumed.
        let image_path = String::new();
        let controls = Controls {
            language: create_control(
                hwnd,
                "COMBOBOX",
                "中文",
                CBS_DROPDOWNLIST | WS_TABSTOP,
                // 语言栏靠右显示：初始坐标按最小客户宽 1020 右对齐
                //（combo 右缘 = 1020-20），layout_operation 会按实际宽度动态重排。
                820,
                TOP_BAR_Y,
                180,
                300,
                ID_LANGUAGE,
            ),
            operation_tabs: [
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    // WS_GROUP 标记「操作模式」单选组的起点：方向键只在这 5
                    // 个之间切换，不会跳到下方 PE 启动方式/通道那一组。
                    // 布局：入口名用用户拍板的全称（长短不一），按钮不等宽、
                    // 间距 3px，从 x=90 排到 ~741。★ 顶栏按钮与语言栏**不在这一行**
                    // （2026-10-26 起独立到 TOP_BAR_Y）：此前两者共用 y=60，而
                    // 741 + 顶栏四个按钮 + 语言栏远超窗口宽度，实机量到 6 对重叠。
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE | WS_GROUP,
                    90,
                    60,
                    56,
                    32,
                    ID_OPERATION_BACKUP,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    149,
                    60,
                    120,
                    32,
                    ID_OPERATION_RESTORE,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    272,
                    60,
                    120,
                    32,
                    ID_OPERATION_RESTORE_DATA,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    395,
                    60,
                    150,
                    32,
                    ID_OPERATION_RESTORE_OTHER,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    548,
                    60,
                    72,
                    32,
                    ID_OPERATION_PE,
                ),
            ],
            source: create_control(
                hwnd,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST | WS_TABSTOP,
                180,
                220,
                800,
                220,
                ID_SOURCE,
            ),
            image: create_control(
                hwnd,
                "EDIT",
                &image_path,
                WS_BORDER | WS_TABSTOP,
                180,
                545,
                800,
                24,
                ID_IMAGE,
            ),
            target: create_control(
                hwnd,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST | WS_TABSTOP,
                180,
                330,
                800,
                220,
                ID_TARGET,
            ),
            index: create_control(
                hwnd,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST | WS_TABSTOP,
                180,
                585,
                300,
                220,
                ID_INDEX,
            ),
            menu: create_control(
                hwnd,
                "EDIT",
                "Windows Backup",
                WS_BORDER | WS_TABSTOP,
                600,
                585,
                380,
                24,
                ID_MENU,
            ),
            compress: create_control(
                hwnd,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST | WS_TABSTOP,
                480,
                585,
                260,
                220,
                ID_COMPRESS,
            ),
            index_name: create_control(
                hwnd,
                "EDIT",
                "",
                WS_BORDER | WS_TABSTOP,
                480,
                585,
                260,
                24,
                ID_INDEX_NAME_EDIT,
            ),
            keep: create_control(
                hwnd,
                "EDIT",
                "",
                WS_BORDER | WS_TABSTOP,
                480,
                585,
                80,
                24,
                ID_KEEP_EDIT,
            ),
            source_details: create_control(
                hwnd,
                "EDIT",
                "",
                WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY | WS_VSCROLL,
                180,
                250,
                800,
                crate::dpi::text_block_height(10, 8),
                ID_SOURCE_DETAILS,
            ),
            target_details: create_control(
                hwnd,
                "EDIT",
                "",
                WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY | WS_VSCROLL,
                180,
                360,
                800,
                crate::dpi::text_block_height(10, 8),
                ID_TARGET_DETAILS,
            ),
            verify_hash: create_control(
                hwnd,
                "BUTTON",
                "严格对比哈希（默认仅对比大小）",
                BS_AUTOCHECKBOX | WS_TABSTOP,
                500,
                585,
                300,
                24,
                ID_VERIFY_HASH,
            ),
            status: create_control(
                hwnd,
                "EDIT",
                "先点击“刷新环境”确认系统、恢复环境和卷身份。默认模式为备份。",
                WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | WS_VSCROLL,
                20,
                440,
                960,
                crate::dpi::text_block_height(9, 25),
                ID_STATUS,
            ),
        };
        add_combo_item(controls.language, "中文");
        add_combo_item(controls.language, "English");
        // 恢复上一次选的语言（默认中文）。索引 0/1 ↔ 中文/English。
        SendMessageW(
            controls.language,
            CB_SETCURSEL,
            match load_ui_language_pref() {
                Language::Chinese => 0,
                Language::English => 1,
            },
            0,
        );
        SendMessageW(controls.operation_tabs[0], BM_SETCHECK, BST_CHECKED, 0);
        // 压缩下拉：初始语言为中文；索引 0/1 ↔ fast/none，默认「压缩」。
        for label in compress_level_labels(Language::Chinese) {
            add_combo_item(controls.compress, label);
        }
        SendMessageW(controls.compress, CB_SETCURSEL, 0, 0);
        SendMessageW(controls.verify_hash, BM_SETCHECK, BST_UNCHECKED, 0);
        // 备份索引名默认值 = 当前系统版本 + 年月日（如 `Windows 11 26200.9550
        // 20261015`，2026-10-15 用户需求），用户可修改。
        // 此刻源卷列表还没填好，先按"源盘 = 当前系统盘"预填；源卷确定后
        // refresh_backup_index_name 会再校正一次（源盘不是系统盘时换成带盘符的
        // 降级名），用户手改过则一律不动。
        let default_index_name = default_backup_index_name(true, None);
        set_text(controls.index_name, &default_index_name);
        // 「操作模式」标签收窄（62px），给全称入口名的 tab 按钮让出左侧空间。
        create_control(hwnd, "STATIC", "操作模式", 0, 20, 55, 62, 22, 2001);
        create_control(hwnd, "STATIC", "源卷", 0, 20, 222, 150, 26, 2003);
        create_control(hwnd, "STATIC", "目标卷", 0, 20, 332, 150, 26, 2005);
        create_control(hwnd, "STATIC", "镜像绝对路径", 0, 20, 550, 130, 22, 2004);
        create_control(hwnd, "STATIC", "WIM 索引", 0, 20, 590, 130, 22, 2007);
        create_control(hwnd, "STATIC", "第二系统名称", 0, 490, 590, 100, 22, 2008);
        create_control(hwnd, "STATIC", "压缩率", 0, 20, 585, 150, 24, 2014);
        create_control(
            hwnd,
            "STATIC",
            ui_text(Language::Chinese, "index_name"),
            0,
            20,
            585,
            130,
            24,
            2015,
        );
        create_control(
            hwnd,
            "STATIC",
            ui_text(Language::Chinese, "keep"),
            0,
            560,
            585,
            110,
            24,
            2016,
        );
        create_control(
            hwnd,
            "STATIC",
            ui_text(Language::Chinese, "language"),
            // 语言标签与 combo 同行靠右（按最小宽 1020 的初始位置，
            // layout_operation 按实际客户宽动态右对齐）。
            0,
            759,
            67,
            55,
            18,
            ID_LANGUAGE_LABEL,
        );
        create_control(
            hwnd,
            "BUTTON",
            "设置…",
            WS_TABSTOP,
            // 初始坐标按最小客户宽 1020 右对齐（在语言栏左侧）；
            // layout_operation 按实际客户宽动态重排。
            661,
            TOP_BAR_Y,
            90,
            24,
            ID_SETTINGS,
        );
        create_control(
            hwnd,
            "BUTTON",
            "查看日志",
            WS_TABSTOP,
            // 初始坐标按最小客户宽 1020：在「备份文件管理…」(353-493) 左侧，
            // 227-347。layout_operation 按实际客户宽动态重排到它左边。
            227,
            TOP_BAR_Y,
            120,
            24,
            ID_VIEW_LOGS,
        );
        create_control(
            hwnd,
            "BUTTON",
            "备份文件管理…",
            WS_TABSTOP,
            // 初始坐标按最小客户宽 1020：在「取消待重启任务」(503-653) 左侧，
            // 353-493。layout_operation 按实际客户宽动态重排到它左边。
            353,
            TOP_BAR_Y,
            140,
            24,
            ID_IMAGE_ADMIN,
        );
        create_control(
            hwnd,
            "BUTTON",
            "取消待重启任务",
            WS_TABSTOP,
            // 初始坐标按最小客户宽 1020：在「设置…」(661-751) 左侧，
            // 503-653。layout_operation 按实际客户宽动态重排到「设置…」左边。
            503,
            TOP_BAR_Y,
            150,
            24,
            ID_CANCEL_PENDING,
        );
        create_control(
            hwnd,
            "BUTTON",
            "刷新环境",
            WS_TABSTOP,
            20,
            630,
            120,
            28,
            ID_REFRESH,
        );
        create_control(
            hwnd,
            "BUTTON",
            "读取镜像",
            WS_TABSTOP,
            150,
            630,
            120,
            28,
            ID_READ_IMAGE,
        );
        create_control(
            hwnd,
            "BUTTON",
            "浏览…",
            WS_TABSTOP,
            910,
            545,
            80,
            24,
            ID_BROWSE_IMAGE,
        );
        create_control(
            hwnd,
            "BUTTON",
            "创建任务",
            // 默认按钮：无焦点时按回车也触发「创建任务」（最常用操作）。
            WS_TABSTOP | BS_DEFPUSHBUTTON,
            280,
            630,
            120,
            28,
            ID_CREATE_TASK,
        );
        create_control(
            hwnd,
            "BUTTON",
            "刷新任务状态",
            WS_TABSTOP,
            410,
            630,
            140,
            28,
            ID_REFRESH_TASK,
        );
        create_control(
            hwnd,
            "BUTTON",
            "重启进入 PE",
            WS_TABSTOP,
            560,
            630,
            140,
            28,
            ID_PE_REBOOT_MAIN,
        );
        create_control(
            hwnd,
            "BUTTON",
            "创建快捷方式",
            WS_TABSTOP,
            710,
            630,
            140,
            28,
            ID_PE_SHORTCUT,
        );
        // 「安装 PE」tab：启动方式单选 + PE 目录名（仅 RAM disk 模式使用）
        create_control(
            hwnd,
            "BUTTON",
            "",
            BS_AUTORADIOBUTTON | WS_TABSTOP | WS_GROUP,
            20,
            700,
            220,
            22,
            ID_PE_MODE_RAM,
        );
        create_control(
            hwnd,
            "BUTTON",
            "",
            BS_AUTORADIOBUTTON | WS_TABSTOP,
            250,
            700,
            220,
            22,
            ID_PE_MODE_DISK,
        );
        create_control(hwnd, "STATIC", "", 0, 20, 728, 110, 22, ID_PE_DIR_LABEL);
        create_control(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP | ES_AUTOHSCROLL | WS_BORDER,
            140,
            726,
            260,
            24,
            ID_PE_DIR_EDIT,
        );
        // 「PE 目录路径」旁的浏览按钮（仅 RAM disk 模式显示，随目录行隐藏）
        create_control(
            hwnd,
            "BUTTON",
            "浏览…",
            WS_TABSTOP,
            410,
            726,
            60,
            24,
            ID_PE_DIR_BROWSE,
        );
        // 「PE 启动项名称」输入框（两种启动方式通用：开机 Boot Manager
        // 菜单里显示的名字，用户可自定义，默认按语言+模式自动填）
        create_control(hwnd, "STATIC", "", 0, 20, 750, 130, 22, ID_PE_NAME_LABEL);
        create_control(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP | ES_AUTOHSCROLL | WS_BORDER,
            140,
            748,
            300,
            24,
            ID_PE_NAME_EDIT,
        );
        SendMessageW(
            GetDlgItem(hwnd, ID_PE_MODE_RAM as i32),
            BM_SETCHECK,
            BST_CHECKED,
            0,
        );
        // 设置文件在程序目录；读不到 / 值非法一律回退默认（3 次 + 自动逃出）。
        // 必须在把 `executable_dir` 移进 State 之前算好。
        let failure_policy = crate::failure_policy::FailurePolicy::load_from_dir(&executable_dir);
        let dism_timeout_secs = crate::dism_timeout::load_from_dir(&executable_dir);
        // 2.6.81：界面字号在**窗口创建之前**读回来——`gui_font()` 与那几个
        //「按行数算的高度」都按它取值，晚了就会用默认值画一遍再改，白闪一下。
        crate::dpi::set_font_size(crate::dpi::load_font_size(&executable_dir));
        let state = Box::new(State {
            root: hwnd,
            tooltip: null_mut(),
            controls,
            executable_dir,
            wim_images: Vec::new(),
            drives,
            operation_index: 0,
            test_drive_choice: None,
            menu_name_touched: false,
            menu_name_auto: None,
            index_name_touched: false,
            index_name_auto: None,
            failure_policy,
            dism_timeout_secs,
        });
        let state_ptr = Box::into_raw(state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
        append_gui_log(
            &*state_ptr,
            "GUI started; elevated native Rust window initialized",
        );
        install_tooltips(&mut *state_ptr);
        set_drive_items(
            &*state_ptr,
            [None, Some(system_drive.clone()), Some(system_drive)],
        );
        apply_language(&mut *state_ptr);
        append_gui_log(
            &*state_ptr,
            &format!(
                "PE controls: ram_radio_checked={} dir_edit_visible={}",
                IsDlgButtonChecked(hwnd, ID_PE_MODE_RAM as i32) != 0,
                !GetDlgItem(hwnd, ID_PE_DIR_EDIT as i32).is_null(),
            ),
        );
        let mut explicit_tab = false;
        if let Ok(image) = std::env::var("BACKUPRESTORE_OPEN_IMAGE")
            && !image.trim().is_empty()
        {
            select_operation(&mut *state_ptr, 1);
            set_text((*state_ptr).controls.image, &image);
            read_image(&mut *state_ptr);
            explicit_tab = true;
        }
        if let Ok(tab) = std::env::var("BACKUPRESTORE_OPEN_TAB")
            && let Ok(index) = tab.parse::<usize>()
            && (0..=4).contains(&index)
        {
            select_operation(&mut *state_ptr, index);
            explicit_tab = true;
        }
        // 命令行参数 --tab N（UAC 提升后命令行参数保留，比环境变量可靠）
        {
            let args: Vec<String> = std::env::args().collect();
            if let Some(pos) = args.iter().position(|a| a == "--tab")
                && let Some(value) = args.get(pos + 1)
                && let Ok(index) = value.parse::<usize>()
                && (0..=4).contains(&index)
            {
                select_operation(&mut *state_ptr, index);
                explicit_tab = true;
            }
        }
        if !explicit_tab {
            select_operation(&mut *state_ptr, 0);
        }
        // 测试钩子：仅在显式 --test-hook 参数下读取 C:\br-test.json（正常启动不读，
        // 避免残留 JSON 导致程序一启动就自动执行备份/还原并弹窗）
        {
            let args: Vec<String> = std::env::args().collect();
            if args.iter().any(|a| a == "--test-hook") {
                test_hook_auto_install(&mut *state_ptr);
            }
        }
        return 0;
    }
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if !state_ptr.is_null() {
        let state = &mut *state_ptr;
        if message == WM_APP_TEST_INSTALL {
            append_gui_log(state, "test hook: install triggered");
            create_task(state);
            return 0;
        }
        if message == WM_APP_ONLINE_DONE || (message == WM_TIMER && w_param == 9002) {
            let result = ONLINE_RESULT
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if let Some(result) = result {
                KillTimer(state.root, 9002);
                // 弹窗期间继续保持忙状态，避免模态消息循环重入创建任务。
                // ★ 失败回执统一带稳定码（2.6.63）：`Coded` 变体的 Display 自带
                // `[BR-…]`，但 `Io`/`Json`/`InvalidTransition` 三类**故意不加前缀**
                // （文案是 OS 本地化串或 serde 原文），它们的码只存在
                // `result.error_code` 里。2.6.51 专门把码带出来给 `--json`，界面
                // 却一直没消费——于是「磁盘满 / 句柄占用」这类最该查码的失败，
                // 用户在弹窗和状态栏里一个码都看不到。
                // `with_code` 幂等：已有码的原样返回，不会叠两个。
                let receipt =
                    crate::failure_receipt::with_code(&result.detail, result.error_code);
                append_gui_log(
                    state,
                    &format!("online completed: success={}\n{receipt}", result.success),
                );
                let status_text = if result.success {
                    "在线任务已完成并通过校验".to_string()
                } else {
                    crate::failure_receipt::with_code(
                        "在线任务失败，请检查日志；不要将部分产物视为成功",
                        result.error_code,
                    )
                };
                set_text(state.controls.status, &status_text);
                show_message(
                    state.root,
                    &receipt,
                    if result.success {
                        "执行完成"
                    } else {
                        "执行失败"
                    },
                    MB_OK
                        | if result.success {
                            MB_ICONINFORMATION
                        } else {
                            MB_ICONERROR
                        },
                );
                ONLINE_BUSY.finish();
                EnableWindow(GetDlgItem(state.root, ID_CREATE_TASK as i32), 1);
            }
            return 0;
        }
        if message == WM_KEYDOWN {
            // 快捷键：Ctrl+B 备份 / Ctrl+R 还原 / Ctrl+P 安装 PE /
            // Ctrl+O 读取镜像 / F5 刷新环境 / Ctrl+Enter 创建任务。
            // Tab、方向键、回车（无 Ctrl）由 IsDialogMessage 处理，这里只接组合键。
            let key = w_param as u32;
            let ctrl_down = ((GetAsyncKeyState(VK_CONTROL as i32) as u16) & 0x8000) != 0;
            let shortcut = match (ctrl_down, key) {
                (true, VK_B) => Some(ID_OPERATION_BACKUP),
                (true, VK_R) => Some(ID_OPERATION_RESTORE),
                (true, VK_P) => Some(ID_OPERATION_PE),
                (true, VK_O) => Some(ID_READ_IMAGE),
                (false, VK_F5) => Some(ID_REFRESH),
                (true, VK_RETURN) => Some(ID_CREATE_TASK),
                _ => None,
            };
            if let Some(control_id) = shortcut {
                // 与真实鼠标点击走完全相同的 WM_COMMAND 路径。
                PostMessageW(hwnd, WM_COMMAND, control_id as WParam, 0);
                return 0;
            }
        }
        if message == WM_SIZE {
            layout_operation(state);
            return 0;
        }
        if message == WM_COMMAND {
            let control_id = w_param & 0xffff;
            let notification = (w_param >> 16) & 0xffff;
            // ★ 统一审计：所有用户操作**先留痕再走业务分支**——不管这个分支
            // 自己有没有写日志、也不管它最后是成功还是失败，gui.log 里都有一行
            // 「用户当时点了什么」。逐个分支补会漏、会随新功能腐化，放这里则
            // 天然全覆盖。分支自己的语义日志（GUI action completed: …）保留不动。
            audit_control_action(control_id, notification);
            if control_id == ID_LANGUAGE && notification == CBN_SELCHANGE {
                apply_language(state);
                // 只在这里保存（用户真的动了选择），初始化时不写盘。
                let language = selected_language(state);
                save_ui_language_pref(language);
                append_gui_log(
                    state,
                    &format!(
                        "GUI action completed: language changed (ui_language={} saved)",
                        match language {
                            Language::Chinese => "zh",
                            Language::English => "en",
                        }
                    ),
                );
                return 0;
            }
            if let Some(index) = match control_id {
                ID_OPERATION_BACKUP => Some(0),
                ID_OPERATION_RESTORE => Some(1),
                ID_OPERATION_RESTORE_DATA => Some(2),
                ID_OPERATION_RESTORE_OTHER => Some(3),
                ID_OPERATION_PE => Some(4),
                _ => None,
            } {
                select_operation(state, index);
                append_gui_log(
                    state,
                    &format!(
                        "GUI action completed: operation selected={}",
                        selected_operation(state)
                    ),
                );
                return 0;
            }
            // B/C 的通道单选切换后刷新一次布局（文案坐标已就绪，这里主要
            // 是把选择写进 gui.log，便于事后审计用户当时选的哪条通道）。
            if matches!(control_id, ID_CHANNEL_ONLINE | ID_CHANNEL_OFFLINE) {
                let online = IsDlgButtonChecked(state.root, ID_CHANNEL_ONLINE as i32) != 0;
                append_gui_log(state, &format!("channel selected: {}", if online { "online" } else { "offline" }));
                return 0;
            }
            if matches!(control_id, ID_SOURCE | ID_TARGET) && notification == CBN_SELCHANGE {
                set_drive_details(state);
                // 备份索引名默认带当前系统版本；源卷换成别的卷后那个版本就不适用了，
                // 按新源卷重算（用户手改过则不动）。
                refresh_backup_index_name(state);
                return 0;
            }
            // 索引切换：菜单名预填跟随新选中的 WIM 索引名（用户手改过则不动）。
            if control_id == ID_INDEX && notification == CBN_SELCHANGE {
                prefill_menu_name_from_image(state);
                return 0;
            }
            // 菜单名文本框：与程序自动值不同即判定用户手改，此后预填不再覆盖。
            if control_id == ID_MENU && notification == EN_CHANGE {
                let current = get_text(state.controls.menu);
                if state.menu_name_auto.as_deref() != Some(current.as_str()) {
                    state.menu_name_touched = true;
                }
                return 0;
            }
            // 备份索引名文本框：同理，与自动值不同即判定用户手改，此后不再重算。
            if control_id == ID_INDEX_NAME_EDIT && notification == EN_CHANGE {
                let current = get_text(state.controls.index_name);
                if state.index_name_auto.as_deref() != Some(current.as_str()) {
                    state.index_name_touched = true;
                }
                return 0;
            }
            match control_id {
                ID_REFRESH => refresh_environment(state),
                ID_READ_IMAGE => read_image(state),
                ID_BROWSE_IMAGE => browse_image(state),
                ID_PE_DIR_BROWSE => browse_pe_dir(state),
                ID_CREATE_TASK => create_task(state),
                ID_REFRESH_TASK => {
                    // 2.6.64 的行为保留：先把最近任务摘要刷到状态框
                    // （`_t-refresh-status.ps1` 的四结局用例依赖它），
                    // 再打开任务与证据窗口。不新增按钮——顶栏刚在 2.6.59
                    // 修过 6 对控件重叠，多一个就要重算整行宽度。
                    refresh_task_status(state);
                    let language = selected_language(state);
                    show_task_admin(state.root, language, &state.executable_dir);
                }
                ID_SETTINGS => open_settings(state),
                ID_IMAGE_ADMIN => open_image_admin(state),
                ID_VIEW_LOGS => open_logs_viewer(state),
                ID_CANCEL_PENDING => cancel_pending_task(state),
                ID_PE_REBOOT_MAIN => {
                    if ONLINE_BUSY.is_busy() {
                        show_message(
                            state.root,
                            "在线任务运行中，禁止重启或更改启动项。",
                            "任务运行中",
                            MB_OK | MB_ICONWARNING,
                        );
                    } else {
                        pe_reboot_to_pe(state);
                    }
                }
                ID_PE_SHORTCUT => pe_create_shortcut(state),
                ID_PE_MODE_RAM | ID_PE_MODE_DISK => {
                    // 切换启动方式：刷新目录行显隐与布局；
                    // 若启动项名称还是旧模式的默认名（用户未自定义），跟随新模式更新
                    let language = selected_language(state);
                    let ram_now = IsDlgButtonChecked(state.root, ID_PE_MODE_RAM as i32) != 0;
                    let name_edit_hwnd = GetDlgItem(state.root, ID_PE_NAME_EDIT as i32);
                    let current = get_text(name_edit_hwnd);
                    let trimmed = current.trim().to_string();
                    if !trimmed.is_empty()
                        && (trimmed == pe_entry_description(language, !ram_now)
                            || trimmed == pe_entry_description(language, ram_now))
                    {
                        let default_name = pe_entry_description(language, ram_now);
                        set_text(name_edit_hwnd, &default_name);
                    }
                    set_operation_visibility(state);
                    layout_operation(state);
                    set_volume_labels(state);
                    // 启动方式切换同样会引发重绘风暴，整窗同步重绘清除残留
                    InvalidateRect(state.root, null(), 1);
                    UpdateWindow(state.root);
                }
                _ => {}
            }
            return 0;
        }
    }
    if message == WM_CLOSE {
        if ONLINE_BUSY.is_busy() {
            show_message(
                hwnd,
                "在线任务尚未完成。为避免中断写入，请等待结果后再关闭。",
                "任务运行中",
                MB_OK | MB_ICONWARNING,
            );
            return 0;
        }
        DestroyWindow(hwnd);
        return 0;
    }
    if message == WM_DESTROY {
        if !state_ptr.is_null() {
            if !(*state_ptr).tooltip.is_null() {
                DestroyWindow((*state_ptr).tooltip);
            }
            drop(Box::from_raw(state_ptr));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(hwnd, message, w_param, l_param)
}

/// Exit selection shared between the PE desktop window procedure and
/// `run_pe_desktop`: 0 = no operation, 1 = backup, 2 = restore,
/// 3 = secondary system. A plain atomic avoids any use-after-free when the
/// window is destroyed inside WM_COMMAND.
static PE_EXIT_TAB: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Guard so a double click on "返回 Windows" cannot run two BCD rewrites at
/// once while the window stays alive during the exit sequence.
static PE_EXITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe fn update_pe_clock(state: &PeDesktopState) {
    let mut time = SystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    GetLocalTime(&mut time);
    let text = format!("{:02}:{:02}:{:02}", time.hour, time.minute, time.second);
    set_text(state.clock, &text);
}

/// Restore the boot manager default to `{current}` and reboot, so a PE session
/// launched through a temporary default does not trap the machine in PE. The
/// EFI system partition is located by enumerating volumes, mounted to `S:`,
/// and the BCD entry is rewritten with `bcdedit /store`.
/// Reboot the PE session. `ExitWindowsEx` needs shutdown privileges that the
/// PE shell may lack, so fall back to `wpeutil.exe reboot` (the PE-native
/// restart tool) exactly like the restart card does.
unsafe fn pe_reboot(hwnd: Hwnd) {
    if ExitWindowsEx(EWX_REBOOT, 0) == 0 {
        let wpe = wide("wpeutil.exe");
        let argument = wide("reboot");
        ShellExecuteW(
            hwnd,
            null(),
            wpe.as_ptr(),
            argument.as_ptr(),
            null(),
            SW_SHOW,
        );
    }
}

/// Best-effort diagnostics written via the **verified** ESP volume path
/// (`\\?\Volume{GUID}\exit-pe.log`, survives unmount and reboot) plus a copy
/// in that same volume's log subdirectory, and `X:\exit-pe.log` (PE RAM disk),
/// so a failed exit attempt can be diagnosed from the next Windows session.
///
/// 此前这里还会盲写 `S:\BackupRestore\logs\exit-pe.log`，等于假设 `S:` 就是 ESP。
/// `S:` 被数据卷占用、或 ESP 挂在别的盘符时，本项目的日志会落到一个无关卷上，
/// 而且读日志的人无法分辨。现在所有 ESP 侧路径都从调用方传进来的已核验卷根派生
/// （见 `text_parsing::esp_log_path_on`），卷根拿不到时就只写 RAM 盘，不猜盘符。
///
/// Writes go through CreateFileW/WriteFile directly because std::fs writes to
/// mounted FAT volumes failed silently inside PE. Returns per-path failure
/// details (including GetLastError) so the caller can surface them during
/// development.
fn write_pe_exit_log(entries: &[String], esp_volume_path: Option<&str>) -> Vec<String> {
    let text = entries.join("\r\n");
    let mut failures: Vec<String> = Vec::new();
    // 优先写卷路径（\\?\Volume{GUID}\ 独立于盘符，PE 内最可靠，重启后仍保留在
    // ESP 上），再写同一个已核验卷上的日志子目录副本，最后写 X:（PE RAM 盘，
    // 重启即丢，仅供当次排查）。
    // 不猜盘符：卷根拿不到时宁可只留 RAM 盘那一份，也不往 S:/Q: 盲写——
    // PE 里 Q: 通常不存在（Win11 侧盘符漂移），S: 也可能是别的卷。
    let mut paths: Vec<String> = Vec::new();
    if let Some(vp) = esp_volume_path {
        let mut full = vp.to_string();
        if !full.ends_with('\\') {
            full.push('\\');
        }
        full.push_str("exit-pe.log");
        paths.push(full);
        paths.push(crate::text_parsing::esp_log_path_on(vp, "exit-pe.log"));
    }
    paths.push("X:\\exit-pe.log".to_string());
    for path in paths {
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let file = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0x40000000, // GENERIC_WRITE
                1,          // FILE_SHARE_READ
                null_mut(),
                2,    // CREATE_ALWAYS
                0x80, // FILE_ATTRIBUTE_NORMAL
                null_mut(),
            )
        };
        if file as isize == -1 || file.is_null() {
            failures.push(format!("create {} failed, err={}", path, unsafe {
                GetLastError()
            }));
            continue;
        }
        let mut written: u32 = 0;
        let ok = unsafe {
            WriteFile(
                file,
                text.as_ptr() as *const c_void,
                text.len() as u32,
                &mut written,
                null_mut(),
            )
        };
        if ok == 0 {
            failures.push(format!("write {} failed, err={}", path, unsafe {
                GetLastError()
            }));
        }
        unsafe {
            CloseHandle(file);
        }
    }
    failures
}

/// Development aid: show the full exit diagnostics in a message box right
/// before rebooting, because log files inside PE are unreliable (volume-path
/// and drive-letter writes both failed in the Parallels PE session).
fn show_diag_dialog(hwnd: Hwnd, diag: &[String]) {
    // 自动点击模式：跳过诊断弹窗（无输入通道，弹窗会阻塞自动流程）。
    if PE_AUTO_CLICK.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let text = diag.join("\r\n");
    let msg: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let cap: Vec<u16> = "BackupRestore exit diag"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        MessageBoxW(hwnd, msg.as_ptr(), cap.as_ptr(), 0x40); // MB_ICONINFORMATION
    }
}

/// Restore the boot manager default to `{current}` and reboot, so a PE session
/// launched through a temporary default does not trap the machine in PE. The
/// EFI system partition is located by enumerating volumes, mounted at a free
/// drive letter (S: preferred, then T:..Z:), and the BCD entry is rewritten by
/// running `bcdedit.exe` synchronously with a captured exit code.
unsafe fn exit_pe_to_windows(hwnd: Hwnd) {
    if PE_EXITING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let mut diag: Vec<String> = Vec::new();
    diag.push("exit_pe_to_windows start".to_string());
    let mut esp_volume_path: Option<String> = None;
    let mut volume = [0u16; 512];
    let handle = FindFirstVolumeW(volume.as_mut_ptr(), 512);
    if handle as isize == -1 || handle.is_null() {
        diag.push("FindFirstVolumeW failed".to_string());
        let _ = write_pe_exit_log(&diag, None);
        show_diag_dialog(hwnd, &diag);
        PE_EXITING.store(false, std::sync::atomic::Ordering::SeqCst);
        return;
    }
    let mut found_bcd = false;
    // 现场认不出 Windows 条目时，若 default 不是 PE 条目，仍可以照着现状重启
    // （bootmgr 会回落到 displayorder 第一项）。与 `found_bcd` 分开记，免得
    // 「改成功了」和「没改但能安全重启」两种结局混成一个布尔。
    let mut reboot_without_target = false;
    loop {
        let length = volume.iter().position(|&unit| unit == 0).unwrap_or(0);
        let volume_path = String::from_utf16_lossy(&volume[..length]);
        let mut volume_name = [0u16; 64];
        let mut filesystem = [0u16; 64];
        let mut serial = 0u32;
        let mut max_component = 0u32;
        let mut flags = 0u32;
        let queried = GetVolumeInformationW(
            wide(&volume_path).as_ptr(),
            volume_name.as_mut_ptr(),
            64,
            &mut serial,
            &mut max_component,
            &mut flags,
            filesystem.as_mut_ptr(),
            64,
        );
        if queried != 0 {
            let fs_length = filesystem.iter().position(|&unit| unit == 0).unwrap_or(0);
            let fs = String::from_utf16_lossy(&filesystem[..fs_length]).to_ascii_uppercase();
            diag.push(format!(
                "volume: {} fs: {}",
                volume_path.trim_end_matches('\\'),
                fs
            ));
            if fs == "FAT" || fs == "FAT32" {
                diag.push(format!(
                    "FAT volume: {}",
                    volume_path.trim_end_matches('\\')
                ));
                // Try S: then T:..Z: for a free mount point (X: is the PE RAM disk).
                let mut mounted_letter: Option<u16> = None;
                for letter in ['S', 'T', 'U', 'V', 'W', 'Y', 'Z'] {
                    let mut mount = [0u16; 4];
                    mount[0] = letter as u16;
                    mount[1] = ':' as u16;
                    mount[2] = '\\' as u16;
                    if SetVolumeMountPointW(mount.as_ptr(), wide(&volume_path).as_ptr()) != 0 {
                        mounted_letter = Some(letter as u16);
                        diag.push(format!("mounted at {}:", letter));
                        break;
                    } else {
                        diag.push(format!(
                            "mount {}: failed, last error: {}",
                            letter,
                            GetLastError()
                        ));
                    }
                }
                if let Some(letter) = mounted_letter {
                    // 盘符显式转换：`letter as u8 as char` 在 u16 >255 时会静默截断成
                    // 错盘符（当前 'S'/'T'/'Z' 恰好 <256 是偶然正确）。转换不出来就
                    // 给一个不可能存在的盘符，让下面的 GetFileAttributesW 自然失败，
                    // 同时留诊断信息——本函数无返回值，不引入新的控制流。
                    let letter_char = char::from_u32(u32::from(letter))
                        .filter(|c| c.is_ascii_alphabetic())
                        .unwrap_or('?');
                    if letter_char == '?' {
                        diag.push(format!("ESP letter {letter} is not an ASCII drive letter"));
                    }
                    let bcd_path = format!("{letter_char}:\\EFI\\Microsoft\\Boot\\BCD");
                    if GetFileAttributesW(wide(&bcd_path).as_ptr()) != u32::MAX {
                        diag.push("BCD file found".to_string());
                        esp_volume_path = Some(volume_path.clone());
                        // **现查**「真正的 Windows 启动项」，不再信赖 ESP 上那个
                        // `pe-exit-guid.txt`。
                        //
                        // 2026-10-29 实测的教训：那个文件是部署/测试时一次性写下的，
                        // **之后没有任何机制刷新它**（本机停在 2026-09-26 09:21），
                        // 于是 `default` 被写成一条早已删除的悬空 GUID；机器还能进
                        // Windows，全靠 bootmgr 回落到 `displayorder` 第一项——是运气。
                        //
                        // `{current}` 在这里不能用：PE 里它解析成 PE 条目本身，而不是
                        // 我们要回去的那个 Windows。所以枚举全部条目，按
                        // 「非 winpe + path 是 winload.efi + 该盘符上文件真的存在」
                        // 认出候选，多条时按启动菜单顺序取第一条。
                        let enum_all_file =
                            std::env::temp_dir().join("backuprestore-exit-enum-all.txt");
                        let enum_all_code = run_cmd_to_file(
                            &format!("bcdedit.exe /store \"{bcd_path}\" /enum all /v"),
                            Some(&enum_all_file),
                        );
                        let enum_all_text = std::fs::read(&enum_all_file)
                            .map(|bytes| decode_console_bytes(&bytes))
                            .unwrap_or_default();
                        diag.push(format!("enum all exit code: {enum_all_code}"));
                        let win_guid = crate::text_parsing::pick_windows_loader(
                            &enum_all_text,
                            |letter| {
                                let probe = format!("{letter}:\\Windows\\System32\\winload.efi");
                                GetFileAttributesW(wide(&probe).as_ptr()) != u32::MAX
                            },
                        );
                        for entry in crate::text_parsing::bcd_loader_entries(&enum_all_text) {
                            diag.push(format!(
                                "enum candidate: {} winpe={} device={:?} path={}",
                                entry.identifier, entry.is_winpe, entry.device_partition, entry.loader_path
                            ));
                        }
                        match &win_guid {
                            Some(guid) => diag.push(format!("live Windows loader: {guid}")),
                            None => {
                                diag.push("no Windows loader identified by live query".to_string())
                            }
                        }
                        if let Some(guid) = win_guid {
                            // Split the operations so a harmless
                            // `deletevalue ... Element not found` cannot hide a
                            // failed default restore behind the final exit code.
                            let set_command = format!(
                                "bcdedit.exe /store \"{bcd_path}\" /set {{bootmgr}} default \"{guid}\""
                            );
                            let set_code = run_cmd_to_file(&set_command, None);
                            diag.push(format!("set default exit code: {set_code}"));

                            // A stale bootsequence overrides default and would
                            // boot PE forever. Missing is already success, so
                            // verify the resulting enum instead of trusting the
                            // delete command's localized error code.
                            let delete_command = format!(
                                "bcdedit.exe /store \"{bcd_path}\" /deletevalue {{bootmgr}} bootsequence"
                            );
                            let delete_code = run_cmd_to_file(&delete_command, None);
                            diag.push(format!("delete bootsequence exit code: {delete_code}"));
                            let enum_command =
                                format!("bcdedit.exe /store \"{bcd_path}\" /enum {{bootmgr}}");
                            let enum_file = std::env::temp_dir().join("backuprestore-exit-bcd.txt");
                            let enum_code = run_cmd_to_file(&enum_command, Some(&enum_file));
                            let enum_text = std::fs::read(&enum_file)
                                .map(|bytes| decode_console_bytes(&bytes))
                                .unwrap_or_default();
                            let bootsequence_present =
                                crate::text_parsing::bcd_output_has_bootsequence(&enum_text);
                            diag.push(format!(
                                "verify bootsequence: enum exit={enum_code}, present={bootsequence_present}"
                            ));
                            if set_code.is_success()
                                && enum_code.is_success()
                                && !bootsequence_present
                            {
                                found_bcd = true;
                            } else {
                                diag.push("BCD restore verification failed".to_string());
                            }
                        } else {
                            // 认不出 Windows 条目时**绝不编一个 GUID 写进去**——那正是
                            // 上一版把 default 写成悬空值的原因。但也不能什么都不做：
                            // 若当前 default 指着一个 PE 条目（进 PE 就是靠它），直接
                            // 重启会又回 PE。所以分两种：
                            //   ① default 是 PE 条目 → 不重启，留在 PE 并弹诊断，让人自己
                            //      从启动菜单选 Windows（此时菜单里 Windows 一定在）；
                            //   ② default 不是 PE 条目 → 清掉 bootsequence 后照常重启，
                            //      行为与现状（bootmgr 回落 displayorder 第一项）一致。
                            let delete_command = format!(
                                "bcdedit.exe /store \"{bcd_path}\" /deletevalue {{bootmgr}} bootsequence"
                            );
                            let delete_code = run_cmd_to_file(&delete_command, None);
                            diag.push(format!(
                                "no target identified: delete bootsequence exit code: {delete_code}"
                            ));
                            let default_is_pe =
                                crate::text_parsing::bcd_boot_manager_default(&enum_all_text)
                                    .is_some_and(|default| {
                                        crate::text_parsing::bcd_loader_entries(&enum_all_text)
                                            .iter()
                                            .any(|entry| {
                                                entry.is_winpe
                                                    && entry
                                                        .identifier
                                                        .eq_ignore_ascii_case(&default)
                                            })
                                    });
                            if default_is_pe {
                                diag.push(
                                    "default still points at a PE entry: staying in PE instead of \
                                     rebooting back into it"
                                        .to_string(),
                                );
                            } else {
                                diag.push(
                                    "default is not a PE entry: rebooting without changing default"
                                        .to_string(),
                                );
                                reboot_without_target = true;
                            }
                        }
                    } else {
                        diag.push("BCD file NOT found at mount point".to_string());
                    }
                    // Write diagnostics while the ESP is still mounted (volume
                    // path also survives unmount, so this is belt and braces).
                    diag.push(format!("volume exit sequence done, success={found_bcd}"));
                    let write_failures = write_pe_exit_log(&diag, Some(&volume_path));
                    for f in &write_failures {
                        diag.push(f.clone());
                    }
                    let mut unmount = [letter, ':' as u16, '\\' as u16, 0];
                    DeleteVolumeMountPointW(unmount.as_mut_ptr());
                    if found_bcd || reboot_without_target {
                        break;
                    }
                } else {
                    diag.push("no free mount point".to_string());
                }
            }
        }
        if FindNextVolumeW(handle, volume.as_mut_ptr(), 512) == 0 {
            break;
        }
    }
    FindVolumeClose(handle);
    // Post-unmount best effort: volume path still works after unmount, plus
    // Q: and X: (PE RAM disk).
    let _ = write_pe_exit_log(&diag, esp_volume_path.as_deref());
    if found_bcd || reboot_without_target {
        diag.push(format!(
            "exit sequence done, rebooting (default restored={found_bcd}, reboot without target={reboot_without_target})"
        ));
        let _ = write_pe_exit_log(&diag, esp_volume_path.as_deref());
        pe_reboot(hwnd);
    } else {
        diag.push("exit sequence failed, staying in PE".to_string());
        let _ = write_pe_exit_log(&diag, esp_volume_path.as_deref());
        show_diag_dialog(hwnd, &diag);
        PE_EXITING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}
/// PE 桌面"备份/还原/第二系统"对话框的全局状态。pe_dialog() 在创建窗口
/// 前写入卷列表与默认值；对话框 WM_CREATE 读取建控件；OK 按钮把用户
/// 输入写回 PE_DLG_OUT 后销毁窗口。
static PE_DLG_VOLUMES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static PE_DLG_DEFAULTS: std::sync::Mutex<(String, String, bool, String)> =
    std::sync::Mutex::new((String::new(), String::new(), false, String::new()));
static PE_DLG_OUT: std::sync::Mutex<Option<(String, String, String)>> = std::sync::Mutex::new(None);

/// PE 内时间戳（YYYYMMDD-HHMMSS），用于自动镜像文件名。
fn pe_timestamp() -> String {
    let mut t: SystemTime = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// 枚举 PE 内所有带盘符卷（排除 X: PE RAM 盘与 S: ESP 挂载点），
/// 返回 "盘符|卷标|文件系统|容量" 列表供对话框下拉。
fn pe_list_volumes() -> Vec<String> {
    let mut list = Vec::new();
    unsafe {
        let mask = GetLogicalDrives();
        for i in 0..26 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let d = (b'A' + i) as char;
            if d == 'X' || d == 'S' {
                continue; // PE RAM 盘与 ESP 不参与选卷
            }
            let root: Vec<u16> = format!("{d}:\\").encode_utf16().chain(Some(0)).collect();
            let mut label = [0u16; 128];
            let mut fs = [0u16; 64];
            let mut serial = 0u32;
            let mut maxlen = 0u32;
            let mut flags = 0u32;
            let ok = GetVolumeInformationW(
                root.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                &mut serial,
                &mut maxlen,
                &mut flags,
                fs.as_mut_ptr(),
                fs.len() as u32,
            );
            let mut total: u64 = 0;
            let mut free: u64 = 0;
            let _ = GetDiskFreeSpaceExW(root.as_ptr(), null_mut(), &mut total, &mut free);
            let total_gb = total as f64 / (1024.0 * 1024.0 * 1024.0);
            let label_s = if ok != 0 {
                String::from_utf16_lossy(&label[..label.iter().position(|&c| c == 0).unwrap_or(0)])
            } else {
                String::new()
            };
            let fs_s = if ok != 0 {
                String::from_utf16_lossy(&fs[..fs.iter().position(|&c| c == 0).unwrap_or(0)])
            } else {
                String::new()
            };
            list.push(format!("{d}:|{label_s}|{fs_s}|{total_gb:.1}GB"));
        }
    }
    list
}

/// 默认镜像保存盘：第一个非系统、非 PE/ESP 的卷。
fn pe_default_image_drive(system: &str) -> String {
    for vol in pe_list_volumes() {
        let d = vol
            .split('|')
            .next()
            .unwrap_or("")
            .trim_end_matches(':')
            .to_string();
        let du = d.to_ascii_uppercase();
        if du.is_empty() || du == system.trim_end_matches(':').to_ascii_uppercase() {
            continue;
        }
        return du;
    }
    "H".to_string()
}

unsafe extern "system" fn window_proc_pe_dialog(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    if message == WM_CREATE {
        let (default_path, default_name, show_name, warning) = {
            let guard = PE_DLG_DEFAULTS.lock().unwrap();
            guard.clone()
        };
        let width = 440;
        // 标签 + 卷下拉
        create_control(
            hwnd,
            "STATIC",
            "目标卷（PE 盘符）：",
            0,
            16,
            14,
            300,
            20,
            ID_PE_DLG_LABEL1,
        );
        let combo = create_control(
            hwnd,
            "COMBOBOX",
            "",
            CBS_DROPDOWNLIST | WS_TABSTOP,
            16,
            34,
            width - 32,
            160,
            ID_PE_DLG_COMBO,
        );
        let volumes = {
            let guard = PE_DLG_VOLUMES.lock().unwrap();
            guard.clone()
        };
        for vol in &volumes {
            add_combo_item(combo, vol);
        }
        SendMessageW(combo, CB_SETCURSEL, 0, 0);
        SendMessageW(combo, CB_SETDROPPEDWIDTH, 300, 0);
        // 镜像路径
        create_control(
            hwnd,
            "STATIC",
            "镜像路径：",
            0,
            16,
            70,
            300,
            20,
            ID_PE_DLG_LABEL2,
        );
        let edit = create_control(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP | ES_AUTOHSCROLL,
            16,
            90,
            width - 32,
            24,
            ID_PE_DLG_EDIT,
        );
        if !default_path.is_empty() {
            let value = wide(&default_path);
            SetWindowTextW(edit, value.as_ptr());
        }
        // 菜单名称（仅第二系统显示）
        if show_name {
            create_control(
                hwnd,
                "STATIC",
                if get_text(hwnd).contains("还原系统") {
                    "镜像索引："
                } else {
                    "菜单名称："
                },
                0,
                16,
                128,
                300,
                20,
                ID_PE_DLG_LABEL3,
            );
            let name_edit = create_control(
                hwnd,
                "EDIT",
                "",
                WS_TABSTOP | ES_AUTOHSCROLL,
                16,
                148,
                width - 32,
                24,
                ID_PE_DLG_NAME,
            );
            if !default_name.is_empty() {
                let value = wide(&default_name);
                SetWindowTextW(name_edit, value.as_ptr());
            }
        }
        // 确定/取消
        create_control(
            hwnd,
            "BUTTON",
            "执行",
            WS_TABSTOP,
            16,
            236,
            120,
            32,
            ID_PE_DLG_OK,
        );
        create_control(
            hwnd,
            "BUTTON",
            "取消",
            WS_TABSTOP,
            152,
            236,
            120,
            32,
            ID_PE_DLG_CANCEL,
        );
        // 警告/说明（红色小字，位于按钮上方）
        let warn_y = if show_name { 196 } else { 180 };
        let warn = create_control(hwnd, "STATIC", "", 0, 16, warn_y, 420, 36, ID_PE_DLG_WARN);
        if !warning.is_empty() {
            let value = wide(&warning);
            SetWindowTextW(warn, value.as_ptr());
        }
        return 0;
    }
    if message == WM_COMMAND {
        let control_id = w_param & 0xffff;
        if control_id == ID_PE_DLG_OK || control_id == ID_PE_DLG_CANCEL {
            if control_id == ID_PE_DLG_OK {
                let combo = GetDlgItem(hwnd, ID_PE_DLG_COMBO as i32);
                let edit = GetDlgItem(hwnd, ID_PE_DLG_EDIT as i32);
                let name_edit = GetDlgItem(hwnd, ID_PE_DLG_NAME as i32);
                let drive = get_text(combo);
                let path = get_text(edit);
                let name = if name_edit.is_null() {
                    String::new()
                } else {
                    get_text(name_edit)
                };
                let mut guard = PE_DLG_OUT.lock().unwrap();
                *guard = Some((drive, path, name));
            } else {
                let mut guard = PE_DLG_OUT.lock().unwrap();
                *guard = None;
            }
            DestroyWindow(hwnd);
            return 0;
        }
    }
    DefWindowProcW(hwnd, message, w_param, l_param)
}

/// PE 内任务对话框：卷下拉 + 镜像路径 + （可选）菜单名称 + 执行/取消。
/// 返回 (卷项, 路径, 名称)；取消返回 None。
unsafe fn pe_dialog(
    owner: Hwnd,
    title: &str,
    show_name: bool,
    warning: &str,
    default_path: &str,
    default_name: &str,
) -> Option<(String, String, String)> {
    unsafe {
        let instance = GetModuleHandleW(null());
        let class_name = wide("BackupRestorePeDialog");
        let class = WndClassExW {
            cb_size: size_of::<WndClassExW>() as u32,
            style: 0,
            wnd_proc: Some(window_proc_pe_dialog),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: instance,
            h_icon: null_mut(),
            h_cursor: null_mut(),
            h_brush: (6usize) as HBrush,
            menu_name: null(),
            class_name: class_name.as_ptr(),
            h_icon_sm: null_mut(),
        };
        let _ = RegisterClassExW(&class); // 已注册则忽略
        // 准备全局状态
        {
            let mut vols = PE_DLG_VOLUMES.lock().unwrap();
            *vols = pe_list_volumes();
        }
        {
            let mut defs = PE_DLG_DEFAULTS.lock().unwrap();
            *defs = (
                default_path.to_string(),
                default_name.to_string(),
                show_name,
                warning.to_string(),
            );
        }
        {
            let mut out = PE_DLG_OUT.lock().unwrap();
            *out = None;
        }
        // ★ 2.6.79：PE 内对话框尺寸按 DPI 缩放，否则 200% 下它只有屏幕的 1/4 大。
        let width = crate::dpi::s(440);
        // 对话框总高需预留 WS_CAPTION 标题栏（约 28-30px），否则底部
        // 「执行/取消」按钮会超出客户区被裁剪，PE 内显示不完整。
        let height = if show_name {
            crate::dpi::s(324)
        } else {
            crate::dpi::s(312)
        };
        let x = ((GetSystemMetrics(SM_CXSCREEN) - width) / 2).max(0);
        let y = ((GetSystemMetrics(SM_CYSCREEN) - height) / 2).max(0);
        let wtitle = wide(title);
        let dlg = CreateWindowExW(
            0,
            class_name.as_ptr(),
            wtitle.as_ptr(),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            x,
            y,
            width,
            height,
            owner,
            null_mut(),
            instance,
            null_mut(),
        );
        if dlg.is_null() {
            return None;
        }
        // 模态消息循环：PeekMessageW 轮询，对话框销毁即返回
        let mut msg: Msg = std::mem::zeroed();
        loop {
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return None;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
                if IsWindow(dlg) == 0 {
                    let guard = PE_DLG_OUT.lock().unwrap();
                    return guard.clone();
                }
            }
            Sleep(20);
        }
    }
}

/// Convert captured console bytes through the OEM code page the child process
/// actually wrote with. Only reached when the bytes are neither UTF-16 nor
/// valid UTF-8, i.e. GBK/CP437 output from `diskpart`, `ipconfig`, `reg`,
/// `manage-bde` or `systeminfo` on a localized WinRE image.
fn sysinfo_oem_to_string(bytes: &[u8]) -> Option<String> {
    crate::text_parsing::decode_code_page(bytes, 1)
}

/// Decode one probe's bytes: UTF-16LE, then strict UTF-8, then the console OEM
/// code page. The final fallback keeps the old lossy behaviour so a probe never
/// disappears just because the conversion failed.
fn decode_console_bytes(bytes: &[u8]) -> String {
    match crate::text_parsing::plan_console_bytes(bytes) {
        crate::text_parsing::ConsoleBytes::Utf16 => decode_bcdedit_bytes(bytes),
        crate::text_parsing::ConsoleBytes::Utf8(text) => text,
        crate::text_parsing::ConsoleBytes::Oem => sysinfo_oem_to_string(bytes)
            .unwrap_or_else(|| String::from_utf8_lossy(bytes).to_string()),
    }
}

/// Read one probe's output file back as text, tolerating the mixed encodings
/// the console tools emit and collapsing a failed probe to an empty string so
/// the caller renders the visible "unavailable" note instead of raw garbage.
fn sysinfo_probe(command: &str, out_dir: &std::path::Path, name: &str) -> String {
    let out_file = out_dir.join(format!("sysinfo-{name}.txt"));
    let _ = std::fs::remove_file(&out_file);
    // Every probe is read-only. A non-zero exit is normal in WinRE (no
    // BitLocker, no display driver, …) so the exit code is not treated as
    // fatal; only genuinely empty output becomes "unavailable".
    let code = run_cmd_to_file_timeout(command, Some(&out_file), 20000);
    let bytes = match std::fs::read(&out_file) {
        Ok(bytes) => bytes,
        Err(_) => return String::new(),
    };
    let _ = std::fs::remove_file(&out_file);
    if matches!(code, CmdOutcome::SpawnFailed(_)) && bytes.is_empty() {
        return String::new();
    }
    let text = decode_console_bytes(&bytes);
    text.trim_end().to_string()
}

/// `reg query "<key>"` probe that keeps only the key's own value rows.
///
/// The bare listing also prints every subkey path; on
/// `...NT\CurrentVersion` that is dozens of lines of noise which pushed the
/// report past the edit control's text limit in the live WinRE run.
fn sysinfo_reg_probe(key: &str, out_dir: &std::path::Path, name: &str) -> String {
    let raw = sysinfo_probe(&crate::text_parsing::registry_query_all(key), out_dir, name);
    crate::text_parsing::reg_key_values(&raw)
}

/// Collect every read-only source for the information window.
fn collect_system_info_sources(
    out_dir: &std::path::Path,
) -> crate::text_parsing::SystemInfoSources {
    let systeminfo = sysinfo_probe("systeminfo", out_dir, "systeminfo");
    crate::text_parsing::SystemInfoSources {
        hostname: sysinfo_probe("hostname", out_dir, "hostname"),
        systeminfo: crate::text_parsing::systeminfo_cpu_memory(&systeminfo),
        os_registry: sysinfo_reg_probe(
            r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            out_dir,
            "os",
        ),
        bios_registry: sysinfo_reg_probe(r"HKLM\HARDWARE\DESCRIPTION\System\BIOS", out_dir, "bios"),
        disks: sysinfo_probe("echo list disk | diskpart", out_dir, "disks"),
        volumes: sysinfo_probe("echo list volume | diskpart", out_dir, "volumes"),
        display_devices: sysinfo_probe(
            r#"pnputil /enum-devices /class Display /connected"#,
            out_dir,
            "display",
        ),
        resolution: sysinfo_screen_resolution(),
        network: sysinfo_probe("ipconfig /all", out_dir, "network"),
        recovery: sysinfo_probe("reagentc /info", out_dir, "reagentc"),
        bitlocker: sysinfo_probe("manage-bde -status", out_dir, "bitlocker"),
        secure_boot: sysinfo_probe(
            r#"reg query "HKLM\SYSTEM\CurrentControlSet\Control\SecureBoot\State" /v UEFISecureBootEnabled"#,
            out_dir,
            "secureboot",
        ),
    }
}

/// Current desktop resolution straight from the Win32 metrics, so the report
/// has a display figure even when no driver-level probe answers.
fn sysinfo_screen_resolution() -> String {
    unsafe {
        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);
        if width <= 0 || height <= 0 {
            return String::new();
        }
        format!("{width}x{height}")
    }
}

/// `WinRE` when the running environment is the recovery environment, `WinPE`
/// for a plain preinstallation image, otherwise the installed Windows.
///
/// The footer used to be hard-coded to `WinPE`, which mislabelled every run
/// that arrived through `reagentc /boottore`. WinRE is the PE image plus
/// `\Windows\System32\Recovery\ReAgent.xml` / `winre.jpg`-style recovery
/// scaffolding, and its `SystemRoot` lives on the RAM disk (`X:`), so those
/// two facts together separate the three cases without running a command.
fn sysinfo_environment_label() -> &'static str {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| String::from(r"C:\Windows"));
    let on_ramdisk = system_root
        .get(..1)
        .is_some_and(|drive| drive.eq_ignore_ascii_case("X"));
    if !on_ramdisk {
        return "Windows";
    }
    let recovery_markers = [
        r"X:\Windows\System32\Recovery\ReAgent.xml",
        r"X:\sources\recovery\RecEnv.exe",
        r"X:\Windows\System32\RecEnv.exe",
    ];
    if recovery_markers
        .iter()
        .any(|marker| std::path::Path::new(marker).exists())
    {
        "WinRE"
    } else {
        "WinPE"
    }
}

/// Show the read-only software/hardware information window.
///
/// Collection happens before the window is created so the text box is filled
/// in one shot; every probe is independently optional.
unsafe fn pe_system_info_from_desktop(parent: Hwnd) {
    let out_dir = std::env::temp_dir();
    let sources = collect_system_info_sources(&out_dir);
    let collected_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let report = crate::text_parsing::format_system_info_report(
        &sources,
        sysinfo_environment_label(),
        std::env::consts::ARCH,
        &collected_at,
    );
    show_system_info_window(parent, &report);
}

/// Read-only information window: one multiline edit filling the client area,
/// both scroll bars, parent disabled while it is up.
unsafe fn show_system_info_window(parent: Hwnd, report: &str) {
    let instance = GetModuleHandleW(null());
    if instance.is_null() {
        show_message(parent, report, "软硬件信息", MB_OK);
        return;
    }
    let class_name = wide("BackupRestoreSysInfo");
    let class = WndClassExW {
        cb_size: size_of::<WndClassExW>() as u32,
        style: 0,
        wnd_proc: Some(system_info_window_proc),
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
    // A second click re-registers the same class; that returns 0 and is not an
    // error, so the result is deliberately not treated as fatal.
    RegisterClassExW(&class);

    let screen_width = GetSystemMetrics(SM_CXSCREEN).max(640);
    let screen_height = GetSystemMetrics(SM_CYSCREEN).max(480);
    let width = (screen_width * 3 / 4).clamp(560, 1100);
    let height = (screen_height * 3 / 4).clamp(420, 820);
    let title = wide("软硬件信息 - BackupRestore");
    let window = CreateWindowExW(
        0,
        class_name.as_ptr(),
        title.as_ptr(),
        WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_VISIBLE,
        (screen_width - width) / 2,
        (screen_height - height) / 2,
        width,
        height,
        parent,
        null_mut(),
        instance,
        null_mut(),
    );
    if window.is_null() {
        // Falling back to a message box keeps the button useful rather than
        // doing nothing at all if the window cannot be created.
        show_message(parent, report, "软硬件信息", MB_OK);
        return;
    }

    // The text is deliberately NOT passed to CreateWindowExW: a multiline EDIT
    // is created with the default 32,767 character limit, and the report
    // routinely exceeds it, which truncated the 按钮边界 section in the live
    // WinRE run. Create it empty, lift the limit, then fill it.
    let text_box = create_control(
        window,
        "EDIT",
        "",
        WS_TABSTOP
            | WS_BORDER
            | ES_MULTILINE
            | ES_AUTOVSCROLL
            | ES_AUTOHSCROLL
            | ES_READONLY
            | WS_VSCROLL
            | WS_HSCROLL,
        8,
        8,
        width - 32,
        height - 56,
        ID_PE_SYSINFO_TEXT,
    );
    // WinRE images do not necessarily ship Consolas, and the fallback lacks
    // Chinese glyphs. The stock GUI font is localized with the image, so the
    // report remains readable without assuming a font file exists.
    let font = gui_font();
    if !text_box.is_null() {
        if !font.is_null() {
            SendMessageW(text_box, WM_SETFONT, font as WParam, 1);
        }
        // 0 means "as large as the control can hold" for multiline edits.
        SendMessageW(text_box, EM_SETLIMITTEXT, 0, 0);
        // set_text normalizes lone LF to CRLF; the EDIT control renders a bare
        // LF as an invisible control character, which glued the report header
        // into one unreadable row.
        set_text(text_box, report);
    }
    ShowWindow(window, SW_SHOW);
    if !text_box.is_null() {
        // Focus immediately so PageDown/Ctrl+End/the wheel scroll the report
        // without first having to Tab into the text box.
        SetFocus(text_box);
    }

    // Modal by hand, following `pe_dialog`: poll with PeekMessageW and stop as
    // soon as the window is gone. A blocking GetMessageW is wrong here twice
    // over: it would hang once the last message is drained, and -- seen live
    // in WinRE -- a quit message left in the queue when `IsWindow` short
    // circuits the loop condition let the desktop's own loop consume it right
    // after, which ended the recovery desktop and returned to Windows just
    // because the user closed this read-only report with Alt+F4.
    EnableWindow(parent, 0);
    let mut message: Msg = std::mem::zeroed();
    // One message per iteration: an empty queue only means "idle", so checking
    // IsWindow after every drain is what ends the loop, and the WM_QUIT branch
    // cannot re-feed itself.
    loop {
        if PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) == 0 {
            if IsWindow(window) == 0 {
                break;
            }
            Sleep(20);
            continue;
        }
        if message.message == WM_QUIT {
            // The quit belongs to the desktop, not to this report window: hand
            // it back so a real exit request still reaches the desktop loop,
            // then stop being modal.
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
}

unsafe extern "system" fn system_info_window_proc(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    match message {
        WM_SIZE => {
            // Keep the text box filling the client area so resizing the window
            // actually gains reading space.
            let edit = GetDlgItem(hwnd, ID_PE_SYSINFO_TEXT as i32);
            if !edit.is_null() {
                let width = (l_param & 0xffff) as i32;
                let height = ((l_param >> 16) & 0xffff) as i32;
                // ★ 2.6.79：内边距随 DPI 缩放；width/height 来自父窗口，已是物理像素。
                MoveWindow(
                    edit,
                    crate::dpi::s(8),
                    crate::dpi::s(8),
                    width - crate::dpi::s(16),
                    height - crate::dpi::s(16),
                    1,
                );
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        // No WM_DESTROY handler on purpose: this window shares its thread with
        // the recovery desktop's message loop, so PostQuitMessage here would
        // take the desktop down with it. See `show_system_info_window`.
        _ => DefWindowProcW(hwnd, message, w_param, l_param),
    }
}

// ─────────────────── 任务与证据窗口（C 项，v2.6.66）───────────────────
//
// 独立模态窗口：列出 `<运行目录>\tasks` 下全部任务，可看详情、清工作树、开目录。
// **解析 / 排序 / 文案全部在 `crate::task_admin`**（无条件编译、本机可单测）；
// 这里只负责控件与消息。不要把解析写回本文件——`cfg(windows)` 里的逻辑在 mac 上
// 是零信息（编译器不打开），本项目的 GUI 缺陷多半就是这么漏掉的。

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
    let buttons = [
        (ID_TASK_REFRESH, if chinese { "刷新列表" } else { "Refresh" }, 238, 100),
        (ID_TASK_CLEANUP, if chinese { "清理工作树" } else { "Clean tree" }, 346, 150),
        (ID_TASK_OPEN_DIR, if chinese { "打开任务目录" } else { "Open dir" }, 504, 130),
        (ID_TASK_CLOSE, if chinese { "关闭" } else { "Close" }, 642, 66),
    ];
    for (id, text, x, width) in buttons {
        create_control(hwnd, "BUTTON", text, WS_TABSTOP, x, 392, width, 28, id);
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
unsafe fn show_task_admin(parent: Hwnd, language: Language, root: &Path) {
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

/// PE 桌面「备份系统」：PE 内直接 dism 捕获，不再跳主 GUI。
/// PE 三个界面入口共用类型化请求，不再拼接逐行破坏命令。
unsafe fn pe_backup_from_desktop(hwnd: Hwnd) {
    pe_run_interactive(hwnd, "backup");
}
unsafe fn pe_restore_from_desktop(hwnd: Hwnd) {
    pe_run_interactive(hwnd, "restore");
}
unsafe fn pe_secondary_from_desktop(hwnd: Hwnd) {
    pe_run_interactive(hwnd, "secondary");
}

unsafe fn pe_run_interactive(hwnd: Hwnd, operation: &str) {
    let (title, warning, default_name) = match operation {
        "backup" => (
            "备份系统 - BackupRestore",
            "捕获所选卷；已有镜像使用候选副本追加，不删除原件。",
            "",
        ),
        "secondary" => (
            "安装第二系统 - BackupRestore",
            "安装到指定目标；必须另外确认需要保留的主系统。",
            "Windows 备份",
        ),
        _ => (
            "还原系统 - BackupRestore",
            "仅接受系统镜像；先校验再格式化，失败即停止。",
            "1",
        ),
    };
    let default_wim = if operation == "backup" {
        format!(
            "{}:\\pe-bk-{}.wim",
            pe_default_image_drive(""),
            pe_timestamp()
        )
    } else {
        String::new()
    };
    let Some((drive_item, image, name)) = pe_dialog(
        hwnd,
        title,
        operation != "backup",
        warning,
        &default_wim,
        default_name,
    ) else {
        return;
    };
    let drive = drive_item
        .split('|')
        .next()
        .unwrap_or("")
        .trim_end_matches(':');
    let main = if operation == "secondary" {
        let Some((item, _, _)) = pe_dialog(
            hwnd,
            "选择需保留的主系统",
            false,
            "此处选择已有主系统卷，仅记录保护身份，不写入该卷。",
            "",
            "",
        ) else {
            return;
        };
        item.chars().next()
    } else {
        None
    };
    let index = if operation == "restore" {
        name.trim().parse::<u32>()
    } else {
        Ok(1)
    };
    let request = index
        .map_err(|_| crate::errc(ErrCode::ImageIndexNotPositive))
        .and_then(|index| {
            // overlay_no_acl=false：PE 桌面三入口恒格式化，无覆盖式组合（任务 #110）。
            crate::pe_operation::new_request(operation, drive, image.trim(), index, main, &name, false)
        });
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            show_message(
                hwnd,
                &error.to_string(),
                "参数校验失败",
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    };
    if operation != "backup" {
        let ask = show_message(
            hwnd,
            &format!(
                "确认将 {} 的索引 {} 还原到卷 {}:？\n此操作将格式化目标卷。输入或角色校验失败时不会格式化。",
                image.trim(),
                request.index,
                drive
            ),
            "还原确认",
            MB_YESNO | MB_ICONWARNING,
        );
        if ask != IDYES {
            return;
        }
    }
    EnableWindow(hwnd, 0);
    let result = crate::pe_operation::execute(&request);
    EnableWindow(hwnd, 1);
    match result {
        Ok(detail) => {
            let title = if operation == "backup" {
                "备份完成"
            } else {
                "还原完成"
            };
            show_message(hwnd, &detail, title, MB_OK | MB_ICONINFORMATION);
            if show_message(
                hwnd,
                "操作已完成，返回 Windows 吗？",
                "BackupRestore",
                MB_YESNO | MB_ICONQUESTION,
            ) == IDYES
            {
                pe_reboot(hwnd);
            }
        }
        Err(error) => {
            show_message(
                hwnd,
                &format!("操作未完成，已停止后续写入。\n{error}"),
                "操作失败",
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

unsafe extern "system" fn window_proc_pe(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    if message == WM_CREATE {
        // ★ 2.6.89：PE 恢复桌面也换成微软雅黑常规字重，与主界面统一。
        //   原来这里用 "Segoe UI"，字重 700 / 600 / 500 三级做层次。
        //   但实测本项目的 PE 镜像里 **Segoe UI 只登记了 Bold(700) / Light(300) /
        //   Semilight(350) / Italic，没有常规体 `segoeui.ttf`**，于是：
        //     700 → 命中真粗体；
        //     600 → 按最接近挑到 Bold（600 比 350 更靠近 700）；
        //     500 → 按最接近挑到 Semilight(350)。
        //   也就是三级字重全是错位的，代码写的和实际画的不是一回事。
        //   微软雅黑在本 PE 镜像里是齐的（`msyh.ttc` 已登记），换成它之后三档字号
        //   用同一个真常规体，层次改由**字号**（44 / 30 / 18）承担。
        //   证据见 `docs/20261030-1520-PE镜像字体核查-微软雅黑可用但SegoeUI缺常规体-v2.6.89-DS41F.md`。
        let face = wide("Microsoft YaHei");
        // ★ 2.6.79：恢复桌面上的大字号同样按 DPI 缩放，与布局坐标同比放大。
        let title_font = CreateFontW(
            -crate::dpi::s(44),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            134,
            0,
            0,
            0,
            0,
            face.as_ptr(),
        );
        let card_font = CreateFontW(
            -crate::dpi::s(30),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            134,
            0,
            0,
            0,
            0,
            face.as_ptr(),
        );
        let bar_font = CreateFontW(
            -crate::dpi::s(18),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            134,
            0,
            0,
            0,
            0,
            face.as_ptr(),
        );
        let width = GetSystemMetrics(SM_CXSCREEN).max(640);
        let height = GetSystemMetrics(SM_CYSCREEN).max(480);

        let title = create_control(
            hwnd,
            "STATIC",
            &format!("BackupRestore 恢复桌面  v{PROGRAM_VERSION}"),
            SS_CENTER,
            0,
            36,
            width,
            64,
            ID_PE_TITLE,
        );
        let card_width = 280;
        let card_height = 108;
        let gap = 36;
        let grid_width = card_width * 3 + gap * 2;
        let start_x = ((width - grid_width) / 2).max(0);
        // 七个入口排成 3 列 3 行（末行只占 1 个），行高按 3 行计算才不会
        // 让最后一行压到底栏上。
        let start_y = ((height - (card_height * 3 + gap * 2 + 150)) / 2).max(16) + 24;
        let cards: [(usize, &str, i32, i32); 7] = [
            (ID_PE_BACKUP, "备份系统", 0, 0),
            (ID_PE_RESTORE, "还原系统", 1, 0),
            (ID_PE_SECONDARY, "安装第二系统", 2, 0),
            (ID_PE_CMD, "命令提示符", 0, 1),
            (ID_PE_EXIT, "返回 Windows", 1, 1),
            // 「重启」已合并进「返回 Windows」，此位改为打开完整主程序 GUI
            (ID_PE_MAIN_GUI, "打开完整程序", 2, 1),
            (ID_PE_SYSINFO, "软硬件信息", 0, 2),
        ];
        let mut card_controls = Vec::with_capacity(cards.len());
        for (id, text, column, row) in cards {
            let x = start_x + column * (card_width + gap);
            let y = start_y + row * (card_height + gap);
            card_controls.push(create_control(
                hwnd,
                "BUTTON",
                text,
                WS_TABSTOP,
                x,
                y,
                card_width,
                card_height,
                id,
            ));
        }
        let bar_y = height - 44;
        let version = create_control(
            hwnd,
            "STATIC",
            // 底栏原先固定写死 WinPE，经 reagentc /boottore 进入 WinRE 时标签不实；
            // 改为实际探测到的环境。
            &format!(
                "BackupRestore v{PROGRAM_VERSION}  |  {}",
                sysinfo_environment_label()
            ),
            0,
            16,
            bar_y,
            360,
            30,
            ID_PE_VERSION,
        );
        let clock = create_control(
            hwnd,
            "STATIC",
            "--:--:--",
            SS_CENTER,
            width - 220,
            bar_y,
            200,
            30,
            ID_PE_CLOCK,
        );

        SendMessageW(title, WM_SETFONT, title_font as WParam, 1);
        for control in &card_controls {
            SendMessageW(*control, WM_SETFONT, card_font as WParam, 1);
        }
        SendMessageW(version, WM_SETFONT, bar_font as WParam, 1);
        SendMessageW(clock, WM_SETFONT, bar_font as WParam, 1);

        // PE 桌面包卡按钮的气泡提示：鼠标悬停显示各按钮用途说明。
        let tooltip = CreateWindowExW(
            WS_EX_TOPMOST,
            wide("tooltips_class32").as_ptr(),
            null(),
            WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
            0,
            0,
            0,
            0,
            hwnd,
            null_mut(),
            null_mut(),
            null_mut(),
        );
        if !tooltip.is_null() {
            for &(id, text) in &[
                (
                    ID_PE_BACKUP,
                    "备份系统：把选中的源分区制作成 WIM 镜像（离线 DISM 捕获），完成后返回 Windows。",
                ),
                (
                    ID_PE_RESTORE,
                    "还原系统：格式化目标分区并把镜像写回、重建引导（危险操作，请先确认目标分区与重要数据）。",
                ),
                (
                    ID_PE_SECONDARY,
                    "安装第二系统：把镜像部署到另一个分区并新增启动项，保留现有 Windows。",
                ),
                (
                    ID_PE_CMD,
                    "命令提示符：打开 PE 环境里的 cmd，可手动执行维护命令。",
                ),
                (
                    ID_PE_EXIT,
                    "返回 Windows：恢复引导启动到当前 Windows 并自动重启。",
                ),
                (
                    ID_PE_MAIN_GUI,
                    "打开完整程序：启动完整的多页签界面（与 Windows 下相同），可执行更多操作。",
                ),
                (
                    ID_PE_SYSINFO,
                    "软硬件信息：只读采集本机操作系统、CPU/内存、主板固件、显示、存储、网络与恢复安全状态，不修改任何配置。",
                ),
            ] {
                let control = GetDlgItem(hwnd, id as i32);
                add_tooltip(tooltip, hwnd, control, text);
            }
        }

        let state = Box::new(PeDesktopState {
            clock,
            fonts: [title_font, card_font, bar_font],
            tooltip,
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        SetTimer(hwnd, PE_TIMER_ID, 1000, None);
        return 0;
    }
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PeDesktopState;
    if !state_ptr.is_null() {
        let state = &mut *state_ptr;
        if message == WM_COMMAND {
            let control_id = w_param & 0xffff;
            match control_id {
                ID_PE_BACKUP => {
                    // PE 内直接执行备份（对话框确认后 dism 捕获，不再跳主 GUI）
                    pe_backup_from_desktop(hwnd);
                    return 0;
                }
                ID_PE_RESTORE => {
                    // PE 内直接执行还原（格式化+Apply+BCDBoot）
                    pe_restore_from_desktop(hwnd);
                    return 0;
                }
                ID_PE_SECONDARY => {
                    // PE 内直接安装第二系统（Apply+BCD 追加）
                    pe_secondary_from_desktop(hwnd);
                    return 0;
                }
                ID_PE_EXIT => {
                    // Return to the main Windows installation: find the EFI
                    // system partition that carries BCD, mount it, restore the
                    // boot manager default to {current}, then reboot. Without
                    // this step a PE set as the default would loop back into
                    // PE forever.
                    exit_pe_to_windows(hwnd);
                    return 0;
                }
                ID_PE_SYSINFO => {
                    // 只读采集并展示软硬件信息，不修改系统、不启动任务。
                    pe_system_info_from_desktop(hwnd);
                    return 0;
                }
                ID_PE_CMD => {
                    let cmd = wide("cmd.exe");
                    ShellExecuteW(hwnd, null(), cmd.as_ptr(), null(), null(), SW_SHOW);
                    return 0;
                }
                ID_PE_MAIN_GUI => {
                    // 打开完整主程序 GUI（多 tab 界面，与 Windows 下相同）。
                    // 注意：当前 exe 名为 Recovery.exe，不满足 main.rs
                    // should_launch_gui 的 "BackupRestore" 检查，无参数启动
                    // 不会进 GUI；必须带 --tab 1（备份页）直接进入。
                    let executable = std::env::current_exe().unwrap_or_else(|_| {
                        std::path::PathBuf::from("X:\\Windows\\System32\\Recovery.exe")
                    });
                    let exe_wide = wide(&executable.to_string_lossy());
                    let argument = wide("--tab 1");
                    ShellExecuteW(
                        hwnd,
                        null(),
                        exe_wide.as_ptr(),
                        argument.as_ptr(),
                        null(),
                        SW_SHOW,
                    );
                    return 0;
                }
                _ => {}
            }
        }
        if message == WM_TIMER && w_param == PE_TIMER_ID {
            update_pe_clock(state);
            return 0;
        }
        if message == WM_CTLCOLORSTATIC || message == WM_CTLCOLORBTN {
            let hdc = l_param as Handle;
            SetBkMode(hdc, 1);
            SetTextColor(hdc, PE_TITLE_TEXT);
            return PE_BACKGROUND as LResult;
        }
        if message == WM_DESTROY {
            KillTimer(hwnd, PE_TIMER_ID);
            if !state.tooltip.is_null() {
                DestroyWindow(state.tooltip);
            }
            for font in state.fonts {
                if !font.is_null() {
                    DeleteObject(font);
                }
            }
            drop(Box::from_raw(state_ptr));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            PostQuitMessage(0);
            return 0;
        }
    }
    DefWindowProcW(hwnd, message, w_param, l_param)
}

/// 自动请求只领取一次；全部成功才返回重启许可，失败保留证据。
/// 旧文本任务保留原件，绝不解释为可重放命令。
fn pe_task_execute() -> bool {
    let result = crate::pe_operation::execute_pending();
    match result {
        Ok(completed) => completed,
        Err(error) => {
            unsafe {
                show_message(
                    null_mut(),
                    &format!(
                        "自动任务未执行或未完成：{error}\n已保留任务与状态，请检查后重新安排。"
                    ),
                    "PE 自动任务已停止",
                    MB_OK | MB_ICONERROR,
                );
            }
            false
        }
    }
}

pub unsafe fn run_pe_desktop() -> Result<Option<usize>, super::TaskError> {
    unsafe {
        let common_controls = InitCommonControlsEx {
            size: size_of::<InitCommonControlsEx>() as u32,
            classes: ICC_WIN95_CLASSES,
        };
        InitCommonControlsEx(&common_controls);
        if !is_elevated() {
            relaunch_elevated()?;
            return Ok(None);
        }
        close_previous_gui_windows();
        // 领取类型化请求；失败停留桌面，无请求则等待用户操作。
        let auto_reboot = pe_task_execute();
        if auto_reboot {
            pe_reboot_with_fallback();
            return Ok(None);
        }
        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            return Err(super::err("GetModuleHandleW failed"));
        }
        let background = CreateSolidBrush(PE_BACKGROUND);
        let class_name = wide("BackupRestorePeDesktop");
        let class = WndClassExW {
            cb_size: size_of::<WndClassExW>() as u32,
            style: 0,
            wnd_proc: Some(window_proc_pe),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: instance,
            h_icon: null_mut(),
            h_cursor: null_mut(),
            h_brush: background,
            menu_name: null(),
            class_name: class_name.as_ptr(),
            h_icon_sm: null_mut(),
        };
        if RegisterClassExW(&class) == 0 {
            return Err(super::err("RegisterClassExW failed (PE desktop)"));
        }
        PE_EXIT_TAB.store(0, std::sync::atomic::Ordering::SeqCst);
        let width = GetSystemMetrics(SM_CXSCREEN).max(640);
        let height = GetSystemMetrics(SM_CYSCREEN).max(480);
        let title = wide(&format!(
            "BackupRestore - PE Recovery Desktop v{PROGRAM_VERSION}"
        ));
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            width,
            height,
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        );
        if window.is_null() {
            // 【2026-10-17 第二轮核查，逃生 C】PE 是 **shell-free** 的
            // （`winpeshl.ini` 直接拉起本程序）：建窗失败后直接 `return Err`
            // 的话，屏幕上什么都没有、没有任务栏也没有命令行，而一次性启动项
            // 仍指向 PE —— 硬断电重启又会再进 PE，真正的死循环。
            // 这里先开一个最大化的命令提示符当手动出口，再让用户选去留。
            let _ = super::append_log(
                &std::path::PathBuf::from(r"X:\BackupRestore-pe-desktop.log"),
                "PE desktop: CreateWindowExW failed; offering a console escape hatch",
            );
            let _ = super::launch_background_console();
            let answer = message_box_standalone(
                "PE 恢复桌面创建窗口失败。\n\
                 已打开一个**最大化**的命令提示符窗口，可用它手动处理\n\
                 （例如 bcdedit /deletevalue {bootmgr} bootsequence、wpeutil reboot）。\n\n\
                 是 = 立即重启（wpeutil reboot）\n\
                 否 = 留在 PE，用命令提示符手动处理\n\n\
                 PE desktop failed to create its window. A maximized console has been\n\
                 opened for manual recovery. Yes = reboot now; No = stay in PE.",
                "PE 桌面启动失败 / PE desktop failed to start",
                MB_YESNO | MB_ICONERROR,
            );
            if answer == IDYES {
                pe_reboot_with_fallback();
            }
            return Err(super::err("CreateWindowExW failed (PE desktop)"));
        }
        ShowWindow(window, SW_SHOW);
        // 旧 pe-click 文本未绑定目标身份，不再自动授权破坏性操作。
        let mut message = Msg {
            hwnd: null_mut(),
            message: 0,
            w_param: 0,
            l_param: 0,
            time: 0,
            point: Point { x: 0, y: 0 },
        };
        loop {
            let result = GetMessageW(&mut message, null_mut(), 0, 0);
            if result <= 0 {
                break;
            }
            // PE 桌面按钮同样支持 Tab 遍历 / 回车触发 / 方向键。
            if IsDialogMessageW(window, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let exit_tab = match PE_EXIT_TAB.load(std::sync::atomic::Ordering::SeqCst) {
            1 => Some(1),
            2 => Some(2),
            3 => Some(3),
            _ => None,
        };
        Ok(exit_tab)
    }
}

/// 用户对待续跑任务的选择。
///
/// 为什么要有它（2026-10-17 第二轮核查，逃生 A）：此前 GUI 一启动就把
/// `resume_pending_boot_task` 的返回值当开关用——有任务就 `shutdown /r`，
/// **窗口都没创建完机器就重启了**，界面上也没有任何叫停手段。现在先问。
enum PendingResumeChoice {
    /// 立即重启继续这个任务
    Reboot,
    /// 本次不重启，任务保留
    Defer,
}

/// 启动早期（还没有主窗口）弹一个框问用户怎么处理待续跑任务。
///
/// 三选一而不是「是/否」：这正是缺口所在——旧逻辑里用户**没有第三个选项**，
/// 想停下这台机器只能去手敲 `abandon-task`，而它在目标卷已擦除后还会拒绝。
unsafe fn ask_pending_resume(pending: &super::PendingResume) -> PendingResumeChoice {
    let text = format!(
        "检测到上次没跑完的还原任务：\n\n\
         任务：{}\n\
         操作：{:?}\n\
         停在：{:?}\n\n\
         是   = 立即重启，继续这个任务\n\
         否 / 取消 / 直接关窗 = 本次不重启（任务与启动项都保留，以后再处理）\n\
         想放弃这个任务：进主窗口，用顶栏「取消待重启任务」\n\n\
         Detected an unfinished recovery task.\n\
         Yes = reboot now and continue it; No / Cancel / closing the box = not this time \
         (task and boot entry are kept). To abandon it, use the top-bar \
         \"Cancel pending reboot task...\" in the main window.",
        pending.task_id, pending.operation, pending.stage
    );
    let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "待续跑的还原任务 / Unfinished recovery task"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let answer = MessageBoxW(
        null_mut(),
        text.as_ptr(),
        title.as_ptr(),
        MB_YESNOCANCEL | MB_ICONWARNING | MB_SETFOREGROUND,
    );
    match answer {
        IDYES => PendingResumeChoice::Reboot,
        // 「否」「取消」与**直接关窗**都按「本次不重启」处理：
        // 不重启永远是最保守的那个分支 —— 绝不能把「没看清楚就关了窗口」
        // 当成同意重启，更不能当成同意放弃。放弃要走主窗口顶栏那个按钮，
        // 那里有独立的二次确认。
        _ => PendingResumeChoice::Defer,
    }
}

/// 没有主窗口时用的消息框（启动早期 / PE 桌面建窗失败时）。
/// 返回 `MessageBoxW` 的返回值，调用方据此判断用户选了哪个按钮。
unsafe fn message_box_standalone(text: &str, title: &str, flags: u32) -> i32 {
    let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    MessageBoxW(
        null_mut(),
        text.as_ptr(),
        title.as_ptr(),
        flags | MB_SETFOREGROUND,
    )
}

/// 顶栏「取消待重启任务」：GUI 里唯一一个「让机器别再重启」的出口。
///
/// 复用 `abandon-task` 那条已经实机验收过的收口路径，不另写清理逻辑；
/// 目标卷已擦除时它会拒绝，那正是既定的安全边界，如实转达给用户。
unsafe fn cancel_pending_task(state: &mut State) {
    let language = selected_language(state);
    let pending = match super::find_pending_boot_task() {
        Ok(pending) => pending,
        Err(error) => {
            append_gui_log(state, &format!("cancel_pending: scan failed: {error}"));
            show_message(
                state.root,
                &format!("扫描任务失败：{error}"),
                if language == Language::English {
                    "Scan failed"
                } else {
                    "扫描失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    };
    let Some(pending) = pending else {
        append_gui_log(state, "cancel_pending: no pending boot task");
        show_message(
            state.root,
            if language == Language::English {
                "No unfinished recovery task is waiting for a reboot."
            } else {
                "当前没有等待重启的未完成任务。"
            },
            if language == Language::English {
                "Nothing pending"
            } else {
                "没有待重启任务"
            },
            MB_OK | MB_ICONINFORMATION,
        );
        return;
    };
    let prompt = format!(
        "任务：{}\n操作：{:?}\n停在：{:?}\n\n\
         放弃这个任务并清理？（清掉它的一次性启动项、按终态收口，任务目录与日志等证据保留）\n\n\
         Abandon this task and clean it up? (one-shot boot entry cleared, task marked terminal, logs kept)",
        pending.task_id, pending.operation, pending.stage
    );
    let answer = show_message(
        state.root,
        &prompt,
        if language == Language::English {
            "Abandon pending task?"
        } else {
            "放弃待重启任务？"
        },
        MB_YESNO | MB_ICONWARNING | MB_SETFOREGROUND,
    );
    if answer != IDYES {
        append_gui_log(state, "cancel_pending: user declined; nothing changed");
        return;
    }
    match super::abandon_pending_task(&pending) {
        Ok(()) => {
            append_gui_log(
                state,
                &format!("cancel_pending: abandoned task {}", pending.task_id),
            );
            show_message(
                state.root,
                if language == Language::English {
                    "Task abandoned and cleaned up. This machine will no longer reboot into the recovery environment for it."
                } else {
                    "已放弃并清理该任务。本机不会再为它重启进恢复环境。"
                },
                if language == Language::English {
                    "Done"
                } else {
                    "已完成"
                },
                MB_OK | MB_ICONINFORMATION,
            );
        }
        Err(error) => {
            append_gui_log(
                state,
                &format!("cancel_pending: abandon failed for {}: {error}", pending.task_id),
            );
            show_message(
                state.root,
                &format!(
                    "放弃失败：{error}\n\n（目标卷已擦除的任务按设计不予清理；\
                     可查看该任务的 prepare.log，或用 resume 命令继续。）"
                ),
                if language == Language::English {
                    "Abandon failed"
                } else {
                    "放弃失败"
                },
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

pub fn run() -> Result<(), super::TaskError> {
    unsafe {
        let common_controls = InitCommonControlsEx {
            size: size_of::<InitCommonControlsEx>() as u32,
            classes: ICC_WIN95_CLASSES,
        };
        InitCommonControlsEx(&common_controls);
        if !is_elevated() {
            return relaunch_elevated();
        }
        // 【2026-10-18 新增 B5】先把「注定跑不下去」的僵尸任务收口，再问续跑。
        // 判据极窄（非终态 + 破坏性边界之前 + 没有一次性启动项），只在不可能
        // 误伤还能续跑的任务时才动手；失败只记日志，不阻塞启动。
        match super::collect_orphan_tasks() {
            Ok(0) => {}
            Ok(count) => {
                let _ = crate::append_log(
                    &crate::log_root::logs_dir().join("gui.log"),
                    &format!("orphan task guard: collected {count} unresumable task(s)"),
                );
            }
            Err(error) => {
                let _ = crate::append_log(
                    &crate::log_root::logs_dir().join("gui.log"),
                    &format!("orphan task guard failed: {error}"),
                );
            }
        }
        // 【2026-10-17 第二轮核查，逃生 A】待续跑任务**先问再做**：
        // 旧代码在这里一路做到 `shutdown /r`，用户只是想打开程序、
        // 主窗口都还没创建就被重启了，而且界面上没有任何叫停手段。
        match super::find_pending_boot_task()? {
            None => {}
            Some(pending) => match ask_pending_resume(&pending) {
                PendingResumeChoice::Reboot => match super::resume_pending_now(&pending) {
                    // 重启已发起：这台机器马上就要重启，GUI 不必再起来。
                    Ok(true) => return Ok(()),
                    // 被闸门挡住（per-stage / 跨阶段总闸）：不重启，继续开窗口，
                    // 用户还能用顶栏「取消待重启任务」处置它。
                    Ok(false) => {}
                    Err(error) => {
                        message_box_standalone(
                            &format!("重启续跑失败：{error}"),
                            "重启失败 / Reboot failed",
                            MB_OK | MB_ICONERROR,
                        );
                    }
                },
                PendingResumeChoice::Defer => {
                    let _ = super::defer_pending_task(&pending);
                }
            },
        }
        close_previous_gui_windows();
        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            return Err(super::err("GetModuleHandleW failed"));
        }
        let class_name = wide("BackupRestoreNativeGui");
        let class = WndClassExW {
            cb_size: size_of::<WndClassExW>() as u32,
            style: 0,
            wnd_proc: Some(window_proc),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: instance,
            h_icon: null_mut(),
            h_cursor: null_mut(),
            h_brush: (6usize) as HBrush,
            menu_name: null(),
            class_name: class_name.as_ptr(),
            h_icon_sm: null_mut(),
        };
        if RegisterClassExW(&class) == 0 {
            return Err(super::err("RegisterClassExW failed"));
        }
        let title = wide(&format!("BackupRestore - Rust GUI v{PROGRAM_VERSION}"));
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            // ★ 2.6.79：初始尺寸按 DPI 缩放（随后会最大化，但还原窗口时也该是对的）。
            crate::dpi::s(120),
            crate::dpi::s(80),
            crate::dpi::s(1020),
            crate::dpi::s(760),
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        );
        if window.is_null() {
            return Err(super::err("CreateWindowExW failed"));
        }
        ShowWindow(window, SW_MAXIMIZE);
        let mut message = Msg {
            hwnd: null_mut(),
            message: 0,
            w_param: 0,
            l_param: 0,
            time: 0,
            point: Point { x: 0, y: 0 },
        };
        loop {
            let result = GetMessageW(&mut message, null_mut(), 0, 0);
            if result <= 0 {
                break;
            }
            // 主界面键盘导航：Tab 遍历控件、回车触发默认按钮（创建任务）、
            // 方向键切换操作模式单选、Ctrl+组合快捷键在 WM_KEYDOWN 处理。
            // F5 刷新：IsDialogMessage 会消费 F5（对话框键盘处理吞掉该键），
            // 导致窗口过程收不到 WM_KEYDOWN，故必须在它之前拦截。
            if message.message == WM_KEYDOWN && ((message.w_param & 0xFFFF) as u32) == 116 {
                let ctrl_down = ((GetAsyncKeyState(VK_CONTROL as i32) as u16) & 0x8000) != 0;
                if !ctrl_down {
                    PostMessageW(window, WM_COMMAND, ID_REFRESH, 0);
                    continue;
                }
            }
            if IsDialogMessageW(window, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod p4_precise_drive_tests {
    use super::*;

    fn drive(letter: &str, partition_type_guid: &str) -> DriveInfo {
        DriveInfo {
            letter: letter.to_string(),
            label: String::new(),
            filesystem: "NTFS".to_string(),
            size_bytes: Some(1024),
            free_bytes: Some(1024),
            volume_guid: String::new(),
            disk_number: Some(0),
            partition_number: Some(1),
            partition_type_guid: partition_type_guid.to_string(),
            has_windows_installation: false,
        }
    }

    /// 2026-10-16 P4：盘符判定不再写死 'C'/'S'。ESP 只认分区类型 GUID，
    /// 与盘符无关——「ESP 不在 S」和「S 是数据盘」都不能误判。
    #[test]
    fn esp_is_judged_by_partition_type_guid_not_letter() {
        let esp = Some(drive("S", crate::windows_prepare::EFI_TYPE));
        assert!(is_efi_system_partition(esp.as_ref()));
        // 同样在 S:，但分区类型是基本数据 → 不是 ESP，不能误拦。
        let data_on_s = Some(drive("S", "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}"));
        assert!(!is_efi_system_partition(data_on_s.as_ref()));
        // ESP 换了字母（M:）照样认得出；GUID 大小写不敏感。
        let esp_on_m = Some(drive("M", &crate::windows_prepare::EFI_TYPE.to_lowercase()));
        assert!(is_efi_system_partition(esp_on_m.as_ref()));
        // 卷信息缺失（枚举失败）时判「不是」，不猜。
        assert!(!is_efi_system_partition(None));
    }

    /// 程序所在卷判定：与可执行文件目录首字母比较，大小写不敏感；
    /// 取不到程序路径时判「不是」——既不能误判「是」，也不假设成 C。
    #[test]
    fn program_drive_compares_executable_directory_prefix() {
        assert!(is_program_drive('H', Path::new("H:\\brwork")));
        assert!(is_program_drive('h', Path::new("H:\\brwork")));
        assert!(!is_program_drive('C', Path::new("H:\\brwork")));
        assert!(!is_program_drive('H', Path::new("")));
    }

    /// 系统盘判定只信 `%SystemDrive%`：本测试不改环境变量，
    /// 而是按同一变量独立解析期望值后比对，锁住「解析逻辑与变量一致」，
    /// 且绝不出现「取不到就退回 C」的写死行为。
    #[test]
    fn system_drive_letter_parses_environment_variable() {
        let expected = std::env::var("SystemDrive")
            .ok()
            .and_then(|value| {
                value
                    .trim_end_matches('\\')
                    .trim_end_matches(':')
                    .chars()
                    .next()
            })
            .map(|letter| letter.to_ascii_uppercase());
        assert_eq!(system_drive_letter(), expected);
    }
}
