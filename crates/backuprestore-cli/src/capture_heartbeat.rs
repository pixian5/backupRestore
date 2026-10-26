//! 捕获心跳线程：让「DISM 到底在扫盘还是卡死了」在日志里可读。
//!
//! ## 为什么要它（2026-10-26 实测）
//!
//! DISM 捕获整卷时**先全卷预扫、再写数据**，预扫期间三层同时静默：
//!
//! 1. 镜像文件恒等于 WIM 头（实测 208 字节，一字节不涨）；
//! 2. `C:\Windows\Logs\DISM\dism.log` 停在 `Executing command line` 之后一行不写；
//! 3. 子进程 stdout 只有两行横幅。
//!
//! 实测捕获 `F:\Program Files`（8.7 GB / 32,732 文件）：07:33:21 起 100 秒恒为
//! 208 字节，之后 50 秒写到 3.4 GB。也就是说**开头零字节是正常的**，而不是卡死。
//! 但在此之前没有任何东西能说明这一点：一次 58 分钟零产出的观测，是靠外部脚本
//! 每 30 秒采样 `candidate.wim` 大小才看清的。产品侧必须自带这个观测。
//!
//! ## 设计边界
//!
//! - **只记录，不干预**：绝不杀进程、不改超时。预扫时长随文件数线性增长
//!   （构建卷这种海量小文件可以到几分钟），在这里杀进程会把正常备份也杀掉。
//!   超时仍由用户显式配置的 `dism_timeout` 逃生阀负责。
//! - **不动 `windows_command::run_line`**：那里是「挂起创建 → 入作业对象 → 恢复」
//!   的强一致序列，为加心跳去改它风险远大于收益。这里只是**旁路观测**：
//!   用 `std::fs::metadata` 读镜像大小，与子进程没有任何句柄关系。
//! - **判据行纯 ASCII、不走 i18n**（同 `RESTORE_SELFCHECK`）：`CAPTURE_HEARTBEAT`
//!   / `CAPTURE_STALL` / `CAPTURE_SUMMARY` 三个前缀就是验收脚本的接口。
//! - Drop 时补一行 `CAPTURE_SUMMARY` 总账，且 Drop 最多阻塞一个切片（250 ms），
//!   不会把捕获收尾拖住。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// 心跳间隔。选 30 秒是因为实测预扫量级在百秒级：更密只会把日志灌满，
/// 更疏就看不出「还在动」。
pub(crate) const INTERVAL: Duration = Duration::from_secs(30);

/// 多久没增长就追加一行 `CAPTURE_STALL`。10 分钟这个量级取自实测：
/// 8.7 GB 子目录预扫 100 秒，整卷是它的数倍到数十倍，10 分钟仍属「可能正常」。
pub(crate) const STALL_AFTER: Duration = Duration::from_secs(600);

/// 停止判断的切片长度：Drop 的等待上限就是它。
const SLICE: Duration = Duration::from_millis(250);

/// 捕获心跳句柄。Drop 即停止并补写总账。
pub(crate) struct Heartbeat {
    stop: Arc<AtomicBool>,
    samples: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
    image: PathBuf,
    log: PathBuf,
    started: Instant,
}

impl Heartbeat {
    /// 启动心跳。`image` 是候选镜像路径（可能还不存在 → 按 0 字节算），
    /// `log` 是任务日志路径，与 `online_operation::log` 同一套时间戳格式。
    pub(crate) fn start(image: PathBuf, log: PathBuf) -> Self {
        Self::start_with(image, log, INTERVAL, STALL_AFTER)
    }

    /// 显式给间隔与停滞阈值——单测用小间隔，生产用默认值。
    pub(crate) fn start_with(
        image: PathBuf,
        log: PathBuf,
        interval: Duration,
        stall_after: Duration,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(AtomicU64::new(0));
        let started = Instant::now();
        let stop_in = Arc::clone(&stop);
        let samples_in = Arc::clone(&samples);
        let image_in = image.clone();
        let log_in = log.clone();
        let handle = std::thread::Builder::new()
            .name("capture-heartbeat".into())
            .spawn(move || {
                let mut last_bytes = 0u64;
                let mut last_change = started;
                loop {
                    if !sleep_interruptible(&stop_in, interval) {
                        break;
                    }
                    // 读不到大小（文件还没创建 / 正在被 DISM 独占改名）按 0 算，
                    // 绝不能因为读失败就让心跳线程死掉。
                    let bytes = image_size(&image_in);
                    let now = Instant::now();
                    if bytes != last_bytes {
                        last_bytes = bytes;
                        last_change = now;
                    }
                    let elapsed = started.elapsed().as_secs();
                    let silent = now.duration_since(last_change).as_secs();
                    let line = if silent >= stall_after.as_secs() {
                        crate::text_parsing::capture_stall_line(elapsed, bytes, silent)
                    } else {
                        crate::text_parsing::capture_heartbeat_line(elapsed, bytes)
                    };
                    samples_in.fetch_add(1, Ordering::SeqCst);
                    // 写日志失败必须吞掉：心跳是旁路观测，不能反过来打断捕获。
                    let _ = crate::append_log(&log_in, &line);
                }
            })
            .ok();
        Self {
            stop,
            samples,
            handle,
            image,
            log,
            started,
        }
    }

    /// 已写出的心跳行数（不含总账）。验收脚本之外也可用于断言。
    ///
    /// **只在测试里用**：生产路径读的是 Drop 里的 `self.samples.load(...)` 本体，
    /// 所以不加 `#[cfg(test)]` 会让 Windows 发布构建报 `dead_code`（本机 `cargo test`
    /// 因 test cfg 成立而看不见这条告警——2026-10-26 VM 原生构建抓到）。
    #[cfg(test)]
    pub(crate) fn samples(&self) -> u64 {
        self.samples.load(Ordering::SeqCst)
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            // 线程每 SLICE 检查一次停止位，所以这里的等待上限是 SLICE，不是 interval。
            let _ = handle.join();
        }
        // 总账在**主线程**写：这样读到的是捕获真正结束时的镜像大小，
        // 而不是最后一次心跳时可能已经过时 30 秒的采样值。
        let line = crate::text_parsing::capture_summary_line(
            self.started.elapsed().as_secs(),
            image_size(&self.image),
            self.samples.load(Ordering::SeqCst),
        );
        let _ = crate::append_log(&self.log, &line);
    }
}

fn image_size(image: &Path) -> u64 {
    std::fs::metadata(image).map(|m| m.len()).unwrap_or(0)
}

/// 睡 `total`，但每 `SLICE` 检查一次停止位；返回 false 表示该停了。
fn sleep_interruptible(stop: &AtomicBool, total: Duration) -> bool {
    let deadline = Instant::now() + total;
    loop {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        std::thread::sleep(SLICE.min(deadline - now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "backuprestore-heartbeat-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn heartbeat_records_heartbeats_then_a_summary() {
        let dir = temp_root("basic");
        let image = dir.join("candidate.wim");
        let log = dir.join("Online.log");
        fs::write(&image, vec![0u8; 208]).expect("seed header");

        {
            let beat = Heartbeat::start_with(
                image.clone(),
                log.clone(),
                Duration::from_millis(40),
                Duration::from_secs(600),
            );
            // 等到至少写出两行心跳，最多等 2 秒。
            let deadline = Instant::now() + Duration::from_secs(2);
            while beat.samples() < 2 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(beat.samples() >= 2, "expected at least two beats");
        }

        let text = fs::read_to_string(&log).expect("log readable");
        assert!(
            text.contains("CAPTURE_HEARTBEAT elapsed_secs="),
            "missing heartbeat: {text}"
        );
        assert!(
            text.contains("phase=pre_scan"),
            "208-byte image must be pre_scan: {text}"
        );
        // 总账是 Drop 写的：有它才说明收尾没丢。
        assert!(
            text.contains("CAPTURE_SUMMARY elapsed_secs="),
            "missing summary: {text}"
        );
        assert!(text.is_ascii(), "heartbeat log must stay ASCII: {text}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn heartbeat_turns_into_a_stall_line_when_the_image_stops_growing() {
        let dir = temp_root("stall");
        let image = dir.join("candidate.wim");
        let log = dir.join("Online.log");
        fs::write(&image, vec![0u8; 208]).expect("seed header");

        {
            // 阈值 0 秒 = 第一拍就判定「没动静」，用来钉住分支选择。
            let beat = Heartbeat::start_with(
                image.clone(),
                log.clone(),
                Duration::from_millis(40),
                Duration::from_secs(0),
            );
            let deadline = Instant::now() + Duration::from_secs(2);
            while beat.samples() < 1 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(beat.samples() >= 1, "expected at least one beat");
        }

        let text = fs::read_to_string(&log).expect("log readable");
        assert!(
            text.contains("CAPTURE_STALL elapsed_secs="),
            "expected a stall line: {text}"
        );
        assert!(
            text.contains("note=dism_scans_whole_volume_before_first_write"),
            "stall line must explain itself: {text}"
        );
        // 停滞行绝不能带「已失败」的结论：预扫慢不等于是失败。
        assert!(!text.contains("failed"), "{text}");
        let _ = fs::remove_dir_all(&dir);
    }
}
