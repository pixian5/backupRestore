//! 会话日志落点：主路径 + **跨重启存活的镜像**。
//!
//! ## 修的是什么（2026-10-17 第二轮核查，日志缺口 1 / 2）
//!
//! WinRE 与 WinPE 里，程序是从 **`X:`（内存盘）** 上跑起来的
//! （`winpeshl.ini -> X:\Windows\System32\Recovery.exe`）。于是：
//!
//! - `main.rs` 的逃生决策日志 `X:\BackupRestore-escape.log`、早期日志
//!   `X:\BackupRestore-Recovery-early.log`；
//! - `native_gui` 的 `gui.log`、`main` 的 `panic.log` / `launcher-errors.log`、
//!   在线操作日志 —— 这些走「`<exe>\logs`」的日志，在 PE/RE 里等于 `X:\Windows\System32\logs`；
//!
//! **全部在重启瞬间消失**。而它们记录的恰恰是最需要复盘的东西：连败几次、
//! 用户选了「是/否/超时」、逃出时清了什么。上一轮审查只问「这段有没有
//! `append_log`」，漏了「**落点重启后还在不在**」。
//!
//! ## 怎么修
//!
//! 给「写一行日志」加一个**镜像**：主路径照旧（行为零变化），另外按**文件名**
//! 往持久化卷上的 `BackupRestore-logs\<会话>\` 再写一份。镜像**只在确认处于
//! RAM 盘会话时**才启用，普通 Windows 上 `mirror()` 恒为 `None`，完全不生效。
//!
//! 镜像根的选择照抄 `native_gui::write_pe_exit_log` 的思路（那一段是全仓唯一
//! 做对的地方：优先独立盘符的卷、不猜 `S:`/`Q:`）：从 `C:` 起逐个盘符试建目录
//! 并写探针，**取第一个真的可写**的卷；系统盘（`X:`）一律跳过。
//!
//! 同一会话内所有日志共用一个子目录（`<pid>-<纳秒>`），避免不同任务的同名
//! 日志（`prepare.log` 等）在镜像里互相串味。
//!
//! 纯判据（`is_ram_session` / `pick_mirror_root` / `mirror_file_name`）不碰
//! 平台 API，本机单测即可覆盖；卷探测是 `#[cfg(windows)]`。

use backuprestore_core::TaskError;
use chrono::Utc;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 镜像根目录名（建在被选中的持久化卷根下）。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const MIRROR_DIR_NAME: &str = "BackupRestore-logs";

/// 镜像目录（含会话子目录）。`None` = 本次不镜像（普通 Windows 或探测失败）。
/// 只写一次、之后只读，用 `OnceLock` 就够了。
static MIRROR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 纯判据：exe 目录是否位于 PE / WinRE 的**内存盘**上。
///
/// 两个条件缺一不可：
/// - `wpeutil_present`：系统盘上有 `Windows\System32\wpeutil.exe` —— 这是
///   `winre_registration` 已经在用的 PE/RE 判据，普通 Windows 上没有它；
/// - exe 目录盘符 == 系统盘盘符：PE/RE 里程序就跑在 `X:\Windows\System32`。
///
/// 少了第一条，一台把程序装在 C: 的普通机器会被误判成 RAM 会话；
/// 少了第二条，程序在别的卷（例如从 `H:` 跑）时也会白建镜像目录。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn is_ram_session(exe_dir: &Path, system_drive: &str, wpeutil_present: bool) -> bool {
    if !wpeutil_present {
        return false;
    }
    match (drive_letter_of(exe_dir), drive_letter_of(Path::new(system_drive))) {
        (Some(exe), Some(system)) => exe.eq_ignore_ascii_case(&system),
        _ => false,
    }
}

/// 取路径的盘符：只认 `X:` / `X:\...` 这种**绝对路径**形态。
/// 相对路径、UNC、`\\?\Volume{...}` 一律返回 `None`（不猜）。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn drive_letter_of(path: &Path) -> Option<char> {
    let text = path.to_string_lossy();
    let mut chars = text.chars();
    let letter = chars.next()?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    if chars.next() != Some(':') {
        return None;
    }
    Some(letter)
}

/// 纯判据：从「候选镜像目录 + 是否真的可写」里挑第一个可写的。
///
/// 判据只看**实测可写**（调用方已经试建过目录并写过探针），不看盘符大小、
/// 不看剩余空间 —— 目的只是「找一个重启后还在的地方」，不是选最优盘。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn pick_mirror_root(candidates: &[(PathBuf, bool)]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|(_, writable)| *writable)
        .map(|(root, _)| root.clone())
}

/// 镜像里的文件名：主路径没有文件名时（裸盘符路径等）退回 `session.log`，
/// 绝不因为取不到名字就整条日志不写。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn mirror_file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "session.log".to_string())
}

/// 本次会话的镜像目录。`None` = 不镜像。
pub(crate) fn mirror() -> Option<&'static PathBuf> {
    MIRROR.get().and_then(|value| value.as_ref())
}

/// 写一行日志：主路径**必写**，镜像**尽力写**。
///
/// 返回值只反映主路径 —— 调用方按主路径的成功与否决定流程，
/// 镜像写不进去不该让任务失败（它只是多留一份证据的尝试）。
pub(crate) fn append(path: &Path, message: &str) -> Result<(), TaskError> {
    let primary = write_line(path, message);
    if let Some(mirror) = mirror() {
        let _ = fs::create_dir_all(mirror);
        let _ = write_line(&mirror.join(mirror_file_name(path)), message);
    }
    primary
}

/// 真正的落盘。语义与原先 `main::append_log` 完全一致（含 Windows 共享模式
/// 与「不 sync_all」的取舍），搬到这里是为了让「镜像」只有一处判断。
fn write_line(path: &Path, message: &str) -> Result<(), TaskError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    // Windows：必须显式放开 Read/Write 共享。进度窗线程正以只读方式持有同一个
    // 日志文件（recovery_progress 每秒读增量），默认的独占语义会让这里报
    // "另一个程序正在使用此文件"。
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let mut file = options.open(path)?;
    writeln!(file, "[{}] {message}", Utc::now().to_rfc3339())?;
    // 不 sync_all：等价 FlushFileBuffers，需要写权限独占，会把并发读日志的
    // 进度窗挤掉。日志的价值在于内容可见，落盘时机交给文件系统。
    file.flush()?;
    Ok(())
}

/// 进程最早处调用一次：判定是否 RAM 会话，是则探测并固定镜像目录。
///
/// 只在 Windows 上有意义；其它平台 `mirror()` 恒为 `None`，`append` 退化成
/// 只写主路径（行为与改动前逐字一致）。
#[cfg(windows)]
pub(crate) fn init() {
    let value = probe_mirror();
    if let Some(mirror) = value.as_ref() {
        // 探到镜像本身就是一条重要事实：事后看日志的人要知道"有第二份在别处"。
        let _ = write_line(
            &mirror.join("session.log"),
            &format!(
                "日志镜像已启用：{}（主日志仍在内存盘上，重启后会消失）",
                mirror.display()
            ),
        );
    }
    let _ = MIRROR.set(value);
}

#[cfg(windows)]
fn probe_mirror() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "X:".to_string());
    let wpeutil = Path::new(&format!(r"{system_drive}\Windows\System32\wpeutil.exe")).is_file();
    if !is_ram_session(&exe_dir, &system_drive, wpeutil) {
        return None;
    }
    let system_letter = drive_letter_of(Path::new(&system_drive));
    let session = format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let mut candidates = Vec::new();
    // 从 C: 起逐个试，跳过系统盘（PE/RE 里就是 X:，写了等于没写）。
    for letter in 'C'..='Z' {
        if Some(letter) == system_letter {
            continue;
        }
        let root = PathBuf::from(format!(r"{letter}:\"));
        if !root.is_dir() {
            continue;
        }
        let dir = root.join(MIRROR_DIR_NAME).join(&session);
        // 实测可写才算数：很多卷在 PE 里只读、或者卷根本没格式化。
        let writable = fs::create_dir_all(&dir).is_ok()
            && fs::write(dir.join(".write-probe"), b"probe").is_ok();
        let _ = fs::remove_file(dir.join(".write-probe"));
        candidates.push((dir, writable));
    }
    pick_mirror_root(&candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "br-session-log-{name}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    /// 四象限：PE 判据（wpeutil）与盘符一致两者缺一不可。
    #[test]
    fn ram_session_needs_both_the_pe_marker_and_a_matching_letter() {
        assert!(
            is_ram_session(Path::new(r"X:\Windows\System32"), "X:", true),
            "PE：程序在系统盘上 → 是 RAM 会话"
        );
        assert!(
            !is_ram_session(Path::new(r"X:\Windows\System32"), "X:", false),
            "普通 Windows 没有 wpeutil → 不能判成 RAM 会话"
        );
        assert!(
            !is_ram_session(Path::new(r"H:\brwork"), "X:", true),
            "程序在另一个卷上跑 → 主路径本就持久，不必镜像"
        );
        assert!(
            !is_ram_session(Path::new(r"X:\Windows\System32"), "", true),
            "取不到 SystemDrive 时不猜"
        );
        assert!(
            !is_ram_session(Path::new("relative/path"), "X:", true),
            "相对路径不猜"
        );
    }

    #[test]
    fn drive_letter_only_accepts_absolute_drive_paths() {
        assert_eq!(drive_letter_of(Path::new(r"C:")), Some('C'));
        assert_eq!(drive_letter_of(Path::new(r"c:\logs")), Some('c'));
        assert_eq!(drive_letter_of(Path::new(r"\\?\Volume{guid}\")), None);
        assert_eq!(drive_letter_of(Path::new("relative")), None);
    }

    /// 只看实测可写，取第一个可写的；全不可写必须是 `None`（不能退而求其次
    /// 挑一个写不进去的，那等于给了个假的安全感）。
    #[test]
    fn picks_the_first_really_writable_candidate() {
        assert_eq!(pick_mirror_root(&[]), None);
        let candidates = vec![
            (PathBuf::from("D:\\logs"), false),
            (PathBuf::from("E:\\logs"), true),
            (PathBuf::from("F:\\logs"), true),
        ];
        assert_eq!(
            pick_mirror_root(&candidates),
            Some(PathBuf::from("E:\\logs"))
        );
        let none = vec![(PathBuf::from("D:\\logs"), false)];
        assert_eq!(pick_mirror_root(&none), None);
    }

    /// 只取**文件名**，不保留目录结构：镜像是「所有日志摊平在一个目录里」。
    /// 这里刻意用 `PathBuf` 逐个分量拼，因为 `\` 在 macOS 上不是分隔符 ——
    /// 写死 Windows 形态的路径会让这条单测在本机上测到别的东西。
    #[test]
    fn mirror_file_name_keeps_only_the_file_name() {
        let nested: PathBuf = ["logs", "gui.log"].iter().collect();
        assert_eq!(mirror_file_name(&nested), "gui.log");
        let root_only: PathBuf = ["/"].iter().collect();
        assert_eq!(mirror_file_name(&root_only), "session.log");
        assert_eq!(mirror_file_name(Path::new("")), "session.log");
    }

    /// 核心回归：**没有镜像时行为与改动前完全一致** —— 只写主路径，
    /// 不额外创建任何目录（`MIRROR` 在本机永远不会 set，因为 `init` 是
    /// `#[cfg(windows)]`）。
    #[test]
    fn without_a_mirror_only_the_primary_path_is_written() {
        assert!(mirror().is_none(), "本机不应启用镜像");
        let root = scratch("primary-only");
        let log = root.join("logs").join("gui.log");
        append(&log, "第一条").unwrap();
        append(&log, "第二条").unwrap();
        let text = fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().count(), 2, "主路径应有两行");
        assert!(text.contains("第一条") && text.contains("第二条"));
        // 除了 logs 目录本身，不该多出任何镜像目录。
        let entries: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1, "只应有 logs，实际：{entries:?}");
        fs::remove_dir_all(&root).unwrap();
    }
}
