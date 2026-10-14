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
use backuprestore_core::PROGRAM_VERSION;

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
const CB_SETCURSEL: u32 = 0x014e;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETDROPPEDWIDTH: u32 = 0x0160;
const BM_SETCHECK: u32 = 0x00f1;
const BST_UNCHECKED: usize = 0;
const BST_CHECKED: usize = 1;
const CBN_SELCHANGE: usize = 1;
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
const ID_OPERATION_BACKUP: usize = 1101;
const ID_OPERATION_RESTORE: usize = 1102;
const ID_OPERATION_SECONDARY: usize = 1103;
const ID_OPERATION_PE: usize = 1104;
// 三入口拆分（2026-10-14 拍板）：「单系统还原」拆成 A/B/C 三个入口，
// tab 总数 4 → 6。B=还原数据到非 C 盘，C=还原系统到非 C 盘。
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
const DEFAULT_GUI_FONT: i32 = 17;
// Edit control messages: EM_SETLIMITTEXT with a zero limit on a multiline EDIT
// lifts the default 32,767 character cap, which silently truncated the tail of
// the information report (the 按钮边界 section never reached the screen).
const EM_SETLIMITTEXT: u32 = 0x00C5;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_ELEVATION_CLASS: u32 = 20;
const TTM_ADDTOOLW: u32 = 0x0432;
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
    reserved: usize,
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
    fn TranslateMessage(message: *const Msg) -> i32;
    fn PeekMessageW(message: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
    fn IsWindow(hwnd: Hwnd) -> i32;
    // 对话框式键盘导航：让普通窗口也能用 Tab 遍历焦点、方向键切换
    // 单选按钮、回车触发默认按钮、Esc 关闭、Alt+助记键。
    fn IsDialogMessageW(hwnd: Hwnd, message: *const Msg) -> i32;
    // 即时物理键盘状态：prlctl 宿主导入的修饰键（如 Ctrl）在消息队列里可能有
    // 时序延迟，GetKeyState 偶发读不到；GetAsyncKeyState 反映当前真实状态，更可靠。
    fn GetAsyncKeyState(key: i32) -> i16;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn GetStockObject(index: i32) -> Handle;
}

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
}

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
    /// 六个操作入口 tab（备份 / 还原系统到 C 盘 / 还原数据到非 C 盘 / 还原系统到非 C 盘 /
    /// 新增第二系统 / 安装 PE），BS_AUTORADIOBUTTON + BS_PUSHLIKE 单选组。
    operation_tabs: [Hwnd; 6],
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

/// GUI diagnostics follow the executable, so moving the complete program
/// directory also moves its audit trail. A failed diagnostic write must not
/// alter a safety check or a pending preparation operation.
fn append_gui_log(state: &State, message: &str) {
    let _ = super::append_log(&state.executable_dir.join("logs").join("gui.log"), message);
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
        WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
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
        return;
    }
    let language = selected_language(state);
    let controls = [
        (state.controls.operation_tabs[0], "backup"),
        (state.controls.operation_tabs[1], "restore_a"),
        (state.controls.operation_tabs[2], "restore_b"),
        (state.controls.operation_tabs[3], "restore_c"),
        (state.controls.operation_tabs[4], "secondary"),
        (state.controls.operation_tabs[5], "pe"),
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
    ];
    for (control, key) in controls {
        let text = tooltip_text(language, key);
        add_tooltip(tooltip, state.root, control, text);
    }
    // Keep the tooltip handle alive by storing it in the state object. The
    // control owns the subclass hooks and automatically tracks child bounds.
    state.tooltip = tooltip;
}

fn tooltip_text(language: Language, key: &str) -> &'static str {
    match (language, key) {
        (Language::Chinese, "probe") => "探测：仅校验任务、WinRE 和卷身份，不写入磁盘、不重启。",
        (Language::Chinese, "backup") => "备份：进入 WinRE 后用 DISM 捕获源卷到镜像路径。",
        (Language::Chinese, "restore") => {
            "单系统还原：格式化目标卷并写入镜像，目标必须是当前系统卷。"
        }
        (Language::Chinese, "secondary") => {
            "新增第二系统：格式化另一个卷并通过 BCDBoot 添加启动项。"
        }
        (Language::Chinese, "restore_a") => {
            "还原系统到 C 盘：目标锁定为当前系统盘（显示实际盘符），必须离线执行——重启进入 PE/RE 完成格式化与还原；破坏性确认与环境确认两道确认后才会开始。"
        }
        (Language::Chinese, "restore_b") => {
            "还原数据到非 C 盘：把数据卷镜像还原到非系统分区。默认在线执行、不格式化（覆盖式应用，卷序列号不变）；切到离线则重启进 PE/RE，先 diskpart 格式化再还原。目标不能是当前系统盘。"
        }
        (Language::Chinese, "restore_c") => {
            "还原系统到非 C 盘：把完整 Windows 镜像部署到另一个分区并新增启动菜单项，默认启动项保持不变。默认在线执行（格式化目标并修复引导，不重启）；离线则重启进恢复环境完成。目标不能是当前系统盘。"
        }
        (Language::Chinese, "channel_online") => {
            "在线执行：不重启，直接在当前系统里完成（B 按「格式化目标分区」选项决定是否先清空目标卷；C 恒格式化并修复引导）。"
        }
        (Language::Chinese, "channel_offline") => {
            "离线执行：创建任务并重启进入 PE/Windows RE 完成（目标卷都会被格式化后写入镜像）。"
        }
        (Language::Chinese, "format_target") => {
            "勾选：先用 diskpart 快速格式化目标卷并校验成功（分区不变、NTFS、卷序列号改变），再写入镜像——还原的完整语义（默认）。\n不勾：覆盖式应用，不格式化（目标卷上镜像没有的文件会保留，卷序列号不变）。\n在线离线通道均生效；离线时该选择会随任务落盘、恢复环境内执行。"
        }
        (Language::Chinese, "source") => "选择备份来源或当前 Windows 分区；身份按 GUID 校验。",
        (Language::Chinese, "target") => "还原时将被格式化的目标分区；备份和探测模式不使用。",
        (Language::Chinese, "image") => {
            "镜像文件必须是绝对路径，例如 B:\\BackupRestore\\Windows.wim。"
        }
        (Language::Chinese, "index") => "选择 WIM 索引；下拉项显示索引及详细元数据。",
        (Language::Chinese, "menu") => "第二系统在 Windows 启动菜单中显示的名称。",
        (Language::Chinese, "compress") => {
            "备份镜像压缩方式（仅首次创建 WIM 时生效）：\n• 压缩（默认/推荐）：使用 fast 压缩，速度和体积平衡\n• 不压缩：WIM 最大（约等于源数据量），备份最快\n增量备份说明：镜像已存在时追加为新索引，压缩方式沿用 WIM 首次创建时的设置；压缩方式不影响能否增量备份。"
        }
        (Language::Chinese, "index_name") => {
            "备份索引名（写入 WIM 的 Name 字段）。默认是程序启动时间，可修改；追加备份时用于区分历史版本。"
        }
        (Language::Chinese, "keep") => {
            "保留最近 N 个索引：备份追加成功后自动删除更旧的索引（0=不清理）。删除不可恢复，请谨慎设置。"
        }
        (Language::Chinese, "verify_hash") => {
            "勾选时全量计算镜像 SHA-256 哈希比对；不勾选时仅对比文件物理大小（毫秒级极速，推荐）。"
        }
        (Language::Chinese, "refresh") => "刷新 Windows、WinRE 和可用卷信息。",
        (Language::Chinese, "read_image") => "只读解析 WIM 索引、哈希和元数据。",
        (Language::Chinese, "create_task") => "创建任务；还原操作会先显示确认对话框。",
        (Language::Chinese, "refresh_task") => "读取程序目录中的最近任务状态。",
        (Language::Chinese, "browse") => {
            "打开文件选择框，浏览并选择镜像文件（.wim）的保存或来源路径。"
        }
        (Language::Chinese, "pe_dir_browse") => {
            "浏览并选择 PE 存放目录路径，PE 镜像（boot.wim）将复制到该目录。"
        }
        (Language::Chinese, "pe_reboot_main") => {
            "部署完成后立即重启进入已安装的 PE 环境，验证 PE 能否正常启动。注意：Parallels 等虚拟机的 bootsequence 可能不被固件消费，真实硬件路径有效。"
        }
        (Language::Chinese, "pe_shortcut") => {
            "在桌面创建「打开 安装 PE」快捷方式，方便以后手动进入已安装的 PE 环境。"
        }
        (Language::Chinese, "pe_mode_ram") => {
            "RAM disk 模式（适合临时验证）：PE 镜像临时复制到内存盘启动，不占用独立分区、不改变现有分区布局。"
        }
        (Language::Chinese, "pe_mode_disk") => {
            "硬盘启动模式（更稳定）：把 PE 部署到独立分区并从该分区启动，适合长期保留；需要提供一个空闲分区。"
        }
        (Language::Chinese, "pe_dir_edit") => {
            "PE 目录路径：PE 镜像（boot.wim）存放的文件夹；请输入完整路径（盘符可为任意合法目录，不限于 C）。RAM disk 模式下 WIM 会复制到此路径下。"
        }
        (Language::Chinese, "language") => {
            "切换界面语言（中文 / English），切换后所有标签、按钮与说明文字随之更新。"
        }
        (Language::English, "probe") => {
            "Inspect: validate task, WinRE and volume identities; no disk write or reboot."
        }
        (Language::English, "backup") => {
            "Backup: enter WinRE and capture the selected source volume with DISM."
        }
        (Language::English, "restore") => "Restore: format the target volume and apply the image.",
        (Language::English, "secondary") => {
            "Second system: format another volume and add a BCDBoot entry."
        }
        (Language::English, "restore_a") => {
            "Restore system to C:: target is locked to the running system drive; must run offline (PE/RE) and requires two confirmations."
        }
        (Language::English, "restore_b") => {
            "Restore data to non-C:: apply a data-volume image without touching boot. Online by default (no format); offline formats via diskpart. Target must not be the system drive."
        }
        (Language::English, "restore_c") => {
            "Restore system to non-C:: deploy a full Windows image to another partition with a new boot entry; default boot entry unchanged. Online by default; target must not be the system drive."
        }
        (Language::English, "channel_online") => {
            "Online: run inside the current Windows session without reboot (B honors the 'Format target volume' option; C always formats and repairs boot)."
        }
        (Language::English, "channel_offline") => {
            "Offline: schedule a task and reboot into PE/RE (target volume is formatted)."
        }
        (Language::English, "format_target") => {
            "Checked (default): quick-format the target via diskpart and verify success (same partition, NTFS, volume serial changed) before applying the image.\nUnchecked: overlay apply without formatting (files not in the image remain; volume serial unchanged).\nApplies to both online and offline channels; offline the choice is persisted with the task and honored inside the recovery environment."
        }
        (Language::English, "source") => {
            "Select the backup source or current Windows volume; GUID identity is verified."
        }
        (Language::English, "target") => {
            "Restore target to be formatted; unused by backup and inspect."
        }
        (Language::English, "image") => {
            "The image file must be an absolute path, such as B:\\BackupRestore\\Windows.wim."
        }
        (Language::English, "index") => "Select a WIM index; each item shows detailed metadata.",
        (Language::English, "menu") => "Name shown for the second system in the Windows boot menu.",
        (Language::English, "compress") => {
            "Backup WIM compression (applies only when the WIM is first created):\n• Compression (default/recommended): uses fast compression for a balanced speed and size\n• No compression: largest WIM (approximately the source size), fastest backup\nIncremental notes: appending to an existing WIM keeps the compression set at first creation; compression does not affect whether incremental backup is available."
        }
        (Language::English, "index_name") => {
            "Backup image name (written to the WIM Name field). Defaults to program start time; editable. Appended backups use it to distinguish history."
        }
        (Language::English, "keep") => {
            "Keep latest N indexes: after a successful append, older indexes are deleted (0 = keep all). Deletion is irreversible; set with care."
        }
        (Language::English, "verify_hash") => {
            "When checked, computes full SHA-256 hash before restoring (slower). When unchecked, compares file size only (fast, recommended)."
        }
        (Language::English, "refresh") => "Refresh Windows, WinRE and eligible volume information.",
        (Language::English, "read_image") => {
            "Read WIM indexes, hash and metadata without modifying the image."
        }
        (Language::English, "create_task") => {
            "Create the task; restore operations require confirmation."
        }
        (Language::English, "refresh_task") => {
            "Read the latest task status from the program directory."
        }
        (Language::English, "browse") => {
            "Open a file dialog to pick the .wim image file (save or source path)."
        }
        (Language::English, "pe_dir_browse") => {
            "Browse for the PE storage folder; the PE image (boot.wim) will be copied there."
        }
        (Language::English, "pe_reboot_main") => {
            "Reboot immediately into the PE environment after deploy to verify it boots. Note: bootsequence may not be consumed by Parallels firmware; the real-hardware path works."
        }
        (Language::English, "pe_shortcut") => {
            "Create an 'Open Installed PE' shortcut on the desktop for easy entry later."
        }
        (Language::English, "pe_mode_ram") => {
            "RAM disk mode (good for quick testing): the PE image is loaded into a memory drive, no partition used, existing layout untouched."
        }
        (Language::English, "pe_mode_disk") => {
            "Hard-disk mode (more stable): deploy PE to its own partition and boot from it; requires a spare partition."
        }
        (Language::English, "pe_dir_edit") => {
            "PE folder path: where the PE image (boot.wim) is stored; enter a full path (any valid drive or folder, not only C). In RAM disk mode the WIM is copied here."
        }
        (Language::English, "language") => {
            "Switch the UI language (Chinese / English); all labels, buttons and help text update immediately."
        }
        _ => "",
    }
}

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Language {
    Chinese,
    English,
}

fn ui_text(language: Language, key: &str) -> &'static str {
    match (language, key) {
        (Language::Chinese, "operation") => "操作模式",
        (Language::Chinese, "source") => "源卷",
        (Language::Chinese, "image") => "镜像绝对路径",
        (Language::Chinese, "target") => "目标卷",
        (Language::Chinese, "index") => "WIM 索引",
        (Language::Chinese, "menu") => "第二系统名称",
        (Language::Chinese, "compress") => "压缩率",
        (Language::Chinese, "index_name") => "索引名",
        (Language::Chinese, "keep") => "保留最近 N 个",
        (Language::Chinese, "verify_hash") => "严格对比哈希（默认仅对比大小）",
        (Language::Chinese, "language") => "语言",
        (Language::Chinese, "refresh") => "刷新环境",
        (Language::Chinese, "read_image") => "读取镜像",
        (Language::Chinese, "browse") => "浏览…",
        (Language::Chinese, "create_task") => "创建任务",
        (Language::Chinese, "refresh_task") => "刷新任务状态",
        (Language::Chinese, "initial_status") => {
            "先点击“刷新环境”确认 Windows、恢复环境和卷身份。默认模式为备份。"
        }
        (Language::Chinese, "probe") => "探测（仅检查）",
        (Language::Chinese, "backup") => "备份",
        // 三个还原入口用用户拍板的全称（2026-10-14：文案不得改动）。
        (Language::Chinese, "restore") => "还原系统到 C 盘",
        (Language::Chinese, "restore_a") => "还原系统到 C 盘",
        (Language::Chinese, "restore_b") => "还原数据到非 C 盘",
        (Language::Chinese, "restore_c") => "还原系统到非 C 盘",
        (Language::Chinese, "secondary") => "新增第二系统",
        (Language::Chinese, "probe_hint") => {
            "「探测」是全程无破坏的安全检查模式，适合首次使用先验证环境：\n\n\
             • 刷新并校验当前 Windows 系统、恢复环境（WinRE / PE 入口）以及各卷身份，身份按磁盘+分区 GUID 校验，不依赖盘符；\n\
             • 点「创建任务」只会生成并校验任务文件与恢复环境载荷，验证环境搭建是否正确；\n\
             • 全程不会备份、不会还原、不会格式化、不会改动引导项、不会重启。\n\n\
             建议：第一次使用本程序先切换到「探测」运行一次，确认系统、WinRE、卷都识别正常，再使用其它功能。"
        }
        (Language::Chinese, "backup_hint") => {
            "「备份」把选中的源分区制作成 WIM 镜像（使用 DISM 捕获）：\n\n\
             • 源卷：要备份的分区，默认当前系统分区；镜像路径：用于保存 .wim 的绝对路径；\n\
             • 压缩方式：「压缩」使用 fast，速度和体积平衡；「不压缩」最快但文件最大；\n\
             • 可设置「索引名」与「保留最近 N 个」管理历史版本；镜像已存在时自动追加为新索引；\n\
             • 流程：准备完成后自动进入恢复环境（PE / WinRE），离线捕获源卷，完成后自动返回 Windows。\n\n\
             说明：系统分区必须在离线环境备份；数据卷可在当前系统在线直接备份。"
        }
        (Language::Chinese, "restore_hint") => {
            "「还原系统到 C 盘」把镜像系统写回当前系统盘并重建引导：\n\n\
             • 目标卷：锁定为当前正在运行的系统盘（显示实际盘符，不可改选）；\n\
             • 镜像与索引：要还原的 WIM 及其中索引；还原会覆盖系统盘上的全部数据；\n\
             • 格式化目标分区（默认勾选）：格式化成功校验后才写入；取消勾选=覆盖式应用（旧文件残留）；\n\
             • 流程：破坏性确认 → 选择 PE/RE → 重启离线（按选项格式化）→ DISM 应用镜像 → BCDBoot 重建引导项；\n\
             • 适用场景：系统损坏、无法开机时把备份恢复回本机。\n\n\
             安全提示：目标无法更换，还原前请确认镜像正确、重要数据已另存，操作不可中途取消。"
        }
        (Language::Chinese, "restore_b_hint") => {
            "「还原数据到非 C 盘」把数据卷镜像还原到非系统分区，不碰引导：\n\n\
             • 目标卷：要接收数据的数据分区（不能是当前系统盘，也不能是已有 Windows 的分区）；\n\
             • 格式化目标分区（默认勾选）：先 diskpart 快速格式化并校验成功（分区不变、NTFS、卷序列号改变），再写入镜像；取消勾选=覆盖式应用、卷上多余文件保留；\n\
             • 通道：在线=不重启直接执行；离线=重启进 PE/RE（格式化选项同样生效，随任务落盘）；\n\
             • 镜像与索引：要还原的 WIM 及其中索引；\n\
             • 适用场景：数据盘损坏或迁移时恢复数据。\n\n\
             安全提示：勾选格式化会清空目标卷上的全部数据；不勾选时同名文件也会被镜像内容覆盖。"
        }
        (Language::Chinese, "restore_c_hint") => {
            "「还原系统到非 C 盘」把完整 Windows 镜像部署到另一个分区，成为可启动的第二系统：\n\n\
             • 目标卷：另选一个分区（不能是当前系统盘），将被格式化并写入镜像；\n\
             • 启动项名称：新系统在 Windows 启动菜单中的显示名称；默认启动项保持当前系统不变；\n\
             • 通道差异：在线=不重启，直接在当前系统格式化目标并修复引导；离线=重启进恢复环境完成；\n\
             • 与「新增第二系统」的区别：本入口默认在线、随时可做；那个入口固定走离线任务链。\n\n\
             安全提示：目标卷数据会被覆盖，部署前请确认该分区上没有需要保留的内容。"
        }
        (Language::Chinese, "secondary_hint") => {
            "「新增第二系统」在保留当前 Windows 的前提下，把另一个镜像部署成可启动的第二系统：\n\n\
             • 目标卷：另选一个空闲分区（不占用当前系统），镜像写入该分区；\n\
             • 启动项名称：自定义新系统在 Windows 启动菜单中显示的名称，可用中文；\n\
             • 流程：离线环境格式化目标分区 → DISM 应用镜像 → BCDBoot 添加启动项；\n\
             • 效果：重启后开机菜单可自由选择进入任一系统，适合双系统共存场景。\n\n\
             说明：目标卷数据会被覆盖，部署前请确认该分区上没有需要保留的内容。"
        }
        (Language::Chinese, "pe") => "安装 PE",
        (Language::Chinese, "pe_hint") => {
            "「安装 PE」部署一个自包含的 PE 备份/恢复环境，作为开机可选启动项：\n\n\
             • 启动方式：RAM disk（不占分区，镜像临时复制到内存盘启动）或硬盘启动（独立分区，更稳定）；\n\
             • PE 目录路径 / 目标卷：指定放置 PE 镜像（boot.wim）的目录或分区；启动项名称可自定义；\n\
             • 部署后新增 BCD 启动项「Windows PE (BackupRestore)」，不改变当前 Windows 默认启动；\n\
             • 用途：当系统无法启动时，可进入 PE 离线备份、还原、安装第二系统、打开完整程序或返回 Windows；\n\
             • 勾选「重启进入 PE」可验证 PE 能否正常启动；PE 使用程序自带的标准 PE 镜像（非 WinRE）。"
        }
        (Language::English, "operation") => "Operation",
        (Language::English, "source") => "Windows source",
        (Language::English, "image") => "Image absolute path",
        (Language::English, "target") => "Restore target",
        (Language::English, "index") => "WIM index",
        (Language::English, "menu") => "Secondary boot name",
        (Language::English, "compress") => "Compression",
        (Language::English, "index_name") => "Image name",
        (Language::English, "keep") => "Keep latest N",
        (Language::English, "verify_hash") => "Verify hash (default: size only)",
        (Language::English, "language") => "Language",
        (Language::English, "refresh") => "Refresh environment",
        (Language::English, "read_image") => "Read image",
        (Language::English, "browse") => "Browse…",
        (Language::English, "create_task") => "Create task",
        (Language::English, "refresh_task") => "Refresh task status",
        (Language::English, "initial_status") => {
            "Click Refresh environment to inspect Windows, WinRE and volume identities. Default mode is backup."
        }
        (Language::English, "probe") => "Inspect",
        (Language::English, "backup") => "Backup",
        (Language::English, "restore") => "Restore C:",
        (Language::English, "restore_a") => "Restore C:",
        (Language::English, "restore_b") => "Restore data",
        (Language::English, "restore_c") => "Restore other",
        (Language::English, "secondary") => "Second system",
        (Language::English, "probe_hint") => {
            "Inspect is a completely non-destructive safety check; run it first to validate your environment.\n\n• Refreshes and verifies the current Windows, recovery environment (WinRE / PE entry) and every volume, identified by disk+partition GUID instead of drive letter;\n• 'Create task' only writes and validates the task files and WinRE payloads to confirm the setup;\n• It never backs up, restores, formats, touches boot entries, or reboots.\n\nTip: first-time users should switch to Inspect and run once to confirm Windows, WinRE and volumes are recognized before using other features."
        }
        (Language::English, "backup_hint") => {
            "Backup captures the selected source partition into a WIM image (via DISM).\n\n• Source volume: the partition to back up (defaults to the system partition); Image path: absolute path for the .wim;\n• Compression: \"Compression\" uses fast for balanced speed and size; \"No compression\" is fastest but largest;\n• 'Image name' and 'Keep latest N' manage history; when the image already exists a new index is appended automatically;\n• Flow: after preparation it enters recovery (PE / WinRE), captures the source volume offline, then returns to Windows.\n\nNote: the system partition must be backed up offline; data volumes can be backed up online in the current system."
        }
        (Language::English, "restore_hint") => {
            "Restore C: writes an image system back to the running system drive and rebuilds the boot loader.\n\n• Target volume: locked to the running system drive (actual letter shown, not selectable);\n• Image & index: the WIM and its index to restore; all data on the system drive is overwritten;\n• Flow: destructive confirmation → choose PE/RE → reboot, format the target offline → DISM Apply → BCDBoot rebuilds the boot entry;\n• Use case: recover this machine from a backup when the system is damaged.\n\nSafety: the target cannot be changed; confirm the image is correct and important data is stored elsewhere. The operation cannot be cancelled midway."
        }
        (Language::English, "restore_b_hint") => {
            "Restore data applies a data-volume image to a non-system partition without touching boot.\n\n• Target volume: a data partition (never the system drive, nor a partition that already has Windows);\n• Format target volume (checked by default): quick-format via diskpart, verify success (same partition, NTFS, serial changed), then write the image; unchecked = overlay apply, extra files remain;\n• Channel: online = no reboot; offline = reboot into PE/RE (always formats there);\n• Image & index: the WIM and its index to restore;\n• Use case: recover or migrate data volumes.\n\nSafety: formatting erases everything on the target; even without it, same-name files are overwritten."
        }
        (Language::English, "restore_c_hint") => {
            "Restore other deploys a full Windows image to another partition as a bootable second system.\n\n• Target volume: another partition (never the system drive); it is formatted and receives the image;\n• Boot name: the name shown in the Windows boot menu; the default boot entry stays on the current system;\n• Channel difference: online = no reboot, format and boot-repair inside the current session; offline = reboot into the recovery environment;\n• Difference from 'Second system': this entry runs online by default; that one always goes through the offline task chain.\n\nSafety: the target volume is overwritten; confirm it holds nothing you need to keep."
        }
        (Language::English, "secondary_hint") => {
            "Second system deploys another image as a bootable system while keeping the current Windows.\n\n• Target volume: pick a spare partition (not the current system) to write the image;\n• Boot name: customize the name shown in the boot menu; Chinese is supported;\n• Flow: format the target offline → DISM Apply → BCDBoot adds a boot entry;\n• Result: after reboot the boot menu lets you choose either system; ideal for dual-boot.\n\nNote: the target volume is overwritten; confirm it holds nothing you need to keep."
        }
        (Language::English, "pe") => "Install PE",
        (Language::English, "pe_hint") => {
            "Install PE deploys a self-contained PE backup/restore environment as an optional boot entry.\n\n• Boot mode: RAM disk (no partition; image loaded into a memory drive) or hard-disk boot (own partition, more stable);\n• PE folder path / target volume: where the PE image (boot.wim) is placed; boot name is customizable;\n• Adds a BCD entry \"Windows PE (BackupRestore)\" without changing the current Windows default boot;\n• Purpose: when the system cannot boot, enter PE to back up, restore, install a second system, open the full program, or return to Windows;\n• Enable 'Reboot into PE' to verify it boots; PE uses the bundled standard PE image (not WinRE)."
        }
        _ => "",
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
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
        x,
        y,
        width,
        height,
        parent,
        id as HMenu,
        null_mut(),
        null_mut(),
    );
    let font = GetStockObject(DEFAULT_GUI_FONT);
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
        reserved: 0,
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
        3 => "restore-system-other",
        4 => "create-secondary",
        5 => "install-pe-entry",
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
        "restore-system-other" => "create-secondary",
        "backup" => "backup",
        "restore-existing" => "restore-existing",
        "create-secondary" => "create-secondary",
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
        "restore-existing" | "restore-data" | "restore-system-other" | "create-secondary"
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

unsafe fn set_operation_tabs(state: &State, language: Language) {
    for (index, (button, key)) in state
        .controls
        .operation_tabs
        .iter()
        .zip([
            "backup", "restore_a", "restore_b", "restore_c", "secondary", "pe",
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

fn operation_display(language: Language, operation: &str) -> &'static str {
    match (language, operation) {
        (Language::Chinese, "probe") => "探测（仅检查）",
        (Language::Chinese, "backup") => "备份",
        (Language::Chinese, "restore-existing") => "还原系统到 C 盘",
        (Language::Chinese, "restore-data") => "还原数据到非 C 盘",
        (Language::Chinese, "restore-system-other") => "还原系统到非 C 盘",
        (Language::Chinese, "create-secondary") => "新增第二系统",
        (Language::Chinese, "install-pe-entry") => "安装 PE",
        (Language::English, "probe") => "Inspect",
        (Language::English, "backup") => "Backup",
        (Language::English, "restore-existing") => "Restore system to C:",
        (Language::English, "restore-data") => "Restore data to non-C:",
        (Language::English, "restore-system-other") => "Restore system to non-C:",
        (Language::English, "create-secondary") => "Second system",
        (Language::English, "install-pe-entry") => "Install PE",
        _ => "",
    }
}

fn operation_hint_key(operation: &str) -> &'static str {
    match operation {
        "backup" => "backup_hint",
        "restore-existing" => "restore_hint",
        "restore-data" => "restore_b_hint",
        "restore-system-other" => "restore_c_hint",
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
        "restore-existing" | "restore-data" | "restore-system-other" | "create-secondary"
            | "install-pe-entry"
    );
    // A/B/C/新增第二系统都要手选 WIM 索引 + 副档校验复选框。
    let show_index = is_image_restore_entry(operation);
    // 源卷行只在「备份」（选备份来源）与「新增第二系统」（显示保留系统）出现；
    // A/B/C 的源卷没有用户语义（A/B 源=目标，C 源=当前系统盘），隐藏以免误导
    // （原 P3 待办的「源卷（当前系统）」文案问题随隐藏一并消除）。
    let show_source = matches!(operation, "backup" | "create-secondary");
    // WIM 压缩方式（fast/none）只对备份首次创建有意义，仅「备份」tab 显示。
    let show_compress = operation == "backup";
    // 「第二系统名称」在「新增第二系统」与 C（他盘系统还原，离线 prepare 必填）
    // 显示；「安装 PE」tab 有自己的「PE 启动项名称」输入框，若此处也显示 menu，
    // 其布局位置（field_x+500, secondary_y）会恰好覆盖右下角「创建快捷方式」按钮。
    let show_menu = matches!(operation, "create-secondary" | "restore-system-other");
    // B/C 入口显示「在线/离线」通道单选；A 必离线（无选择），备份走智能分流。
    let show_channel = matches!(operation, "restore-data" | "restore-system-other");
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
    set_child_visible(
        ID_FORMAT_TARGET as i32,
        matches!(
            operation,
            "restore-existing" | "restore-data" | "restore-system-other"
        ),
    );
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

unsafe fn layout_operation(state: &State) {
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 1020,
        bottom: 760,
    };
    GetClientRect(state.root, &mut rect);
    let client_width = (rect.right - rect.left).max(1020);
    let field_x = 180;
    let field_width = (client_width - field_x - 24).max(700);
    let details_height = 112;
    let row_gap = 8;
    // 三入口拆分后的行可见性：源卷行仅备份/新增第二系统显示（A/B/C 隐藏），
    // 通道单选行仅 B/C 显示。隐藏的行不占位，目标行相应上移。
    let layout_op = selected_operation(state);
    let source_visible = matches!(layout_op, "backup" | "create-secondary");
    let target_visible = matches!(
        layout_op,
        "restore-existing" | "restore-data" | "restore-system-other" | "create-secondary"
            | "install-pe-entry"
    );
    let image_visible = layout_op != "probe";
    let index_visible = is_image_restore_entry(layout_op);
    let channel_visible = matches!(layout_op, "restore-data" | "restore-system-other");
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
    let show_format_target = is_image_restore_entry(layout_op) && layout_op != "create-secondary";
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
    let status_y = last_details_y + 16 + pe_extra_y;
    let image_y = status_y + 84;
    let secondary_y = image_y + 34;
    // C（还原系统到非 C 盘）与「新增第二系统」的 verify_hash 复选框在
    // secondary_y+32（底缘 +56）——按钮行必须让过它，留 8px 间隙。
    // （2.5.2 及此前按钮行在 +40，与复选框重叠 16px，「严格对比哈希」被挡住。）
    let buttons_y = if index_visible {
        if matches!(
            selected_operation(state),
            "create-secondary" | "restore-system-other"
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
        status_y + 84
    };

    let reposition = |hwnd: Hwnd, x: i32, y: i32, width: i32, height: i32| {
        if !hwnd.is_null() {
            MoveWindow(hwnd, x, y, width, height, 1);
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
        67,
        55,
        18,
    );
    reposition(
        state.controls.language,
        client_width - 20 - lang_combo_w,
        60,
        lang_combo_w,
        300,
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
    reposition(state.controls.status, 20, status_y, client_width - 40, 64);

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
            let exe_drive = state
                .executable_dir
                .to_string_lossy()
                .chars()
                .next()
                .unwrap_or('C')
                .to_ascii_uppercase();
            let default_drive = state
                .drives
                .iter()
                .map(|d| d.letter.to_ascii_uppercase())
                .find(|letter| {
                    letter
                        .chars()
                        .next()
                        .map(|c| c != exe_drive)
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
        // 浏览按钮紧贴目录输入框右侧（输入框 380..580，按钮 586..646）
        reposition(
            GetDlgItem(state.root, ID_PE_DIR_BROWSE as i32),
            field_x + 586,
            mode_y,
            60,
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

    let image_width = (field_width - 90).max(400);
    reposition(state.controls.image, field_x, image_y, image_width, 24);
    reposition(
        GetDlgItem(state.root, ID_BROWSE_IMAGE as i32),
        field_x + image_width + 10,
        image_y,
        80,
        24,
    );
    // 「单系统还原」与「新增第二系统」tab 的 WIM 索引与校验复选框布局：
    // C（还原系统到非 C 盘）与「新增第二系统」一样带菜单名输入框，用同一布局。
    let (index_width, verify_hash_x, verify_hash_y) =
        if matches!(
            selected_operation(state),
            "create-secondary" | "restore-system-other"
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
        (Language::Chinese, "restore-system-other") => (
            "源卷（不使用）".into(),
            "目标卷（第二系统位置）".into(),
        ),
        (Language::Chinese, "create-secondary") => {
            ("源卷（保留系统）".into(), "目标卷（第二系统）".into())
        }
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
        (Language::English, "restore-system-other") => (
            "Source (unused)".into(),
            "Target (second system)".into(),
        ),
        (Language::English, "create-secondary") => {
            ("Source (keep)".into(), "Target (second system)".into())
        }
        (Language::English, "install-pe-entry") => {
            ("Source (unused)".into(), "Target (PE install)".into())
        }
        (Language::English, _) => ("Source (inspect)".into(), "Target (unused)".into()),
    };
    set_child_text(state.root, 2003, &source);
    set_child_text(state.root, 2005, &target);
}

unsafe fn select_operation(state: &mut State, index: usize) {
    state.operation_index = index.min(5);
    set_operation_tabs(state, selected_language(state));
    let operation = selected_operation(state);
    if operation == "restore-existing" {
        // A（还原系统到 C 盘）：目标锁定 %SystemDrive%，自动选中（下拉已禁用）。
        // 旧版「源卷选择复制到目标」逻辑随源卷行隐藏一并移除。
        let letter = active_system_letter();
        select_drive(state, state.controls.target, &letter);
    }
    if matches!(operation, "restore-data" | "restore-system-other") {
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
        // 目标若还停在初始默认的系统盘（B/C 都禁止的目标），自动跳到第一个
        // 合法卷：B=非系统且非 Windows 卷；C=非系统卷。找不到就保持原样，
        // 由「创建任务」时的目标闸门给出明确报错（不做静默猜测）。
        let system = active_system_letter();
        if let Some(current) = selected_drive_letter(state, state.controls.target)
            && current.eq_ignore_ascii_case(&system)
            && let Some(candidate) = state.drives.iter().find(|drive| {
                !drive.letter.eq_ignore_ascii_case(&system)
                    && (operation == "restore-system-other" || !drive.has_windows_installation)
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
    // 整窗强制重绘（历史 bug 双保险）：layout_operation 会对十余个控件做
    // MoveWindow，快速连续切换 tab 时重绘风暴容易让个别控件区域残留上一
    // tab 的文本（表现为「PE tab 镜像标签行叠出上一 tab 的 status 文本」）。
    // 在全部布局与文案就绪后同步重绘整窗，彻底擦除残留像素。
    InvalidateRect(state.root, null(), 1);
    UpdateWindow(state.root);
}

unsafe fn selected_drive_letter(state: &State, control: Hwnd) -> Option<String> {
    combo_selection(control)
        .and_then(|index| state.drives.get(index))
        .map(|drive| drive.letter.clone())
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
            "{}: {}\nFile system: {}\nWindows installation: {}\nTotal: {} | Free: {}\nDisk/partition: {}/{}\nPartition type: {}\nVolume GUID: {}",
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
            "{}: {}\n文件系统：{}\nWindows 安装：{}\n总容量：{} | 可用：{}\n磁盘/分区：{}/{}\n分区类型：{}\n卷 GUID：{}",
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
        (Language::Chinese, "source", "create-secondary") => {
            "源卷用途：当前保留不覆盖的 Windows 分区；新增第二系统时目标卷必须与它不同。"
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
        (Language::Chinese, "target", "restore-system-other") => {
            "目标卷用途：将被格式化并写入完整 Windows 作为第二系统，新增启动菜单项、默认启动项不变。必须不同于当前系统盘。"
        }
        (Language::Chinese, "target", "create-secondary") => {
            "目标卷用途：将被格式化并写入镜像作为第二个 Windows；必须不同于源卷和程序目录所在分区。"
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
        (Language::English, "source", "create-secondary") => {
            "Purpose: kept untouched during deployment; the secondary target must differ from it."
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
        (Language::English, "target", "restore-system-other") => {
            "Purpose: formatted and written as a second Windows with a new boot entry; default boot entry unchanged. Must differ from the system drive."
        }
        (Language::English, "target", "create-secondary") => {
            "Purpose: will be formatted and receive the second Windows; it must differ from source and the program directory volume."
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
    let controls = [state.controls.source, state.controls.target];
    for control in controls {
        reset_combo(control);
        if state.drives.is_empty() {
            add_combo_item(
                control,
                if selected_language(state) == Language::English {
                    "No eligible mounted volume"
                } else {
                    "没有可选择的已挂载卷"
                },
            );
            SendMessageW(control, CB_SETCURSEL, 0, 0);
            continue;
        }
        for drive in &state.drives {
            add_combo_item(control, &drive_display(drive, selected_language(state)));
        }
    }
    for (control, wanted) in controls.into_iter().zip(desired.into_iter().skip(1)) {
        let position = wanted
            .and_then(|letter| state.drives.iter().position(|drive| drive.letter == letter))
            .unwrap_or(0);
        SendMessageW(control, CB_SETCURSEL, position, 0);
    }
}

unsafe fn select_drive(state: &State, control: Hwnd, letter: &str) {
    if let Some(position) = state
        .drives
        .iter()
        .position(|drive| drive.letter.eq_ignore_ascii_case(letter))
    {
        SendMessageW(control, CB_SETCURSEL, position, 0);
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
    let dropdown_width = (rect.right - rect.left - 204).max(440) as usize;
    SendMessageW(state.controls.index, CB_SETDROPPEDWIDTH, dropdown_width, 0);
    SendMessageW(state.controls.index, CB_SETCURSEL, selected_position, 0);
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

unsafe fn apply_language(state: &mut State) {
    let language = selected_language(state);
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
        set_text(state.controls.menu, default_menu_name);
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

// ===================== 系统分区处理方式选择（3 按钮模态） =====================
// 智能分流：备份/还原目标为「当前活动系统」时，不能在线执行，弹窗让用户选
// 进入 PE / 进入 Windows RE / 取消。自绘模态对话框（不依赖 comctl32 v6
// TaskDialog），返回 1=进入 PE、2=进入 Windows RE、0=取消。
const ID_CHOICE_BODY: usize = 2000;
const ID_CHOICE_PE: usize = 2001;
const ID_CHOICE_RE: usize = 2002;
const ID_CHOICE_CANCEL: usize = 2003;
/// 系统选择对话框的日志位置：与 GUI 主日志同目录，故障时一起看。
fn system_choice_log_path() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("logs")
        .join("gui.log")
}
/// 对话框文案语言：0=中文 1=English（模态期间单实例，用静态即可）。
static CHOICE_LANGUAGE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
// PE/RE 选择弹窗的正文模式：false=目标为当前系统盘（默认，历史文案）；
// true=用户在 B（还原数据到非 C 盘）显式选择离线通道——目标是数据卷，不涉及系统盘，
// 文案改为通用离线说明。调用方在 ask_system_drive_handler 前置位，弹窗创建时消费。
static CHOICE_GENERIC_OFFLINE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
        let font = GetStockObject(DEFAULT_GUI_FONT) as Handle;
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
        let body = create_control(
            hwnd,
            "STATIC",
            body_text,
            0,
            30,
            24,
            400,
            78,
            ID_CHOICE_BODY,
        );
        let btn_pe = create_control(
            hwnd,
            "BUTTON",
            if english {
                "Enter PE (recommended)"
            } else {
                "进入 PE（推荐）"
            },
            WS_TABSTOP | BS_DEFPUSHBUTTON,
            30,
            112,
            400,
            42,
            ID_CHOICE_PE,
        );
        let btn_re = create_control(
            hwnd,
            "BUTTON",
            if english {
                "Enter Windows RE"
            } else {
                "进入 Windows RE"
            },
            WS_TABSTOP,
            30,
            162,
            400,
            42,
            ID_CHOICE_RE,
        );
        let btn_cancel = create_control(
            hwnd,
            "BUTTON",
            if english { "Cancel" } else { "取消" },
            WS_TABSTOP,
            30,
            212,
            400,
            42,
            ID_CHOICE_CANCEL,
        );
        SendMessageW(body, WM_SETFONT, font as WParam, 1);
        SendMessageW(btn_pe, WM_SETFONT, font as WParam, 1);
        SendMessageW(btn_re, WM_SETFONT, font as WParam, 1);
        SendMessageW(btn_cancel, WM_SETFONT, font as WParam, 1);
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
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(system_choice_log_path())
                    .and_then(|mut file| {
                        use std::io::Write;
                        writeln!(
                            file,
                            "[gui] system drive choice: button clicked, choice={result}"
                        )
                    });
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
unsafe fn ask_system_drive_handler(hwnd: Hwnd, language: Language) -> i32 {
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
        (screen_w - 460) / 2,
        (screen_h - 320) / 2,
        460,
        320,
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

const ID_CHOICE_CHOOSE_LOCATION: usize = 3001;
const ID_CHOICE_PATH_CANCEL: usize = 3002;
static INVALID_PATH_LANGUAGE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

unsafe extern "system" fn window_proc_invalid_path_choice(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    if message == WM_CREATE {
        let english = INVALID_PATH_LANGUAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
        let font = GetStockObject(DEFAULT_GUI_FONT) as Handle;
        let body = create_control(
            hwnd,
            "STATIC",
            if english {
                "The image absolute path is invalid or has not been specified.\n\nPlease click [Choose image location] to select a file, or [Cancel] to return."
            } else {
                "镜像绝对路径无效或尚未指定。\n\n请点击【选择镜像位置】选择文件，或点击【取消】返回。"
            },
            0,
            30,
            24,
            400,
            64,
            0,
        );
        let btn_choose = create_control(
            hwnd,
            "BUTTON",
            if english {
                "Choose image location"
            } else {
                "选择镜像位置"
            },
            WS_TABSTOP | BS_DEFPUSHBUTTON,
            30,
            104,
            400,
            42,
            ID_CHOICE_CHOOSE_LOCATION,
        );
        let btn_cancel = create_control(
            hwnd,
            "BUTTON",
            if english { "Cancel" } else { "取消" },
            WS_TABSTOP,
            30,
            156,
            400,
            42,
            ID_CHOICE_PATH_CANCEL,
        );
        SendMessageW(body, WM_SETFONT, font as WParam, 1);
        SendMessageW(btn_choose, WM_SETFONT, font as WParam, 1);
        SendMessageW(btn_cancel, WM_SETFONT, font as WParam, 1);
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

/// 路径不合法时弹出的模态对话框，提供【选择镜像位置】与【取消】两个按钮。
/// 返回 1=选择镜像位置，0=取消。
unsafe fn ask_invalid_image_path(hwnd: Hwnd, language: Language) -> i32 {
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
    let caption = wide(if language == Language::English {
        "Invalid image path"
    } else {
        "镜像路径无效"
    });
    let screen_w = GetSystemMetrics(SM_CXSCREEN).max(640);
    let screen_h = GetSystemMetrics(SM_CYSCREEN).max(480);
    let dialog = CreateWindowExW(
        0,
        class_name.as_ptr(),
        caption.as_ptr(),
        WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
        (screen_w - 460) / 2,
        (screen_h - 260) / 2,
        460,
        260,
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
    let text = match std::env::current_exe()
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
                format!(
                    "任务 ID：{task_id}\n操作：{operation}\n任务目录：{task_root}\n准备日志：{prepare_log}\n恢复日志：{recovery_log}"
                )
            }
            Err(error) => format!("读取任务状态失败：{error}"),
        },
        None => "尚未找到最近任务记录。".to_string(),
    };
    let text = if language == Language::English {
        text.replace("任务 ID：", "Task ID: ")
            .replace("操作：", "Operation: ")
            .replace("任务目录：", "Task root: ")
            .replace("准备日志：", "Prepare log: ")
            .replace("恢复日志：", "Recovery log: ")
            .replace("状态：", "Status: ")
            .replace(
                "状态：status.json 不可读或尚未生成",
                "Status: status.json is unavailable",
            )
            .replace("读取任务状态失败：", "Task status read failed: ")
    } else {
        text
    };
    set_text(state.controls.status, &text);
    append_gui_log(state, "GUI action completed: refresh latest task status");
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
        append_gui_log(
            state,
            &format!("GUI action blocked: invalid WIM path: {error}"),
        );
        show_message(
            state.root,
            &if language == Language::English {
                format!("Invalid image absolute path: {error}")
            } else {
                format!("镜像绝对路径无效：{error}")
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
            let diagnostic = if images_text.len() > 4096 {
                format!("{}…", &images_text[..4096])
            } else {
                images_text
            };
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
    // "restore-data"(B)/"restore-other"(C)/"secondary" 等
    let tab = json.get("tab").and_then(|v| v.as_str()).unwrap_or("pe");
    let op_index = match tab {
        "backup" => 0,
        "restore" => 1,
        "restore-data" => 2,
        "restore-other" => 3,
        "secondary" => 4,
        _ => 5,
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
            append_gui_log(state, &format!("test hook: source={vol} pos={pos}"));
        } else {
            append_gui_log(state, &format!("test hook: source vol {vol} not found"));
        }
    }
    if let Some(vol) = json.get("target_volume").and_then(|v| v.as_str()) {
        if let Some(pos) = state
            .drives
            .iter()
            .position(|d| d.letter.eq_ignore_ascii_case(vol))
        {
            SendMessageW(state.controls.target, CB_SETCURSEL, pos, 0);
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
        let _ = std::fs::create_dir_all(&sources_dir);
    }
    let wim_dest = PathBuf::from(format!("{sources_dir}\\boot.wim"));
    if wim_dest.is_file() {
        let stock = PathBuf::from(format!("{sources_dir}\\boot.wim.stock"));
        if !stock.is_file() {
            let _ = std::fs::copy(&wim_dest, &stock);
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
        let _ = std::fs::create_dir_all(&boot_dir);
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
    for step in &steps {
        let code = run_cmd_to_file(step, None);
        append_gui_log(state, &format!("PE install bcdedit: {step} -> {code}"));
        if !code.is_success() {
            failed_step = Some(step.clone());
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
    let drive_char = target_drive.chars().next().unwrap_or('C');
    let program_drive = state
        .executable_dir
        .to_string_lossy()
        .chars()
        .next()
        .unwrap_or(' ');
    // 目标分区安全校验：非系统盘、非程序所在盘、非 ESP、可用空间 >= 2GB
    let mut is_system_volume = false;
    let mut free_bytes: u64 = 0;
    for drive in &state.drives {
        if drive.letter.eq_ignore_ascii_case(&drive_char.to_string()) {
            is_system_volume = drive.has_windows_installation;
            free_bytes = drive.free_bytes.unwrap_or(0);
            break;
        }
    }
    if drive_char == 'C' || drive_char == 'S' || drive_char == program_drive {
        append_gui_log(
            state,
            "GUI action blocked: target partition is system/ESP/program volume",
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
    let format_log = state.executable_dir.join("pe-format.log");
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
            &format!("PE 格式化前身份：{before:?}；脚本：{script}"),
        )?;
        let result = crate::windows_command::run_program(
            "diskpart.exe",
            &["/s".into(), script_file.to_string_lossy().into_owned()],
            Some(&format_log),
            300000,
        );
        crate::append_log(&format_log, &format!("diskpart 执行结果：{result}"))?;
        if !result.is_success() {
            return Err(crate::err(&format!("diskpart failed: {result}")));
        }
        let after = crate::windows_prepare::volume_identity(drive_char)?;
        crate::append_log(&format_log, &format!("PE 格式化后身份：{after:?}"))?;
        backuprestore_core::operation_safety::verify_formatted_volume(&before, &after)?;
        if !Path::new(&format!("{drive_char}:\\")).is_dir() {
            return Err(crate::err("formatted volume root inaccessible"));
        }
        std::fs::remove_file(script_file)?;
        crate::append_log(
            &format_log,
            "格式化身份、NTFS 文件系统与新卷序列号已回读验证",
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
    let apply_code = run_cmd_to_file_timeout(
        &format!(
            "dism /Apply-Image /ImageFile:\"{image_path}\" /Index:1 /ApplyDir:{drive_char}:\\"
        ),
        None,
        crate::windows_command::INFINITE,
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
    for step in &steps {
        let code = run_cmd_to_file(step, None);
        append_gui_log(state, &format!("PE hard disk bcdedit: {step} -> {code}"));
        if !code.is_success() {
            failed_step = Some(step.clone());
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
    append_gui_log(state, &format!("PE reboot to PE: {command}"));
    let code = run_cmd_to_file(&command, None);
    append_gui_log(state, &format!("PE reboot to PE: exit code={code}"));
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
    let code = run_cmd_to_file(&command, None);
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

/// 读取最近一次 launcher-errors.log 里的错误信息（用于 prepare 异常退出感知）
fn read_latest_launcher_error(executable_dir: &std::path::Path) -> Option<String> {
    let log_path = executable_dir.join("logs").join("launcher-errors.log");
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
        append_gui_log(
            state,
            &format!("GUI action blocked: invalid image path: {error}"),
        );
        let choice = ask_invalid_image_path(state.root, language);
        if choice == 1 {
            // 用户点击【选择镜像位置】：立即弹出文件浏览窗口
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
        if recheck.is_none() {
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
        recheck.unwrap().to_string()
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
    let target_drive = match selected_or_error(
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
    };
    let source_drive = match operation.as_str() {
        "restore-existing" | "restore-data" => target_drive.clone(),
        "restore-system-other" => active_system_letter(),
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
    if matches!(operation.as_str(), "restore-data" | "restore-system-other")
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
            "GUI action blocked: restore-data/restore-system-other target is the active system drive",
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
    // C（还原系统到非 C 盘）走 create-secondary 语义，离线 prepare 要求启动菜单名必填；
    // 在线通道同样用它给新启动项命名。空名在这里拦下，避免提权后才发现。
    if operation == "restore-system-other" && get_text(state.controls.menu).trim().is_empty() {
        append_gui_log(state, "GUI action blocked: restore-system-other requires a boot menu name");
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
    let operation_label = operation_display(language, &operation);
    let summary = if language == Language::English {
        format!(
            "Mode: {operation_label}\nSource volume: {source_drive}\nImage: {image_path}\nRestore target: {target_drive}\nWIM index: {index}",
        )
    } else {
        format!(
            "模式：{operation_label}\n源卷：{source_drive}\n镜像绝对路径：{image_path}\n目标卷：{target_drive}\nWIM 索引：{index}",
        )
    };
    // 测试钩子（--test-hook + auto_install）下自动接受确认框，与 PE 安装确认同策略：
    // 自动化验收环境无法精确点击弹窗按钮，卡在这里等于流程永远走不下去。
    let destructive_answer = if TEST_AUTO_CONFIRM.load(std::sync::atomic::Ordering::SeqCst) {
        append_gui_log(state, "test hook: destructive confirmation auto-accepted");
        IDYES
    } else {
        show_message(
            state.root,
            &if language == Language::English {
                format!("{summary}\n\nThe target partition will be overwritten. Continue?")
            } else {
                format!("{summary}\n\n目标分区将被覆盖，确认继续？")
            },
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
    if matches!(operation.as_str(), "restore-data" | "restore-system-other") {
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
            let online_image_name = if operation == "restore-system-other" {
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
            "restore-existing" | "restore-data" | "restore-system-other"
        ) && IsDlgButtonChecked(state.root, ID_FORMAT_TARGET as i32) == 0
        {
            append_gui_log(state, "prepare args: --skip-format (overlay restore)");
            arguments.push("--skip-format".to_string());
        }
    }
    if operation == "probe" {
        arguments.push("--no-reboot".to_string());
    }
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
                let last_error = read_latest_launcher_error(&state.executable_dir)
                    .unwrap_or_else(|| {
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
            .map_err(|_| crate::err("镜像索引无效"))?;
        let request = crate::pe_operation::new_request(mode, drive, image_path, index, None, "")?;
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
    append_gui_log(state, &format!("schedule_pe_task: {command}"));
    let code = run_cmd_to_file(&command, None);
    append_gui_log(
        state,
        &format!("schedule_pe_task: bcdedit exit code={code}"),
    );
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
            let result = std::panic::catch_unwind(|| crate::online_operation::execute(&params))
                .unwrap_or_else(|_| OnlineResult {
                    success: false,
                    detail: "在线执行线程异常退出，请检查本轮日志和候选目录；不能判定为成功。"
                        .into(),
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
                60,
                180,
                300,
                ID_LANGUAGE,
            ),
            operation_tabs: [
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    // WS_GROUP 标记「操作模式」单选组的起点：方向键只在这 6
                    // 个之间切换，不会跳到下方 PE 启动方式/通道那一组。
                    // 布局：入口名用用户拍板的全称（长短不一），按钮不等宽、
                    // 间距 3px，从 x=90 排到 ~693；语言栏靠右（layout_operation
                    // 动态右对齐），最小窗口 1020 宽下两者不重叠。
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
                    120,
                    32,
                    ID_OPERATION_RESTORE_OTHER,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    518,
                    60,
                    100,
                    32,
                    ID_OPERATION_SECONDARY,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    621,
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
                70,
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
                70,
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
                90,
                ID_STATUS,
            ),
        };
        add_combo_item(controls.language, "中文");
        add_combo_item(controls.language, "English");
        SendMessageW(controls.language, CB_SETCURSEL, 0, 0);
        SendMessageW(controls.operation_tabs[0], BM_SETCHECK, BST_CHECKED, 0);
        // 压缩下拉：初始语言为中文；索引 0/1 ↔ fast/none，默认「压缩」。
        for label in compress_level_labels(Language::Chinese) {
            add_combo_item(controls.compress, label);
        }
        SendMessageW(controls.compress, CB_SETCURSEL, 0, 0);
        SendMessageW(controls.verify_hash, BM_SETCHECK, BST_UNCHECKED, 0);
        // 备份索引名默认值 = 程序启动时间（本地），用户可修改。
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
        let default_index_name = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            now.year, now.month, now.day, now.hour, now.minute
        );
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
        let state = Box::new(State {
            root: hwnd,
            tooltip: null_mut(),
            controls,
            executable_dir,
            wim_images: Vec::new(),
            drives,
            operation_index: 0,
            test_drive_choice: None,
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
            && (0..=5).contains(&index)
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
                && (0..=5).contains(&index)
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
                append_gui_log(
                    state,
                    &format!(
                        "online completed: success={}\n{}",
                        result.success, result.detail
                    ),
                );
                set_text(
                    state.controls.status,
                    if result.success {
                        "在线任务已完成并通过校验"
                    } else {
                        "在线任务失败，请检查日志；不要将部分产物视为成功"
                    },
                );
                show_message(
                    state.root,
                    &result.detail,
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
            if control_id == ID_LANGUAGE && notification == CBN_SELCHANGE {
                apply_language(state);
                append_gui_log(state, "GUI action completed: language changed");
                return 0;
            }
            if let Some(index) = match control_id {
                ID_OPERATION_BACKUP => Some(0),
                ID_OPERATION_RESTORE => Some(1),
                ID_OPERATION_RESTORE_DATA => Some(2),
                ID_OPERATION_RESTORE_OTHER => Some(3),
                ID_OPERATION_SECONDARY => Some(4),
                ID_OPERATION_PE => Some(5),
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
                return 0;
            }
            match control_id {
                ID_REFRESH => refresh_environment(state),
                ID_READ_IMAGE => read_image(state),
                ID_BROWSE_IMAGE => browse_image(state),
                ID_PE_DIR_BROWSE => browse_pe_dir(state),
                ID_CREATE_TASK => create_task(state),
                ID_REFRESH_TASK => refresh_task_status(state),
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
                        // Read the Windows entry GUID written to the ESP at
                        // deploy time (`bcdedit /enum {current} /v`). `{current}`
                        // cannot be used here: inside PE it resolves to the PE
                        // entry, which does not exist in the store we edit.
                        let mut win_guid: Option<String> = None;
                        let guid_path = format!("{letter_char}:\\pe-exit-guid.txt");
                        if let Ok(text) = std::fs::read_to_string(&guid_path) {
                            let candidate = text.trim().to_string();
                            if !candidate.is_empty() {
                                win_guid = Some(candidate);
                                diag.push(format!("deploy GUID: {}", win_guid.as_ref().unwrap()));
                            }
                        }
                        if win_guid.is_none() {
                            diag.push("no pe-exit-guid.txt on ESP".to_string());
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
                    if found_bcd {
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
    if found_bcd {
        diag.push("exit sequence done, rebooting".to_string());
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
        let width = 440;
        // 对话框总高需预留 WS_CAPTION 标题栏（约 28-30px），否则底部
        // 「执行/取消」按钮会超出客户区被裁剪，PE 内显示不完整。
        let height = if show_name { 324 } else { 312 };
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
    let font = GetStockObject(DEFAULT_GUI_FONT);
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
                MoveWindow(edit, 8, 8, width - 16, height - 16, 1);
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
        .map_err(|_| crate::err("镜像索引必须是正整数"))
        .and_then(|index| {
            crate::pe_operation::new_request(operation, drive, image.trim(), index, main, &name)
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
        let face = wide("Segoe UI");
        let title_font = CreateFontW(-44, 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 0, 0, face.as_ptr());
        let card_font = CreateFontW(-30, 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 0, 0, face.as_ptr());
        let bar_font = CreateFontW(-18, 0, 0, 0, 500, 0, 0, 0, 1, 0, 0, 0, 0, face.as_ptr());
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
            let _ = run_cmd_to_file("wpeutil.exe reboot", None);
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
        if super::resume_pending_boot_task()? {
            return Ok(());
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
        let _ = std::fs::write("C:\\brgui-trace.txt", "before-create-window\n");
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            120,
            80,
            1020,
            760,
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        );
        let _ = std::fs::write(
            "C:\\brgui-trace.txt",
            format!("after-create-window null={}\n", window.is_null()),
        );
        if window.is_null() {
            return Err(super::err("CreateWindowExW failed"));
        }
        let _ = std::fs::write("C:\\brgui-trace.txt", "before-show-window\n");
        ShowWindow(window, SW_MAXIMIZE);
        let _ = std::fs::write("C:\\brgui-trace.txt", "message-loop-start\n");
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
