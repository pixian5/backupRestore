use super::{BootMenuSession, NAME_MAX_CHARS, TIMEOUT_MAX, render_entries};
use crate::native_gui::Msg;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::Arc;

type Hwnd = *mut c_void;
const USERDATA: i32 = -21;
const LIST: i32 = 201;
const NAME: i32 = 202;
const TIMEOUT: i32 = 203;
const HINT: i32 = 204;
const TOGGLE: i32 = 205;
const UP: i32 = 206;
const DOWN: i32 = 207;
const SAVE: i32 = 208;
const CLOSE: i32 = 209;

#[repr(C)]
struct WindowClass {
    size: u32,
    style: u32,
    procedure: Option<unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize>,
    class_extra: i32,
    window_extra: i32,
    instance: Hwnd,
    icon: Hwnd,
    cursor: Hwnd,
    brush: Hwnd,
    menu: *const u16,
    name: *const u16,
    small_icon: Hwnd,
}
#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WindowClass) -> u16;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Hwnd,
        instance: Hwnd,
        param: Hwnd,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn GetDlgItem(hwnd: Hwnd, id: i32) -> Hwnd;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
    fn SendMessageW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
    fn SetFocus(hwnd: Hwnd) -> Hwnd;
    fn GetFocus() -> Hwnd;
    fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    fn ShowWindow(hwnd: Hwnd, show: i32) -> i32;
    fn IsChild(parent: Hwnd, child: Hwnd) -> i32;
    fn GetAsyncKeyState(key: i32) -> i16;
    fn SetTimer(
        hwnd: Hwnd,
        id: usize,
        ms: u32,
        proc: Option<unsafe extern "system" fn(Hwnd, u32, usize, u32)>,
    ) -> usize;
    fn KillTimer(hwnd: Hwnd, id: usize) -> i32;
    fn EnableWindow(hwnd: Hwnd, enable: i32) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Hwnd;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    fn GetStockObject(id: i32) -> Hwnd;
}

struct State {
    session: Arc<BootMenuSession>,
    cursor: usize,
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
unsafe fn state(hwnd: Hwnd) -> Option<&'static mut State> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, USERDATA) } as *mut State;
    if pointer.is_null() {
        None
    } else {
        Some(unsafe { &mut *pointer })
    }
}
unsafe fn text(hwnd: Hwnd) -> String {
    let mut buffer = vec![0u16; 256];
    let size = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), 256) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..size])
}
unsafe fn repaint(hwnd: Hwnd, s: &mut State) {
    let (plan, editable) = s.session.snapshot();
    s.cursor = s.cursor.min(plan.entries.len().saturating_sub(1));
    unsafe {
        let list = GetDlgItem(hwnd, LIST);
        SendMessageW(list, 0x0184, 0, 0); // LB_RESETCONTENT：重排后更新列表。
        for line in render_entries(&plan, s.cursor).split("\r\n") {
            SendMessageW(list, 0x0180, 0, wide(line).as_ptr() as isize);
        }
        SendMessageW(list, 0x0186, s.cursor, 0);
        if let Some(entry) = plan.entries.get(s.cursor) {
            SetWindowTextW(GetDlgItem(hwnd, NAME), wide(&entry.display_name).as_ptr());
        }
        SetWindowTextW(
            GetDlgItem(hwnd, TIMEOUT),
            wide(&plan.timeout_seconds.to_string()).as_ptr(),
        );
        SetWindowTextW(
            GetDlgItem(hwnd, HINT),
            wide(if editable {
                "还原继续运行，选择即时保存。此处显示的顺序就是还原完成后的最终启动菜单；\
                 标有「Step 4生效」的是正在还原的系统。名称/秒数修改后按保存。无倒计时。"
            } else {
                "映像应用完成，方案已锁定并执行；关闭窗口即可。"
            })
            .as_ptr(),
        );
        for id in [LIST, NAME, TIMEOUT, TOGGLE, UP, DOWN, SAVE] {
            EnableWindow(GetDlgItem(hwnd, id), i32::from(editable));
        }
    }
}
unsafe fn action(hwnd: Hwnd, id: i32) {
    let Some(s) = (unsafe { state(hwnd) }) else {
        return;
    };
    match id {
        TOGGLE => s.session.edit(|p| {
            p.toggle(s.cursor);
        }),
        UP => {
            let mut moved = false;
            s.session.edit(|p| {
                moved = p.move_up(s.cursor);
            });
            if moved {
                s.cursor -= 1;
            }
        }
        DOWN => {
            let mut moved = false;
            s.session.edit(|p| {
                moved = p.move_down(s.cursor);
            });
            if moved {
                s.cursor += 1;
            }
        }
        SAVE => {
            let name = unsafe { text(GetDlgItem(hwnd, NAME)) };
            let seconds = unsafe { text(GetDlgItem(hwnd, TIMEOUT)) }.parse::<u32>();
            if let Ok(seconds) = seconds
                && seconds <= TIMEOUT_MAX
                && !name.trim().is_empty()
                && name.chars().count() <= NAME_MAX_CHARS
            {
                s.session.edit(|p| {
                    p.rename(s.cursor, &name);
                    p.timeout_seconds = seconds;
                });
            } else {
                unsafe {
                    SetWindowTextW(
                        GetDlgItem(hwnd, HINT),
                        wide("名称不能为空或超过60字；菜单时间为0至999秒。请修正后保存。").as_ptr(),
                    );
                }
                return;
            }
        }
        CLOSE => {
            unsafe {
                DestroyWindow(hwnd);
            };
            return;
        }
        _ => return,
    }
    unsafe {
        repaint(hwnd, s);
    }
}
unsafe extern "system" fn procedure(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize {
    unsafe {
        match msg {
            0x0111 => {
                let id = (w & 0xffff) as i32;
                if id == LIST && (w >> 16) == 1 {
                    if let Some(s) = state(hwnd) {
                        s.cursor =
                            SendMessageW(GetDlgItem(hwnd, LIST), 0x0188, 0, 0).max(0) as usize;
                        repaint(hwnd, s);
                    }
                } else if (w >> 16) == 0 {
                    action(hwnd, id);
                }
                0
            }
            0x0113 => {
                if let Some(s) = state(hwnd)
                    && !s.session.snapshot().1
                {
                    repaint(hwnd, s);
                }
                0
            }
            0x0010 => {
                DestroyWindow(hwnd);
                0
            }
            0x0082 => {
                KillTimer(hwnd, 1);
                let p = GetWindowLongPtrW(hwnd, USERDATA) as *mut State;
                SetWindowLongPtrW(hwnd, USERDATA, 0);
                if !p.is_null() {
                    drop(Box::from_raw(p));
                }
                DefWindowProcW(hwnd, msg, w, l)
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

/// 非模态子窗口与进度窗共用消息循环，恢复线程不会等待用户关窗。
pub(crate) unsafe fn open_editor(owner: Hwnd, session: Arc<BootMenuSession>) -> Hwnd {
    unsafe {
        let class = wide("BackupRestoreBootMenuClass");
        let wc = WindowClass {
            size: std::mem::size_of::<WindowClass>() as u32,
            style: 0,
            procedure: Some(procedure),
            class_extra: 0,
            window_extra: 0,
            instance: GetModuleHandleW(null()),
            icon: null_mut(),
            cursor: null_mut(),
            brush: 6usize as Hwnd,
            menu: null(),
            name: class.as_ptr(),
            small_icon: null_mut(),
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("引导修复：选择系统、排序、改名").as_ptr(),
            0x00CA0000,
            80,
            80,
            760,
            500,
            owner,
            null_mut(),
            wc.instance,
            null_mut(),
        );
        if hwnd.is_null() {
            return hwnd;
        }
        // 光标默认落在"正在还原的系统"上，而不是硬编码的第 0 项。
        // discover 把还原目标排在**最后**，若光标停在 0，用户打开窗口直接输入名字
        // 再按保存，改的是第一个既有系统（通常就是主系统）的名字。
        let cursor = session.snapshot().0.restore_target_index().unwrap_or(0);
        let boxed = Box::new(State { session, cursor });
        SetWindowLongPtrW(hwnd, USERDATA, Box::into_raw(boxed) as isize);
        let children = [
            (
                "STATIC",
                // 2026-10-15 用户裁定 A4：把「钉住」的两项各自说清。此前只提
                // 「正在还原的系统不可取消」，用户看到「当前的默认启动项」也被钉住
                // 时无从得知它还原后就不再是默认（v2.6.1 起 default 归新系统）。
                "本列表就是还原完成后的最终启动菜单顺序。正在还原的系统不可取消；「当前默认启动项」也不可取消，但还原完成后默认启动的是本次还原的新系统，原系统仍留在菜单里可选。空格勾选，Shift+↑↓排序，F2改名，Esc关闭。",
                0,
                12,
                12,
                720,
                48,
                0,
            ),
            (
                "LISTBOX",
                "",
                0x00800000 | 0x00200000 | 0x0001,
                12,
                64,
                720,
                216,
                LIST,
            ),
            ("BUTTON", "勾选/取消", 0, 12, 288, 140, 30, TOGGLE),
            ("BUTTON", "上移", 0, 165, 288, 90, 30, UP),
            ("BUTTON", "下移", 0, 265, 288, 90, 30, DOWN),
            ("STATIC", "名称", 0, 12, 334, 50, 24, 0),
            ("EDIT", "", 0x00800000 | 0x0080, 70, 330, 440, 28, NAME),
            ("STATIC", "菜单秒数", 0, 525, 334, 85, 24, 0),
            ("EDIT", "", 0x00800000 | 0x2000, 615, 330, 90, 28, TIMEOUT),
            ("BUTTON", "保存名称/时间", 0, 12, 369, 160, 30, SAVE),
            ("BUTTON", "关闭", 0, 610, 369, 100, 30, CLOSE),
            ("STATIC", "", 0, 12, 408, 720, 40, HINT),
        ];
        for (class, title, extra, x, y, width, height, id) in children {
            let child = CreateWindowExW(
                0,
                wide(class).as_ptr(),
                wide(title).as_ptr(),
                0x40000000 | 0x10000000 | 0x00010000 | extra,
                x,
                y,
                width,
                height,
                hwnd,
                id as usize as Hwnd,
                wc.instance,
                null_mut(),
            );
            SendMessageW(child, 0x0030, GetStockObject(17) as usize, 1);
        }
        if let Some(s) = state(hwnd) {
            repaint(hwnd, s);
        }
        SetTimer(hwnd, 1, 500, None);
        ShowWindow(hwnd, 5);
        SetForegroundWindow(hwnd);
        SetFocus(GetDlgItem(hwnd, LIST));
        hwnd
    }
}

/// 在 IsDialogMessage 之前处理编辑框回车、列表空格和排序，避免控件吞键。
pub(crate) unsafe fn handle_key(hwnd: Hwnd, msg: &Msg) -> bool {
    unsafe {
        if hwnd.is_null()
            || msg.message != 0x0100
            || (msg.hwnd != hwnd && IsChild(hwnd, msg.hwnd) == 0)
        {
            return false;
        }
        let key = msg.w_param;
        let focus = GetFocus();
        if key == 0x1b {
            DestroyWindow(hwnd);
            return true;
        }
        if key == 0x0d && (focus == GetDlgItem(hwnd, NAME) || focus == GetDlgItem(hwnd, TIMEOUT)) {
            action(hwnd, SAVE);
            return true;
        }
        if focus == GetDlgItem(hwnd, LIST) {
            if key == 0x20 {
                action(hwnd, TOGGLE);
                return true;
            }
            if key == 0x26 && (GetAsyncKeyState(0x10) as u16 & 0x8000) != 0 {
                action(hwnd, UP);
                return true;
            }
            if key == 0x28 && (GetAsyncKeyState(0x10) as u16 & 0x8000) != 0 {
                action(hwnd, DOWN);
                return true;
            }
            if key == 0x71 {
                SetFocus(GetDlgItem(hwnd, NAME));
                SendMessageW(GetDlgItem(hwnd, NAME), 0x00b1, 0, -1);
                return true;
            }
        }
        false
    }
}
