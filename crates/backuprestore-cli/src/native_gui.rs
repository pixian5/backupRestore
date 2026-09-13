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
const CB_GETLBTEXTLEN: u32 = 0x0148;
const CB_GETLBTEXT: u32 = 0x0149;
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
const SW_MAXIMIZE: i32 = 3;
const MB_OK: u32 = 0x00000000;
const MB_ICONERROR: u32 = 0x00000010;
const MB_YESNO: u32 = 0x00000004;
const MB_ICONWARNING: u32 = 0x00000030;
const MB_ICONINFORMATION: u32 = 0x00000040;
const MB_ICONQUESTION: u32 = 0x00000020;
const IDYES: i32 = 6;
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
const ID_OPERATION_PROBE: usize = 1100;
const ID_OPERATION_BACKUP: usize = 1101;
const ID_OPERATION_RESTORE: usize = 1102;
const ID_OPERATION_SECONDARY: usize = 1103;
const ID_OPERATION_PE: usize = 1104;
const ID_SOURCE: usize = 1202;
const ID_IMAGE: usize = 1203;
const ID_TARGET: usize = 1204;
const ID_INDEX: usize = 1206;
const ID_MENU: usize = 1207;
const ID_COMPRESS: usize = 1208;
const ID_INDEX_NAME_EDIT: usize = 1209;
const ID_KEEP_EDIT: usize = 1210;
const ID_STATUS: usize = 1300;
const ID_LANGUAGE_LABEL: usize = 2009;
const ID_SOURCE_DETAILS: usize = 2012;
const ID_TARGET_DETAILS: usize = 2013;
const OFN_PATHMUSTEXIST: u32 = 0x00000800;
const OFN_FILEMUSTEXIST: u32 = 0x00001000;
const CREATE_NO_WINDOW: u32 = 0x08000000;
const DEFAULT_GUI_FONT: i32 = 17;
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
const ID_PE_TITLE: usize = 1407;
const ID_PE_VERSION: usize = 1408;
const ID_PE_CLOCK: usize = 1409;
// 主窗口「PE 恢复」tab 的"重启进入 PE"按钮（PE 桌面无独立重启按钮，
// 「重启」已合并进「返回 Windows」）
const ID_PE_REBOOT_MAIN: usize = 1410;
// 主窗口「PE 恢复」tab 的"创建桌面快捷方式"按钮
const ID_PE_SHORTCUT: usize = 1411;
// 「PE 恢复」tab 启动方式单选（RAM disk / 硬盘启动）与 PE 目录名输入
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
const PM_REMOVE: u32 = 0x0001;
const WM_QUIT: u32 = 0x0012;
const ES_AUTOHSCROLL: u32 = 0x0080;
const PE_TIMER_ID: usize = 1;
// COLORREF values are 0x00BBGGRR.
const PE_BACKGROUND: u32 = 0x00553a2b; // RGB(43, 58, 85), deep blue-grey.
const PE_TITLE_TEXT: u32 = 0x00e8e8ea; // near-white.

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
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
struct Msg {
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
    time: u32,
    point: Point,
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
    fn ShowWindow(hwnd: Hwnd, command: i32) -> i32;
    fn MoveWindow(hwnd: Hwnd, x: i32, y: i32, width: i32, height: i32, repaint: i32) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn TranslateMessage(message: *const Msg) -> i32;
    fn PeekMessageW(message: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
    fn IsWindow(hwnd: Hwnd) -> i32;
    // 对话框式键盘导航：让普通窗口也能用 Tab 遍历焦点、方向键切换
    // 单选按钮、回车触发默认按钮、Esc 关闭、Alt+助记键。
    fn IsDialogMessageW(hwnd: Hwnd, message: *const Msg) -> i32;
    fn GetKeyState(key: i32) -> i16;
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
    fn SHBrowseForFolderW(info: *const BrowseInfoW) -> *mut c_void;
    fn SHGetPathFromIDListW(pidl: *const c_void, path: *mut u16) -> i32;
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
    fn CreateProcessW(
        app_name: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit_handles: i32,
        creation_flags: u32,
        environment: *mut c_void,
        current_directory: *const u16,
        startup_info: *mut StartupInfoW,
        process_information: *mut ProcessInformation,
    ) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
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
        (state.controls.operation_tabs[0], "probe"),
        (state.controls.operation_tabs[1], "backup"),
        (state.controls.operation_tabs[2], "restore"),
        (state.controls.operation_tabs[3], "secondary"),
        (state.controls.operation_tabs[4], "pe"),
        (state.controls.source, "source"),
        (state.controls.target, "target"),
        (state.controls.image, "image"),
        (state.controls.index, "index"),
        (state.controls.menu, "menu"),
        (state.controls.compress, "compress"),
        (state.controls.index_name, "index_name"),
        (state.controls.keep, "keep"),
        (GetDlgItem(state.root, ID_REFRESH as i32), "refresh"),
        (GetDlgItem(state.root, ID_READ_IMAGE as i32), "read_image"),
        (GetDlgItem(state.root, ID_CREATE_TASK as i32), "create_task"),
        (
            GetDlgItem(state.root, ID_REFRESH_TASK as i32),
            "refresh_task",
        ),
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
        (Language::Chinese, "source") => "选择备份来源或当前 Windows 分区；身份按 GUID 校验。",
        (Language::Chinese, "target") => "还原时将被格式化的目标分区；备份和探测模式不使用。",
        (Language::Chinese, "image") => {
            "镜像文件必须是绝对路径，例如 B:\\BackupRestore\\Windows.wim。"
        }
        (Language::Chinese, "index") => "选择 WIM 索引；下拉项显示索引及详细元数据。",
        (Language::Chinese, "menu") => "第二系统在 Windows 启动菜单中显示的名称。",
        (Language::Chinese, "compress") => {
            "备份镜像压缩率（仅首次创建 WIM 时生效）：\n• fast 快速（默认/推荐）：体积仅比 max 大约 10%，但耗时约 1/3.5，性价比最高\n• max 高压缩：WIM 最小，但备份明显更慢（压缩 CPU 开销大）\n• none 不压缩：WIM 最大（约等于源数据量），备份最快\n增量备份说明：镜像已存在时追加为新索引，压缩率沿用 WIM 首次创建时的设置；压缩率不影响能否增量备份。"
        }
        (Language::Chinese, "index_name") => {
            "备份索引名（写入 WIM 的 Name 字段）。默认是程序启动时间，可修改；追加备份时用于区分历史版本。"
        }
        (Language::Chinese, "keep") => {
            "保留最近 N 个索引：备份追加成功后自动删除更旧的索引（0=不清理）。删除不可恢复，请谨慎设置。"
        }
        (Language::Chinese, "refresh") => "刷新 Windows、WinRE 和可用卷信息。",
        (Language::Chinese, "read_image") => "只读解析 WIM 索引、哈希和元数据。",
        (Language::Chinese, "create_task") => "创建任务；还原操作会先显示确认对话框。",
        (Language::Chinese, "refresh_task") => "读取程序目录中的最近任务状态。",
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
            "Backup WIM compression (applies only when the WIM is first created):\n• fast (default/recommended): only ~10% larger than max, but ~1/3.5 the time — best value\n• max high compression: smallest WIM, notably slower backup (CPU cost)\n• none uncompressed: largest WIM (~source size), fastest backup\nIncremental notes: appending to an existing WIM keeps the compression set at first creation; compression does not affect whether incremental backup is available."
        }
        (Language::English, "index_name") => {
            "Backup image name (written to the WIM Name field). Defaults to program start time; editable. Appended backups use it to distinguish history."
        }
        (Language::English, "keep") => {
            "Keep latest N indexes: after a successful append, older indexes are deleted (0 = keep all). Deletion is irreversible; set with care."
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
        _ => "",
    }
}

#[derive(Clone, Debug)]
struct WimImageInfo {
    index: u32,
    name: String,
    description: String,
    version: String,
    architecture: String,
    edition: String,
    installation_type: String,
    size_bytes: Option<u64>,
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
        (Language::Chinese, "language") => "语言",
        (Language::Chinese, "refresh") => "刷新环境",
        (Language::Chinese, "read_image") => "读取镜像",
        (Language::Chinese, "browse") => "浏览…",
        (Language::Chinese, "create_task") => "创建任务",
        (Language::Chinese, "refresh_task") => "刷新任务状态",
        (Language::Chinese, "initial_status") => {
            "先点击“刷新环境”确认 Windows、恢复环境和卷身份。默认模式为无破坏探测。"
        }
        (Language::Chinese, "probe") => "探测（仅检查）",
        (Language::Chinese, "backup") => "备份",
        (Language::Chinese, "restore") => "单系统还原",
        (Language::Chinese, "secondary") => "新增第二系统",
        (Language::Chinese, "probe_hint") => {
            "探测：只检查 Windows、恢复环境和卷身份；创建任务仅生成并校验任务文件及恢复环境载荷，不备份、不还原、不格式化、不重启。"
        }
        (Language::Chinese, "backup_hint") => {
            "备份：准备完成后进入 Windows 恢复环境，使用 DISM 捕获指定源分区。"
        }
        (Language::Chinese, "restore_hint") => {
            "单系统还原：覆盖目标分区，把镜像系统作为唯一 Windows 系统启动。"
        }
        (Language::Chinese, "secondary_hint") => {
            "新增第二系统：保留当前 Windows，把镜像部署到另一个分区并新增启动项。"
        }
        (Language::Chinese, "pe") => "PE 恢复",
        (Language::Chinese, "pe_hint") => {
            "安装 PE 恢复环境：把 PE 镜像（boot.wim）部署为目标卷上的恢复环境，并新增 BCD 启动项「Windows PE (BackupRestore)」；不修改当前 Windows 默认启动。"
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
        (Language::English, "language") => "Language",
        (Language::English, "refresh") => "Refresh environment",
        (Language::English, "read_image") => "Read image",
        (Language::English, "browse") => "Browse…",
        (Language::English, "create_task") => "Create task",
        (Language::English, "refresh_task") => "Refresh task status",
        (Language::English, "initial_status") => {
            "Click Refresh environment to inspect Windows, WinRE and volume identities. Default mode is non-destructive probe."
        }
        (Language::English, "probe") => "Inspect",
        (Language::English, "backup") => "Backup",
        (Language::English, "restore") => "Restore",
        (Language::English, "secondary") => "Second system",
        (Language::English, "probe_hint") => {
            "probe is a non-destructive check: refresh the volumes, keep this mode, then create a task to validate task files and WinRE payloads. No backup, restore, format or reboot."
        }
        (Language::English, "backup_hint") => {
            "Back up the selected source partition; WinRE performs DISM Capture after preparation."
        }
        (Language::English, "restore_hint") => {
            "Single-system restore: overwrite the target partition and make it the only Windows system."
        }
        (Language::English, "secondary_hint") => {
            "Second system: keep the current Windows, deploy the image to another partition and add a boot entry."
        }
        (Language::English, "pe") => "PE recovery",
        (Language::English, "pe_hint") => {
            "Install PE recovery: deploy the PE image (boot.wim) to the target volume and add a BCD entry named \"Windows PE (BackupRestore)\". The current Windows default boot is untouched."
        }
        _ => "",
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

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
    // Win32 EDIT controls require CRLF for explicit line breaks. Keeping the
    // conversion here makes guidance/details boxes render logical lines
    // consistently instead of depending on automatic wrapping.
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let value = wide(&normalized.replace('\n', "\r\n"));
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

unsafe fn combo_item_text(hwnd: Hwnd, position: usize) -> String {
    let length = SendMessageW(hwnd, CB_GETLBTEXTLEN, position, 0);
    if length <= 0 {
        return String::new();
    }
    let mut buffer = vec![0_u16; (length as usize) + 1];
    SendMessageW(hwnd, CB_GETLBTEXT, position, buffer.as_mut_ptr() as isize);
    String::from_utf16_lossy(&buffer[..length as usize])
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
        1 => "backup",
        2 => "restore-existing",
        3 => "create-secondary",
        4 => "install-pe-entry",
        _ => "probe",
    }
}

unsafe fn set_operation_tabs(state: &State, language: Language) {
    for (index, (button, key)) in state
        .controls
        .operation_tabs
        .iter()
        .zip(["probe", "backup", "restore", "secondary", "pe"])
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
        (Language::Chinese, "restore-existing") => "单系统还原",
        (Language::Chinese, "create-secondary") => "新增第二系统",
        (Language::Chinese, "install-pe-entry") => "安装 PE 恢复环境",
        (Language::English, "probe") => "Inspect",
        (Language::English, "backup") => "Backup",
        (Language::English, "restore-existing") => "Restore",
        (Language::English, "create-secondary") => "Second system",
        (Language::English, "install-pe-entry") => "Install PE recovery",
        _ => "",
    }
}

fn operation_hint_key(operation: &str) -> &'static str {
    match operation {
        "backup" => "backup_hint",
        "restore-existing" => "restore_hint",
        "create-secondary" => "secondary_hint",
        "install-pe-entry" => "pe_hint",
        _ => "probe_hint",
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
        "restore-existing" | "create-secondary" | "install-pe-entry"
    );
    let show_index = matches!(operation, "restore-existing" | "create-secondary");
    // WIM 压缩率（max/fast/none）只对备份首次创建有意义，仅「备份」tab 显示。
    let show_compress = operation == "backup";
    // 「第二系统名称」输入框及其标签只在「新增第二系统」tab 显示；
    // 「PE 恢复」tab 有自己的「PE 启动项名称」输入框，若此处也显示 menu，
    // 其布局位置（field_x+500, secondary_y）会恰好覆盖右下角「创建快捷方式」按钮。
    let show_menu = operation == "create-secondary";
    let set_visible = |hwnd: Hwnd, visible: bool| {
        ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
    };
    let set_child_visible = |id: i32, visible: bool| {
        set_visible(GetDlgItem(state.root, id), visible);
    };

    set_visible(state.controls.image, show_image);
    set_child_visible(ID_BROWSE_IMAGE as i32, show_image);
    set_child_visible(2004, show_image);

    set_visible(state.controls.index, show_index);
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
    // 「重启进入 PE」「创建快捷方式」只在 PE 恢复 tab 显示
    set_child_visible(ID_PE_REBOOT_MAIN as i32, operation == "install-pe-entry");
    set_child_visible(ID_PE_SHORTCUT as i32, operation == "install-pe-entry");
    // 启动方式单选与目录行只在「PE 恢复」tab 显示；硬盘启动模式下隐藏目录路径行
    let pe_mode_ram = IsDlgButtonChecked(state.root, ID_PE_MODE_RAM as i32) != 0;
    let pe_visible = operation == "install-pe-entry";
    append_gui_log(
        state,
        &format!("PE visibility: op={operation} pe_visible={pe_visible} ram_checked={pe_mode_ram}"),
    );
    set_child_visible(ID_PE_MODE_RAM as i32, pe_visible);
    set_child_visible(ID_PE_MODE_DISK as i32, pe_visible);
    // 「PE 启动项名称」输入框两种模式通用，在「PE 恢复」tab 始终显示
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
    let source_combo_y = 100;
    let source_details_y = source_combo_y + 36;
    let target_combo_y = source_details_y + details_height + row_gap;
    let target_details_y = target_combo_y + 36;
    let target_visible = matches!(
        selected_operation(state),
        "restore-existing" | "create-secondary" | "install-pe-entry"
    );
    let image_visible = selected_operation(state) != "probe";
    let index_visible = matches!(
        selected_operation(state),
        "restore-existing" | "create-secondary"
    );
    let last_details_y = if target_visible {
        target_details_y + details_height
    } else {
        source_details_y + details_height
    };
    // 「PE 恢复」tab 的启动方式单选占三行（约 90px：单选行 + 启动项名行 + 目录行），后续 status 下移
    let pe_extra_y = if selected_operation(state) == "install-pe-entry" {
        90
    } else {
        0
    };
    let status_y = last_details_y + 16 + pe_extra_y;
    let image_y = status_y + 84;
    let secondary_y = image_y + 34;
    let buttons_y = if index_visible {
        secondary_y + 40
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
    reposition(state.controls.status, 20, status_y, client_width - 40, 64);

    // 「PE 恢复」tab：行1 RAM disk 单选 + PE 目录路径（完整路径，同排）；
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
    // 「新增第二系统」tab 的 WIM 索引下拉框：宽度 380，右端到 field_x+380=560，
    // 避免与右侧「第二系统名称」标签（2008，x=570 起）重叠（历史布局 440 会遮住下拉框右缘）。
    let index_width = if selected_operation(state) == "create-secondary" {
        380
    } else {
        field_width
    };
    reposition(state.controls.index, field_x, secondary_y, index_width, 220);
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
    // 「PE 恢复」tab 专属按钮（重启进入 PE / 创建快捷方式）：
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
    let (source, target) = match (language, operation) {
        // Keep the left labels short enough for the narrow role column. The
        // detail boxes below carry the complete safety explanation, so a
        // label should identify the purpose without wrapping into a clipped
        // third line on a maximized VM window.
        (Language::Chinese, "backup") => ("源卷（备份来源）", "目标卷（镜像位置）"),
        (Language::Chinese, "restore-existing") => ("源卷（当前系统）", "目标卷（覆盖还原）"),
        (Language::Chinese, "create-secondary") => ("源卷（保留系统）", "目标卷（第二系统）"),
        (Language::Chinese, "install-pe-entry") => ("源卷（不使用）", "目标卷（PE 安装位置）"),
        (Language::Chinese, _) => ("源卷（检查对象）", "目标卷（不使用）"),
        (Language::English, "backup") => ("Source (backup)", "Target (image)"),
        (Language::English, "restore-existing") => ("Source (current)", "Target (overwrite)"),
        (Language::English, "create-secondary") => ("Source (keep)", "Target (second system)"),
        (Language::English, "install-pe-entry") => ("Source (unused)", "Target (PE install)"),
        (Language::English, _) => ("Source (inspect)", "Target (unused)"),
    };
    set_child_text(state.root, 2003, source);
    set_child_text(state.root, 2005, target);
}

unsafe fn select_operation(state: &mut State, index: usize) {
    state.operation_index = index.min(4);
    set_operation_tabs(state, selected_language(state));
    if selected_operation(state) == "restore-existing"
        && let Some(source) = selected_drive_letter(state, state.controls.source)
    {
        select_drive(state, state.controls.target, &source);
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
            "目标卷用途：将被格式化并写入镜像；单系统还原必须选择与源卷相同的分区。"
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
            "Purpose: current Windows to keep; a secondary-system target must differ from it."
        }
        (Language::English, "source", _) => {
            "Purpose: probe only checks the current Windows volume identity; it never writes the volume."
        }
        (Language::English, "target", "restore-existing") => {
            "Purpose: will be formatted and receive the image; single-system restore must select the source partition."
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

fn json_text(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|item| {
            item.as_str()
                .map(ToOwned::to_owned)
                .or_else(|| item.as_u64().map(|number| number.to_string()))
                .or_else(|| item.as_i64().map(|number| number.to_string()))
                .or_else(|| item.as_bool().map(|flag| flag.to_string()))
        })
        .unwrap_or_default()
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

fn parse_wim_images(output: &str) -> Result<Vec<WimImageInfo>, String> {
    let value: serde_json::Value = serde_json::from_str(output)
        .map_err(|error| format!("WIM metadata JSON parse failed: {error}"))?;
    let items = wim_image_items(&value)?;
    let mut images = Vec::with_capacity(items.len());
    for item in items {
        let index = json_text(&item, "ImageIndex")
            .parse::<u32>()
            .map_err(|_| "WIM metadata contains an invalid image index".to_string())?;
        if index == 0 {
            return Err("WIM metadata contains image index 0".to_string());
        }
        let size_bytes = json_text(&item, "ImageSize").parse::<u64>().ok();
        images.push(WimImageInfo {
            index,
            name: json_text(&item, "ImageName"),
            description: json_text(&item, "ImageDescription"),
            version: json_text(&item, "ImageVersion"),
            architecture: json_text(&item, "Architecture"),
            edition: json_text(&item, "EditionId"),
            installation_type: json_text(&item, "InstallationType"),
            size_bytes,
        });
    }
    if images.is_empty() {
        return Err("WIM contains no selectable image indexes".to_string());
    }
    Ok(images)
}

fn wim_image_items(value: &serde_json::Value) -> Result<Vec<serde_json::Value>, String> {
    if let Some(items) = value.as_array() {
        return Ok(items.clone());
    }
    if value.get("ImageIndex").is_some() || value.get("imageIndex").is_some() {
        return Ok(vec![value.clone()]);
    }
    if let Some(images) = value.get("images") {
        return wim_image_items(images);
    }
    Err("WIM metadata did not return an image object, array or images wrapper".to_string())
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

fn format_bytes(size: Option<u64>) -> String {
    let Some(size) = size else {
        return "?".to_string();
    };
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = size as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
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

/// 压缩率下拉显示文本（按语言；用户指定 verbatim）。
/// 索引 0/1/2 固定对应 DISM 术语 max/fast/none（取值映射见 create_task）。
fn compress_level_labels(language: Language) -> [&'static str; 3] {
    if language == Language::Chinese {
        [
            "LZX（文件最小，耗时特别长，CPU占用特别多）",
            "XPRESS（推荐！文件稍大，非常快，CPU占用低）",
            "不压缩（最快，文件最大，几乎不耗CPU）",
        ]
    } else {
        [
            "LZX (smallest file, very slow, highest CPU)",
            "XPRESS (recommended! slightly larger, very fast, low CPU)",
            "No compression (fastest, largest file, almost no CPU)",
        ]
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
    // 压缩率下拉随语言重填（reset + 重填 + 保持原选择，无选择时默认 fast）
    let compress_position = combo_selection(state.controls.compress);
    reset_combo(state.controls.compress);
    for label in compress_level_labels(language) {
        add_combo_item(state.controls.compress, label);
    }
    SendMessageW(
        state.controls.compress,
        CB_SETCURSEL,
        compress_position.unwrap_or(1),
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

fn quote_argument(value: &str) -> String {
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
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
/// 对话框文案语言：0=中文 1=English（模态期间单实例，用静态即可）。
static CHOICE_LANGUAGE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

unsafe extern "system" fn window_proc_system_choice(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    if message == WM_CREATE {
        let english = CHOICE_LANGUAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
        let font = GetStockObject(DEFAULT_GUI_FONT) as Handle;
        let body = create_control(
            hwnd,
            "STATIC",
            if english {
                "The target volume is the currently running system. Backing up or restoring the system partition must run offline in PE or Windows RE. Choose how to proceed:"
            } else {
                "目标分区是当前正在运行的系统。系统分区的备份与还原必须在 PE 或 Windows RE 中离线执行。请选择处理方式："
            },
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
            DestroyWindow(hwnd);
            return 0;
        }
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
                *state_ptr = result;
                DestroyWindow(hwnd);
                return 0;
            }
        }
    }
    if message == WM_DESTROY {
        PostQuitMessage(0);
        return 0;
    }
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
    let result_box = Box::new(0i32);
    SetWindowLongPtrW(dialog, GWLP_USERDATA, Box::into_raw(result_box) as isize);
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
    while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
        // 对话框式键盘导航（Tab 遍历 / 回车默认按钮 / Esc / 方向键切换单选）。
        if IsDialogMessageW(dialog, &message) == 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    let ptr = GetWindowLongPtrW(dialog, GWLP_USERDATA) as *mut i32;
    let result = if ptr.is_null() { 0 } else { *ptr };
    if !ptr.is_null() {
        drop(Box::from_raw(ptr));
    }
    SetWindowLongPtrW(dialog, GWLP_USERDATA, 0);
    // 恢复主窗口
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

unsafe fn browse_image(state: &State) {
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
        set_text(
            state.controls.image,
            &String::from_utf16_lossy(&buffer[..length]),
        );
        append_gui_log(
            state,
            "GUI action completed: image path selected from file dialog",
        );
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
    let mut info = BrowseInfoW {
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

/// Run `bcdedit.exe` (or any command) synchronously via a visible-free cmd
/// wrapper and return its exit code. Output is redirected to `out_file`
/// (optional) so GUID-returning commands can be parsed afterwards.
fn run_cmd_to_file(command: &str, out_file: Option<&std::path::Path>) -> u32 {
    run_cmd_to_file_timeout(command, out_file, 30000)
}

/// 同 `run_cmd_to_file`，但等待超时可指定（毫秒）。PE 内备份/还原/格式化
/// 等 DISM/DiskPart 操作可能超过默认 30 秒，必须用长超时版本。
fn run_cmd_to_file_timeout(
    command: &str,
    out_file: Option<&std::path::Path>,
    timeout_ms: u32,
) -> u32 {
    let mut full = String::from("cmd.exe /c ");
    full.push_str(command);
    if let Some(path) = out_file {
        let quoted = format!("\"{}\"", path.to_string_lossy());
        full.push_str(" > ");
        full.push_str(&quoted);
        full.push_str(" 2>&1");
    }
    let mut command_line: Vec<u16> = full.encode_utf16().chain(Some(0)).collect();
    let mut startup: StartupInfoW = unsafe { std::mem::zeroed() };
    startup.cb = size_of::<StartupInfoW>() as u32;
    let mut process: ProcessInformation = unsafe { std::mem::zeroed() };
    let created = unsafe {
        CreateProcessW(
            null(),
            command_line.as_mut_ptr(),
            null_mut(),
            null_mut(),
            0,
            CREATE_NO_WINDOW,
            null_mut(),
            null_mut(),
            &mut startup,
            &mut process,
        )
    };
    if created == 0 {
        return u32::MAX;
    }
    unsafe {
        WaitForSingleObject(process.process, timeout_ms);
        let mut code: u32 = 0;
        GetExitCodeProcess(process.process, &mut code);
        CloseHandle(process.thread);
        CloseHandle(process.process);
        code
    }
}

/// PE 恢复安装入口：按启动方式单选分派到 RAM disk（目录，不占分区）
/// 或硬盘启动（独立分区）。两种模式可共存，启动项名称不同。
/// 测试钩子（开发/验收用）：启动时若存在 C:\br-test.json 则读取并自动
/// 设置 PE 恢复参数；auto_install=true 时延迟触发安装（确认框自动接受）。
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
    // 1. 操作模式：默认 PE 恢复；支持 "tab":"backup"（备份）/"restore"/"secondary" 等
    let tab = json.get("tab").and_then(|v| v.as_str()).unwrap_or("pe");
    let op_index = match tab {
        "backup" => 1,
        "restore" => 2,
        "secondary" => 3,
        _ => 4,
    };
    select_operation(state, op_index);
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
            &if language == Language::English {
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
                    "Install the PE recovery environment (RAM disk) to {dir_path}?\n\n- Copy the PE WIM to {dir_path}\\sources\\boot.wim\n- Ensure {dir_path}\\boot\\boot.sdi\n- Add boot entry \"{entry_name}\" to the boot menu\n\nNo extra partition is used. The current Windows default boot is NOT changed. Continue?"
                )
            } else {
                format!(
                    "以 RAM disk 方式安装 PE 恢复环境到 {dir_path}？\n\n- 复制 PE 镜像到 {dir_path}\\sources\\boot.wim\n- 确保 {dir_path}\\boot\\boot.sdi 存在\n- 在启动菜单新增「{entry_name}」启动项\n\n不占用独立分区。不修改当前 Windows 默认启动。是否继续？"
                )
            },
            if language == Language::English {
                "Install PE recovery"
            } else {
                "安装 PE 恢复环境"
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
        if let Some(src) = sdi_source {
            if let Err(error) = std::fs::copy(&src, &sdi_dest) {
                append_gui_log(state, &format!("PE install failed: copy boot.sdi: {error}"));
            }
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
        if code != 0 {
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
                    &if language == Language::English {
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
        if code != 0 {
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
            "GUI action completed: PE recovery installed (RAM disk wim -> {dir_path}\\sources\\boot.wim, BCD entry {os_guid_path})"
        ),
    );
    show_message(
        state.root,
        &if language == Language::English {
            format!(
                "PE recovery environment (RAM disk) installed to {dir_path}.\n\nThe boot menu now has \"{entry_name}\". Reboot and choose it from the menu to enter the recovery desktop. The current Windows default boot is unchanged."
            )
        } else {
            format!(
                "PE 恢复环境（RAM disk）已安装到 {dir_path}。\n\n启动菜单已新增「{entry_name}」。重启后从菜单选择即可进入恢复桌面。当前 Windows 默认启动未改动。"
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
                &if language == Language::English {
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
            &if language == Language::English {
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
            &if language == Language::English {
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
                    "Install the PE recovery environment (hard disk boot) to partition {drive_char}:?\n\n- The partition WILL BE FORMATTED (all data on it is lost)\n- Apply the PE WIM to {drive_char}:\\\n- Add boot entry \"{entry_name}\" to the boot menu\n\nThe current Windows default boot is NOT changed. Continue?"
                )
            } else {
                format!(
                    "以硬盘启动方式安装 PE 恢复环境到分区 {drive_char}:？\n\n- 该分区将被格式化（数据全部丢失！）\n- 将 PE 镜像展开到 {drive_char}:\\\n- 在启动菜单新增「{entry_name}」启动项\n\n不修改当前 Windows 默认启动。是否继续？"
                )
            },
            if language == Language::English {
                "Install PE recovery"
            } else {
                "安装 PE 恢复环境"
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
    // 1. 格式化目标分区（diskpart，快速 NTFS）
    let script = format!("select volume {drive_char}\nformat fs=ntfs quick\n");
    let script_file = state
        .executable_dir
        .join("_pe-format.txt")
        .to_string_lossy()
        .to_string();
    let _ = std::fs::write(&script_file, &script);
    let format_code = run_cmd_to_file_timeout(
        // script_file 是程序所在目录下的临时脚本，程序装在含空格目录
        // （如 C:\Users\张三\My Apps\）时路径必须加引号，否则 cmd 会把
        // 路径拆成两个参数导致 diskpart 找不到脚本（与 exit=87 同类问题）。
        &format!("cmd /c diskpart /s \"{script_file}\" > NUL 2>&1"),
        None,
        300000,
    );
    let _ = std::fs::remove_file(&script_file);
    if format_code != 0 {
        append_gui_log(
            state,
            &format!("PE hard disk install failed: format code={format_code}"),
        );
        show_message(
            state.root,
            &if language == Language::English {
                format!("Failed to format partition {drive_char}: (code {format_code}).")
            } else {
                format!("格式化分区 {drive_char}: 失败（退出码 {format_code}）。")
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
    // 2. Apply PE WIM（索引 1 = WinPE）到分区
    let apply_code = run_cmd_to_file_timeout(
        &format!(
            "dism /Apply-Image /ImageFile:\"{image_path}\" /Index:1 /ApplyDir:{drive_char}:\\"
        ),
        None,
        600000,
    );
    if apply_code != 0 {
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
        if code != 0 {
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
                    &if language == Language::English {
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
        if code != 0 {
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
            "GUI action completed: PE recovery installed (hard disk -> {drive_char}:\\, BCD entry {os_guid_path})"
        ),
    );
    show_message(
        state.root,
        &if language == Language::English {
            format!(
                "PE recovery environment (hard disk boot) installed to partition {drive_char}:.\n\nThe boot menu now has \"{entry_name}\". Reboot and choose it from the menu to enter the recovery desktop. The current Windows default boot is unchanged."
            )
        } else {
            format!(
                "PE 恢复环境（硬盘启动）已安装到分区 {drive_char}:。\n\n启动菜单已新增「{entry_name}」。重启后从菜单选择即可进入恢复桌面。当前 Windows 默认启动未改动。"
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
/// needed). bcdedit writes GBK/UTF-16LE output on Chinese/Japanese systems,
/// so detect the interleaved-NUL pattern (or a BOM) and decode before
/// scanning.
fn extract_bcd_guid(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let nul_count = bytes.iter().filter(|&&b| b == 0).count();
    let text =
        if bytes.starts_with(&[0xFF, 0xFE]) || (bytes.len() >= 2 && nul_count > bytes.len() / 4) {
            let body = if bytes.starts_with(&[0xFF, 0xFE]) {
                &bytes[2..]
            } else {
                &bytes[..]
            };
            let units: Vec<u16> = body
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(&bytes).to_string()
        };
    let start = text.find('{')?;
    let end = text[start..].find('}')? + start;
    Some(text[start + 1..end].to_string())
}

/// 「创建快捷方式」：在用户桌面创建指向 BackupRestore.exe 本身的快捷方式
/// 「BackupRestore.lnk」（无参数，双击直接打开主 GUI），方便日常启动程序。
unsafe fn pe_create_shortcut(state: &State) {
    let language = selected_language(state);
    let executable =
        std::env::current_exe().unwrap_or_else(|_| state.executable_dir.join("BackupRestore.exe"));
    let executable_path = executable.to_string_lossy().to_string();
    let ps1 = state.executable_dir.join("_create-pe-shortcut.ps1");
    // Parallels 场景：用户实际桌面是 Mac 桌面映射 C:\Mac\Home\Desktop
    // （Known Folder 已重定向，Windows 物理桌面不显示）。候选路径全部
    // 创建、去重：Known Folder 桌面 + Mac 桌面映射 + 当前用户物理桌面。
    let script = format!(
        "$paths = @()\n\
         $d1 = [Environment]::GetFolderPath('Desktop')\n\
         $d2 = 'C:\\Mac\\Home\\Desktop'\n\
         $d3 = Join-Path $env:USERPROFILE 'Desktop'\n\
         foreach ($p in @($d1, $d2, $d3)) {{ if ($p -and (Test-Path $p) -and ($paths -notcontains $p)) {{ $paths += $p }} }}\n\
         foreach ($p in $paths) {{\n\
         \x20 $s = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $p 'BackupRestore.lnk'))\n\
         \x20 $s.TargetPath = '{executable_path}'\n\
         \x20 $s.IconLocation = '{executable_path},0'\n\
         \x20 $s.Save()\n\
         }}\n\
         'TARGETS=' + ($paths -join ';')\n"
    );
    // PowerShell 5.1 按 ANSI 读无 BOM 的 ps1，中文路径会乱码，加 UTF-8 BOM。
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(script.as_bytes());
    let _ = std::fs::write(&ps1, bytes);
    let command = format!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
        ps1.to_string_lossy()
    );
    append_gui_log(state, &format!("PE create shortcut: {command}"));
    let out_file = state.executable_dir.join("_create-pe-shortcut.out");
    let code = run_cmd_to_file(&command, Some(&out_file));
    if let Ok(text) = std::fs::read_to_string(&out_file) {
        append_gui_log(
            state,
            &format!("PE create shortcut output: {}", text.trim()),
        );
    }
    append_gui_log(state, &format!("PE create shortcut: exit code={code}"));
    if code == 0 {
        show_message(
            state.root,
            &if language == Language::English {
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
                format!("Failed to create the shortcut (exit code {code}).")
            } else {
                format!("创建快捷方式失败（退出码 {code}）。")
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

/// 「重启进入 PE」：设置 {bootmgr} bootsequence 指向已安装的 PE 启动项，
/// 配置成功后**立即自动重启**（无需用户手动重启），由 bootmgr 自动进入 PE
/// 恢复环境（无需手动选菜单）。PE 恢复桌面启动时会自动清除该 bootsequence
/// （见 pe_self_clean_bootsequence），因此之后再重启会正常回到主系统，
/// 全程无需用户操作。
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
            &if language == Language::English {
                "No PE entry installed yet. Open the PE Recovery tab, choose a target volume and a PE WIM, then click Create Task first."
            } else {
                "尚未安装 PE 恢复环境。请先在「PE 恢复」页选择目标卷和 PE 镜像，点击「创建任务」完成安装。"
            },
            if language == Language::English {
                "PE entry missing"
            } else {
                "未安装 PE 恢复环境"
            },
            MB_OK | MB_ICONINFORMATION,
        );
        return;
    };
    // 1. 写 PE 任务配置到 ESP（PE 启动按配置自动执行，无需用户操作）
    let mount_code = run_cmd_to_file("mountvol.exe S: /S", None);
    append_gui_log(
        state,
        &format!("PE reboot to PE: mountvol S: code={mount_code}"),
    );
    let task = "clean_bootsequence\nverify\nreboot\n";
    let write_ok = std::fs::write("S:\\pe-task.txt", task).is_ok();
    append_gui_log(
        state,
        &format!("PE reboot to PE: write pe-task.txt ok={write_ok}"),
    );
    // 2. 设置 bootsequence 引导进 PE
    let command = format!("bcdedit.exe /set {{bootmgr}} bootsequence {{{guid}}}");
    append_gui_log(state, &format!("PE reboot to PE: {command}"));
    let code = run_cmd_to_file(&command, None);
    append_gui_log(state, &format!("PE reboot to PE: exit code={code}"));
    if code == 0 && write_ok {
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
            if fallback != 0 {
                // 两条路都失败：提示用户手动重启
                show_message(
                    state.root,
                    &if language == Language::English {
                        format!(
                            "PE task configured and boot sequence set, but auto-reboot failed (ExitWindowsEx and shutdown.exe both failed). Please restart manually."
                        )
                    } else {
                        format!(
                            "已写入 PE 任务配置并设置一次性启动项，但自动重启失败（ExitWindowsEx 与 shutdown.exe 均失败），请手动重启进入 PE。"
                        )
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

/// 桌面快捷方式入口（`--pe-reboot`）：无主窗口，设置 bootsequence 指向
/// 已安装的 PE 恢复环境，用无父窗口消息框提示结果。非提升进程先提权重启。
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
            "尚未安装 PE 恢复环境。请先打开 BackupRestore，在「PE 恢复」页安装后再使用本快捷方式。\n\nPE recovery environment not installed yet. Install it from the PE Recovery tab first.",
        );
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            wide("未安装 PE 恢复环境").as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
        return Ok(());
    };
    let command = format!("bcdedit.exe /set {{bootmgr}} bootsequence {{{guid}}}");
    let code = run_cmd_to_file(&command, None);
    if code == 0 {
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

unsafe fn create_task(state: &State) {
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
    let index = selected_wim_index(state);
    if matches!(operation.as_str(), "restore-existing" | "create-secondary") && index.is_none() {
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
    let index = index.unwrap_or(1).to_string();
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
    let source_drive = match selected_or_error(
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
    };
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
    if matches!(operation.as_str(), "restore-existing" | "create-secondary")
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
    let image_path = get_text(state.controls.image).trim().to_string();
    if operation != "probe"
        && let Err(error) = backuprestore_core::validate_absolute_path(&image_path)
    {
        append_gui_log(
            state,
            &format!("GUI action blocked: invalid image path: {error}"),
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
    if matches!(operation.as_str(), "restore-existing" | "create-secondary")
        && show_message(
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
        ) != IDYES
    {
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
    if matches!(operation.as_str(), "restore-existing" | "create-secondary")
        && PathBuf::from(&image_path).is_file()
    {
        let index_number = index.parse::<u32>().unwrap_or(1);
        let metadata_check = crate::read_index_metadata(Path::new(&image_path), index_number);
        let hash_ok = match &metadata_check {
            Ok(metadata) => backuprestore_core::sha256_file(&image_path)
                .map(|actual| actual.eq_ignore_ascii_case(&metadata.image_sha256))
                .unwrap_or(false),
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
        if !prompt.is_empty()
            && show_message(state.root, &prompt, &title, MB_YESNO | MB_ICONWARNING) != IDYES
        {
            append_gui_log(
                state,
                &format!("restore cancelled: metadata check declined; operation={operation}"),
            );
            return;
        }
        force_restore_hash = metadata_check.is_err() || !hash_ok;
    }
    // ===== 智能分流：备份/还原目标为「当前活动系统」→ 弹窗选 PE/RE/取消 =====
    // 判断用「当前活动系统」（%SystemDrive%），不是「任何含 Windows 的卷」：
    // 双系统时另一个 Windows 卷并未运行，可直接在线备份/还原。
    let compress = if operation == "backup" {
        // 下拉显示说明文本（LZX/XPRESS/不压缩），取值按索引映射回 DISM 术语
        match combo_selection(state.controls.compress) {
            Some(0) => "max".to_string(),
            Some(2) => "none".to_string(),
            _ => "fast".to_string(),
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
        if affected_upper == system_upper {
            // 当前活动系统：必须离线处理，弹窗让用户选 PE / RE / 取消。
            // 测试钩子配置了 system_drive_choice 时直接采用（跳过弹窗）。
            let choice = match state.test_drive_choice {
                Some(c) => c,
                None => ask_system_drive_handler(state.root, language),
            };
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
                        let confirm = show_message(
                            state.root,
                            &if language == Language::English {
                                "A backup task will be created and prepared with administrator rights, then the system will restart into Windows RE to run it. Continue?"
                            } else {
                                "将创建备份任务并以管理员权限准备，准备完成后系统会重启进入 Windows RE 执行备份。是否继续？"
                            },
                            if language == Language::English {
                                "Enter Windows RE"
                            } else {
                                "进入 Windows RE"
                            },
                            MB_YESNO | MB_ICONQUESTION,
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
            );
            return;
        }
    }
    let executable =
        std::env::current_exe().unwrap_or_else(|_| state.executable_dir.join("BackupRestore.exe"));
    let mut arguments = vec![
        "prepare".to_string(),
        "--operation".to_string(),
        operation.clone(),
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
        // 压缩率下拉：值即 max/fast/none（DISM 术语，语言无关，已在上方分流处解析）。
        arguments.extend(["--compress".to_string(), compress]);
        // 备份索引名（用户输入，默认程序启动时间）与保留最近 N 个索引。
        arguments.extend(["--image-name".to_string(), image_name]);
        if let Some(keep) = keep_indexes {
            arguments.extend(["--keep-indexes".to_string(), keep.to_string()]);
        }
    }
    if matches!(operation.as_str(), "restore-existing" | "create-secondary") {
        arguments.push("--allow-destructive".to_string());
        // 档案缺失/哈希不匹配且用户已确认 → 跳过哈希校验。
        if force_restore_hash {
            arguments.push("--force-restore-hash".to_string());
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
    let result = ShellExecuteW(
        state.root,
        runas.as_ptr(),
        executable_wide.as_ptr(),
        params.as_ptr(),
        null(),
        SW_HIDE,
    );
    if result <= 32 {
        append_gui_log(
            state,
            &format!(
                "GUI action failed: elevated prepare launch; operation={operation}; ShellExecute={result}"
            ),
        );
        set_text(
            state.controls.status,
            &if language == Language::English {
                format!(
                    "Could not start the elevated preparation script (ShellExecute code {result})."
                )
            } else {
                format!("无法启动管理员准备脚本，ShellExecute 错误码：{result}")
            },
        );
        // 状态栏可能被 tab 控件覆盖不可见，必须用弹窗给出明确反馈。
        show_message(
            state.root,
            &if language == Language::English {
                format!(
                    "Could not start the elevated preparation script (ShellExecute code {result}). No task, WinRE or reboot was requested."
                )
            } else {
                format!(
                    "无法启动管理员准备脚本（ShellExecute 错误码 {result}）。未创建任务、未修改 WinRE、未请求重启。"
                )
            },
            if language == Language::English {
                "Preparation launch failed"
            } else {
                "准备启动失败"
            },
            MB_OK | MB_ICONERROR,
        );
    } else {
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
}

// ===================== 智能分流：进入 PE / 在线直接执行 =====================

/// 智能分流「进入 PE」：把备份/还原动作写入 ESP 的 S:\pe-task.txt，设置
/// bootsequence 指向已安装 PE，自动重启。PE 启动后按配置自动执行备份/还原，
/// 结尾 `reboot` 使 PE 执行完自动重启回 Windows，全程无需用户操作。
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
            &if language == Language::English {
                "No PE entry installed yet. Open the PE Recovery tab, install the PE first, then retry."
            } else {
                "尚未安装 PE 恢复环境。请先在「PE 恢复」页完成 PE 安装，再重试。"
            },
            if language == Language::English {
                "PE entry missing"
            } else {
                "未安装 PE 恢复环境"
            },
            MB_OK | MB_ICONINFORMATION,
        );
        return;
    };
    // PE 任务解析按空白分词：路径含空格会拆坏，先拦截提示
    if image_path.contains(' ') {
        show_message(
            state.root,
            &if language == Language::English {
                "The PE task channel does not support spaces in the WIM path yet. Move the image to a path without spaces and retry."
            } else {
                "PE 自动执行通道暂不支持带空格的镜像路径。请把 WIM 放到无空格路径后重试。"
            },
            if language == Language::English {
                "Path not supported"
            } else {
                "路径暂不支持"
            },
            MB_OK | MB_ICONWARNING,
        );
        return;
    }
    let mount_code = run_cmd_to_file("mountvol.exe S: /S", None);
    append_gui_log(
        state,
        &format!("schedule_pe_task: mountvol S: code={mount_code}"),
    );
    let task = if operation == "backup" {
        format!("backup {source_drive} \"{image_path}\"\nreboot\n")
    } else {
        format!("restore \"{image_path}\" {target_drive}\nreboot\n")
    };
    let write_ok = std::fs::write("S:\\pe-task.txt", &task).is_ok();
    append_gui_log(
        state,
        &format!("schedule_pe_task: write pe-task.txt ok={write_ok} task={task}"),
    );
    let command = format!("bcdedit.exe /set {{bootmgr}} bootsequence {{{guid}}}");
    append_gui_log(state, &format!("schedule_pe_task: {command}"));
    let code = run_cmd_to_file(&command, None);
    append_gui_log(
        state,
        &format!("schedule_pe_task: bcdedit exit code={code}"),
    );
    if code == 0 && write_ok {
        append_gui_log(state, "schedule_pe_task: bootsequence set, auto reboot now");
        if ExitWindowsEx(EWX_REBOOT, 0) == 0 {
            let fallback = run_cmd_to_file("shutdown.exe /r /t 0 /f", None);
            append_gui_log(
                state,
                &format!("schedule_pe_task: shutdown.exe fallback code={fallback}"),
            );
            if fallback != 0 {
                show_message(
                    state.root,
                    &if language == Language::English {
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

/// 在线备份/还原的后台参数（线程内只读，避免跨线程借用 State）。
struct OnlineOpParams {
    operation: String,
    source_drive: String,
    target_drive: String,
    image_path: String,
    index: String,
    compress: String,
    image_name: String,
    keep_indexes: Option<u32>,
}

/// 在线执行结果（后台线程写完，主窗口 WM_APP_ONLINE_DONE 读取显示）。
static ONLINE_RESULT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 后台执行 DISM 在线备份/还原（数据盘/非活动系统），完成后回主窗口消息。
/// 备份：镜像不存在 → /Capture-Image；存在 → /Append-Image 追加新索引；
/// 成功后按保留策略删除最旧索引。还原：dism /Apply-Image。
fn execute_online(params: &OnlineOpParams) -> String {
    let out = std::env::temp_dir().join("br-online-op.txt");
    let command = if params.operation == "backup" {
        let name = if params.image_name.is_empty() {
            "Windows Backup".to_string()
        } else {
            params.image_name.clone()
        };
        // 索引名可能带空格（默认是"2026-09-13 20:43"这类时间），
        // 命令是拼成字符串交给 cmd 执行的，/Name 必须加引号，
        // 否则 DISM 把带空格的名称拆成两个参数报 87（参数错误）。
        if std::path::Path::new(&params.image_path).is_file() {
            format!(
                "dism.exe /Append-Image /ImageFile:\"{}\" /CaptureDir:{}:\\ /Name:\"{}\"",
                params.image_path, params.source_drive, name
            )
        } else {
            format!(
                "dism.exe /Capture-Image /ImageFile:\"{}\" /CaptureDir:{}:\\ /Name:\"{}\" /Compress:{}",
                params.image_path, params.source_drive, name, params.compress
            )
        }
    } else {
        format!(
            "dism.exe /Apply-Image /ImageFile:\"{}\" /Index:{} /ApplyDir:{}:\\",
            params.image_path, params.index, params.target_drive
        )
    };
    let code = run_cmd_to_file_timeout(&command, Some(&out), 600000);
    let mut summary = format!("[ONLINE {}] exit={}\n", params.operation, code);
    if let Ok(text) = std::fs::read_to_string(&out) {
        summary.push_str(&text);
    } else {
        summary.push_str("(no output captured)\n");
    }
    // 保留最近 N 个索引：备份追加成功后连续删除最旧索引（Index 1）直至剩余 N 个。
    if code == 0 && params.operation == "backup" && params.keep_indexes.is_some() {
        let keep = params.keep_indexes.unwrap_or(0).max(1);
        let mut removed = 0_u32;
        for _ in 0..64 {
            let count = rust_cli_output(&["wim-info", &params.image_path])
                .ok()
                .and_then(|output| parse_wim_images(&output).ok())
                .map(|images| images.len())
                .unwrap_or(0);
            if count <= keep as usize {
                break;
            }
            let del_out = std::env::temp_dir().join("br-online-del.txt");
            let del = format!(
                "dism.exe /English /Delete-Image /ImageFile:\"{}\" /Index:1",
                params.image_path
            );
            let del_code = run_cmd_to_file_timeout(&del, Some(&del_out), 120000);
            if del_code != 0 {
                summary.push_str(&format!(
                    "[keep] deleting oldest index failed: exit={del_code}\n"
                ));
                break;
            }
            removed += 1;
        }
        if removed > 0 {
            summary.push_str(&format!(
                "[keep] removed {removed} older index(es), kept latest {keep}\n"
            ));
        }
    }
    summary
}

/// 启动在线备份/还原后台线程（非当前活动系统的卷可直接在线处理）。
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
) {
    let language = selected_language(state);
    let params = OnlineOpParams {
        operation: operation.to_string(),
        source_drive: source_drive.to_string(),
        target_drive: target_drive.to_string(),
        image_path: image_path.to_string(),
        index: index.to_string(),
        compress: compress.to_string(),
        image_name: image_name.to_string(),
        keep_indexes,
    };
    let root = state.root as usize;
    // 清掉上次结果，避免读到旧内容
    *ONLINE_RESULT.lock().unwrap() = None;
    std::thread::spawn(move || {
        let result = execute_online(&params);
        *ONLINE_RESULT.lock().unwrap() = Some(result);
        unsafe {
            PostMessageW(root as Hwnd, WM_APP_ONLINE_DONE, 0, 0);
        }
    });
    set_text(
        state.controls.status,
        &if language == Language::English {
            "Online backup/restore started in the background. A result dialog will appear when it finishes."
        } else {
            "已启动在线备份/还原（后台执行），完成后会弹出结果。"
        },
    );
    append_gui_log(
        state,
        &format!(
            "online operation started: op={operation} source={source_drive} target={target_drive}"
        ),
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
                755,
                60,
                225,
                300,
                ID_LANGUAGE,
            ),
            operation_tabs: [
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    // WS_GROUP 标记「操作模式」单选组的起点：方向键只在这 5
                    // 个之间切换，不会跳到下方 PE 启动方式那一组。
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE | WS_GROUP,
                    180,
                    60,
                    95,
                    32,
                    ID_OPERATION_PROBE,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    279,
                    60,
                    95,
                    32,
                    ID_OPERATION_BACKUP,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    378,
                    60,
                    95,
                    32,
                    ID_OPERATION_RESTORE,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    477,
                    60,
                    95,
                    32,
                    ID_OPERATION_SECONDARY,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    576,
                    60,
                    95,
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
            status: create_control(
                hwnd,
                "EDIT",
                "先点击“刷新环境”确认系统、恢复环境和卷身份。默认模式为无破坏探测。",
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
        // 压缩率下拉：显示按语言给出的说明文本（中文 verbatim 见
        // compress_level_labels），初始语言为中文；索引 0/1/2 ↔ max/fast/none，
        // 默认 fast（索引 1）。
        for label in compress_level_labels(Language::Chinese) {
            add_combo_item(controls.compress, label);
        }
        SendMessageW(controls.compress, CB_SETCURSEL, 1, 0);
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
        create_control(hwnd, "STATIC", "操作模式", 0, 20, 55, 130, 22, 2001);
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
            0,
            690,
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
        // 「PE 恢复」tab：启动方式单选 + PE 目录名（仅 RAM disk 模式使用）
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
                GetDlgItem(hwnd, ID_PE_DIR_EDIT as i32) != null_mut(),
            ),
        );
        if let Ok(image) = std::env::var("BACKUPRESTORE_OPEN_IMAGE") {
            if !image.trim().is_empty() {
                select_operation(&mut *state_ptr, 2);
                set_text((*state_ptr).controls.image, &image);
                read_image(&mut *state_ptr);
            }
        }
        if let Ok(tab) = std::env::var("BACKUPRESTORE_OPEN_TAB") {
            if let Ok(index) = tab.parse::<usize>() {
                if (1..=4).contains(&index) {
                    select_operation(&mut *state_ptr, index);
                }
            }
        }
        // 命令行参数 --tab N（UAC 提升后命令行参数保留，比环境变量可靠）
        {
            let args: Vec<String> = std::env::args().collect();
            if let Some(pos) = args.iter().position(|a| a == "--tab") {
                if let Some(value) = args.get(pos + 1) {
                    if let Ok(index) = value.parse::<usize>() {
                        if (1..=4).contains(&index) {
                            select_operation(&mut *state_ptr, index);
                        }
                    }
                }
            }
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
        if message == WM_APP_ONLINE_DONE {
            // 在线备份/还原后台线程完成：读取结果并弹窗展示
            let result = ONLINE_RESULT.lock().unwrap().take().unwrap_or_default();
            let language = selected_language(state);
            let success = result.contains("exit=0")
                || result.contains("The operation completed successfully");
            let (text, caption, flags) = if success {
                (
                    if language == Language::English {
                        format!("Online backup/restore completed successfully.\n\n{result}")
                    } else {
                        format!("在线备份/还原执行成功。\n\n{result}")
                    },
                    if language == Language::English {
                        "Completed"
                    } else {
                        "执行成功"
                    },
                    MB_OK | MB_ICONINFORMATION,
                )
            } else {
                (
                    if language == Language::English {
                        format!("Online backup/restore finished with errors.\n\n{result}")
                    } else {
                        format!("在线备份/还原执行结束，但可能存在问题。\n\n{result}")
                    },
                    if language == Language::English {
                        "Finished with errors"
                    } else {
                        "执行结束（可能存在问题）"
                    },
                    MB_OK | MB_ICONWARNING,
                )
            };
            append_gui_log(
                state,
                &format!("online operation finished: success={success}"),
            );
            show_message(state.root, &text, &caption, flags);
            return 0;
        }
        if message == WM_KEYDOWN {
            // 快捷键：Ctrl+B 备份 / Ctrl+R 还原 / Ctrl+P PE 恢复 /
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
                ID_OPERATION_PROBE => Some(0),
                ID_OPERATION_BACKUP => Some(1),
                ID_OPERATION_RESTORE => Some(2),
                ID_OPERATION_SECONDARY => Some(3),
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
            if matches!(control_id, ID_SOURCE | ID_TARGET) && notification == CBN_SELCHANGE {
                if control_id == ID_SOURCE
                    && selected_operation(state) == "restore-existing"
                    && let Some(source) = selected_drive_letter(state, state.controls.source)
                {
                    select_drive(state, state.controls.target, &source);
                }
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
                ID_PE_REBOOT_MAIN => pe_reboot_to_pe(state),
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

/// Minimal Win32 structs used by `exit_pe_to_windows` to run `bcdedit.exe`
/// synchronously and capture its exit code instead of a silent ShellExecuteW.
#[repr(C)]
struct StartupInfoW {
    cb: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    x_size: u32,
    y_size: u32,
    x_count_chars: u32,
    y_count_chars: u32,
    fill_attribute: u32,
    flags: u32,
    show_window: u16,
    cb_reserved2: u16,
    reserved2: *mut u8,
    std_input: Handle,
    std_output: Handle,
    std_error: Handle,
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

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
/// Append the UTF-16 encoding of `source` (without a trailing NUL) to a
/// command-line buffer, so several `wide()` results can be joined safely.
fn push_wide_into(target: &mut Vec<u16>, source: &str) {
    target.extend(source.encode_utf16());
}

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

/// Best-effort diagnostics written via the volume path
/// (`\\?\Volume{GUID}\exit-pe.log`, survives unmount and reboot), plus
/// `Q:\exit-pe.log` and `X:\exit-pe.log` (PE RAM disk) so a failed exit
/// attempt can be diagnosed from the next Windows session. Writes go through
/// CreateFileW/WriteFile directly because std::fs writes to mounted FAT
/// volumes failed silently inside PE. Returns per-path failure details
/// (including GetLastError) so the caller can surface them during development.
fn write_pe_exit_log(entries: &[String], esp_volume_path: Option<&str>) -> Vec<String> {
    let text = entries.join("\r\n");
    let mut failures: Vec<String> = Vec::new();
    // 优先写卷路径（\\.\Volume{GUID}\ 独立于盘符，PE 内最可靠，重启后仍
    // 保留在 ESP 上）；再写 S:（若已挂载）与 X:（PE RAM 盘，重启丢失）。
    // 注意：不要写 Q: —— PE 里 Q: 通常不存在（Win11 侧盘符漂移）。
    let mut paths: Vec<String> = Vec::new();
    if let Some(vp) = esp_volume_path {
        let mut full = vp.to_string();
        if !full.ends_with('\\') {
            full.push('\\');
        }
        full.push_str("exit-pe.log");
        paths.push(full);
    }
    paths.push("S:\\exit-pe.log".to_string());
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
        pe_reboot(hwnd);
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
                    let mut bcd_path: Vec<u16> = Vec::new();
                    bcd_path.push(letter);
                    bcd_path.push(':' as u16);
                    bcd_path.push('\\' as u16);
                    push_wide_into(&mut bcd_path, "EFI\\Microsoft\\Boot\\BCD");
                    bcd_path.push(0);
                    if GetFileAttributesW(bcd_path.as_ptr()) != u32::MAX {
                        diag.push("BCD file found".to_string());
                        esp_volume_path = Some(volume_path.clone());
                        // Read the Windows entry GUID written to the ESP at
                        // deploy time (`bcdedit /enum {current} /v`). `{current}`
                        // cannot be used here: inside PE it resolves to the PE
                        // entry, which does not exist in the store we edit.
                        let mut win_guid: Option<String> = None;
                        let guid_path = format!("{}:\\pe-exit-guid.txt", letter as u8 as char);
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
                        let mut command_line: Vec<u16> = Vec::new();
                        // Development: keep the console window visible, echo the
                        // bcdedit exit codes on the same window and hold it open
                        // with `pause` so the output can be inspected before
                        // reboot. In auto-click mode the pause/echo are omitted
                        // (no human to press a key) so the reboot happens right
                        // after bcdedit finishes.
                        push_wide_into(&mut command_line, "cmd.exe /c ");
                        push_wide_into(&mut command_line, "bcdedit.exe /store ");
                        command_line.extend_from_slice(&bcd_path[..bcd_path.len() - 1]);
                        match &win_guid {
                            Some(guid) => {
                                push_wide_into(&mut command_line, " /set {bootmgr} default ");
                                push_wide_into(&mut command_line, guid);
                            }
                            None => {
                                push_wide_into(&mut command_line, " /enum");
                            }
                        }
                        // 同时清除 bootsequence：若进 PE 用的是 bootsequence
                        // 方式（PE RAM 盘无法回写消费），不清会导致每次重启
                        // 都再进 PE（死循环）。无 bootsequence 时该命令报错
                        // 无害（default 已设置）。
                        push_wide_into(&mut command_line, " & bcdedit.exe /store ");
                        command_line.extend_from_slice(&bcd_path[..bcd_path.len() - 1]);
                        push_wide_into(&mut command_line, " /deletevalue {bootmgr} bootsequence");
                        if !PE_AUTO_CLICK.load(std::sync::atomic::Ordering::SeqCst) {
                            push_wide_into(
                                &mut command_line,
                                " & call echo EXIT_CODE=%errorlevel% & pause",
                            );
                        }
                        command_line.push(0);
                        let mut startup: StartupInfoW = std::mem::zeroed();
                        startup.cb = size_of::<StartupInfoW>() as u32;
                        let mut process: ProcessInformation = std::mem::zeroed();
                        let created = CreateProcessW(
                            null(),
                            command_line.as_mut_ptr(),
                            null_mut(),
                            null_mut(),
                            0,
                            0, // visible console during development
                            null_mut(),
                            null_mut(),
                            &mut startup,
                            &mut process,
                        );
                        if created != 0 {
                            // 60 s: enough for the developer to read the
                            // paused bcdedit output and press a key.
                            let wait = WaitForSingleObject(process.process, 60000);
                            if wait == 0 {
                                let mut code: u32 = 0;
                                GetExitCodeProcess(process.process, &mut code);
                                diag.push(format!("bcdedit exit code: {}", code));
                                // deletevalue 无值可删时 bcdedit 也返回非 0
                                // （"Element not found"），default 已设置则无碍。
                                if code != 0 {
                                    diag.push(format!("bcdedit reported failure (exit {})", code));
                                }
                            } else {
                                // 超时 = cmd 还挂在 pause 等待人工按键（手动
                                // 开发模式）；bcdedit 命令本身早已执行完。
                                diag.push(
                                    "bcdedit wait timed out (cmd paused, waiting for key)"
                                        .to_string(),
                                );
                            }
                            CloseHandle(process.thread);
                            CloseHandle(process.process);
                            found_bcd = true;
                        } else {
                            diag.push(format!(
                                "CreateProcessW failed, last error: {}",
                                GetLastError()
                            ));
                        }
                    } else {
                        diag.push("BCD file NOT found at mount point".to_string());
                    }
                    // Write diagnostics while the ESP is still mounted (volume
                    // path also survives unmount, so this is belt and braces).
                    diag.push("exit sequence done, rebooting".to_string());
                    let write_failures = write_pe_exit_log(&diag, Some(&volume_path));
                    for f in &write_failures {
                        diag.push(f.clone());
                    }
                    // Development: surface the whole diagnostic chain (volume
                    // enum, mount, bcdedit exit code, log-write errors) before
                    // rebooting, since PE log files are unreliable.
                    show_diag_dialog(hwnd, &diag);
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
    pe_reboot(hwnd);
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

/// 从动作结果里提取关键行，用于结果对话框（避免把 diskpart/dism 长输出
/// 全部塞进弹窗）。
fn pe_result_preview(result: &str) -> String {
    let mut lines = Vec::new();
    for line in result.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if l.starts_with('[')
            || l.contains("code=")
            || l.contains("REFUSED")
            || l.contains("successfully")
            || l.contains("Error")
            || l.contains("failed")
            || l.contains("skipped")
            || l.contains("system drive")
            || l.contains("actual drive")
        {
            lines.push(l.to_string());
        }
    }
    if lines.is_empty() {
        result.to_string()
    } else {
        lines.join("\n")
    }
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
                "菜单名称：",
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

/// PE 桌面「备份系统」：PE 内直接 dism 捕获，不再跳主 GUI。
unsafe fn pe_backup_from_desktop(hwnd: Hwnd) {
    // 自动点击模式（S:\pe-click.txt）：跳过对话框，用配置参数直接执行，
    // 完成后恢复 BCD 并自动重启回 Windows（等效鼠标点击整条链路）。
    if let Some(params) = pe_click_params("backup") {
        if params.len() >= 2 {
            let mut result = String::new();
            let mut reboot = false;
            let mut src = params[0].trim_end_matches(':').to_string();
            // AUTO：先用 find-drive 定位含 marker.txt 的测试盘（PE 盘符漂移）
            if src == "AUTO" {
                execute_pe_task_line("find-drive marker.txt", &mut result, &mut reboot);
                src = resolve_drive("AUTO");
            }
            let wim = resolve_pe_wim_path(&params[1], &src);
            execute_pe_task_line(&format!("backup {src} {wim}"), &mut result, &mut reboot);
            let _ = std::fs::write("S:\\pe-gui-backup.txt", &result);
            exit_pe_to_windows(hwnd);
            return;
        }
    }
    // 1. 定位系统卷（默认源卷）
    let mut result = String::new();
    let mut reboot = false;
    execute_pe_task_line("find-system-drive", &mut result, &mut reboot);
    let system = resolve_drive("AUTO");
    // 2. 默认镜像路径：数据卷 + 时间戳文件名（避开 dism 转义字母）
    let image_drive = pe_default_image_drive(&system);
    let default_wim = format!("{image_drive}:\\pe-bk-{}.wim", pe_timestamp());
    // 3. 对话框确认源卷与镜像路径
    let out = pe_dialog(
        hwnd,
        "备份系统 - BackupRestore",
        false,
        "将把所选卷捕获为 WIM 镜像（不修改源卷）。",
        &default_wim,
        "",
    );
    let Some((drive_item, wim, _name)) = out else {
        return;
    };
    let src = drive_item
        .split('|')
        .next()
        .unwrap_or("")
        .trim_end_matches(':')
        .to_string();
    let wim = wim.trim().to_string();
    if src.is_empty() || wim.is_empty() {
        return;
    }
    // 4. 执行备份
    result.clear();
    execute_pe_task_line(&format!("backup {src} {wim}"), &mut result, &mut reboot);
    // 5. 日志落 ESP（重启后可读回验证）
    let _ = std::fs::write("S:\\pe-gui-backup.txt", &result);
    // 6. 结果展示
    show_message(
        hwnd,
        &pe_result_preview(&result),
        "备份完成",
        MB_OK | MB_ICONINFORMATION,
    );
    // 7. 询问是否返回 Windows
    let ask = show_message(
        hwnd,
        "备份完成。返回 Windows 吗？",
        "BackupRestore",
        MB_YESNO | MB_ICONQUESTION,
    );
    if ask == IDYES {
        pe_reboot(hwnd);
    }
}

/// PE 桌面「还原系统」：格式化目标卷 + Apply WIM + BCDBoot（目标为系统卷时）。
unsafe fn pe_restore_from_desktop(hwnd: Hwnd) {
    // 自动点击模式：跳过对话框与二次确认，直接格式化+还原+修复引导。
    if let Some(params) = pe_click_params("restore") {
        if params.len() >= 2 {
            let mut result = String::new();
            let mut reboot = false;
            let mut target = params[1].trim_end_matches(':').to_string();
            // AUTO：先用 find-drive 定位含 marker.txt 的测试盘（PE 盘符漂移）
            if target == "AUTO" {
                execute_pe_task_line("find-drive marker.txt", &mut result, &mut reboot);
                target = resolve_drive("AUTO");
            }
            let wim = resolve_pe_wim_path(&params[0], &target);
            execute_pe_task_line(
                &format!("format {target} --allow-system"),
                &mut result,
                &mut reboot,
            );
            execute_pe_task_line(&format!("restore {wim} {target}"), &mut result, &mut reboot);
            execute_pe_task_line(&format!("bcdboot {target} S"), &mut result, &mut reboot);
            let _ = std::fs::write("S:\\pe-gui-restore.txt", &result);
            exit_pe_to_windows(hwnd);
            return;
        }
    }
    let mut result = String::new();
    let mut reboot = false;
    execute_pe_task_line("find-system-drive", &mut result, &mut reboot);
    let _system = resolve_drive("AUTO");
    let out = pe_dialog(
        hwnd,
        "还原系统 - BackupRestore",
        false,
        "警告：将格式化目标卷并应用 WIM，目标卷数据将被覆盖！",
        "H:\\pe-wim1.wim",
        "",
    );
    let Some((drive_item, wim, _name)) = out else {
        return;
    };
    let target = drive_item
        .split('|')
        .next()
        .unwrap_or("")
        .trim_end_matches(':')
        .to_string();
    let wim = wim.trim().to_string();
    if target.is_empty() || wim.is_empty() {
        return;
    }
    // 二次确认（高危操作）
    let ask = show_message(
        hwnd,
        &format!(
            "确认将 {} 还原到卷 {}:？\n此操作将格式化目标卷，数据不可恢复！",
            wim, target
        ),
        "还原确认",
        MB_YESNO | MB_ICONWARNING,
    );
    if ask != IDYES {
        return;
    }
    result.clear();
    // 1. 格式化（还原系统放行系统卷）
    execute_pe_task_line(
        &format!("format {target} --allow-system"),
        &mut result,
        &mut reboot,
    );
    // 2. 应用 WIM
    execute_pe_task_line(&format!("restore {wim} {target}"), &mut result, &mut reboot);
    // 3. 修复引导（目标是系统卷时执行 bcdboot）
    execute_pe_task_line(&format!("bcdboot {target} S"), &mut result, &mut reboot);
    let _ = std::fs::write("S:\\pe-gui-restore.txt", &result);
    show_message(
        hwnd,
        &pe_result_preview(&result),
        "还原完成",
        MB_OK | MB_ICONINFORMATION,
    );
    // 还原后必须重启
    let ask2 = show_message(
        hwnd,
        "还原完成，现在重启进入系统？",
        "BackupRestore",
        MB_YESNO | MB_ICONQUESTION,
    );
    if ask2 == IDYES {
        pe_reboot(hwnd);
    }
}

/// PE 桌面「安装第二系统」：Apply WIM 到目标卷 + BCD 追加启动项。
unsafe fn pe_secondary_from_desktop(hwnd: Hwnd) {
    // 自动点击模式：跳过对话框与确认，直接格式化+还原+追加启动项。
    if let Some(params) = pe_click_params("secondary") {
        if params.len() >= 2 {
            let mut result = String::new();
            let mut reboot = false;
            let mut target = params[1].trim_end_matches(':').to_string();
            // AUTO：先用 find-drive 定位含 marker.txt 的测试盘（PE 盘符漂移）
            if target == "AUTO" {
                execute_pe_task_line("find-drive marker.txt", &mut result, &mut reboot);
                target = resolve_drive("AUTO");
            }
            let wim = resolve_pe_wim_path(&params[0], &target);
            let menu = if params.len() >= 3 {
                params[2].clone()
            } else {
                "Windows 备份".to_string()
            };
            execute_pe_task_line(&format!("format {target}"), &mut result, &mut reboot);
            execute_pe_task_line(&format!("restore {wim} {target}"), &mut result, &mut reboot);
            execute_pe_task_line(
                &format!("add-secondary-entry {target} {menu}"),
                &mut result,
                &mut reboot,
            );
            let _ = std::fs::write("S:\\pe-gui-secondary.txt", &result);
            exit_pe_to_windows(hwnd);
            return;
        }
    }
    let out = pe_dialog(
        hwnd,
        "安装第二系统 - BackupRestore",
        true,
        "将把 WIM 应用到目标卷并添加启动菜单项（不影响 Windows 默认启动）。",
        "H:\\pe-wim1.wim",
        "Windows 备份",
    );
    let Some((drive_item, wim, name)) = out else {
        return;
    };
    let target = drive_item
        .split('|')
        .next()
        .unwrap_or("")
        .trim_end_matches(':')
        .to_string();
    let wim = wim.trim().to_string();
    if target.is_empty() || wim.is_empty() {
        return;
    }
    let menu = if name.trim().is_empty() {
        "Windows 备份"
    } else {
        name.trim()
    };
    let ask = show_message(
        hwnd,
        &format!(
            "确认将 {} 安装为第二系统到卷 {}:？\n菜单名称：{}",
            wim, target, menu
        ),
        "安装第二系统",
        MB_YESNO | MB_ICONWARNING,
    );
    if ask != IDYES {
        return;
    }
    let mut result = String::new();
    let mut reboot = false;
    // 1. 格式化（第二系统目标默认拒绝系统卷/ESP）
    execute_pe_task_line(&format!("format {target}"), &mut result, &mut reboot);
    // 2. 应用 WIM
    execute_pe_task_line(&format!("restore {wim} {target}"), &mut result, &mut reboot);
    // 3. BCD 追加启动项
    execute_pe_task_line(
        &format!("add-secondary-entry {target} {menu}"),
        &mut result,
        &mut reboot,
    );
    let _ = std::fs::write("S:\\pe-gui-secondary.txt", &result);
    show_message(
        hwnd,
        &pe_result_preview(&result),
        "安装第二系统完成",
        MB_OK | MB_ICONINFORMATION,
    );
    let ask2 = show_message(
        hwnd,
        "第二系统已添加，现在重启查看启动菜单？",
        "BackupRestore",
        MB_YESNO | MB_ICONQUESTION,
    );
    if ask2 == IDYES {
        pe_reboot(hwnd);
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
        let start_y = ((height - (card_height * 2 + gap + 150)) / 2).max(16) + 24;
        let cards: [(usize, &str, i32, i32); 6] = [
            (ID_PE_BACKUP, "备份系统", 0, 0),
            (ID_PE_RESTORE, "还原系统", 1, 0),
            (ID_PE_SECONDARY, "安装第二系统", 2, 0),
            (ID_PE_CMD, "命令提示符", 0, 1),
            (ID_PE_EXIT, "返回 Windows", 1, 1),
            // 「重启」已合并进「返回 Windows」，此位改为打开完整主程序 GUI
            (ID_PE_MAIN_GUI, "打开完整程序", 2, 1),
        ];
        let mut card_controls = Vec::with_capacity(6);
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
            &format!("BackupRestore v{PROGRAM_VERSION}  |  WinPE"),
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

        let state = Box::new(PeDesktopState {
            clock,
            fonts: [title_font, card_font, bar_font],
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

/// Full-screen, shell-free recovery desktop for WinPE sessions. The main GUI
/// follows when the technician picks backup/restore/secondary (returned as the
/// operation tab index); `None` means the desktop was dismissed without one.
/// PE 启动时自清 bootsequence。
///
/// PE 是 RAM 盘（X:\），bootmgr 引导 PE 后无法把"已消费 bootsequence"写回
/// ESP 的 BCD，导致 bootsequence 残留：每次重启都会再次进入 PE、回不了
/// 主系统。本函数在 PE 恢复桌面启动时（用户可见 UI 出现前）静默挂载 ESP
/// 分区（S:）并清除 {bootmgr} bootsequence，恢复"下次重启回主系统"的
/// 正常行为，全程无需用户操作。结果写入 ESP 根目录
/// pe-bootsequence-clean.log，重启后可回到 Windows 读回验证。
/// 配置驱动的 PE 任务执行：Windows 侧把要执行的动作写入 ESP 的
/// `S:\pe-task.txt`（每行一个动作，`reboot` 表示执行完自动重启回
/// Windows），PE 启动时读取并按配置逐条执行，结果落盘
/// `S:\pe-task-result.txt`，配置改名为 `.done` 防止重复执行。
/// 返回 `true` 表示配置要求执行后自动重启（调用方自动重启，无需用户
/// 操作）；无配置返回 `false`（正常显示 PE 恢复桌面）。
///
/// 解析动作里的盘符参数：`AUTO` 表示用 attach-vhd 实际挂载出的盘符
/// （PE 里 diskpart 手动 assign 不生效，VHD 分区由系统自动分配盘符，
/// 通过枚举 marker.txt 所在盘符得到）。
fn resolve_drive(drive: &str) -> String {
    if drive.eq_ignore_ascii_case("AUTO") {
        std::fs::read_to_string("S:\\pe-drive.txt")
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        drive.to_string()
    }
}

/// 解析动作里的路径参数：路径中的 `AUTO:` 前缀替换为实际盘符。
fn resolve_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("AUTO:") {
        format!("{}:{rest}", resolve_drive("AUTO"))
    } else {
        path.to_string()
    }
}

/// 解析自动点击配置里的 WIM 路径：`AUTO:PE\xxx.wim` 前缀在 PE 内自动
/// 定位「PE 源盘」（含 BackupRestorePE\sources\boot.wim 的卷），解决
/// Win11 侧盘符（如 Q:）在 PE 里盘符漂移、路径不存在的问题。
/// `exclude` 为数据盘盘符（find-drive 定位的源/目标盘），排除它是因为
/// 数据盘上也可能残留 BackupRestorePE 目录（如 RAM 模式验证部署过）。
fn resolve_pe_wim_path(wim: &str, exclude: &str) -> String {
    if let Some(rest) = wim.strip_prefix("AUTO:PE") {
        format!("{}:{rest}", find_pe_source_drive(exclude))
    } else {
        wim.to_string()
    }
}

/// 在 PE 内定位「PE 源盘」：枚举 C:-Z: 找含
/// BackupRestorePE\sources\boot.wim 的卷（PE 从它内存启动）。
fn find_pe_source_drive(exclude: &str) -> String {
    for letter in 'C'..='Z' {
        let letter = letter.to_string();
        if letter == exclude {
            continue;
        }
        let path = format!("{letter}:\\BackupRestorePE\\sources\\boot.wim");
        if std::path::Path::new(&path).exists() {
            return letter;
        }
    }
    "Q".to_string() // 兜底：Win11 侧约定盘符（PE 找不到时按字面尝试）
}

/// 读取 PE 桌面自动点击配置 S:\pe-click.txt（首行 action，空格分隔参数）。
/// 格式：`backup <源盘> <wim路径>` / `restore <wim路径> <目标盘>`
/// / `secondary <wim路径> <目标盘> [菜单名]`。文件不存在返回 None。
/// 该机制让「Win11 配置 → 重启进 PE → 自动点击按钮（与鼠标点击同路径）
/// → 自动执行 → 自动回 Win11」全链路无需任何人工操作。
fn read_pe_click_config() -> Option<(&'static str, Vec<String>)> {
    let config = std::fs::read_to_string("S:\\pe-click.txt").ok()?;
    let mut parts = config.split_whitespace();
    let action = parts.next()?;
    let params: Vec<String> = parts.map(|s| s.to_string()).collect();
    let action = match action {
        "backup" => "backup",
        "restore" => "restore",
        "secondary" => "secondary",
        "exit" => "exit",
        "main" => "main",
        _ => return None,
    };
    Some((action, params))
}

/// 供 PE 桌面按钮 handler 使用的自动参数分支：返回动作与参数配置，
/// 存在且动作匹配时调用方应跳过对话框、直接执行。
/// 注意：handler 运行时 `S:\pe-click.txt` 已被启动流程改名 `.done`
/// （防止重复触发），因此这里读 `.done` 才能拿到本次点击的参数。
fn pe_click_params(action: &str) -> Option<Vec<String>> {
    let config = std::fs::read_to_string("S:\\pe-click.txt.done").ok()?;
    let mut parts = config.split_whitespace();
    let config_action = parts.next()?;
    if config_action != action {
        return None;
    }
    Some(parts.map(|s| s.to_string()).collect())
}

/// 支持动作（每行一个，空格分隔参数）：
/// - `clean_bootsequence`：挂载 ESP 并清除 {bootmgr} bootsequence
/// - `verify`：取证（bootmgr 枚举 / ESP 目录列表 / 结果日志读回）
/// - `attach-vhd <路径> <盘符>`：diskpart 挂载 VHD 并分配盘符（PE 测试用）
/// - `backup <盘符> <wim路径>`：dism 捕获卷为 WIM
/// - `restore <wim路径> <盘符>`：dism 应用 WIM 到卷
/// - `verify-file <路径>`：检查文件是否存在（测试验证）
/// - `reboot`：全部执行完后自动重启回 Windows
fn execute_pe_task_line(action: &str, result: &mut String, reboot: &mut bool) {
    let parts: Vec<&str> = action.split_whitespace().collect();
    match parts.as_slice() {
        [] => {}
        ["reboot"] => *reboot = true,
        ["clean_bootsequence"] => {
            let mount_code = run_cmd_to_file("mountvol.exe S: /S", None);
            let clean_code = run_cmd_to_file(
                "bcdedit.exe /store S:\\EFI\\Microsoft\\Boot\\BCD /deletevalue {bootmgr} bootsequence",
                None,
            );
            result.push_str(&format!(
                "clean_bootsequence: mountvol={mount_code}, clean={clean_code}\n"
            ));
        }
        ["verify"] => {
            // 取证 A：bootmgr 条目当前状态（确认 bootsequence 已清除）
            run_cmd_to_file(
                "cmd /c bcdedit.exe /store S:\\EFI\\Microsoft\\Boot\\BCD /enum {bootmgr} > S:\\verify-bcd-enum.txt 2>&1",
                None,
            );
            match std::fs::read_to_string("S:\\verify-bcd-enum.txt") {
                Ok(text) => result.push_str(&format!("[ENUM_BOOTMGR]\n{text}\n")),
                Err(_) => result.push_str("[ENUM_BOOTMGR] read failed\n"),
            }
            // 取证 B：ESP 根目录文件列表
            run_cmd_to_file("cmd /c dir S:\\ > S:\\verify-dir.txt 2>&1", None);
            if let Ok(text) = std::fs::read_to_string("S:\\verify-dir.txt") {
                result.push_str(&format!("[DIR_S]\n{text}\n"));
            }
            // 取证 C：结果日志读回（确认已落盘）
            match std::fs::read_to_string("S:\\pe-task-result.txt") {
                Ok(text) => result.push_str(&format!("[RESULT_READBACK]\n{text}\n")),
                Err(_) => result.push_str("[RESULT_READBACK] not yet written\n"),
            }
        }
        ["attach-vhd", vhd, _drive] => {
            // PE 里盘符可能与 Windows 不同：先搜索 VHD 文件实际所在盘符，
            // 再用实际路径 attach（diskpart 路径错误会直接失败）。
            let file = vhd
                .rsplit('\\')
                .next()
                .unwrap_or(vhd)
                .rsplit('/')
                .next()
                .unwrap_or(vhd);
            let find_out = "S:\\find-vhd.txt";
            run_cmd_to_file(
                &format!(
                    "cmd /c for %d in (C D E F G H I J K L M N O P Q R S T U V W X Y Z) do @if exist %d:\\{file} echo %d > {find_out}"
                ),
                None,
            );
            let mut real_vhd = vhd.to_string();
            if let Ok(text) = std::fs::read_to_string(find_out) {
                if let Some(line) = text.lines().next() {
                    let drv = line.trim().trim_end_matches(':');
                    if drv.len() == 1 && drv.as_bytes()[0].is_ascii_alphabetic() {
                        real_vhd = format!("{drv}:\\{file}");
                    }
                }
            }
            result.push_str(&format!("attach-vhd: file={vhd}, resolved={real_vhd}\n"));
            // diskpart 挂载 VHD（先 automount enable 确保 PE 自动分配盘符，
            // 不手动 assign——PE 里 assign 不生效）
            let script = format!("automount enable\nselect vdisk file={real_vhd}\nattach vdisk\n");
            let script_file = "X:\\attach-vhd.txt";
            let _ = std::fs::write(script_file, &script);
            let code = run_cmd_to_file(
                &format!("cmd /c diskpart /s {script_file} > S:\\attach-vhd-out.txt 2>&1"),
                None,
            );
            result.push_str(&format!("attach-vhd {real_vhd}: code={code}\n"));
            if let Ok(text) = std::fs::read_to_string("S:\\attach-vhd-out.txt") {
                result.push_str(&format!("[ATTACH_VHD]\n{text}\n"));
            }
            // 枚举 marker.txt 所在盘符（VHD 卷自动分配的盘符）
            run_cmd_to_file(
                "cmd /c for %d in (C D E F G H I J K L M N O P Q R S T U V W X Y Z) do @if exist %d:\\marker.txt echo %d > S:\\find-marker.txt",
                None,
            );
            let mut actual = String::new();
            if let Ok(text) = std::fs::read_to_string("S:\\find-marker.txt") {
                actual = text.lines().next().unwrap_or("").trim().to_string();
            }
            let _ = std::fs::write("S:\\pe-drive.txt", &actual);
            result.push_str(&format!("attach-vhd: actual drive = {actual}\n"));
            // 诊断：attach 后实际盘符卷内容
            if !actual.is_empty() {
                run_cmd_to_file(
                    &format!("cmd /c dir {actual}:\\ > S:\\dir-attached.txt 2>&1"),
                    None,
                );
                if let Ok(text) = std::fs::read_to_string("S:\\dir-attached.txt") {
                    result.push_str(&format!("[DIR_ATTACHED {actual}:]\n{text}\n"));
                }
            }
        }
        ["backup", drive, wim] => {
            // dism 捕获卷为 WIM（程序备份核心就是 dism Capture-Image）
            // 目标 WIM 已存在时 dism 会追加索引，先删除保证单索引
            let d = resolve_drive(drive);
            let _ = std::fs::remove_file(wim);
            let out = "S:\\backup-out.txt";
            // 备份进度 GUI：后台窗口线程读 DISM 输出文件实时刷新，
            // 不弹 cmd 黑窗、不阻塞界面（见 recovery_progress.rs）。
            let progress = crate::recovery_progress::spawn(PathBuf::from(out));
            // 生成 DISM 排除配置（临时目录/回收站/浏览器缓存），写到 PE 的
            // X: RAM 盘，不会落在捕获卷内；配置失败则不带排除继续捕获。
            let mut exclude_arg = String::new();
            let config_path = std::env::temp_dir().join("BackupRestore-exclusions.ini");
            let source_root = format!("{d}:\\");
            if let Ok(text) = backuprestore_core::build_capture_exclusions(Path::new(&source_root))
            {
                if std::fs::write(&config_path, text).is_ok() {
                    exclude_arg = format!(" /ConfigFile:{}", config_path.display());
                }
            }
            // PE 的 dism 对长参数敏感，用最小参数集（Name 值不能含连字符）
            run_cmd_to_file_timeout(
                &format!(
                    "cmd /c dism.exe /Capture-Image /ImageFile:{wim} /CaptureDir:{d}:\\ /Name:PE{exclude_arg} > {out} 2>&1"
                ),
                None,
                600000,
            );
            crate::recovery_progress::request_close(&progress);
            if let Ok(text) = std::fs::read_to_string(out) {
                result.push_str(&format!("[BACKUP {d}: -> {wim}]\n{text}\n"));
            } else {
                result.push_str(&format!("backup {d}: -> {wim}: no output\n"));
            }
        }
        ["delete-file", path] => {
            // 删除文件（还原验证：删掉后 restore 应恢复它）
            let p = resolve_path(path);
            let out = "S:\\delete-out.txt";
            run_cmd_to_file(&format!("cmd /c del /q {p} > {out} 2>&1"), None);
            if let Ok(text) = std::fs::read_to_string(out) {
                result.push_str(&format!("[DELETE_FILE {p}]\n{text}\n"));
            } else {
                result.push_str(&format!("delete-file {p}: no output\n"));
            }
        }
        ["find-drive", marker] => {
            // 枚举含标记文件的盘符（真实分区在 PE 里盘符可能变化，
            // 物理分区会自动挂载，只需找到实际盘符）
            run_cmd_to_file(
                &format!(
                    "cmd /c for %d in (C D E F G H I J K L M N O P Q R S T U V W X Y Z) do @if exist %d:\\{marker} echo %d > S:\\find-drive.txt"
                ),
                None,
            );
            let mut actual = String::new();
            if let Ok(text) = std::fs::read_to_string("S:\\find-drive.txt") {
                actual = text.lines().next().unwrap_or("").trim().to_string();
            }
            let _ = std::fs::write("S:\\pe-drive.txt", &actual);
            result.push_str(&format!("find-drive {marker}: actual drive = {actual}\n"));
            if !actual.is_empty() {
                run_cmd_to_file(
                    &format!("cmd /c dir {actual}:\\ > S:\\dir-attached.txt 2>&1"),
                    None,
                );
                if let Ok(text) = std::fs::read_to_string("S:\\dir-attached.txt") {
                    result.push_str(&format!("[DIR_ATTACHED {actual}:]\n{text}\n"));
                }
            }
        }
        ["dism-diag"] => {
            // 诊断 PE 的 dism Capture-Image 各变体（一次进 PE 拿全部信息）
            let cases: [(&str, &str); 4] = [
                (
                    "c1_img=H:\\pe-wim1.wim dir=T:\\",
                    "cmd /c dism.exe /Capture-Image /ImageFile:H:\\pe-wim1.wim /CaptureDir:T:\\ /Name:X > S:\\diag1.txt 2>&1",
                ),
                (
                    "c2_img=X:\\pe-wim1.wim dir=T:\\",
                    "cmd /c dism.exe /Capture-Image /ImageFile:X:\\pe-wim1.wim /CaptureDir:T:\\ /Name:X > S:\\diag2.txt 2>&1",
                ),
                (
                    "c3_img=H:\\pe-wim1.wim dir=T:",
                    "cmd /c dism.exe /Capture-Image /ImageFile:H:\\pe-wim1.wim /CaptureDir:T: /Name:X > S:\\diag3.txt 2>&1",
                ),
                (
                    "c4_img=H:\\pewim1.wim dir=T:\\",
                    "cmd /c dism.exe /Capture-Image /ImageFile:H:\\pewim1.wim /CaptureDir:T:\\ /Name:X > S:\\diag4.txt 2>&1",
                ),
            ];
            for (label, command) in cases {
                let out = match label.chars().nth(1) {
                    Some('1') => "S:\\diag1.txt",
                    Some('2') => "S:\\diag2.txt",
                    Some('3') => "S:\\diag3.txt",
                    _ => "S:\\diag4.txt",
                };
                run_cmd_to_file(command, None);
                result.push_str(&format!("[{label}]\n"));
                if let Ok(text) = std::fs::read_to_string(out) {
                    result.push_str(&text);
                    result.push('\n');
                } else {
                    result.push_str("no output\n");
                }
            }
        }
        ["restore", wim, drive] => {
            // dism 应用 WIM 到卷
            let d = resolve_drive(drive);
            let out = "S:\\restore-out.txt";
            // 还原进度 GUI（同备份：后台窗口线程读 DISM 输出实时刷新）。
            let progress = crate::recovery_progress::spawn(PathBuf::from(out));
            run_cmd_to_file_timeout(
                &format!(
                    "cmd /c dism.exe /Apply-Image /ImageFile:{wim} /Index:1 /ApplyDir:{d}:\\ > {out} 2>&1"
                ),
                None,
                600000,
            );
            crate::recovery_progress::request_close(&progress);
            if let Ok(text) = std::fs::read_to_string(out) {
                result.push_str(&format!("[RESTORE {wim} -> {d}:]\n{text}\n"));
            } else {
                result.push_str(&format!("restore {wim} -> {d}: no output\n"));
            }
        }
        ["verify-file", path] => {
            // 检查文件是否存在（PE 精简版无 PowerShell，用 cmd if exist）
            let p = resolve_path(path);
            let out = "S:\\verify-file-out.txt";
            run_cmd_to_file(
                &format!("cmd /c if exist {p} (echo FOUND) else (echo MISSING) > {out} 2>&1"),
                None,
            );
            if let Ok(text) = std::fs::read_to_string(out) {
                result.push_str(&format!("[VERIFY_FILE {p}]\n{text}\n"));
            } else {
                result.push_str(&format!("verify-file {p}: no output\n"));
            }
        }
        ["find-system-drive"] => {
            // 枚举含 Windows 的卷（PE 里系统分区盘符会变），写入 S:\pe-drive.txt
            run_cmd_to_file(
                "cmd /c for %d in (C D E F G H I J K L M N O P Q R S T U V W X Y Z) do @if exist %d:\\Windows\\System32\\Config\\SYSTEM echo %d > S:\\find-system.txt",
                None,
            );
            let mut actual = String::new();
            if let Ok(text) = std::fs::read_to_string("S:\\find-system.txt") {
                actual = text.lines().next().unwrap_or("").trim().to_string();
            }
            let _ = std::fs::write("S:\\pe-drive.txt", &actual);
            result.push_str(&format!("find-system-drive: system drive = {actual}\n"));
        }
        ["format", drive] | ["format", drive, "--allow-system"] => {
            let d = resolve_drive(drive);
            let allow_system = parts.len() == 3;
            let d_upper = d.trim_end_matches(':').to_ascii_uppercase();
            // 保护 1：PE 运行盘 X: 与 ESP S: 一律拒绝
            if d_upper == "X" || d_upper == "S" || d_upper.is_empty() {
                result.push_str(&format!("format {d}: REFUSED: PE RAM disk or ESP\n"));
            } else {
                // 保护 2：含 Windows 的卷默认拒绝，仅 --allow-system 放行（还原系统场景）
                run_cmd_to_file(
                    &format!(
                        "cmd /c if exist {d_upper}:\\Windows\\System32\\Config\\SYSTEM (echo SYS) else (echo NOSYS) > S:\\format-check.txt"
                    ),
                    None,
                );
                let mut is_system = false;
                if let Ok(text) = std::fs::read_to_string("S:\\format-check.txt") {
                    is_system = text.contains("SYS");
                }
                if is_system && !allow_system {
                    result.push_str(&format!(
                        "format {d_upper}: REFUSED: system volume requires --allow-system\n"
                    ));
                } else {
                    // diskpart 按盘符选卷并快速格式化
                    let script = format!("select volume {d_upper}\nformat fs=ntfs quick\n");
                    let script_file = "X:\\pe-format.txt";
                    let _ = std::fs::write(script_file, &script);
                    let code = run_cmd_to_file_timeout(
                        &format!("cmd /c diskpart /s {script_file} > S:\\format-out.txt 2>&1"),
                        None,
                        300000,
                    );
                    result.push_str(&format!("format {d_upper}: code={code}\n"));
                    if let Ok(text) = std::fs::read_to_string("S:\\format-out.txt") {
                        result.push_str(&format!("[FORMAT {d_upper}]\n{text}\n"));
                    }
                }
            }
        }
        ["bcdboot", drive] | ["bcdboot", drive, ..] => {
            let d = resolve_drive(drive);
            let esp_letter = if parts.len() >= 3 {
                parts[2].trim_end_matches(':')
            } else {
                "S"
            };
            // 仅当目标是系统卷（SYSTEM hive + winload.efi 双条件）才执行 bcdboot 重建引导
            run_cmd_to_file(
                &format!(
                    "cmd /c if exist {d}:\\Windows\\System32\\Config\\SYSTEM (if exist {d}:\\Windows\\system32\\winload.efi (echo SYS) else (echo NOSYS)) else (echo NOSYS) > S:\\bcdboot-check.txt"
                ),
                None,
            );
            let mut is_system = false;
            if let Ok(text) = std::fs::read_to_string("S:\\bcdboot-check.txt") {
                is_system = text.contains("SYS");
            }
            if is_system {
                // 记录修复前 {bootmgr} 默认条目所引用的卷（partition=X:）。
                // 注意：default 字段可能是 {default} 别名，bcdboot 执行后该别名会
                // 重新绑定到新条目，所以必须按「原默认卷」找回真实条目 GUID 再恢复。
                run_cmd_to_file(
                    "cmd /c bcdedit.exe /store S:\\EFI\\Microsoft\\Boot\\BCD /enum {default} > S:\\bcdboot-def-before.txt 2>&1",
                    None,
                );
                let mut old_partition = String::new();
                if let Ok(text) = std::fs::read_to_string("S:\\bcdboot-def-before.txt") {
                    for line in text.lines() {
                        let t = line.trim();
                        if let Some(rest) = t.strip_prefix("device") {
                            // 形如 partition=C:
                            if let Some(p) = rest.find("partition=") {
                                let tail = &rest[p + "partition=".len()..];
                                let letter = tail.trim().chars().next().unwrap_or('?');
                                if letter.is_ascii_alphabetic() {
                                    old_partition = format!("{letter}:");
                                    break;
                                }
                            }
                        }
                    }
                }
                let code = run_cmd_to_file_timeout(
                    &format!(
                        "cmd /c bcdboot.exe {d}:\\Windows /s {esp_letter}: /f UEFI > S:\\bcdboot-out.txt 2>&1"
                    ),
                    None,
                    300000,
                );
                result.push_str(&format!("bcdboot {d}: /s {esp_letter}: code={code}\n"));
                if let Ok(text) = std::fs::read_to_string("S:\\bcdboot-out.txt") {
                    result.push_str(&format!("[BCDBOOT {d}]\n{text}\n"));
                }
                // 恢复 default 与菜单顺序：按原默认卷在 BCD 中找回真实条目并设回默认/第一
                if !old_partition.is_empty() {
                    run_cmd_to_file(
                        "cmd /c bcdedit.exe /store S:\\EFI\\Microsoft\\Boot\\BCD /enum > S:\\bcdboot-after.txt 2>&1",
                        None,
                    );
                    let mut real_guid = String::new();
                    let mut cur_guid = String::new();
                    if let Ok(text) = std::fs::read_to_string("S:\\bcdboot-after.txt") {
                        for line in text.lines() {
                            let t = line.trim();
                            // bcdedit 语言：中文系统输出"标识符"，英文系统输出"identifier"
                            if let Some(rest) = t
                                .strip_prefix("标识符")
                                .or_else(|| t.strip_prefix("identifier"))
                            {
                                if let Some(s) = rest.find('{') {
                                    if let Some(e) = rest[s + 1..].find('}') {
                                        cur_guid = rest[s..s + 1 + e + 1].to_string();
                                        continue;
                                    }
                                }
                            }
                            if cur_guid.is_empty() {
                                continue;
                            }
                            // device partition=X: 且该条目是 osloader（有 path 行）
                            if let Some(rest) = t.strip_prefix("device") {
                                let dev = rest.trim();
                                if dev.contains(&old_partition) && cur_guid != "{bootmgr}" {
                                    real_guid = cur_guid.clone();
                                    break;
                                }
                            }
                        }
                    }
                    if !real_guid.is_empty() {
                        run_cmd_to_file(
                            &format!(
                                "cmd /c bcdedit.exe /store S:\\EFI\\Microsoft\\Boot\\BCD /set {{bootmgr}} default {real_guid} > S:\\bcdboot-restore.txt 2>&1"
                            ),
                            None,
                        );
                        run_cmd_to_file(
                            &format!(
                                "cmd /c bcdedit.exe /store S:\\EFI\\Microsoft\\Boot\\BCD /displayorder {real_guid} /addfirst > S:\\bcdboot-order.txt 2>&1"
                            ),
                            None,
                        );
                        if let Ok(text) = std::fs::read_to_string("S:\\bcdboot-restore.txt") {
                            result.push_str(&format!("[DEFAULT_RESTORE {real_guid}] {text}\n"));
                        }
                    } else {
                        result.push_str(&format!(
                            "bcdboot: default entry for {old_partition} not found after bcdboot\n"
                        ));
                    }
                } else {
                    result.push_str(
                        "bcdboot: no previous default, keeping bcdboot-assigned default\n",
                    );
                }
            } else {
                result.push_str(&format!("bcdboot {d}: skipped (not a system volume)\n"));
            }
        }
        ["add-secondary-entry", drive, ..] => {
            let d = resolve_drive(drive);
            let name = parts[2..].join(" ");
            run_cmd_to_file("mountvol.exe S: /S", None);
            // 创建 osloader 条目（完整字段避免 0xc0000225）
            run_cmd_to_file(
                &format!(
                    "cmd /c bcdedit.exe /store S:\\EFI\\Microsoft\\Boot\\BCD /create /d \"{name}\" /application osloader > S:\\pe-addsec-create.txt 2>&1"
                ),
                None,
            );
            let mut guid = String::new();
            if let Ok(text) = std::fs::read_to_string("S:\\pe-addsec-create.txt") {
                result.push_str(&format!("[ADD_SECONDARY_CREATE]\n{text}\n"));
                // 提取 {guid}（bcdedit 输出 "The entry {xxxx-...} was successfully created."）
                if let Some(start) = text.find('{') {
                    if let Some(end) = text[start + 1..].find('}') {
                        guid = text[start..start + 1 + end + 1].to_string();
                    }
                }
            }
            if guid.is_empty() {
                result.push_str(&format!(
                    "add-secondary-entry: GUID parse failed for {name}\n"
                ));
            } else {
                let steps = [
                    ("device", format!("partition={d}:")),
                    ("osdevice", format!("partition={d}:")),
                    ("path", r"\Windows\system32\winload.efi".to_string()),
                    ("systemroot", r"\Windows".to_string()),
                    ("nx", "OptIn".to_string()),
                ];
                for (field, value) in steps {
                    let code = run_cmd_to_file(
                        &format!(
                            "cmd /c bcdedit.exe /store S:\\EFI\\Microsoft\\Boot\\BCD /set {guid} {field} {value} > S:\\pe-addsec-set.txt 2>&1"
                        ),
                        None,
                    );
                    result.push_str(&format!("add-secondary-entry set {field}: code={code}\n"));
                }
                let code = run_cmd_to_file(
                    &format!(
                        "cmd /c bcdedit.exe /store S:\\EFI\\Microsoft\\Boot\\BCD /displayorder {guid} /addlast > S:\\pe-addsec-order.txt 2>&1"
                    ),
                    None,
                );
                result.push_str(&format!("add-secondary-entry displayorder: code={code}\n"));
                // default 保持不变（部署期已写入 pe-exit-guid.txt 的 Windows 条目 GUID）
            }
        }
        other => result.push_str(&format!("unknown action: {}\n", other.join(" "))),
    }
}

/// 配置驱动的 PE 任务执行：Windows 侧把要执行的动作写入 ESP 的
/// `S:\pe-task.txt`（每行一个动作，`reboot` 表示执行完自动重启回
/// Windows），PE 启动时读取并按配置逐条执行，结果落盘
/// `S:\pe-task-result.txt`，配置改名为 `.done` 防止重复执行。
/// 返回 `true` 表示配置要求执行后自动重启（调用方自动重启，无需用户
/// 操作）；无配置返回 `false`（正常显示 PE 恢复桌面）。
fn pe_task_execute() -> bool {
    // 0. 先挂载 ESP 到 S:——任务配置就存在 S:\pe-task.txt，PE 启动时
    //    S: 尚未挂载，必须先挂载才能读到配置。
    let mount_code = run_cmd_to_file("mountvol.exe S: /S", None);
    let task_file = "S:\\pe-task.txt";
    let Ok(config) = std::fs::read_to_string(task_file) else {
        return false; // 无配置：正常显示 PE 桌面
    };
    let mut reboot = false;
    let mut result = format!("PE task execute start, mountvol={mount_code}\n");
    for line in config.lines() {
        let action = line.trim();
        execute_pe_task_line(action, &mut result, &mut reboot);
    }
    let _ = std::fs::write("S:\\pe-task-result.txt", &result);
    // 配置标记完成（防下次重复执行）
    let _ = std::fs::rename(task_file, "S:\\pe-task.txt.done");
    reboot
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
        // PE 恢复桌面启动：读取 Windows 侧写入 ESP 的任务配置
        // S:\pe-task.txt，按配置执行动作；配置含 reboot 则执行完自动
        // 重启回 Windows，全程无需用户操作。无配置则正常显示 PE 桌面。
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
        // PE 桌面「自动点击」（开发/验收）：存在 S:\pe-click.txt 时，自动
        // 向主窗口投递对应按钮的 WM_COMMAND——与真实鼠标点击走完全相同
        // 的分发路径（window_proc_pe → handler → 确认框自动接受 → 执行 →
        // 恢复 BCD 并重启回 Windows）。配置随即改名 .done 防重复执行。
        let auto_click_id = match read_pe_click_config() {
            Some(("backup", _)) => Some(ID_PE_BACKUP),
            Some(("restore", _)) => Some(ID_PE_RESTORE),
            Some(("secondary", _)) => Some(ID_PE_SECONDARY),
            Some(("exit", _)) => Some(ID_PE_EXIT),
            Some(("main", _)) => Some(ID_PE_MAIN_GUI),
            _ => None,
        };
        if let Some(btn_id) = auto_click_id {
            PE_AUTO_CLICK.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = std::fs::rename("S:\\pe-click.txt", "S:\\pe-click.txt.done");
            PostMessageW(window, WM_COMMAND, btn_id as WParam, 0);
        }
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
                    PostMessageW(window, WM_COMMAND as u32, ID_REFRESH as usize, 0);
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
mod json_text_tests {
    use super::json_text;
    use serde_json::json;

    #[test]
    fn json_text_reads_boolean_values() {
        let value = json!({
            "hasWindowsInstallation": true,
            "name": "C",
            "count": 7,
        });
        assert_eq!(json_text(&value, "hasWindowsInstallation"), "true");
        assert_eq!(json_text(&value, "name"), "C");
        assert_eq!(json_text(&value, "count"), "7");
        assert_eq!(json_text(&value, "missing"), "");
        assert_eq!(
            json_text(
                &json!({"hasWindowsInstallation": false}),
                "hasWindowsInstallation"
            ),
            "false"
        );
    }
}
