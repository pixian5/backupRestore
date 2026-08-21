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
const CB_SETCURSEL: u32 = 0x014e;
const CB_GETCURSEL: u32 = 0x0147;
const CB_GETLBTEXTLEN: u32 = 0x0149;
const CB_GETLBTEXT: u32 = 0x0148;
const SW_SHOWNORMAL: i32 = 1;
const MB_OK: u32 = 0x00000000;
const MB_ICONERROR: u32 = 0x00000010;
const MB_YESNO: u32 = 0x00000004;
const MB_ICONWARNING: u32 = 0x00000030;
const IDYES: i32 = 6;

const ID_REFRESH: usize = 1001;
const ID_READ_IMAGE: usize = 1002;
const ID_CREATE_TASK: usize = 1003;
const ID_OPERATION: usize = 1100;
const ID_TASK: usize = 1101;
const ID_SOURCE: usize = 1102;
const ID_IMAGE: usize = 1103;
const ID_TARGET: usize = 1104;
const ID_RELATIVE: usize = 1105;
const ID_INDEX: usize = 1106;
const ID_MENU: usize = 1107;
const ID_STATUS: usize = 1200;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
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
    fn MessageBoxW(hwnd: Hwnd, text: *const u16, caption: *const u16, flags: u32) -> i32;
    fn PostQuitMessage(exit_code: i32);
    fn SendMessageW(hwnd: Hwnd, message: u32, w_param: WParam, l_param: LParam) -> LResult;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn ShowWindow(hwnd: Hwnd, command: i32) -> i32;
    fn TranslateMessage(message: *const Msg) -> i32;
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

unsafe fn combo_text(hwnd: Hwnd) -> String {
    let index = SendMessageW(hwnd, CB_GETCURSEL, 0, 0);
    if index < 0 {
        return String::new();
    }
    let length = SendMessageW(hwnd, CB_GETLBTEXTLEN, index as usize, 0);
    if length < 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length as usize + 1];
    SendMessageW(
        hwnd,
        CB_GETLBTEXT,
        index as usize,
        buffer.as_mut_ptr() as isize,
    );
    String::from_utf16_lossy(&buffer[..length as usize])
}

unsafe fn add_combo_item(hwnd: Hwnd, value: &str) {
    let value = wide(value);
    SendMessageW(hwnd, CB_ADDSTRING, 0, value.as_ptr() as isize);
}

fn quote_argument(value: &str) -> String {
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
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
    let text = powershell_output(
        r#"$os=Get-CimInstance Win32_OperatingSystem; $fw=(Get-ComputerInfo -Property BiosFirmwareType).BiosFirmwareType; $vol=@(Get-Volume | ? DriveLetter | ? FileSystem -eq 'NTFS' | % { "$($_.DriveLetter): $($_.FileSystem) free=$($_.SizeRemaining)" }); @("Windows: $($os.Caption) build=$($os.BuildNumber) arch=$env:PROCESSOR_ARCHITECTURE","Firmware: $fw",'NTFS volumes:', $vol) -join [Environment]::NewLine"#,
    );
    set_text(state.controls.status, &text);
}

unsafe fn read_image(state: &State) {
    let image = get_text(state.controls.image)
        .trim()
        .trim_end_matches(':')
        .to_string();
    let relative = get_text(state.controls.relative);
    let command = format!(
        "$p=Join-Path '{}:\\' '{}'; if(-not(Test-Path -LiteralPath $p)){{throw \"WIM not found: $p\"}}; $h=(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant(); $m=Get-Content -LiteralPath (Join-Path (Split-Path -Parent $p) 'metadata.json') -Raw|ConvertFrom-Json; \"image=$p`nsha256=$h`nmetadata=$($m.imageSha256)`nminimumTarget=$($m.minimumTargetSize)\"",
        image.replace('"', "''"),
        relative.replace('"', "''"),
    );
    let text = powershell_output(&command);
    set_text(state.controls.status, &text);
}

unsafe fn create_task(state: &State) {
    let operation = combo_text(state.controls.operation);
    let index = get_text(state.controls.index);
    if index
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .is_none()
    {
        show_message(
            state.root,
            "WIM 索引必须是大于等于 1 的整数。",
            "参数校验失败",
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let summary = format!(
        "模式：{operation}\n任务卷：{}\n源卷：{}\n镜像卷：{}\n目标卷：{}\n镜像：{}\\{}\nWIM 索引：{index}",
        get_text(state.controls.task),
        get_text(state.controls.source),
        get_text(state.controls.image),
        get_text(state.controls.target),
        get_text(state.controls.image),
        get_text(state.controls.relative),
    );
    if matches!(operation.as_str(), "restore-existing" | "create-secondary")
        && show_message(
            state.root,
            &format!("{summary}\n\n目标分区将被覆盖，确认继续？"),
            "破坏性确认",
            MB_YESNO | MB_ICONWARNING,
        ) != IDYES
    {
        set_text(state.controls.status, "用户取消了破坏性任务。");
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
        get_text(state.controls.task)
            .trim()
            .trim_end_matches(':')
            .to_string(),
        "-SourceDrive".to_string(),
        get_text(state.controls.source)
            .trim()
            .trim_end_matches(':')
            .to_string(),
        "-ImageDrive".to_string(),
        get_text(state.controls.image)
            .trim()
            .trim_end_matches(':')
            .to_string(),
        "-TargetDrive".to_string(),
        get_text(state.controls.target)
            .trim()
            .trim_end_matches(':')
            .to_string(),
        "-ImageRelativePath".to_string(),
        get_text(state.controls.relative),
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
            &format!("无法启动管理员准备脚本，ShellExecute 错误码：{result}"),
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
        let controls = Controls {
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
                "",
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
                "C",
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
                "",
                WS_BORDER | WS_TABSTOP,
                160,
                160,
                220,
                24,
                ID_IMAGE,
            ),
            target: create_control(
                hwnd,
                "EDIT",
                "C",
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
                "BackupRestore\\Windows.wim",
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
        add_combo_item(controls.operation, "probe");
        add_combo_item(controls.operation, "backup");
        add_combo_item(controls.operation, "restore-existing");
        add_combo_item(controls.operation, "create-secondary");
        SendMessageW(controls.operation, CB_SETCURSEL, 0, 0);
        create_control(hwnd, "STATIC", "操作模式", 0, 20, 55, 130, 22, 2001);
        create_control(hwnd, "STATIC", "任务卷盘符", 0, 20, 91, 130, 22, 2002);
        create_control(hwnd, "STATIC", "Windows 源盘符", 0, 20, 127, 130, 22, 2003);
        create_control(hwnd, "STATIC", "镜像卷盘符", 0, 20, 163, 130, 22, 2004);
        create_control(hwnd, "STATIC", "还原目标盘符", 0, 20, 199, 130, 22, 2005);
        create_control(hwnd, "STATIC", "镜像相对路径", 0, 20, 235, 130, 22, 2006);
        create_control(hwnd, "STATIC", "WIM 索引", 0, 20, 271, 130, 22, 2007);
        create_control(hwnd, "STATIC", "第二系统名称", 0, 20, 307, 130, 22, 2008);
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
            "创建任务",
            WS_TABSTOP,
            280,
            350,
            120,
            28,
            ID_CREATE_TASK,
        );
        let state = Box::new(State {
            root: hwnd,
            controls,
            executable_dir,
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        return 0;
    }
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if !state_ptr.is_null() {
        let state = &*state_ptr;
        if message == WM_COMMAND {
            match w_param & 0xffff {
                ID_REFRESH => refresh_environment(state),
                ID_READ_IMAGE => read_image(state),
                ID_CREATE_TASK => create_task(state),
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
