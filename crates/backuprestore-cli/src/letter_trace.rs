//! 盘符时间线：谁在什么时候把盘符挂上来、什么时候撤销。
//!
//! ## 为什么要它（2026-10-29 用户裁决「要观测，把事实钉死」）
//!
//! 准备恢复环境这条链上，**恢复分区是被某段代码中途挂上盘符的**：`recovery_identity()`
//! 解析出「盘符是 R:」，而那个 R: 不是用户给的、也不是注册信息里带的——是
//! `identity_from_diskpart()` 跑 `diskpart assign letter=R` 现挂的。挂载是**持久**写入
//! （`HKLM\SYSTEM\MountedDevices`），可这条链上**没有任何租约覆盖它**：
//! `MountLease` 只在「本次调用亲手挂上去」时才建，而这次挂载发生在更早的
//! `recovery_identity()` 里，到 `ensure_volume_mounted_leased()` 时卷已经有盘符了，
//! 于是走「复用」分支、`created=false`、租约是空的（`windows_prepare.rs` 里那条
//! `debug_assert` 盯的就是这个前提）。
//!
//! 结果：准备流程一旦在挂载之后失败/被杀，这个 R: 就留在系统里。它不会立刻出问题，
//! 因为分区还在；等哪天分区被删掉，`MountedDevices` 里的记录还在，Windows 就会为
//! 一个不存在的卷反复弹「R:\ 无法访问」。本项目 2026-10-27 已经一次性查出 6 个这样的
//! 幽灵盘符（B/G/I/Q/V/Z）。
//!
//! 在动任何代码之前，先把事实看清楚：**谁挂的、什么时候挂的、有没有被收回**。
//! 这个模块就是那台摄像机。
//!
//! ## 两类记录，缺一不可
//!
//! 1. **有主的事件**（`note()`）：`ASSIGN` / `REUSE` / `HOLD` / `RELEASE` / `EXIT`。
//!    采样器只能看见「R: 出现了」，说不出是谁让它出现的——归属必须由动手的那段代码
//!    自己写下来。
//! 2. **采样**（`BACKUPRESTORE_LETTER_TRACE_MS` 设置后才开）：每 N 毫秒读一次
//!    `GetLogicalDrives()`，**只在盘符集合变化时**写一行。它负责抓「没人声明的变化」——
//!    也就是真正的意外（别的进程挂的、Windows 自己收回的、`diskpart` 之外的手）。
//!
//! ## 边界
//!
//! - **只观测，不干预**：不撤任何盘符、不改租约行为。时间线里的 `EXIT` 行是
//!   「进程退出时还剩哪些盘符」，判「有没有泄漏」拿它和进程外的现状对照。
//! - **默认开销≈0**：不设环境变量时不启采样线程，只写几条有主事件（每次准备个位数行）。
//! - **输出纯 ASCII**：这份文件要整段 `type` 回来排障，混进非 ASCII 会变成乱码。
//! - 判据逻辑（位图解析、变化描述）在 `text_parsing`，本机单测能跑；这里只管采样与落盘。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::text_parsing::{letter_delta_line, letters_in_mask};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLogicalDrives() -> u32;
}

/// 采样间隔的环境变量。值是毫秒数；不设 = 不采样。
const SAMPLE_ENV: &str = "BACKUPRESTORE_LETTER_TRACE_MS";

/// 采样间隔的合理区间。
///
/// 下限 50 ms：一次 `diskpart` 调用本身就是几百毫秒，再密只会把文件写满而看不到更多东西。
/// 上限 5 s：再疏就可能错过「挂上、用完、马上撤掉」这种短命挂载。
const MIN_INTERVAL_MS: u64 = 50;
const MAX_INTERVAL_MS: u64 = 5_000;

struct Sink {
    log: PathBuf,
    start: Instant,
}

static SINK: OnceLock<Sink> = OnceLock::new();
static SAMPLER_STARTED: OnceLock<()> = OnceLock::new();
static STOPPED: AtomicBool = AtomicBool::new(false);
/// 采样线程与主流程会同时写同一个文件：用一把锁保证行不交错。
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// 开始一份新的时间线。**幂等**：同一个进程里第二次调用直接返回。
///
/// 落点是调用方给的路径。文件是**追加**的，每次开始先写一行分隔头
/// （`# drive letter trace: ... run=...`）——理由：盘符泄漏是「很久以后才发作」的
/// 问题，下一次准备就把上一次的证据覆盖掉，等于把证据销毁在用户报障之前。
/// 代价可控：不设环境变量时每次准备只有十几行。
pub(crate) fn start(log: PathBuf) {
    if SINK
        .set(Sink {
            log: log.clone(),
            start: Instant::now(),
        })
        .is_err()
    {
        return;
    }
    if let Some(parent) = log.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let interval = sample_interval_ms();
    let exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "?".to_string());
    let header = format!(
        "# drive letter trace: pid={} exe={} sample_ms={} started={}\n",
        std::process::id(),
        exe,
        interval
            .map(|ms| ms.to_string())
            .unwrap_or_else(|| "off".to_string()),
        chrono::Utc::now().to_rfc3339(),
    );
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log) {
        let _ = file.write_all(header.as_bytes());
        let _ = file.flush();
    }
    note(&match current_letters() {
        Some(letters) => format!("START now={}", spell(&letters)),
        None => "START letters-unknown".to_string(),
    });
    if let Some(ms) = interval
        && SAMPLER_STARTED.set(()).is_ok()
    {
        spawn_sampler(ms);
    }
}

/// 记一条有主的事件（`ASSIGN` / `REUSE` / `HOLD` / `RELEASE` / …）。
///
/// 时间线没开时是**静默空转**：调用点不必判断「现在有没有开」，
/// 也不会因为观测没开就改变任何行为。
pub(crate) fn note(line: &str) {
    let Some(sink) = SINK.get() else {
        return;
    };
    let elapsed = sink.start.elapsed().as_millis() as u64;
    append(sink, &format!("t+{elapsed}ms {line}"));
}

/// 收尾：写一行 `EXIT now=…`（进程退出前还剩哪些盘符），并停掉采样线程。
pub(crate) fn finish() {
    STOPPED.store(true, Ordering::Relaxed);
    if SINK.get().is_none() {
        return;
    }
    note(&match current_letters() {
        Some(letters) => format!("EXIT now={}", spell(&letters)),
        None => "EXIT letters-unknown".to_string(),
    });
}

fn append(sink: &Sink, line: &str) {
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&sink.log)
    {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

fn spell(letters: &[char]) -> String {
    letters.iter().collect()
}

/// 当前已挂载的盘符（只关心 `C..=Z`）。
///
/// `GetLogicalDrives()` 返回 0 表示**调用失败**，不是「一个盘符都没有」——
/// 这两种情形混在一起会让时间线凭空出现一条「所有盘符都消失了」的假事件。
/// 所以失败一律返回 `None`，由调用方记 `letters-unknown`。
fn current_letters() -> Option<Vec<char>> {
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return None;
    }
    Some(letters_in_mask(mask))
}

fn sample_interval_ms() -> Option<u64> {
    let raw = std::env::var(SAMPLE_ENV).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<u64>() {
        Ok(ms) => Some(ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS)),
        Err(_) => None,
    }
}

fn spawn_sampler(interval_ms: u64) {
    let spawned = std::thread::Builder::new()
        .name("letter-trace".to_string())
        .spawn(move || {
            let mut last = current_letters().unwrap_or_default();
            loop {
                std::thread::sleep(Duration::from_millis(interval_ms));
                if STOPPED.load(Ordering::Relaxed) {
                    break;
                }
                let Some(now) = current_letters() else {
                    continue;
                };
                let elapsed = SINK
                    .get()
                    .map(|sink| sink.start.elapsed().as_millis() as u64)
                    .unwrap_or_default();
                if let Some(line) = letter_delta_line(elapsed, &last, &now) {
                    if let Some(sink) = SINK.get() {
                        append(sink, &line);
                    }
                    last = now;
                }
            }
        });
    // 采样线程起不来只影响观测粒度，不该把准备流程打断。
    if spawned.is_err() {
        note("SAMPLER-UNAVAILABLE (thread spawn failed; only attributed events will be recorded)");
    }
}
