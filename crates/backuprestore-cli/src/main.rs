#![cfg_attr(windows, windows_subsystem = "windows")]

//! Small Windows front-end/recovery host entry point.
//!
//! The GUI can call this binary for task validation and execution.  Keeping
//! the recovery path as a command-line program also makes it usable from
//! WinRE's `winpeshl.ini` without depending on a desktop runtime.

use backuprestore_core::{TaskError, sha256_file};
use std::env;
use std::path::Path;

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
#[cfg_attr(not(windows), allow(dead_code))]
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
#[cfg(any(windows, test))]
#[allow(unused_imports)]
pub(crate) use image_admin::{
    default_image_dir, format_size, index_metadata_path, legacy_metadata_path,
    read_index_metadata,
};
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
mod wim_parse;
#[cfg_attr(not(windows), allow(dead_code))]
mod bcd_parse;
#[cfg_attr(not(windows), allow(dead_code))]
mod step_progress;
/// 卷挂载判定、设备路径规范化与盘符时间线追踪（v2.9.9 从 `text_parsing` 抽离）。
/// **无条件编译**：调用方（`recovery_volume` / `letter_trace` / `prepare_bcd` 等）
/// 多为 Windows 专属，判断逻辑集中在这里才能被 macOS 单测覆盖。
#[cfg_attr(not(windows), allow(dead_code))]
mod volume_parse;
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
pub(crate) mod volume_device;
#[cfg(windows)]
pub(crate) mod prepare_bcd;
#[cfg(windows)]
pub(crate) mod prepare_env;
#[cfg(windows)]
pub(crate) mod prepare_validation;
#[cfg(windows)]
pub(crate) mod environment_admin;
#[cfg(windows)]
pub(crate) mod prepare_payload;
#[cfg(windows)]
pub(crate) mod prepare_mount;
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
            .and_then(crate::task_admin::cli_validate_task),
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
                .and_then(|(r, i)| crate::task_admin::cli_show_status(r, i))
        }
        #[cfg(windows)]
        Some("prepare") => windows_prepare::prepare(args.collect()),
        #[cfg(windows)]
        Some("list-volumes") => volume_device::cli_list_volumes(),
        #[cfg(windows)]
        Some("list-boot-entries") => environment_admin::list_boot_entries(),
        #[cfg(windows)]
        Some("inspect-environment") => environment_admin::inspect_environment(),
        #[cfg(windows)]
        Some("wim-info") => crate::image_admin::cli_wim_info(args.collect()),
        #[cfg(windows)]
        Some("online") => online_operation::cli_online(args.collect()),
        #[cfg(windows)]
        Some("list-images") => crate::image_admin::cli_list(args.collect()),
        #[cfg(windows)]
        Some("delete-old-indexes") => crate::image_admin::cli_delete(args.collect()),
        Some("recover") => cli_recover(args),
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
            root.and_then(|r| id.and_then(|id| environment_admin::abandon_task(r, id)))
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


pub(crate) fn err(message: &str) -> TaskError {
    TaskError::Invalid(message.into())
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

