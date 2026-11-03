//! 进程执行、管道流转、超时控制与带日志捕获。
//!
//! 将底层命令行程序调用与日志重定向从 main.rs 抽离，
//! 统一提供带等待上限、带代码页解码和带崩溃免疫的命令执行支持。

#[cfg(windows)]
use std::fs::OpenOptions;
use std::io::{BufReader, Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use std::path::Path;
use std::process::Command;
#[cfg(windows)]
use std::process::{Child, ChildStderr, ChildStdout, ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::{Duration, Instant};

use backuprestore_core::TaskError;
use crate::error_code::{ErrCode, errc_with};
use crate::err;
#[cfg(windows)]
use crate::append_log;
#[cfg(windows)]
use crate::{CREATE_NO_WINDOW, recovery_fault};

#[cfg(windows)]
pub(crate) fn run_logged(program: &str, args: &[&str], log: &Path) -> Result<(), TaskError> {
    run_logged_timeout(program, args, log, u32::MAX)
}

/// 等子进程结束；`wait_ms == u32::MAX` 时**等到底**（旧行为，任何调用点都不受影响）。
///
/// 超时返回 `Ok(None)`：进程已被 kill 并回收，调用方随后 join 读取线程，
/// 再把「超时」如实报错——这样输出尾部照样进日志，失败仍可归因。
#[cfg(windows)]
pub(crate) fn wait_child(
    child: &mut Child,
    wait_ms: u32,
) -> Result<Option<ExitStatus>, TaskError> {
    if wait_ms == u32::MAX {
        return Ok(Some(child.wait()?));
    }
    let deadline = Instant::now() + Duration::from_millis(wait_ms as u64);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(200));
    }
}

/// 带等待上限的 `run_logged`。
///
/// 【2026-10-25 真缺陷修复】此前 apply 走的是无条件 `child.wait()`——于是
/// `--dism-timeout-secs` 在恢复环境里**只记了一行**「DISM wait limit: N sec」，
/// 实际等待仍是无限的：
///   ① 用户以为配了逃生阀，卡死时照样卡死；
///   ② 超时类失败永远走不到 `classify_apply_failure`，D1 的 wimlib 降级
///      在恢复环境主链上**永不触发**（实机：`--dism-timeout-secs 1` 跑 38GB
///      镜像，6 分钟正常完成、日志里没有降级行，才把这条挖出来）。
#[cfg(windows)]
pub(crate) fn run_logged_timeout(
    program: &str,
    args: &[&str],
    log: &Path,
    wait_ms: u32,
) -> Result<(), TaskError> {
    append_log(log, &format!("running {program} {}", args.join(" ")))?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    if let Err(error) = recovery_fault::pause_running_bcdboot(program, &mut child, log) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let log_file = OpenOptions::new().create(true).append(true).open(log)?;
    let sink = Arc::new(Mutex::new(log_file));
    let stdout: ChildStdout = child
        .stdout
        .take()
        .ok_or_else(|| err(&format!("{program} stdout was not piped")))?;
    let stderr: ChildStderr = child
        .stderr
        .take()
        .ok_or_else(|| err(&format!("{program} stderr was not piped")))?;
    let stdout_sink = Arc::clone(&sink);
    let stdout_thread = thread::spawn(move || stream_to_log("stdout", stdout, stdout_sink));
    let stderr_sink = Arc::clone(&sink);
    let stderr_thread = thread::spawn(move || stream_to_log("stderr", stderr, stderr_sink));
    let status = wait_child(&mut child, wait_ms)?;
    stdout_thread
        .join()
        .map_err(|_| err(&format!("{program} stdout reader panicked")))??;
    stderr_thread
        .join()
        .map_err(|_| err(&format!("{program} stderr reader panicked")))??;
    let Some(status) = status else {
        return Err(errc_with(
            ErrCode::ProgramTimeout,
            &[program, &wait_ms.to_string()],
        ));
    };
    if !status.success() {
        // 【D2 2026-10-24】失败回执带上输出尾部：外部命令的真实原因写在
        // stdout/stderr 里（已由上面的 reader 线程落进日志文件），只回退出码
        // 等于现场无法归因——分类判据（classify_apply_failure）也需要这段文本
        // 作为输入。tail_text 是跨平台纯文件读，截断宽容忍解码。
        let tail = crate::text_parsing::tail_text(log, crate::text_parsing::LOG_TAIL_MAX_BYTES);
        return Err(errc_with(
            ErrCode::ProgramFailedNamed,
            &[
                program,
                &format!("{status}"),
                &crate::text_parsing::LOG_TAIL_MAX_BYTES.to_string(),
                &tail,
            ],
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn capture_logged(program: &str, args: &[&str], log: &Path) -> Result<String, TaskError> {
    append_log(log, &format!("capturing {program} {}", args.join(" ")))?;
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    let mut text = crate::text_parsing::decode_windows_bytes(&output.stdout);
    text.push_str(&crate::text_parsing::decode_windows_bytes(&output.stderr));
    if !text.is_empty() {
        append_log(log, &text)?;
    }
    if !output.status.success() {
        // 【2026-10-27】这里原来用无码 err()：同一类失败（外部命令非零退出）
        // 在 run_logged 那条路径上带 BR-APL-009，在这里却不带码——--json
        // 的 errorCode 恒为 null，用户拿不到可查询的稳定标识。
        // 现在两条路径同码同结构：程序名 + 退出状态 + 输出尾部（在内存里，不重读文件）。
        return Err(errc_with(
            ErrCode::ProgramFailedNamed,
            &[
                program,
                &format!("{}", output.status),
                &crate::text_parsing::LOG_TAIL_MAX_BYTES.to_string(),
                crate::text_parsing::tail_of_text(&text, crate::text_parsing::LOG_TAIL_MAX_BYTES),
            ],
        ));
    }
    Ok(text)
}

/// 跑一个命令并把 stdout/stderr **整体落进日志**，无论退出码如何都返回原始 `Output`。
///
/// 与 `capture_logged` 的分工：那个在非零退出时用 `?` 把输出**扔掉**，只留
/// "failed with <status>"。而 DISM 的真实原因就写在 stdout/stderr 里——只丢
/// 这一句等于把唯一线索丢了。
#[cfg(windows)]
pub(crate) fn run_output_logged(
    program: &str,
    args: &[&str],
    log: &Path,
) -> Result<Output, TaskError> {
    append_log(log, &format!("running {program} {}", args.join(" ")))?;
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    let mut text = crate::text_parsing::decode_windows_bytes(&output.stdout);
    text.push_str(&crate::text_parsing::decode_windows_bytes(&output.stderr));
    if !text.is_empty() {
        append_log(log, &text)?;
    }
    Ok(output)
}

/// 取输出尾部若干非空行做**错误原因摘要**，用于拼进返回给用户/上层日志的错误消息。
#[cfg(windows)]
pub(crate) fn output_tail_summary(output: &Output, limit: usize) -> String {
    let mut text = crate::text_parsing::decode_windows_bytes(&output.stdout);
    text.push_str(&crate::text_parsing::decode_windows_bytes(&output.stderr));
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let start = lines.len().saturating_sub(limit);
    lines[start..].join(" | ")
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn stream_to_log<R: Read>(
    label: &str,
    stream: R,
    sink: Arc<Mutex<std::fs::File>>,
) -> Result<(), TaskError> {
    let mut reader = BufReader::new(stream);
    let mut line: Vec<u8> = Vec::new();
    let mut saw_cr = false;
    let emit = |line: &[u8], sink: &Arc<Mutex<std::fs::File>>| -> Result<(), TaskError> {
        let text = crate::text_parsing::decode_windows_bytes(line);
        let mut file = sink
            .lock()
            .map_err(|_| err("recovery log lock was poisoned"))?;
        writeln!(file, "[{label}] {text}")?;
        file.flush()?;
        Ok(())
    };
    loop {
        let mut byte = [0u8; 1];
        if reader.read(&mut byte)? == 0 {
            if !line.is_empty() {
                emit(&line, &sink)?;
            }
            break;
        }
        match byte[0] {
            b'\n' => {
                if saw_cr {
                    saw_cr = false;
                    line.clear();
                } else {
                    emit(&line, &sink)?;
                    line.clear();
                }
            }
            b'\r' => {
                emit(&line, &sink)?;
                line.clear();
                saw_cr = true;
            }
            other => {
                line.push(other);
                saw_cr = false;
            }
        }
    }
    Ok(())
}

pub(crate) fn run_command(program: &str, args: Vec<String>) -> Result<(), TaskError> {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(errc_with(
            ErrCode::ProgramExitNonZero,
            &[program, &format!("{status}")],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dism_carriage_return_progress_streams_each_update() {
        use std::io::Cursor;
        use std::sync::{Arc, Mutex};

        let dir = std::env::temp_dir().join("br_stream_test");
        let _ = std::fs::create_dir_all(&dir);
        let log_path = dir.join("recovery.log");
        let _ = std::fs::remove_file(&log_path);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap();
        let sink = Arc::new(Mutex::new(file));
        let data: Vec<u8> = b"[=   10.0% =]\r[=   20.0% =]\r[=  100.0% =]\nDone\n".to_vec();
        stream_to_log("stdout", Cursor::new(data), Arc::clone(&sink)).unwrap();
        drop(sink);
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(
            content.contains("10.0%"),
            "missing first percent update: {content}"
        );
        assert!(
            content.contains("20.0%"),
            "missing second percent update: {content}"
        );
        assert!(
            content.contains("100.0%"),
            "missing final percent update: {content}"
        );
        assert!(content.contains("Done"), "missing trailing line: {content}");
        assert!(
            !content.contains("[stdout] \n[stdout] "),
            "carriage-return+newline doubled a line: {content}"
        );
        let _ = std::fs::remove_file(&log_path);
    }

    #[test]
    fn dispatch_failure_carries_its_code_even_without_json() {
        let error = run_command("sh", vec!["-c".to_string(), "exit 3".to_string()])
            .expect_err("非零退出必须报错");
        assert_eq!(error.code(), Some("BR-APL-010"), "{error}");
        assert!(
            error.to_string().starts_with("[BR-APL-010]"),
            "Display 必须带码前缀：{error}"
        );
        assert!(
            !error.to_string().ends_with('\n'),
            "文案不该以换行结尾：{error:?}"
        );
    }
}
