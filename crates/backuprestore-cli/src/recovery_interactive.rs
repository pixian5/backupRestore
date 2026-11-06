//! WinRE / WinPE 恢复环境交互式宿主与后台控制台逃生管理 (Recovery Interactive Host & Console Escape).
//!
//! 负责在 WinRE 裸环境下提供模态错误提示框、重试与失败暂停策略、
//! 自动逃离死循环重试回 Windows（`auto_escape_to_windows`），
//! 以及在后台拉起最大化命令提示符为运维与排障兜底。

#[cfg(windows)]
use std::collections::BTreeMap;
#[cfg(windows)]
use std::path::{Path, PathBuf};

use backuprestore_core::TaskError;
#[cfg(windows)]
use backuprestore_core::TaskStore;

#[cfg(windows)]
use crate::append_log;
#[allow(unused_imports)]
use crate::err;
#[allow(unused_imports)]
use crate::error_code::{ErrCode, errc, errc_with};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(not(windows))]
pub(crate) fn recover_env_interactive(path: String) -> Result<(), TaskError> {
    crate::recovery_engine::recover_env(path)
}

#[cfg(windows)]
pub(crate) fn recover_env_interactive(path: String) -> Result<(), TaskError> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    // 策略与逃生日志先备好：入口探测若失败，必须能"落盘 + 走 auto_escape"，
    // 不能再 `?` 直接 return Err——那会让进程 exit(1)、一次性启动项（bootsequence）
    // 仍指向本恢复环境，下次重启又进 RE 又失败，无限打转（2026-10-16 审查，
    // docs/20261016-2250 卡死 7：旧代码两个 `?` 都在恢复循环之外）。
    let values = crate::recovery_volume::read_env_file(&path).unwrap_or_default();
    let policy = crate::failure_policy::FailurePolicy::from_values(&values);
    let language = ui_language(&values);
    // 恢复环境里的语言来自 `UI_LANGUAGE`（由 `prepare --ui-language` 写进
    // `RecoveryTask.env`）。同步给全局语言层，本进程后续写到日志里的
    // 结论性文案就跟着这个语言走——此前只有弹窗文案跟、日志一律中文。
    crate::i18n::set(language);
    let escape_log = PathBuf::from(r"X:\BackupRestore-escape.log");
    let _ = append_log(&escape_log, &policy.log_line(0));
    // 自定义恢复入口没有经过系统恢复桌面；先枚举即插即用设备，
    // 否则 ARM64 恢复环境中的键盘可能尚未就绪，错误窗口无法操作。
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"X:\Windows".into());
    let init_log = PathBuf::from(&system_root).join("Temp/BackupRestore-init.log");
    // 【2026-10-16 审查修订，日志缺口 3】三个流的输出全部落盘，失败不再静默：
    // 此前 `reg query` 只按退出码决定是否跑 wpeinit，退出码本身与它自己的
    // stdout/stderr 全丢；wpeinit 的输出进 Stdio::null()。WinRE 里"键盘/设备
    // 未就绪"这类故障事后完全无从判断走了哪条分支、失败原因是什么。
    match std::process::Command::new("reg.exe")
        .args(["query", r"HKLM\SYSTEM\CurrentControlSet\Control\MiniNT"])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) if output.status.success() => {
            // 恢复环境里：wpeinit 负责枚举即插即用设备（含键盘）。它失败
            // 意味着后面的错误弹框可能根本无法操作，属确定性失败。
            if let Err(error) = crate::process_runner::run_logged("wpeinit.exe", &[], &init_log) {
                let _ = append_log(
                    &escape_log,
                    &format!(
                        "recovery entry failed: wpeinit did not run ({error}); \
                         auto-escaping instead of returning so the machine cannot boot-loop back into WinRE"
                    ),
                );
                auto_escape_to_windows(&path, &escape_log);
                return Err(error);
            }
        }
        Ok(output) => {
            let _ = append_log(
                &escape_log,
                &format!(
                    "reg query MiniNT exited with {} ({}); skipping wpeinit",
                    output.status,
                    crate::output_tail_summary(&output, 4)
                ),
            );
        }
        Err(error) => {
            let _ = append_log(
                &escape_log,
                &format!("reg query MiniNT could not be started ({error}); skipping wpeinit"),
            );
        }
    }
    // 【2026-10-17 用户要求】一进恢复环境就在后台开一个最大化的命令提示符：
    // WinRE 是 shell-free 的，没有它，用户（和我们自己排障时）没有任何命令行
    // 出口。它留在后台，本程序的弹框用 MB_SETFOREGROUND 保持在前台。
    // 失败只记日志：cmd 起不来不该让恢复链跟着失败。
    if let Err(error) = launch_background_console() {
        let _ = append_log(
            &escape_log,
            &format!("recovery console could not be started ({error}); continuing without it"),
        );
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            window: *mut std::ffi::c_void,
            text: *const u16,
            title: *const u16,
            flags: u32,
        ) -> i32;
        // 带超时的 MessageBox。`wLanguageId=0`（用线程默认语言），
        // `dwMilliseconds=0` 表示立即超时并返回 0。user32 一直导出它
        // （自 XP 起），是给无人值守场景加超时的标准手段。
        fn MessageBoxTimeoutW(
            window: *mut std::ffi::c_void,
            text: *const u16,
            title: *const u16,
            flags: u32,
            language_id: u16,
            milliseconds: u32,
        ) -> i32;
    }
    // 同一条恢复链连续失败的次数。用于「连败 N 次后如何处理」：
    // 恢复环境里的一次性启动项（bootsequence）在失败后仍指向本恢复环境，
    // 若用户每次都选「重试」而问题又是确定性的，机器会在「失败 → 重启 → 又失败」
    // 之间无限打转（2026-10-16 实测：GUI 与 RE 都没给用户任何自救手段，
    // 只能从宿主侧挂 PE 光盘手改 BCD 才逃得出来）。
    let mut consecutive_failures: u32 = 0;
    // 第一次进入 pause 分支的时刻。`None` = 没配等待上限（历史行为，无限等）。
    let mut pause_deadline: Option<Instant> = None;
    loop {
        match crate::recovery_engine::recover_env(path.clone()) {
            Ok(()) => return Ok(()),
            Err(error) => {
                consecutive_failures += 1;
                let _ = append_log(&escape_log, &policy.log_line(consecutive_failures));
                // 策略 auto-escape：连败达阈值 = 确定性失败，直接自动逃出，不询问用户。
                if policy.auto_escape_now(consecutive_failures) {
                    let _ = append_log(
                        &escape_log,
                        &format!(
                            "consecutive failures reached the auto-escape threshold ({}); \
                             auto-escape: disarm one-shot boot entry, mark task terminal, reboot to Windows",
                            policy.threshold
                        ),
                    );
                    auto_escape_to_windows(&path, &escape_log);
                    // auto_escape_to_windows 内部会重启；走到这里说明重启失败，
                    // 仍要把错误如实返回，让上层记录日志。
                    return Err(error);
                }
                // 策略 pause：达阈值**不自动处理**，弹框停在恢复环境等用户按按钮。
                // 「是」= 立即执行与 auto-escape 完全相同的处理；「否」= 继续重试。
                // 弹框是模态的：不点就一直等，这就是「暂停在恢复环境里」的语义。
                if policy.pause_now(consecutive_failures) {
                    let deadline = match pause_deadline {
                        Some(deadline) => Some(deadline),
                        None => {
                            let deadline = policy
                                .has_pause_wait_limit()
                                .then(|| {
                                    Instant::now()
                                        + Duration::from_secs(policy.pause_max_wait_secs as u64)
                                });
                            pause_deadline = deadline;
                            deadline
                        }
                    };
                    let mut text = policy.pause_prompt(
                        &error.to_string(),
                        consecutive_failures,
                        language,
                    );
                    text.push('\0');
                    let text: Vec<u16> = text.encode_utf16().collect();
                    let title: Vec<u16> = crate::failure_policy::FailurePolicy::prompt_title(language)
                        .encode_utf16()
                        .collect();
                    // MB_YESNO|MB_ICONWARNING|MB_SETFOREGROUND：
                    // 恢复环境没有桌面外壳帮错误窗口取得键盘焦点。
                    const MB_YESNO_WARN_FOREGROUND: u32 = 0x10034;
                    // 剩余毫秒；已到期 → 0（MessageBoxTimeoutW 的 0 表示立即超时返回）。
                    let remaining_ms = deadline
                        .map(|deadline| {
                            deadline
                                .saturating_duration_since(Instant::now())
                                .as_millis()
                                .min(u32::MAX as u128) as u32
                        })
                        .unwrap_or(u32::MAX);
                    let result = if deadline.is_some() {
                        let _ = append_log(
                            &escape_log,
                            &format!(
                                "pause prompt waits at most {remaining_ms} ms (limit {} sec); \
                                 timeout runs the same cleanup as Yes",
                                policy.pause_max_wait_secs
                            ),
                        );
                        unsafe {
                            MessageBoxTimeoutW(
                                std::ptr::null_mut(),
                                text.as_ptr(),
                                title.as_ptr(),
                                MB_YESNO_WARN_FOREGROUND,
                                0,
                                remaining_ms,
                            )
                        }
                    } else {
                        unsafe {
                            MessageBoxW(
                                std::ptr::null_mut(),
                                text.as_ptr(),
                                title.as_ptr(),
                                MB_YESNO_WARN_FOREGROUND,
                            )
                        }
                    };
                    // 6 = IDYES；0 = 超时（MessageBoxTimeoutW 的超时返回值）。
                    if result == 6 || result == 0 {
                        let _ = append_log(
                            &escape_log,
                            &format!(
                                "{} after {consecutive_failures} failure(s) (pause policy); \
                                 auto-escape: disarm one-shot boot entry, mark task terminal, \
                                 reboot to Windows",
                                if result == 6 {
                                    "user requested manual cleanup"
                                } else {
                                    "pause wait limit expired with no user input"
                                }
                            ),
                        );
                        auto_escape_to_windows(&path, &escape_log);
                        return Err(error);
                    }
                    let _ = append_log(
                        &escape_log,
                        &format!(
                            "user chose to keep retrying after {consecutive_failures} failure(s) \
                             (pause policy)"
                        ),
                    );
                    continue;
                }
                // 未达阈值：给用户「重试 / 放弃」的选择。
                let mut text = policy.retry_prompt(
                    &error.to_string(),
                    consecutive_failures,
                    language,
                    crate::recovery_progress::bcd_rolled_back(),
                );
                text.push('\0');
                let text: Vec<u16> = text.encode_utf16().collect();
                let title: Vec<u16> = crate::failure_policy::FailurePolicy::prompt_title(language)
                    .encode_utf16()
                    .collect();
                let result = unsafe {
                    // MB_SETFOREGROUND：恢复环境没有桌面外壳帮错误窗口取得键盘焦点。
                    MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), 0x10035)
                };
                // 选“重试”（IDRETRY=4）继续；其余（含取消、关闭窗口）走自动逃出。
                if result != 4 {
                    let _ = append_log(
                        &escape_log,
                        &format!(
                            "user declined retry after {consecutive_failures} failure(s); \
                             auto-escape: disarm one-shot boot entry, mark task terminal, reboot to Windows"
                        ),
                    );
                    auto_escape_to_windows(&path, &escape_log);
                    return Err(error);
                }
            }
        }
    }
}

/// 恢复环境里那个「最大化、留在后台」的命令提示符。
///
/// 具备 OnceLock 幂等保护，恢复环境链与 PE 链多次调用均安全。
#[cfg(not(windows))]
#[allow(dead_code)]
pub(crate) fn launch_background_console() -> Result<(), TaskError> {
    Ok(())
}

#[cfg(windows)]
pub(crate) fn launch_background_console() -> Result<(), TaskError> {
    static SPAWNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if SPAWNED.get().is_some() {
        return Ok(());
    }
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
        reserved2: u16,
        reserved2_ptr: *mut u8,
        std_input: *mut std::ffi::c_void,
        std_output: *mut std::ffi::c_void,
        std_error: *mut std::ffi::c_void,
    }
    #[repr(C)]
    struct ProcessInformation {
        process: *mut std::ffi::c_void,
        thread: *mut std::ffi::c_void,
        process_id: u32,
        thread_id: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateProcessW(
            application_name: *const u16,
            command_line: *mut u16,
            process_attributes: *mut std::ffi::c_void,
            thread_attributes: *mut std::ffi::c_void,
            inherit_handles: i32,
            creation_flags: u32,
            environment: *mut std::ffi::c_void,
            current_directory: *const u16,
            startup_info: *mut StartupInfoW,
            process_information: *mut ProcessInformation,
        ) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindow(hwnd: *mut std::ffi::c_void, command: i32) -> i32;
        fn SetWindowPos(
            hwnd: *mut std::ffi::c_void,
            insert_after: *mut std::ffi::c_void,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
    }
    const STARTF_USESHOWWINDOW: u32 = 0x0000_0001;
    const SW_SHOWMINNOACTIVE: u16 = 7;
    const SW_SHOWMAXIMIZED: i32 = 3;
    const HWND_BOTTOM: *mut std::ffi::c_void = 1 as *mut _;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    let mut command_line: Vec<u16> = r#"cmd.exe /k title BackupRestore recovery console"#
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut startup: StartupInfoW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<StartupInfoW>() as u32;
    startup.flags = STARTF_USESHOWWINDOW;
    startup.show_window = SW_SHOWMINNOACTIVE;
    let mut info: ProcessInformation = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            std::ptr::null(),
            command_line.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            CREATE_NEW_CONSOLE,
            std::ptr::null_mut(),
            std::ptr::null(),
            &mut startup,
            &mut info,
        )
    };
    if ok == 0 {
        return Err(errc(ErrCode::SpawnConsoleFailed));
    }
    let console_pid = info.process_id;
    unsafe {
        CloseHandle(info.thread);
        CloseHandle(info.process);
    }
    match wait_for_toplevel_window(console_pid, std::time::Duration::from_secs(3)) {
        Some(console) => unsafe {
            ShowWindow(console, SW_SHOWMAXIMIZED);
            SetWindowPos(
                console,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        },
        None => {
            return Err(errc(ErrCode::ConsoleWindowTimeout));
        }
    }
    let _ = SPAWNED.set(());
    Ok(())
}

#[cfg(windows)]
fn wait_for_toplevel_window(
    pid: u32,
    timeout: std::time::Duration,
) -> Option<*mut std::ffi::c_void> {
    static TARGET_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    static FOUND_HWND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    unsafe extern "system" fn enum_callback(hwnd: *mut std::ffi::c_void, _lparam: isize) -> i32 {
        let target = TARGET_PID.load(std::sync::atomic::Ordering::SeqCst);
        if target == 0 {
            return 0;
        }
        let mut window_pid: u32 = 0;
        let visible = unsafe { IsWindowVisible(hwnd) } != 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut window_pid) };
        if visible && window_pid == target {
            FOUND_HWND.store(hwnd as usize, std::sync::atomic::Ordering::SeqCst);
            return 0;
        }
        1
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(*mut std::ffi::c_void, isize) -> i32>,
            lparam: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(hwnd: *mut std::ffi::c_void, process_id: *mut u32) -> u32;
        fn IsWindowVisible(hwnd: *mut std::ffi::c_void) -> i32;
    }
    TARGET_PID.store(pid, std::sync::atomic::Ordering::SeqCst);
    FOUND_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
    let deadline = std::time::Instant::now() + timeout;
    let found = loop {
        unsafe { EnumWindows(Some(enum_callback), 0) };
        let found = FOUND_HWND.load(std::sync::atomic::Ordering::SeqCst);
        if found != 0 {
            break Some(found as *mut std::ffi::c_void);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    TARGET_PID.store(0, std::sync::atomic::Ordering::SeqCst);
    FOUND_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
    found
}

#[cfg(windows)]
fn ui_language(values: &BTreeMap<String, String>) -> crate::pe_safety::MessageLanguage {
    match values
        .get("UI_LANGUAGE")
        .map(|value| value.trim().to_ascii_lowercase())
    {
        Some(value) if value.starts_with("en") => crate::pe_safety::MessageLanguage::English,
        _ => crate::pe_safety::MessageLanguage::Chinese,
    }
}

#[cfg(windows)]
fn auto_escape_to_windows(env_path: &str, log: &Path) {
    let _ = (|| -> Result<(), TaskError> {
        let values = crate::recovery_volume::read_env_file(env_path)?;
        let task_id = crate::recovery_volume::env_required(&values, "TASK_ID")?;
        let workspace_root_rel = crate::recovery_volume::env_required(&values, "WORKSPACE_ROOT_REL")?;
        let store_rel = crate::recovery_engine::split_workspace_root_rel(&workspace_root_rel, &task_id)?;
        let mut mounts = crate::text_parsing::MountedVolumes::new();
        let letter = crate::recovery_volume::mount_env_volume(&mut mounts, &values, "WORKSPACE", 'T', log, false)?;
        let store = TaskStore::new(PathBuf::from(format!(r"{}:\{store_rel}", letter)));
        let task_dir = store.task_dir(&task_id)?;
        if let Some(entry) = crate::boot_entry::ReBootEntry::read(&task_dir)? {
            crate::boot_entry::disarm(&entry, log)?;
            let _ = append_log(log, "auto-escape: one-shot boot entry disarmed");
        }
        match store.load(&task_id) {
            Ok(mut task) if task.status.is_incomplete() => {
                match store.write_failure(
                    &mut task,
                    1,
                    "恢复在恢复环境中连续失败，已自动逃出并标记为终态（任务与现场保留，可人工诊断或清理）",
                ) {
                    Ok(_) => {
                        let _ = append_log(
                            log,
                            "auto-escape: task marked terminal (Failed); evidence retained",
                        );
                    }
                    Err(error) => {
                        let _ = append_log(
                            log,
                            &format!("auto-escape: failed to mark task terminal: {error}"),
                        );
                    }
                }
            }
            Ok(_) => {
                let _ = append_log(log, "auto-escape: task already terminal; left as-is");
            }
            Err(error) => {
                let _ = append_log(
                    log,
                    &format!("auto-escape: cannot load task to mark terminal: {error}"),
                );
            }
        }
        Ok(())
    })();
    let _ = crate::process_runner::run_logged("wpeutil.exe", &["reboot"], log);
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::*;

    #[test]
    fn ui_language_fallback() {
        #[cfg(windows)]
        {
            let mut map = std::collections::BTreeMap::new();
            assert!(matches!(ui_language(&map), crate::pe_safety::MessageLanguage::Chinese));

            map.insert("UI_LANGUAGE".into(), "en-US".into());
            assert!(matches!(ui_language(&map), crate::pe_safety::MessageLanguage::English));

            map.insert("UI_LANGUAGE".into(), "zh-CN".into());
            assert!(matches!(ui_language(&map), crate::pe_safety::MessageLanguage::Chinese));
        }
    }
}
