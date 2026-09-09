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
use std::path::PathBuf;
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
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SW_MAXIMIZE: i32 = 3;
const MB_OK: u32 = 0x00000000;
const MB_ICONERROR: u32 = 0x00000010;
const MB_YESNO: u32 = 0x00000004;
const MB_ICONWARNING: u32 = 0x00000030;
const IDYES: i32 = 6;

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
const ID_SOURCE: usize = 1202;
const ID_IMAGE: usize = 1203;
const ID_TARGET: usize = 1204;
const ID_INDEX: usize = 1206;
const ID_MENU: usize = 1207;
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
const ID_PE_REBOOT: usize = 1406;
const ID_PE_TITLE: usize = 1407;
const ID_PE_VERSION: usize = 1408;
const ID_PE_CLOCK: usize = 1409;
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
}

#[link(name = "comctl32")]
unsafe extern "system" {
    fn InitCommonControlsEx(init: *const InitCommonControlsEx) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
    fn ExitWindowsEx(flags: u32, reserved: u32) -> i32;
    fn SetTimer(hwnd: Hwnd, id: usize, elapsed: u32, timer_proc: Option<unsafe extern "system" fn(Hwnd, u32, usize, u32)>) -> usize;
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
}

const GWLP_USERDATA: i32 = -21;

#[repr(C)]
struct TokenElevation {
    token_is_elevated: u32,
}

struct Controls {
    language: Hwnd,
    operation_tabs: [Hwnd; 4],
    source: Hwnd,
    image: Hwnd,
    target: Hwnd,
    index: Hwnd,
    menu: Hwnd,
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
        (state.controls.source, "source"),
        (state.controls.target, "target"),
        (state.controls.image, "image"),
        (state.controls.index, "index"),
        (state.controls.menu, "menu"),
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
        (Language::English, "operation") => "Operation",
        (Language::English, "source") => "Windows source",
        (Language::English, "image") => "Image absolute path",
        (Language::English, "target") => "Restore target",
        (Language::English, "index") => "WIM index",
        (Language::English, "menu") => "Secondary boot name",
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
        1 => "backup",
        2 => "restore-existing",
        3 => "create-secondary",
        _ => "probe",
    }
}

unsafe fn set_operation_tabs(state: &State, language: Language) {
    for (index, (button, key)) in state
        .controls
        .operation_tabs
        .iter()
        .zip(["probe", "backup", "restore", "secondary"])
        .enumerate()
    {
        set_text(*button, ui_text(language, key));
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
        (Language::English, "probe") => "Inspect",
        (Language::English, "backup") => "Backup",
        (Language::English, "restore-existing") => "Restore",
        (Language::English, "create-secondary") => "Second system",
        _ => "",
    }
}

fn operation_hint_key(operation: &str) -> &'static str {
    match operation {
        "backup" => "backup_hint",
        "restore-existing" => "restore_hint",
        "create-secondary" => "secondary_hint",
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
    let show_target = matches!(operation, "restore-existing" | "create-secondary");
    let show_index = matches!(operation, "restore-existing" | "create-secondary");
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

    set_visible(state.controls.target, show_target);
    set_visible(state.controls.target_details, show_target);
    set_child_visible(2005, show_target);
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
        "restore-existing" | "create-secondary"
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
    let status_y = last_details_y + 16;
    let image_y = status_y + 84;
    let secondary_y = image_y + 34;
    let buttons_y = if index_visible {
        secondary_y + 40
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

    let image_width = (field_width - 90).max(400);
    reposition(state.controls.image, field_x, image_y, image_width, 24);
    reposition(
        GetDlgItem(state.root, ID_BROWSE_IMAGE as i32),
        field_x + image_width + 10,
        image_y,
        80,
        24,
    );
    let index_width = if selected_operation(state) == "create-secondary" {
        440
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
        (Language::Chinese, _) => ("源卷（检查对象）", "目标卷（不使用）"),
        (Language::English, "backup") => ("Source (backup)", "Target (image)"),
        (Language::English, "restore-existing") => ("Source (current)", "Target (overwrite)"),
        (Language::English, "create-secondary") => ("Source (keep)", "Target (second system)"),
        (Language::English, _) => ("Source (inspect)", "Target (unused)"),
    };
    set_child_text(state.root, 2003, source);
    set_child_text(state.root, 2005, target);
}

unsafe fn select_operation(state: &mut State, index: usize) {
    state.operation_index = index.min(3);
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
    for (id, key) in [
        (2001, "operation"),
        (2003, "source"),
        (2004, "image"),
        (2005, "target"),
        (2007, "index"),
        (2008, "menu"),
        (ID_LANGUAGE_LABEL, "language"),
    ] {
        set_child_text(state.root, id, ui_text(language, key));
    }
    for (id, key) in [
        (ID_REFRESH, "refresh"),
        (ID_READ_IMAGE, "read_image"),
        (ID_BROWSE_IMAGE, "browse"),
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
    let text = wide(text);
    let caption = wide(caption);
    MessageBoxW(hwnd, text.as_ptr(), caption.as_ptr(), flags)
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

unsafe fn create_task(state: &State) {
    let language = selected_language(state);
    let operation = selected_operation(state).to_string();
    append_gui_log(
        state,
        &format!("GUI action started: create task; operation={operation}"),
    );
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
    if matches!(operation.as_str(), "restore-existing" | "create-secondary") {
        arguments.push("--allow-destructive".to_string());
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
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    180,
                    60,
                    115,
                    32,
                    ID_OPERATION_PROBE,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    300,
                    60,
                    115,
                    32,
                    ID_OPERATION_BACKUP,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    420,
                    60,
                    115,
                    32,
                    ID_OPERATION_RESTORE,
                ),
                create_control(
                    hwnd,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTORADIOBUTTON | BS_PUSHLIKE,
                    540,
                    60,
                    115,
                    32,
                    ID_OPERATION_SECONDARY,
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
        create_control(hwnd, "STATIC", "操作模式", 0, 20, 55, 130, 22, 2001);
        create_control(hwnd, "STATIC", "源卷", 0, 20, 222, 150, 26, 2003);
        create_control(hwnd, "STATIC", "目标卷", 0, 20, 332, 150, 26, 2005);
        create_control(hwnd, "STATIC", "镜像绝对路径", 0, 20, 550, 130, 22, 2004);
        create_control(hwnd, "STATIC", "WIM 索引", 0, 20, 590, 130, 22, 2007);
        create_control(hwnd, "STATIC", "第二系统名称", 0, 490, 590, 100, 22, 2008);
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
            WS_TABSTOP,
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
        let state = Box::new(State {
            root: hwnd,
            tooltip: null_mut(),
            controls,
            executable_dir,
            wim_images: Vec::new(),
            drives,
            operation_index: 0,
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
        if let Ok(image) = std::env::var("BACKUPRESTORE_OPEN_IMAGE") {
            if !image.trim().is_empty() {
                select_operation(&mut *state_ptr, 2);
                set_text((*state_ptr).controls.image, &image);
                read_image(&mut *state_ptr);
            }
        }
        if let Ok(tab) = std::env::var("BACKUPRESTORE_OPEN_TAB") {
            if let Ok(index) = tab.parse::<usize>() {
                if (1..=3).contains(&index) {
                    select_operation(&mut *state_ptr, index);
                }
            }
        }
        return 0;
    }
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if !state_ptr.is_null() {
        let state = &mut *state_ptr;
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
                ID_CREATE_TASK => create_task(state),
                ID_REFRESH_TASK => refresh_task_status(state),
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
unsafe fn exit_pe_to_windows(hwnd: Hwnd) {
    let mut volume = [0u16; 512];
    let handle = FindFirstVolumeW(volume.as_mut_ptr(), 512);
    if handle as isize == -1 || handle.is_null() {
        ExitWindowsEx(EWX_REBOOT, 0);
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
            let fs_length = filesystem
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(0);
            let fs = String::from_utf16_lossy(&filesystem[..fs_length]).to_ascii_uppercase();
            if fs == "FAT" || fs == "FAT32" {
                let mount = wide("S:\\");
                if SetVolumeMountPointW(mount.as_ptr(), wide(&volume_path).as_ptr()) != 0 {
                    let bcd = wide("S:\\EFI\\Microsoft\\Boot\\BCD");
                    if GetFileAttributesW(bcd.as_ptr()) != u32::MAX {
                        let arguments = wide(
                            "/store S:\\EFI\\Microsoft\\Boot\\BCD /set {bootmgr} default {current}",
                        );
                        let bcdedit = wide("bcdedit.exe");
                        ShellExecuteW(hwnd, null(), bcdedit.as_ptr(), arguments.as_ptr(), null(), 0);
                        Sleep(1500);
                        found_bcd = true;
                    }
                    DeleteVolumeMountPointW(mount.as_ptr());
                    if found_bcd {
                        break;
                    }
                }
            }
        }
        if FindNextVolumeW(handle, volume.as_mut_ptr(), 512) == 0 {
            break;
        }
    }
    FindVolumeClose(handle);
    ExitWindowsEx(EWX_REBOOT, 0);
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
            (ID_PE_REBOOT, "重启", 2, 1),
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
                    PE_EXIT_TAB.store(1, std::sync::atomic::Ordering::SeqCst);
                    DestroyWindow(hwnd);
                    return 0;
                }
                ID_PE_RESTORE => {
                    PE_EXIT_TAB.store(2, std::sync::atomic::Ordering::SeqCst);
                    DestroyWindow(hwnd);
                    return 0;
                }
                ID_PE_SECONDARY => {
                    PE_EXIT_TAB.store(3, std::sync::atomic::Ordering::SeqCst);
                    DestroyWindow(hwnd);
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
                ID_PE_REBOOT => {
                    if ExitWindowsEx(EWX_REBOOT, 0) == 0 {
                        let wpe = wide("wpeutil.exe");
                        let argument = wide("reboot");
                        ShellExecuteW(hwnd, null(), wpe.as_ptr(), argument.as_ptr(), null(), SW_SHOW);
                    }
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
            TranslateMessage(&message);
            DispatchMessageW(&message);
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
            TranslateMessage(&message);
            DispatchMessageW(&message);
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
