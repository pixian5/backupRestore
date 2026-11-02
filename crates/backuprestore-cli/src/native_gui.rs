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
/// WM_USER(0x0400) + 24：给气泡提示设最大宽度。
/// ★ 2.6.98：**不设这个就没有折行** —— Win32 气泡默认把整段文案摊成一条横贯屏幕的细条，
///   用户反馈的「只能显示一行」就是这个原因。单位是像素，调用处按 DPI 缩放。
const TTM_SETMAXTIPWIDTH: u32 = 0x0418;
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
    fn GetDlgCtrlID(hwnd: Hwnd) -> i32;
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
    /// 2.6.90：查 GDI **实际选中**的字体族名。`CreateFontW` 找不到字体会静默
    /// 挑一个替代品，句柄照样有效——只有这个调用能把"到底用的是哪个字体"问出来。
    fn GetTextFaceW(hdc: Handle, count: i32, name: *mut u16) -> i32;
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

pub(crate) struct State {
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

/// `EnumChildWindows` 回调：把子窗口句柄收集进 `l_param` 指向的 Vec。
unsafe extern "system" fn collect_child_hwnd(hwnd: Hwnd, l_param: LParam) -> i32 {
    let out = &mut *(l_param as *mut Vec<Hwnd>);
    out.push(hwnd);
    1
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
    // ★ 2.6.98：气泡提示默认**不折行**。不设最大宽度时，长文案会摊成一条横贯屏幕的
    //   细条（用户反馈「只能显示一行」）。这里给提示窗设上限宽度，comctl32 才会按词折行。
    //   420 基准像素 ≈ 20 个中文字/宽度适中；按 DPI 缩放（200% 下 840 物理像素）。
    SendMessageW(tooltip, TTM_SETMAXTIPWIDTH, 0, crate::dpi::s(420) as isize);
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
        // ★ 2.6.98：这两个之前漏挂了。`image_admin` 的文案早就写好了却没接上，
        //   「安装 PE」页的启动项名称输入框则连文案都没有。
        (
            GetDlgItem(state.root, ID_IMAGE_ADMIN as i32),
            "image_admin",
        ),
        (
            GetDlgItem(state.root, ID_PE_NAME_EDIT as i32),
            "pe_name_edit",
        ),
    ];
    let mut added = 0usize;
    let mut missing = 0usize;
    let mut attached: Vec<Hwnd> = Vec::new();
    for (control, key) in controls {
        if control.is_null() {
            missing += 1;
            continue;
        }
        let text = tooltip_text(language, key);
        add_tooltip(tooltip, state.root, control, text);
        attached.push(control);
        added += 1;
    }
    let tool_count = SendMessageW(tooltip, TTM_GETTOOLCOUNT, 0, 0) as usize;
    append_gui_log(
        state,
        &format!(
            "tooltips installed: added={added} missing={missing} tool_count={tool_count} balloon=true wrap={} hwnd={}",
            crate::dpi::s(420),
            tooltip as usize
        ),
    );
    // ★ 2.6.98：**提示覆盖审计**。枚举所有可交互子控件（按钮 / 编辑框 / 下拉框），
    //   把没挂提示的按 `id:类名` 列进日志。用户问「还有哪些没提示」时，这个列表就是答案，
    //   不用人肉数控件——而且每次启动都会重算，新增控件漏挂会立刻现形。
    let mut children: Vec<Hwnd> = Vec::new();
    EnumChildWindows(
        state.root,
        collect_child_hwnd,
        &mut children as *mut Vec<Hwnd> as LParam,
    );
    let mut untipped: Vec<String> = Vec::new();
    let (mut n_button, mut n_edit, mut n_combo) = (0usize, 0usize, 0usize);
    for child in &children {
        let mut buf = [0u16; 64];
        let n = GetClassNameW(*child, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        let class = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        match class.as_str() {
            "Button" => n_button += 1,
            "Edit" => n_edit += 1,
            "ComboBox" => n_combo += 1,
            _ => continue,
        }
        if attached.contains(child) {
            continue;
        }
        untipped.push(format!("{}:{class}", GetDlgCtrlID(*child)));
    }
    // ★ 关键：把**候选数量**一起打出来。只报 untipped=0 是没有说服力的——
    //   万一枚举根本没找到按钮（类名比较写错、控件还没创建），也会"成功报 0"。
    //   候选总数 > 0 且 untipped=0 才算真的全覆盖。
    append_gui_log(
        state,
        &format!(
            "tooltip audit: children={} candidates(button={} edit={} combo={}) total_candidates={} tipped={} untipped={} [{}]",
            children.len(),
            n_button,
            n_edit,
            n_combo,
            n_button + n_edit + n_combo,
            attached.len(),
            untipped.len(),
            untipped.join(" ")
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
pub(crate) struct DriveInfo {
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
/// 界面字体的字重（400 = 常规）。2.6.88 起从 700 改成 400；单独提成常量是为了
/// 让日志里报的字重和 `CreateFontW` 实际传的值不可能走岔。
const GUI_FONT_WEIGHT: i32 = 400;

/// GDI 本次实际选中的字体族名只记一次。
static FONT_RESOLVED_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 把 GDI **实际选中**的字体族名写进 GUI 日志（每个进程一次）。
///
/// 为什么需要这一行：`CreateFontW` 找不到请求的字体时**不会失败**，会静默挑一个最接近
/// 的替代品，返回的句柄照样有效。于是"代码里写着微软雅黑"和"屏幕上真的是微软雅黑"
/// 是两件事，而且后者没有任何痕迹可查。
/// 2026-10-30 就吃过这个亏：PE 里字体到底有没有生效，只能靠拆镜像 + 查注册表 + 看截图推断。
/// 有了这行日志，字体被换掉（比如镜像精简了 `msyh.ttc`）能第一时间在日志里看到。
unsafe fn log_resolved_font(state: &mut State) {
    if FONT_RESOLVED_LOGGED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let screen = GetDC(null_mut());
    if screen.is_null() {
        return;
    }
    let previous = SelectObject(screen, gui_font());
    let mut buffer = [0u16; 64];
    let written = GetTextFaceW(screen, buffer.len() as i32, buffer.as_mut_ptr());
    SelectObject(screen, previous);
    let _ = ReleaseDC(null_mut(), screen);
    let face = if written > 1 {
        let end = (written as usize - 1).min(buffer.len());
        String::from_utf16_lossy(&buffer[..end])
    } else {
        String::from("<unreadable>")
    };
    append_gui_log(
        state,
        &format!(
            "font: requested=\"Microsoft YaHei\" weight={GUI_FONT_WEIGHT} size={} -> GDI selected \"{face}\"",
            crate::dpi::font_size()
        ),
    );
}

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
        GUI_FONT_WEIGHT,
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

// 「任务与证据」窗口（2026-11-02 拆出，见子模块头部注释）。
mod task_admin_window;
use task_admin_window::show_task_admin;

// PE 恢复桌面（2026-11-02 拆出，见子模块头部注释）。
mod pe_desktop;
mod pe_installer;
mod operation_tabs;
pub(crate) use operation_tabs::*;
mod drive_selection;
pub(crate) use drive_selection::*;
mod task_executor;
pub(crate) use task_executor::*;
pub(crate) use pe_installer::*;
pub use pe_installer::pe_reboot_standalone;
pub(crate) use pe_desktop::run_pe_desktop;

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
            // ★ 2.6.96：60 是按 14 号字定的，18 号英文 `Browse…` 要 76 → 切 16。
            //   这里只影响「PE 页未激活时的创建值」（激活后由 layout_operation 按实测重排）。
            96,
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
        // 记一行「GDI 到底选中了哪个字体」——静默回退是这套界面里最难查的一类问题。
        log_resolved_font(&mut *state_ptr);
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
