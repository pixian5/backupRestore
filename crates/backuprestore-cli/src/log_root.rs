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
            letter,
        });
    }
    // 【2026-10-18 缺陷修复 L3】兜底轮排除「装着 Windows 的离线卷」。
    // 注释早就写着「可能撞见还原目标卷（正是用户要躲开的）」，但实现只按可写取
    // 第一个：在 RE 里系统盘是 X:（RAM），真正的 Windows 在 C: ——而 C: 往往正是
    // 本次还原的目标卷，日志落上去会在格式化时被抹掉，还原成功即丢证据。
    // 先看有没有「既不是工作卷、也不像 Windows 卷」的可写卷；实在没有才退回任意
    // 可写卷（日志再差也要能写，不能因为挑地方而完全不写）。
    let looks_like_windows = |letter: char| {
        PathBuf::from(format!(r"{letter}:\Windows\System32\config\SYSTEM")).is_file()
    };
    let selected = candidates
        .iter()
        .find(|candidate| candidate.is_work && candidate.writable)
        .or_else(|| {
            candidates
                .iter()
                .find(|candidate| candidate.writable && !looks_like_windows(candidate.letter))
        })
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
    /// 卷盘符（L3：判断该卷是不是「装着 Windows 的离线卷」需要它）。
    letter: char,
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

/// 日志目录里最近修改的日志文件（新的在前）。
///
/// 【为什么要有它】错误码体系落地后，报障第一步就是「日志给我」，而日志根目录
/// 下既有常驻的 `gui.log`，也有每个在线任务一层子目录的任务日志
/// （`<根>/<动作>/<会话>/…`）——让用户自己在资源管理器里翻目录找本轮日志，
/// 是把排障成本转嫁给用户。图形界面的「查看日志」入口要列的就是这个清单。
///
/// 【为什么是纯函数】排序、截断和「什么算日志文件」都是平台无关逻辑，
/// 放在这里本机（macOS）就能单测；GUI 只负责把它显示出来。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LogFileEntry {
    pub path: std::path::PathBuf,
    pub size: u64,
    /// 拿不到修改时间时如实为空（不能拿"现在"糊弄排序）。
    pub modified: Option<std::time::SystemTime>,
}

/// 递归深度上限。日志目录只有「动作/会话」两层，留一倍余量：
/// 既躲得开用户自己塞进去的深层目录，也防止无限递归。
const LOG_SCAN_MAX_DEPTH: usize = 4;

/// 列举 `root` 下的日志文件，按修改时间倒序取前 `limit` 个。
///
/// 只认 `.log` 后缀（大小写不敏感）：目录里还有 `log-root.txt`、
/// `status.json` 等说明性文件，混进来会稀释清单。目录不可读返回空清单——
/// 「没有日志可看」与「看不了」在界面上都表现为空列表加一句提示，
/// 不额外分叉。
pub(crate) fn recent_log_files(root: &Path, limit: usize) -> Vec<LogFileEntry> {
    if limit == 0 {
        return Vec::new();
    }
    let mut found: Vec<LogFileEntry> = Vec::new();
    collect_log_files(root, 0, &mut found);
    found.sort_by(|left, right| right.modified.cmp(&left.modified));
    found.truncate(limit);
    found
}

fn collect_log_files(dir: &Path, depth: usize, out: &mut Vec<LogFileEntry>) {
    if depth >= LOG_SCAN_MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let path = entry.path();
        if metadata.is_dir() {
            collect_log_files(&path, depth + 1, out);
            continue;
        }
        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("log"))
        {
            continue;
        }
        out.push(LogFileEntry {
            path,
            size: metadata.len(),
            modified: metadata.modified().ok(),
        });
    }
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

    /// 造一个带指定修改时间的日志文件（显式设时间，不靠 sleep 赌精度）。
    fn write_log(path: &std::path::Path, modified: std::time::SystemTime) {
        std::fs::write(path, b"log").unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(modified).unwrap();
    }

    fn stamp(seconds: u64) -> std::time::SystemTime {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)
    }

    /// 清单按修改时间倒序：报障要的是「本轮日志」，新的必须在最上面。
    #[test]
    fn recent_log_files_returns_the_newest_first() {
        let root = std::env::temp_dir().join(format!("br-log-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("backup").join("session")).unwrap();
        write_log(&root.join("gui.log"), stamp(1_000));
        write_log(&root.join("old.log"), stamp(500));
        // 任务日志在子目录里——只扫根目录会让本轮日志永远找不到。
        write_log(&root.join("backup").join("session").join("task.log"), stamp(2_000));

        let files = recent_log_files(&root, 10);
        let names: Vec<String> = files
            .iter()
            .map(|entry| entry.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["task.log", "gui.log", "old.log"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 只认 `.log`：`log-root.txt`、`status.json` 混进来会把清单冲淡。
    #[test]
    fn recent_log_files_ignores_non_log_files() {
        let root = std::env::temp_dir().join(format!("br-log-filter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        write_log(&root.join("gui.log"), stamp(10));
        write_log(&root.join("log-root.txt"), stamp(20));
        write_log(&root.join("status.json"), stamp(30));
        write_log(&root.join("panic.LOG"), stamp(40));

        let files = recent_log_files(&root, 10);
        let names: Vec<String> = files
            .iter()
            .map(|entry| entry.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // 后缀大小写不敏感：Windows 上文件名大小写随手写，不能只认小写。
        assert_eq!(names, vec!["panic.LOG", "gui.log"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 截断与边界：清单长度受 `limit` 约束，`limit=0` 和「目录不存在」
    /// 都返回空清单而不是 panic（界面据此显示一句提示）。
    #[test]
    fn recent_log_files_respects_the_limit_and_missing_roots() {
        let root = std::env::temp_dir().join(format!("br-log-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        write_log(&root.join("a.log"), stamp(1));
        write_log(&root.join("b.log"), stamp(2));
        write_log(&root.join("c.log"), stamp(3));
        assert_eq!(recent_log_files(&root, 2).len(), 2);
        assert_eq!(recent_log_files(&root, 0).len(), 0);
        assert_eq!(
            recent_log_files(&root, 2)[0].path.file_name().unwrap(),
            "c.log"
        );
        let _ = std::fs::remove_dir_all(&root);
        assert!(recent_log_files(&root, 5).is_empty());
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
