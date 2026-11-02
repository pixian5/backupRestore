//! 恢复运行宿主引擎（Recovery Engine）。
//!
//! 承载 WinRE / PE 环境下的主恢复执行循环、状态机转移、
//! DISM / wimlib 还原驱动、引导项清理与终态收敛。

use std::collections::BTreeMap;
#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::process::Stdio;
#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
use chrono::Utc;

use backuprestore_core::{Task, TaskError, TaskStore};
#[cfg(windows)]
use backuprestore_core::{
    canonical_compression, BootMode, Operation, PayloadManifest, Stage, StatusRecord,
    read_json, verify_image_file,
};

#[cfg(windows)]
use crate::error_code::{ErrCode, errc, errc_with};
#[cfg(windows)]
use crate::process_runner::{capture_logged, run_logged, run_logged_timeout};
#[cfg(windows)]
use crate::recovery_volume::*;
#[cfg(windows)]
use crate::recovery_bcd::*;
#[cfg(windows)]
use crate::text_parsing::MountedVolumes;
#[cfg(windows)]
use crate::{
    append_log, boot_entry, pe_safety, recovery_fault, recovery_progress, resume_safety,
    secondary_registry, windows_prepare, winre_registration,
};
use crate::err;
#[cfg(windows)]
use crate::{
    index_metadata_path, legacy_metadata_path, read_index_metadata,
    split_workspace_root_rel, stage_resumable_after_interruption,
};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(not(windows))]
pub(crate) fn recover_env(_path: String) -> Result<(), TaskError> {
    Err(err(
        "recover-env is only available on Windows/WinRE; use recover --dry-run on this host",
    ))
}

#[cfg(not(windows))]
pub(crate) fn recover_env_interactive(path: String) -> Result<(), TaskError> {
    recover_env(path)
}

#[cfg(windows)]
pub(crate) fn recover_env_interactive(path: String) -> Result<(), TaskError> {
    // 策略与逃生日志先备好：入口探测若失败，必须能"落盘 + 走 auto_escape"，
    // 不能再 `?` 直接 return Err——那会让进程 exit(1)、一次性启动项（bootsequence）
    // 仍指向本恢复环境，下次重启又进 RE 又失败，无限打转（2026-10-16 审查，
    // docs/20261016-2250 卡死 7：旧代码两个 `?` 都在恢复循环之外）。
    let values = read_env_file(&path).unwrap_or_default();
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
            if let Err(error) = run_logged("wpeinit.exe", &[], &init_log) {
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
    //
    // 【2026-10-16 用户拍板：次数与处理方式都做成设置项】次数（默认 3）与
    // 「达阈值后怎么处理」由用户在 Windows 端程序里设置，随任务落进
    // `RecoveryTask.env`（`FAILURE_THRESHOLD` / `FAILURE_ACTION`）：
    //   - auto-escape（默认）：达阈值直接自动逃出，不等用户——即 v2.6.8 的行为。
    //       ①清掉本任务的一次性启动项（disarm）②把任务写成终态（现场保留）
    //       ③重启回正常系统，让机器回到一个可用的 Windows，而不是卡在恢复环境里。
    //   - pause：达阈值**不自动处理**，弹框停在恢复环境里（弹框是模态的，机器
    //       会一直等），由用户按按钮手动执行与 auto-escape 完全相同的那套处理。
    //
    // 判据与文案的**单一来源**是 `crate::failure_policy`（GUI 设置窗口 /
    // prepare 落盘 / 这里三处共用），本函数不再自己写一份阈值或文案——
    // 项目有 P3「文案分叉」前科（GUI 曾复制一份硬编码的双语硬拒文案）。
    //
    // 第 ② 步的终态标记是关键：只 disarm 不标终态的话，任务会停在 TargetErased
    // 等可续跑阶段，下一次从**任务所在 workspace** 启动程序时
    // `resume_pending_boot_task` 会再武装一次并重启回 RE
    // （`boot-resume-{stage}.attempted` 每阶段只挡一次），而 `abandon-task`
    // 在擦除后又拒绝清理 → 任务永久悬置、用户仍无自救出口。
    // （`values` / `policy` / `language` / `escape_log` 已在本函数开头备好。）
    let mut consecutive_failures: u32 = 0;
    // 第一次进入 pause 分支的时刻。`None` = 没配等待上限（历史行为，无限等）。
    let mut pause_deadline: Option<Instant> = None;
    loop {
        match recover_env(path.clone()) {
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
                //
                // 【2026-10-16 审查修订，卡死 1】配了 `pause_max_wait_secs` 时改用
                // `MessageBoxTimeoutW`：超时（返回 0）执行与「是」完全相同的清理。
                // 默认 0 = 不限制，仍用 `MessageBoxW` 无限等——**行为与旧版一致**。
                // 上限从第一次进入暂停态起算（`pause_deadline`），不是每次弹框各自计时。
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
                    // 两者执行完全相同的清理：超时在无人值守时就是"没人来按按钮"。
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
                        // auto_escape_to_windows 内部会重启；走到这里说明重启失败。
                        return Err(error);
                    }
                    // 用户选「否」= 继续重试：直接进入下一轮，不再弹「重试/取消」框
                    //（阈值已到，那个框的语义已被本框覆盖）。
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
                    // 按过 B 回退时，续跑链校验必然失败、重试也大概率再失败——
                    // 把这点写进对话框，让用户带着信息选（2026-10-23 用户拍板：
                    // 不自动终态，只把信息说清）。
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
                    // auto_escape_to_windows 内部会重启；走到这里说明重启失败，
                    // 仍要把错误如实返回，让上层记录日志。
                    return Err(error);
                }
            }
        }
    }
}

/// 恢复环境里那个「最大化、留在后台」的命令提示符。
///
/// ## 为什么要有它（2026-10-17 用户要求 + 第二轮核查逃生 C）
/// WinRE / WinPE 是 **shell-free** 的：`winpeshl.ini` 直接拉起本程序，屏幕上
/// 只有我们的窗口 —— 没有任务栏、没有开始菜单、也没有任何命令行。于是：
/// - 用户想手动查 BCD / 看盘符 / 跑 `wpeutil`，**没有任何入口**；
/// - 一旦我们的窗口起不来（建窗失败、panic），这台机器就成了黑屏。
///
/// 这里在后台开一个**最大化**的 cmd 当常驻退路；本程序的窗口保持在前台，
/// 不会被它抢走。
///
/// 刻意用 `CreateProcessW` 而不是 `Command`：`Command` 设不了窗口显示状态，
/// 而「最大化」是这个需求的一半（另一半是「在后台」）。
///
/// ## 2026-10-20 修复：「在后台」不能只靠创建标志（实机三次复现）
///
/// 旧实现用 `STARTF_USESHOWWINDOW` + `SW_SHOWMAXIMIZED` 创建。根据 Win32
/// 文档，`SW_SHOWMAXIMIZED` **会激活窗口**——cmd 一起手就抢走前台，并且压在
/// 随后才创建的进度窗之上：2026-10-19/20 三次实机（任务 2355190e /
/// 6bf2ff22 / 7924c644）恢复全程 cmd 盖住「BackupRestore 恢复进度」窗，
/// 键盘（B/L/M/K/P）全部打在控制台上。`MB_SETFOREGROUND` 只保得住模态框，
/// 普通主窗抢不过被激活的 cmd。
///
/// 现在的顺序：
/// ① 创建时用 `SW_SHOWMINNOACTIVE`（最小化且**不激活**，不抢前台）；
/// ② 轮询等 cmd 的顶层窗口真正建出来（进程起来 ≠ 窗口就绪，按 PID 匹配）；
/// ③ `ShowWindow(SW_SHOWMAXIMIZED)` 放大；
/// ④ `SetWindowPos(HWND_BOTTOM, SWP_NOACTIVATE)` 压到 z 序最底层。
/// 前台则由进度窗创建后显式 `SetForegroundWindow` 抢回（见
/// `recovery_progress::run_window`）——那里才是用户按 B 键的地方。
#[cfg(windows)]
pub(crate) fn launch_background_console() -> Result<(), TaskError> {
    // 【2026-10-29】本函数改成**幂等**：恢复环境链与 PE 链现在都会调用它
    // （用户要求「PE 里的进度窗也要把 cmd 放到后台」）。若两条链在同一个进程里
    // 先后走到，旧行为会开出第二个 cmd 窗口——用户看到的是两层黑窗，且
    // 「压到底层」的只有后开的那一个。这里一次生效、后续调用直接返回成功。
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
    // 「最大化 + 后台」这一半在 user32：ShowWindow 放大、SetWindowPos 压底；
    // 窗口就绪等待（按 PID 找 cmd 顶层窗口）见 wait_for_toplevel_window。
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
    // ⑦ 最小化且不激活（SW_SHOWMINNOACTIVE）——与 ③ 的 SW_SHOWMAXIMIZED 是
    // 两个不同的值，别合并：前者管「创建时别抢前台」，后者管「之后放大」。
    const SW_SHOWMINNOACTIVE: u16 = 7;
    const SW_SHOWMAXIMIZED: i32 = 3;
    const HWND_BOTTOM: *mut std::ffi::c_void = 1 as *mut _;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    // `/k` 让窗口留在屏幕上（不是跑完就消失）；标题只作提示，不参与判据。
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
    // 等 cmd 的顶层窗口就绪后「最大化 + 压底」。窗口没拿到也不失败：控制台
    // 只是个退路，进程已经活着（最坏情况是它以最小化待在任务栏外）。
    match wait_for_toplevel_window(console_pid, std::time::Duration::from_secs(3)) {
        Some(console) => unsafe {
            // 先放大再压底：ShowWindow(SW_SHOWMAXIMIZED) 自身会激活，压底一步
            // 把它按回 z 序最底层；SWP_NOACTIVATE 保证这一步不再激活它。
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
    // 只有真正把窗口摆好了才记「已开过」，失败路径仍允许下一次重试。
    let _ = SPAWNED.set(());
    Ok(())
}

/// 轮询等待 `pid` 进程的**可见顶层窗口**出现，超时返回 `None`。
///
/// 为什么必须等：`CreateProcessW` 返回时 cmd 的窗口往往还没建出来（conhost
/// 初始化要几十到几百毫秒），直接 `ShowWindow` 会打在一个空句柄上。窗口就绪
/// 后才有「最大化」可谈——这是「在后台最大化」需求的第二半。
#[cfg(windows)]
fn wait_for_toplevel_window(
    pid: u32,
    timeout: std::time::Duration,
) -> Option<*mut std::ffi::c_void> {
    // EnumWindows 回调不能捕获环境，用进程级静态传参。本函数只在恢复环境
    // 启动路径调用一次，静态冲突风险为零；仍然在拿到结果后立即复位。
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
            return 0; // 命中即停：继续枚举只会拿到同进程的其它零碎窗口
        }
        1
    }
    // 签名必须与 native_gui.rs 里那份同 crate 声明逐字一致（回调也要包
    // Option），否则触发 clashing_extern_declarations 警告。
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

/// 从 `RecoveryTask.env` 读界面语言：`UI_LANGUAGE=en*` 用英文，其余一律中文
/// （读不到时保持历史行为 = 中文）。
///
/// 为什么要跟着任务走：GUI 的语言选择是**会话级**的、不落盘；如果恢复环境
/// 固定中文，英文界面的用户会在恢复环境里突然看到中文提示。prepare 创建任务时
/// 把当时的界面语言写进 env，恢复环境就能与用户当时看到的一致。
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


/// 恢复环境失败后「自动逃出」：清掉本任务的一次性启动项、删除本任务载荷，
/// 然后重启回正常 Windows，避免机器卡在恢复环境里无限重启。
///
/// 为什么需要它：`bootsequence` 在恢复链失败后**仍指向恢复环境**，用户每次选
/// 「重试」都会再次进入同一个失败。历史上唯一的逃出办法是从宿主侧挂 PE 光盘、
/// 手改 BCD（`docs/20260916-0759`），对终端用户不可行。
///
/// 安全边界：只动**本任务**的 BCD 对象与载荷（按精确 GUID），绝不碰别的启动项；
/// 清理失败也仍然尝试重启（宁可回到 Windows 让用户看到问题，也不要卡在 RE）。
#[cfg(windows)]
fn auto_escape_to_windows(env_path: &str, log: &Path) {
    // 尽力而为：任何一步失败都只记日志，绝不阻断最后的重启。
    let _ = (|| -> Result<(), TaskError> {
        let values = read_env_file(env_path)?;
        let task_id = env_required(&values, "TASK_ID")?;
        let workspace_root_rel = env_required(&values, "WORKSPACE_ROOT_REL")?;
        let store_rel = split_workspace_root_rel(&workspace_root_rel, &task_id)?;
        let mut mounts = MountedVolumes::new();
        let letter = mount_env_volume(&mut mounts, &values, "WORKSPACE", 'T', log, false)?;
        let store = TaskStore::new(PathBuf::from(format!(r"{}:\{store_rel}", letter)));
        let task_dir = store.task_dir(&task_id)?;
        if let Some(entry) = boot_entry::ReBootEntry::read(&task_dir)? {
            boot_entry::disarm(&entry, log)?;
            let _ = append_log(log, "auto-escape: one-shot boot entry disarmed");
        }
        // 【终态收口，2026-10-16 用户拍板】把任务写成终态（Failed），现场全部保留。
        // 为什么必须做：见 `recover_env_interactive` 的注释——只 disarm 不标终态，
        // 任务会停在 TargetErased 等可续跑阶段，下次从**任务所在 workspace** 启动
        // 程序时又会被自动续跑一次（`boot-resume-{stage}.attempted` 每阶段只挡一次），
        // 而 `abandon-task` 在擦除后拒绝清理 → 任务永久悬置、用户没有自救出口。
        // 标成 Failed 后：①不再被自动续跑 ②可用 abandon-task 收口清理
        // ③任务目录/日志/BCD 证据全部保留，供人工诊断。
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
    // 无论清理是否成功都重启：回到 Windows 总比卡在 RE 里好。
    let _ = run_logged("wpeutil.exe", &["reboot"], log);
}

#[cfg(windows)]
pub(crate) fn recover_env(path: String) -> Result<(), TaskError> {
    use backuprestore_core::{validate_payload_files, validate_task_id};

    // === 持久早期日志 ===
    // 【2026-10-18 缺陷修复 L4】此前硬编码 `C:\…` 与 `C:\WinRE-PoC\…`：
    // WinRE 里 C: 常常正是**离线 Windows = 还原目标卷**，这些「跨重启可回读」
    // 的日志会在随后的格式化中被一起抹掉，与 v2.6.15 定下的「日志根不落在
    // PE、不落在目标卷」（`log_root::logs_dir()`）自相矛盾。现在统一走日志根。
    let persistent_logs = [crate::log_root::logs_dir().join("recovery-early.log")];
    let meta_log = |line: &str| {
        use std::io::Write;
        for p in &persistent_logs {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
            {
                let _ = writeln!(f, "{line}");
            }
        }
    };
    meta_log(&format!("=== recover-env START path='{path}' ==="));

    let values = match read_env_file(&path) {
        Ok(v) => v,
        Err(e) => {
            meta_log(&format!("read_env_file FAILED: {e}"));
            return Err(e);
        }
    };
    let task_id = env_required(&values, "TASK_ID")?;
    validate_task_id(&task_id)?;
    // 【2026-10-16 审查修订，卡死 2/3/6】把任务级的 DISM 等待上限灌进进程环境变量，
    // 让本进程内所有 DISM 调用（`pe_operation::command` 的 long 分支、
    // `online_operation::dism`、`native_gui` 的 PE 安装）都能读到。
    // 恢复环境是**无人值守**的：只能靠任务自带的值，不能指望谁记得设环境变量。
    // 值为 0（默认/未设置）时同样显式设一次，避免父进程遗留的旧值干扰。
    {
        let secs = crate::dism_timeout::from_values(&values);
        // set_var 在 2024 edition 里是 unsafe（多线程并发修改环境不安全）；
        // 此刻还在单线程入口，且必须在任何子进程派生之前完成。
        unsafe { std::env::set_var(crate::dism_timeout::ENV_VAR, secs.to_string()) };
        meta_log(&format!("DISM wait limit: {secs} sec (0 = unlimited)"));
    }
    // 【2026-10-17 修复】暂停总上限同样灌进进程环境变量。此前
    // `recovery_progress::wait_if_paused` 读的 `PAUSE_TIMEOUT_ENV_VAR` 全仓没有
    // 写入方 → 永久 0（不限制），用户在设置里配的 `failure_pause_max_wait_secs`
    // 对「暂停后无人点继续」这一路完全不生效；只有失败弹框那条路
    // （`MessageBoxTimeoutW`）用到了它。这里让两条路**同源**。
    {
        let policy = crate::failure_policy::FailurePolicy::from_values(&values);
        let secs = policy.pause_timeout_env_value();
        // 0 也要显式写：避免父进程遗留的旧值干扰（与 DISM 同一个理由）。
        unsafe { std::env::set_var(crate::failure_policy::PAUSE_TIMEOUT_ENV_VAR, secs.to_string()) };
        meta_log(&format!("pause total limit: {secs} sec (0 = unlimited)"));
    }
    let workspace_root_rel = env_required(&values, "WORKSPACE_ROOT_REL")?;
    let store_rel = split_workspace_root_rel(&workspace_root_rel, &task_id)?;
    meta_log(&format!(
        "TASK_ID={task_id} workspace_root_rel={workspace_root_rel}"
    ));
    // Until the workspace volume is mounted only an ephemeral WinRE path is
    // available.  Switch to the task directory immediately after mounting;
    // all task/recovery logs that survive WinRE are stored there.
    let mut early_log = PathBuf::from(r"X:\BackupRestore-Recovery-early.log");
    let operation_hint = values.get("OPERATION").map(|s| s.as_str());
    // WinRE 恢复进度窗口（失败静默降级，非关键）：GUI 显示阶段/DISM 进度/日志尾部。
    let progress = recovery_progress::spawn(early_log.clone(), operation_hint);
    // 每次重试拥有自己的窗口，任何提前返回都关闭旧窗口。
    struct ProgressGuard(Arc<recovery_progress::ProgressShared>);
    impl Drop for ProgressGuard {
        fn drop(&mut self) {
            recovery_progress::close_and_wait(&self.0);
        }
    }
    let _progress_guard = ProgressGuard(Arc::clone(&progress));
    // One registry for every role mounted in this WinRE session, so two roles
    // that resolve to the same volume share its letter instead of stealing it
    // from each other.
    let mut mounts = MountedVolumes::new();
    let task_letter =
        match mount_env_volume(&mut mounts, &values, "WORKSPACE", 'T', &early_log, false) {
            Ok(letter) => {
                meta_log(&format!("mount WORKSPACE OK -> {letter}:"));
                letter
            }
            Err(e) => {
                meta_log(&format!("mount WORKSPACE FAILED: {e}"));
                return Err(e);
            }
        };
    let store = TaskStore::new(PathBuf::from(format!(r"{}:\{store_rel}", task_letter)));
    let task_dir = store.task_dir(&task_id)?;
    early_log = task_dir.join("Recovery-early.log");
    // RAM 盘初始化日志也落到任务目录，便于跨重启核验设备初始化。
    if let Ok(system_root) = std::env::var("SystemRoot") {
        for (source, name) in [
            ("Temp/BackupRestore-init.log", "WinPE-init.log"),
            ("System32/wpeinit.log", "wpeinit.log"),
        ] {
            let source = PathBuf::from(&system_root).join(source);
            if source.is_file() {
                fs::copy(source, task_dir.join(name))?;
            }
        }
    }
    // 早期日志切到任务目录后，进度窗口跟随新日志并重置读取偏移。
    progress.switch_log_path(early_log.clone(), 0);
    let mut task = store.load(&task_id)?;
    meta_log(&format!(
        "task loaded status={:?} operation={:?}",
        task.status, task.operation
    ));
    if task.status == Stage::RecoveryComplete {
        // 清理中断后的重入仅清理，不依赖已删除的载荷文件，也绝不再次格式化。
        recovery_fault::configure(values.get("TEST_FAULT").map(String::as_str), &task_dir);
        complete_recovery_cleanup(&store, &mut task, &early_log)?;
        return run_logged("wpeutil.exe", &["reboot"], &early_log);
    }
    if task.status == Stage::Success {
        return run_logged("wpeutil.exe", &["reboot"], &early_log);
    }
    if task.status == Stage::Failed {
        return Err(err(&format!(
            "task is already terminal at stage {:?}; refusing to run it again",
            task.status
        )));
    }
    // 这里曾经在入口立即把注册 WinRE 覆写回干净原件（commits fcb48a1），已于
    // 2026-09-28 回退：那样会让整个会话的注册位都不是注入件，而断电续跑要求重启后
    // bootmgr 加载的注册位必须仍是注入件（含 winpeshl → 自动拉起 Recovery.exe），
    // 否则重启会落进微软原版 WinRE、任务不会自动继续。
    //
    // 注册位只有一处，却要同时扮演「载荷宿主」与「捕获纯净」两个互斥角色，且不能用
    // 第二宿主位置绕开（WinRE 强校验启动路径 == ReAgent 注册位置），因此只能按时间
    // 分片：任务存活期保持注入件，只在 DISM 捕获前那一刻翻成干净原件
    // （见 `restore_clean_winre_before_capture`），任务终结时再还原一次。
    // 完整设计与场景矩阵：docs/20260928-0745-registered-winre-policy-and-resume-design.md
    // 主日志默认在任务目录；挂载镜像卷后（见下）切换到镜像同目录 Recovery.log，
    // 方便用户在 WIM 旁直接查看。workspace_log 保留给 GUI 状态报告读取。
    let workspace_log = store.log_path(&task_id)?;
    let mut log = workspace_log.clone();
    let mut efi_root = None;
    // WinRE cleanup is part of the recovery contract.  Defer the terminal
    // success write until the original registered WinRE image has been
    // restored and verified below.
    let result = (|| -> Result<(), TaskError> {
        let persisted_status: StatusRecord = read_json(store.status_path(&task_id)?)?;
        if !persisted_status.task_id.eq_ignore_ascii_case(&task.task_id) {
            return Err(err("status.json task_id does not match task.json"));
        }
        if persisted_status.operation != task.operation {
            return Err(err("status.json operation does not match task.json"));
        }
        // The normal-host preparation records boot-requested in status.json
        // without rewriting the payload task.json. Accept exactly that one-step
        // ahead state; any other divergence indicates a torn or tampered task.
        if persisted_status.stage != task.status
            && !(task.status == Stage::Prepared && persisted_status.stage == Stage::BootRequested)
        {
            return Err(err("status.json stage does not match task.json"));
        }
        verify_task_identity_env(&values, &task)?;
        let manifest: PayloadManifest = read_json(task_dir.join("manifest.json"))?;
        if !manifest.task_id.eq_ignore_ascii_case(&task.task_id) {
            return Err(err("payload manifest task_id does not match task.json"));
        }
        let recovery_exe = task_dir.join("payload").join("Recovery.exe");
        let task_json = task_dir.join("payload").join("task.json");
        let recovery_task_env = task_dir.join("payload").join("RecoveryTask.env");
        let original = task_dir.join("original").join("Winre.wim");
        let staged = task_dir.join("stage").join("Winre.wim");
        if !recovery_exe.is_file() {
            return Err(err("Recovery.exe is required for every WinRE operation"));
        }
        verify_running_recovery_binary(&manifest)?;
        validate_payload_files(
            &manifest,
            &recovery_exe,
            &task_json,
            &recovery_task_env,
            &original,
            &staged,
        )?;
        // The payload task is the immutable copy staged before the one-time
        // boot request. Compare every field except the durable stage: after a
        // real interruption the workspace task is legitimately at
        // TargetErased/ImageApplied/BootRepaired while the immutable payload
        // remains Prepared.
        let payload_task: Task = read_json(&task_json)?;
        payload_task.validate()?;
        let mut comparable_payload = payload_task;
        let mut comparable_workspace = task.clone();
        // Drive letters are intentionally ephemeral.  WinRE remounts the
        // same GUID-identified volumes using its own letters (for example
        // P:/Q: become G:/H:).  Comparing those letters made every
        // interrupted task fail before it could resume, despite all stable
        // partition identities being identical.
        clear_task_drive_letters(&mut comparable_payload);
        clear_task_drive_letters(&mut comparable_workspace);
        comparable_payload.status = comparable_workspace.status;
        if comparable_payload != comparable_workspace {
            return Err(err(
                "payload task does not match the workspace task prepared for this recovery",
            ));
        }

        recovery_fault::configure(values.get("TEST_FAULT").map(String::as_str), &task_dir);
        // 先完成任务和不可变载荷校验，再判断哪些角色确实受本任务格式化影响。
        let recovery_letter = mount_env_volume(
            &mut mounts,
            &values,
            "RECOVERY",
            'R',
            &early_log,
            resume_safety::role_was_erased(&task, &values, "RECOVERY"),
        )?;
        meta_log(&format!("mount RECOVERY OK (r={recovery_letter})"));
        let efi_letter = if task.operation != Operation::Probe {
            let reformatted_ok = resume_safety::role_was_erased(&task, &values, "SOURCE");
            let source_letter = mount_env_volume(
                &mut mounts,
                &values,
                "SOURCE",
                'S',
                &early_log,
                reformatted_ok,
            )?;
            let image_letter =
                mount_env_volume(&mut mounts, &values, "IMAGE", 'I', &early_log, false)?;
            if let Some(source) = task.source.as_mut() {
                source.drive_letter = Some(source_letter);
            }
            if let Some(image) = task.image.as_mut() {
                image.volume.drive_letter = Some(image_letter);
            }
            // 日志放到镜像同目录（如 E:\Recovery.log），与 WIM 并排便于查看；
            // 写失败则回退任务目录日志，不影响恢复流程。
            // 备份任务用 destination 记录镜像，还原任务用 image。
            let image_relative = match &task.destination {
                Some(spec) => Some(spec.relative_path.clone()),
                None => task.image.as_ref().map(|spec| spec.relative_path.clone()),
            };
            if let Some(relative) = image_relative {
                let parent = Path::new(&relative.replace('\\', "/"))
                    .parent()
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_default();
                let candidate = if parent.is_empty() || parent == "/" {
                    PathBuf::from(format!("{image_letter}:\\Recovery.log"))
                } else {
                    PathBuf::from(format!(
                        "{image_letter}:\\{}\\Recovery.log",
                        parent.trim_matches('/')
                    ))
                };
                // 候选日志若是新文件，把任务目录已有的早期日志一并复制过去，
                // 保证镜像旁的 Recovery.log 内容完整。
                if !candidate.exists() && workspace_log.exists() {
                    let _ = fs::copy(&workspace_log, &candidate);
                }
                let candidate_offset = fs::metadata(&candidate).map(|m| m.len()).unwrap_or(0);
                match append_log(&candidate, "Recovery log continued in image directory") {
                    Ok(_) => {
                        log = candidate;
                        // 主日志切到镜像同目录（如 E:\Recovery.log）后，进度窗口跟随追加点。
                        progress.switch_log_path(log.clone(), candidate_offset);
                    }
                    Err(error) => {
                        let _ = append_log(
                            &log,
                            &format!(
                                "image-directory log unavailable ({error}); keeping workspace log"
                            ),
                        );
                    }
                }
            }
            if let Some(destination) = task.destination.as_mut() {
                destination.volume.drive_letter = Some(image_letter);
            }
            if task.target.is_some() {
                let target_letter = mount_env_volume(
                    &mut mounts,
                    &values,
                    "TARGET",
                    'W',
                    &early_log,
                    matches!(
                        task.status,
                        Stage::TargetErased | Stage::ImageApplied | Stage::BootRepaired
                    ),
                )?;
                // WinRE may reuse E: for an image/data volume; keep EFI on a
                // late temporary letter to avoid mount collisions.
                let efi_letter =
                    mount_env_volume(&mut mounts, &values, "EFI", 'Z', &early_log, false)?;
                if let Some(target) = task.target.as_mut() {
                    target.volume.drive_letter = Some(target_letter);
                }
                Some(efi_letter)
            } else {
                None
            }
        } else if let Some(source) = task.source.as_mut() {
            // Probe tasks are allowed to keep their task files on the source
            // partition. Reuse the verified workspace mount when both names
            // identify the same partition.
            let workspace_volume = task
                .workspace_volume
                .as_ref()
                .ok_or_else(|| err("probe task is missing workspace volume identity"))?;
            if !workspace_volume.same_partition(source) {
                source.drive_letter = Some(mount_env_volume(
                    &mut mounts,
                    &values,
                    "SOURCE",
                    'S',
                    &early_log,
                    false,
                )?);
            } else {
                source.drive_letter = Some(task_letter);
                append_log(
                    &early_log,
                    &format!("Probe source is the task partition; reusing {task_letter}:"),
                )?;
            }
            None
        } else {
            None
        };
        efi_root = efi_letter.map(|letter| PathBuf::from(format!(r"{}:\", letter)));
        append_log(
            &log,
            &format!(
                "Recovery.exe started from env task={} operation={:?}",
                task_id, task.operation
            ),
        )?;
        meta_log(&format!(
            "reaching recover_windows (operation={:?})",
            task.operation
        ));
        recover_windows(
            &store,
            &mut task,
            &log,
            efi_root.as_deref(),
            Some(&values),
            false,
            Some(&progress),
        )
    })();
    if let Err(error) = result {
        // 【2026-10-23 实机发现】用户按过 B 回退时，续跑链校验必然失败（BCD 被整份
        // 换回准备期快照，任务自建的 loader/device options 随之消失），但此前现场
        // 只剩一句 `invalid task: 续跑启动链字段不符：device`，看不出是用户自己按了 B。
        // 这里把因果补进日志与错误文案——**只改说明，不改行为**：失败仍按原路径
        // 收口（保留现场、可重试），避免掩盖真正的缺陷。
        let error = if crate::recovery_progress::bcd_rolled_back()
            && error.to_string().contains("续跑启动链")
        {
            let _ = append_log(
                &log,
                crate::i18n::msg("rollback_bcd_noted"),
            );
            match annotate_chain_failure(&error.to_string(), true) {
                Some(text) => crate::err(&text),
                None => error,
            }
        } else {
            error
        };
        // C: 可能已经被擦除；回滚旧 BCD 不会还原系统文件，并可能移除续跑对象。
        // 保留当前阶段与外部载荷，停止本次执行，故障解除后从同一阶段重试。
        let persisted = store.write_recovery_error_coded(&task, error.code(), error.to_string());
        let _ = append_log(
            &log,
            &format!(
                "Recovery retry required at {:?}: {error}; boot entry and payload retained",
                task.status
            ),
        );
        if let Err(state_error) = persisted {
            let _ = append_log(
                &log,
                &format!("Unable to persist recovery error: {state_error}"),
            );
        }
        return Err(error);
    }
    store.write_transition(&mut task, Stage::RecoveryComplete)?;
    complete_recovery_cleanup(&store, &mut task, &log)?;
    meta_log("FINAL: success, wpeutil reboot");
    run_logged("wpeutil.exe", &["reboot"], &log)
}

/// 续跑链校验失败时，若本次运行回退过 BCD，就把因果补进错误文案。
///
/// 抽成纯函数便于单测：真实的回退标记是进程级静态量（`bcd_rolled_back`），
/// 单测里造不出来，也不该为了让测试能过就把标记做成可变全局。
///
/// 只有**同时**满足「本次回退过」与「错误是续跑链校验」才改写——其它失败
/// （镜像损坏、载荷缺失等）必须保持原文，不能被这句说明糊住。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn annotate_chain_failure(error: &str, rolled_back: bool) -> Option<String> {
    if rolled_back && error.contains("续跑启动链") {
        // `TaskError` 的 Display 自带 `invalid task: ` 前缀，而调用方还要再包一层
        // `crate::err`。不剥掉就会在对话框里出现
        // 「invalid task: invalid task: 续跑启动链…」（2026-10-23 实机撞见）。
        let body = error.strip_prefix("invalid task: ").unwrap_or(error);
        Some(format!(
            "{body}（本次运行中用户按 B 回退过 BCD：BCD 已还原为任务准备期快照，\
             续跑链校验失败属预期）"
        ))
    } else {
        None
    }
}

/// 还原终态自检（2026-10-26）：把「目标卷序列号前后 + Windows 内核文件」写成一行
/// ASCII 结论。它取代人工「放哨兵文件 + 记序列号」：`serial_before` 与
/// `serial_after` 不同即证明目标卷被重写过。
///
/// ★ 自检是**记录**，失败绝不能把一次成功的还原判成失败，所以调用处一律忽略
/// 返回值；盘符或卷身份取不到时字段记 `unknown`，宁可少记也不改判定。
#[cfg(windows)]
fn append_restore_selfcheck(log: &Path, task: &backuprestore_core::Task) {
    let Some(target) = task.target.as_ref() else {
        return;
    };
    let Some(letter) = target.volume.drive_letter else {
        return;
    };
    let after = crate::windows_prepare::volume_identity(letter)
        .map(|identity| identity.volume_serial)
        .unwrap_or_else(|_| "unknown".to_string());
    let ntoskrnl = PathBuf::from(format!(r"{letter}:\Windows\System32\ntoskrnl.exe")).is_file();
    let _ = append_log(
        log,
        &crate::text_parsing::restore_selfcheck_line(
            letter,
            &target.volume.volume_serial,
            &after,
            ntoskrnl,
        ),
    );
}

/// 该阶段证明磁盘、引导、恢复注册均已完成；重复执行不会重做磁盘操作。
#[cfg(windows)]
pub(crate) fn complete_recovery_cleanup(
    store: &TaskStore,
    task: &mut Task,
    log: &Path,
) -> Result<(), TaskError> {
    if task.status != Stage::RecoveryComplete {
        return Err(errc(ErrCode::TaskNotRecovered));
    }
    append_log(log, crate::i18n::msg("step_cleanup"))?;
    let env = store
        .task_dir(&task.task_id)?
        .join("payload/RecoveryTask.env");
    if env.is_file() {
        let values = read_env_file(env)?;
        recovery_fault::configure(
            values.get("TEST_FAULT").map(String::as_str),
            &store.task_dir(&task.task_id)?,
        );
    }
    let result = (|| {
        finalize_boot_entry_after_task(&store.task_dir(&task.task_id)?, log)?;
        // 终态自检：备份任务没有 target，函数自身会直接返回，不用在这里分叉。
        append_restore_selfcheck(log, task);
        recovery_fault::checkpoint("success-before", log)?;
        store.write_transition(task, Stage::Success)?;
        recovery_fault::checkpoint("success-after", log)?;
        append_log(log, "Boot entry cleaned; task marked successful")
    })();
    if let Err(error) = &result {
        // 【2026-10-16 审查修订，日志缺口 4】写盘失败不用 `?`：那会用写盘错误
        // 替换掉 `result` 里的原始恢复错误向上传播（`result` 仍会在函数末尾
        // 返回）。两个失败各自记一行，事后能区分"清理失败"与"连状态都没记上"。
        if task.status == Stage::RecoveryComplete {
            if let Err(state_error) =
                store.write_recovery_error(task, format!("恢复已完成，待重试清理：{error}"))
            {
                let _ = append_log(
                    log,
                    &crate::i18n::msg_tpl(
                        "recovery_done_status_write",
                        &[&error.to_string(), &state_error.to_string()],
                    ),
                );
            }
        }
        if let Err(log_error) =
            append_log(log, &format!("Recovery completed; cleanup pending: {error}"))
        {
            let _ = append_log(
                log,
                &crate::i18n::msg_tpl("recovery_done_log_write", &[&error.to_string(), &log_error.to_string()]),
            );
        }
    }
    result
}

/// 显式重试既有任务：先核验外部启动链；已恢复的任务只补做清理。
#[cfg(windows)]
pub(crate) fn resume_task(root: String, id: String) -> Result<(), TaskError> {
    let store = TaskStore::new(root);
    let mut task = store.load(&id)?;
    let dir = store.task_dir(&id)?;
    let log = store.log_path(&id)?;
    if task.status == Stage::RecoveryComplete {
        return complete_recovery_cleanup(&store, &mut task, &log);
    }
    let explicitly_prepared = task.status == Stage::Prepared;
    if !explicitly_prepared && !stage_resumable_after_interruption(task.status) {
        return Err(errc(ErrCode::TaskNotResumable));
    }
    if explicitly_prepared {
        // `prepare --no-reboot` leaves a complete task/payload and BCD template
        // but intentionally does not arm it. Only an explicit user resume may
        // turn that prepared task into a one-shot recovery boot.
        store.write_transition(&mut task, Stage::BootRequested)?;
    }
    let entry = boot_entry::ReBootEntry::read(&dir)?.ok_or_else(|| errc(ErrCode::TaskMissingLaunchRecord))?;
    boot_entry::rearm(&entry, &log)?;
    append_log(
        &log,
        "Explicit resume: validated existing task and boot payload",
    )?;
    run_logged("shutdown.exe", &["/r", "/t", "0"], &log)
}

/// 终态清理：清一次性启动、删自建 BCD 对象、删镜像卷上的载荷 WIM。
/// 幂等；没有记录（旧通道任务）时直接返回，不报错。
#[cfg(windows)]
fn finalize_boot_entry_after_task(task_dir: &Path, log: &Path) -> Result<(), TaskError> {
    let Some(entry) = boot_entry::ReBootEntry::read(task_dir)? else {
        append_log(
            log,
            "Boot entry cleanup: no recorded boot entry; nothing to clean",
        )?;
        return Ok(());
    };
    boot_entry::disarm(&entry, log)
}

#[cfg(any(windows, test))]
#[allow(dead_code)]
pub(crate) fn clear_task_drive_letters(task: &mut Task) {
    if let Some(source) = task.source.as_mut() {
        source.drive_letter = None;
    }
    if let Some(workspace) = task.workspace_volume.as_mut() {
        workspace.drive_letter = None;
    }
    if let Some(image) = task.image.as_mut() {
        image.volume.drive_letter = None;
    }
    if let Some(destination) = task.destination.as_mut() {
        destination.volume.drive_letter = None;
    }
    if let Some(target) = task.target.as_mut() {
        target.volume.drive_letter = None;
    }
}

/// `winpeshl.ini` starts this GUI-subsystem Rust binary directly.  Verify the
/// actual executable loaded from the task WinRE image as well as the immutable
/// workspace payload copy before touching any disk.  Checking only the copy in
/// the workspace would not prove that WinRE launched the staged Rust payload.
#[cfg(windows)]
fn verify_running_recovery_binary(manifest: &PayloadManifest) -> Result<(), TaskError> {
    let running = env::current_exe()?;
    backuprestore_core::verify_sha256(&running, &manifest.recovery_sha256).map_err(|error| {
        err(&format!(
            "running WinRE Recovery.exe does not match the prepared payload: {error}"
        ))
    })
}


#[cfg(not(windows))]
pub(crate) fn recover_windows(
    _store: &TaskStore,
    _task: &mut Task,
    _log: &Path,
    _efi_root: Option<&Path>,
    _metadata_context: Option<&BTreeMap<String, String>>,
    _finalize_success: bool,
    _progress: Option<&()>,
) -> Result<(), TaskError> {
    Err(err(
        "real Recovery execution is only available on Windows/WinRE",
    ))
}

/// 离线还原是否需要准备「引导修复会话」（STEP 1 预应用判据）。
///
/// 必须与另外两处同源，否则判据漂移会让会话该建没建、收尾却按 Windows 走，
/// 在 STEP 4 炸「Windows 系统卷缺少引导修复方案」：
/// 1. 收尾路由：灌完镜像后探测 `Windows\System32\config\SYSTEM` 是否存在；
/// 2. `resolve_efi_root_for_recovery`：决定要不要提前挂 ESP；
/// 3. 本函数：决定要不要建会话（第二阶段「引导修复」按钮也挂在它上面）。
///
/// create-secondary 的常态是「prepare 刚格式化完的空卷」：灌镜像前探测必然
/// false，所以必须认 `role=NewWindows`。2026-10-19 实机撞见（06fc5e8 引入时
/// 即只认前者；run10 能过只因那次目标卷已装着 Windows）。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn recovery_needs_boot_menu_session(
    target_is_windows: bool,
    role: backuprestore_core::TargetRole,
) -> bool {
    target_is_windows || role == backuprestore_core::TargetRole::NewWindows
}

#[cfg(windows)]
pub(crate) fn recover_windows(
    store: &TaskStore,
    task: &mut Task,
    log: &Path,
    efi_root: Option<&Path>,
    metadata_context: Option<&BTreeMap<String, String>>,
    finalize_success: bool,
    progress: Option<&Arc<crate::recovery_progress::ProgressShared>>,
) -> Result<(), TaskError> {
    use backuprestore_core::TargetRole;
    if task.status == Stage::Prepared {
        store.write_transition(task, Stage::RecoveryStarted)?;
    }
    if task.status == Stage::BootRequested {
        store.write_transition(task, Stage::RecoveryStarted)?;
    }
    if task.status == Stage::RecoveryStarted {
        store.write_transition(task, Stage::Preflight)?;
    }
    let image_path = task
        .image
        .as_ref()
        .map(|image| resolve_volume_path(&image.volume, &image.relative_path, log))
        .transpose()?;
    if let Some(path) = &image_path {
        let image = task.image.as_ref().expect("image path implies image");
        let verify_hash = task.verify_hash.unwrap_or(false);
        if verify_hash {
            verify_image_file(path, image)?;
        } else {
            // 快速校验：仅核对文件存在性和物理大小，避免数分钟无谓读盘
            let size = fs::metadata(path)
                .map_err(|e| err(&format!("failed to read image metadata: {e}")))?
                .len();
            if image.size_bytes != 0 && size != image.size_bytes {
                return Err(err(&format!(
                    "image size mismatch: expected {}, got {size}",
                    image.size_bytes
                )));
            }
        }
        if matches!(
            task.operation,
            Operation::RestoreExisting | Operation::CreateSecondary
        ) {
            // 配套备份 metadata 存在时校验 WIM 哈希与目标分区大小；缺失时
            // 视为第三方/PE WIM（如安装 WinRE/PE 为第二系统），跳过该校验。
            if let Ok(metadata) = read_index_metadata(path, image.index) {
                if verify_hash && !metadata.image_sha256.eq_ignore_ascii_case(&image.sha256) {
                    // 用户在准备阶段显式接受过这份镜像时，旧副档不再充当判据。
                    // 镜像在用户确认之后是否又被改动，已由上面的 verify_image_file
                    // 按「接受时的真实哈希与大小」核验；这里只确认例外确实绑定当前
                    // 索引与内容，不是别的镜像或别的索引留下的通用放行。
                    let covered = task.image_acceptance.as_ref().is_some_and(|accepted| {
                        accepted.covers(image.index, &image.sha256, image.size_bytes)
                    });
                    if !covered {
                        return Err(err("image hash does not match backup metadata"));
                    }
                    append_log(
                        log,
                        &format!(
                            "image hash differs from sidecar {} but the user accepted {} at prepare time; sidecar is no longer the source-of-truth for this task",
                            metadata.image_sha256, image.sha256
                        ),
                    )?;
                }
                let target = task.target.as_ref().ok_or_else(|| err("missing target"))?;
                let required = metadata
                    .required_target_size()
                    .max(metadata.source.partition_size);
                if required == 0 || target.volume.partition_size < required {
                    return Err(err(&format!(
                        "target partition is too small: {} < {}",
                        target.volume.partition_size, required
                    )));
                }
            }
        }
        run_logged(
            "dism.exe",
            &["/Get-WimInfo", &format!("/WimFile:{}", path.display())],
            log,
        )?;
    }
    match task.operation {
        Operation::Probe => {
            append_log(log, "Probe task validated; no disk operation requested")?;
            if task.status == Stage::RecoveryStarted {
                store.write_transition(task, Stage::Preflight)?;
            }
            if task.status == Stage::Preflight && finalize_success {
                store.write_transition(task, Stage::Success)?;
            }
        }
        Operation::Backup => {
            use backuprestore_core::{BackupMetadata, PROGRAM_VERSION, write_json_atomic};
            append_log(log, crate::i18n::msg("step_mount"))?;
            if let Some(p) = progress {
                p.reset_start_time();
            }
            let source = task.source.clone().ok_or_else(|| err("missing source"))?;
            let destination = task
                .destination
                .clone()
                .ok_or_else(|| err("missing destination"))?;
            let source_path = resolve_volume_root(&source, log)?;
            // v1.7.11 新通道：载荷只在镜像卷上，注册位全程只读，所以捕获进镜像的
            // 天生就是任务前的干净 WinRE —— 方案 D 的「捕获前翻回干净件」不再需要。
            let destination_path =
                resolve_volume_path(&destination.volume, &destination.relative_path, log)?;
            // 备份索引名：任务自定义（GUI 默认「系统版本+年月日」，如
            // `Windows 11 26200.9550 20261015`）或兜底默认名。CLI 侧没有
            // image_name 时不能照搬 GUI 的算法——这里可能跑在 RE 里，读到的
            // "当前系统"是被还原的环境而不是被备份的源卷。
            let image_name = task
                .image_name
                .clone()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Windows Backup".to_string());
            let partial = PathBuf::from(format!("{}.partial", destination_path.display()));
            if let Some(parent) = partial.parent() {
                fs::create_dir_all(parent)?;
            }
            match task.status {
                Stage::Preflight => {
                    store.write_transition(task, Stage::Capturing)?;
                }
                Stage::Capturing => {
                    append_log(
                        log,
                        "Resuming backup from capturing stage; replacing partial image",
                    )?;
                }
                stage => return Err(err(&format!("backup cannot resume from stage {stage:?}"))),
            }
            // 备份耗时统计：记录开始时间，捕获完成后计算总耗时与平均速度。
            let backup_started = Utc::now();
            append_log(
                log,
                &format!("Backup capture started at {}", backup_started.to_rfc3339()),
            )?;
            let existing = destination_path.exists();
            if existing && !destination_path.is_file() {
                return Err(err("backup image path exists but is not a regular file"));
            }
            let previous_hash = existing
                .then(|| backuprestore_core::sha256_file(&destination_path))
                .transpose()?;
            let previous_indexes = if existing {
                wim_indexes(&destination_path, log)?
            } else {
                Vec::new()
            };
            let mut previous_metadata = backuprestore_core::image_metadata::load_sidecars(
                &destination_path,
                &previous_indexes,
            )?;
            let legacy_path = legacy_metadata_path(&destination_path)?;
            let legacy_metadata: Option<BackupMetadata> = legacy_path
                .is_file()
                .then(|| read_json(&legacy_path))
                .transpose()
                .ok()
                .flatten();

            if let Some(value) = legacy_metadata.as_ref()
                && previous_indexes.contains(&value.wim_index)
                && previous_hash
                    .as_deref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(&value.image_sha256))
            {
                previous_metadata
                    .entry(value.wim_index)
                    .or_insert_with(|| value.clone());
            }

            // 生成 DISM 排除配置（Parallels 卷根占位符/回收站/临时目录/更新缓存/
            // 各用户浏览器缓存），WinRE 的 TEMP 位于 X: RAM 盘，配置文件不会落在
            // 捕获卷内。排除项会显著缩小 WIM 体积，且全部为 DISM 规范内的根路径写法。
            // 与 GUI 在线备份、PE 备份共用 core 的实现，避免某条路径漏接 /ConfigFile。
            // self_exe_dir 用 store.root（= Windows 侧运行目录，RE 内由
            // `resume <task-root>` 参数带进来）而不是 current_exe()：RE 内
            // current_exe 是恢复环境里的暂存路径，与捕获卷不同卷，动态排除项
            // 会永远不生成（2026-10-24 实测：五个部署件全被打进 T: 镜像）。
            // 同卷时排除自身部署件；tasks\logs\ 证据仍全留。
            let exclude_config_path = backuprestore_core::write_capture_exclusion_config(
                &source_path,
                Some(store.root.as_path()),
            )?;
            let exclude_arg = format!("/ConfigFile:{}", exclude_config_path.display());

            // A power loss can leave a partial WIM behind.  For the first
            // capture, only that partial file is removed.  For an existing
            // WIM, copy it to a same-volume candidate and append there; the
            // original remains recoverable until the candidate has passed
            // DISM inspection and is renamed into place.
            let _ = fs::remove_file(&partial);
            append_log(log, crate::i18n::msg("step_capture"))?;
            let new_index = if existing {
                let candidate = PathBuf::from(format!(
                    "{}.{}.append-candidate.wim",
                    destination_path.display(),
                    task.task_id
                ));
                let _ = fs::remove_file(&candidate);
                fs::copy(&destination_path, &candidate)?;
                let append_args = [
                    "/English",
                    "/Append-Image",
                    &format!("/ImageFile:{}", candidate.display()),
                    &format!("/CaptureDir:{}", source_path.display()),
                    &format!("/Name:{image_name}"),
                    "/CheckIntegrity",
                    &exclude_arg,
                ];
                // 捕获心跳：恢复环境里这条链同样要「先全卷预扫再写」，期间镜像恒为
                // WIM 头、DISM 日志静默，进度窗口的百分比会长时间不动。心跳行让
                // 「还在扫盘」与「真的卡死」在日志里可区分（只观测，不干预）。
                let heartbeat =
                    crate::capture_heartbeat::Heartbeat::start(candidate.clone(), log.to_path_buf());
                let append_result = run_logged("dism.exe", &append_args, log);
                drop(heartbeat);
                if let Err(error) = append_result {
                    let _ = fs::remove_file(&candidate);
                    return Err(error);
                }
                let candidate_indexes = wim_indexes(&candidate, log)?;
                if candidate_indexes.len() != previous_indexes.len().saturating_add(1) {
                    let _ = fs::remove_file(&candidate);
                    return Err(err("WIM append did not create exactly one new index"));
                }
                let index = *candidate_indexes
                    .last()
                    .ok_or_else(|| err("WIM append returned no indexes"))?;
                fs::rename(&candidate, &destination_path)?;
                append_log(log, &format!("Appended backup as WIM index {index}"))?;
                index
            } else {
                // 压缩率：仅首次创建 WIM 时生效；追加备份沿用 WIM 已有压缩设置。
                let compress_level =
                    canonical_compression(task.compress.as_deref().unwrap_or("fast"))?;
                let heartbeat = crate::capture_heartbeat::Heartbeat::start(
                    partial.clone(),
                    log.to_path_buf(),
                );
                let capture_result = run_logged(
                    "dism.exe",
                    &[
                        "/English",
                        "/Capture-Image",
                        &format!("/ImageFile:{}", partial.display()),
                        &format!("/CaptureDir:{}", source_path.display()),
                        &format!("/Name:{image_name}"),
                        &format!("/Compress:{compress_level}"),
                        "/CheckIntegrity",
                        &exclude_arg,
                    ],
                    log,
                );
                drop(heartbeat);
                capture_result?;
                let indexes = wim_indexes(&partial, log)?;
                if indexes != [1] {
                    return Err(err("first backup capture did not produce WIM index 1"));
                }
                fs::rename(&partial, &destination_path)?;
                1
            };
            let image_size = fs::metadata(&destination_path)?.len();
            // 备份耗时统计：总耗时与平均速度（字节/秒）。
            let backup_finished = Utc::now();
            let duration_secs = (backup_finished - backup_started).num_seconds().max(1) as u64;
            let bytes_per_sec = image_size / duration_secs;
            append_log(
                log,
                &format!(
                    "Backup capture finished: started={} finished={} duration={}s speed={}/s image={} bytes",
                    backup_started.to_rfc3339(),
                    backup_finished.to_rfc3339(),
                    duration_secs,
                    bytes_per_sec,
                    image_size,
                ),
            )?;
            append_log(log, crate::i18n::msg("step_verify"))?;
            let image_sha256 = backuprestore_core::sha256_file(&destination_path)?;
            let source_volume_serial = source.volume_serial.clone();
            let source_partition_size = source.partition_size;
            let context_value = |key: &str, fallback: &str| {
                metadata_context
                    .and_then(|values| env_optional(values, key))
                    .unwrap_or_else(|| fallback.to_string())
            };
            let context_u64 = |key: &str, fallback: u64| {
                metadata_context
                    .and_then(|values| env_optional_u64(values, key))
                    .unwrap_or(fallback)
            };
            let metadata = BackupMetadata {
                version: 3,
                image_type: "wim".into(),
                created: Utc::now(),
                started: Some(backup_started),
                duration_secs: Some(duration_secs),
                bytes_per_sec: Some(bytes_per_sec),
                computer: context_value("COMPUTERNAME", "WinRE"),
                windows_edition: context_value("WINDOWS_EDITION", "unknown"),
                architecture: context_value("WINDOWS_ARCHITECTURE", &native_windows_architecture()),
                windows_build: context_value("WINDOWS_BUILD", "unknown"),
                wim_index: new_index,
                image_sha256: image_sha256.clone(),
                image_size,
                source,
                captured_used_bytes: context_u64("SOURCE_USED_BYTES", 0),
                reserved_bytes: context_u64("RESERVED_BYTES", 0),
                minimum_target_size: context_u64("MINIMUM_TARGET_SIZE", source_partition_size),
                volume_serial: source_volume_serial,
                program_version: PROGRAM_VERSION.into(),
                // 与 DISM /Name 的有效索引名一致，供 sidecar 快速路径呈现真名
                image_name: Some(image_name),
            };
            previous_metadata.insert(new_index, metadata.clone());
            backuprestore_core::image_metadata::write_sidecars(
                &destination_path,
                &previous_metadata,
                &image_sha256,
                image_size,
                0,
            )?;

            // Retain a single legacy sidecar only when it clearly belongs to
            // this image.  New code always reads index-specific metadata;
            // this preserves old index-1 restores without clobbering another
            // WIM stored in the same directory.
            let legacy_belongs_to_image = if !legacy_path.exists() {
                true
            } else {
                legacy_metadata.as_ref().is_some_and(|value| {
                    previous_hash
                        .as_deref()
                        .is_some_and(|hash| value.image_sha256.eq_ignore_ascii_case(hash))
                })
            };
            if legacy_belongs_to_image {
                let first_path = index_metadata_path(&destination_path, 1)?;
                if first_path.try_exists()? {
                    let legacy: BackupMetadata = read_json(first_path)?;
                    write_json_atomic(&legacy_path, &legacy)?;
                }
            }
            append_log(
                log,
                &format!("Backup metadata written for WIM index {new_index}"),
            )?;
            // 删除后立即回读索引，再使用共用快照重编号，任何读写错误都使任务失败。
            if let Some(keep) = task.keep_indexes.filter(|n| *n > 0) {
                let keep = keep.max(1).min(new_index);
                for remaining in (keep + 1..=new_index).rev() {
                    run_logged(
                        "dism.exe",
                        &[
                            "/English",
                            "/Delete-Image",
                            &format!("/ImageFile:{}", destination_path.display()),
                            "/Index:1",
                        ],
                        log,
                    )?;
                    if wim_indexes(&destination_path, log)? != (1..remaining).collect::<Vec<_>>() {
                        return Err(err("WIM retention index read-back mismatch"));
                    }
                }
                if keep < new_index {
                    backuprestore_core::image_metadata::renumber_sidecars(
                        &destination_path,
                        new_index,
                        new_index - keep,
                    )?;
                    if legacy_belongs_to_image {
                        let path = index_metadata_path(&destination_path, 1)?;
                        if path.try_exists()? {
                            let first: BackupMetadata = read_json(path)?;
                            write_json_atomic(&legacy_path, &first)?;
                        } else if legacy_path.try_exists()? {
                            fs::remove_file(&legacy_path)?;
                        }
                    }
                    append_log(
                        log,
                        &format!(
                            "Kept latest {keep} WIM indexes; sidecars renumbered and verified"
                        ),
                    )?;
                }
            }
            if finalize_success {
                append_log(log, crate::i18n::msg("step_cleanup"))?;
                store.write_transition(task, Stage::Success)?;
            }
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let mut target = task.target.clone().ok_or_else(|| err("missing target"))?;
            // v1.7.11 新通道：格式化目标卷不再威胁"续跑启动源"（它在镜像卷上），
            // 因此 F1 执行层二次闸整体取消。目标卷自带的 Winre.wim 会被镜像里的
            // 干净原版替换，这与 Ghost 类工具语义一致（注册侧处置见设计文档 §3.3）。
            let image_index = task.image.as_ref().map(|image| image.index).unwrap_or(1);
            if target.role == TargetRole::NewWindows
                && task.boot_plan.mode != BootMode::AddSecondary
            {
                return Err(err("secondary target requires add-secondary boot plan"));
            }
            let image_path = image_path.ok_or_else(|| err("missing image"))?;
            // 目标卷同样按身份解析：PE 里记录盘符可能已被别的卷占用，而
            // `diskpart select disk/partition` 用的磁盘编号是稳定的，所以只要
            // 拿到"这个卷现在叫什么字母"就能安全格式化/应用。
            let target_letter = resolve_volume_letter(&target.volume, log)?;
            let target_root = PathBuf::from(format!("{target_letter}:\\"));
            target.volume.drive_letter = Some(target_letter);
            let live = windows_prepare::volume_identity(target_letter)?;
            let previous = (target.volume.disk_number, target.volume.partition_number);
            target.volume = pe_safety::rebind_after_boot(
                &target.volume,
                &live,
                !matches!(
                    task.status,
                    Stage::TargetErased | Stage::ImageApplied | Stage::BootRepaired
                ),
            )?;
            append_log(
                log,
                &crate::i18n::msg_tpl(
                    "restore_identity_rebound",
                    &[
                        &format!("{previous:?}"),
                        &format!(
                            "{:?}",
                            (target.volume.disk_number, target.volume.partition_number)
                        ),
                    ],
                ),
            )?;
            // 收尾按「目标性质」路由，不按「操作类型」：目标是另一套 Windows 时，
            // create-secondary 和 restore-existing 需要的收尾是同一套（保主系统
            // default + 修盘符绑定 + 按第二系统注册 WinRE）。判据来自准备阶段落盘的
            // is_active_system——RE 内 `%SystemDrive%` 是 RAM 盘 X:，探测不到用户系统卷。
            // 注：default 归属不再由此判据分支——v2.6.4 起只有一条规则
            // （一律指向本次还原出来的系统），固化在 boot_menu::decide_default。
            let target_is_secondary = !target.is_active_system;
            // 第二阶段按钮需要在目标身份确认后尽早出现；方案落盘并不修改 BCD，
            // 实际写入延后到映像应用完成后的引导修复阶段。
            // 判据必须与收尾路由（下方「灌完镜像后探测 SYSTEM hive」那扇门）和
            // `resolve_efi_root_for_recovery` 完全一致：目标**当前**是 Windows 卷，
            // 或本次要把完整系统装进一个空卷（`role=NewWindows`，create-secondary
            // 的常态——prepare 刚格式化完目标，灌镜像前探测必然 false）。
            // 只认前者会让 C 入口离线流在 STEP 4 炸「Windows 系统卷缺少引导修复
            // 方案」：会话没建，收尾却按 Windows 走（2026-10-19 实机撞见，
            // 06fc5e8 引入时即如此；run10 能过只因那次目标已装着 Windows）。
            let boot_menu_session = if recovery_needs_boot_menu_session(
                pe_safety::target_is_windows_volume(&target_root)?,
                target.role.clone(),
            ) {
                // 优先用任务里记录的第二系统菜单名（用户在准备阶段自己起的）；
                // 没有它才由 BootMenuSession 按镜像副档版本自动起名。
                let preferred_name = target.boot_menu_name.clone();
                let session = crate::boot_menu::BootMenuSession::prepare(
                    &target.volume,
                    &task.task_id,
                    &store.task_dir(&task.task_id)?,
                    log,
                    preferred_name.as_deref(),
                )?;
                // default 归属（docs/20261015-0725 S1，2026-10-15 用户修订）：
                // 无论目标是活动系统盘（重建唯一系统）还是另一个分区（新增第二
                // 系统），还原完成后 default 一律指向本次还原出来的系统——用户
                // 要的就是"还原完开机进新系统"。该规则已固化在
                // `boot_menu::decide_default` 里，apply 只执行不推断。
                if let Some(p) = progress {
                    p.set_boot_menu(Arc::clone(&session));
                    // 装载「BCD 回退」动作：点击时把 ESP 上的 BCD 恢复成任务准备期
                    // 保存的快照（bcd-before-raw / bcd-before-export）。ESP 身份在
                    // 点击时才解析（此刻还没到 STEP 3 的 find_efi_root），
                    // 用免盘符探测避免抢盘符。
                    let rollback_task_dir = store.task_dir(&task.task_id)?;
                    let rollback_log = log.to_path_buf();
                    p.set_bcd_rollback(Arc::new(move || {
                        let efi = windows_prepare::esp_identity_without_drive_letter()
                            .map_err(|error| error.to_string())?;
                        windows_prepare::rollback_bcd_snapshot(
                            &rollback_task_dir,
                            &efi,
                            &rollback_log,
                        )
                        .map_err(|error| error.to_string())
                    }));
                }
                append_log(log, crate::i18n::msg("boot_candidates_ready"))?;
                Some(session)
            } else {
                append_log(log, crate::i18n::msg("skip_boot_candidates_not_windows"))?;
                None
            };
            append_log(log, crate::i18n::msg("step_mount"))?;
            if let Some(p) = progress {
                p.reset_start_time();
            }
            // 一次性启动已经被固件消费。完整系统还原必须在擦除前重新武装，
            // 使真正断电后的下一次开机仍能从外部分区进入同一任务。
            let resume_entry = if metadata_context.is_some() {
                let entry = boot_entry::ReBootEntry::read(&store.task_dir(&task.task_id)?)?
                    .ok_or_else(|| errc(ErrCode::RestoreMissingResumeBoot))?;
                if entry.wim_volume.same_partition(&target.volume) {
                    return Err(errc(ErrCode::ResumePayloadOnEraseTarget));
                }
                if !task
                    .image
                    .as_ref()
                    .is_some_and(|image| image.volume.same_partition(&entry.wim_volume))
                {
                    return Err(errc(ErrCode::ResumePayloadVolumeMismatch));
                }
                boot_entry::rearm(&entry, log)?;
                Some(entry)
            } else {
                None
            };
            // TargetErased is deliberately persisted before formatting.  If
            // power fails after that write, formatting and applying the WIM
            // are safe to repeat on the explicitly selected target.  Older
            // task versions persisted ImageApplied before DISM completed, so
            // resume from ImageApplied also re-runs Apply-Image defensively.
            if matches!(task.status, Stage::Preflight | Stage::TargetErased) {
                let entering_target_erased = task.status == Stage::Preflight;
                if task.status == Stage::Preflight {
                    store.write_transition(task, Stage::TargetErased)?;
                }
                // 格式化选项（TargetSpec.format_target，GUI 复选框落盘）：取消勾选时
                // 覆盖式应用——跳过 diskpart 格式化与格式化后校验（不格式化就没有
                // 「序列号已变」的校验对象），直接进入镜像应用。断电续跑从
                // TargetErased 重入时同样按此选项跳过，行为一致。
                if target.format_target {
                    format_target_partition(store, task, &target.volume, log)?;
                    if !target_root.is_dir() {
                        return Err(err(
                            "restore target is unavailable after DiskPart format operation",
                        ));
                    }
                    verify_partition_identity_after_format(&target.volume, log)?;
                } else {
                    append_log(
                        log,
                        crate::i18n::msg("overlay_skip_format"),
                    )?;
                    if !target_root.is_dir() {
                        return Err(err(
                            "restore target is unavailable for overlay apply (no format requested)",
                        ));
                    }
                }
                if entering_target_erased {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-target-erased",
                        Stage::TargetErased,
                        log,
                    );
                }
            }
            if matches!(task.status, Stage::TargetErased | Stage::ImageApplied) {
                pe_safety::verify_binding(
                    &target.volume,
                    &windows_prepare::volume_identity(target_letter)?,
                    false,
                )?;
                append_log(log, crate::i18n::msg("step_apply"))?;
                let entering_image_applied = task.status == Stage::TargetErased;
                // 应用镜像前是最后一个「还没动目标」的安全暂停点：用户可在这里
                // 停下确认镜像/目标，再决定是否继续。
                if let Some(p) = progress {
                    // Err = 暂停期间窗口被关 / 暂停超时：中断恢复链并按失败处理
                    //（现场保留；auto-escape 时会 disarm + 标终态 + 重启回 Windows）。
                    p.wait_if_paused(log, "STEP 2/4 还原镜像")?;
                }
                recovery_fault::checkpoint("apply", log)?;
                // 【2026-10-22 任务 #110】「覆盖式应用（取消格式化）打到已含
                // Windows 的卷」按准备期落盘的 `overlay_no_acl` 路由执行通道：
                // DISM 会在还原 `Program Files\WindowsApps` 等 TrustedInstaller
                // 保护目录的 SD/owner 时被拒（RestoreDirMetadata 0x80070005 →
                // exit 5，PE 内对照实验钉死，见 docs/20261016-1413 §2）；
                // wimlib 自行获取 SeRestore/SeBackup 提权（POC E5：禁用提权的
                // 父进程下仍 exit 0），且该提权覆盖 SD 还原（2026-10-23 概念
                // 验证 docs/20261022-2308：tiny + 全量两级，镜像 SD 逐字节
                // 还原），RE 上下文同样可用。判据、用户确认与二进制定位
                // 均在准备期/GUI/载荷注入完成，此处只读标记决定走哪条路。
                let overlay_no_acl = task
                    .target
                    .as_ref()
                    .is_some_and(|spec| spec.overlay_no_acl);
                if overlay_no_acl {
                    // RE 内 current_exe（Recovery.exe）就在注入后的 System32，
                    // wimlib 二进制由载荷注入链放在同一目录（winre_payload）。
                    let executable_dir = std::env::current_exe()
                        .map_err(|error| {
                            errc_with(ErrCode::ProgramDirUnresolvable, &[&error.to_string()])
                        })?
                        .parent()
                        .ok_or_else(|| errc(ErrCode::ProgramDirResolveFailed))?
                        .to_path_buf();
                    crate::wimlib_apply::apply_overlay(
                        &executable_dir,
                        &image_path,
                        image_index,
                        &target_root,
                        log,
                        crate::dism_timeout::wait_ms(),
                    )?;
                } else if let Err(error) = run_logged_timeout(
                    "dism.exe",
                    &[
                        "/English",
                        "/Apply-Image",
                        &format!("/ImageFile:{}", image_path.display()),
                        &format!("/Index:{}", image_index),
                        &format!("/ApplyDir:{}", target_root.display()),
                    ],
                    log,
                    // 设置里的 DISM 等待上限（默认 u32::MAX = 不限制，零回归）。
                    // 以前这里写死无限等待，配置项形同虚设。
                    crate::dism_timeout::wait_ms(),
                ) {
                    // 【D1 2026-10-24】DISM apply 失败 → 按判据自动降级 wimlib
                    // 重试一次（设计 docs/20261024-1636；判据/分类/文案单一来源
                    // = failure_policy，编排 = wimlib_apply）。四个 apply 落点
                    // （RE 主链 / PE / 在线数据卷分支 / 在线第二系统分支）共用
                    // 同一个编排函数，避免「PE 降级了、RE 没降级」的分叉。
                    crate::wimlib_apply::fallback_after_dism_failure(
                        &error,
                        log,
                        &image_path,
                        image_index,
                        &target_root,
                    )?;
                }
                if task.status == Stage::TargetErased {
                    store.write_transition(task, Stage::ImageApplied)?;
                }
                if entering_image_applied {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-image-applied",
                        Stage::ImageApplied,
                        log,
                    );
                }
            }
            if matches!(task.status, Stage::ImageApplied | Stage::BootRepaired)
                && target_root
                    .join("Windows")
                    .join("System32\\config\\SYSTEM")
                    .is_file()
            {
                append_log(log, crate::i18n::msg("step_verify"))?;
                let efi = find_efi_root(efi_root)?;
                let entering_boot_repaired = task.status == Stage::ImageApplied;
                // 修引导前暂停点：BCD 尚未被改动，用户在此停下可安全放弃。
                if let Some(p) = progress {
                    p.wait_if_paused(log, "STEP 3/4 校验（引导修复前）")?;
                }
                // BootRepaired is recorded immediately before BCDBoot so a
                // failure is eligible for BCD rollback.  A retry from that
                // stage simply re-runs BCDBoot and validates its output.
                if task.status == Stage::ImageApplied {
                    store.write_transition(task, Stage::BootRepaired)?;
                }
                if entering_boot_repaired {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-boot-repaired",
                        Stage::BootRepaired,
                        log,
                    );
                }
                if metadata_context
                    .and_then(|values| env_optional(values, "TEST_FAULT"))
                    .as_deref()
                    == Some("bcdboot-failure")
                {
                    return Err(err("development test fault: BCDBoot failure injected"));
                }
                pe_safety::verify_binding(
                    &target.volume,
                    &windows_prepare::volume_identity(target_letter)?,
                    false,
                )?;
                let windows_root = target_root.join("Windows");
                // target_is_secondary 已在引导方案准备处按同一判据声明（default 策略共用），
                // 这里不再重复声明，避免两处判据漂移。
                // Capture the primary BCD state from the byte-for-byte task
                // snapshot before BCDBoot.  It remains available even if a
                // power failure occurs after BCDBoot changes the live store.
                let previous_boot_manager = if target_is_secondary {
                    secondary_registry::repair(&target_root, target_letter, &target.volume, log)?;
                    Some(boot_manager_state_from_task_snapshot(store, task, log)?)
                } else {
                    None
                };
                let mut bcd_args = vec![
                    windows_root.to_string_lossy().into_owned(),
                    "/s".to_string(),
                    efi.to_string_lossy().into_owned(),
                    "/f".to_string(),
                    "UEFI".to_string(),
                    "/v".to_string(),
                ];
                if target_is_secondary {
                    // 2026-10-15 用户规则修订：**不再加 /d**。此前加 /d 是为了让
                    // default 留在原主系统；现在规则反过来——还原完成后 default 就
                    // 该指向本次还原出来的系统，而 bcdboot 的默认行为正是如此。
                    // 旧注释里"失败后用户被拖进半成品系统"的顾虑实际不成立：
                    // 这里是镜像应用完成之后才跑 bcdboot，那一刻目标卷已是完整
                    // Windows，后续失败（WinRE 注册等）不会让它启动不了。
                    // /addlast 只管固件条目的添加位置，与 default 无关，保留——
                    // 菜单顺序最终仍由 boot_menu::apply 按用户方案写定。
                    bcd_args.push("/addlast".to_string());
                    // 顺带给新条目一个可读名字。最终命名仍由引导修复
                    // （boot_menu::apply，最后一个写 BCD 的步骤）按用户意图覆盖；
                    // 但那一步之前若失败，菜单里留下的就是 BCDBoot 的默认描述
                    // （实测为 "Windows PreInstallation Environment"），用户分不清
                    // 这是哪一项。先写上任务里的名字，失败态也能辨认。
                    if let Some(name) = target.boot_menu_name.as_deref() {
                        let name = name.trim();
                        if !name.is_empty() {
                            bcd_args.push("/description".to_string());
                            bcd_args.push(name.to_string());
                        }
                    }
                }
                let bcd_refs: Vec<&str> = bcd_args.iter().map(String::as_str).collect();
                if let Some(entry) = &resume_entry {
                    boot_entry::rearm(entry, log)?;
                }
                recovery_fault::checkpoint("bcdboot-before", log)?;
                run_logged("bcdboot.exe", &bcd_refs, log)?;
                recovery_fault::checkpoint("bcdboot-after", log)?;
                // 显式记录收尾路由与 default 意图（G4）：A 场景（重建唯一系统）
                // 从此留下「default 指向新 loader」的日志，不再只靠 BCD 现状反推。
                append_log(
                    log,
                    if target_is_secondary {
                        crate::i18n::msg("route_secondary")
                    } else {
                        crate::i18n::msg("route_unique")
                    },
                )?;
                // bcdboot 不带 /d 时本就会把 default 指向新 loader，但 /s 自定义
                // ESP 场景下 BFSVC 的行为不完全可控，所以立刻回读一次确认，不对
                // 就当场改。放在这个位置是因为此刻环境已知良好（bcdboot 刚成功、
                // ESP 仍挂载），比等流程失败后在未知状态下补救可靠得多。
                assert_boot_default_points_at_target(&efi, target_letter, log)?;
                if let Some(entry) = &resume_entry {
                    boot_entry::rearm(entry, log)?;
                }
                let boot_manager = efi.join("EFI\\Microsoft\\Boot\\bootmgfw.efi");
                if !boot_manager.exists() {
                    return Err(err("BCDBoot reported success but bootmgfw.efi is missing"));
                }
                verify_bcd_target(&efi, &target_root, log)?;
                // 【2026-10-28 用户裁定】注册只有两态：成功，或带码失败
                // （缺恢复环境组件 → `BR-RE-047`）。原来的「降级跳过」会让任务报成功
                // 却没有恢复环境，已删除。
                if target_is_secondary {
                    winre_registration::repair_secondary(&target_root, &efi, &target.volume, log)?;
                } else {
                    winre_registration::repair(&target_root, &efi, &target.volume, log)?;
                }
                if target_is_secondary {
                    // 菜单名只有「新增第二系统」才由用户指定；把完整 Windows 镜像还原到
                    // 另一个分区时没有这个意图，此时只定位 loader、不改它的描述。
                    // 但 preserve_primary_boot_manager 两种情况都要跑——保住主系统的
                    // 默认启动项与菜单顺序是安全行为，不依赖菜单名。
                    let menu_name = target
                        .boot_menu_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty());
                    let identifier =
                        set_secondary_boot_menu(&efi, &target_root, menu_name, log)?;
                    preserve_primary_boot_manager(
                        &efi,
                        previous_boot_manager.as_ref().expect("secondary BCD state"),
                        &identifier,
                        log,
                    )?;
                }
                // 引导修复必须是**最后**一个写 BCD 的步骤：用户在弹窗里选的名字、
                // 顺序、勾选与菜单秒数是最终意图。早先放在 bcdboot 之前时，
                // 后面的 set_secondary_boot_menu 会用 task.boot_menu_name 覆盖掉改名，
                // preserve_primary_boot_manager 又会按快照顺序重写 displayorder，
                // 把用户的排序和取消勾选一起丢掉——与弹窗里
                // "本列表就是还原完成后的最终启动菜单顺序"的承诺直接矛盾。
                // 目标系统是必选项；其它系统按勾选处理，既有 PE 入口由 apply 保留。
                let boot_menu_plan = boot_menu_session
                    .as_ref()
                    .ok_or_else(|| errc(ErrCode::WindowsVolumeMissingPlan))?
                    .freeze()?;
                crate::boot_menu::apply(&boot_menu_plan, &efi, log, resume_entry.as_ref())?;
                if finalize_success {
                    append_restore_selfcheck(log, task);
                    append_log(log, crate::i18n::msg("step_cleanup"))?;
                    store.write_transition(task, Stage::Success)?;
                }
            } else if matches!(task.status, Stage::ImageApplied | Stage::BootRepaired) {
                // 数据卷还原：目标卷无 SYSTEM hive（非 Windows 系统），跳过 BCDBoot 启动修复。
                append_log(log, crate::i18n::msg("step_verify"))?;
                append_log(
                    log,
                    "data-volume restore: target has no SYSTEM hive; skipping BCDBoot",
                )?;
                if task.status == Stage::ImageApplied {
                    store.write_transition(task, Stage::BootRepaired)?;
                }
                if finalize_success {
                    append_restore_selfcheck(log, task);
                    append_log(log, crate::i18n::msg("step_cleanup"))?;
                    store.write_transition(task, Stage::Success)?;
                }
            } else {
                return Err(err(&format!(
                    "restore cannot resume from stage {:?}",
                    task.status
                )));
            }
        }
    }
    append_log(log, "Recovery completed")?;
    Ok(())
}

/// Development-only fault injection for a power loss after a durable recovery
/// stage.  Reboot immediately without writing `failed` or cleaning WinRE, so
/// a subsequent normal-Windows GUI launch must detect and resume the task.
#[cfg(windows)]
fn interrupt_after_stage_if_requested(
    metadata_context: Option<&BTreeMap<String, String>>,
    fault: &str,
    stage: Stage,
    log: &Path,
) {
    if metadata_context
        .and_then(|values| env_optional(values, "TEST_FAULT"))
        .as_deref()
        != Some(fault)
    {
        return;
    }
    // A simulated power-loss task is deliberately booted again by the normal
    // GUI. Persist a one-shot marker before rebooting so the same injected
    // fault cannot fire forever when the resumed stage is re-entered.
    let marker = log.with_file_name(format!(".fault-{fault}.triggered"));
    if marker.is_file() {
        // ★ 被陈旧标记抑制时**必须大声说出来**（2026-10-28）。
        //   标记是「一次性」语义，挂在任务目录里防止重启后无限重放同一故障。
        //   但目录会被复用（验收夹具就是固定 task id），于是上一轮的标记会把
        //   这一轮的注入吃掉 —— 用例照样绿，其实什么都没验。
        //   静默地不注入是本项目最贵的坑之一（「报告通过但什么都没验」），
        //   所以这里留一条**可被 grep 的痕迹**：injection=suppressed_by_stale_marker。
        let _ = append_log(
            log,
            &format!(
                "TEST_FAULT={fault} injection=suppressed_by_stale_marker marker={}; \
                 an earlier run left this marker, so NO fault was injected this time",
                marker.display()
            ),
        );
        return;
    }
    if fs::write(&marker, format!("stage={stage:?}\n")).is_err() {
        let _ = append_log(
            log,
            "Development test fault marker could not be persisted; refusing injection",
        );
        return;
    }
    let _ = append_log(
        log,
        &format!(
            "Development test fault: simulating power loss after durable stage {stage:?}; rebooting without cleanup"
        ),
    );
    let _ = run_logged("wpeutil.exe", &["reboot"], log);
    std::process::exit(0);
}


#[cfg(windows)]
pub(crate) fn wim_indexes(path: &Path, log: &Path) -> Result<Vec<u32>, TaskError> {
    let output = capture_logged(
        "dism.exe",
        &[
            "/English",
            "/Get-WimInfo",
            &format!("/WimFile:{}", path.display()),
        ],
        log,
    )?;
    let mut indexes = crate::windows_prepare::parse_dism_images(&output)?
        .iter()
        .filter_map(|image| image.get("ImageIndex").and_then(serde_json::Value::as_u64))
        .filter_map(|index| u32::try_from(index).ok())
        .collect::<Vec<_>>();
    indexes.sort_unstable();
    indexes.dedup();
    if indexes.is_empty() || indexes.contains(&0) {
        return Err(err("DISM returned invalid WIM indexes"));
    }
    Ok(indexes)
}


