//! CLI 结构化输出（`--json`）。
//!
//! 【为什么需要】CLI 的人类可读输出是中文（本产品是中文产品），自动化判据若去
//! 匹配中文文案会有两个硬伤：
//!   ① 措辞随版本变化 → 判据脆；
//!   ② 要同时应付两种字节流——cmd 内建是 GBK、本程序 stdout 是 UTF-8，
//!      2026-10-24 实测：把日志的非 ASCII 字节剥掉后，`DISM 结果：退出码 0`
//!      会变成虚假的 `DISM  0`，据此写出的判据是错的。
//! `--json` 让命令把**结论**以结构化字段输出，判据不再解析自然语言。
//!
//! 【边界（务必知情）】
//!   · 只影响 **stdout 的最后一行**；日志文件、GUI 弹窗文案完全不变
//!     （日志无语言维度，`append_log` 不带 language，这是既有设计）；
//!   · 失败时 `error` 是给人看的诊断串（`[BR-域-序号] 文案`），**不是判据输入**；
//!     判据请判 `success` / `error_code` / `log_path` / `operation` 这些结构化字段。
//!     `error_code` 是稳定契约（无码的用法错误为 `null`），`error` 的文案随时会改。
//!
//! 【设计】
//!   · 全局开关只在此模块内可见，避免散落各处各自判断；
//!   · 输出恒为**单行** JSON，便于 `python3 -c "json.load(sys.stdin)"`；
//!   · 路径含反斜杠，交给 `serde_json` 转义，绝不手写拼接。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static JSON_MODE: AtomicBool = AtomicBool::new(false);

/// 打开结构化输出（`main` 检测到 `--json` 时调用一次）。
pub(crate) fn enable() {
    JSON_MODE.store(true, Ordering::SeqCst);
}

/// 是否处于 `--json` 模式。
pub(crate) fn enabled() -> bool {
    JSON_MODE.load(Ordering::SeqCst)
}

/// 构造命令结论的 JSON 对象。字段固定，缺失值一律 `null`（不省略键，
/// 否则调用方要为"键在不在"写两套解析）。
///
/// `task_id` 只有建 TaskStore 的命令（`prepare`）有；`online` 走独立日志目录、
/// 不建任务目录，传 `None`。
pub(crate) fn command_result(
    command: &str,
    operation: &str,
    success: bool,
    task_id: Option<&str>,
    log_path: Option<&str>,
    error: Option<&str>,
    error_code: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "command": command,
        "operation": operation,
        "success": success,
        "task_id": task_id,
        "log_path": log_path,
        "error": error,
        // 稳定的 `BR-域-序号`；用法错误（无码）为 null。
        // 判据判这个字段，**不要**去解析 `error` 里的 `[码] ` 前缀。
        "error_code": error_code,
    })
}

/// `--json` 模式下打印一行 JSON；否则什么都不做（调用方走原来的中文输出）。
pub(crate) fn emit(value: &serde_json::Value) {
    if enabled() {
        println!("{}", value);
    }
}

/// 命令**上报**自己的结构化详情（task_id / log_path），供 `main` 收尾时
/// 统一输出那一行结论。
///
/// 为什么不各自 `emit`：那会让 `prepare` / `online` 输出两行 JSON
/// （一行自己的、一行 main 的），消费方被迫猜该读哪行。改为"上报 + 单一出口"，
/// 每个命令在 json 模式下**只可能有一行**结论。
static DETAIL: Mutex<Option<Detail>> = Mutex::new(None);

pub(crate) struct Detail {
    pub task_id: Option<String>,
    pub log_path: Option<String>,
}

pub(crate) fn set_detail(task_id: Option<String>, log_path: Option<String>) {
    if let Ok(mut guard) = DETAIL.lock() {
        *guard = Some(Detail { task_id, log_path });
    }
}

/// 取走上报的详情（取走即清空，保证不被后续命令读到）。
pub(crate) fn take_detail() -> Option<Detail> {
    DETAIL.lock().ok().and_then(|mut guard| guard.take())
}

#[cfg(test)]
mod tests {
    use super::command_result;

    #[test]
    fn fields_are_fixed_and_nullable() {
        let ok = command_result(
            "online",
            "restore-existing",
            true,
            None,
            Some(r"H:\logs\a.log"),
            None,
            None,
        );
        assert_eq!(ok["command"], "online");
        assert_eq!(ok["operation"], "restore-existing");
        assert_eq!(ok["success"], true);
        assert_eq!(ok["log_path"], r"H:\logs\a.log");
        assert!(ok["error"].is_null());
        assert!(ok["task_id"].is_null(), "online 不建任务目录，task_id 应为 null");

        let prep = command_result(
            "prepare",
            "backup",
            true,
            Some("abc-123"),
            Some("p.log"),
            None,
            None,
        );
        assert_eq!(prep["task_id"], "abc-123");

        let bad = command_result(
            "online",
            "backup",
            false,
            None,
            None,
            Some("[BR-APL-001] boom"),
            Some("BR-APL-001"),
        );
        assert_eq!(bad["success"], false);
        assert!(bad["log_path"].is_null(), "缺日志路径时必须是 null 而非省略");
        assert_eq!(bad["error"], "[BR-APL-001] boom");
        // 码是独立字段：消费方判它，不解析 error 里的方括号前缀。
        assert_eq!(bad["error_code"], "BR-APL-001");
    }

    #[test]
    fn windows_path_backslashes_are_escaped() {
        // 单反斜杠必须被 JSON 转义成 \\，否则消费方 json.loads 会报错。
        let value = command_result(
            "online",
            "backup",
            true,
            None,
            Some(r"H:\BRImages\x.wim"),
            None,
            None,
        );
        let text = value.to_string();
        assert!(text.contains(r"H:\\BRImages\\x.wim"), "实际输出：{text}");
        // 反解一次，确认是合法 JSON 且路径还原无损。
        let back: serde_json::Value = serde_json::from_str(&text).expect("必须是合法 JSON");
        assert_eq!(back["log_path"], r"H:\BRImages\x.wim");
    }

    #[test]
    fn enabled_defaults_to_false() {
        // 不测 enable()：开关是进程级全局，并行单测会互相污染，
        // 真实开/关由实机冒烟覆盖（br-smoke.sh 带 --json 全链跑）。
        assert!(!super::enabled());
    }
}
