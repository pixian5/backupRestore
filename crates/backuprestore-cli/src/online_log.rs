//! 在线任务本轮工作目录（日志 + 引导方案 session）的选址。
//!
//! 【2026-10-16 审查修订，日志缺口 9】此前固定写 `%TEMP%\BackupRestore-online-*`：
//! 磁盘清理、甚至「把含 TEMP 的卷还原掉」这类操作都会把完整现场连带删掉，
//! 事后只剩 `gui.log` 里一行含路径的摘要，而现场恰恰是排查还原失败唯一的材料。
//!
//! 【2026-10-18 用户要求】「日志放到一个不是 PE、不是目标分区的一个地方」：
//! 选址从 `<exe>\logs` 搬到 `log_root::logs_dir()`（本机即 `H:\brwork\logs`，
//! 项目约定工作卷）。「程序目录不可写」的旧降级语义由 `log_root` 的卷探测
//! 吸收——探测全军覆没才退回 `%TEMP%`，并把真实原因随返回值带出，调用方
//! 如实写进日志与返回详情，绝不显示一个对不上的路径。
//!
//! 每轮一个目录、日志体积只有几 KB；这里刻意**不**做旧目录清理——删文件一旦
//! 误伤并发实例的现场，代价远大于收益。
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
///
/// `operation` 决定目录名里的动作标记（见 `dir_tag`）：**备份任务以前也叫
/// `BackupRestore-online-*`**，名字看着像还原，翻日志时容易找错方向（旧待办
/// B6）。现在按动作分叉，找不到标记就退回 `online`。
pub(crate) fn resolve_dir(operation: &str) -> Result<(PathBuf, String), std::io::Error> {
    resolve_in(crate::log_root::logs_dir(), dir_tag(operation))
}

/// operation → 目录名标记。**白名单**：绝不把外部字符串直接拼进路径
/// （`..`、分隔符之类会把目录建到别处去）。不认识的一律按 `online`。
fn dir_tag(operation: &str) -> &'static str {
    match operation.trim().to_ascii_lowercase().as_str() {
        "backup" => "backup",
        "restore-existing" | "restore" => "restore",
        "create-secondary" => "secondary",
        "probe" => "probe",
        "image-admin" => "image-admin",
        _ => "online",
    }
}

/// 真逻辑：在 `base` 下建本轮目录，建不起退 `%TEMP%` 并说清原因。
/// 单独拆出来是为了让单测能传入临时目录——`log_root` 是进程级单例，
/// 直接测它会写进真实日志根。
fn resolve_in(base: &Path, tag: &str) -> Result<(PathBuf, String), std::io::Error> {
    let run_name = format!(
        "BackupRestore-{tag}-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let dir = base.join(&run_name);
    match fs::create_dir_all(&dir) {
        Ok(()) => Ok((dir, String::new())),
        Err(error) => {
            let reason = format!(
                "日志目录不可写（{}：{error}），已退回 %TEMP%",
                base.display()
            );
            let dir = std::env::temp_dir().join(run_name);
            fs::create_dir_all(&dir)?;
            Ok((dir, reason))
        }
    }
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
    /// `T/BackupRestore-backup` 匹配不上 `T/BackupRestore-backup-33523-...`。
    /// 判「在 %TEMP% 下」必须分量级地比父目录 + 名字前缀。
    fn is_temp_run_dir(dir: &Path) -> bool {
        let name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        name.starts_with("BackupRestore-") && dir.parent() == Some(std::env::temp_dir().as_path())
    }

    /// 目录名按动作分叉：备份任务不能再叫 `online`（旧待办 B6——名字像还原，
    /// 翻日志时容易找错方向）。
    #[test]
    fn dir_name_marks_the_action() {
        assert_eq!(dir_tag("backup"), "backup");
        assert_eq!(dir_tag("restore-existing"), "restore");
        assert_eq!(dir_tag("create-secondary"), "secondary");
        assert_eq!(dir_tag("probe"), "probe");
        // 大小写与空白归一
        assert_eq!(dir_tag("  BACKUP "), "backup");
        // 不认识的一律回 online —— 绝不把外部串拼进路径（`..`/分隔符会建到别处）
        assert_eq!(dir_tag("../../evil"), "online");
        assert_eq!(dir_tag(""), "online");
    }

    /// 备份任务的目录名必须带 backup，且不再出现 online 字样。
    #[test]
    fn backup_dir_is_not_named_online() {
        let root = scratch("backup-tag");
        let base = root.join("logs");
        fs::create_dir_all(&base).unwrap();
        let (dir, _) = resolve_in(&base, dir_tag("backup")).unwrap();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("BackupRestore-backup-"), "实际：{name}");
        assert!(!name.contains("online"), "备份目录不该带 online，实际：{name}");
        fs::remove_dir_all(&root).unwrap();
    }

    /// 日志根可写时本轮目录必须挂在它下面，且不得落在 `%TEMP%`——
    /// 缺口 9 的全部意义就是让现场躲开 TEMP 清理。
    #[test]
    fn prefers_the_log_root_over_temp() {
        let root = scratch("primary");
        let base = root.join("logs");
        fs::create_dir_all(&base).unwrap();
        let (dir, note) = resolve_in(&base, "backup").unwrap();
        assert!(
            dir.starts_with(&base),
            "应落在日志根下，实际：{}",
            dir.display()
        );
        assert!(
            !is_temp_run_dir(&dir),
            "不能退到 %TEMP%，实际：{}",
            dir.display()
        );
        assert!(note.is_empty(), "主路径不该带降级说明，实际：{note}");
        assert!(dir.is_dir(), "目录必须真的建出来");
        fs::remove_dir_all(&root).unwrap();
    }

    /// 日志根不可写时降级 `%TEMP%`，并给出人看的原因。
    /// 用「base 本身是一个普通文件」模拟不可写，跨平台可复现。
    #[test]
    fn falls_back_to_temp_and_says_why() {
        let root = scratch("fallback");
        let base = root.join("logs");
        fs::create_dir_all(&root).unwrap();
        fs::write(&base, b"not a directory").unwrap();
        let (dir, note) = resolve_in(&base, "backup").unwrap();
        assert!(
            is_temp_run_dir(&dir),
            "应退回 %TEMP%，实际：{}",
            dir.display()
        );
        assert!(note.contains("日志目录不可写"), "原因必须写清楚，实际：{note}");
        assert!(dir.is_dir(), "回退目录也必须真的建出来");
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    /// `resolve_dir()` 与 `log_root` 同源：进程里只有一个日志根，在线工作
    /// 目录必须挂在它下面（本机 macOS 落在 `<exe>\logs` 兜底分支）。
    #[test]
    fn resolve_dir_hangs_off_the_shared_log_root() {
        let (dir, note) = resolve_dir("backup").unwrap();
        assert!(
            dir.starts_with(crate::log_root::logs_dir()),
            "应挂在统一日志根下，实际：{}",
            dir.display()
        );
        assert!(note.is_empty(), "本机日志根可写，不该有降级说明，实际：{note}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 降级原因必须原样透出系统错误码——「不可写」与「拒绝访问」对排障是
    /// 两回事，不能只报一句话了事。
    #[test]
    fn fallback_reason_carries_the_real_system_error() {
        let root = scratch("reason");
        let base = root.join("logs");
        fs::create_dir_all(&root).unwrap();
        fs::write(&base, b"not a directory").unwrap();
        let (dir, note) = resolve_in(&base, "backup").unwrap();
        assert!(note.contains('（'), "原因里应带括号包裹的系统错误，实际：{note}");
        assert!(note.contains('）'), "原因里应带括号包裹的系统错误，实际：{note}");
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
