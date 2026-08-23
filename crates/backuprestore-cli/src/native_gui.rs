//! Rust-native Win32 front end.
//!
//! The GUI deliberately uses only Win32 APIs exposed by the Windows SDK. This
//! keeps the offline build self-contained and avoids pulling a GUI framework or
//! another network dependency into the recovery package. Destructive work is
//! still performed by the existing elevated PowerShell preparation contract;
//! this module owns the window, fields, validation, confirmation and status.

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::mem::size_of;
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
const CBS_DROPDOWNLIST: u32 = 0x0003;
const CB_ADDSTRING: u32 = 0x0143;
const CB_RESETCONTENT: u32 = 0x014b;
const CB_SETCURSEL: u32 = 0x014e;
const CB_GETCURSEL: u32 = 0x0147;
const CBN_SELCHANGE: usize = 1;
const SW_SHOWNORMAL: i32 = 1;
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
const ID_OPERATION: usize = 1100;
const ID_TASK: usize = 1101;
const ID_SOURCE: usize = 1102;
const ID_IMAGE: usize = 1103;
const ID_TARGET: usize = 1104;
const ID_RELATIVE: usize = 1105;
const ID_INDEX: usize = 1106;
const ID_MENU: usize = 1107;
const ID_STATUS: usize = 1200;
const ID_LANGUAGE_LABEL: usize = 2009;
const OFN_PATHMUSTEXIST: u32 = 0x00000800;
const OFN_FILEMUSTEXIST: u32 = 0x00001000;
const OFN_OVERWRITEPROMPT: u32 = 0x00000002;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
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
    fn TranslateMessage(message: *const Msg) -> i32;
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
    operation: Hwnd,
    task: Hwnd,
    source: Hwnd,
    image: Hwnd,
    target: Hwnd,
    relative: Hwnd,
    index: Hwnd,
    menu: Hwnd,
    status: Hwnd,
}

struct State {
    root: Hwnd,
    controls: Controls,
    executable_dir: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Language {
    Chinese,
    English,
}

fn ui_text(language: Language, key: &str) -> &'static str {
    match (language, key) {
        (Language::Chinese, "operation") => "操作模式",
        (Language::Chinese, "task") => "任务卷盘符",
        (Language::Chinese, "source") => "Windows 源盘符",
        (Language::Chinese, "image") => "镜像绝对路径",
        (Language::Chinese, "target") => "还原目标盘符",
        (Language::Chinese, "relative") => "镜像绝对路径",
        (Language::Chinese, "index") => "WIM 索引",
        (Language::Chinese, "menu") => "第二系统名称",
        (Language::Chinese, "language") => "语言 / Language",
        (Language::Chinese, "refresh") => "刷新环境",
        (Language::Chinese, "read_image") => "读取镜像",
        (Language::Chinese, "browse") => "浏览…",
        (Language::Chinese, "create_task") => "创建任务",
        (Language::Chinese, "refresh_task") => "刷新任务状态",
        (Language::Chinese, "initial_status") => {
            "先点击“刷新环境”确认 Windows、WinRE 和卷身份。默认模式为无破坏 probe。"
        }
        (Language::Chinese, "probe") => "probe（探测）",
        (Language::Chinese, "backup") => "backup（备份）",
        (Language::Chinese, "restore") => "restore-existing（单系统还原）",
        (Language::Chinese, "secondary") => "create-secondary（第二系统）",
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
        (Language::English, "probe") => "probe",
        (Language::English, "backup") => "backup",
        (Language::English, "restore") => "restore-existing",
        (Language::English, "secondary") => "create-secondary",
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
    CreateWindowExW(
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
    )
}

unsafe fn set_text(hwnd: Hwnd, value: &str) {
    let value = wide(value);
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

unsafe fn set_operation_items(state: &State, language: Language) {
    let selected = combo_index(state.controls.operation);
    reset_combo(state.controls.operation);
    for key in ["probe", "backup", "restore", "secondary"] {
        add_combo_item(state.controls.operation, ui_text(language, key));
    }
    SendMessageW(state.controls.operation, CB_SETCURSEL, selected.min(3), 0);
}

unsafe fn selected_operation(state: &State) -> &'static str {
    match combo_index(state.controls.operation) {
        1 => "backup",
        2 => "restore-existing",
        3 => "create-secondary",
        _ => "probe",
    }
}

unsafe fn apply_language(state: &State) {
    let language = selected_language(state);
    set_operation_items(state, language);
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
    set_text(state.controls.status, ui_text(language, "initial_status"));
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

fn powershell_output(command: &str) -> String {
    match Command::new("powershell.exe")
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

unsafe fn refresh_environment(state: &State) {
    let language = selected_language(state);
    set_text(
        state.controls.status,
        if language == Language::English {
            "Refreshing Windows, WinRE and NTFS volume information…"
        } else {
            "正在刷新 Windows、WinRE 和 NTFS 卷信息…"
        },
    );
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
    set_text(state.controls.status, &text);
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

fn suggested_drive_defaults() -> (String, String, String) {
    let system = std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".to_string())
        .trim()
        .trim_end_matches(':')
        .to_ascii_uppercase();
    let output = powershell_output(
        r#"$system=$env:SystemDrive.TrimEnd(':').ToUpperInvariant(); $candidates=@(Get-Volume -ErrorAction SilentlyContinue | ? DriveLetter | ? FileSystem -eq 'NTFS' | % { $p=Get-Partition -DriveLetter $_.DriveLetter -ErrorAction SilentlyContinue; if($p -and $p.Type -notin @('Recovery','System','Reserved')) { "$($_.DriveLetter)".ToUpperInvariant() } } | sort -Unique); $task=$candidates|? { $_ -ne $system }|select -First 1; $image=$candidates|? { $_ -ne $system -and $_ -ne $task }|select -First 1; "$system|$task|$image""#,
    );
    let parts = output.split('|').map(str::trim).collect::<Vec<_>>();
    if parts.len() == 3 && parts[0].len() == 1 && parts[1].len() <= 1 && parts[2].len() <= 1 {
        return (
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
        );
    }
    (system, String::new(), String::new())
}

fn normalize_drive(value: String, label: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches(':').to_ascii_uppercase();
    if value.len() == 1 && value.as_bytes()[0].is_ascii_alphabetic() {
        Ok(value)
    } else {
        Err(format!("{label}必须是单个盘符，例如 C。"))
    }
}

unsafe fn read_image(state: &State) {
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
        "$p={}; if(-not(Test-Path -LiteralPath $p)){{throw \"WIM not found: $p\"}}; $h=(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant(); $m=Get-Content -LiteralPath (Join-Path (Split-Path -Parent $p) 'metadata.json') -Raw|ConvertFrom-Json; \"image=$p`nsha256=$h`nmetadata=$($m.imageSha256)`nminimumTarget=$($m.minimumTargetSize)\"",
        powershell_single_quote(&image_path),
    );
    let text = powershell_output(&command);
    let text = if language == Language::English {
        text.replace("image=", "Image: ")
            .replace("sha256=", "SHA-256: ")
            .replace("metadata=", "Metadata SHA-256: ")
            .replace("minimumTarget=", "Minimum target size: ")
    } else {
        text.replace("image=", "镜像：")
            .replace("sha256=", "SHA-256：")
            .replace("metadata=", "metadata SHA-256：")
            .replace("minimumTarget=", "最小目标容量：")
    };
    set_text(state.controls.status, &text);
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
    let index = get_text(state.controls.index);
    if index
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .is_none()
    {
        show_message(
            state.root,
            if language == Language::English {
                "WIM index must be a positive integer."
            } else {
                "WIM 索引必须是大于等于 1 的整数。"
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
    let task_drive = match normalize_drive(
        get_text(state.controls.task),
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
    let source_drive = match normalize_drive(
        get_text(state.controls.source),
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
    let target_drive = match normalize_drive(
        get_text(state.controls.target),
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
    let summary = if language == Language::English {
        format!(
            "Mode: {operation}\nTask volume: {task_drive}\nSource volume: {source_drive}\nImage: {image_path}\nRestore target: {target_drive}\nWIM index: {index}",
        )
    } else {
        format!(
            "模式：{operation}\n任务卷：{task_drive}\n源卷：{source_drive}\n镜像绝对路径：{image_path}\n目标卷：{target_drive}\nWIM 索引：{index}",
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
        SW_SHOWNORMAL,
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
            "已启动管理员准备脚本。请在任务结果中查看 status.json、prepare.log 和 Recovery.log；这不是恢复成功证明。",
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
        let (system_drive, task_drive, image_drive) = suggested_drive_defaults();
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
                430,
                18,
                120,
                300,
                ID_LANGUAGE,
            ),
            operation: create_control(
                hwnd,
                "COMBOBOX",
                "probe",
                CBS_DROPDOWNLIST | WS_TABSTOP,
                160,
                52,
                220,
                300,
                ID_OPERATION,
            ),
            task: create_control(
                hwnd,
                "EDIT",
                &task_drive,
                WS_BORDER | WS_TABSTOP,
                160,
                88,
                220,
                24,
                ID_TASK,
            ),
            source: create_control(
                hwnd,
                "EDIT",
                &system_drive,
                WS_BORDER | WS_TABSTOP,
                160,
                124,
                220,
                24,
                ID_SOURCE,
            ),
            image: create_control(
                hwnd,
                "EDIT",
                &image_path,
                WS_BORDER | WS_TABSTOP,
                160,
                160,
                360,
                24,
                ID_IMAGE,
            ),
            target: create_control(
                hwnd,
                "EDIT",
                &system_drive,
                WS_BORDER | WS_TABSTOP,
                160,
                196,
                220,
                24,
                ID_TARGET,
            ),
            relative: create_control(
                hwnd,
                "EDIT",
                "",
                WS_BORDER | WS_TABSTOP,
                160,
                232,
                300,
                24,
                ID_RELATIVE,
            ),
            index: create_control(
                hwnd,
                "EDIT",
                "1",
                WS_BORDER | WS_TABSTOP,
                160,
                268,
                220,
                24,
                ID_INDEX,
            ),
            menu: create_control(
                hwnd,
                "EDIT",
                "Windows Backup",
                WS_BORDER | WS_TABSTOP,
                160,
                304,
                300,
                24,
                ID_MENU,
            ),
            status: create_control(
                hwnd,
                "EDIT",
                "先点击“刷新环境”确认 Windows、WinRE 和卷身份。默认模式为无破坏 probe。",
                WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | WS_VSCROLL,
                20,
                390,
                540,
                150,
                ID_STATUS,
            ),
        };
        ShowWindow(controls.relative, 0);
        add_combo_item(controls.language, "中文");
        add_combo_item(controls.language, "English");
        SendMessageW(controls.language, CB_SETCURSEL, 0, 0);
        add_combo_item(controls.operation, ui_text(Language::Chinese, "probe"));
        add_combo_item(controls.operation, ui_text(Language::Chinese, "backup"));
        add_combo_item(controls.operation, ui_text(Language::Chinese, "restore"));
        add_combo_item(controls.operation, ui_text(Language::Chinese, "secondary"));
        SendMessageW(controls.operation, CB_SETCURSEL, 0, 0);
        create_control(hwnd, "STATIC", "操作模式", 0, 20, 55, 130, 22, 2001);
        create_control(hwnd, "STATIC", "任务卷盘符", 0, 20, 91, 130, 22, 2002);
        create_control(hwnd, "STATIC", "Windows 源盘符", 0, 20, 127, 130, 22, 2003);
        create_control(hwnd, "STATIC", "镜像绝对路径", 0, 20, 163, 130, 22, 2004);
        create_control(hwnd, "STATIC", "还原目标盘符", 0, 20, 199, 130, 22, 2005);
        create_control(hwnd, "STATIC", "WIM 索引", 0, 20, 271, 130, 22, 2007);
        create_control(hwnd, "STATIC", "第二系统名称", 0, 20, 307, 130, 22, 2008);
        create_control(
            hwnd,
            "STATIC",
            ui_text(Language::Chinese, "language"),
            0,
            430,
            0,
            120,
            18,
            ID_LANGUAGE_LABEL,
        );
        create_control(
            hwnd,
            "BUTTON",
            "刷新环境",
            WS_TABSTOP,
            20,
            350,
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
            350,
            120,
            28,
            ID_READ_IMAGE,
        );
        create_control(
            hwnd,
            "BUTTON",
            "浏览…",
            WS_TABSTOP,
            525,
            160,
            60,
            24,
            ID_BROWSE_IMAGE,
        );
        create_control(
            hwnd,
            "BUTTON",
            "创建任务",
            WS_TABSTOP,
            280,
            350,
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
            350,
            140,
            28,
            ID_REFRESH_TASK,
        );
        let state = Box::new(State {
            root: hwnd,
            controls,
            executable_dir,
        });
        let state_ptr = Box::into_raw(state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
        apply_language(&*state_ptr);
        return 0;
    }
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if !state_ptr.is_null() {
        let state = &*state_ptr;
        if message == WM_COMMAND {
            let control_id = w_param & 0xffff;
            let notification = (w_param >> 16) & 0xffff;
            if control_id == ID_LANGUAGE && notification == CBN_SELCHANGE {
                apply_language(state);
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
            600,
            610,
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        );
        if window.is_null() {
            return Err(super::err("CreateWindowExW failed"));
        }
        ShowWindow(window, SW_SHOWNORMAL);
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
