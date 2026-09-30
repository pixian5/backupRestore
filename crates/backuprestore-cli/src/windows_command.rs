//! 受作业对象约束的 Windows 子进程；超时必须结束整棵进程树，而不只是 cmd。
use std::ffi::c_void;
use std::fs::OpenOptions;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

type Handle = *mut c_void;
pub(crate) const INFINITE: u32 = u32::MAX;
const WAIT_TIMEOUT: u32 = 258;
// 进程树未确认退出时，禁止本实例再次发起外部写操作。
static CONTAINMENT_FAILED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CmdOutcome {
    Exited(u32),
    TimedOut { after_ms: u32, terminated: bool },
    SpawnFailed(u32),
    ExitCodeUnavailable(u32),
    WaitFailed { error: u32, terminated: bool },
    ContainmentFailed,
    Skipped,
}
impl CmdOutcome {
    pub(crate) fn is_success(self) -> bool {
        matches!(self, Self::Exited(0))
    }
}
impl std::fmt::Display for CmdOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exited(code) => write!(f, "退出码 {code}"),
            Self::TimedOut {
                after_ms,
                terminated,
            } => write!(f, "等待 {after_ms} 毫秒超时；进程树确认结束={terminated}"),
            Self::SpawnFailed(e) => write!(f, "进程未能安全启动，系统错误={e}"),
            Self::ExitCodeUnavailable(e) => write!(f, "无法读取退出码，系统错误={e}（结果未知）"),
            Self::WaitFailed { error, terminated } => {
                write!(f, "等待失败，系统错误={error}；进程树确认结束={terminated}")
            }
            Self::ContainmentFailed => write!(
                f,
                "无法确认进程树已结束，禁止重试；请检查后台进程或重启系统"
            ),
            Self::Skipped => write!(f, "前置条件不满足，未执行"),
        }
    }
}

#[repr(C)]
struct StartupInfo {
    cb: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    x_size: u32,
    y_size: u32,
    x_chars: u32,
    y_chars: u32,
    fill: u32,
    flags: u32,
    show: u16,
    reserved_len: u16,
    reserved_bytes: *mut u8,
    input: Handle,
    output: Handle,
    error: Handle,
}
#[repr(C)]
struct StartupInfoEx {
    base: StartupInfo,
    attributes: *mut c_void,
}
#[repr(C)]
struct ProcessInfo {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}
#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    process_time: i64,
    job_time: i64,
    flags: u32,
    min_working: usize,
    max_working: usize,
    active_limit: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
#[derive(Default)]
struct ExtendedLimits {
    basic: BasicLimits,
    io: [u64; 6],
    process_memory: usize,
    job_memory: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}
#[repr(C)]
#[derive(Default)]
struct Accounting {
    user_time: i64,
    kernel_time: i64,
    period_user: i64,
    period_kernel: i64,
    page_faults: u32,
    total_processes: u32,
    active_processes: u32,
    terminated_processes: u32,
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateProcessW(
        app: *const u16,
        line: *mut u16,
        pa: *mut c_void,
        ta: *mut c_void,
        inherit: i32,
        flags: u32,
        env: *mut c_void,
        cwd: *const u16,
        startup: *mut StartupInfo,
        info: *mut ProcessInfo,
    ) -> i32;
    pub(crate) fn WaitForSingleObject(handle: Handle, ms: u32) -> u32;
    pub(crate) fn GetExitCodeProcess(handle: Handle, code: *mut u32) -> i32;
    fn TerminateProcess(handle: Handle, code: u32) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetLastError() -> u32;
    fn ResumeThread(thread: Handle) -> u32;
    fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: u32, data: *const c_void, len: u32) -> i32;
    fn QueryInformationJobObject(
        job: Handle,
        class: u32,
        data: *mut c_void,
        len: u32,
        returned: *mut u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn TerminateJobObject(job: Handle, code: u32) -> i32;
    fn SetHandleInformation(handle: Handle, mask: u32, flags: u32) -> i32;
    fn InitializeProcThreadAttributeList(
        list: *mut c_void,
        count: u32,
        flags: u32,
        size: *mut usize,
    ) -> i32;
    fn UpdateProcThreadAttribute(
        list: *mut c_void,
        flags: u32,
        attribute: usize,
        value: *mut c_void,
        size: usize,
        previous: *mut c_void,
        returned: *mut usize,
    ) -> i32;
    fn DeleteProcThreadAttributeList(list: *mut c_void);
}
struct OwnedHandle(Handle);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Attributes(Vec<usize>);
impl Attributes {
    fn pointer(&mut self) -> *mut c_void {
        self.0.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.pointer());
        }
    }
}

/// Windows 参数引用规则：引号前以及末尾的反斜杠必须成倍转义。
fn quote(arg: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '"' {
            out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            out.extend(std::iter::repeat_n('\\', slashes));
        }
        slashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// 用户输入以独立参数传给 DISM，不经过命令解释器，避免名称/路径中的元字符被执行。
pub(crate) fn run_program(
    program: &str,
    args: &[String],
    out: Option<&Path>,
    ms: u32,
) -> CmdOutcome {
    let line = std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ");
    run_line(&line, out, ms)
}
pub(crate) fn run_shell(command: &str, out: Option<&Path>, ms: u32) -> CmdOutcome {
    run_line(&format!("cmd.exe /d /v:off /c {command}"), out, ms)
}

fn stopped(job: Handle) -> bool {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let mut info = Accounting::default();
        let ok = unsafe {
            QueryInformationJobObject(
                job,
                1,
                (&mut info as *mut Accounting).cast(),
                std::mem::size_of::<Accounting>() as u32,
                null_mut(),
            )
        };
        if ok == 0 {
            return false;
        }
        if info.active_processes == 0 {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn stop_tree(job: Handle) -> bool {
    // 即使 API 报错，也查询活动进程数；只有确认归零才能允许重试。
    unsafe {
        TerminateJobObject(job, 1);
    }
    let done = stopped(job);
    if !done {
        CONTAINMENT_FAILED.store(true, Ordering::SeqCst);
    }
    done
}
fn run_line(line: &str, out: Option<&Path>, ms: u32) -> CmdOutcome {
    if CONTAINMENT_FAILED.load(Ordering::SeqCst) {
        return CmdOutcome::ContainmentFailed;
    }
    if line.contains('\0') {
        return CmdOutcome::SpawnFailed(87);
    }
    let io_error =
        |e: std::io::Error| CmdOutcome::SpawnFailed(e.raw_os_error().unwrap_or(1) as u32);
    // 每个句柄只通过显式继承列表传给这个子进程，避免并发命令互相持有输出文件。
    let output = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(out.unwrap_or(Path::new("NUL")))
    {
        Ok(f) => f,
        Err(e) => return io_error(e),
    };
    let input = match OpenOptions::new().read(true).open("NUL") {
        Ok(f) => f,
        Err(e) => return io_error(e),
    };
    unsafe {
        let job = OwnedHandle(CreateJobObjectW(null_mut(), null()));
        if job.0.is_null() {
            return CmdOutcome::SpawnFailed(GetLastError());
        }
        let mut limits = ExtendedLimits::default();
        limits.basic.flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if SetInformationJobObject(
            job.0,
            9,
            (&limits as *const ExtendedLimits).cast(),
            std::mem::size_of::<ExtendedLimits>() as u32,
        ) == 0
        {
            return CmdOutcome::SpawnFailed(GetLastError());
        }
        let mut handles = [input.as_raw_handle(), output.as_raw_handle()];
        for h in handles {
            if SetHandleInformation(h, 1, 1) == 0 {
                return CmdOutcome::SpawnFailed(GetLastError());
            }
        }
        let mut size = 0;
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
        if InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut size) == 0 {
            return CmdOutcome::SpawnFailed(GetLastError());
        }
        let mut attributes = Attributes(storage);
        if UpdateProcThreadAttribute(
            attributes.pointer(),
            0,
            0x20002,
            handles.as_mut_ptr().cast(),
            std::mem::size_of_val(&handles),
            null_mut(),
            null_mut(),
        ) == 0
        {
            return CmdOutcome::SpawnFailed(GetLastError());
        }
        let mut startup: StartupInfoEx = std::mem::zeroed();
        startup.base.cb = std::mem::size_of::<StartupInfoEx>() as u32;
        startup.base.flags = 0x100; // STARTF_USESTDHANDLES
        startup.base.input = handles[0];
        startup.base.output = handles[1];
        startup.base.error = handles[1];
        startup.attributes = attributes.pointer();
        let mut info: ProcessInfo = std::mem::zeroed();
        let mut line: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();
        // 挂起创建 → 加入作业 → 恢复：不留「先生成子进程、后加入作业」的竞态窗口。
        if CreateProcessW(
            null(),
            line.as_mut_ptr(),
            null_mut(),
            null_mut(),
            1,
            0x08000000 | 4 | 0x80000,
            null_mut(),
            null(),
            &mut startup.base,
            &mut info,
        ) == 0
        {
            return CmdOutcome::SpawnFailed(GetLastError());
        }
        let process = OwnedHandle(info.process);
        let thread = OwnedHandle(info.thread);
        if AssignProcessToJobObject(job.0, process.0) == 0 {
            let error = GetLastError();
            TerminateProcess(process.0, 1); // 尚未恢复，不可能已有子进程。
            if WaitForSingleObject(process.0, 10000) != 0 {
                CONTAINMENT_FAILED.store(true, Ordering::SeqCst);
                return CmdOutcome::ContainmentFailed;
            }
            return CmdOutcome::SpawnFailed(error);
        }
        if ResumeThread(thread.0) == u32::MAX {
            let error = GetLastError();
            return if stop_tree(job.0) {
                CmdOutcome::SpawnFailed(error)
            } else {
                CmdOutcome::ContainmentFailed
            };
        }
        let wait = WaitForSingleObject(process.0, ms);
        match wait {
            0 => {
                let mut code = 0;
                let read = GetExitCodeProcess(process.0, &mut code);
                let error = GetLastError();
                // 主进程即使退出，也不能把仍在写入的后代留在后台。
                if !stop_tree(job.0) {
                    return CmdOutcome::ContainmentFailed;
                }
                if read == 0 {
                    CmdOutcome::ExitCodeUnavailable(error)
                } else {
                    CmdOutcome::Exited(code)
                }
            }
            WAIT_TIMEOUT => CmdOutcome::TimedOut {
                after_ms: ms,
                terminated: stop_tree(job.0),
            },
            other => {
                let error = if other == u32::MAX {
                    GetLastError()
                } else {
                    other
                };
                CmdOutcome::WaitFailed {
                    error,
                    terminated: stop_tree(job.0),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_zero_exit_is_success() {
        assert!(CmdOutcome::Exited(0).is_success());
        for result in [
            CmdOutcome::Exited(259),
            CmdOutcome::SpawnFailed(0),
            CmdOutcome::ExitCodeUnavailable(0),
            CmdOutcome::TimedOut {
                after_ms: 1,
                terminated: true,
            },
            CmdOutcome::WaitFailed {
                error: 0,
                terminated: true,
            },
            CmdOutcome::Skipped,
            CmdOutcome::ContainmentFailed,
        ] {
            assert!(!result.is_success());
        }
    }
    #[test]
    fn quoted_arguments_keep_trailing_slash() {
        assert_eq!(quote("C:\\"), "\"C:\\\\\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
    }
    #[test]
    fn command_exit_codes_are_real() {
        assert_eq!(run_shell("exit /b 7", None, 5000), CmdOutcome::Exited(7));
        assert_eq!(run_shell("exit /b 0", None, 5000), CmdOutcome::Exited(0));
        assert!(matches!(
            run_program("br-nonexistent-command-94837.exe", &[], None, 5000),
            CmdOutcome::SpawnFailed(_)
        ));
    }
    #[test]
    fn timeout_terminates_nested_child() {
        let output = std::env::temp_dir().join(format!("br-job-{}.txt", std::process::id()));
        let line = "cmd /c \"ping -n 4 127.0.0.1 >NUL & echo ORPHAN\"";
        assert!(matches!(
            run_shell(line, Some(&output), 200),
            CmdOutcome::TimedOut {
                terminated: true,
                ..
            }
        ));
        std::thread::sleep(Duration::from_secs(4));
        let text = std::fs::read_to_string(&output).unwrap();
        assert!(!text.contains("ORPHAN"));
        std::fs::remove_file(output).unwrap();
    }
}
