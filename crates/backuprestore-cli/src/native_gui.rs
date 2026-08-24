//! Rust-native Win32 front end.
//!
//! The GUI deliberately uses only Win32 APIs exposed by the Windows SDK. This
//! keeps the offline build self-contained and avoids pulling a GUI framework or
//! another network dependency into the recovery package. Destructive work is
//! still performed by the existing elevated PowerShell preparation contract;
//! this module owns the window, fields, validation, confirmation and status.

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::fs;
use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::ptr::{null, null_mut};

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
const ID_TASK: usize = 1201;
const ID_SOURCE: usize = 1202;
const ID_IMAGE: usize = 1203;
const ID_TARGET: usize = 1204;
const ID_RELATIVE: usize = 1205;
const ID_INDEX: usize = 1206;
const ID_MENU: usize = 1207;
const ID_STATUS: usize = 1300;
const ID_LANGUAGE_LABEL: usize = 2009;
const ID_TASK_DETAILS: usize = 2011;
const ID_SOURCE_DETAILS: usize = 2012;
const ID_TARGET_DETAILS: usize = 2013;
const OFN_PATHMUSTEXIST: u32 = 0x00000800;
const OFN_FILEMUSTEXIST: u32 = 0x00001000;
const OFN_OVERWRITEPROMPT: u32 = 0x00000002;
const CREATE_NO_WINDOW: u32 = 0x08000000;
const DEFAULT_GUI_FONT: i32 = 17;

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
    fn MessageBoxW(hwnd: Hwnd, text: *const u16, caption: *const u16, flags: u32) -> i32;
    fn PostQuitMessage(exit_code: i32);
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

const GWLP_USERDATA: i32 = -21;

struct Controls {
    language: Hwnd,
    operation_tabs: [Hwnd; 4],
    task: Hwnd,
    source: Hwnd,
    image: Hwnd,
    target: Hwnd,
    relative: Hwnd,
    index: Hwnd,
    menu: Hwnd,
    task_details: Hwnd,
    source_details: Hwnd,
    target_details: Hwnd,
    status: Hwnd,
}

struct State {
    root: Hwnd,
    controls: Controls,
    executable_dir: PathBuf,
    wim_images: Vec<WimImageInfo>,
    drives: Vec<DriveInfo>,
    operation_index: usize,
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Language {
    Chinese,
    English,
}

fn ui_text(language: Language, key: &str) -> &'static str {
    match (language, key) {
        (Language::Chinese, "operation") => "操作模式",
        (Language::Chinese, "task") => "任务卷",
        (Language::Chinese, "source") => "源卷",
        (Language::Chinese, "image") => "镜像绝对路径",
        (Language::Chinese, "target") => "目标卷",
        (Language::Chinese, "relative") => "镜像绝对路径",
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
        (Language::English, "task") => "Task volume",
        (Language::English, "source") => "Windows source",
        (Language::English, "image") => "Image absolute path",
        (Language::English, "target") => "Restore target",
        (Language::English, "relative") => "Image absolute path",
        (Language::English, "index") => "WIM index",
        (Language::English, "menu") => "Secondary boot name",
        (Language::English, "language") => "Language / 语言",
        (Language::English, "refresh") => "Refresh environment",
        (Language::English, "read_image") => "Read image",
        (Language::English, "browse") => "Browse…",
        (Language::English, "create_task") => "Create task",
        (Language::English, "refresh_task") => "Refresh task status",
        (Language::English, "initial_status") => {
            "Click Refresh environment to inspect Windows, WinRE and volume identities. Default mode is non-destructive probe."
        }
        (Language::English, "probe") => "probe (inspect only)",
        (Language::English, "backup") => "backup",
        (Language::English, "restore") => "restore-existing (replace current)",
        (Language::English, "secondary") => "create-secondary (add another)",
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

unsafe fn set_text(hwnd: Hwnd, value: &str) {
    // Win32 EDIT controls require CRLF for explicit line breaks. Keeping the
    // conversion here makes guidance/details boxes render logical lines
    // consistently instead of depending on automatic wrapping.
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let value = wide(&normalized.replace('\n', "\r\n"));
    SetWindowTextW(hwnd, value.as_ptr());
}

unsafe fn get_text(hwnd: Hwnd) -> String {
    let length = SendMessageW(hwnd, 0x000e, 0, 0) as usize;
    let mut buffer = vec![0u16; length.saturating_add(1)];
    SendMessageW(hwnd, 0x000d, buffer.len(), buffer.as_mut_ptr() as isize);
    String::from_utf16_lossy(&buffer[..length])
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
        (Language::English, "probe") => "probe (inspect only)",
        (Language::English, "backup") => "backup",
        (Language::English, "restore-existing") => "restore-existing (replace current)",
        (Language::English, "create-secondary") => "create-secondary (add another)",
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
    let task_combo_y = 100;
    let task_details_y = task_combo_y + 36;
    let source_combo_y = task_details_y + details_height + row_gap;
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
    reposition(state.controls.task, field_x, task_combo_y, field_width, 220);
    reposition(
        state.controls.task_details,
        field_x,
        task_details_y,
        field_width,
        details_height,
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
    reposition(state.controls.index, field_x, secondary_y, 440, 220);
    reposition(
        state.controls.menu,
        field_x + 500,
        secondary_y,
        (field_width - 500).max(300),
        24,
    );

    reposition(GetDlgItem(state.root, 2002), 20, task_combo_y + 2, 150, 40);
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
    let (task, source, target) = match (language, operation) {
        (Language::Chinese, "backup") => (
            "任务卷（保存任务、WinRE 与日志）",
            "源卷（要备份的 Windows 分区）",
            "目标卷",
        ),
        (Language::Chinese, "restore-existing") => (
            "任务卷（保存任务、WinRE 与日志）",
            "源卷（要替换的当前 Windows）",
            "目标卷（写入镜像；必须等于源卷）",
        ),
        (Language::Chinese, "create-secondary") => (
            "任务卷（保存任务、WinRE 与日志）",
            "源卷（保留的当前 Windows）",
            "目标卷（写入第二系统；将格式化）",
        ),
        (Language::Chinese, _) => (
            "任务卷（保存任务、WinRE 与日志）",
            "源卷（只核验当前 Windows 身份）",
            "目标卷",
        ),
        (Language::English, "backup") => (
            "Task volume (task, WinRE and logs)",
            "Source volume (Windows to back up)",
            "Target volume",
        ),
        (Language::English, "restore-existing") => (
            "Task volume (task, WinRE and logs)",
            "Source volume (current Windows to replace)",
            "Target volume (receives image; must match source)",
        ),
        (Language::English, "create-secondary") => (
            "Task volume (task, WinRE and logs)",
            "Source volume (current Windows to keep)",
            "Target volume (second system; will be formatted)",
        ),
        (Language::English, _) => (
            "Task volume (task, WinRE and logs)",
            "Source volume (only checks current Windows identity)",
            "Target volume",
        ),
    };
    set_child_text(state.root, 2002, task);
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
    if language == Language::English {
        format!(
            "{}: | {} | {} | total {} | free {} | disk {}/partition {}",
            drive.letter,
            drive.filesystem,
            label,
            format_bytes(drive.size_bytes),
            format_bytes(drive.free_bytes),
            disk,
            partition,
        )
    } else {
        format!(
            "{}: | {} | 卷标 {} | 总容量 {} | 可用 {} | 磁盘 {}/分区 {}",
            drive.letter,
            drive.filesystem,
            label,
            format_bytes(drive.size_bytes),
            format_bytes(drive.free_bytes),
            disk,
            partition,
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
            "{}: {}\nFile system: {}\nTotal: {} | Free: {}\nDisk/partition: {}/{}\nPartition type: {}\nVolume GUID: {}",
            drive.letter,
            if drive.label.is_empty() {
                "no label"
            } else {
                &drive.label
            },
            drive.filesystem,
            format_bytes(drive.size_bytes),
            format_bytes(drive.free_bytes),
            disk,
            partition,
            drive.partition_type_guid,
            drive.volume_guid,
        )
    } else {
        format!(
            "{}: {}\n文件系统：{}\n总容量：{} | 可用：{}\n磁盘/分区：{}/{}\n分区类型：{}\n卷 GUID：{}",
            drive.letter,
            if drive.label.is_empty() {
                "无卷标"
            } else {
                &drive.label
            },
            drive.filesystem,
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
        (Language::Chinese, "task", _) => {
            "任务卷用途：保存任务状态、恢复环境载荷和日志。备份/还原时不能与源卷或目标卷相同；禁止 EFI、MSR、恢复分区。"
        }
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
            "目标卷用途：将被格式化并写入镜像作为第二个 Windows；必须不同于源卷和任务卷。"
        }
        (Language::Chinese, "target", "backup") => {
            "目标卷用途：备份模式不会写入此卷；镜像保存位置由“镜像绝对路径”决定。"
        }
        (Language::Chinese, "target", _) => {
            "目标卷用途：探测模式只核验身份；不会备份、还原、格式化或写入。"
        }
        (Language::English, "task", _) => {
            "Purpose: stores task state, WinRE payload and logs. For backup/restore it must differ from source and target; EFI, MSR and Recovery are forbidden."
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
            "Purpose: will be formatted and receive the second Windows; it must differ from source and task."
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
        ("task", selected_drive_letter(state, state.controls.task)),
        (
            "source",
            selected_drive_letter(state, state.controls.source),
        ),
        (
            "target",
            selected_drive_letter(state, state.controls.target),
        ),
    ];
    let detail_controls = [
        state.controls.task_details,
        state.controls.source_details,
        state.controls.target_details,
    ];
    let operation = selected_operation(state);
    for (role, letter) in selected {
        let index = match role {
            "task" => 0,
            "source" => 1,
            _ => 2,
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
    let controls = [
        state.controls.task,
        state.controls.source,
        state.controls.target,
    ];
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
    for (control, wanted) in controls.into_iter().zip(desired) {
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
    let items = if let Some(items) = value.as_array() {
        items.clone()
    } else if value.get("ImageIndex").is_some() {
        vec![value]
    } else {
        return Err("WIM metadata did not return an image object or array".to_string());
    };
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

fn report_has_wim_images(report: &serde_json::Value) -> bool {
    match report.get("images") {
        Some(serde_json::Value::Array(items)) => !items.is_empty(),
        Some(serde_json::Value::Object(item)) => item.get("ImageIndex").is_some(),
        _ => false,
    }
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
    SendMessageW(state.controls.index, CB_SETCURSEL, selected_position, 0);
}

unsafe fn apply_language(state: &State) {
    let language = selected_language(state);
    set_operation_tabs(state, language);
    let desired = [
        selected_drive_letter(state, state.controls.task),
        selected_drive_letter(state, state.controls.source),
        selected_drive_letter(state, state.controls.target),
    ];
    set_drive_items(state, desired);
    set_wim_items(state);
    for (id, key) in [
        (2001, "operation"),
        (2002, "task"),
        (2003, "source"),
        (2004, "image"),
        (2005, "target"),
        (2006, "relative"),
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
    set_drive_details(state);
    set_volume_labels(state);
    set_operation_visibility(state);
    layout_operation(state);
    set_operation_guidance(state);
}

fn quote_argument(value: &str) -> String {
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn powershell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn powershell_output_elevated(command: &str) -> String {
    let nonce = format!("{}-{}", std::process::id(), std::process::id());
    let script_path = std::env::temp_dir().join(format!("BackupRestore-wim-{nonce}.ps1"));
    let output_path = std::env::temp_dir().join(format!("BackupRestore-wim-{nonce}.json"));
    let _ = fs::remove_file(&script_path);
    let _ = fs::remove_file(&output_path);
    let script = format!(
        "$ErrorActionPreference='Stop'; try {{ $result = & {{ {command} }} | Out-String; Set-Content -LiteralPath {output} -Value $result -Encoding UTF8; exit 0 }} catch {{ Set-Content -LiteralPath {output} -Value ($_ | Out-String) -Encoding UTF8; exit 1 }}",
        output = powershell_single_quote(&output_path.to_string_lossy()),
    );
    if let Err(error) = fs::write(&script_path, script) {
        return format!("elevated WIM reader setup failed: {error}");
    }
    let launcher = format!(
        "$child=Start-Process -FilePath powershell.exe -Verb RunAs -WindowStyle Hidden -Wait -PassThru -ArgumentList @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',{script}); if($child.ExitCode -ne 0){{ exit $child.ExitCode }}",
        script = powershell_single_quote(&script_path.to_string_lossy()),
    );
    let launch_result = Command::new("powershell.exe")
        .creation_flags(CREATE_NO_WINDOW)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &launcher,
        ])
        .output();
    let text = match launch_result {
        Ok(_output) if output_path.exists() => fs::read_to_string(&output_path)
            .unwrap_or_else(|error| format!("elevated WIM reader output failed: {error}")),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.is_empty() {
                format!("elevated WIM reader exited with code {}", output.status)
            } else {
                stderr.into_owned()
            }
        }
        Err(error) => format!("elevated WIM reader launch failed: {error}"),
    };
    let _ = fs::remove_file(&script_path);
    let _ = fs::remove_file(&output_path);
    text.trim_start_matches('\u{feff}').trim().to_string()
}

fn powershell_output(command: &str) -> String {
    match Command::new("powershell.exe")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .output()
    {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            if text.is_empty() {
                text = String::from_utf8_lossy(&output.stderr).into_owned();
            }
            text.trim().to_string()
        }
        Err(error) => format!("环境检查失败：{error}"),
    }
}

unsafe fn show_message(hwnd: Hwnd, text: &str, caption: &str, flags: u32) -> i32 {
    let text = wide(text);
    let caption = wide(caption);
    MessageBoxW(hwnd, text.as_ptr(), caption.as_ptr(), flags)
}

unsafe fn refresh_environment(state: &mut State) {
    let language = selected_language(state);
    let desired = [
        selected_drive_letter(state, state.controls.task),
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
    let text = powershell_output(
        r#"$os=Get-CimInstance Win32_OperatingSystem; $fw=(Get-ComputerInfo -Property BiosFirmwareType).BiosFirmwareType; $vol=@(Get-Volume | ? DriveLetter | ? FileSystem -eq 'NTFS' | % { "$($_.DriveLetter): $($_.FileSystem) free=$($_.SizeRemaining)" }); @("Windows: $($os.Caption) build=$($os.BuildNumber) arch=$env:PROCESSOR_ARCHITECTURE","Firmware: $fw",'NTFS volumes:') + $vol -join [Environment]::NewLine"#,
    );
    let text = if language == Language::English {
        text
    } else {
        text.replace("Windows:", "Windows：")
            .replace("Firmware:", "固件：")
            .replace("NTFS volumes:", "NTFS 卷：")
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
}

unsafe fn refresh_task_status(state: &State) {
    let language = selected_language(state);
    set_text(
        state.controls.status,
        if language == Language::English {
            "Reading the latest task status…"
        } else {
            "正在读取最近任务状态…"
        },
    );
    let text = powershell_output(
        r#"$p=Join-Path $env:ProgramData 'BackupRestore\last-task.json'; if(-not(Test-Path -LiteralPath $p)){ '尚未找到最近任务记录。' } else { try { $r=Get-Content -LiteralPath $p -Raw|ConvertFrom-Json; $lines=@("任务 ID：$($r.taskId)","操作：$($r.operation)","任务目录：$($r.taskRoot)","准备日志：$($r.prepareLog)","恢复日志：$($r.recoveryLog)"); if($r.statusJson -and (Test-Path -LiteralPath $r.statusJson)){ $s=Get-Content -LiteralPath $r.statusJson -Raw|ConvertFrom-Json; $lines += "状态：$($s|ConvertTo-Json -Compress)" } else { $lines += '状态：status.json 不可读或尚未生成' }; $lines -join [Environment]::NewLine } catch { "读取任务状态失败：$($_.Exception.Message)" } }"#,
    );
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
}

fn discover_drives() -> Vec<DriveInfo> {
    let output = powershell_output(
        r#"$reserved=@('{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}','{e3c9e316-0b5c-4db8-817d-f92df00215ae}','{de94bba4-06d1-4d40-a16a-bfd50179d6ac}'); $items=@(Get-Volume -ErrorAction SilentlyContinue | ? { $_.DriveLetter -and $_.FileSystem } | % { $v=$_; $p=Get-Partition -DriveLetter $v.DriveLetter -ErrorAction SilentlyContinue; if($p -and $reserved -notcontains "$($p.GptType)") { [ordered]@{letter="$($v.DriveLetter)";label="$($v.FileSystemLabel)";filesystem="$($v.FileSystem)";sizeBytes=[UInt64]$v.Size;freeBytes=[UInt64]$v.SizeRemaining;volumeGuid="$($v.UniqueId)";diskNumber=[int]$p.DiskNumber;partitionNumber=[int]$p.PartitionNumber;partitionTypeGuid="$($p.GptType)"} } }); $items | ConvertTo-Json -Compress -Depth 4"#,
    );
    parse_drive_infos(&output).unwrap_or_default()
}

fn suggested_drive_defaults(drives: &[DriveInfo]) -> (String, String, String) {
    let system = std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".to_string())
        .trim()
        .trim_end_matches(':')
        .to_ascii_uppercase();
    let is_image = |drive: &DriveInfo| {
        PathBuf::from(format!(r"{}:\BackupRestore\Windows.wim", drive.letter)).exists()
    };
    let image = drives
        .iter()
        .find(|drive| is_image(drive))
        .or_else(|| drives.iter().find(|drive| drive.letter != system));
    let task = drives.iter().find(|drive| {
        drive.letter != system && image.is_none_or(|image| image.letter != drive.letter)
    });
    (
        system,
        task.map(|drive| drive.letter.clone()).unwrap_or_default(),
        image.map(|drive| drive.letter.clone()).unwrap_or_default(),
    )
}

unsafe fn read_image(state: &mut State) {
    let language = selected_language(state);
    let image_path = get_text(state.controls.image).trim().to_string();
    if let Err(error) = backuprestore_core::validate_absolute_path(&image_path) {
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
    let command = format!(
        "$p={}; if(-not(Test-Path -LiteralPath $p)){{throw \"WIM not found: $p\"}}; if(-not(Get-Command Get-WindowsImage -ErrorAction SilentlyContinue)){{throw \"Get-WindowsImage is unavailable\"}}; $h=(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant(); $metadataPath=Join-Path (Split-Path -Parent $p) 'metadata.json'; $metadata=if(Test-Path -LiteralPath $metadataPath){{Get-Content -LiteralPath $metadataPath -Raw|ConvertFrom-Json}}else{{$null}}; $images=@(Get-WindowsImage -ImagePath $p -ErrorAction Stop|Select-Object ImageIndex,ImageName,ImageDescription,ImageVersion,Architecture,EditionId,InstallationType,ImageSize); [ordered]@{{image=$p;sha256=$h;metadataSha256=if($metadata){{$metadata.imageSha256}}else{{''}};minimumTarget=if($metadata){{$metadata.minimumTargetSize}}else{{''}};images=$images}}|ConvertTo-Json -Compress -Depth 4",
        powershell_single_quote(&image_path),
    );
    let mut text = powershell_output(&command);
    let mut report_result = serde_json::from_str::<serde_json::Value>(&text);
    let needs_elevation = report_result
        .as_ref()
        .map(|report| !report_has_wim_images(report))
        .unwrap_or(true);
    if needs_elevation {
        let elevated_text = powershell_output_elevated(&command);
        if let Ok(elevated_report) = serde_json::from_str::<serde_json::Value>(&elevated_text) {
            text = elevated_text;
            report_result = Ok(elevated_report);
        }
    }
    let report: serde_json::Value = match report_result {
        Ok(value) => value,
        Err(error) => {
            state.wim_images.clear();
            set_wim_items(state);
            set_text(
                state.controls.status,
                &if language == Language::English {
                    format!("Could not read WIM metadata: {error}\n{text}")
                } else {
                    format!("读取 WIM 详细信息失败：{error}\n{text}")
                },
            );
            return;
        }
    };
    let images_json = report
        .get("images")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    let images_text = match serde_json::to_string(&images_json) {
        Ok(value) => value,
        Err(error) => {
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
        }
        Err(error) => {
            state.wim_images.clear();
            set_wim_items(state);
            set_text(
                state.controls.status,
                &if language == Language::English {
                    format!("Could not parse WIM indexes: {error}")
                } else {
                    format!("无法解析 WIM 索引：{error}")
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
                OFN_OVERWRITEPROMPT
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
    }
}

unsafe fn create_task(state: &State) {
    let language = selected_language(state);
    let operation = selected_operation(state).to_string();
    let index = selected_wim_index(state);
    if matches!(operation.as_str(), "restore-existing" | "create-secondary") && index.is_none() {
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
    let task_drive = match selected_or_error(
        state.controls.task,
        if language == Language::English {
            "Task volume"
        } else {
            "任务卷"
        },
    ) {
        Ok(value) => value,
        Err(error) => {
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
    let image_path = get_text(state.controls.image).trim().to_string();
    if operation != "probe"
        && let Err(error) = backuprestore_core::validate_absolute_path(&image_path)
    {
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
    let image_path = image_path;
    let image_drive = image_path
        .chars()
        .next()
        .map(|value| value.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    if operation != "probe" && image_drive == source_drive {
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
    let operation_label = operation_display(language, &operation);
    let summary = if language == Language::English {
        format!(
            "Mode: {operation_label}\nTask volume: {task_drive}\nSource volume: {source_drive}\nImage: {image_path}\nRestore target: {target_drive}\nWIM index: {index}",
        )
    } else {
        format!(
            "模式：{operation_label}\n任务卷：{task_drive}\n源卷：{source_drive}\n镜像绝对路径：{image_path}\n目标卷：{target_drive}\nWIM 索引：{index}",
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
    let script = state.executable_dir.join("BackupRestore.ps1");
    let mut arguments = vec![
        "-NoProfile".to_string(),
        "-WindowStyle".to_string(),
        "Hidden".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        script.to_string_lossy().into_owned(),
        "-Operation".to_string(),
        operation.clone(),
        "-TaskDrive".to_string(),
        task_drive.clone(),
        "-SourceDrive".to_string(),
        source_drive,
        "-TargetDrive".to_string(),
        target_drive,
        "-ImagePath".to_string(),
        image_path,
        "-WimIndex".to_string(),
        index,
        "-BootMenuName".to_string(),
        get_text(state.controls.menu),
    ];
    if matches!(operation.as_str(), "restore-existing" | "create-secondary") {
        arguments.push("-AllowDestructive".to_string());
    }
    if operation == "probe" {
        arguments.push("-NoReboot".to_string());
    }
    let params = arguments
        .iter()
        .map(|argument| quote_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let runas = wide("runas");
    let powershell = wide("powershell.exe");
    let params = wide(&params);
    let result = ShellExecuteW(
        state.root,
        runas.as_ptr(),
        powershell.as_ptr(),
        params.as_ptr(),
        null(),
        SW_HIDE,
    );
    if result <= 32 {
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
        set_text(
            state.controls.status,
            if operation == "probe" {
                if language == Language::English {
                    "Probe preparation started with -NoReboot. Check status.json and logs; no backup, restore or reboot will run."
                } else {
                    "已启动探测准备流程（不重启）。请查看任务状态和日志；不会备份、还原或格式化。"
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
        let (system_drive, task_drive, image_drive) = suggested_drive_defaults(&drives);
        let image_path = if image_drive.is_empty() {
            String::new()
        } else {
            format!(r"{}:\BackupRestore\Windows.wim", image_drive)
        };
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
            task: create_control(
                hwnd,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST | WS_TABSTOP,
                180,
                110,
                800,
                220,
                ID_TASK,
            ),
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
            relative: create_control(
                hwnd,
                "EDIT",
                "",
                WS_BORDER | WS_TABSTOP,
                180,
                585,
                300,
                24,
                ID_RELATIVE,
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
            task_details: create_control(
                hwnd,
                "EDIT",
                "",
                WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY | WS_VSCROLL,
                180,
                140,
                800,
                70,
                ID_TASK_DETAILS,
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
        ShowWindow(controls.relative, 0);
        add_combo_item(controls.language, "中文");
        add_combo_item(controls.language, "English");
        SendMessageW(controls.language, CB_SETCURSEL, 0, 0);
        SendMessageW(controls.operation_tabs[0], BM_SETCHECK, BST_CHECKED, 0);
        create_control(hwnd, "STATIC", "操作模式", 0, 20, 55, 130, 22, 2001);
        create_control(hwnd, "STATIC", "任务卷", 0, 20, 112, 150, 26, 2002);
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
            controls,
            executable_dir,
            wim_images: Vec::new(),
            drives,
            operation_index: 0,
        });
        let state_ptr = Box::into_raw(state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
        set_drive_items(
            &*state_ptr,
            [
                Some(task_drive),
                Some(system_drive.clone()),
                Some(system_drive),
            ],
        );
        apply_language(&*state_ptr);
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
                return 0;
            }
            if matches!(control_id, ID_TASK | ID_SOURCE | ID_TARGET)
                && notification == CBN_SELCHANGE
            {
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
        let title = wide("BackupRestore - Rust GUI");
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
