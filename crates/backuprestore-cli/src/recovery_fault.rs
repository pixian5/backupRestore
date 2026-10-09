//! 显式开发验收钩子。配置来自已验证的任务载荷，正常任务不会触发。
use backuprestore_core::TaskError;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

static ACTIVE: Mutex<Option<(String, PathBuf)>> = Mutex::new(None);

pub(crate) fn valid(value: &str) -> bool {
    if value == "acceptance:bcdboot-running:hold" {
        return true;
    }
    if matches!(
        value,
        "acceptance:cleanup-matrix:hold" | "acceptance:boot-matrix:hold"
    ) {
        return true;
    }
    let Some((point, mode)) = value
        .strip_prefix("acceptance:")
        .and_then(|v| v.split_once(':'))
    else {
        return false;
    };
    matches!(
        point,
        "apply"
            | "bcdboot-before"
            | "bcdboot-after"
            | "registration-register"
            | "registration-enable"
            | "registration-reconnect"
            | "cleanup-sequence"
            | "cleanup-objects"
            | "cleanup-payload"
            | "success-before"
            | "success-after"
    ) && (matches!(mode, "error" | "error-once" | "hold" | "reboot")
        || point == "apply" && mode == "disk-full")
}

/// 让真实 BCDBoot 先执行，再暂停仍存活的进程，供宿主在其内部强制断电。
/// 若命令已结束则明确判定未命中，绝不把“完成后断电”算作工具内断电。
pub(crate) fn pause_running_bcdboot(
    program: &str,
    child: &mut std::process::Child,
    log: &Path,
) -> Result<(), TaskError> {
    use std::os::windows::io::AsRawHandle;
    if !program.eq_ignore_ascii_case("bcdboot.exe") {
        return Ok(());
    }
    let active = ACTIVE.lock().expect("验收配置锁").clone();
    let Some((value, dir)) = active else {
        return Ok(());
    };
    if !matches!(
        value.as_str(),
        "acceptance:boot-matrix:hold" | "acceptance:bcdboot-running:hold"
    ) || dir.join("fault-bcdboot-running.json").try_exists()?
    {
        return Ok(());
    }
    std::thread::sleep(std::time::Duration::from_millis(10));
    if child.try_wait()?.is_some() {
        return Err(crate::err("BCDBoot 已结束，运行中断电点未命中"));
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSuspendProcess(process: *mut std::ffi::c_void) -> i32;
        fn NtResumeProcess(process: *mut std::ffi::c_void) -> i32;
    }
    let handle = child.as_raw_handle();
    if unsafe { NtSuspendProcess(handle) } != 0 {
        return Err(crate::err("无法暂停运行中的 BCDBoot"));
    }
    let result = (|| {
        crate::append_log(
            log,
            &format!("BCDBOOT_PROCESS_SUSPENDED pid={}", child.id()),
        )?;
        checkpoint("bcdboot-running", log)
    })();
    unsafe {
        NtResumeProcess(handle);
    }
    result
}

pub(crate) fn configure(value: Option<&str>, task_dir: &Path) {
    *ACTIVE.lock().expect("验收配置锁") = value
        .filter(|v| valid(v))
        .map(|v| (v.to_owned(), task_dir.to_owned()));
}

pub(crate) fn checkpoint(point: &str, log: &Path) -> Result<(), TaskError> {
    let active = ACTIVE.lock().expect("验收配置锁").clone();
    let Some((value, dir)) = active else {
        return Ok(());
    };
    let mode = if (value == "acceptance:cleanup-matrix:hold"
        && (point.starts_with("cleanup-") || point.starts_with("success-")))
        || (value == "acceptance:boot-matrix:hold"
            && (point.starts_with("bcdboot-") || point.starts_with("registration-")))
    {
        "hold"
    } else if let Some(mode) = value.strip_prefix(&format!("acceptance:{point}:")) {
        mode
    } else {
        return Ok(());
    };
    let marker = dir.join(format!("fault-{point}.json"));
    if dir.join("fault-released").try_exists()? || (mode != "error" && marker.try_exists()?) {
        return Ok(());
    }
    backuprestore_core::write_json_atomic(
        &marker,
        &serde_json::json!({"point":point,"mode":mode,"at":chrono::Utc::now()}),
    )?;
    crate::append_log(log, &format!("ACCEPTANCE_CHECKPOINT {point} {mode}"))?;
    match mode {
        "disk-full" => {
            // 只允许不超过 8 GiB 的专用测试卷，真实耗尽空间后让 DISM 自身报错。
            let task: backuprestore_core::Task =
                backuprestore_core::read_json(dir.join("task.json"))?;
            let target = task
                .target
                .ok_or_else(|| crate::err("空间故障缺少目标"))?
                .volume;
            let letter = target
                .drive_letter
                .ok_or_else(|| crate::err("空间故障缺少盘符"))?;
            if target.partition_size > 8 * 1024 * 1024 * 1024
                || std::env::var("SystemRoot")
                    .unwrap_or_default()
                    .starts_with(&format!("{letter}:"))
            {
                return Err(crate::err("空间故障只允许小型非系统测试卷"));
            }
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn GetDiskFreeSpaceExW(
                    path: *const u16,
                    free: *mut u64,
                    total: *mut u64,
                    all_free: *mut u64,
                ) -> i32;
            }
            let root: Vec<u16> = format!("{letter}:\\\0").encode_utf16().collect();
            let mut free = 0;
            if unsafe {
                GetDiskFreeSpaceExW(
                    root.as_ptr(),
                    &mut free,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            crate::run_logged(
                "fsutil.exe",
                &[
                    "file",
                    "createnew",
                    &format!("{letter}:\\br-acceptance-space.tmp"),
                    &free.saturating_sub(65536).to_string(),
                ],
                log,
            )
        }
        "error" | "error-once" => Err(crate::err(&format!("验收注入错误：{point}"))),
        "hold" => loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        },
        "reboot" => {
            crate::run_logged("wpeutil.exe", &["reboot"], log)?;
            std::process::exit(0)
        }
        _ => unreachable!(),
    }
}
