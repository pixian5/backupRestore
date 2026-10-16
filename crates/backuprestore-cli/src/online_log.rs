//! 在线任务本轮工作目录（日志 + 引导方案 session）的选址。
//!
//! 【2026-10-16 审查修订，日志缺口 9】此前固定写 `%TEMP%\BackupRestore-online-*`：
//! 磁盘清理、甚至「把含 TEMP 的卷还原掉」这类操作都会把完整现场连带删掉，
//! 事后只剩 `gui.log` 里一行含路径的摘要，而现场恰恰是排查还原失败唯一的材料。
//!
//! 选址规则：
//! 1. 优先 `<exe>\logs\BackupRestore-online-<pid>-<nanos>`——与 `gui.log`、
//!    `panic.log`、PE bcdedit 输出同一处，属于部署自带目录，系统清理规则不碰。
//!    每轮独立子目录，多实例并发互不覆盖，`run_program` 的 append 语义也
//!    因此永远从空文件开始。
//! 2. 程序目录不可写时（装在 Program Files、从只读介质或共享运行）降级回
//!    `%TEMP%`，并把真实原因随返回值带出，调用方如实写进日志与返回详情——
//!    绝不显示一个对不上的路径。
//! 3. `current_exe` 都取不到（极少数情况）：直接 `%TEMP%`，等同旧行为，不猜。
//!
//! 每轮一个目录、日志体积只有几 KB，落在程序目录也不会堆积成问题；这里
//! 刻意**不**做旧目录清理——删文件一旦误伤并发实例的现场，代价远大于收益。
//!
//! 纯路径决策，不碰任何平台 API，本机单测即可覆盖全部分支。

use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

/// 解析本轮工作目录。
///
/// 返回 `(目录, 降级原因)`：第二个值可空，非空时必须由调用方如实展示——
/// 用户在弹窗里看到的路径、日志第一行的位置说明、`OnlineResult.detail`
/// 三处必须一致，否则排障时按日志里的路径去翻会扑空。
pub(crate) fn resolve_dir(exe_dir: Option<&Path>) -> Result<(PathBuf, String), std::io::Error> {
    let run_name = format!(
        "BackupRestore-online-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    if let Some(exe_dir) = exe_dir {
        let dir = exe_dir.join("logs").join(&run_name);
        match fs::create_dir_all(&dir) {
            Ok(()) => return Ok((dir, String::new())),
            Err(error) => {
                let reason = format!("程序目录不可写（{error}），已退回 %TEMP%");
                let dir = std::env::temp_dir().join(run_name);
                fs::create_dir_all(&dir)?;
                return Ok((dir, reason));
            }
        }
    }
    let dir = std::env::temp_dir().join(run_name);
    fs::create_dir_all(&dir)?;
    Ok((dir, "无法确定程序目录，日志落在 %TEMP%".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "br-online-log-{name}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    /// `Path::starts_with` 是**分量**前缀匹配，不是字符串前缀：
    /// `T/BackupRestore-online` 匹配不上 `T/BackupRestore-online-33523-...`。
    /// 判「在 %TEMP% 下」必须分量级地比父目录 + 名字前缀。
    fn is_temp_online_dir(dir: &Path) -> bool {
        let name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        name.starts_with("BackupRestore-online") && dir.parent() == Some(std::env::temp_dir().as_path())
    }

    /// 程序目录可写时日志必须落在 `<exe>\logs\BackupRestore-online-*`，
    /// 且不得落在 `%TEMP%`——缺口 9 的全部意义就是让现场躲开 TEMP 清理。
    #[test]
    fn prefers_the_program_directory_over_temp() {
        let root = scratch("primary");
        let exe_dir = root.join("exe");
        fs::create_dir_all(&exe_dir).unwrap();
        let (dir, note) = resolve_dir(Some(&exe_dir)).unwrap();
        assert!(
            dir.starts_with(exe_dir.join("logs")),
            "应落在程序目录 logs 下，实际：{}",
            dir.display()
        );
        assert!(
            !is_temp_online_dir(&dir),
            "不能退到 %TEMP%，实际：{}",
            dir.display()
        );
        assert!(note.is_empty(), "主路径不该带降级说明，实际：{note}");
        assert!(dir.is_dir(), "目录必须真的建出来");
        fs::remove_dir_all(&root).unwrap();
    }

    /// 程序目录不可写时降级 `%TEMP%`，并给出人看的原因。
    /// 用「logs 位置是一个普通文件」模拟不可写，跨平台可复现。
    #[test]
    fn falls_back_to_temp_and_says_why() {
        let root = scratch("fallback");
        let exe_dir = root.join("exe");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::write(exe_dir.join("logs"), b"not a directory").unwrap();
        let (dir, note) = resolve_dir(Some(&exe_dir)).unwrap();
        assert!(
            is_temp_online_dir(&dir),
            "应退回 %TEMP%，实际：{}",
            dir.display()
        );
        assert!(note.contains("程序目录不可写"), "原因必须写清楚，实际：{note}");
        assert!(dir.is_dir(), "回退目录也必须真的建出来");
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 取不到程序目录时等同旧行为：直接 `%TEMP%`，原因写明「无法确定程序目录」。
    #[test]
    fn without_exe_directory_uses_temp_only() {
        let (dir, note) = resolve_dir(None).unwrap();
        assert!(
            is_temp_online_dir(&dir),
            "应落在 %TEMP%，实际：{}",
            dir.display()
        );
        assert!(note.contains("无法确定程序目录"), "实际：{note}");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 降级原因必须原样透出系统错误码——「程序目录不可写」与「拒绝访问」
    /// 对排障是两回事，不能只报一句话了事。
    #[test]
    fn fallback_reason_carries_the_real_system_error() {
        let root = scratch("reason");
        let exe_dir = root.join("exe");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::write(exe_dir.join("logs"), b"not a directory").unwrap();
        let (dir, note) = resolve_dir(Some(&exe_dir)).unwrap();
        assert!(note.contains('（'), "原因里应带括号包裹的系统错误，实际：{note}");
        assert!(note.contains('）'), "原因里应带括号包裹的系统错误，实际：{note}");
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
