//! DISM 等待上限：把「无限等待」变成**可配置**，但默认仍是无限。
//!
//! ## 为什么需要它（2026-10-16 审查，docs/20261016-2250 卡死 2/3/6）
//!
//! 全仓 `WaitForSingleObject(..., INFINITE)` 共 7 处，**全部用于等 DISM 退出**
//! （`pe_operation.rs`×4、`online_operation.rs`×2、`native_gui.rs`×1）。
//! 这三处的共同症状是：
//!
//! - PE 桌面：`EnableWindow(hwnd, 0)` → `pe_operation::execute()` → `EnableWindow(hwnd, 1)`。
//!   DISM 挂死则恢复语句永不到达，**PE 桌面窗口永久禁用**（不能点也不能关）。
//! - 主 GUI：安装 PE 从 `WM_COMMAND` **同步**等 DISM，消息循环被阻塞，
//!   整窗「未响应」、无进度、无取消。
//! - 在线操作：工作线程永久等待，「创建任务」按钮与状态永远停在「执行中」。
//!
//! DISM 是唯一可能长时间挂住的外部依赖（等 I/O 挂起、镜像被独占、卷正忙），
//! 所以这是**最大的单一风险面**：给它加超时能同时缓解三个高严重度卡死点。
//!
//! ## 为什么默认仍是「不限制」
//!
//! 合法的大镜像还原就是慢：58 GB 镜像的 `/Apply-Image` 在机械盘上跑几十分钟
//! 是正常的。若把默认值设成某个固定秒数，**超时会杀掉一个本来能成功的还原**——
//! 把一个"永久挂死"的 bug 换成一个"误杀合法任务"的 bug，更糟。
//!
//! 因此本模块只提供**逃生阀**：默认 0 = 不限制（= 历史行为，零回归），
//! 用户/运维显式配置正整数秒后才生效。超时的语义不是"放弃任务"，而是
//! "结束这次 DISM、保留现场、报错"——现场保留意味着可以重跑。
//!
//! ## 三个入口（与 `failure_policy` 同构）
//!
//! ① 进程环境变量 `BACKUPRESTORE_DISM_TIMEOUT_SECS`
//! ② 任务级 `RecoveryTask.env` 的 `DISM_TIMEOUT_SECS`（恢复环境无人值守时用它）
//! ③ 程序设置 `BackupRestore.settings.ini` 的 `dism_timeout_secs`
//!
//! 模块**不依赖 Windows**（纯解析 + 进程环境变量），本机 macOS 即可单测全覆盖。
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::pe_safety::MessageLanguage;

/// 进程环境变量名。
pub(crate) const ENV_VAR: &str = "BACKUPRESTORE_DISM_TIMEOUT_SECS";
/// 落进 `RecoveryTask.env` 的键名（恢复环境的内部契约）。
pub(crate) const ENV_FILE_KEY: &str = "DISM_TIMEOUT_SECS";
/// 设置文件里的键名（给用户看的小写形式）。
pub(crate) const SETTINGS_KEY: &str = "dism_timeout_secs";
/// 0 = 不限制（= INFINITE，历史行为）。
pub(crate) const DEFAULT_SECS: u32 = 0;
/// 上限 24 小时：再往上等于关闭该保护，不如明确拒绝。
pub(crate) const MAX_SECS: u32 = 24 * 60 * 60;
/// `windows_command::INFINITE`（`u32::MAX`）。本模块跨平台，不能引用那个常量，
/// 语义完全一致，就在这里重复定义一次并注明来源。
const WAIT_MS_UNLIMITED: u32 = u32::MAX;

/// 解析秒数文本。`Ok(0)` = 不限制；`Ok(n)` = n 秒；`Err` = 无法识别。
///
/// 只接受纯 ASCII 数字：`+30`、`30.0`、全角「３」一律拒绝——静默回退默认
/// 会让用户以为设置生效了，是比报错更坏的结果（与 `failure_policy` 同一条理由）。
pub(crate) fn parse_secs(text: &str) -> Result<u32, String> {
    let trimmed = text.trim();
    if !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(format!("必须是 0 到 {MAX_SECS} 之间的整数（秒）"));
    }
    let value: u32 = trimmed
        .parse()
        .map_err(|_| format!("必须是 0 到 {MAX_SECS} 之间的整数（秒）"))?;
    if value > MAX_SECS {
        return Err(format!("最大 {MAX_SECS} 秒（24 小时）"));
    }
    Ok(value)
}

/// 秒数 → `WaitForSingleObject` 的毫秒参数。0 = 无限等待。
pub(crate) fn wait_ms_from_secs(secs: u32) -> u32 {
    if secs == 0 {
        WAIT_MS_UNLIMITED
    } else {
        // 秒→毫秒可能溢出（MAX_SECS=86400 → 86_400_000，远在 u32 内），
        // 仍用饱和乘法兜底。
        secs.saturating_mul(1000).max(1)
    }
}

/// 读进程环境变量，返回本次 DISM 等待应使用的毫秒数。
///
/// 缺失 / 非法值 → `WAIT_MS_UNLIMITED`（绝不因为一个坏值让恢复环境起不来，
/// 与 `failure_policy::from_values` 的容错口径一致）。
pub(crate) fn wait_ms() -> u32 {
    match std::env::var(ENV_VAR)
        .ok()
        .as_deref()
        .and_then(|value| parse_secs(value).ok())
    {
        Some(secs) => wait_ms_from_secs(secs),
        None => WAIT_MS_UNLIMITED,
    }
}

/// 从 `RecoveryTask.env` 的键值表读秒数（0 = 不限制）。
pub(crate) fn from_values(values: &BTreeMap<String, String>) -> u32 {
    values
        .get(ENV_FILE_KEY)
        .and_then(|value| parse_secs(value).ok())
        .unwrap_or(DEFAULT_SECS)
}

/// 落进 `RecoveryTask.env` 的一行（键, 值）。只在该值**不是默认**时才写，
/// 避免给每个任务都多一行没有信息量的 0。
pub(crate) fn env_pair(secs: u32) -> Option<(&'static str, String)> {
    (secs != DEFAULT_SECS).then(|| (ENV_FILE_KEY, secs.to_string()))
}

/// 从设置文件文本里读秒数。不认识的行/键一律忽略。
pub(crate) fn from_settings_text(text: &str) -> u32 {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == SETTINGS_KEY {
            // 【2026-10-18 缺陷修复 L11】走与用户输入**同一套**严格口径
            // （`parse_secs`：只认纯数字 + `MAX_SECS` 上限）。此前这里
            // `parse::<u32>().unwrap_or(DEFAULT)`，既接受 `999999999`
            // 这种超出上限的值，也和界面输入的校验规则不一致。
            return parse_secs(value).unwrap_or(DEFAULT_SECS);
        }
    }
    DEFAULT_SECS
}

/// 设置文件里的一行（0 也写，让用户能明确看到"当前是不限制"）。
pub(crate) fn settings_line(secs: u32) -> String {
    format!("{SETTINGS_KEY}={secs}\n")
}

/// 读设置文件里的秒数（缺文件 / 读失败 → 默认 0）。
///
/// 设置文件由 `failure_policy` 拥有（同名 `BackupRestore.settings.ini`），
/// 这里只追加自己那一行、不动别人写的键——两处各自写会互相覆盖。
pub(crate) fn load_from_dir(executable_dir: &Path) -> u32 {
    let path = crate::failure_policy::FailurePolicy::settings_path(executable_dir);
    match fs::read_to_string(&path) {
        Ok(text) => from_settings_text(&text),
        // 文件不存在是常态（用户从没打开过设置窗口）。
        Err(_) => DEFAULT_SECS,
    }
}

/// 【2026-10-17 修复】进程启动时把设置文件里的秒数灌进进程环境变量。
///
/// ## 修的是什么
///
/// `wait_ms()` 只读**进程环境变量**。而给这个变量赋值的唯一位置是
/// `recover_env`（恢复环境入口）——于是三处消费方里有两处**永远读不到**：
///
/// - `online_operation::dism`（**在线还原**）：跑在普通 Windows 里的 CLI/GUI 进程，
///   从不经过 `recover-env`。
/// - `native_gui` 的「安装 PE」（`dism /Apply-Image`）：GUI 进程，同上。
///
/// 也就是说：用户在设置窗口里配了 `dism_timeout_secs=300`，对这两条路**完全不生效**，
/// `wait_ms()` 恒为 `INFINITE`。模块头部却把「程序设置 ini」列为三个入口之一 ——
/// 文档与行为不符，且失效是**静默**的（没有报错、没有日志）。
///
/// ## 修法
///
/// 在 `main()` 最早处（任何子命令分派与子进程派生之前）调一次
/// `init_process_env_from_settings()`：环境变量**未被显式设置**时，用 ini 的值补上。
/// 已经设了的（命令行 / 父进程 / `recover-env` 的任务级值）**一律不覆盖**——
/// 显式优先于文件，否则 `recover_env` 的任务级值会被 ini 悄悄改掉。
// 调用点在 `main()`，那里是 `#[cfg(windows)]`（模块本身在非 Windows 非测试
// 构建里被裁掉）；非 Windows 构建下允许未使用，免得 `dead_code` 噪音。
// 行为由本文件 `plan_env_value` 的单测在 macOS 上照常锁定。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn init_process_env_from_settings() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    if let Some(secs) = plan_env_value(std::env::var(ENV_VAR).is_ok(), load_from_dir(dir)) {
        // set_var 在 2024 edition 里是 unsafe；此刻在单线程入口，且在任何
        // 子进程派生之前。
        unsafe { std::env::set_var(ENV_VAR, secs.to_string()) };
    }
}

/// 上一条的**纯判据**：给定「环境变量是否已显式设置」与「ini 里的秒数」，
/// 决定要不要灌入（`Some(secs)`）以及灌多少。
///
/// 抽成纯函数是为了单测能锁定"显式优先 / 0 不写"两条规则，且**不碰进程环境**
/// —— 直接改环境变量的测试在 `cargo test` 的并行调度下会互相干扰（本文件既有
/// 的 `wait_ms` 测试已经动过它）。
pub(crate) fn plan_env_value(env_already_set: bool, settings_secs: u32) -> Option<u32> {
    if env_already_set {
        // 显式设置优先：命令行 / 父进程 / recover-env 的任务级值不能被 ini 覆盖。
        return None;
    }
    // 0 不写：让 `wait_ms()` 的"读不到 = 不限制"保持单一语义，
    // 也避免给每个进程都多设一个没有信息量的 0。
    (settings_secs != DEFAULT_SECS).then_some(settings_secs)
}

/// 把秒数**合并写进**设置文件：只替换自己那一行，其余键原样保留。
///
/// 为什么不能像 `failure_policy::save_to_dir` 那样整文件重写：那两个键和这个键
/// 由不同入口写（失败策略来自设置窗口的保存按钮，DISM 上限来自同一个窗口的
/// 另一个文本框）。整文件重写任何一处都会把另一处冲掉，而这种丢失是**静默**的。
pub(crate) fn save_to_dir(secs: u32, executable_dir: &Path) -> std::io::Result<()> {
    let path = crate::failure_policy::FailurePolicy::settings_path(executable_dir);
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut kept: Vec<&str> = existing
        .lines()
        .filter(|line| line.trim().split_once('=').map(|(key, _)| key.trim()) != Some(SETTINGS_KEY))
        .collect();
    // 首次写入时补一行说明，让手工编辑的用户知道这行是什么。
    if kept.is_empty() {
        kept.push("# BackupRestore 程序设置（本文件由程序自动生成，可手工编辑）");
    }
    // 先取出整行再 trim，避免"临时 String 的借用随语句结束而失效"。
    let line = settings_line(secs);
    kept.push(line.trim_end());
    let mut text = kept.join("\n");
    text.push('\n');
    fs::write(&path, text)
}

/// 设置窗口里的说明文字（解释 0 的含义与风险）。
pub(crate) fn hint(language: MessageLanguage) -> &'static str {
    match language {
        MessageLanguage::Chinese => {
            "0 = 不限制（默认，与旧版一致）。大镜像还原本来就可能跑几十分钟，设小了会杀掉本来能成功的任务。仅在 DISM 挂死且需要程序自行脱身时设置。"
        }
        MessageLanguage::English => {
            "0 = unlimited (default, same as before). Restoring a large image can legitimately take tens of minutes; a small value would kill a task that would otherwise succeed. Set it only when DISM hangs and the program should free itself."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn zero_means_unlimited_and_matches_history() {
        // 默认 0 → INFINITE：**不得**因为本模块的引入改变既有行为。
        assert_eq!(DEFAULT_SECS, 0);
        assert_eq!(wait_ms_from_secs(0), WAIT_MS_UNLIMITED);
        assert_eq!(wait_ms(), WAIT_MS_UNLIMITED);
    }

    #[test]
    fn seconds_convert_to_milliseconds() {
        assert_eq!(wait_ms_from_secs(1), 1_000);
        assert_eq!(wait_ms_from_secs(60), 60_000);
        assert_eq!(wait_ms_from_secs(MAX_SECS), MAX_SECS * 1_000);
    }

    #[test]
    fn parser_rejects_junk_instead_of_guessing() {
        assert_eq!(parse_secs(" 30 "), Ok(30));
        assert_eq!(parse_secs("0"), Ok(0));
        assert_eq!(parse_secs(&MAX_SECS.to_string()), Ok(MAX_SECS));
        // 空串、正号、小数、全角数字、超范围、负数一律拒绝。
        for junk in ["", "   ", "+30", "30.0", "３０", "86401", "-1", "abc", "30s"] {
            assert!(parse_secs(junk).is_err(), "{junk:?} should be rejected");
        }
    }

    #[test]
    fn env_file_key_is_read_tolerantly() {
        assert_eq!(from_values(&values(&[])), DEFAULT_SECS);
        assert_eq!(from_values(&values(&[(ENV_FILE_KEY, "120")])), 120);
        assert_eq!(from_values(&values(&[(ENV_FILE_KEY, "0")])), 0);
        // 非法值 → 不限制（恢复环境必须能起来）
        assert_eq!(from_values(&values(&[(ENV_FILE_KEY, "nope")])), DEFAULT_SECS);
        assert_eq!(from_values(&values(&[(ENV_FILE_KEY, "99999")])), DEFAULT_SECS);
    }

    #[test]
    fn env_pair_is_written_only_when_not_default() {
        assert_eq!(env_pair(0), None);
        assert_eq!(
            env_pair(90),
            Some((ENV_FILE_KEY, "90".to_string()))
        );
    }

    #[test]
    fn settings_round_trip() {
        assert_eq!(from_settings_text(&settings_line(0)), 0);
        assert_eq!(from_settings_text(&settings_line(600)), 600);
        // 注释与未知键必须被忽略，不能污染解析。
        let text = "# comment\nunknown_key=7\n; another\ndism_timeout_secs=45\n";
        assert_eq!(from_settings_text(text), 45);
        // 缺行 → 默认
        assert_eq!(from_settings_text("failure_threshold=3\n"), DEFAULT_SECS);
        // 坏值 → 默认（设置坏了不能让 GUI 起不来）
        assert_eq!(from_settings_text("dism_timeout_secs=abc\n"), DEFAULT_SECS);
    }

    #[test]
    fn save_to_dir_merges_instead_of_clobbering_the_other_keys() {
        // 失败策略与 DISM 上限由不同入口写进**同一个**文件。任何一处整文件
        // 重写都会把另一处静默冲掉——这条测试钉住"合并写"。
        let dir = std::env::temp_dir().join(format!(
            "backuprestore-dism-timeout-merge-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        let path = crate::failure_policy::FailurePolicy::settings_path(&dir);

        // 先让 failure_policy 写它的两个键。
        let policy = crate::failure_policy::FailurePolicy::new(7, crate::failure_policy::FailureAction::PauseInRecovery)
            .expect("valid");
        policy.save_to_dir(&dir).expect("policy save");
        assert_eq!(
            crate::failure_policy::FailurePolicy::load_from_dir(&dir).threshold,
            7
        );

        // 再写 DISM 上限：failure_policy 的键必须原样保留。
        save_to_dir(300, &dir).expect("dism save");
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains("failure_threshold=7"), "{text}");
        assert!(text.contains("failure_action=pause"), "{text}");
        assert!(text.contains("dism_timeout_secs=300"), "{text}");
        assert_eq!(from_settings_text(&text), 300);
        assert_eq!(
            crate::failure_policy::FailurePolicy::load_from_dir(&dir),
            policy
        );

        // 反向：failure_policy 再存一次，DISM 那一行必须还在。
        policy.save_to_dir(&dir).expect("policy save again");
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains("dism_timeout_secs=300"), "{text}");
        assert_eq!(from_settings_text(&text), 300);

        // 改值只替换自己那一行，不会留下两份。
        save_to_dir(0, &dir).expect("dism save");
        let text = fs::read_to_string(&path).expect("read");
        assert_eq!(text.matches("dism_timeout_secs=").count(), 1, "{text}");
        assert_eq!(from_settings_text(&text), 0);

        // load_from_dir 覆盖"文件不存在 → 默认"。
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(load_from_dir(&dir), DEFAULT_SECS);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_var_wins_over_everything_else_in_the_same_process() {
        // 进程环境变量是最终裁判：`recover_env` 把任务级的值写进它。
        unsafe { std::env::set_var(ENV_VAR, "17") };
        assert_eq!(wait_ms(), 17_000);
        unsafe { std::env::set_var(ENV_VAR, "0") };
        assert_eq!(wait_ms(), WAIT_MS_UNLIMITED);
        unsafe { std::env::set_var(ENV_VAR, "garbage") };
        assert_eq!(wait_ms(), WAIT_MS_UNLIMITED);
        unsafe { std::env::remove_var(ENV_VAR) };
        assert_eq!(wait_ms(), WAIT_MS_UNLIMITED);
    }

    #[test]
    fn plan_env_value_keeps_explicit_settings_and_skips_zero() {
        // 【2026-10-17 修复】这两条规则是 Bug 1 的核心，必须钉死：
        // ① 已显式设置 → 绝不覆盖（否则 recover-env 的任务级值会被 ini 改掉）；
        // ② ini 为 0（默认）→ 不写，保持"读不到 = 不限制"的单一语义。
        assert_eq!(plan_env_value(true, 300), None);
        assert_eq!(plan_env_value(true, 0), None);
        assert_eq!(plan_env_value(false, 0), None);
        // 未显式设置且 ini 有值 → 灌入，让在线还原 / GUI 安装 PE 也能读到。
        assert_eq!(plan_env_value(false, 300), Some(300));
        assert_eq!(plan_env_value(false, MAX_SECS), Some(MAX_SECS));
    }

    #[test]
    fn init_from_settings_survives_a_missing_settings_file() {
        // 设置文件不存在是常态（用户从没打开过设置窗口）：init 必须静默无操作，
        // 不能让程序起不来。用一个注定没有 ini 的临时目录验证纯读取那一步。
        let dir = std::env::temp_dir().join(format!(
            "backuprestore-dism-timeout-nosettings-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        assert_eq!(load_from_dir(&dir), DEFAULT_SECS);
        assert_eq!(plan_env_value(false, load_from_dir(&dir)), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_are_distinct_so_a_wrong_key_cannot_silently_win() {
        // 与 `failure_policy` 同一前科：键名不同 → 永远读不到值 → 静默回退默认。
        assert_ne!(SETTINGS_KEY, ENV_FILE_KEY);
        assert_ne!(ENV_FILE_KEY, ENV_VAR);
        assert_ne!(SETTINGS_KEY, ENV_VAR);
    }

    #[test]
    fn hint_is_present_for_both_languages_and_free_of_markup() {
        for language in [
            crate::pe_safety::MessageLanguage::Chinese,
            crate::pe_safety::MessageLanguage::English,
        ] {
            let text = hint(language);
            assert!(!text.is_empty());
            // Win32 的 STATIC 不渲染 Markdown（P3 前科：`**粗体**` 会原样显示成星号）。
            for marker in ["**", "__", "`", "##"] {
                assert!(!text.contains(marker), "hint must not contain {marker:?}: {text}");
            }
        }
    }
}
