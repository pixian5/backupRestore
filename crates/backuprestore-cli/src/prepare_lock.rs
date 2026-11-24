//! 准备期「在办锁」：把「正在准备中的任务」与「孤儿任务」区分开（prepare_lock.rs）。
//!
//! 【2026-11-24 实机缺陷，两次复现】准备流程（`prepare_payload`）先把任务落盘（阶段 = `Prepared`），
//! 要到最后建启动项时才写 `boot-entry.json`。中间要做的事——拷两遍恢复环境镜像、注入载荷、
//! 一次刻意的 5 秒等待——在虚拟机里约 17 秒。这段时间里，任务在「孤儿任务守卫」
//! （`pending_task::collect_orphan_tasks`）眼里与真正的孤儿**一模一样**：
//! 非终态、没写过目标卷、没有启动项记录。
//!
//! 而界面程序**每次启动**都先跑守卫（开机自启、用户再点一次图标都会触发）。于是在办的准备被它
//! 标成失败、任务产物被删。两种时序、两种后果（都是实机日志）：
//! - 守卫早到：删掉了空的 `mount` 目录 → 85 毫秒后 DISM（部署映像服务和管理工具）挂载报
//!   `0xc1420115`「用户尝试装入不存在的目录」；
//! - 守卫迟到几秒：删产物被原有的「镜像树还挂着」保护拦住，但任务已被标失败 → 准备照常走完、
//!   一次性启动**已武装**，10 毫秒后状态推进被拒（`BR-CORE-074`）→ 再补偿，按字节回退启动配置
//!   被挡（`BR-TSK-035`，补偿未完成）。
//!
//! 做法：准备流程从任务落盘那一刻起，以**不与他人共享**的方式打开任务目录里的 `preparing.lock`，
//! 一直握到准备结束。别人（守卫、放弃任务的入口）去碰这个文件：
//! - 共享冲突 → 有进程握着它 → 任务在办，一律不动；
//! - 打得开，或文件不存在 → 没人在办。进程崩溃、断电时系统关闭句柄，锁自动失效，
//!   **不会留下永远清不掉的僵尸**。
//!
//! 文件带「所有句柄关闭即删除」标志，准备正常结束时不留残余。

#![cfg(windows)]

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use backuprestore_core::TaskError;

use crate::err;

/// 任务目录里的在办锁文件名（不在 `cleanup_task_artifacts` 的白名单里，清理产物不会碰它）。
pub(crate) const PREPARING_LOCK: &str = "preparing.lock";

/// `winnt.h` 的 `FILE_FLAG_DELETE_ON_CLOSE`：最后一个句柄关闭时删除文件（进程崩溃时同样生效）。
const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;

/// `winerror.h` 的 `ERROR_SHARING_VIOLATION`：别人正以不兼容的共享方式握着这个文件。
const ERROR_SHARING_VIOLATION: i32 = 32;

/// 握在手里的在办锁。离开作用域即放手（句柄关闭，锁文件随之自动删除）。
pub(crate) struct PreparingGuard {
    /// 只为持有句柄而存在。
    _handle: File,
}

pub(crate) fn lock_path(task_dir: &Path) -> PathBuf {
    task_dir.join(PREPARING_LOCK)
}

/// 握住 `task_dir` 的在办锁；返回的守卫必须活到准备结束。
pub(crate) fn hold(task_dir: &Path) -> Result<PreparingGuard, TaskError> {
    let path = lock_path(task_dir);
    // 不用 create_new：断电后可能留下一个没人握着的旧锁文件，直接接管即可（本次放手时它一并被删）。
    let handle = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(&path)
        .map_err(|error| {
            err(&format!(
                "cannot hold the preparing lock {}: {error}",
                path.display()
            ))
        })?;
    Ok(PreparingGuard { _handle: handle })
}

/// 对一个任务目录「有没有人在办」的读数。三种结果分开报，不让「读不到」长得像「没人在办」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreparingProbe {
    /// 有进程握着在办锁（共享冲突）。
    Held,
    /// 锁文件不存在。
    NoLockFile,
    /// 锁文件在，但谁都能打开：没人握着（例如断电留下的旧文件）。
    NotHeld,
    /// 其它错误：证明不了没人在办。
    Unreadable(io::ErrorKind),
}

pub(crate) fn probe(task_dir: &Path) -> PreparingProbe {
    match OpenOptions::new().read(true).open(lock_path(task_dir)) {
        Ok(_) => PreparingProbe::NotHeld,
        Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => PreparingProbe::Held,
        Err(error) if error.kind() == io::ErrorKind::NotFound => PreparingProbe::NoLockFile,
        Err(error) => PreparingProbe::Unreadable(error.kind()),
    }
}

/// 守卫与两条放弃入口用的判据：拿不出「没人在办」的证据，就当它在办、不动它。
pub(crate) fn in_flight(task_dir: &Path) -> bool {
    !matches!(
        probe(task_dir),
        PreparingProbe::NoLockFile | PreparingProbe::NotHeld
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "backuprestore-prepare-lock-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_held_lock_reads_as_in_flight_and_vanishes_when_released() {
        let dir = temp_dir("held");
        assert!(!in_flight(&dir), "没有锁文件 = 没人在办");
        assert_eq!(probe(&dir), PreparingProbe::NoLockFile);

        let guard = hold(&dir).unwrap();
        assert!(lock_path(&dir).is_file(), "握住期间锁文件必须存在");
        assert!(in_flight(&dir), "有人握着 = 在办");
        assert_eq!(probe(&dir), PreparingProbe::Held);
        // 「另一个进程」的视角：以普通方式打开必须被拒（共享冲突），不是读到一个空文件。
        assert!(
            File::open(lock_path(&dir)).is_err(),
            "在办期间别人不得打开锁文件"
        );

        drop(guard);
        assert!(
            !lock_path(&dir).exists(),
            "放手后锁文件必须自动删除，不留残余"
        );
        assert!(!in_flight(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stale_lock_file_nobody_holds_is_not_in_flight() {
        // 断电可能留下没人握着的旧锁文件：它不能让任务永远被当成「在办」而清不掉。
        let dir = temp_dir("stale");
        std::fs::write(lock_path(&dir), b"").unwrap();
        assert_eq!(probe(&dir), PreparingProbe::NotHeld);
        assert!(!in_flight(&dir));
        // 旧文件可以被新的准备直接接管。
        let guard = hold(&dir).unwrap();
        assert!(in_flight(&dir));
        drop(guard);
        assert!(!lock_path(&dir).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_task_directory_is_not_in_flight() {
        let dir = temp_dir("missing").join("no-such-task");
        assert_eq!(probe(&dir), PreparingProbe::NoLockFile);
        assert!(!in_flight(&dir));
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }
}
