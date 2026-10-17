//! 日志根目录的统一解析。
//!
//! 【2026-10-18 用户要求】「日志应该放到一个不是 PE、不是目标分区的一个地方」。
//!
//! ## 旧行为的问题
//!
//! 所有日志此前一律写 `<exe>\logs`：`gui.log`、`panic.log`、
//! `launcher-errors.log`、`prepare-bootstrap.log`、PE 安装期的
//! `boot-sequence.log` / `pe-reboot.log` / `pe-format.log`、bcdedit 诊断输出、
//! 在线任务本轮工作目录。本机部署目录是 `C:\Users\Public\backupRestore-package`：
//!
//! - C: 是系统盘，A 入口还原（离线）时它就是**目标分区**——日志写在
//!   「即将被重写」的卷上，还原成功之时正是日志消失之刻；
//! - PE/WinRE 里 `<exe>` 是内存盘 `X:\Windows\System32`，重启即失；
//! - `<exe>\logs` 随程序目录搬迁而漂移，用户要的「一个固定的地方」不存在。
//!
//! ## 新策略（进程内只解析一次，所有日志站点共用，保证读写同源）
//!
//! 1. **`H:\brwork\logs`**：本项目磁盘布局约定的事务工作卷
//!    （`H:\brwork` + `H:\BRImages`；还原目标是 C:/E:/T:/P:，Y: 是 WinRE）。
//!    既不是 PE 也不是任何目标分区，路径固定、可预期。`H:\brwork`
//!    不存在或不可写时不硬造（陌生机器上不猜）。
//! 2. **卷探测**：`C:` 到 `Z:` 逐个试建目录并写探针，跳过系统盘与程序所在盘
//!    （PE 里程序盘就是内存盘，跳了才躲得开重启即失）。第一轮只认
//!    「卷根已有 `brwork`」的工作卷——按内容认出它，不死磕 H: 这个字母；
//!    都没有再按字母序取第一个可写卷，建 `<卷>\BackupRestore-logs`
//!    （与 `session_log` 的镜像目录同名）。探测时建出来又没被选中的目录
//!    会清掉，不在各个卷根留垃圾。
//! 3. **`<exe>\logs`**：最后兜底，保证日志永远有地方写。
//!
//! 解析结果写进 `<根>\log-root.txt`（一行：时间 + 路径 + 来源）——日志自己
//! 说明自己住在哪，排障时不用猜。
//!
//! 纯判据（`probe_letters` / `candidate_in`）不碰平台 API，本机单测可覆盖；
//! 卷探测是 Windows 语义，其它平台上所有 `X:\` 都不是目录，自然落到兜底。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// 日志根目录。进程内只解析一次；GUI、CLI、PE 桌面各入口共用，
/// 写方与读方因此永远同源（`launcher-errors.log` 由 CLI 写、GUI 读，
/// 两个进程各解析一次，策略确定性保证结果一致）。
pub(crate) fn logs_dir() -> &'static Path {
    ROOT.get_or_init(resolve).as_path()
}

fn resolve() -> PathBuf {
    let (dir, source) = match work_volume_dir() {
        Some(dir) => (dir, "工作卷约定 H:\\brwork\\logs"),
        None => match probe_dir() {
            Some(dir) => (dir, "卷探测（跳过系统盘与程序盘）"),
            None => (exe_logs_dir(), "程序目录兜底 <exe>\\logs"),
        },
    };
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(
        dir.join("log-root.txt"),
        format!(
            "[{}] 日志根目录：{}（来源：{source}）\n",
            chrono::Utc::now().to_rfc3339(),
            dir.display()
        ),
    );
    dir
}

/// 策略 1：项目约定工作卷。只认**已存在**的 `H:\brwork`——
/// 不在陌生机器上凭空造目录。
fn work_volume_dir() -> Option<PathBuf> {
    let work = PathBuf::from(r"H:\brwork");
    if !work.is_dir() {
        return None;
    }
    let logs = work.join("logs");
    writable_dir(&logs).then_some(logs)
}

/// 策略 2：卷探测。第一轮只认带 `brwork` 的工作卷，第二轮才按字母序取
/// 第一个可写卷——否则可能撞见还原目标卷（那正是用户要躲开的）。
fn probe_dir() -> Option<PathBuf> {
    let letters = probe_letters(system_drive_letter(), exe_drive_letter());
    let mut candidates: Vec<Candidate> = Vec::new();
    for letter in letters {
        let root = PathBuf::from(format!(r"{letter}:\"));
        if !root.is_dir() {
            continue;
        }
        let dir = candidate_in(&root);
        let fresh = !dir.exists();
        let writable = writable_dir(&dir);
        let is_work = root.join("brwork").is_dir();
        candidates.push(Candidate {
            dir,
            fresh,
            writable,
            is_work,
        });
    }
    let selected = candidates
        .iter()
        .find(|candidate| candidate.is_work && candidate.writable)
        .or_else(|| candidates.iter().find(|candidate| candidate.writable))
        .map(|candidate| candidate.dir.clone());
    // 探测时建出来又没被选中的目录清掉，不在各个卷根留垃圾。
    for candidate in candidates.iter().filter(|candidate| candidate.fresh) {
        if selected.as_deref() != Some(candidate.dir.as_path()) {
            let _ = std::fs::remove_dir_all(&candidate.dir);
        }
    }
    selected
}

struct Candidate {
    dir: PathBuf,
    /// 探测前是否不存在（不存在 = 我们建的，没选中就该清掉）。
    fresh: bool,
    writable: bool,
    /// 卷根是否带 `brwork`（按内容认工作卷）。
    is_work: bool,
}

/// 候选目录：卷根已有 `brwork` 就认它（按内容认工作卷），否则
/// `<卷>\BackupRestore-logs`（与 `session_log` 的镜像目录同名）。
fn candidate_in(volume_root: &Path) -> PathBuf {
    let work = volume_root.join("brwork");
    if work.is_dir() {
        work.join("logs")
    } else {
        volume_root.join("BackupRestore-logs")
    }
}

/// 实测可写：能建目录、能写探针文件。很多卷在 PE 里只读或没格式化，
/// `is_dir` 通不过实地考察。
fn writable_dir(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-probe");
    if std::fs::write(&probe, b"probe").is_err() {
        return false;
    }
    let _ = std::fs::remove_file(&probe);
    true
}

/// 策略 3：程序目录兜底。
fn exe_logs_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
}

fn system_drive_letter() -> Option<char> {
    std::env::var("SystemDrive")
        .ok()
        .as_deref()
        .map(Path::new)
        .and_then(drive_letter)
}

fn exe_drive_letter() -> Option<char> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
        .as_deref()
        .and_then(drive_letter)
}

/// 取 `C:` / `c:\...` 形态的盘符；其它形态不猜。与 `session_log`
/// 的判断同源，避免两套标准。
fn drive_letter(path: &Path) -> Option<char> {
    crate::session_log::drive_letter_of(path)
}

/// 探测顺序：`C:` 到 `Z:`，跳过系统盘与程序盘。纯函数。
fn probe_letters(system: Option<char>, exe: Option<char>) -> Vec<char> {
    let system = system.map(|letter| letter.to_ascii_uppercase());
    let exe = exe.map(|letter| letter.to_ascii_uppercase());
    ('C'..='Z')
        .filter(|letter| Some(*letter) != system && Some(*letter) != exe)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 系统盘与程序盘都要跳过：PE 里程序盘就是内存盘，跳了才躲得开重启即失。
    #[test]
    fn probe_letters_skips_system_and_program_drives() {
        let letters = probe_letters(Some('C'), Some('D'));
        assert!(!letters.contains(&'C'), "系统盘不探测");
        assert!(!letters.contains(&'D'), "程序盘不探测");
        assert_eq!(letters.first(), Some(&'E'));
        assert_eq!(letters.last(), Some(&'Z'));
        // 大小写不敏感（SystemDrive 可能是小写）。
        let lower = probe_letters(Some('c'), None);
        assert!(!lower.contains(&'C'));
        assert!(lower.contains(&'D'));
    }

    /// 卷根已有 `brwork` 时候选是它的 `logs` 子目录——按内容认工作卷。
    #[test]
    fn candidate_prefers_an_existing_work_directory() {
        let root = std::env::temp_dir().join(format!("br-log-root-work-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("brwork")).unwrap();
        assert_eq!(candidate_in(&root), root.join("brwork").join("logs"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 没有 `brwork` 的卷用固定目录名，且与 session_log 的镜像目录同名。
    #[test]
    fn candidate_falls_back_to_the_shared_directory_name() {
        let root = std::env::temp_dir().join(format!("br-log-root-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(
            candidate_in(&root),
            root.join(crate::session_log::MIRROR_DIR_NAME)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 兜底分支在任何平台都必然可解析：本机（macOS）没有 `H:\brwork`、
    /// 没有 `C:\` 形态的卷，`logs_dir()` 必须落到 `<exe>\logs`，
    /// 既不能死锁也不能 panic。
    #[test]
    fn logs_dir_always_resolves_to_a_logs_directory() {
        let dir = logs_dir();
        assert!(dir.ends_with("logs"), "实际：{}", dir.display());
    }
}
