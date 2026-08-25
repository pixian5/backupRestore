#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
use std::ptr::null_mut;

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
#[link(name = "shell32")]
unsafe extern "system" {
    fn ShellExecuteW(
        hwnd: *mut core::ffi::c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> isize;
}

#[cfg(windows)]
fn main() {
    let log_path = r"C:\BackupRestorePE\elevate-build.log";
    let _ = std::fs::write(log_path, "Launching elevated WinPE builder.\r\n");
    let operation = wide("runas");
    let file = wide(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
    let parameters = wide(
        r#"-NoProfile -ExecutionPolicy Bypass -File "C:\BackupRestorePE\build-backuprestore-pe.ps1" -Root "C:\BackupRestorePE""#,
    );
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            null_mut(),
            1,
        )
    };
    if result <= 32 {
        let _ = std::fs::write(log_path, format!("ShellExecuteW failed: {result}\r\n"));
        eprintln!("ShellExecuteW failed: {result}");
        std::process::exit(1);
    }
    let _ = std::fs::write(log_path, format!("ShellExecuteW result: {result}\r\n"));
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This helper is only for Windows VM execution.");
    std::process::exit(2);
}
