#![cfg_attr(windows, windows_subsystem = "windows")]

//! Small Windows front-end/recovery host entry point.
//!
//! The GUI can call this binary for task validation and execution.  Keeping
//! the recovery path as a command-line program also makes it usable from
//! WinRE's `winpeshl.ini` without depending on a desktop runtime.

use backuprestore_core::{Stage, StatusRecord, Task, TaskError, TaskStore, read_json, sha256_file};
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(any(windows, test))]
mod boot_cleanup;
#[cfg(windows)]
mod dpi;
#[cfg(windows)]
mod boot_entry;
/// DISM 等待上限：把「无限等待」变成可配置，**默认仍是无限**（零回归）。
/// 跨平台纯逻辑，本机单测即可覆盖全部分支。
/// 全局语言层：让日志与 CLI 结论文案也能按语言输出（界面之外的地方）。
/// ★ 2026-10-25：**无条件编译**。`text_parsing`（无条件）要用它的判据文案做
/// 双语终态识别；一旦门控成 `any(windows, test)`，macOS 上不带 `test` 的
/// `cargo check` 就 E0433（而 `cargo test` 因为带 `test` cfg 反而能过，
/// v2.6.41 只跑单测所以没发现）。`MessageLanguage` 也随之从 `pe_safety`
/// 搬到本模块（`pe_safety` 重导出，旧路径不变）。
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
mod i18n;
/// 错误码与错误文案的单一来源（`BR-<域>-<序号>`）。
/// 无条件编译：`pe_safety` / `boot_entry` 等被无条件引用，它们都要用 `errc`。
/// 非 Windows 构建下没人调 `errc`，允许 dead_code。
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
mod error_code;
// 非 Windows 构建下这批调用点全在 cfg 里，re-export 会报未使用。
#[allow(unused_imports)]
pub(crate) use error_code::{ErrCode, errc, errc_with};
/// CLI 结构化输出（`--json`）：让自动化判据不再解析中文文案，只认字段。
/// 纯逻辑 + 本机单测，故与 `pe_safety` 同门控。
#[cfg(any(windows, test))]
mod cli_json;
/// 捕获心跳：把「DISM 在扫盘还是卡死」变成日志里可读的状态。
/// **跨平台**（纯 std 线程 + 文件读），本机单测即可覆盖三分支（心跳/停滞/总账）。
mod capture_heartbeat;
#[cfg(any(windows, test))]
mod dism_timeout;
/// 恢复环境「连续失败 N 次后如何处理」策略：判据、文案与设置的单一来源。
/// **跨平台**（纯逻辑 + 文件读写），本机单测即可覆盖全部分支。
#[cfg(any(windows, test))]
mod failure_policy;
/// 还原末期的引导修复：候选枚举与方案编辑是**跨平台**纯逻辑（有本机单测），
/// 对话框与实际 bcdboot/bcdedit 执行在 Windows 侧。
#[cfg(any(windows, test))]
mod boot_menu;
/// 半成品启动项簿记：**跨平台**（`boot_entry` 是 Windows-only，其单测在 macOS 上
/// 一次都不编译）。这里放纯数据与文件 I/O，让簿记语义有本机单测兜底。
#[cfg(any(windows, test))]
mod boot_record;
// 文案表不依赖任何 Win32 句柄，**无条件编译**——这样 mac 上也能单测它
// （2026-10-26 拆分的目的之一）。`native_gui` 仍然是 Windows 专属。
mod gui_text;
/// 失败回执的错误码可见性：把 `error_code` 拼进界面文字（幂等）。
/// **无条件编译**——`Io`/`Json` 这类失败的码只存在字段里，界面必须能看到；
/// 纯字符串逻辑，本机单测即可覆盖全部规则。
mod failure_receipt;
/// 「最近任务状态」回执：解析 `status.json` 并把阶段 / 进度 / 错误码 / 失败原因
/// 变成界面行。**无条件编译**（纯 JSON 与字符串逻辑），本机单测覆盖全部分支。
mod status_receipt;
/// 任务与证据管理：把 `<运行目录>\tasks` 读成界面行（排序、坏记录表述、详情文本）。
/// **无条件编译**（纯逻辑，不碰 Win32），因此 mac 上能单测全部分支。
mod task_admin;
#[cfg(windows)]
mod native_gui;
/// 在线任务本轮工作目录的选址：统一日志根（`log_root`）优先、`%TEMP%` 兜底。
/// 纯路径决策，本机单测即可覆盖全部分支。
#[cfg(any(windows, test))]
mod online_log;
#[cfg(windows)]
mod online_operation;
/// 备份文件管理（P1-c）：纯逻辑（钳制）跨平台可测；DISM 相关的列举/删除
/// 单独 `#[cfg(windows)]`（与 `online_operation` 同门控）。
#[cfg(any(windows, test))]
mod image_admin;
#[cfg(windows)]
mod pe_operation;
#[cfg(any(windows, test))]
mod pe_safety;
#[cfg(windows)]
mod recovery_fault;
#[cfg(windows)]
mod recovery_progress;
#[cfg(any(windows, test))]
mod resume_safety;
#[cfg(any(windows, test))]
mod secondary_registry;
/// 会话日志落点：主路径 + **跨重启存活的镜像**（PE/RE 的 `X:` 是内存盘）。
/// 无条件编译：`append_log` 已委托给它，本机（macOS）构建也要能过。
mod session_log;
/// 日志根目录的统一解析（用户 2026-10-18：不放 PE、不放目标分区）。
/// 无条件编译：`append_log` / panic hook / launcher 都走它，本机构建也要能过。
mod log_root;
/// 设置文件名的单一来源。无条件编译：无条件模块 `i18n` 也要读该文件，
/// 常量若留在 `cfg(any(windows, test))` 的 `failure_policy` 里会 E0433。
mod settings_file;
#[cfg_attr(not(windows), allow(dead_code))]
mod text_parsing;
/// 进度窗的纯文本逻辑（时间格式化 / 剩余时间估算 / 4 阶段骨架）。
/// **无条件编译**：`recovery_progress` 整份是 `#[cfg(windows)]`，逻辑留在那里
/// 写多少单测都不会在本机跑；剥到这里才能被 `cargo test` 真正执行。
#[cfg_attr(not(windows), allow(dead_code))]
mod progress_text;
#[cfg(windows)]
mod windows_command;
/// 盘符时间线（谁挂的、什么时候、有没有被收回）。纯判据在 `text_parsing`，
/// 本模块只做采样与落盘，因此与 `windows_prepare` 同门控。
#[cfg(windows)]
mod letter_trace;
#[cfg(any(windows, test))]
pub(crate) mod prepare_plan;
#[cfg(windows)]
mod windows_prepare;
/// wimlib 覆盖式应用通道（任务 #110，2026-10-22 用户裁定「换 wimlib」；
/// 2026-10-23 产品化「保留安全描述符还原」）。
/// 纯逻辑（参数构造 / 二进制定位）跨平台可测；实际执行走 `windows_command`，
/// 因此执行函数单独 `#[cfg(windows)]`，与 `windows_command` 同门控。
#[cfg(any(windows, test))]
mod wimlib_apply;
#[cfg(any(windows, test))]
mod winre_payload;
#[cfg(any(windows, test))]
mod winre_registration;
mod process_runner;
pub(crate) use process_runner::*;
#[cfg(any(windows, test))]
mod recovery_bcd;
#[cfg(any(windows, test))]
pub(crate) use recovery_bcd::*;
#[cfg(any(windows, test))]
mod recovery_volume;
#[cfg(any(windows, test))]
pub(crate) use recovery_volume::*;
mod recovery_engine;
pub(crate) use recovery_engine::*;
mod pending_task;
#[allow(unused_imports)]
pub(crate) use pending_task::*;

fn usage() -> ! {
    eprintln!(
        "BackupRestore commands:\n  validate-task <task.json>\n  hash <file>\n  status <task-root> <task-id>\n  prepare --operation <probe|backup|restore-existing|create-secondary> --source-drive <letter> [--target-drive <letter>] [--image-path <absolute-wim>] [--wim-index <n>] [--boot-menu-name <name>] [--allow-destructive] [--no-reboot] [--skip-format] [--fail-threshold <1-20>] [--fail-action <auto-escape|pause>] [--ui-language <zh|en>]\n  prepare ... [--test-efi-drive <letter>] [--test-fault <identity-env-mismatch|bcdboot-failure|power-loss-window>]  (development test only)
  online --operation <backup|restore-existing|create-secondary> --source-drive <letter> [--target-drive <letter>] [--image-path <absolute-wim>] [--index <n>] [--compress <fast|none> (default fast)] [--image-name <name>] [--keep-indexes <n>] [--format-target] [--ui-language <zh|en>]\n  list-volumes\n  inspect-environment\n  wim-info <absolute-wim> [--skip-hash]\n  recover <task-root> <task-id> [--dry-run] [--efi-root <mounted EFI root>]\n  resume <task-root> <task-id>  (retry existing recovery or cleanup)\n  recover-env <RecoveryTask.env>
  list-images [dir]  (default H:\\BRImages; list WIMs and their indexes)
  delete-old-indexes <absolute-wim> --keep <n>  (destructive: delete the oldest indexes, keep the latest n)\n  run-command <program> [args...]\n  --json  (print the verdict as ONE line of JSON: command/operation/success/log_path/error; for automation)\n  --open-image <absolute-wim>  (GUI only)\n  --tab <0..=4>  (GUI only: 0=backup 1=restore-A to C: 2=restore-B to non-C: 3=second system 4=install PE)\n  --pe-desktop  (WinPE recovery desktop, GUI only)\n"
    );
    std::process::exit(2)
}

/// 关掉「设备未就绪 / 无法访问」这类**系统**弹窗（2026-10-27）。
///
/// ★ 事故：机器上一个残留盘符 `V:`（`HKLM\SYSTEM\MountedDevices` 里的
/// `\DosDevices\V:`，指向的分区早已删除）让 Windows 在任何人碰到 `V:` 时弹出
/// 「V:\\ 无法访问。设备未就绪。」。只要进程不设 `SEM_FAILCRITICALERRORS`，
/// 一次 `GetFileAttributesW("V:\\")` 就足以弹一次框——弹的是谁的框，用户只看到
/// 「又是这个程序」。
///
/// 这里设的是**进程**错误模式，之后派生的线程与子进程都继承，因此全进程（含
/// GUI）从此只拿到错误码、不再弹框。
///
/// 不设 `SEM_NOGPFAULTERRORBOX` / `SEM_NOALIGNMENTFAULTEXCEPT`：那两个是关掉
/// **崩溃报告**，会把真正的程序缺陷藏起来。本函数只关「设备/文件不在」这类
/// 环境噪音，崩溃照样要报。
#[cfg(windows)]
fn suppress_system_error_dialogs() {
    const SEM_FAILCRITICALERRORS: u32 = 0x0001;
    const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;
    unsafe {
        unsafe extern "system" {
            fn GetErrorMode() -> u32;
            fn SetErrorMode(mode: u32) -> u32;
        }
        let current = GetErrorMode();
        SetErrorMode(current | SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX);
    }
}

fn main() {
    // 必须最先做：任何窗口创建之前声明 DPI 感知，否则 200% 缩放下界面被
    // 系统位图放大、底部控件落到窗口外（2026-10-30 实测）。
    #[cfg(windows)]
    crate::dpi::init();
    // 必须最先做：之后任何一次落盘/探测都不该再弹系统框。
    #[cfg(windows)]
    suppress_system_error_dialogs();
    // 必须最先做：`session_log::append` 之后的每一次落盘都要知道该不该镜像。
    // PE / WinRE 里主日志落在内存盘 `X:` 上，重启即丢（2026-10-17 第二轮核查
    // 日志缺口 1/2），镜像是唯一能跨重启活下来的那一份。
    #[cfg(windows)]
    crate::session_log::init();
    // 崩溃兜底：清理「上次进程被强杀/崩溃、没来得及撤销」的临时盘符。
    // 只清「登记还在、但登记它的 PID 已经不在了」的——并存的另一个实例（GUI 会派生
    // CLI 去准备）绝不会被误伤。判活走 OpenProcess，不猜时间戳。
    #[cfg(windows)]
    crate::windows_prepare::release_dead_owner_leases(
        &crate::log_root::logs_dir().join("mount-leases.log"),
    );
    #[cfg(windows)]
    install_panic_logging();
    // 【2026-10-17 修复】在任何子命令分派与子进程派生之前，把设置文件里的
    // DISM 等待上限灌进进程环境变量。缺这一步时，`wait_ms()` 只在
    // `recover-env`（恢复环境）里被赋过值，在线还原与 GUI「安装 PE」两条路
    // 恒为不限制 —— 用户在设置里配的上限静默失效。
    // 模块是 `cfg(any(windows, test))`：macOS 的非测试构建里没有它，这里必须
    // 同样门控，否则本机 `cargo build` 会报找不到模块（DISM 也只存在于 Windows）。
    #[cfg(windows)]
    crate::dism_timeout::init_process_env_from_settings();
    // macOS 编译时 `arguments` 不被修改（`retain` 在 `#[cfg(windows)]` 块内），
    // 门控 `mut` 避免 `unused_mut` 告警而不引入 `#[allow]`。
    #[cfg(windows)]
    let mut arguments: Vec<String> = env::args().skip(1).collect();
    #[cfg(not(windows))]
    let arguments: Vec<String> = env::args().skip(1).collect();
    // `--json`：命令把结论以单行 JSON 打印（command/operation/success/
    // log_path/error），让自动化判据只认字段、不再解析中文文案。
    // ★ 2026-10-25 订正：**必须真的从 arguments 摘除**。旧注释说「子命令解析
    // 循环遇到未知 `--json` 走 `_ => {}`，无需摘除」——那只对 `online` 成立；
    // `prepare` 是严格解析（曾因此直接拒绝 `--json`），而 `recover` /
    // `abandon-task` 这些按**位置**取参数的命令会把 `--json` 当成第一个业务
    // 参数（实机撞出：`recover C:\x --json` 报「任务标识无效：--json」）。
    // 在分发前统一摘掉，所有命令一劳永逸。
    #[cfg(windows)]
    {
        if arguments.iter().any(|a| a == "--json") {
            crate::cli_json::enable();
            arguments.retain(|a| a != "--json");
        }
        // 语言：`BACKUPRESTORE_LANG=en*` 让日志与 CLI 结论文案走英文。
        // 放在最前，后面 RE 侧读到 UI_LANGUAGE、GUI 切语言时会再覆盖一次。
        crate::i18n::init_from_env();
    }
    if arguments.is_empty() && should_launch_gui() {
        if let Err(error) = launch_gui() {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    if arguments
        .first()
        .is_some_and(|argument| argument == "--open-image")
    {
        let image = arguments
            .get(1)
            .ok_or_else(|| err("--open-image requires an absolute WIM path"));
        let result = image.and_then(|image| {
            backuprestore_core::validate_absolute_path(image)?;
            // This is a GUI launch option, not a task-preparation argument.
            // The native window consumes it once during initialization.
            unsafe { env::set_var("BACKUPRESTORE_OPEN_IMAGE", image) };
            launch_gui()
        });
        if let Err(error) = result {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    if arguments
        .first()
        .is_some_and(|argument| argument == "--tab")
    {
        // GUI 启动选项：--tab N 打开指定操作模式（**0 基**：0=备份 1=还原A 系统→C盘
        // 2=还原B 数据→非C盘 3=新增非C盘第二系统 4=安装 PE）。越界值交给
        // native_gui 侧按默认（备份）处理，这里只做数字校验。
        // （2026-10-26 订正：旧注释写 1..=5，与实际 0 基索引不符；且原第 6 个入口
        // 「探测」已按用户要求移除，上限确为 4。）
        let tab = arguments
            .get(1)
            .ok_or_else(|| err("--tab requires an index 0..=4"));
        let result = tab.and_then(|index| {
            let _ = index
                .parse::<usize>()
                .map_err(|_| err("--tab index must be a number"))?;
            unsafe { env::set_var("BACKUPRESTORE_OPEN_TAB", index.clone()) };
            launch_gui()
        });
        if let Err(error) = result {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    if arguments.iter().any(|argument| argument == "--test-hook") {
        // 测试钩子：打开 GUI 并读取 C:\br-test.json 自动填充字段 / 自动路由。
        // 必须作为 GUI 启动选项处理，否则首参是 --test-hook 时会落入 CLI
        // dispatch 打 usage 并以退出码 2 结束。native_gui 的 WM_CREATE 会从
        // 命令行参数自行识别 --test-hook 与 --tab。
        if let Err(error) = launch_gui() {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    if arguments
        .first()
        .is_some_and(|argument| argument == "--pe-desktop")
    {
        // WinPE recovery desktop: full-screen shell-free landing window. The
        // desktop returns the operation tab the technician picked; hand it to
        // the main GUI so it opens directly on that page.
        let result = unsafe { native_gui::run_pe_desktop() }.and_then(|tab| {
            if let Some(tab) = tab {
                unsafe { env::set_var("BACKUPRESTORE_OPEN_TAB", tab.to_string()) };
                launch_gui()?;
            }
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    if arguments
        .first()
        .is_some_and(|argument| argument == "--pe-reboot")
    {
        // 桌面快捷方式入口：设置 bootsequence 指向已安装的 PE 恢复环境，
        // 下次重启自动进入 PE。非提升进程会先提权重启。
        let result = unsafe { native_gui::pe_reboot_standalone() };
        if let Err(error) = result {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        return;
    }
    // 命令名与 operation 先留一份：--json 的结论行要用。
    // 必须在 `into_iter()` 之前取——那会 move 掉 `arguments`，之后再借用会
    // 编译失败（E0382）。
    // 注意这两行**不在** `cfg(windows)` 里（只有下面的 emit 块是），所以本机
    // check 也会检查它们；非 Windows 下没人用 → allow 掉「未使用」。
    #[cfg_attr(not(windows), allow(unused_variables))]
    let command_name = arguments.first().cloned().unwrap_or_default();
    #[cfg_attr(not(windows), allow(unused_variables))]
    let operation_arg = arguments
        .iter()
        .position(|a| a == "--operation")
        .and_then(|i| arguments.get(i + 1))
        .cloned()
        .unwrap_or_default();
    let mut args = arguments.into_iter();
    let result = match args.next().as_deref() {
        Some("validate-task") => args
            .next()
            .ok_or_else(|| err("task.json is required"))
            .and_then(validate_task),
        Some("hash") => args
            .next()
            .ok_or_else(|| err("file is required"))
            .and_then(|p| {
                sha256_file(p).map(|h| {
                    println!("{h}");
                })
            }),
        Some("status") => {
            let root = args.next().ok_or_else(|| err("task root is required"));
            let id = args.next().ok_or_else(|| err("task id is required"));
            root.and_then(|r| id.map(|i| (r, i)))
                .and_then(|(r, i)| show_status(r, i))
        }
        #[cfg(windows)]
        Some("prepare") => windows_prepare::prepare(args.collect()),
        #[cfg(windows)]
        Some("list-volumes") => windows_prepare::list_volumes(),
        #[cfg(windows)]
        Some("list-boot-entries") => windows_prepare::list_boot_entries(),
        #[cfg(windows)]
        Some("inspect-environment") => windows_prepare::inspect_environment(),
        #[cfg(windows)]
        Some("wim-info") => {
            // 解析 wim-info 的参数：<absolute-wim> [--skip-hash]
            // 注意：main() 返回 ()，不能用 ?；把剩余参数收集后检测 --skip-hash
            let remaining: Vec<String> = args.collect();
            let skip_hash = remaining.iter().any(|a| a == "--skip-hash");
            // 第一个参数（非标志）为 WIM 路径
            remaining
                .into_iter()
                .find(|a| !a.starts_with("--"))
                .ok_or_else(|| err("WIM image path is required"))
                .and_then(|image_path| windows_prepare::wim_info(image_path, skip_hash))
        }
        #[cfg(windows)]
        Some("online") => online_cli(args.collect()),
        #[cfg(windows)]
        Some("list-images") => {
            // 备份文件管理（P1-c）：列目录下的 WIM 及索引元数据，目录缺省 H:\BRImages。
            let remaining: Vec<String> = args.collect();
            let dir = remaining
                .into_iter()
                .find(|a| !a.starts_with("--"))
                .unwrap_or_else(default_image_dir);
            image_admin_cli_list(dir)
        }
        #[cfg(windows)]
        Some("delete-old-indexes") => {
            // 备份文件管理（P1-c）：删最旧索引、保留最新 N 个（破坏性；GUI 侧有确认框）。
            let remaining: Vec<String> = args.collect();
            let image = remaining
                .iter()
                .find(|a| !a.starts_with("--"))
                .cloned()
                .ok_or_else(|| err("WIM image path is required"));
            let keep = remaining
                .iter()
                .position(|a| a == "--keep")
                .and_then(|i| remaining.get(i + 1))
                .and_then(|v| v.parse::<u32>().ok())
                .ok_or_else(|| err("--keep <n> is required"));
            image
                .and_then(|image| keep.map(|keep| (image, keep)))
                .and_then(|(image, keep)| image_admin_cli_delete(image, keep))
        }
        Some("recover") => {
            let root = args.next().ok_or_else(|| err("task root is required"));
            let id = args.next().ok_or_else(|| err("task id is required"));
            let options = parse_recover_options(args.collect());
            root.and_then(|r| id.map(|i| (r, i)))
                .and_then(|(r, i)| options.map(|options| (r, i, options)))
                .and_then(|(r, i, options)| recover(r, i, options))
        }
        #[cfg(windows)]
        Some("resume") => {
            let root = args.next().ok_or_else(|| err("task root is required"));
            let id = args.next().ok_or_else(|| err("task id is required"));
            root.and_then(|r| id.and_then(|id| resume_task(r, id)))
        }
        #[cfg(windows)]
        Some("abandon-task") => {
            let root = args.next().ok_or_else(|| err("task root is required"));
            let id = args.next().ok_or_else(|| err("task id is required"));
            root.and_then(|r| id.and_then(|id| windows_prepare::abandon_task(r, id)))
        }
        Some("recover-env") => args
            .next()
            .ok_or_else(|| err("RecoveryTask.env is required"))
            .and_then(recover_env_interactive),
        Some("run-command") => {
            let program = args.next().ok_or_else(|| err("program is required"));
            program.and_then(|p| run_command(&p, args.collect()))
        }
        _ => usage(),
    };
    // `--json`：**所有**命令统一在最后输出一行结构化结论。此前只有 online /
    // prepare 两处各自接了，其余命令（recover / resume / abandon-task / …）
    // 在 json 模式下没有输出，判据只能回退到解析中文文案。
    // task_id 留 null：各命令的参数位置不同，统一推断容易错，需要的话由命令
    // 自己输出（prepare 已输出自己的 task_id）。
    #[cfg(windows)]
    {
        if crate::cli_json::enabled() {
            let (ok, error_text, error_code) = match &result {
                Ok(()) => (true, None, None),
                Err(error) => (false, Some(error.to_string()), error.code()),
            };
            // operation 已在 `into_iter()` 之前从 `--operation <值>` 取好，
            // 口径与命令自己理解的一致（probe / backup / restore-existing / …）。
            let (task_id, log_path) = match crate::cli_json::take_detail() {
                Some(detail) => (detail.task_id, detail.log_path),
                None => (None, None),
            };
            crate::cli_json::emit(&crate::cli_json::command_result(
                &command_name,
                &operation_arg,
                ok,
                task_id.as_deref(),
                log_path.as_deref(),
                error_text.as_deref(),
                error_code,
            ));
        }
    }
    if let Err(error) = result {
        #[cfg(windows)]
        record_launch_error(&error);
        // 失败收尾也要还盘符：否则「挂上去、出错退出」正好是最容易留下幽灵盘符的路径。
        #[cfg(windows)]
        crate::windows_prepare::release_process_leases(
            &crate::log_root::logs_dir().join("mount-leases.log"),
        );
        // 时间线补一行 EXIT：把「进程退出那一刻还剩哪些盘符」钉在文件里，
        // 事后判「有没有泄漏」拿它跟进程外的现状对照，不必靠回忆。
        #[cfg(windows)]
        crate::letter_trace::finish();
        eprintln!("error: {error}");
        std::process::exit(1);
    }
    // ★ 正常收尾：把 `hold_for_process()` 留下的盘符全部撤销。
    //   不撤的话，`mountvol <letter>: <卷>` 的**持久**记录会留在 MountedDevices 里，
    //   等那个分区哪天被删掉，就变成 Windows 反复弹「X:\ 无法访问」的幽灵盘符。
    //   崩溃/被强杀的路径走不到这里，由下次启动的 PID 清扫兜底。
    #[cfg(windows)]
    crate::windows_prepare::release_process_leases(
        &crate::log_root::logs_dir().join("mount-leases.log"),
    );
    // 退出前把盘符现状写进时间线（见 `letter_trace` 的模块注释）。
    #[cfg(windows)]
    crate::letter_trace::finish();
}

#[cfg(windows)]
fn online_cli(args: Vec<String>) -> Result<(), TaskError> {
    let mut operation = String::new();
    let mut source_drive = String::new();
    let mut target_drive = String::new();
    let mut image_path = String::new();
    let mut index = String::new();
    let mut compress = String::new();
    let mut image_name = String::new();
    let mut keep_indexes: Option<u32> = None;
    // `--ui-language`：online 自己的语言开关。注意与 `prepare --ui-language` 的
    // 区别——prepare 那个只写进任务 env 给恢复环境用；online 在 Windows 里直接跑，
    // 旗标切的是**本进程**的日志与 CLI 结论文案。
    let mut ui_language: Option<crate::i18n::MessageLanguage> = None;
    // CLI 在线还原的「格式化目标分区」开关（与 GUI 复选框同语义，默认不格式化）
    let mut format_target = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--format-target" => {
                format_target = true;
            }
            "--operation" => {
                i += 1;
                operation = args.get(i).cloned().unwrap_or_default();
            }
            "--source-drive" => {
                i += 1;
                source_drive = args.get(i).cloned().unwrap_or_default();
            }
            "--target-drive" => {
                i += 1;
                target_drive = args.get(i).cloned().unwrap_or_default();
            }
            "--image-path" => {
                i += 1;
                image_path = args.get(i).cloned().unwrap_or_default();
            }
            "--index" => {
                i += 1;
                index = args.get(i).cloned().unwrap_or_default();
            }
            "--compress" => {
                i += 1;
                compress = args.get(i).cloned().unwrap_or_default();
            }
            "--image-name" => {
                i += 1;
                image_name = args.get(i).cloned().unwrap_or_default();
            }
            "--keep-indexes" => {
                i += 1;
                keep_indexes = args.get(i).and_then(|v| v.parse().ok());
            }
            "--ui-language" => {
                // 与 `prepare --ui-language` 同口径：只认 zh*/en* 前缀（`zh-CN`、
                // `en-US` 都接受），其余明确拒绝，避免把恢复环境看不懂的值放进来。
                i += 1;
                let raw = args
                    .get(i)
                    .cloned()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase();
                if raw.starts_with("zh") {
                    ui_language = Some(crate::i18n::MessageLanguage::Chinese);
                } else if raw.starts_with("en") {
                    ui_language = Some(crate::i18n::MessageLanguage::English);
                } else {
                    return Err(err("--ui-language must be zh or en"));
                }
            }
            _ => {}
        }
        i += 1;
    }
    if operation.is_empty() {
        return Err(err("online requires --operation"));
    }
    // 不显式指定 = `fast`（与 GUI 压缩下拉「不选也是 fast」同口径）。空串原样下传
    // 会被 core 的 `canonical_compression` 判 BR-CORE-034——可选旗标不该变成必填
    // （2026-10-25 实机撞到：不带 --compress 的 T: 备份直接失败，冒烟一直显式传
    // fast 所以没暴露）。
    if compress.is_empty() {
        compress = "fast".to_string();
    }
    // 显式旗标 > env `BACKUPRESTORE_LANG` > ini `ui_language` > 中文：
    // 自动化要确定性，一次命令也能覆盖界面偏好。
    if let Some(language) = ui_language {
        crate::i18n::set(language);
    }
    let params = crate::online_operation::OnlineOpParams {
        operation,
        source_drive,
        target_drive,
        image_path,
        index,
        compress,
        image_name,
        keep_indexes,
        // CLI 在线还原的「格式化目标分区」开关：与 GUI 复选框同语义，
        // 默认不格式化保持旧行为，显式 --format-target 才清空目标卷。
        format_target,
    };
    let result = crate::online_operation::execute(&params);
    // `--json` 下只打印一行结构化结论（判据认字段），中文 detail 不再进 stdout；
    // 否则保持旧行为（人类读中文摘要）。日志内容与 GUI 弹窗两种模式完全一致。
    // 详情上报给 `main` 统一输出那一行结论（避免两行 JSON）。
    crate::cli_json::set_detail(None, result.log_path.clone());
    if !crate::cli_json::enabled() {
        println!("{}", result.detail);
    }
    if result.success {
        Ok(())
    } else {
        // 【2026-10-26 修复】此前无论内部是什么错，一律换成一句无码的 `err(...)`，
        // 把 `dism()` 等真实带码错误吞掉，`--json` 的 `error_code` 恒为 `null`——
        // 错误码体系在 `online` 这条主路径上从未生效。现在把码原样带出来；
        // **文案一字不改**（历史判据可能 still 在匹配这句英文），只多一个码。
        let message = "online operation did not complete successfully";
        Err(match result.error_code {
            Some(code) => TaskError::coded(code, message),
            None => err(message),
        })
    }
}

/// 备份文件管理的默认目录：项目约定的镜像卷 `H:\BRImages`；不存在则退回程序目录。
/// 只影响 `list-images` 的缺省值与 GUI 对话框初值，用户随时可以显式指定别的目录。
#[cfg(windows)]
fn default_image_dir() -> String {
    let preferred = std::path::PathBuf::from("H:\\BRImages");
    if preferred.is_dir() {
        return preferred.to_string_lossy().into_owned();
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .to_string_lossy()
        .into_owned()
}

/// `list-images`：打印镜像与索引清单。人类可读模式给人看；`--json` 模式每镜像一行
/// 结构化结论（供自动化/排障），日志路径统一走 `cli_json` 单一出口。
#[cfg(windows)]
fn image_admin_cli_list(dir: String) -> Result<(), TaskError> {
    let dir = std::path::PathBuf::from(dir);
    let (log_dir, _note) = crate::online_log::resolve_dir("image-admin")
        .map_err(|error| err(&format!("cannot create log directory: {error}")))?;
    let log_path = log_dir.join("admin.log");
    let images = crate::image_admin::list_images(&dir, &log_path)?;
    if crate::cli_json::enabled() {
        for image in &images {
            let indexes: Vec<_> = image
                .indexes
                .iter()
                .map(|index| {
                    let entry = image.entries.get(index);
                    serde_json::json!({
                        "index": index,
                        "name": entry.and_then(|e| e.name.clone()),
                        "created": entry.and_then(|e| e.created.clone()),
                        "source": entry.and_then(|e| e.source.clone()),
                        "image_size": entry.and_then(|e| e.image_size),
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::json!({
                    "command": "list-images",
                    "success": true,
                    "dir": dir.display().to_string(),
                    "image": image.path.display().to_string(),
                    "file_size": image.file_size,
                    "index_count": image.indexes.len(),
                    "indexes": indexes,
                    "log_path": log_path.display().to_string(),
                })
            );
        }
        if images.is_empty() {
            println!(
                "{}",
                serde_json::json!({
                    "command": "list-images",
                    "success": true,
                    "dir": dir.display().to_string(),
                    "image": null,
                    "index_count": 0,
                    "indexes": [],
                    "log_path": log_path.display().to_string(),
                })
            );
        }
    } else if images.is_empty() {
        println!("目录下没有 WIM 镜像：{}", dir.display());
    } else {
        for image in &images {
            println!(
                "{}  ({}, {} 个索引)",
                image.path.display(),
                format_size(image.file_size),
                image.indexes.len()
            );
            for index in &image.indexes {
                let entry = image.entries.get(index);
                let name = entry
                    .and_then(|e| e.name.clone())
                    .unwrap_or_else(|| "(无副档)".to_string());
                let created = entry
                    .and_then(|e| e.created.clone())
                    .unwrap_or_else(|| "-".to_string());
                let source = entry
                    .and_then(|e| e.source.clone())
                    .unwrap_or_else(|| "-".to_string());
                let size = entry
                    .and_then(|e| e.image_size)
                    .map(format_size)
                    .unwrap_or_else(|| "-".to_string());
                println!("  索引 {index:<3} {name:<24} {created}  {size:<12} {source}");
            }
        }
        println!("共 {} 个镜像：{}", images.len(), dir.display());
    }
    crate::cli_json::set_detail(None, Some(log_path.display().to_string()));
    Ok(())
}

/// `delete-old-indexes`：破坏性操作。删除前后各记一行日志；结果同时进 `--json`。
#[cfg(windows)]
fn image_admin_cli_delete(image: String, keep: u32) -> Result<(), TaskError> {
    let image = std::path::PathBuf::from(image);
    let (log_dir, _note) = crate::online_log::resolve_dir("image-admin")
        .map_err(|error| err(&format!("cannot create log directory: {error}")))?;
    let log_path = log_dir.join("admin.log");
    crate::append_log(
        &log_path,
        &format!(
            "delete-old-indexes started: image={} keep={keep}",
            image.display()
        ),
    )?;
    let (removed, kept) = crate::image_admin::delete_oldest_indexes(&image, keep, &log_path)?;
    crate::append_log(
        &log_path,
        &format!(
            "delete-old-indexes done: removed={removed} kept={kept} image={}",
            image.display()
        ),
    )?;
    if !crate::cli_json::enabled() {
        println!(
            "已删除 {removed} 个旧索引，保留最新 {kept} 个：{}",
            image.display()
        );
    }
    crate::cli_json::set_detail(None, Some(log_path.display().to_string()));
    Ok(())
}

/// 字节数的人类可读格式（二进制单位，与界面其它容量显示一致）。
#[cfg(windows)]
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Preparation can fail before a task directory exists, for example while the
/// native volume identity preflight is opening a newly attached disk. Persist
/// that diagnostic in the dedicated log root (see `crate::log_root`) without
/// creating a task or changing WinRE/BCD, so the GUI and non-console
/// executable do not lose the reason.
#[cfg(windows)]
fn record_launch_error(error: &TaskError) {
    // 2026-10-18：与 GUI 的读取方（`native_gui::read_latest_launcher_error`）
    // 同源。曾写 `<exe>\logs`——程序目录可能在系统盘/还原目标上，而这条日志
    // 记录的恰是「连任务都还没建成就失败了」的最早期故障。
    let _ = append_log(
        &crate::log_root::logs_dir().join("launcher-errors.log"),
        &error.to_string(),
    );
}

/// panic 载荷落盘到 `<exe>\logs\panic.log`，同时保留默认 hook（继续打 stderr）。
///
/// 为什么必须自己装 hook（2026-10-16 审查，docs/20261016-2250 日志缺口 2）：
/// 产物是 `#![windows_subsystem = "windows"]`，**没有控制台**，panic 的默认
/// 行为是「往 stderr 写一段然后 abort/展开」——在 windows subsystem 下那段文本
/// 无处可去。而 GUI 侧 `native_gui` 的在线线程是 `catch_unwind` 包着的，
/// `unwrap_or_else(|_| …)` 把 payload 整个丢掉，只留一句「线程异常退出」。
/// 两头一夹：**在线操作 panic 之后，日志里既没有断言消息也没有文件行号**，
/// 连是哪个 `expect`/`unwrap`/断言炸的都无从判断。
///
/// 装在这里而不是只在 `native_gui::run` 里：任何路径（GUI / recover / PE 桌面）
/// 的 panic 都能留下痕迹。hook 里只做最小文件写，绝不 panic-on-panic。
#[cfg(windows)]
fn install_panic_logging() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "<unknown location>".to_string());
        // payload 可能是 &str 或 String（Rust 只保证这两个），也可能是自定义类型
        // （Rust 2024 前 Display 也可）——逐层降级取文本，取不到就记类型名。
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .or_else(|| {
                info.payload()
                    .downcast_ref::<&dyn std::fmt::Display>()
                    .map(|value| value.to_string())
            })
            .unwrap_or_else(|| "<panic payload not printable>".to_string());
        let thread = std::thread::current()
            .name()
            .map(|name| name.to_string())
            .unwrap_or_else(|| "<unnamed thread>".to_string());
        // 2026-10-18：panic.log 与其它日志同源（`log_root::logs_dir()`）。
        // 程序目录可能在系统盘/还原目标上，而 panic 恰恰最容易发生在动那些
        // 卷的时候——日志不能跟着陪葬。
        let _ = append_log(
            &crate::log_root::logs_dir().join("panic.log"),
            &crate::i18n::msg_tpl(
                "panic_backtrace",
                &[
                    thread.as_str(),
                    location.as_str(),
                    payload.as_str(),
                    &format!("{:?}", std::backtrace::Backtrace::capture()),
                ],
            ),
        );
    }));
}

/// 把 `catch_unwind` 捕获到的 panic 载荷取成可读文本。
///
/// 与 `install_panic_logging` 配合使用：hook 保证"日志里一定有"，这个函数保证
/// "错误消息里也有"。此前 `unwrap_or_else(|_| …)` 把 payload 整段丢弃。
#[cfg(windows)]
pub(crate) fn panic_payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .or_else(|| {
            payload
                .downcast_ref::<&dyn std::fmt::Display>()
                .map(|value| value.to_string())
        })
        .unwrap_or_else(|| "panic 载荷不是可打印文本（详见 panic.log）".to_string())
}

#[derive(Default)]
struct RecoverOptions {
    dry_run: bool,
    efi_root: Option<PathBuf>,
}

fn parse_recover_options(arguments: Vec<String>) -> Result<RecoverOptions, TaskError> {
    let mut options = RecoverOptions::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--dry-run" => options.dry_run = true,
            "--efi-root" => {
                let root = arguments
                    .next()
                    .ok_or_else(|| err("--efi-root requires a mounted EFI root path"))?;
                if root.trim().is_empty() {
                    return Err(err("--efi-root cannot be empty"));
                }
                options.efi_root = Some(PathBuf::from(root));
            }
            _ => return Err(err(&format!("unknown recover option: {argument}"))),
        }
    }
    Ok(options)
}

#[cfg(not(windows))]
fn should_launch_gui() -> bool {
    false
}

#[cfg(windows)]
fn should_launch_gui() -> bool {
    env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })
        .is_some_and(|stem| stem.eq_ignore_ascii_case("BackupRestore"))
}

#[cfg(not(windows))]
fn launch_gui() -> Result<(), TaskError> {
    Err(err("the GUI launcher is only available on Windows"))
}

#[cfg(windows)]
fn launch_gui() -> Result<(), TaskError> {
    native_gui::run()
}


/// A prepared task may be a deliberate `--no-reboot` diagnostic and must not
/// start WinRE by itself. Every later non-terminal stage, however, represents
/// an already authorized recovery that was interrupted after WinRE staging;
/// it is safe to re-request the one-time WinRE boot after artifact validation.
#[cfg(any(windows, test))]
pub(crate) fn stage_resumable_after_interruption(stage: Stage) -> bool {
    matches!(
        stage,
        Stage::BootRequested
            | Stage::RecoveryStarted
            | Stage::Preflight
            | Stage::Capturing
            | Stage::TargetErased
            | Stage::ImageApplied
            | Stage::BootRepaired
            | Stage::RecoveryComplete
    )
}

pub(crate) fn err(message: &str) -> TaskError {
    TaskError::Invalid(message.into())
}

/// Each WIM index owns a sidecar so appending a new capture never makes the
/// target-size and source identity of earlier indexes ambiguous.  The legacy
/// directory-level `metadata.json` remains a read fallback for images made by
/// older releases.
// sidecar（副档）簿记统一在 core 里实现，CLI / WinRE / GUI 三个入口共用同一套，
// 避免「哪个入口备份的镜像」在元数据上产生差异（v2.0.1 修 P1-1）。
#[cfg(windows)]
pub(crate) fn index_metadata_path(image: &Path, index: u32) -> Result<PathBuf, TaskError> {
    backuprestore_core::image_metadata::index_metadata_path(image, index)
}

#[cfg(windows)]
pub(crate) fn legacy_metadata_path(image: &Path) -> Result<PathBuf, TaskError> {
    backuprestore_core::image_metadata::legacy_metadata_path(image)
}

#[cfg(windows)]
pub(crate) fn read_index_metadata(
    image: &Path,
    index: u32,
) -> Result<backuprestore_core::BackupMetadata, TaskError> {
    backuprestore_core::image_metadata::read_index_metadata(image, index)
}

/// 解析卷内工作区相对路径；从最后一个 tasks 分隔符切分，允许上层目录也叫 tasks。
#[cfg(any(windows, test))]
pub(crate) fn split_workspace_root_rel(value: &str, task_id: &str) -> Result<String, TaskError> {
    backuprestore_core::validate_relative_path(value)?;
    let normalized = value.replace('/', "\\");
    let (store_rel, relative_id) = normalized
        .rsplit_once("\\tasks\\")
        .ok_or_else(|| err("WORKSPACE_ROOT_REL must contain \\tasks\\"))?;
    if store_rel.is_empty() {
        return Err(err("WORKSPACE_ROOT_REL has an empty workspace path"));
    }
    if relative_id != task_id {
        return Err(err("WORKSPACE_ROOT_REL task id does not match TASK_ID"));
    }
    Ok(store_rel.to_string())
}

fn validate_task(path: String) -> Result<(), TaskError> {
    let task: Task = read_json(&path)?;
    task.validate()?;
    println!(
        "valid task {} operation={:?} status={:?}",
        task.task_id, task.operation, task.status
    );
    Ok(())
}

fn show_status(root: String, id: String) -> Result<(), TaskError> {
    let store = TaskStore::new(root);
    let task = store.load(&id)?;
    let status: StatusRecord = read_json(store.status_path(&id)?)?;
    if status.task_id != task.task_id {
        return Err(err("status file task_id does not match task.json"));
    }
    if status.operation != task.operation {
        return Err(err("status file operation does not match task.json"));
    }
    println!(
        "{}",
        serde_json::to_string_pretty::<backuprestore_core::StatusRecord>(&status)?
    );
    eprintln!("operation={:?} stage={:?}", task.operation, task.status);
    Ok(())
}

/// 写一行日志。
///
/// 真正的实现在 `session_log`：除主路径外，若本次运行在 PE / WinRE 的**内存盘**
/// 会话里，还会按文件名往持久化卷的 `BackupRestore-logs\<会话>\` 多写一份。
/// 理由（2026-10-17 第二轮核查，日志缺口 1/2）：`X:` 上的日志重启即丢，而丢掉的
/// 恰恰是恢复环境的逃生决策与 PE 会话内的操作记录。普通 Windows 上镜像恒为
/// `None`，行为与改动前逐字一致。
pub(crate) fn append_log(path: &Path, message: &str) -> Result<(), TaskError> {
    crate::session_log::append(path, message)
}

fn recover(root: String, id: String, options: RecoverOptions) -> Result<(), TaskError> {
    let store = TaskStore::new(root);
    // 【2026-10-19 D1】`recover` 的第一个参数是**工作区根**，内部再拼 `tasks/<id>`。
    // 传成 `...\tasks` 时旧代码只报 `os error 3`，连是哪个路径都看不出来——在 PE 里
    // 排查这个代价极高（无 shell、无 tasklist，只能截图猜）。先做一次显式存在性
    // 检查，把完整路径和正确用法一起写进错误。
    let task_path = store
        .task_path(&id)
        .map_err(|error| errc_with(ErrCode::TaskIdInvalid, &[&id, &error.to_string()]))?;
    if !task_path.is_file() {
        return Err(errc_with(
            ErrCode::TaskFileMissing,
            &[&task_path.display().to_string()],
        ));
    }
    let mut task = store.load(&id)?;
    let log = store.log_path(&id)?;
    append_log(
        &log,
        &format!(
            "Recovery started operation={:?} task={}",
            task.operation, task.task_id
        ),
    )?;
    if task.status == Stage::Success {
        append_log(&log, "Task already successful; refusing to run twice")?;
        return Ok(());
    }
    if task.status == Stage::Failed {
        return Err(err(
            "task is failed; create a new task after reviewing recovery.log",
        ));
    }
    if options.dry_run {
        println!(
            "dry-run: task={} operation={:?} stage={:?}",
            task.task_id, task.operation, task.status
        );
        if let Some(image) = &task.image {
            println!(
                "image={} sha256={}",
                image
                    .absolute_path
                    .as_deref()
                    .unwrap_or(&image.relative_path),
                image.sha256
            );
        }
        if let Some(target) = &task.target {
            println!(
                "target partition={} role={:?}",
                target.volume.partition_guid, target.role
            );
        }
        append_log(&log, "Dry-run completed; no disk operation performed")?;
        return Ok(());
    }
    if !cfg!(windows) {
        return Err(err(
            "real Recovery execution is only available on Windows/WinRE; use --dry-run on this host",
        ));
    }
    if let Some(efi_root) = options.efi_root.as_deref() {
        if !efi_root.is_dir() {
            return Err(err("--efi-root must be an existing mounted EFI directory"));
        }
        append_log(
            &log,
            &format!(
                "Using explicit EFI root for direct recovery: {}",
                efi_root.display()
            ),
        )?;
    }
    // 【2026-10-18 缺陷修复 M7】手动 `recover` 也要把任务 env 交给恢复链。
    // 此前这里传 `None`，于是：①擦除前**不重新武装**一次性启动项（中断后落回
    // 原版 WinRE、无法续跑）；②不校验「续跑载荷卷 ≠ 擦除目标卷」（可能误擦
    // 载荷所在卷）；③副档元数据拿不到 env 里的值，只能走兜底。
    #[cfg(windows)]
    let metadata_values: Option<BTreeMap<String, String>> = {
        let dir = store.task_dir(&id)?;
        let env = dir.join("payload/RecoveryTask.env");
        let values = if env.is_file() {
            read_env_file(env)?
        } else {
            BTreeMap::new()
        };
        recovery_fault::configure(values.get("TEST_FAULT").map(String::as_str), &dir);
        if task.status == Stage::RecoveryComplete {
            return complete_recovery_cleanup(&store, &mut task, &log);
        }
        Some(values)
    };
    #[cfg(not(windows))]
    let metadata_values: Option<BTreeMap<String, String>> = None;
    // 【2026-10-19 D3】EFI 根必须在这里（STEP 1 之前）就解析好。旧代码把它留到
    // STEP 3，而 PE 里 ESP 默认没有盘符：镜像照灌 30GB 之后才发现没有 EFI 根，
    // 等于白灌一遍（D3）。现在提前解析，并且不再强制 `--efi-root`——能自动挂载
    // 就自动挂载，挂不上就在**任何写盘动作之前**失败。
    #[cfg(windows)]
    let efi_root: Option<PathBuf> = match options.efi_root.clone() {
        Some(root) => Some(root),
        None => resolve_efi_root_for_recovery(&task, &log, metadata_values.as_ref())?,
    };
    #[cfg(not(windows))]
    let efi_root: Option<PathBuf> = options.efi_root.clone();
    let result = recover_windows(
        &store,
        &mut task,
        &log,
        efi_root.as_deref(),
        metadata_values.as_ref(),
        false,
        None,
    );
    if let Err(error) = &result {
        // 【2026-10-16 审查修订，日志缺口 4】三处加固，都不改变主流程的成败：
        // ① 先落 Recovery.log 再写 status.json——GUI 的错误文案引导用户"查看
        //    Recovery.log"，而这条路径此前只写 status.json，日志里没有错误内容。
        // ② 写盘失败不再 `?`——那会用**写盘错误**替换掉原始恢复错误向上传播，
        //    把真正的失败原因顶掉（旧代码 `store.write_recovery_error(...)?`）。
        // ③ 两个失败都各自记一行，事后能看出"恢复失败"与"连错误都没记上"的区别。
        if let Err(log_error) = append_log(
            &log,
            &crate::i18n::msg_tpl("recovery_failed", &[&error.to_string()]),
        ) {
            let _ = append_log(
                &log,
                &crate::i18n::msg_tpl("recovery_failed_log_write", &[&error.to_string(), &log_error.to_string()]),
            );
        }
        if let Err(write_error) =
            store.write_recovery_error_coded(&task, error.code(), error.to_string())
        {
            let _ = append_log(
                &log,
                &crate::i18n::msg_tpl(
                    "recovery_failed_status_write",
                    &[&error.to_string(), &write_error.to_string()],
                ),
            );
        }
    }
    #[cfg(windows)]
    if result.is_ok() {
        store.write_transition(&mut task, Stage::RecoveryComplete)?;
        complete_recovery_cleanup(&store, &mut task, &log)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        bcd_identifier_from_line, claim_boot_resume_attempt, diskpart_format_script,
        parse_boot_manager_state, parse_recover_options, split_workspace_root_rel,
        stage_resumable_after_interruption,
    };

    #[test]
    fn dispatch_failure_carries_its_code_even_without_json() {
        // 「码是契约」要追到出口：`run-command` 直通通道的程序失败必须带码，
        // 否则 `--json` 的 errorCode 恒为 null（2026-10-27 由
        // `secondary_menu_repair_rejections` 的断言逼出来的同类缺陷）。
        //
        // 刻意用 `sh -c 'exit 3'` 而不是 Windows 专有程序：本项目最贵的教训之一是
        // **`#[cfg(windows)]` 里的测试在 mac 上是零信息**（编译器根本不打开那个文件）。
        // 这个用例在两侧都能跑，才真正守得住。
        let error = super::run_command("sh", vec!["-c".to_string(), "exit 3".to_string()])
            .expect_err("非零退出必须报错");
        assert_eq!(error.code(), Some("BR-APL-010"), "{error}");
        assert!(
            error.to_string().starts_with("[BR-APL-010]"),
            "Display 必须带码前缀：{error}"
        );
        // 直通通道没有输出尾部可附；模板带尾部槽时塞空串会留下裸换行。
        assert!(
            !error.to_string().ends_with('\n'),
            "文案不该以换行结尾：{error:?}"
        );
    }

    #[test]
    fn recovery_boot_menu_gate_matches_finalize_routing() {
        use backuprestore_core::TargetRole;
        // create-secondary 的常态：prepare 刚格式化完的空卷——灌镜像前探测必然
        // false，但收尾按 Windows 走，会话必须建（2026-10-19 实机缺陷）。
        assert!(super::recovery_needs_boot_menu_session(
            false,
            TargetRole::NewWindows
        ));
        // A 入口（重建唯一系统）：目标当前就是 Windows 卷。
        assert!(super::recovery_needs_boot_menu_session(
            true,
            TargetRole::ExistingWindows
        ));
        // 数据卷还原：目标无 Windows、也不是新增系统——会话不该建，
        // 否则空会话反而会在 STEP 4 被无条件要求。
        assert!(!super::recovery_needs_boot_menu_session(
            false,
            TargetRole::ExistingWindows
        ));
        // 既有 Windows 卷上新增第二系统（覆盖式）：两个条件都真。
        assert!(super::recovery_needs_boot_menu_session(
            true,
            TargetRole::NewWindows
        ));
    }

    #[test]
    fn secondary_menu_omits_deleted_objects_and_requires_primary() {
        let primary = "{11111111-2222-4333-8444-555555555555}";
        let removed = "{22222222-3333-4444-8555-666666666666}";
        let secondary = "{33333333-4444-4555-8666-777777777777}";
        let other = "{44444444-5555-4666-8777-888888888888}";
        let previous = super::BcdBootManagerState {
            default: primary.into(),
            display_order: vec![primary.into(), removed.into(), other.into()],
            resume_object: None,
            loader_resume_objects: Vec::new(),
        };
        let live = format!("identifier {primary}\n\n标识符 {secondary}\n\nidentifier {other}");
        assert_eq!(
            super::secondary_boot_order(&previous, secondary, &live).unwrap(),
            [primary, other, secondary]
        );
        assert!(
            super::secondary_boot_order(&previous, secondary, &format!("identifier {secondary}"))
                .is_err()
        );
        assert!(
            super::secondary_boot_order(&previous, secondary, &format!("identifier {primary}"))
                .is_err()
        );
    }

    /// **只读拒绝路径**（2026-10-27 新增）。
    ///
    /// `preserve_primary_boot_manager` 第一件事是 `bcdedit /store <EFI>\EFI\Microsoft\Boot\BCD
    /// /enum all /v`：**读不到存储就返回，一个字节都不写**。而原用例
    /// `secondary_menu_repair_native` 要真实第二系统任务 + 已挂载 ESP + 快照，
    /// 环境一删就「永远不跑」——偏偏它是**改启动菜单**，最该有常态护栏。
    /// 这里只验「BCD 存储不可读时拒绝且不写」：不需要 ESP、不需要快照、不需要第二系统。
    #[cfg(windows)]
    #[test]
    #[ignore = "需要 Windows 管理员（只读拒绝路径：BCD 存储不存在时必须拒绝且不写）"]
    fn secondary_menu_repair_rejections() {
        let tmp = std::env::temp_dir().join(format!(
            "br-menu-reject-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let log = tmp.join("menu-reject.log");
        let previous = super::BcdBootManagerState {
            default: "{aaaaaaaa-0000-0000-0000-000000000001}".to_string(),
            display_order: vec!["{aaaaaaaa-0000-0000-0000-000000000001}".to_string()],
            resume_object: None,
            loader_resume_objects: Vec::new(),
        };
        let secondary = "{aaaaaaaa-0000-0000-0000-000000000002}";
        let error = super::preserve_primary_boot_manager(&tmp, &previous, secondary, &log)
            .expect_err("BCD 存储不存在时必须拒绝");
        // 契约：失败必须带**稳定且确定**的码（本项目「码要追到出口」的纪律）。
        // 2026-10-27 这条断言第一次跑就变红：`capture_logged` 用无码 `err()`，
        // 与 `run_logged` 的 `BR-APL-009` 不一致。只断言「有码」会漏掉「码不对」，
        // 所以这里钉死具体码。
        assert_eq!(error.code(), Some("BR-APL-009"), "外部命令失败的稳定码：{error}");
        assert!(
            error.to_string().contains("bcdedit"),
            "失败文案必须点名是哪个程序：{error}"
        );
        // 不得在目标根上创建/改动任何启动相关目录（EFI 目录本身就是「已经动过」的证据）。
        assert!(
            !tmp.join("EFI").exists(),
            "拒绝路径不得创建 EFI 目录：{}",
            tmp.join("EFI").display()
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "需要已完成的第二系统任务、EFI 挂载点及新核验快照；真实修改启动菜单"]
    fn secondary_menu_repair_native() {
        use std::path::PathBuf;
        let snapshot = std::env::var("BR_ACCEPTANCE_SNAPSHOT").expect("新快照");
        assert!(super::bcd_identifier_from_line(&snapshot).is_some());
        let root = std::env::var("BR_ACCEPTANCE_ROOT").expect("任务根目录");
        let id = std::env::var("BR_ACCEPTANCE_TASK").expect("任务ID");
        let efi = PathBuf::from(std::env::var("BR_ACCEPTANCE_EFI").expect("已挂载ESP"));
        let secondary = std::env::var("BR_ACCEPTANCE_SECONDARY").expect("已核验第二加载器");
        assert!(super::bcd_identifier_from_line(&secondary).is_some());
        let store = super::TaskStore::new(root);
        let task = store.load(&id).unwrap();
        assert_eq!(task.status, super::Stage::Success);
        assert_eq!(task.operation, backuprestore_core::Operation::CreateSecondary);
        let log = store
            .task_dir(&id)
            .unwrap()
            .join("menu-repair-acceptance.log");
        let previous = super::boot_manager_state_from_task_snapshot(&store, &task, &log).unwrap();
        super::preserve_primary_boot_manager(&efi, &previous, &secondary, &log).unwrap();
        println!("SECONDARY_MENU_REPAIR_VERIFIED");
    }
    use backuprestore_core::Stage;

    #[test]
    fn automatic_boot_resume_is_limited_to_once_per_durable_stage() {
        let root = std::env::temp_dir().join(format!(
            "backuprestore-resume-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).unwrap();

        assert!(claim_boot_resume_attempt(&root, Stage::BootRequested).unwrap());
        assert!(!claim_boot_resume_attempt(&root, Stage::BootRequested).unwrap());
        assert!(claim_boot_resume_attempt(&root, Stage::RecoveryStarted).unwrap());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_options_accept_explicit_efi_root() {
        let options =
            parse_recover_options(vec!["--dry-run".into(), "--efi-root".into(), "E:\\".into()])
                .expect("options should parse");
        assert!(options.dry_run);
        assert_eq!(options.efi_root.unwrap().to_string_lossy(), "E:\\");
    }

    #[test]
    fn recover_options_reject_unknown_or_incomplete_flags() {
        assert!(parse_recover_options(vec!["--unknown".into()]).is_err());
        assert!(parse_recover_options(vec!["--efi-root".into()]).is_err());
    }

    #[test]
    fn diskpart_format_script_uses_verified_target_numbers() {
        assert_eq!(
            diskpart_format_script(Some(3), Some(2)).unwrap(),
            "select disk 3\r\nselect partition 2\r\nformat fs=ntfs quick\r\n"
        );
        assert!(diskpart_format_script(None, Some(2)).is_err());
        assert!(diskpart_format_script(Some(3), None).is_err());
        assert!(diskpart_format_script(Some(3), Some(0)).is_err());
    }

    #[test]
    fn workspace_path_allows_directory_named_tasks() {
        let task_id = "11111111-1111-4111-8111-111111111111";
        assert_eq!(
            split_workspace_root_rel(&format!("tasks\\tasks\\{task_id}"), task_id).unwrap(),
            "tasks"
        );
        assert_eq!(
            split_workspace_root_rel(
                &format!("Tools\\tasks\\BackupRestore\\tasks\\{task_id}"),
                task_id
            )
            .unwrap(),
            "Tools\\tasks\\BackupRestore"
        );
        assert!(
            split_workspace_root_rel("..\\tasks\\11111111-1111-4111-8111-111111111111", task_id)
                .is_err()
        );
        assert!(
            split_workspace_root_rel(
                "tasks\\tasks\\22222222-2222-4222-8222-222222222222",
                task_id
            )
            .is_err()
        );
    }

    #[test]
    fn bcd_identifier_parser_ignores_localized_field_labels() {
        assert_eq!(
            bcd_identifier_from_line(
                "\u{fffd}\u{fffd}\u{fffd} {3f66f61e-a1ce-11f1-9d29-f5ba5ed7cdd2}"
            ),
            Some("{3f66f61e-a1ce-11f1-9d29-f5ba5ed7cdd2}")
        );
        assert_eq!(bcd_identifier_from_line("device partition=H:"), None);
        assert_eq!(bcd_identifier_from_line("{not-a-bcd-guid}"), None);
    }

    #[test]
    fn secondary_loader_selection_ignores_resume_object() {
        let output = "Windows Boot Loader\nidentifier {11111111-2222-4333-8444-555555555555}\ndevice partition=Q:\nosdevice partition=Q:\npath \\Windows\\system32\\winload.efi\n\nWindows Resume Application\nidentifier {22222222-3333-4444-8555-666666666666}\ndevice partition=Q:\npath \\Windows\\system32\\winresume.efi";
        let selected = output
            .split("\n\n")
            .filter_map(|block| {
                let identifier = block.lines().find_map(bcd_identifier_from_line)?;
                let lower = block.to_ascii_lowercase();
                let targets_partition = lower.lines().any(|line| {
                    let line = line.trim_start();
                    (line.starts_with("device") || line.starts_with("osdevice"))
                        && line.contains("partition=q:")
                });
                (lower.contains("winload.efi") && targets_partition).then_some(identifier)
            })
            .next();
        assert_eq!(selected, Some("{11111111-2222-4333-8444-555555555555}"));
    }

    #[test]
    fn boot_manager_state_parser_accepts_guid_and_alias_only() {
        assert_eq!(
            parse_boot_manager_state(
                "Firmware Boot Manager\nidentifier {aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\ndisplayorder {ffffffff-1111-4222-8333-444444444444}\n\nWindows Boot Manager\nidentifier {9dea862c-5cdd-4e70-acc1-f32b344d4795}\ndefault {11111111-2222-4333-8444-555555555555}\ndisplayorder {11111111-2222-4333-8444-555555555555}"
            )
            .unwrap()
            .default,
            "{11111111-2222-4333-8444-555555555555}"
        );
        assert_eq!(
            parse_boot_manager_state(
                "Windows Boot Manager\nidentifier {9dea862c-5cdd-4e70-acc1-f32b344d4795}\n默认 {default}\n显示顺序 {default}"
            )
            .unwrap()
            .display_order,
            vec!["{default}"]
        );
        assert!(parse_boot_manager_state("default dangerous;value").is_none());
    }

    #[test]
    fn boot_manager_state_parser_preserves_resume_object_and_loader_mapping() {
        let state = parse_boot_manager_state(
            "Windows Boot Manager\nidentifier {9dea862c-5cdd-4e70-acc1-f32b344d4795}\ndefault {11111111-2222-4333-8444-555555555555}\nresumeobject {aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\ndisplayorder {11111111-2222-4333-8444-555555555555}\n\nWindows Boot Loader\nidentifier {11111111-2222-4333-8444-555555555555}\ndevice partition=C:\npath \\Windows\\system32\\winload.efi\nresumeobject {bbbbbbbb-2222-4333-8444-555555555555}",
        )
        .unwrap();
        assert_eq!(
            state.resume_object.as_deref(),
            Some("{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}")
        );
        assert_eq!(
            state.loader_resume_objects,
            vec![(
                "{11111111-2222-4333-8444-555555555555}".into(),
                "{bbbbbbbb-2222-4333-8444-555555555555}".into(),
            )]
        );
    }

    #[test]
    fn boot_menu_does_not_absorb_tools_or_later_guid_fields() {
        for field in ["displayorder", "显示顺序"] {
            let state = parse_boot_manager_state(&format!(
                "identifier {{9dea862c-5cdd-4e70-acc1-f32b344d4795}}\r\ndefault {{default}}\r\n{field} {{default}}\r\n                        {{current}}\r\ntoolsdisplayorder {{memdiag}}\r\n                        {{other}}\r\ntimeout 0"
            )).unwrap();
            assert_eq!(state.display_order, ["{default}", "{current}"]);
        }
    }

    #[test]
    fn interruption_resume_accepts_only_staged_nonterminal_tasks() {
        assert!(!stage_resumable_after_interruption(Stage::Prepared));
        assert!(stage_resumable_after_interruption(Stage::BootRequested));
        assert!(stage_resumable_after_interruption(Stage::TargetErased));
        assert!(stage_resumable_after_interruption(Stage::ImageApplied));
        assert!(stage_resumable_after_interruption(Stage::BootRepaired));
        assert!(!stage_resumable_after_interruption(Stage::Success));
        assert!(!stage_resumable_after_interruption(Stage::Failed));
    }

    #[cfg(windows)]
    #[test]
    fn prepare_options_reject_missing_image_or_invalid_drive() {
        assert!(
            super::windows_prepare::parse_prepare_options(vec![
                "--operation".into(),
                "probe".into(),
                "--source-drive".into(),
                "C".into(),
            ])
            .is_ok()
        );
        assert!(
            super::windows_prepare::parse_prepare_options(vec![
                "--operation".into(),
                "restore-existing".into(),
                "--source-drive".into(),
                "C".into(),
            ])
            .is_err()
        );
        assert!(
            super::windows_prepare::parse_prepare_options(vec![
                "--operation".into(),
                "probe".into(),
                "--source-drive".into(),
                "CC".into(),
            ])
            .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn prepare_fault_options_require_efi_only_for_efi_faults() {
        let base = vec![
            "--operation".into(),
            "probe".into(),
            "--source-drive".into(),
            "P".into(),
        ];
        let mut power_loss = base.clone();
        power_loss.extend(["--test-fault".into(), "power-loss-window".into()]);
        assert!(
            super::windows_prepare::parse_prepare_options(power_loss).is_ok(),
            "power-loss-window uses the real system EFI by default"
        );

        for fault in ["identity-env-mismatch", "bcdboot-failure"] {
            let mut missing_efi = base.clone();
            missing_efi.extend(["--test-fault".into(), fault.into()]);
            assert!(
                super::windows_prepare::parse_prepare_options(missing_efi).is_err(),
                "{fault} must require an explicit development EFI"
            );
        }
        for fault in [
            "power-loss-target-erased",
            "power-loss-image-applied",
            "power-loss-boot-repaired",
        ] {
            let mut system_efi_fault = base.clone();
            system_efi_fault.extend(["--test-fault".into(), fault.into()]);
            assert!(
                super::windows_prepare::parse_prepare_options(system_efi_fault).is_ok(),
                "{fault} must use the current system EFI by default"
            );
        }
    }

    /// 【2026-10-23 实机发现】按 B 回退后续跑链校验失败，文案要说明因果。
    #[test]
    fn chain_failure_note_only_for_bcd_rollback_sessions() {
        let chain = "invalid task: 续跑启动链字段不符：device";
        // 回退过 + 续跑链错 → 补说明，且原文不丢
        let note = crate::annotate_chain_failure(chain, true).expect("应补说明");
        assert!(note.contains("续跑启动链字段不符：device"), "原文不能丢：{note}");
        assert!(note.contains("按 B 回退过 BCD"), "要点明是用户按了 B：{note}");
        // 不能把 `invalid task: ` 前缀带进来——调用方还会再包一层，否则对话框
        // 会出现「invalid task: invalid task: …」（2026-10-23 实机撞见）。
        assert!(
            !note.starts_with("invalid task: "),
            "前缀必须剥掉：{note}"
        );
        // 没回退过 → 不改写
        assert_eq!(crate::annotate_chain_failure(chain, false), None);
        // 回退过但错误不是续跑链 → 保持原文，不能被说明糊住
        assert_eq!(
            crate::annotate_chain_failure("payload WIM is missing", true),
            None
        );
    }
}
