#![cfg_attr(windows, windows_subsystem = "windows")]

//! Small Windows front-end/recovery host entry point.
//!
//! The GUI can call this binary for task validation and execution.  Keeping
//! the recovery path as a command-line program also makes it usable from
//! WinRE's `winpeshl.ini` without depending on a desktop runtime.

#[cfg(windows)]
use crate::text_parsing::{
    MountedVolumes, VolumeMountQuery, classify_mountvol_output, parse_mountvol_listing,
};
#[cfg(windows)]
use backuprestore_core::canonical_compression;
#[cfg(windows)]
use backuprestore_core::{BootMode, Operation, PayloadManifest, verify_image_file};
use backuprestore_core::{Stage, StatusRecord, Task, TaskError, TaskStore, read_json, sha256_file};
// 非 Windows 构建里 `Utc` 只被 `#[cfg(windows)]` 的代码用到（备份时间戳等）；
// macOS 上留着它会产生 unused import，门控掉。
#[cfg_attr(not(windows), allow(unused_imports))]
use chrono::Utc;
use std::collections::BTreeMap;
use std::env;
// 同上：`OpenOptions` 只被 `#[cfg(any(windows, test))]` 的续跑标记用到。
#[cfg_attr(not(windows), allow(unused_imports))]
use std::fs::{self, OpenOptions};
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use std::process::{ChildStderr, ChildStdout, Output, Stdio};
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(any(windows, test))]
mod boot_cleanup;
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

fn usage() -> ! {
    eprintln!(
        "BackupRestore commands:\n  validate-task <task.json>\n  hash <file>\n  status <task-root> <task-id>\n  prepare --operation <probe|backup|restore-existing|create-secondary> --source-drive <letter> [--target-drive <letter>] [--image-path <absolute-wim>] [--wim-index <n>] [--boot-menu-name <name>] [--allow-destructive] [--no-reboot] [--skip-format] [--fail-threshold <1-20>] [--fail-action <auto-escape|pause>] [--ui-language <zh|en>]\n  prepare ... [--test-efi-drive <letter>] [--test-fault <identity-env-mismatch|bcdboot-failure|power-loss-window>]  (development test only)
  online --operation <backup|restore-existing|create-secondary> --source-drive <letter> [--target-drive <letter>] [--image-path <absolute-wim>] [--index <n>] [--compress <fast|none> (default fast)] [--image-name <name>] [--keep-indexes <n>] [--format-target] [--ui-language <zh|en>]\n  list-volumes\n  inspect-environment\n  wim-info <absolute-wim> [--skip-hash]\n  recover <task-root> <task-id> [--dry-run] [--efi-root <mounted EFI root>]\n  resume <task-root> <task-id>  (retry existing recovery or cleanup)\n  recover-env <RecoveryTask.env>
  list-images [dir]  (default H:\\BRImages; list WIMs and their indexes)
  delete-old-indexes <absolute-wim> --keep <n>  (destructive: delete the oldest indexes, keep the latest n)\n  run-command <program> [args...]\n  --json  (print the verdict as ONE line of JSON: command/operation/success/log_path/error; for automation)\n  --open-image <absolute-wim>  (GUI only)\n  --tab <0..=4>  (GUI only: 0=backup 1=restore-A to C: 2=restore-B to non-C: 3=second system 4=install PE)\n  --pe-desktop  (WinPE recovery desktop, GUI only)\n"
    );
    std::process::exit(2)
}

fn main() {
    // 必须最先做：`session_log::append` 之后的每一次落盘都要知道该不该镜像。
    // PE / WinRE 里主日志落在内存盘 `X:` 上，重启即丢（2026-10-17 第二轮核查
    // 日志缺口 1/2），镜像是唯一能跨重启活下来的那一份。
    #[cfg(windows)]
    crate::session_log::init();
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
    let mut arguments: Vec<String> = env::args().skip(1).collect();
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
        eprintln!("error: {error}");
        std::process::exit(1);
    }
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

/// Claim the single automatic retry allowed for a pending stage. A distinct
/// marker per durable stage permits resuming after later progress, while a
/// failed WinRE handoff cannot reboot Windows forever at the same stage.
#[cfg(any(windows, test))]
fn claim_boot_resume_attempt(task_dir: &Path, stage: Stage) -> Result<bool, TaskError> {
    let marker = task_dir.join(format!("boot-resume-{stage:?}.attempted"));
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(mut file) => {
            if let Err(error) = file
                .write_all(b"automatic WinRE resume claimed\n")
                .and_then(|()| file.sync_all())
            {
                drop(file);
                let _ = fs::remove_file(marker);
                return Err(error.into());
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
fn release_boot_resume_attempt(task_dir: &Path, stage: Stage) -> Result<(), TaskError> {
    let marker = task_dir.join(format!("boot-resume-{stage:?}.attempted"));
    match fs::remove_file(marker) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// 跨阶段**自动续跑的总次数上限**。
///
/// 为什么需要它（2026-10-17 第二轮核查，逃生 B）：`claim_boot_resume_attempt`
/// 的标记是**每阶段一个**，而可续跑阶段有 8 个 —— 阶段一推进，计数就归零。
/// 于是「回 Windows → 启动程序 → 又重启进恢复环境」最多能走 8 轮，每轮都重启
/// 用户的机器，而且**全程没问过用户一次**。
/// 这里再加一道**跨阶段**的总闸：累计到上限就**不再自动重启**，改由用户决定
/// （继续 / 放弃）。两者是互补关系，不是替代：per-stage 标记防「同一阶段反复
/// 重启」，总闸防「阶段推进式地被反复重启」。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const MAX_TOTAL_RESUME_ATTEMPTS: u32 = 3;

/// 纯判据：累计已重启 `count` 次之后，是否还允许下一次自动续跑。
/// `count` 从 0 起算（0 = 一次都没重启过）。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn allow_total_attempt(count: u32, max: u32) -> bool {
    count < max
}

#[cfg(test)]
mod total_resume_attempt_tests {
    use super::{MAX_TOTAL_RESUME_ATTEMPTS, allow_total_attempt};

    /// 默认上限下恰好允许 3 次：第 4 次必须被挡住 —— 这正是「阶段推进式
    /// 反复重启」的总闸（2026-10-17 第二轮核查，逃生 B）。
    #[test]
    fn allows_exactly_the_configured_number_of_reboots() {
        assert!(allow_total_attempt(0, MAX_TOTAL_RESUME_ATTEMPTS));
        assert!(allow_total_attempt(1, MAX_TOTAL_RESUME_ATTEMPTS));
        assert!(allow_total_attempt(2, MAX_TOTAL_RESUME_ATTEMPTS));
        assert!(
            !allow_total_attempt(3, MAX_TOTAL_RESUME_ATTEMPTS),
            "第 4 次必须被挡住"
        );
        assert_eq!(MAX_TOTAL_RESUME_ATTEMPTS, 3);
    }

    /// `max = 0` 的语义是「一次都不自动重启」，不能因为 0 < 0 不成立之外的
    /// 任何理由放行第一次。
    #[test]
    fn zero_max_forbids_every_attempt() {
        assert!(!allow_total_attempt(0, 0));
        assert!(!allow_total_attempt(5, 0));
    }
}

/// 一个「已经授权、但被中断」的待续跑任务。
///
/// 只携带**展示与执行**需要的最小信息：不持有 `Task` 本身，避免调用方拿着
/// 一份可能已经过期的完整记录去写盘。
#[cfg(windows)]
#[derive(Clone)]
pub(crate) struct PendingResume {
    pub task_id: String,
    pub workspace: PathBuf,
    pub task_dir: PathBuf,
    pub operation: Operation,
    pub stage: Stage,
}

/// 跨阶段计数文件（任务目录内，与 per-stage 标记并列）。
#[cfg_attr(not(windows), allow(dead_code))]
const TOTAL_RESUME_MARKER: &str = "boot-resume-total.count";

#[cfg(windows)]
fn total_attempt_count(task_dir: &Path) -> u32 {
    fs::read_to_string(task_dir.join(TOTAL_RESUME_MARKER))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

#[cfg(windows)]
fn bump_total_attempt(task_dir: &Path) -> Result<u32, TaskError> {
    // 【2026-10-18 缺陷修复 L9】`saturating_add`：计数文件若被写成 `u32::MAX`，
    // 此前 debug 会 panic、release 会回绕成 0（等于把总闸整个失效）。
    //
    // 原子落盘（tmp + rename）：此前是 `fs::write` 直写，断电落在「截断」与
    // 「写完」之间会得到一个空/半截文件 → 解析失败按 0 处理 → 总闸同样失效。
    let count = total_attempt_count(task_dir).saturating_add(1);
    let final_path = task_dir.join(TOTAL_RESUME_MARKER);
    let tmp_path = task_dir.join(format!("{TOTAL_RESUME_MARKER}.tmp"));
    fs::write(&tmp_path, count.to_string())?;
    fs::rename(&tmp_path, &final_path)?;
    Ok(count)
}

/// 找出行李：一个「已经授权、但回到 Windows 后没再推进」的任务。
///
/// **只做发现与核验，绝不重启**（2026-10-17 第二轮核查，逃生 A）：此前
/// `resume_pending_boot_task` 一路做到 `shutdown /r`，调用方
/// `native_gui::run()` 拿它当 bool 用，于是用户只是想打开程序、窗口都没见着
/// 就被重启了，界面上也没有任何叫停的手段。现在「要不要重启」交回给调用方，
/// 由它负责询问用户。
#[cfg(windows)]
/// 【2026-10-18 新增：僵尸任务自动收口 guard（B5）】
///
/// 此前「任务停在非终态又永远跑不下去」只能靠人工收口（10-16 那次 `1bb72a5e`
/// 卡在非终态，最后是手工处置的）。收口判据刻意定得极窄，**只在不可能误伤
/// 任何还能继续的任务时才动手**：
///
/// ① 任务处于非终态（`is_incomplete()`）；
/// ② 它的阶段在**破坏性边界之前**（`can_abandon_before_target_write`）——
///    目标卷还没被擦除，此时收口不会留下「已擦除但没应用」的残局；
/// ③ 任务目录里**没有一次性启动项记录**——没有启动项就意味着它**永远不可能**
///    被续跑（续跑链第一步就是读这条记录），继续挂着只会阻塞新任务。
///
/// 三条全中才写 `Failed` 并清理工作产物（证据目录与日志保留，与
/// `abandon-task` 的收口口径一致）。任何一步出错都只记日志、不动任务。
pub(crate) fn collect_orphan_tasks() -> Result<usize, TaskError> {
    let executable = env::current_exe()?;
    let workspace = executable
        .parent()
        .ok_or_else(|| err("executable has no workspace directory"))?;
    let tasks_root = workspace.join("tasks");
    if !tasks_root.is_dir() {
        return Ok(0);
    }
    let store = TaskStore::new(workspace);
    let log = crate::log_root::logs_dir().join("gui.log");
    let mut collected = 0usize;
    for entry in fs::read_dir(&tasks_root)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let task_id = entry.file_name().to_string_lossy().to_string();
        let Ok(mut task) = store.load(&task_id) else {
            continue;
        };
        if !task.status.is_incomplete() || !task.status.can_abandon_before_target_write() {
            continue;
        }
        // 有启动项 = 还能续跑，绝不能碰。
        if crate::boot_entry::ReBootEntry::read(&entry.path())
            .ok()
            .flatten()
            .is_some()
        {
            continue;
        }
        let _ = crate::append_log(
            &log,
            &format!(
                "orphan task guard: task {task_id} stage={:?} has no boot entry and cannot resume; marking Failed",
                task.status
            ),
        );
        if store
            .write_failure(&mut task, 2, "自动收口：无一次性启动项，任务无法续跑")
            .is_err()
        {
            continue;
        }
        let _ = store.cleanup_task_artifacts(&task_id);
        collected += 1;
    }
    Ok(collected)
}

#[cfg(not(windows))]
pub(crate) fn collect_orphan_tasks() -> Result<usize, TaskError> {
    Ok(0)
}

#[cfg(windows)]
pub(crate) fn find_pending_boot_task() -> Result<Option<PendingResume>, TaskError> {
    let executable = env::current_exe()?;
    let workspace = executable
        .parent()
        .ok_or_else(|| err("executable has no workspace directory"))?;
    let tasks_root = workspace.join("tasks");
    if !tasks_root.is_dir() {
        return Ok(None);
    }

    let mut pending = Vec::new();
    for entry in fs::read_dir(&tasks_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let task_id = entry.file_name().to_string_lossy().to_string();
        let status_path = entry.path().join("status.json");
        let Ok(task) = TaskStore::new(workspace).load(&task_id) else {
            continue;
        };
        if !stage_resumable_after_interruption(task.status) || task.validate().is_err() {
            continue;
        }
        let Ok(status) = read_json::<StatusRecord>(&status_path) else {
            continue;
        };
        if status.task_id.eq_ignore_ascii_case(&task.task_id)
            && status.operation == task.operation
            && status.stage == task.status
            && task.task_id.eq_ignore_ascii_case(&task_id)
        {
            pending.push((task, entry.path()));
        }
    }

    if pending.is_empty() {
        return Ok(None);
    }
    if pending.len() != 1 {
        let log = workspace.join("logs").join("launcher-errors.log");
        append_log(
            &log,
            &format!(
                "Pending recovery not resumed: {} valid resumable tasks found",
                pending.len()
            ),
        )?;
        return Ok(None);
    }

    let (mut task, task_dir) = pending.pop().expect("pending length checked above");
    if task.status == Stage::RecoveryComplete {
        complete_recovery_cleanup(
            &TaskStore::new(workspace),
            &mut task,
            &task_dir.join("prepare.log"),
        )?;
        return Ok(None);
    }
    Ok(Some(PendingResume {
        task_id: task.task_id.clone(),
        workspace: workspace.to_path_buf(),
        task_dir,
        operation: task.operation.clone(),
        stage: task.status,
    }))
}

/// 立即重启去续跑这个任务。返回 `Ok(true)` = 重启已发起；`Ok(false)` = 被闸门
/// 挡住、没有重启（per-stage 标记或跨阶段总闸），现场完整保留。
#[cfg(windows)]
pub(crate) fn resume_pending_now(pending: &PendingResume) -> Result<bool, TaskError> {
    let log = pending.task_dir.join("prepare.log");
    let attempts = total_attempt_count(&pending.task_dir);
    if !allow_total_attempt(attempts, MAX_TOTAL_RESUME_ATTEMPTS) {
        append_log(
            &log,
            &crate::i18n::msg_tpl(
                "resume_limit_reached",
                &[
                    &MAX_TOTAL_RESUME_ATTEMPTS.to_string(),
                    &attempts.to_string(),
                ],
            ),
        )?;
        return Ok(false);
    }
    if !claim_boot_resume_attempt(&pending.task_dir, pending.stage)? {
        append_log(
            &log,
            &format!(
                "Automatic WinRE resume suppressed: stage {:?} already had its one retry; manual inspection is required",
                pending.stage
            ),
        )?;
        // 自动重试次数限制只停止自动重启，绝不删除唯一的恢复入口。
        append_log(
            &log,
            crate::i18n::msg("resume_paused"),
        )?;
        return Ok(false);
    }
    for required in [
        pending.task_dir.join("payload").join("Recovery.exe"),
        pending.task_dir.join("payload").join("RecoveryTask.env"),
        pending.task_dir.join("payload").join("task.json"),
        pending.task_dir.join("manifest.json"),
        pending.task_dir.join("original").join("Winre.wim"),
        pending.task_dir.join("stage").join("Winre.wim"),
    ] {
        if !required.is_file() {
            append_log(
                &log,
                &format!(
                    "Pending boot recovery refused: required artifact is missing: {}",
                    required.display()
                ),
            )?;
            release_boot_resume_attempt(&pending.task_dir, pending.stage)?;
            return Ok(false);
        }
    }

    // 【2026-10-18 缺陷修复 M1】计数**不再在重启之前**落盘。
    // 此前 `bump_total_attempt` 在 `shutdown` 之前就 +1，而 shutdown 失败只回滚
    // per-stage 标记、不回滚总计数：于是「连续三次重启失败、机器一次都没重启」
    // 也会把总闸烧到上限 → 自动续跑被永久关闭，日志却只写「已达总上限」，
    // 看不出其实一次都没重启过。现在只在 `shutdown` 真的成功之后才计数。
    let planned = total_attempt_count(&pending.task_dir).saturating_add(1);
    append_log(
        &log,
        &crate::i18n::msg_tpl(
            "resume_detected",
            &[
                &pending.task_id,
                &format!("{:?}", pending.operation),
                &format!("{:?}", pending.stage),
                &planned.to_string(),
                &MAX_TOTAL_RESUME_ATTEMPTS.to_string(),
            ],
        ),
    )?;
    // Invariant A before re-arming: the registered WinRE must still be this
    // task's payload, otherwise the reboot lands in the stock Microsoft WinRE
    // (no winpeshl hook) and the interrupted task silently never resumes.
    // v1.7.11 新通道：续跑只依赖「我们自建的 BCD 条目 + 镜像卷上的载荷 WIM」，
    // 与系统注册 WinRE 无关（注册位全程只读）。
    let Some(entry) = boot_entry::ReBootEntry::read(&pending.task_dir)? else {
        append_log(
            &log,
            "Pending boot recovery abandoned: task has no recorded boot entry (old WinRE-channel task)",
        )?;
        release_boot_resume_attempt(&pending.task_dir, pending.stage)?;
        return Ok(false);
    };
    if let Err(error) = boot_entry::rearm(&entry, &log) {
        append_log(
            &log,
            &format!("Pending boot recovery abandoned: our boot entry is unusable: {error}"),
        )?;
        release_boot_resume_attempt(&pending.task_dir, pending.stage)?;
        return Ok(false);
    }
    append_log(&log, "Re-armed our one-shot boot entry for pending task")?;
    // 【2026-10-16 审查修订，日志缺口 6】三个流全部落盘：此前 shutdown 的三流
    // 都是 `Stdio::null()`，重启失败时只剩一个退出码，完全不知道 Windows
    // 为什么拒绝重启（权限不足 / 有挂起的更新 / 别的会话占着）。
    let shutdown = run_output_logged("shutdown.exe", &["/r", "/t", "0"], &log)?;
    if !shutdown.status.success() {
        let tail = output_tail_summary(&shutdown, 6);
        let detail = if tail.is_empty() {
            String::new()
        } else {
            format!("：{tail}")
        };
        append_log(
            &log,
            &format!(
                "Pending boot recovery failed: shutdown exited {}{}",
                shutdown.status, detail
            ),
        )?;
        release_boot_resume_attempt(&pending.task_dir, pending.stage)?;
        return Err(err(&format!(
            "unable to restart into Windows RE for pending task{}",
            detail
        )));
    }
    // 重启确实发出去了，此刻才把「跨阶段第 N 次」记进计数（M1：失败不计数）。
    let count = bump_total_attempt(&pending.task_dir)?;
    append_log(
        &log,
        &crate::i18n::msg_tpl(
            "resume_restart_requested",
            &[&count.to_string(), &MAX_TOTAL_RESUME_ATTEMPTS.to_string()],
        ),
    )?;
    Ok(true)
}

/// 用户选择「本次不重启」：只记一行，任务与载荷原样保留。
#[cfg(windows)]
pub(crate) fn defer_pending_task(pending: &PendingResume) -> Result<(), TaskError> {
    append_log(
        &pending.task_dir.join("prepare.log"),
        &crate::i18n::msg_tpl(
            "resume_declined",
            &[&pending.task_id, &format!("{:?}", pending.stage)],
        ),
    )
}

/// 用户选择「放弃这个任务」：走 `abandon-task` 那条已经实机验收过的终态收口
/// （disarm + 清载荷 + 保留证据），不在这里另写一份清理逻辑。
#[cfg(windows)]
pub(crate) fn abandon_pending_task(pending: &PendingResume) -> Result<(), TaskError> {
    append_log(
        &pending.task_dir.join("prepare.log"),
        &crate::i18n::msg_tpl(
            "resume_abandoned",
            &[&pending.task_id, &format!("{:?}", pending.stage)],
        ),
    )?;
    windows_prepare::abandon_task(
        pending.workspace.to_string_lossy().into_owned(),
        pending.task_id.clone(),
    )
}

/// A prepared task may be a deliberate `--no-reboot` diagnostic and must not
/// start WinRE by itself. Every later non-terminal stage, however, represents
/// an already authorized recovery that was interrupted after WinRE staging;
/// it is safe to re-request the one-time WinRE boot after artifact validation.
#[cfg(any(windows, test))]
fn stage_resumable_after_interruption(stage: Stage) -> bool {
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
fn index_metadata_path(image: &Path, index: u32) -> Result<PathBuf, TaskError> {
    backuprestore_core::image_metadata::index_metadata_path(image, index)
}

#[cfg(windows)]
fn legacy_metadata_path(image: &Path) -> Result<PathBuf, TaskError> {
    backuprestore_core::image_metadata::legacy_metadata_path(image)
}

#[cfg(windows)]
fn read_index_metadata(
    image: &Path,
    index: u32,
) -> Result<backuprestore_core::BackupMetadata, TaskError> {
    backuprestore_core::image_metadata::read_index_metadata(image, index)
}

/// 解析卷内工作区相对路径；从最后一个 tasks 分隔符切分，允许上层目录也叫 tasks。
#[cfg(any(windows, test))]
fn split_workspace_root_rel(value: &str, task_id: &str) -> Result<String, TaskError> {
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

#[cfg(not(windows))]
fn recover_env(_path: String) -> Result<(), TaskError> {
    Err(err(
        "recover-env is only available on Windows/WinRE; use recover --dry-run on this host",
    ))
}

#[cfg(not(windows))]
fn recover_env_interactive(path: String) -> Result<(), TaskError> {
    recover_env(path)
}

#[cfg(windows)]
fn recover_env_interactive(path: String) -> Result<(), TaskError> {
    // 策略与逃生日志先备好：入口探测若失败，必须能"落盘 + 走 auto_escape"，
    // 不能再 `?` 直接 return Err——那会让进程 exit(1)、一次性启动项（bootsequence）
    // 仍指向本恢复环境，下次重启又进 RE 又失败，无限打转（2026-10-16 审查，
    // docs/20261016-2250 卡死 7：旧代码两个 `?` 都在恢复循环之外）。
    let values = read_env_file(&path).unwrap_or_default();
    let policy = crate::failure_policy::FailurePolicy::from_values(&values);
    let language = ui_language(&values);
    // 恢复环境里的语言来自 `UI_LANGUAGE`（由 `prepare --ui-language` 写进
    // `RecoveryTask.env`）。同步给全局语言层，本进程后续写到日志里的
    // 结论性文案就跟着这个语言走——此前只有弹窗文案跟、日志一律中文。
    crate::i18n::set(language);
    let escape_log = PathBuf::from(r"X:\BackupRestore-escape.log");
    let _ = append_log(&escape_log, &policy.log_line(0));
    // 自定义恢复入口没有经过系统恢复桌面；先枚举即插即用设备，
    // 否则 ARM64 恢复环境中的键盘可能尚未就绪，错误窗口无法操作。
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"X:\Windows".into());
    let init_log = PathBuf::from(&system_root).join("Temp/BackupRestore-init.log");
    // 【2026-10-16 审查修订，日志缺口 3】三个流的输出全部落盘，失败不再静默：
    // 此前 `reg query` 只按退出码决定是否跑 wpeinit，退出码本身与它自己的
    // stdout/stderr 全丢；wpeinit 的输出进 Stdio::null()。WinRE 里"键盘/设备
    // 未就绪"这类故障事后完全无从判断走了哪条分支、失败原因是什么。
    match std::process::Command::new("reg.exe")
        .args(["query", r"HKLM\SYSTEM\CurrentControlSet\Control\MiniNT"])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) if output.status.success() => {
            // 恢复环境里：wpeinit 负责枚举即插即用设备（含键盘）。它失败
            // 意味着后面的错误弹框可能根本无法操作，属确定性失败。
            if let Err(error) = run_logged("wpeinit.exe", &[], &init_log) {
                let _ = append_log(
                    &escape_log,
                    &format!(
                        "recovery entry failed: wpeinit did not run ({error}); \
                         auto-escaping instead of returning so the machine cannot boot-loop back into WinRE"
                    ),
                );
                auto_escape_to_windows(&path, &escape_log);
                return Err(error);
            }
        }
        Ok(output) => {
            let _ = append_log(
                &escape_log,
                &format!(
                    "reg query MiniNT exited with {} ({}); skipping wpeinit",
                    output.status,
                    crate::output_tail_summary(&output, 4)
                ),
            );
        }
        Err(error) => {
            let _ = append_log(
                &escape_log,
                &format!("reg query MiniNT could not be started ({error}); skipping wpeinit"),
            );
        }
    }
    // 【2026-10-17 用户要求】一进恢复环境就在后台开一个最大化的命令提示符：
    // WinRE 是 shell-free 的，没有它，用户（和我们自己排障时）没有任何命令行
    // 出口。它留在后台，本程序的弹框用 MB_SETFOREGROUND 保持在前台。
    // 失败只记日志：cmd 起不来不该让恢复链跟着失败。
    if let Err(error) = launch_background_console() {
        let _ = append_log(
            &escape_log,
            &format!("recovery console could not be started ({error}); continuing without it"),
        );
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            window: *mut std::ffi::c_void,
            text: *const u16,
            title: *const u16,
            flags: u32,
        ) -> i32;
        // 带超时的 MessageBox。`wLanguageId=0`（用线程默认语言），
        // `dwMilliseconds=0` 表示立即超时并返回 0。user32 一直导出它
        // （自 XP 起），是给无人值守场景加超时的标准手段。
        fn MessageBoxTimeoutW(
            window: *mut std::ffi::c_void,
            text: *const u16,
            title: *const u16,
            flags: u32,
            language_id: u16,
            milliseconds: u32,
        ) -> i32;
    }
    // 同一条恢复链连续失败的次数。用于「连败 N 次后如何处理」：
    // 恢复环境里的一次性启动项（bootsequence）在失败后仍指向本恢复环境，
    // 若用户每次都选「重试」而问题又是确定性的，机器会在「失败 → 重启 → 又失败」
    // 之间无限打转（2026-10-16 实测：GUI 与 RE 都没给用户任何自救手段，
    // 只能从宿主侧挂 PE 光盘手改 BCD 才逃得出来）。
    //
    // 【2026-10-16 用户拍板：次数与处理方式都做成设置项】次数（默认 3）与
    // 「达阈值后怎么处理」由用户在 Windows 端程序里设置，随任务落进
    // `RecoveryTask.env`（`FAILURE_THRESHOLD` / `FAILURE_ACTION`）：
    //   - auto-escape（默认）：达阈值直接自动逃出，不等用户——即 v2.6.8 的行为。
    //       ①清掉本任务的一次性启动项（disarm）②把任务写成终态（现场保留）
    //       ③重启回正常系统，让机器回到一个可用的 Windows，而不是卡在恢复环境里。
    //   - pause：达阈值**不自动处理**，弹框停在恢复环境里（弹框是模态的，机器
    //       会一直等），由用户按按钮手动执行与 auto-escape 完全相同的那套处理。
    //
    // 判据与文案的**单一来源**是 `crate::failure_policy`（GUI 设置窗口 /
    // prepare 落盘 / 这里三处共用），本函数不再自己写一份阈值或文案——
    // 项目有 P3「文案分叉」前科（GUI 曾复制一份硬编码的双语硬拒文案）。
    //
    // 第 ② 步的终态标记是关键：只 disarm 不标终态的话，任务会停在 TargetErased
    // 等可续跑阶段，下一次从**任务所在 workspace** 启动程序时
    // `resume_pending_boot_task` 会再武装一次并重启回 RE
    // （`boot-resume-{stage}.attempted` 每阶段只挡一次），而 `abandon-task`
    // 在擦除后又拒绝清理 → 任务永久悬置、用户仍无自救出口。
    // （`values` / `policy` / `language` / `escape_log` 已在本函数开头备好。）
    let mut consecutive_failures: u32 = 0;
    // 第一次进入 pause 分支的时刻。`None` = 没配等待上限（历史行为，无限等）。
    let mut pause_deadline: Option<Instant> = None;
    loop {
        match recover_env(path.clone()) {
            Ok(()) => return Ok(()),
            Err(error) => {
                consecutive_failures += 1;
                let _ = append_log(&escape_log, &policy.log_line(consecutive_failures));
                // 策略 auto-escape：连败达阈值 = 确定性失败，直接自动逃出，不询问用户。
                if policy.auto_escape_now(consecutive_failures) {
                    let _ = append_log(
                        &escape_log,
                        &format!(
                            "consecutive failures reached the auto-escape threshold ({}); \
                             auto-escape: disarm one-shot boot entry, mark task terminal, reboot to Windows",
                            policy.threshold
                        ),
                    );
                    auto_escape_to_windows(&path, &escape_log);
                    // auto_escape_to_windows 内部会重启；走到这里说明重启失败，
                    // 仍要把错误如实返回，让上层记录日志。
                    return Err(error);
                }
                // 策略 pause：达阈值**不自动处理**，弹框停在恢复环境等用户按按钮。
                // 「是」= 立即执行与 auto-escape 完全相同的处理；「否」= 继续重试。
                // 弹框是模态的：不点就一直等，这就是「暂停在恢复环境里」的语义。
                //
                // 【2026-10-16 审查修订，卡死 1】配了 `pause_max_wait_secs` 时改用
                // `MessageBoxTimeoutW`：超时（返回 0）执行与「是」完全相同的清理。
                // 默认 0 = 不限制，仍用 `MessageBoxW` 无限等——**行为与旧版一致**。
                // 上限从第一次进入暂停态起算（`pause_deadline`），不是每次弹框各自计时。
                if policy.pause_now(consecutive_failures) {
                    let deadline = match pause_deadline {
                        Some(deadline) => Some(deadline),
                        None => {
                            let deadline = policy
                                .has_pause_wait_limit()
                                .then(|| {
                                    Instant::now()
                                        + Duration::from_secs(policy.pause_max_wait_secs as u64)
                                });
                            pause_deadline = deadline;
                            deadline
                        }
                    };
                    let mut text = policy.pause_prompt(
                        &error.to_string(),
                        consecutive_failures,
                        language,
                    );
                    text.push('\0');
                    let text: Vec<u16> = text.encode_utf16().collect();
                    let title: Vec<u16> = crate::failure_policy::FailurePolicy::prompt_title(language)
                        .encode_utf16()
                        .collect();
                    // MB_YESNO|MB_ICONWARNING|MB_SETFOREGROUND：
                    // 恢复环境没有桌面外壳帮错误窗口取得键盘焦点。
                    const MB_YESNO_WARN_FOREGROUND: u32 = 0x10034;
                    // 剩余毫秒；已到期 → 0（MessageBoxTimeoutW 的 0 表示立即超时返回）。
                    let remaining_ms = deadline
                        .map(|deadline| {
                            deadline
                                .saturating_duration_since(Instant::now())
                                .as_millis()
                                .min(u32::MAX as u128) as u32
                        })
                        .unwrap_or(u32::MAX);
                    let result = if deadline.is_some() {
                        let _ = append_log(
                            &escape_log,
                            &format!(
                                "pause prompt waits at most {remaining_ms} ms (limit {} sec); \
                                 timeout runs the same cleanup as Yes",
                                policy.pause_max_wait_secs
                            ),
                        );
                        unsafe {
                            MessageBoxTimeoutW(
                                std::ptr::null_mut(),
                                text.as_ptr(),
                                title.as_ptr(),
                                MB_YESNO_WARN_FOREGROUND,
                                0,
                                remaining_ms,
                            )
                        }
                    } else {
                        unsafe {
                            MessageBoxW(
                                std::ptr::null_mut(),
                                text.as_ptr(),
                                title.as_ptr(),
                                MB_YESNO_WARN_FOREGROUND,
                            )
                        }
                    };
                    // 6 = IDYES；0 = 超时（MessageBoxTimeoutW 的超时返回值）。
                    // 两者执行完全相同的清理：超时在无人值守时就是"没人来按按钮"。
                    if result == 6 || result == 0 {
                        let _ = append_log(
                            &escape_log,
                            &format!(
                                "{} after {consecutive_failures} failure(s) (pause policy); \
                                 auto-escape: disarm one-shot boot entry, mark task terminal, \
                                 reboot to Windows",
                                if result == 6 {
                                    "user requested manual cleanup"
                                } else {
                                    "pause wait limit expired with no user input"
                                }
                            ),
                        );
                        auto_escape_to_windows(&path, &escape_log);
                        // auto_escape_to_windows 内部会重启；走到这里说明重启失败。
                        return Err(error);
                    }
                    // 用户选「否」= 继续重试：直接进入下一轮，不再弹「重试/取消」框
                    //（阈值已到，那个框的语义已被本框覆盖）。
                    let _ = append_log(
                        &escape_log,
                        &format!(
                            "user chose to keep retrying after {consecutive_failures} failure(s) \
                             (pause policy)"
                        ),
                    );
                    continue;
                }
                // 未达阈值：给用户「重试 / 放弃」的选择。
                let mut text = policy.retry_prompt(
                    &error.to_string(),
                    consecutive_failures,
                    language,
                    // 按过 B 回退时，续跑链校验必然失败、重试也大概率再失败——
                    // 把这点写进对话框，让用户带着信息选（2026-10-23 用户拍板：
                    // 不自动终态，只把信息说清）。
                    crate::recovery_progress::bcd_rolled_back(),
                );
                text.push('\0');
                let text: Vec<u16> = text.encode_utf16().collect();
                let title: Vec<u16> = crate::failure_policy::FailurePolicy::prompt_title(language)
                    .encode_utf16()
                    .collect();
                let result = unsafe {
                    // MB_SETFOREGROUND：恢复环境没有桌面外壳帮错误窗口取得键盘焦点。
                    MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), 0x10035)
                };
                // 选“重试”（IDRETRY=4）继续；其余（含取消、关闭窗口）走自动逃出。
                if result != 4 {
                    let _ = append_log(
                        &escape_log,
                        &format!(
                            "user declined retry after {consecutive_failures} failure(s); \
                             auto-escape: disarm one-shot boot entry, mark task terminal, reboot to Windows"
                        ),
                    );
                    auto_escape_to_windows(&path, &escape_log);
                    // auto_escape_to_windows 内部会重启；走到这里说明重启失败，
                    // 仍要把错误如实返回，让上层记录日志。
                    return Err(error);
                }
            }
        }
    }
}

/// 恢复环境里那个「最大化、留在后台」的命令提示符。
///
/// ## 为什么要有它（2026-10-17 用户要求 + 第二轮核查逃生 C）
/// WinRE / WinPE 是 **shell-free** 的：`winpeshl.ini` 直接拉起本程序，屏幕上
/// 只有我们的窗口 —— 没有任务栏、没有开始菜单、也没有任何命令行。于是：
/// - 用户想手动查 BCD / 看盘符 / 跑 `wpeutil`，**没有任何入口**；
/// - 一旦我们的窗口起不来（建窗失败、panic），这台机器就成了黑屏。
///
/// 这里在后台开一个**最大化**的 cmd 当常驻退路；本程序的窗口保持在前台，
/// 不会被它抢走。
///
/// 刻意用 `CreateProcessW` 而不是 `Command`：`Command` 设不了窗口显示状态，
/// 而「最大化」是这个需求的一半（另一半是「在后台」）。
///
/// ## 2026-10-20 修复：「在后台」不能只靠创建标志（实机三次复现）
///
/// 旧实现用 `STARTF_USESHOWWINDOW` + `SW_SHOWMAXIMIZED` 创建。根据 Win32
/// 文档，`SW_SHOWMAXIMIZED` **会激活窗口**——cmd 一起手就抢走前台，并且压在
/// 随后才创建的进度窗之上：2026-10-19/20 三次实机（任务 2355190e /
/// 6bf2ff22 / 7924c644）恢复全程 cmd 盖住「BackupRestore 恢复进度」窗，
/// 键盘（B/L/M/K/P）全部打在控制台上。`MB_SETFOREGROUND` 只保得住模态框，
/// 普通主窗抢不过被激活的 cmd。
///
/// 现在的顺序：
/// ① 创建时用 `SW_SHOWMINNOACTIVE`（最小化且**不激活**，不抢前台）；
/// ② 轮询等 cmd 的顶层窗口真正建出来（进程起来 ≠ 窗口就绪，按 PID 匹配）；
/// ③ `ShowWindow(SW_SHOWMAXIMIZED)` 放大；
/// ④ `SetWindowPos(HWND_BOTTOM, SWP_NOACTIVATE)` 压到 z 序最底层。
/// 前台则由进度窗创建后显式 `SetForegroundWindow` 抢回（见
/// `recovery_progress::run_window`）——那里才是用户按 B 键的地方。
#[cfg(windows)]
pub(crate) fn launch_background_console() -> Result<(), TaskError> {
    #[repr(C)]
    struct StartupInfoW {
        cb: u32,
        reserved: *mut u16,
        desktop: *mut u16,
        title: *mut u16,
        x: u32,
        y: u32,
        x_size: u32,
        y_size: u32,
        x_count_chars: u32,
        y_count_chars: u32,
        fill_attribute: u32,
        flags: u32,
        show_window: u16,
        reserved2: u16,
        reserved2_ptr: *mut u8,
        std_input: *mut std::ffi::c_void,
        std_output: *mut std::ffi::c_void,
        std_error: *mut std::ffi::c_void,
    }
    #[repr(C)]
    struct ProcessInformation {
        process: *mut std::ffi::c_void,
        thread: *mut std::ffi::c_void,
        process_id: u32,
        thread_id: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateProcessW(
            application_name: *const u16,
            command_line: *mut u16,
            process_attributes: *mut std::ffi::c_void,
            thread_attributes: *mut std::ffi::c_void,
            inherit_handles: i32,
            creation_flags: u32,
            environment: *mut std::ffi::c_void,
            current_directory: *const u16,
            startup_info: *mut StartupInfoW,
            process_information: *mut ProcessInformation,
        ) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    // 「最大化 + 后台」这一半在 user32：ShowWindow 放大、SetWindowPos 压底；
    // 窗口就绪等待（按 PID 找 cmd 顶层窗口）见 wait_for_toplevel_window。
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindow(hwnd: *mut std::ffi::c_void, command: i32) -> i32;
        fn SetWindowPos(
            hwnd: *mut std::ffi::c_void,
            insert_after: *mut std::ffi::c_void,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
    }
    const STARTF_USESHOWWINDOW: u32 = 0x0000_0001;
    // ⑦ 最小化且不激活（SW_SHOWMINNOACTIVE）——与 ③ 的 SW_SHOWMAXIMIZED 是
    // 两个不同的值，别合并：前者管「创建时别抢前台」，后者管「之后放大」。
    const SW_SHOWMINNOACTIVE: u16 = 7;
    const SW_SHOWMAXIMIZED: i32 = 3;
    const HWND_BOTTOM: *mut std::ffi::c_void = 1 as *mut _;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    // `/k` 让窗口留在屏幕上（不是跑完就消失）；标题只作提示，不参与判据。
    let mut command_line: Vec<u16> = r#"cmd.exe /k title BackupRestore recovery console"#
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut startup: StartupInfoW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<StartupInfoW>() as u32;
    startup.flags = STARTF_USESHOWWINDOW;
    startup.show_window = SW_SHOWMINNOACTIVE;
    let mut info: ProcessInformation = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            std::ptr::null(),
            command_line.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            CREATE_NEW_CONSOLE,
            std::ptr::null_mut(),
            std::ptr::null(),
            &mut startup,
            &mut info,
        )
    };
    if ok == 0 {
        return Err(errc(ErrCode::SpawnConsoleFailed));
    }
    let console_pid = info.process_id;
    unsafe {
        CloseHandle(info.thread);
        CloseHandle(info.process);
    }
    // 等 cmd 的顶层窗口就绪后「最大化 + 压底」。窗口没拿到也不失败：控制台
    // 只是个退路，进程已经活着（最坏情况是它以最小化待在任务栏外）。
    match wait_for_toplevel_window(console_pid, std::time::Duration::from_secs(3)) {
        Some(console) => unsafe {
            // 先放大再压底：ShowWindow(SW_SHOWMAXIMIZED) 自身会激活，压底一步
            // 把它按回 z 序最底层；SWP_NOACTIVATE 保证这一步不再激活它。
            ShowWindow(console, SW_SHOWMAXIMIZED);
            SetWindowPos(
                console,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        },
        None => {
            return Err(errc(ErrCode::ConsoleWindowTimeout));
        }
    }
    Ok(())
}

/// 轮询等待 `pid` 进程的**可见顶层窗口**出现，超时返回 `None`。
///
/// 为什么必须等：`CreateProcessW` 返回时 cmd 的窗口往往还没建出来（conhost
/// 初始化要几十到几百毫秒），直接 `ShowWindow` 会打在一个空句柄上。窗口就绪
/// 后才有「最大化」可谈——这是「在后台最大化」需求的第二半。
#[cfg(windows)]
fn wait_for_toplevel_window(
    pid: u32,
    timeout: std::time::Duration,
) -> Option<*mut std::ffi::c_void> {
    // EnumWindows 回调不能捕获环境，用进程级静态传参。本函数只在恢复环境
    // 启动路径调用一次，静态冲突风险为零；仍然在拿到结果后立即复位。
    static TARGET_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    static FOUND_HWND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    unsafe extern "system" fn enum_callback(hwnd: *mut std::ffi::c_void, _lparam: isize) -> i32 {
        let target = TARGET_PID.load(std::sync::atomic::Ordering::SeqCst);
        if target == 0 {
            return 0;
        }
        let mut window_pid: u32 = 0;
        let visible = unsafe { IsWindowVisible(hwnd) } != 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut window_pid) };
        if visible && window_pid == target {
            FOUND_HWND.store(hwnd as usize, std::sync::atomic::Ordering::SeqCst);
            return 0; // 命中即停：继续枚举只会拿到同进程的其它零碎窗口
        }
        1
    }
    // 签名必须与 native_gui.rs 里那份同 crate 声明逐字一致（回调也要包
    // Option），否则触发 clashing_extern_declarations 警告。
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(*mut std::ffi::c_void, isize) -> i32>,
            lparam: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(hwnd: *mut std::ffi::c_void, process_id: *mut u32) -> u32;
        fn IsWindowVisible(hwnd: *mut std::ffi::c_void) -> i32;
    }
    TARGET_PID.store(pid, std::sync::atomic::Ordering::SeqCst);
    FOUND_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
    let deadline = std::time::Instant::now() + timeout;
    let found = loop {
        unsafe { EnumWindows(Some(enum_callback), 0) };
        let found = FOUND_HWND.load(std::sync::atomic::Ordering::SeqCst);
        if found != 0 {
            break Some(found as *mut std::ffi::c_void);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    TARGET_PID.store(0, std::sync::atomic::Ordering::SeqCst);
    FOUND_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
    found
}

/// 从 `RecoveryTask.env` 读界面语言：`UI_LANGUAGE=en*` 用英文，其余一律中文
/// （读不到时保持历史行为 = 中文）。
///
/// 为什么要跟着任务走：GUI 的语言选择是**会话级**的、不落盘；如果恢复环境
/// 固定中文，英文界面的用户会在恢复环境里突然看到中文提示。prepare 创建任务时
/// 把当时的界面语言写进 env，恢复环境就能与用户当时看到的一致。
#[cfg(windows)]
fn ui_language(values: &BTreeMap<String, String>) -> crate::pe_safety::MessageLanguage {
    match values
        .get("UI_LANGUAGE")
        .map(|value| value.trim().to_ascii_lowercase())
    {
        Some(value) if value.starts_with("en") => crate::pe_safety::MessageLanguage::English,
        _ => crate::pe_safety::MessageLanguage::Chinese,
    }
}


/// 恢复环境失败后「自动逃出」：清掉本任务的一次性启动项、删除本任务载荷，
/// 然后重启回正常 Windows，避免机器卡在恢复环境里无限重启。
///
/// 为什么需要它：`bootsequence` 在恢复链失败后**仍指向恢复环境**，用户每次选
/// 「重试」都会再次进入同一个失败。历史上唯一的逃出办法是从宿主侧挂 PE 光盘、
/// 手改 BCD（`docs/20260916-0759`），对终端用户不可行。
///
/// 安全边界：只动**本任务**的 BCD 对象与载荷（按精确 GUID），绝不碰别的启动项；
/// 清理失败也仍然尝试重启（宁可回到 Windows 让用户看到问题，也不要卡在 RE）。
#[cfg(windows)]
fn auto_escape_to_windows(env_path: &str, log: &Path) {
    // 尽力而为：任何一步失败都只记日志，绝不阻断最后的重启。
    let _ = (|| -> Result<(), TaskError> {
        let values = read_env_file(env_path)?;
        let task_id = env_required(&values, "TASK_ID")?;
        let workspace_root_rel = env_required(&values, "WORKSPACE_ROOT_REL")?;
        let store_rel = split_workspace_root_rel(&workspace_root_rel, &task_id)?;
        let mut mounts = MountedVolumes::new();
        let letter = mount_env_volume(&mut mounts, &values, "WORKSPACE", 'T', log, false)?;
        let store = TaskStore::new(PathBuf::from(format!(r"{}:\{store_rel}", letter)));
        let task_dir = store.task_dir(&task_id)?;
        if let Some(entry) = boot_entry::ReBootEntry::read(&task_dir)? {
            boot_entry::disarm(&entry, log)?;
            let _ = append_log(log, "auto-escape: one-shot boot entry disarmed");
        }
        // 【终态收口，2026-10-16 用户拍板】把任务写成终态（Failed），现场全部保留。
        // 为什么必须做：见 `recover_env_interactive` 的注释——只 disarm 不标终态，
        // 任务会停在 TargetErased 等可续跑阶段，下次从**任务所在 workspace** 启动
        // 程序时又会被自动续跑一次（`boot-resume-{stage}.attempted` 每阶段只挡一次），
        // 而 `abandon-task` 在擦除后拒绝清理 → 任务永久悬置、用户没有自救出口。
        // 标成 Failed 后：①不再被自动续跑 ②可用 abandon-task 收口清理
        // ③任务目录/日志/BCD 证据全部保留，供人工诊断。
        match store.load(&task_id) {
            Ok(mut task) if task.status.is_incomplete() => {
                match store.write_failure(
                    &mut task,
                    1,
                    "恢复在恢复环境中连续失败，已自动逃出并标记为终态（任务与现场保留，可人工诊断或清理）",
                ) {
                    Ok(_) => {
                        let _ = append_log(
                            log,
                            "auto-escape: task marked terminal (Failed); evidence retained",
                        );
                    }
                    Err(error) => {
                        let _ = append_log(
                            log,
                            &format!("auto-escape: failed to mark task terminal: {error}"),
                        );
                    }
                }
            }
            Ok(_) => {
                let _ = append_log(log, "auto-escape: task already terminal; left as-is");
            }
            Err(error) => {
                let _ = append_log(
                    log,
                    &format!("auto-escape: cannot load task to mark terminal: {error}"),
                );
            }
        }
        Ok(())
    })();
    // 无论清理是否成功都重启：回到 Windows 总比卡在 RE 里好。
    let _ = run_logged("wpeutil.exe", &["reboot"], log);
}

#[cfg(windows)]
fn recover_env(path: String) -> Result<(), TaskError> {
    use backuprestore_core::{validate_payload_files, validate_task_id};

    // === 持久早期日志 ===
    // 【2026-10-18 缺陷修复 L4】此前硬编码 `C:\…` 与 `C:\WinRE-PoC\…`：
    // WinRE 里 C: 常常正是**离线 Windows = 还原目标卷**，这些「跨重启可回读」
    // 的日志会在随后的格式化中被一起抹掉，与 v2.6.15 定下的「日志根不落在
    // PE、不落在目标卷」（`log_root::logs_dir()`）自相矛盾。现在统一走日志根。
    let persistent_logs = [crate::log_root::logs_dir().join("recovery-early.log")];
    let meta_log = |line: &str| {
        use std::io::Write;
        for p in &persistent_logs {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
            {
                let _ = writeln!(f, "{line}");
            }
        }
    };
    meta_log(&format!("=== recover-env START path='{path}' ==="));

    let values = match read_env_file(&path) {
        Ok(v) => v,
        Err(e) => {
            meta_log(&format!("read_env_file FAILED: {e}"));
            return Err(e);
        }
    };
    let task_id = env_required(&values, "TASK_ID")?;
    validate_task_id(&task_id)?;
    // 【2026-10-16 审查修订，卡死 2/3/6】把任务级的 DISM 等待上限灌进进程环境变量，
    // 让本进程内所有 DISM 调用（`pe_operation::command` 的 long 分支、
    // `online_operation::dism`、`native_gui` 的 PE 安装）都能读到。
    // 恢复环境是**无人值守**的：只能靠任务自带的值，不能指望谁记得设环境变量。
    // 值为 0（默认/未设置）时同样显式设一次，避免父进程遗留的旧值干扰。
    {
        let secs = crate::dism_timeout::from_values(&values);
        // set_var 在 2024 edition 里是 unsafe（多线程并发修改环境不安全）；
        // 此刻还在单线程入口，且必须在任何子进程派生之前完成。
        unsafe { std::env::set_var(crate::dism_timeout::ENV_VAR, secs.to_string()) };
        meta_log(&format!("DISM wait limit: {secs} sec (0 = unlimited)"));
    }
    // 【2026-10-17 修复】暂停总上限同样灌进进程环境变量。此前
    // `recovery_progress::wait_if_paused` 读的 `PAUSE_TIMEOUT_ENV_VAR` 全仓没有
    // 写入方 → 永久 0（不限制），用户在设置里配的 `failure_pause_max_wait_secs`
    // 对「暂停后无人点继续」这一路完全不生效；只有失败弹框那条路
    // （`MessageBoxTimeoutW`）用到了它。这里让两条路**同源**。
    {
        let policy = crate::failure_policy::FailurePolicy::from_values(&values);
        let secs = policy.pause_timeout_env_value();
        // 0 也要显式写：避免父进程遗留的旧值干扰（与 DISM 同一个理由）。
        unsafe { std::env::set_var(crate::failure_policy::PAUSE_TIMEOUT_ENV_VAR, secs.to_string()) };
        meta_log(&format!("pause total limit: {secs} sec (0 = unlimited)"));
    }
    let workspace_root_rel = env_required(&values, "WORKSPACE_ROOT_REL")?;
    let store_rel = split_workspace_root_rel(&workspace_root_rel, &task_id)?;
    meta_log(&format!(
        "TASK_ID={task_id} workspace_root_rel={workspace_root_rel}"
    ));
    // Until the workspace volume is mounted only an ephemeral WinRE path is
    // available.  Switch to the task directory immediately after mounting;
    // all task/recovery logs that survive WinRE are stored there.
    let mut early_log = PathBuf::from(r"X:\BackupRestore-Recovery-early.log");
    let operation_hint = values.get("OPERATION").map(|s| s.as_str());
    // WinRE 恢复进度窗口（失败静默降级，非关键）：GUI 显示阶段/DISM 进度/日志尾部。
    let progress = recovery_progress::spawn(early_log.clone(), operation_hint);
    // 每次重试拥有自己的窗口，任何提前返回都关闭旧窗口。
    struct ProgressGuard(Arc<recovery_progress::ProgressShared>);
    impl Drop for ProgressGuard {
        fn drop(&mut self) {
            recovery_progress::close_and_wait(&self.0);
        }
    }
    let _progress_guard = ProgressGuard(Arc::clone(&progress));
    // One registry for every role mounted in this WinRE session, so two roles
    // that resolve to the same volume share its letter instead of stealing it
    // from each other.
    let mut mounts = MountedVolumes::new();
    let task_letter =
        match mount_env_volume(&mut mounts, &values, "WORKSPACE", 'T', &early_log, false) {
            Ok(letter) => {
                meta_log(&format!("mount WORKSPACE OK -> {letter}:"));
                letter
            }
            Err(e) => {
                meta_log(&format!("mount WORKSPACE FAILED: {e}"));
                return Err(e);
            }
        };
    let store = TaskStore::new(PathBuf::from(format!(r"{}:\{store_rel}", task_letter)));
    let task_dir = store.task_dir(&task_id)?;
    early_log = task_dir.join("Recovery-early.log");
    // RAM 盘初始化日志也落到任务目录，便于跨重启核验设备初始化。
    if let Ok(system_root) = std::env::var("SystemRoot") {
        for (source, name) in [
            ("Temp/BackupRestore-init.log", "WinPE-init.log"),
            ("System32/wpeinit.log", "wpeinit.log"),
        ] {
            let source = PathBuf::from(&system_root).join(source);
            if source.is_file() {
                fs::copy(source, task_dir.join(name))?;
            }
        }
    }
    // 早期日志切到任务目录后，进度窗口跟随新日志并重置读取偏移。
    progress.switch_log_path(early_log.clone(), 0);
    let mut task = store.load(&task_id)?;
    meta_log(&format!(
        "task loaded status={:?} operation={:?}",
        task.status, task.operation
    ));
    if task.status == Stage::RecoveryComplete {
        // 清理中断后的重入仅清理，不依赖已删除的载荷文件，也绝不再次格式化。
        recovery_fault::configure(values.get("TEST_FAULT").map(String::as_str), &task_dir);
        complete_recovery_cleanup(&store, &mut task, &early_log)?;
        return run_logged("wpeutil.exe", &["reboot"], &early_log);
    }
    if task.status == Stage::Success {
        return run_logged("wpeutil.exe", &["reboot"], &early_log);
    }
    if task.status == Stage::Failed {
        return Err(err(&format!(
            "task is already terminal at stage {:?}; refusing to run it again",
            task.status
        )));
    }
    // 这里曾经在入口立即把注册 WinRE 覆写回干净原件（commits fcb48a1），已于
    // 2026-09-28 回退：那样会让整个会话的注册位都不是注入件，而断电续跑要求重启后
    // bootmgr 加载的注册位必须仍是注入件（含 winpeshl → 自动拉起 Recovery.exe），
    // 否则重启会落进微软原版 WinRE、任务不会自动继续。
    //
    // 注册位只有一处，却要同时扮演「载荷宿主」与「捕获纯净」两个互斥角色，且不能用
    // 第二宿主位置绕开（WinRE 强校验启动路径 == ReAgent 注册位置），因此只能按时间
    // 分片：任务存活期保持注入件，只在 DISM 捕获前那一刻翻成干净原件
    // （见 `restore_clean_winre_before_capture`），任务终结时再还原一次。
    // 完整设计与场景矩阵：docs/20260928-0745-registered-winre-policy-and-resume-design.md
    // 主日志默认在任务目录；挂载镜像卷后（见下）切换到镜像同目录 Recovery.log，
    // 方便用户在 WIM 旁直接查看。workspace_log 保留给 GUI 状态报告读取。
    let workspace_log = store.log_path(&task_id)?;
    let mut log = workspace_log.clone();
    let mut efi_root = None;
    // WinRE cleanup is part of the recovery contract.  Defer the terminal
    // success write until the original registered WinRE image has been
    // restored and verified below.
    let result = (|| -> Result<(), TaskError> {
        let persisted_status: StatusRecord = read_json(store.status_path(&task_id)?)?;
        if !persisted_status.task_id.eq_ignore_ascii_case(&task.task_id) {
            return Err(err("status.json task_id does not match task.json"));
        }
        if persisted_status.operation != task.operation {
            return Err(err("status.json operation does not match task.json"));
        }
        // The normal-host preparation records boot-requested in status.json
        // without rewriting the payload task.json. Accept exactly that one-step
        // ahead state; any other divergence indicates a torn or tampered task.
        if persisted_status.stage != task.status
            && !(task.status == Stage::Prepared && persisted_status.stage == Stage::BootRequested)
        {
            return Err(err("status.json stage does not match task.json"));
        }
        verify_task_identity_env(&values, &task)?;
        let manifest: PayloadManifest = read_json(task_dir.join("manifest.json"))?;
        if !manifest.task_id.eq_ignore_ascii_case(&task.task_id) {
            return Err(err("payload manifest task_id does not match task.json"));
        }
        let recovery_exe = task_dir.join("payload").join("Recovery.exe");
        let task_json = task_dir.join("payload").join("task.json");
        let recovery_task_env = task_dir.join("payload").join("RecoveryTask.env");
        let original = task_dir.join("original").join("Winre.wim");
        let staged = task_dir.join("stage").join("Winre.wim");
        if !recovery_exe.is_file() {
            return Err(err("Recovery.exe is required for every WinRE operation"));
        }
        verify_running_recovery_binary(&manifest)?;
        validate_payload_files(
            &manifest,
            &recovery_exe,
            &task_json,
            &recovery_task_env,
            &original,
            &staged,
        )?;
        // The payload task is the immutable copy staged before the one-time
        // boot request. Compare every field except the durable stage: after a
        // real interruption the workspace task is legitimately at
        // TargetErased/ImageApplied/BootRepaired while the immutable payload
        // remains Prepared.
        let payload_task: Task = read_json(&task_json)?;
        payload_task.validate()?;
        let mut comparable_payload = payload_task;
        let mut comparable_workspace = task.clone();
        // Drive letters are intentionally ephemeral.  WinRE remounts the
        // same GUID-identified volumes using its own letters (for example
        // P:/Q: become G:/H:).  Comparing those letters made every
        // interrupted task fail before it could resume, despite all stable
        // partition identities being identical.
        clear_task_drive_letters(&mut comparable_payload);
        clear_task_drive_letters(&mut comparable_workspace);
        comparable_payload.status = comparable_workspace.status;
        if comparable_payload != comparable_workspace {
            return Err(err(
                "payload task does not match the workspace task prepared for this recovery",
            ));
        }

        recovery_fault::configure(values.get("TEST_FAULT").map(String::as_str), &task_dir);
        // 先完成任务和不可变载荷校验，再判断哪些角色确实受本任务格式化影响。
        let recovery_letter = mount_env_volume(
            &mut mounts,
            &values,
            "RECOVERY",
            'R',
            &early_log,
            resume_safety::role_was_erased(&task, &values, "RECOVERY"),
        )?;
        meta_log(&format!("mount RECOVERY OK (r={recovery_letter})"));
        let efi_letter = if task.operation != Operation::Probe {
            let reformatted_ok = resume_safety::role_was_erased(&task, &values, "SOURCE");
            let source_letter = mount_env_volume(
                &mut mounts,
                &values,
                "SOURCE",
                'S',
                &early_log,
                reformatted_ok,
            )?;
            let image_letter =
                mount_env_volume(&mut mounts, &values, "IMAGE", 'I', &early_log, false)?;
            if let Some(source) = task.source.as_mut() {
                source.drive_letter = Some(source_letter);
            }
            if let Some(image) = task.image.as_mut() {
                image.volume.drive_letter = Some(image_letter);
            }
            // 日志放到镜像同目录（如 E:\Recovery.log），与 WIM 并排便于查看；
            // 写失败则回退任务目录日志，不影响恢复流程。
            // 备份任务用 destination 记录镜像，还原任务用 image。
            let image_relative = match &task.destination {
                Some(spec) => Some(spec.relative_path.clone()),
                None => task.image.as_ref().map(|spec| spec.relative_path.clone()),
            };
            if let Some(relative) = image_relative {
                let parent = Path::new(&relative.replace('\\', "/"))
                    .parent()
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_default();
                let candidate = if parent.is_empty() || parent == "/" {
                    PathBuf::from(format!("{image_letter}:\\Recovery.log"))
                } else {
                    PathBuf::from(format!(
                        "{image_letter}:\\{}\\Recovery.log",
                        parent.trim_matches('/')
                    ))
                };
                // 候选日志若是新文件，把任务目录已有的早期日志一并复制过去，
                // 保证镜像旁的 Recovery.log 内容完整。
                if !candidate.exists() && workspace_log.exists() {
                    let _ = fs::copy(&workspace_log, &candidate);
                }
                let candidate_offset = fs::metadata(&candidate).map(|m| m.len()).unwrap_or(0);
                match append_log(&candidate, "Recovery log continued in image directory") {
                    Ok(_) => {
                        log = candidate;
                        // 主日志切到镜像同目录（如 E:\Recovery.log）后，进度窗口跟随追加点。
                        progress.switch_log_path(log.clone(), candidate_offset);
                    }
                    Err(error) => {
                        let _ = append_log(
                            &log,
                            &format!(
                                "image-directory log unavailable ({error}); keeping workspace log"
                            ),
                        );
                    }
                }
            }
            if let Some(destination) = task.destination.as_mut() {
                destination.volume.drive_letter = Some(image_letter);
            }
            if task.target.is_some() {
                let target_letter = mount_env_volume(
                    &mut mounts,
                    &values,
                    "TARGET",
                    'W',
                    &early_log,
                    matches!(
                        task.status,
                        Stage::TargetErased | Stage::ImageApplied | Stage::BootRepaired
                    ),
                )?;
                // WinRE may reuse E: for an image/data volume; keep EFI on a
                // late temporary letter to avoid mount collisions.
                let efi_letter =
                    mount_env_volume(&mut mounts, &values, "EFI", 'Z', &early_log, false)?;
                if let Some(target) = task.target.as_mut() {
                    target.volume.drive_letter = Some(target_letter);
                }
                Some(efi_letter)
            } else {
                None
            }
        } else if let Some(source) = task.source.as_mut() {
            // Probe tasks are allowed to keep their task files on the source
            // partition. Reuse the verified workspace mount when both names
            // identify the same partition.
            let workspace_volume = task
                .workspace_volume
                .as_ref()
                .ok_or_else(|| err("probe task is missing workspace volume identity"))?;
            if !workspace_volume.same_partition(source) {
                source.drive_letter = Some(mount_env_volume(
                    &mut mounts,
                    &values,
                    "SOURCE",
                    'S',
                    &early_log,
                    false,
                )?);
            } else {
                source.drive_letter = Some(task_letter);
                append_log(
                    &early_log,
                    &format!("Probe source is the task partition; reusing {task_letter}:"),
                )?;
            }
            None
        } else {
            None
        };
        efi_root = efi_letter.map(|letter| PathBuf::from(format!(r"{}:\", letter)));
        append_log(
            &log,
            &format!(
                "Recovery.exe started from env task={} operation={:?}",
                task_id, task.operation
            ),
        )?;
        meta_log(&format!(
            "reaching recover_windows (operation={:?})",
            task.operation
        ));
        recover_windows(
            &store,
            &mut task,
            &log,
            efi_root.as_deref(),
            Some(&values),
            false,
            Some(&progress),
        )
    })();
    if let Err(error) = result {
        // 【2026-10-23 实机发现】用户按过 B 回退时，续跑链校验必然失败（BCD 被整份
        // 换回准备期快照，任务自建的 loader/device options 随之消失），但此前现场
        // 只剩一句 `invalid task: 续跑启动链字段不符：device`，看不出是用户自己按了 B。
        // 这里把因果补进日志与错误文案——**只改说明，不改行为**：失败仍按原路径
        // 收口（保留现场、可重试），避免掩盖真正的缺陷。
        let error = if crate::recovery_progress::bcd_rolled_back()
            && error.to_string().contains("续跑启动链")
        {
            let _ = append_log(
                &log,
                crate::i18n::msg("rollback_bcd_noted"),
            );
            match annotate_chain_failure(&error.to_string(), true) {
                Some(text) => crate::err(&text),
                None => error,
            }
        } else {
            error
        };
        // C: 可能已经被擦除；回滚旧 BCD 不会还原系统文件，并可能移除续跑对象。
        // 保留当前阶段与外部载荷，停止本次执行，故障解除后从同一阶段重试。
        let persisted = store.write_recovery_error_coded(&task, error.code(), error.to_string());
        let _ = append_log(
            &log,
            &format!(
                "Recovery retry required at {:?}: {error}; boot entry and payload retained",
                task.status
            ),
        );
        if let Err(state_error) = persisted {
            let _ = append_log(
                &log,
                &format!("Unable to persist recovery error: {state_error}"),
            );
        }
        return Err(error);
    }
    store.write_transition(&mut task, Stage::RecoveryComplete)?;
    complete_recovery_cleanup(&store, &mut task, &log)?;
    meta_log("FINAL: success, wpeutil reboot");
    run_logged("wpeutil.exe", &["reboot"], &log)
}

/// 续跑链校验失败时，若本次运行回退过 BCD，就把因果补进错误文案。
///
/// 抽成纯函数便于单测：真实的回退标记是进程级静态量（`bcd_rolled_back`），
/// 单测里造不出来，也不该为了让测试能过就把标记做成可变全局。
///
/// 只有**同时**满足「本次回退过」与「错误是续跑链校验」才改写——其它失败
/// （镜像损坏、载荷缺失等）必须保持原文，不能被这句说明糊住。
fn annotate_chain_failure(error: &str, rolled_back: bool) -> Option<String> {
    if rolled_back && error.contains("续跑启动链") {
        // `TaskError` 的 Display 自带 `invalid task: ` 前缀，而调用方还要再包一层
        // `crate::err`。不剥掉就会在对话框里出现
        // 「invalid task: invalid task: 续跑启动链…」（2026-10-23 实机撞见）。
        let body = error.strip_prefix("invalid task: ").unwrap_or(error);
        Some(format!(
            "{body}（本次运行中用户按 B 回退过 BCD：BCD 已还原为任务准备期快照，\
             续跑链校验失败属预期）"
        ))
    } else {
        None
    }
}

/// 还原终态自检（2026-10-26）：把「目标卷序列号前后 + Windows 内核文件」写成一行
/// ASCII 结论。它取代人工「放哨兵文件 + 记序列号」：`serial_before` 与
/// `serial_after` 不同即证明目标卷被重写过。
///
/// ★ 自检是**记录**，失败绝不能把一次成功的还原判成失败，所以调用处一律忽略
/// 返回值；盘符或卷身份取不到时字段记 `unknown`，宁可少记也不改判定。
#[cfg(windows)]
fn append_restore_selfcheck(log: &Path, task: &backuprestore_core::Task) {
    let Some(target) = task.target.as_ref() else {
        return;
    };
    let Some(letter) = target.volume.drive_letter else {
        return;
    };
    let after = crate::windows_prepare::volume_identity(letter)
        .map(|identity| identity.volume_serial)
        .unwrap_or_else(|_| "unknown".to_string());
    let ntoskrnl = PathBuf::from(format!(r"{letter}:\Windows\System32\ntoskrnl.exe")).is_file();
    let _ = append_log(
        log,
        &crate::text_parsing::restore_selfcheck_line(
            letter,
            &target.volume.volume_serial,
            &after,
            ntoskrnl,
        ),
    );
}

/// 该阶段证明磁盘、引导、恢复注册均已完成；重复执行不会重做磁盘操作。
#[cfg(windows)]
fn complete_recovery_cleanup(
    store: &TaskStore,
    task: &mut Task,
    log: &Path,
) -> Result<(), TaskError> {
    if task.status != Stage::RecoveryComplete {
        return Err(errc(ErrCode::TaskNotRecovered));
    }
    append_log(log, crate::i18n::msg("step_cleanup"))?;
    let env = store
        .task_dir(&task.task_id)?
        .join("payload/RecoveryTask.env");
    if env.is_file() {
        let values = read_env_file(env)?;
        recovery_fault::configure(
            values.get("TEST_FAULT").map(String::as_str),
            &store.task_dir(&task.task_id)?,
        );
    }
    let result = (|| {
        finalize_boot_entry_after_task(&store.task_dir(&task.task_id)?, log)?;
        // 终态自检：备份任务没有 target，函数自身会直接返回，不用在这里分叉。
        append_restore_selfcheck(log, task);
        recovery_fault::checkpoint("success-before", log)?;
        store.write_transition(task, Stage::Success)?;
        recovery_fault::checkpoint("success-after", log)?;
        append_log(log, "Boot entry cleaned; task marked successful")
    })();
    if let Err(error) = &result {
        // 【2026-10-16 审查修订，日志缺口 4】写盘失败不用 `?`：那会用写盘错误
        // 替换掉 `result` 里的原始恢复错误向上传播（`result` 仍会在函数末尾
        // 返回）。两个失败各自记一行，事后能区分"清理失败"与"连状态都没记上"。
        if task.status == Stage::RecoveryComplete {
            if let Err(state_error) =
                store.write_recovery_error(task, format!("恢复已完成，待重试清理：{error}"))
            {
                let _ = append_log(
                    log,
                    &crate::i18n::msg_tpl(
                        "recovery_done_status_write",
                        &[&error.to_string(), &state_error.to_string()],
                    ),
                );
            }
        }
        if let Err(log_error) =
            append_log(log, &format!("Recovery completed; cleanup pending: {error}"))
        {
            let _ = append_log(
                log,
                &crate::i18n::msg_tpl("recovery_done_log_write", &[&error.to_string(), &log_error.to_string()]),
            );
        }
    }
    result
}

/// 显式重试既有任务：先核验外部启动链；已恢复的任务只补做清理。
#[cfg(windows)]
fn resume_task(root: String, id: String) -> Result<(), TaskError> {
    let store = TaskStore::new(root);
    let mut task = store.load(&id)?;
    let dir = store.task_dir(&id)?;
    let log = store.log_path(&id)?;
    if task.status == Stage::RecoveryComplete {
        return complete_recovery_cleanup(&store, &mut task, &log);
    }
    let explicitly_prepared = task.status == Stage::Prepared;
    if !explicitly_prepared && !stage_resumable_after_interruption(task.status) {
        return Err(errc(ErrCode::TaskNotResumable));
    }
    if explicitly_prepared {
        // `prepare --no-reboot` leaves a complete task/payload and BCD template
        // but intentionally does not arm it. Only an explicit user resume may
        // turn that prepared task into a one-shot recovery boot.
        store.write_transition(&mut task, Stage::BootRequested)?;
    }
    let entry = boot_entry::ReBootEntry::read(&dir)?.ok_or_else(|| errc(ErrCode::TaskMissingLaunchRecord))?;
    boot_entry::rearm(&entry, &log)?;
    append_log(
        &log,
        "Explicit resume: validated existing task and boot payload",
    )?;
    run_logged("shutdown.exe", &["/r", "/t", "0"], &log)
}

/// 终态清理：清一次性启动、删自建 BCD 对象、删镜像卷上的载荷 WIM。
/// 幂等；没有记录（旧通道任务）时直接返回，不报错。
#[cfg(windows)]
fn finalize_boot_entry_after_task(task_dir: &Path, log: &Path) -> Result<(), TaskError> {
    let Some(entry) = boot_entry::ReBootEntry::read(task_dir)? else {
        append_log(
            log,
            "Boot entry cleanup: no recorded boot entry; nothing to clean",
        )?;
        return Ok(());
    };
    boot_entry::disarm(&entry, log)
}

#[cfg(any(windows, test))]
#[allow(dead_code)]
fn clear_task_drive_letters(task: &mut Task) {
    if let Some(source) = task.source.as_mut() {
        source.drive_letter = None;
    }
    if let Some(workspace) = task.workspace_volume.as_mut() {
        workspace.drive_letter = None;
    }
    if let Some(image) = task.image.as_mut() {
        image.volume.drive_letter = None;
    }
    if let Some(destination) = task.destination.as_mut() {
        destination.volume.drive_letter = None;
    }
    if let Some(target) = task.target.as_mut() {
        target.volume.drive_letter = None;
    }
}

/// `winpeshl.ini` starts this GUI-subsystem Rust binary directly.  Verify the
/// actual executable loaded from the task WinRE image as well as the immutable
/// workspace payload copy before touching any disk.  Checking only the copy in
/// the workspace would not prove that WinRE launched the staged Rust payload.
#[cfg(windows)]
fn verify_running_recovery_binary(manifest: &PayloadManifest) -> Result<(), TaskError> {
    let running = env::current_exe()?;
    backuprestore_core::verify_sha256(&running, &manifest.recovery_sha256).map_err(|error| {
        err(&format!(
            "running WinRE Recovery.exe does not match the prepared payload: {error}"
        ))
    })
}

#[cfg(windows)]
fn read_env_file(path: impl AsRef<Path>) -> Result<BTreeMap<String, String>, TaskError> {
    let mut values = BTreeMap::new();
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| err("RecoveryTask.env contains a malformed line"))?;
        if key.is_empty()
            || !key.chars().enumerate().all(|(index, ch)| {
                ch == '_' || ch.is_ascii_alphanumeric() && (index > 0 || ch.is_ascii_alphabetic())
            })
        {
            return Err(err("RecoveryTask.env contains an invalid key"));
        }
        if values.insert(key.to_string(), value.to_string()).is_some() {
            return Err(err("RecoveryTask.env contains a duplicate or empty key"));
        }
    }
    Ok(values)
}

#[cfg(windows)]
fn env_required(values: &BTreeMap<String, String>, key: &str) -> Result<String, TaskError> {
    values
        .get(key)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| err(&format!("RecoveryTask.env is missing {key}")))
}

#[cfg(windows)]
fn env_u32(values: &BTreeMap<String, String>, key: &str) -> Result<u32, TaskError> {
    env_required(values, key)?
        .parse::<u32>()
        .map_err(|_| err(&format!("RecoveryTask.env {key} is not a number")))
}

#[cfg(windows)]
fn env_u64(values: &BTreeMap<String, String>, key: &str) -> Result<u64, TaskError> {
    env_required(values, key)?
        .parse::<u64>()
        .map_err(|_| err(&format!("RecoveryTask.env {key} is not a number")))
}

#[cfg(windows)]
fn env_optional(values: &BTreeMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

#[cfg(windows)]
fn env_optional_u64(values: &BTreeMap<String, String>, key: &str) -> Option<u64> {
    env_optional(values, key).and_then(|value| value.parse::<u64>().ok())
}

#[cfg(windows)]
fn verify_task_identity_env(
    values: &BTreeMap<String, String>,
    task: &Task,
) -> Result<(), TaskError> {
    fn verify(
        values: &BTreeMap<String, String>,
        prefix: &str,
        identity: &backuprestore_core::VolumeIdentity,
        allow_reformatted_serial: bool,
    ) -> Result<(), TaskError> {
        for (suffix, expected, label) in [
            ("VOLUME_GUID", identity.volume_guid.as_str(), "volume"),
            ("DISK_GUID", identity.disk_guid.as_str(), "disk"),
            (
                "PARTITION_GUID",
                identity.partition_guid.as_str(),
                "partition",
            ),
            (
                "PARTITION_TYPE_GUID",
                identity.partition_type_guid.as_str(),
                "partition type",
            ),
            ("FILESYSTEM", identity.filesystem.as_str(), "filesystem"),
        ] {
            let actual = env_required(values, &format!("{prefix}_{suffix}"))?;
            if !expected.eq_ignore_ascii_case(&actual) {
                return Err(err(&format!(
                    "{prefix} {label} differs between task.json and RecoveryTask.env"
                )));
            }
        }
        for (suffix, expected, label) in [
            ("DISK_NUMBER", identity.disk_number, "disk number"),
            (
                "PARTITION_NUMBER",
                identity.partition_number,
                "partition number",
            ),
        ] {
            let actual = env_u32(values, &format!("{prefix}_{suffix}"))?;
            if expected != Some(actual) {
                return Err(err(&format!(
                    "{prefix} {label} differs between task.json and RecoveryTask.env"
                )));
            }
        }
        for (suffix, expected, label) in [
            (
                "PARTITION_OFFSET",
                identity.partition_offset,
                "partition offset",
            ),
            ("PARTITION_SIZE", identity.partition_size, "partition size"),
        ] {
            let actual = env_u64(values, &format!("{prefix}_{suffix}"))?;
            if expected != actual {
                return Err(err(&format!(
                    "{prefix} {label} differs between task.json and RecoveryTask.env"
                )));
            }
        }
        if !allow_reformatted_serial && !identity.volume_serial.trim().is_empty() {
            let actual = env_required(values, &format!("{prefix}_VOLUME_SERIAL"))?;
            if !identity.volume_serial.eq_ignore_ascii_case(&actual) {
                return Err(err(&format!(
                    "{prefix} volume serial differs between task.json and RecoveryTask.env"
                )));
            }
        }
        Ok(())
    }

    if let Some(source) = task.source.as_ref() {
        verify(values, "SOURCE", source, false)?;
    }
    let workspace_volume = task
        .workspace_volume
        .as_ref()
        .ok_or_else(|| err("task.json is missing workspace_volume identity"))?;
    verify(values, "WORKSPACE", workspace_volume, false)?;
    match task.operation {
        Operation::Backup => {
            let destination = task
                .destination
                .as_ref()
                .ok_or_else(|| err("backup task is missing destination"))?;
            verify(values, "IMAGE", &destination.volume, false)?;
            verify_image_absolute_path(values, destination.absolute_path.as_deref())?;
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let image = task
                .image
                .as_ref()
                .ok_or_else(|| err("restore task is missing image"))?;
            verify(values, "IMAGE", &image.volume, false)?;
            verify_image_absolute_path(values, image.absolute_path.as_deref())?;
            let target = task
                .target
                .as_ref()
                .ok_or_else(|| err("restore task is missing target"))?;
            verify(
                values,
                "TARGET",
                &target.volume,
                matches!(
                    task.status,
                    Stage::TargetErased | Stage::ImageApplied | Stage::BootRepaired
                ),
            )?;
        }
        Operation::Probe => {}
    }
    Ok(())
}

#[cfg(windows)]
fn verify_image_absolute_path(
    values: &BTreeMap<String, String>,
    expected: Option<&str>,
) -> Result<(), TaskError> {
    let expected = expected.ok_or_else(|| err("task is missing image absolute path"))?;
    backuprestore_core::validate_absolute_path(expected)?;
    let actual = env_required(values, "IMAGE_ABSOLUTE_PATH")?;
    backuprestore_core::validate_absolute_path(&actual)?;
    if !expected.eq_ignore_ascii_case(&actual) {
        return Err(err(
            "image absolute path differs between task.json and RecoveryTask.env",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn mount_env_volume(
    mounts: &mut MountedVolumes,
    values: &BTreeMap<String, String>,
    prefix: &str,
    letter: char,
    log: &Path,
    allow_reformatted_serial: bool,
) -> Result<char, TaskError> {
    let disk = env_u32(values, &format!("{prefix}_DISK_NUMBER"))?;
    let partition = env_u32(values, &format!("{prefix}_PARTITION_NUMBER"))?;
    let expected = env_required(values, &format!("{prefix}_VOLUME_GUID"))?;
    let mount_started = Instant::now();
    append_log(
        log,
        &format!(
            "mount {prefix} start: expected_guid={expected} requested_letter={letter}: disk={disk} partition={partition}"
        ),
    )?;
    // An earlier role already mounted this very volume. diskpart's
    // `assign letter=` MOVES a volume's letter instead of adding one, so
    // assigning a second letter here would tear that role's mount down and
    // leave its paths dangling. Share the letter, but still verify the volume
    // against this role's own recorded identity.
    if let Some(existing) = mounts.existing(&expected) {
        append_log(
            log,
            &format!(
                "mount {prefix}: volume already mounted at {existing}: by an earlier role, reusing that letter"
            ),
        )?;
        verify_mounted_volume(existing, &expected, log)?;
        verify_live_volume_identity(existing, values, prefix, allow_reformatted_serial)?;
        append_log(
            log,
            &format!(
                "mount {prefix} complete via shared letter in {}ms",
                mount_started.elapsed().as_millis()
            ),
        )?;
        return Ok(existing);
    }
    // Drive letters are ephemeral across boots: WinRE assigns letters in its
    // own discovery order, so the requested letter can already belong to a
    // different volume while the expected volume already sits at another
    // letter (a real-C: backup with many attached volumes hit exactly that:
    // IMAGE wanted I: but WinRE had given I: to an unrelated volume, and the
    // old refusal aborted the whole task). Enumerate every current mount
    // first and never assign over a foreign mount — diskpart's
    // `assign letter=` would steal it: reuse the expected volume's own letter
    // when it has one, otherwise walk candidate letters until one is free.
    let listing = match enumerate_mounted_volumes(log) {
        Ok(listing) => listing,
        Err(error) => {
            append_log(
                log,
                &format!(
                    "mount {prefix}: mountvol listing unavailable ({error}); probing letters directly"
                ),
            )?;
            Vec::new()
        }
    };
    if let Some((_, letters)) = listing
        .iter()
        .find(|(guid, _)| crate::text_parsing::same_volume(guid, &expected))
        && let Some(&existing) = letters.first()
    {
        verify_mounted_volume(existing, &expected, log)?;
        verify_live_volume_identity(existing, values, prefix, allow_reformatted_serial)?;
        append_log(
            log,
            &format!(
                "mount {prefix} complete via pre-mounted letter {existing}: in {}ms",
                mount_started.elapsed().as_millis()
            ),
        )?;
        mounts.record(&expected, existing);
        return Ok(existing);
    }
    for letter in mount_letter_candidates(letter, &listing, mounts) {
        match query_mounted_volume(letter, log)? {
            VolumeMountQuery::Mounted(actual) => {
                if crate::text_parsing::same_volume(&actual, &expected) {
                    verify_mounted_volume(letter, &expected, log)?;
                    verify_live_volume_identity(letter, values, prefix, allow_reformatted_serial)?;
                    append_log(
                        log,
                        &format!(
                            "mount {prefix} complete via pre-mounted letter {letter}: in {}ms",
                            mount_started.elapsed().as_millis()
                        ),
                    )?;
                    mounts.record(&expected, letter);
                    return Ok(letter);
                }
                // Occupied by a foreign volume: assigning over it would tear
                // down whoever relies on it, so try the next free letter.
                append_log(
                    log,
                    &format!(
                        "mount {prefix}: {letter}: is already mounted to {actual}, trying the next free letter"
                    ),
                )?;
            }
            // An unproven letter must not be assigned over: doing so can tear
            // down another role's mount, and it bypasses the checks above.
            VolumeMountQuery::Unknown => {
                append_log(
                    log,
                    &format!(
                        "mount {prefix}: {letter}: mount state could not be determined, trying the next free letter"
                    ),
                )?;
            }
            VolumeMountQuery::Unmounted => {
                match assign_volume_letter(
                    letter,
                    disk,
                    partition,
                    &expected,
                    values,
                    prefix,
                    log,
                    allow_reformatted_serial,
                    mounts,
                    mount_started,
                ) {
                    Ok(letter) => return Ok(letter),
                    Err(error) => {
                        append_log(
                            log,
                            &format!(
                                "mount {prefix}: assigning {letter}: failed ({error}); trying the next free letter"
                            ),
                        )?;
                    }
                }
            }
        }
    }
    Err(err(&format!(
        "mount {prefix}: every candidate drive letter is unavailable for volume {expected}"
    )))
}

/// Assign `expected` to `letter` via mountvol, falling back to diskpart, and
/// verify both the resulting mount and the live volume identity. This is the
/// single assignment path behind `mount_env_volume`'s candidate-letter loop.
#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn assign_volume_letter(
    letter: char,
    disk: u32,
    partition: u32,
    expected: &str,
    values: &BTreeMap<String, String>,
    prefix: &str,
    log: &Path,
    allow_reformatted_serial: bool,
    mounts: &mut MountedVolumes,
    mount_started: Instant,
) -> Result<char, TaskError> {
    // X: is the writable WinRE RAM disk. C: may be the offline Windows
    // volume (or unavailable), so never use it for the assignment script.
    let script = PathBuf::from(format!(
        r"X:\Windows\Temp\BackupRestore-assign-{letter}.txt"
    ));
    append_log(
        log,
        &format!("mount {prefix}: assigning {letter}: with mountvol.exe"),
    )?;
    let direct_started = Instant::now();
    // 【2026-10-16 审查修订，日志缺口 5】mountvol 的输出落盘：此前三流全
    // `Stdio::null()`，失败只记退出码。mountvol 是 WinRE 里的历史故障点
    // （卷未联机 / GUID 不匹配 / 挂载点被占），错误文本恰恰是唯一线索。
    // 用 `run_program` 而不是 `.output()`：它带作业对象约束进程树、支持超时，
    // 且把输出写进文件——正好接上原来那个手工 try_wait 超时循环。
    let direct_out = script.with_extension("mountvol.out");
    let _ = std::fs::remove_file(&direct_out);
    let direct_outcome = windows_command::run_program(
        "mountvol.exe",
        &[format!("{letter}:"), expected.to_string()],
        Some(&direct_out),
        30_000,
    );
    let direct_text = std::fs::read(&direct_out)
        .map(|bytes| crate::text_parsing::decode_windows_bytes(&bytes))
        .unwrap_or_default();
    if !direct_text.trim().is_empty() {
        append_log(log, &crate::i18n::msg_tpl("mountvol_output", &[direct_text.as_str()]))?;
    }
    append_log(
        log,
        &format!(
            "mountvol.exe direct assignment for {letter}: {direct_outcome} after {}ms",
            direct_started.elapsed().as_millis()
        ),
    )?;
    let _ = std::fs::remove_file(&direct_out);
    if direct_outcome.is_success() {
        verify_mounted_volume(letter, expected, log)?;
        verify_live_volume_identity(letter, values, prefix, allow_reformatted_serial)?;
        append_log(
            log,
            &format!(
                "mount {prefix} complete via mountvol in {}ms",
                mount_started.elapsed().as_millis()
            ),
        )?;
        mounts.record(expected, letter);
        return Ok(letter);
    }
    let bare = crate::text_parsing::bare_volume_guid(expected)
        .and_then(|value| crate::text_parsing::require_guid(value, "恢复卷").ok())
        .ok_or_else(|| errc(ErrCode::MountFallbackNoVolumeId))?;
    let path = format!(r"\\?\Volume{bare}\");
    let live = windows_prepare::volume_identity_at_path(&path, expected.to_string())?;
    verify_env_volume_identity(&live, values, prefix, allow_reformatted_serial)?;
    append_log(
        log,
        &crate::i18n::msg_tpl(
            "mount_identity_verified",
            &[
                &format!("{disk}"),
                &format!("{partition}"),
                &format!("{:?}", live.disk_number),
                &format!("{:?}", live.partition_number),
            ],
        ),
    )?;
    let disk = live.disk_number.ok_or_else(|| errc(ErrCode::LiveDiskNumberMissing))?;
    let partition = live
        .partition_number
        .ok_or_else(|| errc(ErrCode::LivePartitionNumberMissing))?;
    let body =
        format!("select disk {disk}\r\nselect partition {partition}\r\nassign letter={letter}\r\n");
    fs::write(&script, body)?;
    let script_arg = script.to_string_lossy().into_owned();
    append_log(
        log,
        &format!("running diskpart.exe /s {script_arg} with file-backed output"),
    )?;
    let diskpart_log = log.with_extension("diskpart.log");
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&diskpart_log)?;
    let stderr = stdout.try_clone()?;
    let diskpart_started = Instant::now();
    let mut child = Command::new("diskpart.exe")
        .args(["/s", script_arg.as_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            append_log(
                log,
                &format!(
                    "diskpart.exe timed out after {}ms",
                    diskpart_started.elapsed().as_millis()
                ),
            )?;
            return Err(err(
                "diskpart timed out while assigning the recovery volume",
            ));
        }
        thread::sleep(Duration::from_millis(100));
    };
    append_log(
        log,
        &format!(
            "diskpart.exe completed with {status}; output={}",
            diskpart_log.display()
        ),
    )?;
    let _ = fs::remove_file(&script);
    verify_mounted_volume(letter, expected, log)?;
    verify_live_volume_identity(letter, values, prefix, allow_reformatted_serial)?;
    append_log(
        log,
        &format!(
            "mount {prefix} complete via diskpart in {}ms",
            mount_started.elapsed().as_millis()
        ),
    )?;
    mounts.record(expected, letter);
    Ok(letter)
}

/// Snapshot every current volume→letter mount by running `mountvol` with no
/// arguments. Drive letters are assigned in boot-order luck, so the recovery
/// path must check what is actually mounted instead of trusting the letters
/// recorded at preparation time. GUID lines and `X:\` tokens are ASCII even
/// in localized WinRE, so lossy decoding keeps the parse reliable.
#[cfg(windows)]
fn enumerate_mounted_volumes(log: &Path) -> Result<Vec<(String, Vec<char>)>, TaskError> {
    let started = Instant::now();
    let mut child = Command::new("mountvol.exe")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            let mut raw = Vec::new();
            if let Some(mut stdout) = child.stdout.take() {
                stdout.read_to_end(&mut raw)?;
            }
            let output = String::from_utf8_lossy(&raw).into_owned();
            let listing = parse_mountvol_listing(&output);
            append_log(
                log,
                &format!(
                    "mountvol listing: {} volumes, {} mounted letters, exited {status} after {}ms",
                    listing.len(),
                    listing
                        .iter()
                        .map(|(_, letters)| letters.len())
                        .sum::<usize>(),
                    started.elapsed().as_millis()
                ),
            )?;
            return Ok(listing);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err("mountvol listing timed out"));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Candidate letters for a role, best first. The role's own letter leads so
/// the common path is unchanged; the spares are only walked when that letter
/// is occupied by a foreign volume or unproven. Never offer a letter that is
/// currently mounted by any volume, the WinRE ramdisk X:, the offline Windows
/// volume C:, floppy A:/B:, or a letter another role already mounted in this
/// recovery session (R:/S:/T:/W:/Z:).
#[cfg(windows)]
fn mount_letter_candidates(
    requested: char,
    listing: &[(String, Vec<char>)],
    mounts: &MountedVolumes,
) -> Vec<char> {
    let mut mounted_letters: Vec<char> = listing
        .iter()
        .flat_map(|(_, letters)| letters.iter().copied())
        .collect();
    mounted_letters.extend(mounts.letters());
    let reserved = ['A', 'B', 'C', 'X', 'R', 'S', 'T', 'W', 'Z'];
    let mut candidates = vec![requested];
    for spare in ['J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'U', 'V', 'Y'] {
        if spare != requested && !mounted_letters.contains(&spare) && !reserved.contains(&spare) {
            candidates.push(spare);
        }
    }
    candidates
}

#[cfg(windows)]
fn verify_live_volume_identity(
    letter: char,
    values: &BTreeMap<String, String>,
    prefix: &str,
    allow_reformatted_serial: bool,
) -> Result<(), TaskError> {
    // A volume GUID alone is insufficient: it can survive reformatting or a
    // stale mount assignment. Re-read the live GPT/device identity after
    // mounting and compare every immutable field recorded at preparation.
    let live = crate::windows_prepare::volume_identity_with_known_guid(
        letter,
        env_required(values, &format!("{prefix}_VOLUME_GUID"))?,
    )?;
    verify_env_volume_identity(&live, values, prefix, allow_reformatted_serial)
}

#[cfg(windows)]
fn verify_env_volume_identity(
    live: &backuprestore_core::VolumeIdentity,
    values: &BTreeMap<String, String>,
    prefix: &str,
    allow_reformatted_serial: bool,
) -> Result<(), TaskError> {
    // 卷 GUID 单独比：env 里存的是裸 `{GUID}`，而读回来的是 `\\?\Volume{GUID}\`，
    // 直接比会把同一个卷判成两个（2026-09-30 restore-existing 就这么整任务失败过）。
    {
        let expected = env_required(values, &format!("{prefix}_VOLUME_GUID"))?;
        if !crate::text_parsing::same_volume(&live.volume_guid, &expected) {
            return Err(err(&format!(
                "{prefix} volume GUID differs after mounting: expected {expected}, got {}",
                live.volume_guid
            )));
        }
    }
    for (suffix, actual, label) in [
        ("DISK_GUID", live.disk_guid.as_str(), "disk GUID"),
        (
            "PARTITION_GUID",
            live.partition_guid.as_str(),
            "partition GUID",
        ),
        (
            "PARTITION_TYPE_GUID",
            live.partition_type_guid.as_str(),
            "partition type",
        ),
        ("FILESYSTEM", live.filesystem.as_str(), "filesystem"),
    ] {
        let expected = env_required(values, &format!("{prefix}_{suffix}"))?;
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(err(&format!(
                "{prefix} {label} differs after mounting: expected {expected}, got {actual}"
            )));
        }
    }
    for (suffix, actual, label) in [
        ("DISK_NUMBER", live.disk_number, "disk number"),
        (
            "PARTITION_NUMBER",
            live.partition_number,
            "partition number",
        ),
    ] {
        // 准备期编号仅校验记录格式；重启后地址由完整稳定身份确认。
        env_u32(values, &format!("{prefix}_{suffix}"))?;
        if actual.is_none() || (suffix == "PARTITION_NUMBER" && actual == Some(0)) {
            return Err(err(&format!(
                "{prefix} {label} is unavailable after mounting"
            )));
        }
    }
    for (suffix, actual, label) in [
        (
            "PARTITION_OFFSET",
            live.partition_offset,
            "partition offset",
        ),
        ("PARTITION_SIZE", live.partition_size, "partition size"),
    ] {
        let expected = env_u64(values, &format!("{prefix}_{suffix}"))?;
        if actual != expected {
            return Err(err(&format!(
                "{prefix} {label} differs after mounting: expected {expected}, got {actual}"
            )));
        }
    }
    if !allow_reformatted_serial
        && let Some(expected) = env_optional(values, &format!("{prefix}_VOLUME_SERIAL"))
        && (live.volume_serial.is_empty() || !live.volume_serial.eq_ignore_ascii_case(&expected))
    {
        return Err(err(&format!(
            "{prefix} volume serial differs after mounting"
        )));
    }
    Ok(())
}

/// Ask `mountvol` what is mounted at `letter:`.
///
/// A timeout returns `Unknown`, never `Unmounted`. WinRE is exactly where
/// this query has been observed to hang, and answering "nothing is mounted"
/// on a hang let the caller assign a letter whose real state was unknown.
#[cfg(windows)]
fn query_mounted_volume(letter: char, log: &Path) -> Result<VolumeMountQuery, TaskError> {
    let started = Instant::now();
    let mut child = Command::new("mountvol.exe")
        .args([format!("{letter}:"), "/L".to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait()? {
            // mountvol 在中文/非英文 WinRE 里会输出本地化（GBK 等非 UTF-8）文本。
            // 这里必须用 lossy 解码：严格 read_to_string 会以
            // "stream did not contain valid UTF-8" 整个失败，导致 WinRE 内
            // 挂载卷全挂、任务闪退重启循环。GUID 行始终是 ASCII，lossy 解码
            // 不影响 classify_mountvol_output 的判断。
            let mut raw = Vec::new();
            if let Some(mut stdout) = child.stdout.take() {
                stdout.read_to_end(&mut raw)?;
            }
            let output = String::from_utf8_lossy(&raw).into_owned();
            let query = classify_mountvol_output(status.success(), &output);
            append_log(
                log,
                &format!(
                    "mountvol query {letter}: exited {status} after {}ms -> {query:?}",
                    started.elapsed().as_millis()
                ),
            )?;
            return Ok(query);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            append_log(
                log,
                &format!(
                    "mountvol query {letter}: timed out after 5000ms; mount state is unproven"
                ),
            )?;
            return Ok(VolumeMountQuery::Unknown);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(windows)]
fn verify_mounted_volume(letter: char, expected: &str, log: &Path) -> Result<(), TaskError> {
    let root = PathBuf::from(format!("{letter}:\\"));
    if !root.is_dir() {
        return Err(err(&format!(
            "volume {letter}: is not accessible after assignment"
        )));
    }
    let actual = match query_mounted_volume(letter, log)? {
        VolumeMountQuery::Mounted(actual) => actual,
        VolumeMountQuery::Unmounted => {
            return Err(err(&format!(
                "volume {letter}: has no mounted volume identity"
            )));
        }
        VolumeMountQuery::Unknown => {
            return Err(err(&format!(
                "volume {letter}: mount identity query did not answer, cannot confirm it"
            )));
        }
    };
    if !crate::text_parsing::same_volume(&actual, expected) {
        return Err(err(&format!(
            "volume identity mismatch for {letter}: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn native_windows_architecture() -> String {
    let reported = [
        env::var("PROCESSOR_ARCHITEW6432").ok(),
        env::var("PROCESSOR_ARCHITECTURE").ok(),
    ];
    if reported
        .iter()
        .flatten()
        .any(|value| value.eq_ignore_ascii_case("ARM64"))
    {
        "arm64".into()
    } else if reported
        .iter()
        .flatten()
        .any(|value| value.eq_ignore_ascii_case("AMD64"))
    {
        "x64".into()
    } else {
        "unknown".into()
    }
}

#[cfg(not(windows))]
fn recover_windows(
    _store: &TaskStore,
    _task: &mut Task,
    _log: &Path,
    _efi_root: Option<&Path>,
    _metadata_context: Option<&BTreeMap<String, String>>,
    _finalize_success: bool,
    _progress: Option<&()>,
) -> Result<(), TaskError> {
    Err(err(
        "real Recovery execution is only available on Windows/WinRE",
    ))
}

/// 离线还原是否需要准备「引导修复会话」（STEP 1 预应用判据）。
///
/// 必须与另外两处同源，否则判据漂移会让会话该建没建、收尾却按 Windows 走，
/// 在 STEP 4 炸「Windows 系统卷缺少引导修复方案」：
/// 1. 收尾路由：灌完镜像后探测 `Windows\System32\config\SYSTEM` 是否存在；
/// 2. `resolve_efi_root_for_recovery`：决定要不要提前挂 ESP；
/// 3. 本函数：决定要不要建会话（第二阶段「引导修复」按钮也挂在它上面）。
///
/// create-secondary 的常态是「prepare 刚格式化完的空卷」：灌镜像前探测必然
/// false，所以必须认 `role=NewWindows`。2026-10-19 实机撞见（06fc5e8 引入时
/// 即只认前者；run10 能过只因那次目标卷已装着 Windows）。
fn recovery_needs_boot_menu_session(
    target_is_windows: bool,
    role: backuprestore_core::TargetRole,
) -> bool {
    target_is_windows || role == backuprestore_core::TargetRole::NewWindows
}

#[cfg(windows)]
fn recover_windows(
    store: &TaskStore,
    task: &mut Task,
    log: &Path,
    efi_root: Option<&Path>,
    metadata_context: Option<&BTreeMap<String, String>>,
    finalize_success: bool,
    progress: Option<&Arc<crate::recovery_progress::ProgressShared>>,
) -> Result<(), TaskError> {
    use backuprestore_core::TargetRole;
    if task.status == Stage::Prepared {
        store.write_transition(task, Stage::RecoveryStarted)?;
    }
    if task.status == Stage::BootRequested {
        store.write_transition(task, Stage::RecoveryStarted)?;
    }
    if task.status == Stage::RecoveryStarted {
        store.write_transition(task, Stage::Preflight)?;
    }
    let image_path = task
        .image
        .as_ref()
        .map(|image| resolve_volume_path(&image.volume, &image.relative_path, log))
        .transpose()?;
    if let Some(path) = &image_path {
        let image = task.image.as_ref().expect("image path implies image");
        let verify_hash = task.verify_hash.unwrap_or(false);
        if verify_hash {
            verify_image_file(path, image)?;
        } else {
            // 快速校验：仅核对文件存在性和物理大小，避免数分钟无谓读盘
            let size = fs::metadata(path)
                .map_err(|e| err(&format!("failed to read image metadata: {e}")))?
                .len();
            if image.size_bytes != 0 && size != image.size_bytes {
                return Err(err(&format!(
                    "image size mismatch: expected {}, got {size}",
                    image.size_bytes
                )));
            }
        }
        if matches!(
            task.operation,
            Operation::RestoreExisting | Operation::CreateSecondary
        ) {
            // 配套备份 metadata 存在时校验 WIM 哈希与目标分区大小；缺失时
            // 视为第三方/PE WIM（如安装 WinRE/PE 为第二系统），跳过该校验。
            if let Ok(metadata) = read_index_metadata(path, image.index) {
                if verify_hash && !metadata.image_sha256.eq_ignore_ascii_case(&image.sha256) {
                    // 用户在准备阶段显式接受过这份镜像时，旧副档不再充当判据。
                    // 镜像在用户确认之后是否又被改动，已由上面的 verify_image_file
                    // 按「接受时的真实哈希与大小」核验；这里只确认例外确实绑定当前
                    // 索引与内容，不是别的镜像或别的索引留下的通用放行。
                    let covered = task.image_acceptance.as_ref().is_some_and(|accepted| {
                        accepted.covers(image.index, &image.sha256, image.size_bytes)
                    });
                    if !covered {
                        return Err(err("image hash does not match backup metadata"));
                    }
                    append_log(
                        log,
                        &format!(
                            "image hash differs from sidecar {} but the user accepted {} at prepare time; sidecar is no longer the source-of-truth for this task",
                            metadata.image_sha256, image.sha256
                        ),
                    )?;
                }
                let target = task.target.as_ref().ok_or_else(|| err("missing target"))?;
                let required = metadata
                    .required_target_size()
                    .max(metadata.source.partition_size);
                if required == 0 || target.volume.partition_size < required {
                    return Err(err(&format!(
                        "target partition is too small: {} < {}",
                        target.volume.partition_size, required
                    )));
                }
            }
        }
        run_logged(
            "dism.exe",
            &["/Get-WimInfo", &format!("/WimFile:{}", path.display())],
            log,
        )?;
    }
    match task.operation {
        Operation::Probe => {
            append_log(log, "Probe task validated; no disk operation requested")?;
            if task.status == Stage::RecoveryStarted {
                store.write_transition(task, Stage::Preflight)?;
            }
            if task.status == Stage::Preflight && finalize_success {
                store.write_transition(task, Stage::Success)?;
            }
        }
        Operation::Backup => {
            use backuprestore_core::{BackupMetadata, PROGRAM_VERSION, write_json_atomic};
            append_log(log, crate::i18n::msg("step_mount"))?;
            if let Some(p) = progress {
                p.reset_start_time();
            }
            let source = task.source.clone().ok_or_else(|| err("missing source"))?;
            let destination = task
                .destination
                .clone()
                .ok_or_else(|| err("missing destination"))?;
            let source_path = resolve_volume_root(&source, log)?;
            // v1.7.11 新通道：载荷只在镜像卷上，注册位全程只读，所以捕获进镜像的
            // 天生就是任务前的干净 WinRE —— 方案 D 的「捕获前翻回干净件」不再需要。
            let destination_path =
                resolve_volume_path(&destination.volume, &destination.relative_path, log)?;
            // 备份索引名：任务自定义（GUI 默认「系统版本+年月日」，如
            // `Windows 11 26200.9550 20261015`）或兜底默认名。CLI 侧没有
            // image_name 时不能照搬 GUI 的算法——这里可能跑在 RE 里，读到的
            // "当前系统"是被还原的环境而不是被备份的源卷。
            let image_name = task
                .image_name
                .clone()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Windows Backup".to_string());
            let partial = PathBuf::from(format!("{}.partial", destination_path.display()));
            if let Some(parent) = partial.parent() {
                fs::create_dir_all(parent)?;
            }
            match task.status {
                Stage::Preflight => {
                    store.write_transition(task, Stage::Capturing)?;
                }
                Stage::Capturing => {
                    append_log(
                        log,
                        "Resuming backup from capturing stage; replacing partial image",
                    )?;
                }
                stage => return Err(err(&format!("backup cannot resume from stage {stage:?}"))),
            }
            // 备份耗时统计：记录开始时间，捕获完成后计算总耗时与平均速度。
            let backup_started = Utc::now();
            append_log(
                log,
                &format!("Backup capture started at {}", backup_started.to_rfc3339()),
            )?;
            let existing = destination_path.exists();
            if existing && !destination_path.is_file() {
                return Err(err("backup image path exists but is not a regular file"));
            }
            let previous_hash = existing
                .then(|| backuprestore_core::sha256_file(&destination_path))
                .transpose()?;
            let previous_indexes = if existing {
                wim_indexes(&destination_path, log)?
            } else {
                Vec::new()
            };
            let mut previous_metadata = backuprestore_core::image_metadata::load_sidecars(
                &destination_path,
                &previous_indexes,
            )?;
            let legacy_path = legacy_metadata_path(&destination_path)?;
            let legacy_metadata: Option<BackupMetadata> = legacy_path
                .is_file()
                .then(|| read_json(&legacy_path))
                .transpose()
                .ok()
                .flatten();

            if let Some(value) = legacy_metadata.as_ref()
                && previous_indexes.contains(&value.wim_index)
                && previous_hash
                    .as_deref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(&value.image_sha256))
            {
                previous_metadata
                    .entry(value.wim_index)
                    .or_insert_with(|| value.clone());
            }

            // 生成 DISM 排除配置（Parallels 卷根占位符/回收站/临时目录/更新缓存/
            // 各用户浏览器缓存），WinRE 的 TEMP 位于 X: RAM 盘，配置文件不会落在
            // 捕获卷内。排除项会显著缩小 WIM 体积，且全部为 DISM 规范内的根路径写法。
            // 与 GUI 在线备份、PE 备份共用 core 的实现，避免某条路径漏接 /ConfigFile。
            // self_exe_dir 用 store.root（= Windows 侧运行目录，RE 内由
            // `resume <task-root>` 参数带进来）而不是 current_exe()：RE 内
            // current_exe 是恢复环境里的暂存路径，与捕获卷不同卷，动态排除项
            // 会永远不生成（2026-10-24 实测：五个部署件全被打进 T: 镜像）。
            // 同卷时排除自身部署件；tasks\logs\ 证据仍全留。
            let exclude_config_path = backuprestore_core::write_capture_exclusion_config(
                &source_path,
                Some(store.root.as_path()),
            )?;
            let exclude_arg = format!("/ConfigFile:{}", exclude_config_path.display());

            // A power loss can leave a partial WIM behind.  For the first
            // capture, only that partial file is removed.  For an existing
            // WIM, copy it to a same-volume candidate and append there; the
            // original remains recoverable until the candidate has passed
            // DISM inspection and is renamed into place.
            let _ = fs::remove_file(&partial);
            append_log(log, crate::i18n::msg("step_capture"))?;
            let new_index = if existing {
                let candidate = PathBuf::from(format!(
                    "{}.{}.append-candidate.wim",
                    destination_path.display(),
                    task.task_id
                ));
                let _ = fs::remove_file(&candidate);
                fs::copy(&destination_path, &candidate)?;
                let append_args = [
                    "/English",
                    "/Append-Image",
                    &format!("/ImageFile:{}", candidate.display()),
                    &format!("/CaptureDir:{}", source_path.display()),
                    &format!("/Name:{image_name}"),
                    "/CheckIntegrity",
                    &exclude_arg,
                ];
                // 捕获心跳：恢复环境里这条链同样要「先全卷预扫再写」，期间镜像恒为
                // WIM 头、DISM 日志静默，进度窗口的百分比会长时间不动。心跳行让
                // 「还在扫盘」与「真的卡死」在日志里可区分（只观测，不干预）。
                let heartbeat =
                    crate::capture_heartbeat::Heartbeat::start(candidate.clone(), log.to_path_buf());
                let append_result = run_logged("dism.exe", &append_args, log);
                drop(heartbeat);
                if let Err(error) = append_result {
                    let _ = fs::remove_file(&candidate);
                    return Err(error);
                }
                let candidate_indexes = wim_indexes(&candidate, log)?;
                if candidate_indexes.len() != previous_indexes.len().saturating_add(1) {
                    let _ = fs::remove_file(&candidate);
                    return Err(err("WIM append did not create exactly one new index"));
                }
                let index = *candidate_indexes
                    .last()
                    .ok_or_else(|| err("WIM append returned no indexes"))?;
                fs::rename(&candidate, &destination_path)?;
                append_log(log, &format!("Appended backup as WIM index {index}"))?;
                index
            } else {
                // 压缩率：仅首次创建 WIM 时生效；追加备份沿用 WIM 已有压缩设置。
                let compress_level =
                    canonical_compression(task.compress.as_deref().unwrap_or("fast"))?;
                let heartbeat = crate::capture_heartbeat::Heartbeat::start(
                    partial.clone(),
                    log.to_path_buf(),
                );
                let capture_result = run_logged(
                    "dism.exe",
                    &[
                        "/English",
                        "/Capture-Image",
                        &format!("/ImageFile:{}", partial.display()),
                        &format!("/CaptureDir:{}", source_path.display()),
                        &format!("/Name:{image_name}"),
                        &format!("/Compress:{compress_level}"),
                        "/CheckIntegrity",
                        &exclude_arg,
                    ],
                    log,
                );
                drop(heartbeat);
                capture_result?;
                let indexes = wim_indexes(&partial, log)?;
                if indexes != [1] {
                    return Err(err("first backup capture did not produce WIM index 1"));
                }
                fs::rename(&partial, &destination_path)?;
                1
            };
            let image_size = fs::metadata(&destination_path)?.len();
            // 备份耗时统计：总耗时与平均速度（字节/秒）。
            let backup_finished = Utc::now();
            let duration_secs = (backup_finished - backup_started).num_seconds().max(1) as u64;
            let bytes_per_sec = image_size / duration_secs;
            append_log(
                log,
                &format!(
                    "Backup capture finished: started={} finished={} duration={}s speed={}/s image={} bytes",
                    backup_started.to_rfc3339(),
                    backup_finished.to_rfc3339(),
                    duration_secs,
                    bytes_per_sec,
                    image_size,
                ),
            )?;
            append_log(log, crate::i18n::msg("step_verify"))?;
            let image_sha256 = backuprestore_core::sha256_file(&destination_path)?;
            let source_volume_serial = source.volume_serial.clone();
            let source_partition_size = source.partition_size;
            let context_value = |key: &str, fallback: &str| {
                metadata_context
                    .and_then(|values| env_optional(values, key))
                    .unwrap_or_else(|| fallback.to_string())
            };
            let context_u64 = |key: &str, fallback: u64| {
                metadata_context
                    .and_then(|values| env_optional_u64(values, key))
                    .unwrap_or(fallback)
            };
            let metadata = BackupMetadata {
                version: 3,
                image_type: "wim".into(),
                created: Utc::now(),
                started: Some(backup_started),
                duration_secs: Some(duration_secs),
                bytes_per_sec: Some(bytes_per_sec),
                computer: context_value("COMPUTERNAME", "WinRE"),
                windows_edition: context_value("WINDOWS_EDITION", "unknown"),
                architecture: context_value("WINDOWS_ARCHITECTURE", &native_windows_architecture()),
                windows_build: context_value("WINDOWS_BUILD", "unknown"),
                wim_index: new_index,
                image_sha256: image_sha256.clone(),
                image_size,
                source,
                captured_used_bytes: context_u64("SOURCE_USED_BYTES", 0),
                reserved_bytes: context_u64("RESERVED_BYTES", 0),
                minimum_target_size: context_u64("MINIMUM_TARGET_SIZE", source_partition_size),
                volume_serial: source_volume_serial,
                program_version: PROGRAM_VERSION.into(),
                // 与 DISM /Name 的有效索引名一致，供 sidecar 快速路径呈现真名
                image_name: Some(image_name),
            };
            previous_metadata.insert(new_index, metadata.clone());
            backuprestore_core::image_metadata::write_sidecars(
                &destination_path,
                &previous_metadata,
                &image_sha256,
                image_size,
                0,
            )?;

            // Retain a single legacy sidecar only when it clearly belongs to
            // this image.  New code always reads index-specific metadata;
            // this preserves old index-1 restores without clobbering another
            // WIM stored in the same directory.
            let legacy_belongs_to_image = if !legacy_path.exists() {
                true
            } else {
                legacy_metadata.as_ref().is_some_and(|value| {
                    previous_hash
                        .as_deref()
                        .is_some_and(|hash| value.image_sha256.eq_ignore_ascii_case(hash))
                })
            };
            if legacy_belongs_to_image {
                let first_path = index_metadata_path(&destination_path, 1)?;
                if first_path.try_exists()? {
                    let legacy: BackupMetadata = read_json(first_path)?;
                    write_json_atomic(&legacy_path, &legacy)?;
                }
            }
            append_log(
                log,
                &format!("Backup metadata written for WIM index {new_index}"),
            )?;
            // 删除后立即回读索引，再使用共用快照重编号，任何读写错误都使任务失败。
            if let Some(keep) = task.keep_indexes.filter(|n| *n > 0) {
                let keep = keep.max(1).min(new_index);
                for remaining in (keep + 1..=new_index).rev() {
                    run_logged(
                        "dism.exe",
                        &[
                            "/English",
                            "/Delete-Image",
                            &format!("/ImageFile:{}", destination_path.display()),
                            "/Index:1",
                        ],
                        log,
                    )?;
                    if wim_indexes(&destination_path, log)? != (1..remaining).collect::<Vec<_>>() {
                        return Err(err("WIM retention index read-back mismatch"));
                    }
                }
                if keep < new_index {
                    backuprestore_core::image_metadata::renumber_sidecars(
                        &destination_path,
                        new_index,
                        new_index - keep,
                    )?;
                    if legacy_belongs_to_image {
                        let path = index_metadata_path(&destination_path, 1)?;
                        if path.try_exists()? {
                            let first: BackupMetadata = read_json(path)?;
                            write_json_atomic(&legacy_path, &first)?;
                        } else if legacy_path.try_exists()? {
                            fs::remove_file(&legacy_path)?;
                        }
                    }
                    append_log(
                        log,
                        &format!(
                            "Kept latest {keep} WIM indexes; sidecars renumbered and verified"
                        ),
                    )?;
                }
            }
            if finalize_success {
                append_log(log, crate::i18n::msg("step_cleanup"))?;
                store.write_transition(task, Stage::Success)?;
            }
        }
        Operation::RestoreExisting | Operation::CreateSecondary => {
            let mut target = task.target.clone().ok_or_else(|| err("missing target"))?;
            // v1.7.11 新通道：格式化目标卷不再威胁"续跑启动源"（它在镜像卷上），
            // 因此 F1 执行层二次闸整体取消。目标卷自带的 Winre.wim 会被镜像里的
            // 干净原版替换，这与 Ghost 类工具语义一致（注册侧处置见设计文档 §3.3）。
            let image_index = task.image.as_ref().map(|image| image.index).unwrap_or(1);
            if target.role == TargetRole::NewWindows
                && task.boot_plan.mode != BootMode::AddSecondary
            {
                return Err(err("secondary target requires add-secondary boot plan"));
            }
            let image_path = image_path.ok_or_else(|| err("missing image"))?;
            // 目标卷同样按身份解析：PE 里记录盘符可能已被别的卷占用，而
            // `diskpart select disk/partition` 用的磁盘编号是稳定的，所以只要
            // 拿到"这个卷现在叫什么字母"就能安全格式化/应用。
            let target_letter = resolve_volume_letter(&target.volume, log)?;
            let target_root = PathBuf::from(format!("{target_letter}:\\"));
            target.volume.drive_letter = Some(target_letter);
            let live = windows_prepare::volume_identity(target_letter)?;
            let previous = (target.volume.disk_number, target.volume.partition_number);
            target.volume = pe_safety::rebind_after_boot(
                &target.volume,
                &live,
                !matches!(
                    task.status,
                    Stage::TargetErased | Stage::ImageApplied | Stage::BootRepaired
                ),
            )?;
            append_log(
                log,
                &crate::i18n::msg_tpl(
                    "restore_identity_rebound",
                    &[
                        &format!("{previous:?}"),
                        &format!(
                            "{:?}",
                            (target.volume.disk_number, target.volume.partition_number)
                        ),
                    ],
                ),
            )?;
            // 收尾按「目标性质」路由，不按「操作类型」：目标是另一套 Windows 时，
            // create-secondary 和 restore-existing 需要的收尾是同一套（保主系统
            // default + 修盘符绑定 + 按第二系统注册 WinRE）。判据来自准备阶段落盘的
            // is_active_system——RE 内 `%SystemDrive%` 是 RAM 盘 X:，探测不到用户系统卷。
            // 注：default 归属不再由此判据分支——v2.6.4 起只有一条规则
            // （一律指向本次还原出来的系统），固化在 boot_menu::decide_default。
            let target_is_secondary = !target.is_active_system;
            // 第二阶段按钮需要在目标身份确认后尽早出现；方案落盘并不修改 BCD，
            // 实际写入延后到映像应用完成后的引导修复阶段。
            // 判据必须与收尾路由（下方「灌完镜像后探测 SYSTEM hive」那扇门）和
            // `resolve_efi_root_for_recovery` 完全一致：目标**当前**是 Windows 卷，
            // 或本次要把完整系统装进一个空卷（`role=NewWindows`，create-secondary
            // 的常态——prepare 刚格式化完目标，灌镜像前探测必然 false）。
            // 只认前者会让 C 入口离线流在 STEP 4 炸「Windows 系统卷缺少引导修复
            // 方案」：会话没建，收尾却按 Windows 走（2026-10-19 实机撞见，
            // 06fc5e8 引入时即如此；run10 能过只因那次目标已装着 Windows）。
            let boot_menu_session = if recovery_needs_boot_menu_session(
                pe_safety::target_is_windows_volume(&target_root)?,
                target.role.clone(),
            ) {
                // 优先用任务里记录的第二系统菜单名（用户在准备阶段自己起的）；
                // 没有它才由 BootMenuSession 按镜像副档版本自动起名。
                let preferred_name = target.boot_menu_name.clone();
                let session = crate::boot_menu::BootMenuSession::prepare(
                    &target.volume,
                    &task.task_id,
                    &store.task_dir(&task.task_id)?,
                    log,
                    preferred_name.as_deref(),
                )?;
                // default 归属（docs/20261015-0725 S1，2026-10-15 用户修订）：
                // 无论目标是活动系统盘（重建唯一系统）还是另一个分区（新增第二
                // 系统），还原完成后 default 一律指向本次还原出来的系统——用户
                // 要的就是"还原完开机进新系统"。该规则已固化在
                // `boot_menu::decide_default` 里，apply 只执行不推断。
                if let Some(p) = progress {
                    p.set_boot_menu(Arc::clone(&session));
                    // 装载「BCD 回退」动作：点击时把 ESP 上的 BCD 恢复成任务准备期
                    // 保存的快照（bcd-before-raw / bcd-before-export）。ESP 身份在
                    // 点击时才解析（此刻还没到 STEP 3 的 find_efi_root），
                    // 用免盘符探测避免抢盘符。
                    let rollback_task_dir = store.task_dir(&task.task_id)?;
                    let rollback_log = log.to_path_buf();
                    p.set_bcd_rollback(Arc::new(move || {
                        let efi = windows_prepare::esp_identity_without_drive_letter()
                            .map_err(|error| error.to_string())?;
                        windows_prepare::rollback_bcd_snapshot(
                            &rollback_task_dir,
                            &efi,
                            &rollback_log,
                        )
                        .map_err(|error| error.to_string())
                    }));
                }
                append_log(log, crate::i18n::msg("boot_candidates_ready"))?;
                Some(session)
            } else {
                append_log(log, crate::i18n::msg("skip_boot_candidates_not_windows"))?;
                None
            };
            append_log(log, crate::i18n::msg("step_mount"))?;
            if let Some(p) = progress {
                p.reset_start_time();
            }
            // 一次性启动已经被固件消费。完整系统还原必须在擦除前重新武装，
            // 使真正断电后的下一次开机仍能从外部分区进入同一任务。
            let resume_entry = if metadata_context.is_some() {
                let entry = boot_entry::ReBootEntry::read(&store.task_dir(&task.task_id)?)?
                    .ok_or_else(|| errc(ErrCode::RestoreMissingResumeBoot))?;
                if entry.wim_volume.same_partition(&target.volume) {
                    return Err(errc(ErrCode::ResumePayloadOnEraseTarget));
                }
                if !task
                    .image
                    .as_ref()
                    .is_some_and(|image| image.volume.same_partition(&entry.wim_volume))
                {
                    return Err(errc(ErrCode::ResumePayloadVolumeMismatch));
                }
                boot_entry::rearm(&entry, log)?;
                Some(entry)
            } else {
                None
            };
            // TargetErased is deliberately persisted before formatting.  If
            // power fails after that write, formatting and applying the WIM
            // are safe to repeat on the explicitly selected target.  Older
            // task versions persisted ImageApplied before DISM completed, so
            // resume from ImageApplied also re-runs Apply-Image defensively.
            if matches!(task.status, Stage::Preflight | Stage::TargetErased) {
                let entering_target_erased = task.status == Stage::Preflight;
                if task.status == Stage::Preflight {
                    store.write_transition(task, Stage::TargetErased)?;
                }
                // 格式化选项（TargetSpec.format_target，GUI 复选框落盘）：取消勾选时
                // 覆盖式应用——跳过 diskpart 格式化与格式化后校验（不格式化就没有
                // 「序列号已变」的校验对象），直接进入镜像应用。断电续跑从
                // TargetErased 重入时同样按此选项跳过，行为一致。
                if target.format_target {
                    format_target_partition(store, task, &target.volume, log)?;
                    if !target_root.is_dir() {
                        return Err(err(
                            "restore target is unavailable after DiskPart format operation",
                        ));
                    }
                    verify_partition_identity_after_format(&target.volume, log)?;
                } else {
                    append_log(
                        log,
                        crate::i18n::msg("overlay_skip_format"),
                    )?;
                    if !target_root.is_dir() {
                        return Err(err(
                            "restore target is unavailable for overlay apply (no format requested)",
                        ));
                    }
                }
                if entering_target_erased {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-target-erased",
                        Stage::TargetErased,
                        log,
                    );
                }
            }
            if matches!(task.status, Stage::TargetErased | Stage::ImageApplied) {
                pe_safety::verify_binding(
                    &target.volume,
                    &windows_prepare::volume_identity(target_letter)?,
                    false,
                )?;
                append_log(log, crate::i18n::msg("step_apply"))?;
                let entering_image_applied = task.status == Stage::TargetErased;
                // 应用镜像前是最后一个「还没动目标」的安全暂停点：用户可在这里
                // 停下确认镜像/目标，再决定是否继续。
                if let Some(p) = progress {
                    // Err = 暂停期间窗口被关 / 暂停超时：中断恢复链并按失败处理
                    //（现场保留；auto-escape 时会 disarm + 标终态 + 重启回 Windows）。
                    p.wait_if_paused(log, "STEP 2/4 还原镜像")?;
                }
                recovery_fault::checkpoint("apply", log)?;
                // 【2026-10-22 任务 #110】「覆盖式应用（取消格式化）打到已含
                // Windows 的卷」按准备期落盘的 `overlay_no_acl` 路由执行通道：
                // DISM 会在还原 `Program Files\WindowsApps` 等 TrustedInstaller
                // 保护目录的 SD/owner 时被拒（RestoreDirMetadata 0x80070005 →
                // exit 5，PE 内对照实验钉死，见 docs/20261016-1413 §2）；
                // wimlib 自行获取 SeRestore/SeBackup 提权（POC E5：禁用提权的
                // 父进程下仍 exit 0），且该提权覆盖 SD 还原（2026-10-23 概念
                // 验证 docs/20261022-2308：tiny + 全量两级，镜像 SD 逐字节
                // 还原），RE 上下文同样可用。判据、用户确认与二进制定位
                // 均在准备期/GUI/载荷注入完成，此处只读标记决定走哪条路。
                let overlay_no_acl = task
                    .target
                    .as_ref()
                    .is_some_and(|spec| spec.overlay_no_acl);
                if overlay_no_acl {
                    // RE 内 current_exe（Recovery.exe）就在注入后的 System32，
                    // wimlib 二进制由载荷注入链放在同一目录（winre_payload）。
                    let executable_dir = std::env::current_exe()
                        .map_err(|error| {
                            errc_with(ErrCode::ProgramDirUnresolvable, &[&error.to_string()])
                        })?
                        .parent()
                        .ok_or_else(|| errc(ErrCode::ProgramDirResolveFailed))?
                        .to_path_buf();
                    crate::wimlib_apply::apply_overlay(
                        &executable_dir,
                        &image_path,
                        image_index,
                        &target_root,
                        log,
                        crate::dism_timeout::wait_ms(),
                    )?;
                } else if let Err(error) = run_logged_timeout(
                    "dism.exe",
                    &[
                        "/English",
                        "/Apply-Image",
                        &format!("/ImageFile:{}", image_path.display()),
                        &format!("/Index:{}", image_index),
                        &format!("/ApplyDir:{}", target_root.display()),
                    ],
                    log,
                    // 设置里的 DISM 等待上限（默认 u32::MAX = 不限制，零回归）。
                    // 以前这里写死无限等待，配置项形同虚设。
                    crate::dism_timeout::wait_ms(),
                ) {
                    // 【D1 2026-10-24】DISM apply 失败 → 按判据自动降级 wimlib
                    // 重试一次（设计 docs/20261024-1636；判据/分类/文案单一来源
                    // = failure_policy，编排 = wimlib_apply）。四个 apply 落点
                    // （RE 主链 / PE / 在线数据卷分支 / 在线第二系统分支）共用
                    // 同一个编排函数，避免「PE 降级了、RE 没降级」的分叉。
                    crate::wimlib_apply::fallback_after_dism_failure(
                        &error,
                        log,
                        &image_path,
                        image_index,
                        &target_root,
                    )?;
                }
                if task.status == Stage::TargetErased {
                    store.write_transition(task, Stage::ImageApplied)?;
                }
                if entering_image_applied {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-image-applied",
                        Stage::ImageApplied,
                        log,
                    );
                }
            }
            if matches!(task.status, Stage::ImageApplied | Stage::BootRepaired)
                && target_root
                    .join("Windows")
                    .join("System32\\config\\SYSTEM")
                    .is_file()
            {
                append_log(log, crate::i18n::msg("step_verify"))?;
                let efi = find_efi_root(efi_root)?;
                let entering_boot_repaired = task.status == Stage::ImageApplied;
                // 修引导前暂停点：BCD 尚未被改动，用户在此停下可安全放弃。
                if let Some(p) = progress {
                    p.wait_if_paused(log, "STEP 3/4 校验（引导修复前）")?;
                }
                // BootRepaired is recorded immediately before BCDBoot so a
                // failure is eligible for BCD rollback.  A retry from that
                // stage simply re-runs BCDBoot and validates its output.
                if task.status == Stage::ImageApplied {
                    store.write_transition(task, Stage::BootRepaired)?;
                }
                if entering_boot_repaired {
                    interrupt_after_stage_if_requested(
                        metadata_context,
                        "power-loss-boot-repaired",
                        Stage::BootRepaired,
                        log,
                    );
                }
                if metadata_context
                    .and_then(|values| env_optional(values, "TEST_FAULT"))
                    .as_deref()
                    == Some("bcdboot-failure")
                {
                    return Err(err("development test fault: BCDBoot failure injected"));
                }
                pe_safety::verify_binding(
                    &target.volume,
                    &windows_prepare::volume_identity(target_letter)?,
                    false,
                )?;
                let windows_root = target_root.join("Windows");
                // target_is_secondary 已在引导方案准备处按同一判据声明（default 策略共用），
                // 这里不再重复声明，避免两处判据漂移。
                // Capture the primary BCD state from the byte-for-byte task
                // snapshot before BCDBoot.  It remains available even if a
                // power failure occurs after BCDBoot changes the live store.
                let previous_boot_manager = if target_is_secondary {
                    secondary_registry::repair(&target_root, target_letter, &target.volume, log)?;
                    Some(boot_manager_state_from_task_snapshot(store, task, log)?)
                } else {
                    None
                };
                let mut bcd_args = vec![
                    windows_root.to_string_lossy().into_owned(),
                    "/s".to_string(),
                    efi.to_string_lossy().into_owned(),
                    "/f".to_string(),
                    "UEFI".to_string(),
                    "/v".to_string(),
                ];
                if target_is_secondary {
                    // 2026-10-15 用户规则修订：**不再加 /d**。此前加 /d 是为了让
                    // default 留在原主系统；现在规则反过来——还原完成后 default 就
                    // 该指向本次还原出来的系统，而 bcdboot 的默认行为正是如此。
                    // 旧注释里"失败后用户被拖进半成品系统"的顾虑实际不成立：
                    // 这里是镜像应用完成之后才跑 bcdboot，那一刻目标卷已是完整
                    // Windows，后续失败（WinRE 注册等）不会让它启动不了。
                    // /addlast 只管固件条目的添加位置，与 default 无关，保留——
                    // 菜单顺序最终仍由 boot_menu::apply 按用户方案写定。
                    bcd_args.push("/addlast".to_string());
                    // 顺带给新条目一个可读名字。最终命名仍由引导修复
                    // （boot_menu::apply，最后一个写 BCD 的步骤）按用户意图覆盖；
                    // 但那一步之前若失败，菜单里留下的就是 BCDBoot 的默认描述
                    // （实测为 "Windows PreInstallation Environment"），用户分不清
                    // 这是哪一项。先写上任务里的名字，失败态也能辨认。
                    if let Some(name) = target.boot_menu_name.as_deref() {
                        let name = name.trim();
                        if !name.is_empty() {
                            bcd_args.push("/description".to_string());
                            bcd_args.push(name.to_string());
                        }
                    }
                }
                let bcd_refs: Vec<&str> = bcd_args.iter().map(String::as_str).collect();
                if let Some(entry) = &resume_entry {
                    boot_entry::rearm(entry, log)?;
                }
                recovery_fault::checkpoint("bcdboot-before", log)?;
                run_logged("bcdboot.exe", &bcd_refs, log)?;
                recovery_fault::checkpoint("bcdboot-after", log)?;
                // 显式记录收尾路由与 default 意图（G4）：A 场景（重建唯一系统）
                // 从此留下「default 指向新 loader」的日志，不再只靠 BCD 现状反推。
                append_log(
                    log,
                    if target_is_secondary {
                        crate::i18n::msg("route_secondary")
                    } else {
                        crate::i18n::msg("route_unique")
                    },
                )?;
                // bcdboot 不带 /d 时本就会把 default 指向新 loader，但 /s 自定义
                // ESP 场景下 BFSVC 的行为不完全可控，所以立刻回读一次确认，不对
                // 就当场改。放在这个位置是因为此刻环境已知良好（bcdboot 刚成功、
                // ESP 仍挂载），比等流程失败后在未知状态下补救可靠得多。
                assert_boot_default_points_at_target(&efi, target_letter, log)?;
                if let Some(entry) = &resume_entry {
                    boot_entry::rearm(entry, log)?;
                }
                let boot_manager = efi.join("EFI\\Microsoft\\Boot\\bootmgfw.efi");
                if !boot_manager.exists() {
                    return Err(err("BCDBoot reported success but bootmgfw.efi is missing"));
                }
                verify_bcd_target(&efi, &target_root, log)?;
                let registration = if target_is_secondary {
                    winre_registration::repair_secondary(&target_root, &efi, &target.volume, log)?
                } else {
                    winre_registration::repair(&target_root, &efi, &target.volume, log)?
                };
                // 降级（用户 2026-10-16 裁定，原 P5）：跳过原因已由 preflight 写入
                // 任务日志，结尾复述一次——最终结果必须明确告知，不静默跳过。
                if let winre_registration::RegistrationOutcome::Skipped(reason) = registration {
                    append_log(log, &crate::i18n::msg_tpl("re_registration_degraded", &[reason.as_str()]))?;
                }
                if target_is_secondary {
                    // 菜单名只有「新增第二系统」才由用户指定；把完整 Windows 镜像还原到
                    // 另一个分区时没有这个意图，此时只定位 loader、不改它的描述。
                    // 但 preserve_primary_boot_manager 两种情况都要跑——保住主系统的
                    // 默认启动项与菜单顺序是安全行为，不依赖菜单名。
                    let menu_name = target
                        .boot_menu_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty());
                    let identifier =
                        set_secondary_boot_menu(&efi, &target_root, menu_name, log)?;
                    preserve_primary_boot_manager(
                        &efi,
                        previous_boot_manager.as_ref().expect("secondary BCD state"),
                        &identifier,
                        log,
                    )?;
                }
                // 引导修复必须是**最后**一个写 BCD 的步骤：用户在弹窗里选的名字、
                // 顺序、勾选与菜单秒数是最终意图。早先放在 bcdboot 之前时，
                // 后面的 set_secondary_boot_menu 会用 task.boot_menu_name 覆盖掉改名，
                // preserve_primary_boot_manager 又会按快照顺序重写 displayorder，
                // 把用户的排序和取消勾选一起丢掉——与弹窗里
                // "本列表就是还原完成后的最终启动菜单顺序"的承诺直接矛盾。
                // 目标系统是必选项；其它系统按勾选处理，既有 PE 入口由 apply 保留。
                let boot_menu_plan = boot_menu_session
                    .as_ref()
                    .ok_or_else(|| errc(ErrCode::WindowsVolumeMissingPlan))?
                    .freeze()?;
                crate::boot_menu::apply(&boot_menu_plan, &efi, log, resume_entry.as_ref())?;
                if finalize_success {
                    append_restore_selfcheck(log, task);
                    append_log(log, crate::i18n::msg("step_cleanup"))?;
                    store.write_transition(task, Stage::Success)?;
                }
            } else if matches!(task.status, Stage::ImageApplied | Stage::BootRepaired) {
                // 数据卷还原：目标卷无 SYSTEM hive（非 Windows 系统），跳过 BCDBoot 启动修复。
                append_log(log, crate::i18n::msg("step_verify"))?;
                append_log(
                    log,
                    "data-volume restore: target has no SYSTEM hive; skipping BCDBoot",
                )?;
                if task.status == Stage::ImageApplied {
                    store.write_transition(task, Stage::BootRepaired)?;
                }
                if finalize_success {
                    append_restore_selfcheck(log, task);
                    append_log(log, crate::i18n::msg("step_cleanup"))?;
                    store.write_transition(task, Stage::Success)?;
                }
            } else {
                return Err(err(&format!(
                    "restore cannot resume from stage {:?}",
                    task.status
                )));
            }
        }
    }
    append_log(log, "Recovery completed")?;
    Ok(())
}

/// Development-only fault injection for a power loss after a durable recovery
/// stage.  Reboot immediately without writing `failed` or cleaning WinRE, so
/// a subsequent normal-Windows GUI launch must detect and resume the task.
#[cfg(windows)]
fn interrupt_after_stage_if_requested(
    metadata_context: Option<&BTreeMap<String, String>>,
    fault: &str,
    stage: Stage,
    log: &Path,
) {
    if metadata_context
        .and_then(|values| env_optional(values, "TEST_FAULT"))
        .as_deref()
        != Some(fault)
    {
        return;
    }
    // A simulated power-loss task is deliberately booted again by the normal
    // GUI. Persist a one-shot marker before rebooting so the same injected
    // fault cannot fire forever when the resumed stage is re-entered.
    let marker = log.with_file_name(format!(".fault-{fault}.triggered"));
    if marker.is_file() {
        return;
    }
    if fs::write(&marker, format!("stage={stage:?}\n")).is_err() {
        let _ = append_log(
            log,
            "Development test fault marker could not be persisted; refusing injection",
        );
        return;
    }
    let _ = append_log(
        log,
        &format!(
            "Development test fault: simulating power loss after durable stage {stage:?}; rebooting without cleanup"
        ),
    );
    let _ = run_logged("wpeutil.exe", &["reboot"], log);
    std::process::exit(0);
}

/// 记录的身份与现场身份是否指向**同一个卷**。
///
/// 判据全部是「格式化也不会变」的稳定标识：卷 GUID、分区 GUID、磁盘 GUID +
/// 分区偏移/大小。**绝不**用盘符或卷序列号——盘符重启就重排（D2 的根因），
/// 序列号格式化就变。
#[cfg(windows)]
fn volume_identity_matches(
    recorded: &backuprestore_core::VolumeIdentity,
    live: &backuprestore_core::VolumeIdentity,
) -> bool {
    if !recorded.volume_guid.is_empty()
        && crate::text_parsing::same_volume(&recorded.volume_guid, &live.volume_guid)
    {
        return true;
    }
    if !recorded.partition_guid.is_empty()
        && recorded.partition_guid.eq_ignore_ascii_case(&live.partition_guid)
        && recorded.partition_offset == live.partition_offset
        && recorded.partition_size == live.partition_size
    {
        return true;
    }
    // 老任务（副档里没有卷 GUID/分区 GUID）的兜底：磁盘 GUID + 偏移 + 大小。
    // 偏移必须非零，否则「同一块盘上偏移 0 的假匹配」会把别的卷当成目标。
    !recorded.disk_guid.is_empty()
        && recorded.partition_offset > 0
        && recorded.disk_guid.eq_ignore_ascii_case(&live.disk_guid)
        && recorded.partition_offset == live.partition_offset
        && recorded.partition_size == live.partition_size
}

/// 把任务记录的卷身份解析成**本次运行里真实可用**的盘符。
///
/// 【2026-10-19 D2】旧实现直接用任务里记的盘符。PE/WinRE 的盘符是启动时按
/// 发现顺序分配的，与准备阶段记的字母没有任何契约：实机里记录 `H:` 的镜像卷
/// 在 PE 里变成了 `E:`，于是 `failed to read image metadata (os error 3)`——
/// 连"哪个卷没找到"都看不出来。现在按顺序：
///   ① 记录盘符（必须复核身份，防止它已经指向别的卷）；
///   ② `mountvol` 现挂载表里按卷 GUID 找它当前的字母；
///   ③ 逐字母探测（老任务没有卷 GUID 时的兜底）；
///   ④ 卷在、但当前没有盘符（PE 常见）→ 按 GUID 分配一个空闲字母。
#[cfg(windows)]
fn resolve_volume_letter(
    volume: &backuprestore_core::VolumeIdentity,
    log: &Path,
) -> Result<char, TaskError> {
    let mut tried: Vec<String> = Vec::new();
    if let Some(letter) = volume.drive_letter {
        match crate::windows_prepare::volume_identity(letter) {
            Ok(live) if volume_identity_matches(volume, &live) => {
                append_log(
                    log,
                    &crate::i18n::msg_tpl("volume_resolved_same_letter", &[&format!("{letter}")]),
                )?;
                return Ok(letter);
            }
            Ok(live) => tried.push(format!(
                "{letter}: 已被别的卷占用（现场卷 GUID {}）",
                live.volume_guid
            )),
            Err(error) => tried.push(format!("{letter}: 不可读（{error}）")),
        }
    }
    let listing = match enumerate_mounted_volumes(log) {
        Ok(listing) => listing,
        Err(error) => {
            append_log(
                log,
                &crate::i18n::msg_tpl("mountvol_table_unavailable", &[&error.to_string()]),
            )?;
            Vec::new()
        }
    };
    if !volume.volume_guid.is_empty() {
        for (guid, letters) in &listing {
            if !crate::text_parsing::same_volume(guid, &volume.volume_guid) {
                continue;
            }
            for &letter in letters {
                if let Ok(live) = crate::windows_prepare::volume_identity(letter)
                    && volume_identity_matches(volume, &live)
                {
                    append_log(
                        log,
                        &crate::i18n::msg_tpl(
                            "volume_resolved_by_guid",
                            &[&format!("{letter}"), &format!("{:?}", volume.drive_letter)],
                        ),
                    )?;
                    return Ok(letter);
                }
            }
        }
    }
    // 逐字母探测：老任务可能没有卷 GUID，或 mountvol 输出不可解析。
    for letter in ('A'..='Z').filter(|letter| !matches!(letter, 'A' | 'B' | 'X')) {
        match crate::windows_prepare::volume_identity(letter) {
            Ok(live) if volume_identity_matches(volume, &live) => {
                append_log(
                    log,
                    &crate::i18n::msg_tpl(
                        "volume_resolved_by_probe",
                        &[&format!("{letter}"), &format!("{:?}", volume.drive_letter)],
                    ),
                )?;
                return Ok(letter);
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    // 卷存在但没有盘符：按卷 GUID 分配一个空闲字母（PE 里恢复分区/ESP 常见）。
    if !volume.volume_guid.is_empty()
        && let Ok(letter) = assign_letter_by_volume_guid(volume, &listing, log)
    {
        return Ok(letter);
    }
    Err(errc_with(
        ErrCode::VolumeRecordUnresolvable,
        &[
            if volume.volume_guid.is_empty() {
                "<未记录>"
            } else {
                volume.volume_guid.as_str()
            },
            &format!("{:?}", volume.disk_number),
            &format!("{:?}", volume.partition_number),
            &volume.partition_size.to_string(),
            &format!("{:?}", volume.drive_letter),
            &if tried.is_empty() {
                "无".to_string()
            } else {
                tried.join("；")
            },
        ],
    ))
}

/// 卷存在但当前没有盘符时，按卷 GUID 用 `mountvol` 分配一个空闲字母并复核身份。
#[cfg(windows)]
fn assign_letter_by_volume_guid(
    volume: &backuprestore_core::VolumeIdentity,
    listing: &[(String, Vec<char>)],
    log: &Path,
) -> Result<char, TaskError> {
    let bare = crate::text_parsing::bare_volume_guid(&volume.volume_guid)
        .and_then(|value| crate::text_parsing::require_guid(value, "卷").ok())
        .ok_or_else(|| errc(ErrCode::VolumeGuidMalformed))?;
    let path = format!(r"\\?\Volume{bare}\");
    let mounts = MountedVolumes::new();
    for letter in mount_letter_candidates(volume.drive_letter.unwrap_or('Z'), listing, &mounts) {
        if !matches!(query_mounted_volume(letter, log)?, VolumeMountQuery::Unmounted) {
            continue;
        }
        let out = log.with_extension(format!("mountvol-{letter}.out"));
        let _ = fs::remove_file(&out);
        let outcome = windows_command::run_program(
            "mountvol.exe",
            &[format!("{letter}:"), path.clone()],
            Some(&out),
            30_000,
        );
        let text = fs::read(&out)
            .map(|bytes| crate::text_parsing::decode_windows_bytes(&bytes))
            .unwrap_or_default();
        let _ = fs::remove_file(&out);
        if !outcome.is_success() {
            append_log(
                log,
                &crate::i18n::msg_tpl(
                    "assign_letter_failed",
                    &[&format!("{letter}").as_str(), &format!("{outcome}").as_str(), text.as_str()],
                ),
            )?;
            continue;
        }
        let live = crate::windows_prepare::volume_identity(letter)?;
        if volume_identity_matches(volume, &live) {
            append_log(
                log,
                &crate::i18n::msg_tpl("assign_letter_ok", &[&format!("{letter}")]),
            )?;
            return Ok(letter);
        }
        append_log(
            log,
            &crate::i18n::msg_tpl(
            "assign_letter_mismatch",
            &[&format!("{letter}"), &live.volume_guid],
        ),
        )?;
    }
    Err(errc(ErrCode::VolumeExistsNoLetter))
}

/// 按磁盘号 + 分区号找该分区**当前**的盘符（没有盘符返回 `None`）。
///
/// 用途：WinRE 落在独立恢复分区时，BCD 恢复对象可能用盘符写法，而那个盘符是
/// 注册当时的临时字母。这里用磁盘/分区编号（稳定）反查现场字母，供
/// `winre_registration` 核对恢复对象到底指向哪个分区。
#[cfg(windows)]
pub(crate) fn letter_for_partition(
    disk: u32,
    partition: u32,
    log: &Path,
) -> Result<Option<char>, TaskError> {
    for letter in ('C'..='Z').filter(|letter| !matches!(letter, 'X')) {
        let Ok(live) = crate::windows_prepare::volume_identity(letter) else {
            continue;
        };
        if live.disk_number == Some(disk) && live.partition_number == Some(partition) {
            append_log(
                log,
                &crate::i18n::msg_tpl(
                    "device_has_letter",
                    &[&format!("{disk}"), &format!("{partition}"), &format!("{letter}")],
                ),
            )?;
            return Ok(Some(letter));
        }
    }
    append_log(
        log,
        &crate::i18n::msg_tpl(
            "device_no_letter",
            &[&format!("{disk}"), &format!("{partition}")],
        ),
    )?;
    Ok(None)
}

#[cfg(windows)]
fn resolve_volume_root(
    volume: &backuprestore_core::VolumeIdentity,
    log: &Path,
) -> Result<PathBuf, TaskError> {
    Ok(PathBuf::from(format!(
        "{}:\\",
        resolve_volume_letter(volume, log)?
    )))
}

/// 直接恢复（`recover` 命令）的 EFI 根解析。
///
/// 【2026-10-19 D3】系统还原必须写 BCD，而 PE 里 ESP 默认没有盘符。旧代码把
/// `find_efi_root` 放在 STEP 3（镜像灌完之后），于是"不传 `--efi-root` 就白灌
/// 一遍镜像"。现在在 STEP 1 之前解析：优先用任务 env 里记录的 EFI 卷身份，
/// 其次免盘符探测 ESP，两条路都失败就在**任何写盘动作之前**硬失败。
///
/// 数据卷还原（目标不是 Windows 系统卷）不需要 BCD，返回 `None` 即可，不得
/// 因为"找不到 ESP"而阻断一个本来不需要引导的还原。
#[cfg(windows)]
fn resolve_efi_root_for_recovery(
    task: &Task,
    log: &Path,
    values: Option<&BTreeMap<String, String>>,
) -> Result<Option<PathBuf>, TaskError> {
    if !matches!(
        task.operation,
        Operation::RestoreExisting | Operation::CreateSecondary
    ) {
        return Ok(None);
    }
    let target = task.target.as_ref().ok_or_else(|| err("missing target"))?;
    let target_root = resolve_volume_root(&target.volume, log)?;
    // 判据与收尾路由一致：目标当前是 Windows 系统卷，或本次要把完整系统装进
    // 一个空卷（NewWindows）。数据卷还原两个条件都不满足 → 不需要 EFI。
    let target_is_windows = pe_safety::target_is_windows_volume(&target_root).unwrap_or(false);
    if !target_is_windows && task.target.as_ref().map(|t| t.role.clone()) != Some(backuprestore_core::TargetRole::NewWindows)
    {
        append_log(
            log,
            crate::i18n::msg("data_volume_skip_efi"),
        )?;
        return Ok(None);
    }
    if let Some(values) = values
        && values.contains_key("EFI_VOLUME_GUID")
    {
        let mut mounts = MountedVolumes::new();
        match mount_env_volume(&mut mounts, values, "EFI", 'Z', log, false) {
            Ok(letter) => {
                append_log(
                    log,
                    &crate::i18n::msg_tpl("esp_mounted_early", &[&format!("{letter}")]),
                )?;
                return Ok(Some(PathBuf::from(format!("{letter}:\\"))));
            }
            Err(error) => {
                append_log(
                    log,
                    &crate::i18n::msg_tpl("esp_mount_failed", &[&error.to_string()]),
                )?;
            }
        }
    }
    let identity = crate::windows_prepare::esp_identity_without_drive_letter()?;
    let letter = resolve_volume_letter(&identity, log)?;
    append_log(
        log,
        &crate::i18n::msg_tpl("esp_probed_early", &[&format!("{letter}")]),
    )?;
    Ok(Some(PathBuf::from(format!("{letter}:\\"))))
}

#[cfg(windows)]
fn verify_partition_identity_after_format(
    expected: &backuprestore_core::VolumeIdentity,
    log: &Path,
) -> Result<(), TaskError> {
    let letter = expected
        .drive_letter
        .ok_or_else(|| err("formatted restore target lost its drive letter"))?;
    let actual = crate::windows_prepare::volume_identity(letter)?;
    for (name, left, right) in [
        (
            "disk GUID",
            expected.disk_guid.as_str(),
            actual.disk_guid.as_str(),
        ),
        (
            "partition GUID",
            expected.partition_guid.as_str(),
            actual.partition_guid.as_str(),
        ),
        (
            "partition type",
            expected.partition_type_guid.as_str(),
            actual.partition_type_guid.as_str(),
        ),
    ] {
        if !left.eq_ignore_ascii_case(right) {
            return Err(err(&format!(
                "formatted restore target {name} changed unexpectedly"
            )));
        }
    }
    if expected.disk_number != actual.disk_number
        || expected.partition_number != actual.partition_number
        || expected.partition_offset != actual.partition_offset
        || expected.partition_size != actual.partition_size
    {
        return Err(err(
            "formatted restore target disk/partition geometry changed unexpectedly",
        ));
    }
    append_log(log, "Formatted target partition identity re-verified")?;
    Ok(())
}

#[cfg_attr(not(windows), allow(dead_code))]
fn diskpart_format_script(
    disk_number: Option<u32>,
    partition_number: Option<u32>,
) -> Result<String, TaskError> {
    let disk_number = disk_number
        .ok_or_else(|| err("restore target has no verified disk number for DiskPart formatting"))?;
    let partition_number = partition_number.ok_or_else(|| {
        err("restore target has no verified partition number for DiskPart formatting")
    })?;
    if partition_number == 0 {
        return Err(err(
            "restore target has an invalid partition number for DiskPart",
        ));
    }
    Ok(format!(
        "select disk {disk_number}\r\nselect partition {partition_number}\r\nformat fs=ntfs quick\r\n"
    ))
}

#[cfg(windows)]
fn format_target_partition(
    store: &TaskStore,
    task: &Task,
    target: &backuprestore_core::VolumeIdentity,
    log: &Path,
) -> Result<(), TaskError> {
    let letter = target
        .drive_letter
        .ok_or_else(|| errc(ErrCode::FormatTargetLetterMissing))?;
    // 在生成写盘命令前再次严格比较此次绑定，拒绝核验后发生的编号变化。
    pe_safety::verify_binding(target, &windows_prepare::volume_identity(letter)?, false)?;
    let task_dir = store.task_dir(&task.task_id)?;
    let script = task_dir.join("diskpart-format.txt");
    let body = diskpart_format_script(target.disk_number, target.partition_number)?;
    fs::write(&script, body)?;
    let script_arg = script.to_string_lossy().into_owned();
    append_log(
        log,
        &format!(
            "Formatting verified target with diskpart.exe: disk={} partition={} script={script_arg}",
            target.disk_number.unwrap_or_default(),
            target.partition_number.unwrap_or_default(),
        ),
    )?;
    run_logged("diskpart.exe", &["/s", script_arg.as_str()], log)
}
#[cfg(windows)]
fn resolve_volume_path(
    volume: &backuprestore_core::VolumeIdentity,
    relative: &str,
    log: &Path,
) -> Result<PathBuf, TaskError> {
    Ok(resolve_volume_root(volume, log)?
        .join(relative.replace('\\', std::path::MAIN_SEPARATOR_STR)))
}
#[cfg(windows)]
fn find_efi_root(override_root: Option<&Path>) -> Result<PathBuf, TaskError> {
    override_root
        .map(Path::to_path_buf)
        .ok_or_else(|| err("EFI root must be explicitly mounted before recovery"))
}

#[cfg(windows)]
fn wim_indexes(path: &Path, log: &Path) -> Result<Vec<u32>, TaskError> {
    let output = capture_logged(
        "dism.exe",
        &[
            "/English",
            "/Get-WimInfo",
            &format!("/WimFile:{}", path.display()),
        ],
        log,
    )?;
    let mut indexes = crate::windows_prepare::parse_dism_images(&output)?
        .iter()
        .filter_map(|image| image.get("ImageIndex").and_then(serde_json::Value::as_u64))
        .filter_map(|index| u32::try_from(index).ok())
        .collect::<Vec<_>>();
    indexes.sort_unstable();
    indexes.dedup();
    if indexes.is_empty() || indexes.contains(&0) {
        return Err(err("DISM returned invalid WIM indexes"));
    }
    Ok(indexes)
}

#[cfg(windows)]
fn run_logged(program: &str, args: &[&str], log: &Path) -> Result<(), TaskError> {
    run_logged_timeout(program, args, log, u32::MAX)
}

/// 等子进程结束；`wait_ms == u32::MAX` 时**等到底**（旧行为，任何调用点都不受影响）。
///
/// 超时返回 `Ok(None)`：进程已被 kill 并回收，调用方随后 join 读取线程，
/// 再把「超时」如实报错——这样输出尾部照样进日志，失败仍可归因。
#[cfg(windows)]
fn wait_child(
    child: &mut std::process::Child,
    wait_ms: u32,
) -> Result<Option<std::process::ExitStatus>, TaskError> {
    if wait_ms == u32::MAX {
        return Ok(Some(child.wait()?));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms as u64);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// 带等待上限的 `run_logged`。
///
/// 【2026-10-25 真缺陷修复】此前 apply 走的是无条件 `child.wait()`——于是
/// `--dism-timeout-secs` 在恢复环境里**只记了一行**「DISM wait limit: N sec」，
/// 实际等待仍是无限的：
///   ① 用户以为配了逃生阀，卡死时照样卡死；
///   ② 超时类失败永远走不到 `classify_apply_failure`，D1 的 wimlib 降级
///      在恢复环境主链上**永不触发**（实机：`--dism-timeout-secs 1` 跑 38GB
///      镜像，6 分钟正常完成、日志里没有降级行，才把这条挖出来）。
#[cfg(windows)]
fn run_logged_timeout(
    program: &str,
    args: &[&str],
    log: &Path,
    wait_ms: u32,
) -> Result<(), TaskError> {
    append_log(log, &format!("running {program} {}", args.join(" ")))?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    if let Err(error) = recovery_fault::pause_running_bcdboot(program, &mut child, log) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let log_file = OpenOptions::new().create(true).append(true).open(log)?;
    let sink = Arc::new(Mutex::new(log_file));
    let stdout: ChildStdout = child
        .stdout
        .take()
        .ok_or_else(|| err(&format!("{program} stdout was not piped")))?;
    let stderr: ChildStderr = child
        .stderr
        .take()
        .ok_or_else(|| err(&format!("{program} stderr was not piped")))?;
    let stdout_sink = Arc::clone(&sink);
    let stdout_thread = thread::spawn(move || stream_to_log("stdout", stdout, stdout_sink));
    let stderr_sink = Arc::clone(&sink);
    let stderr_thread = thread::spawn(move || stream_to_log("stderr", stderr, stderr_sink));
    let status = wait_child(&mut child, wait_ms)?;
    stdout_thread
        .join()
        .map_err(|_| err(&format!("{program} stdout reader panicked")))??;
    stderr_thread
        .join()
        .map_err(|_| err(&format!("{program} stderr reader panicked")))??;
    let Some(status) = status else {
        return Err(errc_with(
            ErrCode::ProgramTimeout,
            &[program, &wait_ms.to_string()],
        ));
    };
    if !status.success() {
        // 【D2 2026-10-24】失败回执带上输出尾部：外部命令的真实原因写在
        // stdout/stderr 里（已由上面的 reader 线程落进日志文件），只回退出码
        // 等于现场无法归因——分类判据（classify_apply_failure）也需要这段文本
        // 作为输入。tail_text 是跨平台纯文件读，截断宽容忍解码。
        let tail = crate::text_parsing::tail_text(log, crate::text_parsing::LOG_TAIL_MAX_BYTES);
        return Err(errc_with(
            ErrCode::ProgramFailedNamed,
            &[
                program,
                &format!("{status}"),
                &crate::text_parsing::LOG_TAIL_MAX_BYTES.to_string(),
                &tail,
            ],
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn capture_logged(program: &str, args: &[&str], log: &Path) -> Result<String, TaskError> {
    append_log(log, &format!("capturing {program} {}", args.join(" ")))?;
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    let mut text = crate::text_parsing::decode_windows_bytes(&output.stdout);
    text.push_str(&crate::text_parsing::decode_windows_bytes(&output.stderr));
    if !text.is_empty() {
        append_log(log, &text)?;
    }
    if !output.status.success() {
        return Err(err(&format!("{program} failed with {}", output.status)));
    }
    Ok(text)
}

/// 跑一个命令并把 stdout/stderr **整体落进日志**，无论退出码如何都返回原始 `Output`。
///
/// 与 `capture_logged` 的分工：那个在非零退出时用 `?` 把输出**扔掉**，只留
/// "failed with <status>"。而 DISM 的真实原因就写在 stdout/stderr 里——只丢
/// 这一句等于把唯一线索丢了。三处 DISM 预检（`windows_prepare` 的
/// Mount/Unmount-Wim 与 Get-WimInfo）此前只取 `status`，导致 GUI 点「创建任务」
/// 之后最常见的第一道失败在 `prepare.log` 里只剩一句无原因的错误，事后只能
/// 盲猜重试（2026-10-16 审查结论，见 docs/20261016-2250 日志缺口 1）。
///
/// 输出整体落盘（不截断），避免"摘要里没有的那一行恰好是关键行"。
#[cfg(windows)]
pub(crate) fn run_output_logged(
    program: &str,
    args: &[&str],
    log: &Path,
) -> Result<Output, TaskError> {
    append_log(log, &format!("running {program} {}", args.join(" ")))?;
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    let mut text = crate::text_parsing::decode_windows_bytes(&output.stdout);
    text.push_str(&crate::text_parsing::decode_windows_bytes(&output.stderr));
    if !text.is_empty() {
        append_log(log, &text)?;
    }
    Ok(output)
}

/// 取输出尾部若干非空行做**错误原因摘要**，用于拼进返回给用户/上层日志的错误消息。
///
/// 只做尾部而非全文：错误消息要能在 GUI 弹窗和终端里一行读完，整段 DISM 输出
/// 会把它撑爆。完整输出已由 `run_output_logged` 落盘，这里只负责"让错误本身
/// 自解释"。
#[cfg(windows)]
pub(crate) fn output_tail_summary(output: &Output, limit: usize) -> String {
    let mut text = crate::text_parsing::decode_windows_bytes(&output.stdout);
    text.push_str(&crate::text_parsing::decode_windows_bytes(&output.stderr));
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let start = lines.len().saturating_sub(limit);
    lines[start..].join(" | ")
}

#[cfg(any(windows, test))]
fn bcd_identifier_from_line(line: &str) -> Option<&str> {
    let start = line.find('{')?;
    let end = line[start..].find('}')? + start + 1;
    let identifier = &line[start..end];
    (identifier.len() == 38
        && identifier.starts_with('{')
        && identifier.ends_with('}')
        && identifier[1..37]
            .chars()
            .enumerate()
            .all(|(index, character)| {
                matches!(index, 8 | 13 | 18 | 23)
                    .then_some(character == '-')
                    .unwrap_or_else(|| character.is_ascii_hexdigit())
            }))
    .then_some(identifier)
}

#[cfg(windows)]
fn set_secondary_boot_menu(
    efi_root: &Path,
    target_root: &Path,
    // None = 调用方没有命名意图（把完整 Windows 还原到另一个分区），此时只定位
    // loader 标识符给 preserve_primary_boot_manager 用，不改它的描述。
    menu_name: Option<&str>,
    log: &Path,
) -> Result<String, TaskError> {
    let store = efi_root.join("EFI\\Microsoft\\Boot\\BCD");
    if !store.exists() {
        return Err(err("BCD store is missing after BCDBoot"));
    }
    let store_arg = store.to_string_lossy().into_owned();
    let output = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let letter = target_root
        .to_string_lossy()
        .chars()
        .next()
        .ok_or_else(|| err("secondary target has no drive letter"))?
        .to_ascii_lowercase();
    let target_needle = format!("partition={letter}:");
    // Parse blank-line-separated BCD objects. The first GUID in a block is
    // the object identifier; later GUIDs belong to recovery/resume/inherit
    // references. Restrict matches to a Windows loader path so a
    // winresume.efi object cannot receive the secondary menu description.
    let normalized_output = output.replace("\r\n", "\n");
    let identifier = normalized_output
        .split("\n\n")
        .filter_map(|block| {
            let identifier = block.lines().find_map(bcd_identifier_from_line)?;
            let lower = block.to_ascii_lowercase();
            let is_loader = lower.contains("winload.efi");
            let targets_partition = lower.lines().any(|line| {
                let line = line.trim_start();
                (line.starts_with("device")
                    || line.starts_with("osdevice")
                    || line.starts_with("设备")
                    || line.starts_with("os 设备"))
                    && line.contains(&target_needle)
            });
            (is_loader && targets_partition).then_some(identifier.to_string())
        })
        .next()
        .ok_or_else(|| {
            err("BCDBoot created no Windows loader tied to the selected target partition")
        })?;
    let Some(menu_name) = menu_name else {
        append_log(
            log,
            &format!("Secondary Windows loader {identifier} kept its BCDBoot description"),
        )?;
        return Ok(identifier);
    };
    let description_args = [
        "/store",
        store_arg.as_str(),
        "/set",
        identifier.as_str(),
        "description",
        menu_name,
    ];
    run_logged("bcdedit.exe", &description_args, log)?;
    let verify = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    if !verify
        .to_ascii_lowercase()
        .contains(&menu_name.to_ascii_lowercase())
    {
        return Err(err("BCD menu name was not visible after update"));
    }
    append_log(
        log,
        &format!("Secondary Windows loader {identifier} named {menu_name}"),
    )?;
    Ok(identifier.clone())
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct BcdBootManagerState {
    default: String,
    display_order: Vec<String>,
    /// Boot Manager 自身的 resumeobject——BCDBoot 改写 `{bootmgr}` 时会把它指向
    /// 新 loader 的休眠恢复程序，断电矩阵实测发现我们此前只恢复 default/displayorder，
    /// 该字段被默默换掉且无人校验。主系统 loader 自己的 resumeobject 不受影响。
    resume_object: Option<String>,
    /// 快照中每个 Windows loader 到其 resumeobject 的映射，用来在替换第二系统时
    /// 精确回收旧 loader 的孤儿 resume 对象；不按描述文字或“所有未引用对象”盲删。
    loader_resume_objects: Vec<(String, String)>,
}

/// Read the original Boot Manager state from the BCD snapshot captured while
/// preparing the task. BCDBoot documents that `/addlast` is ignored when `/s`
/// specifies a volume, and it can replace Boot Manager's default with the new
/// loader. The snapshot is therefore the source of truth across retries.
#[cfg(windows)]
fn boot_manager_state_from_task_snapshot(
    store: &TaskStore,
    task: &Task,
    log: &Path,
) -> Result<BcdBootManagerState, TaskError> {
    let task_dir = store.task_dir(&task.task_id)?;
    let text_snapshot = task_dir.join("bcd-before-bootmgr.txt");
    if text_snapshot.is_file() {
        let output = fs::read_to_string(&text_snapshot)?;
        return parse_boot_manager_state(&output)
            .ok_or_else(|| err("saved Boot Manager state is missing or invalid"));
    }
    let raw_snapshot = task_dir.join("bcd-before-raw");
    let snapshot = if raw_snapshot.is_file() {
        raw_snapshot
    } else {
        task_dir.join("bcd-before-export")
    };
    if !snapshot.is_file() {
        return Err(err(
            "task BCD snapshot is missing before secondary boot repair",
        ));
    }
    boot_manager_state_from_store(&snapshot, log)
}

#[cfg(windows)]
fn boot_manager_state_from_store(
    store: &Path,
    log: &Path,
) -> Result<BcdBootManagerState, TaskError> {
    let store_arg = store.to_string_lossy().into_owned();
    let output = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    parse_boot_manager_state(&output)
        .ok_or_else(|| err("existing Boot Manager state is missing or invalid"))
}

/// 确认默认启动项指向**本次还原出来的系统**；不对就当场改过去。
///
/// 2026-10-15 用户规则：还原完成后 default 一律指向新还原的系统（此前第二系统
/// 场景刻意保留原主系统 default，用 bcdboot `/d` 换取"失败后不会进半成品系统"，
/// 已按用户要求翻转——见 `boot_menu::decide_default` 的注释）。
///
/// bcdboot 不带 `/d` 时本就会把 default 设成新 loader，但本项目所有 bcdboot 都
/// 带 `/s` 自定义 ESP，BFSVC 在该场景下对默认项的处理不完全可控，所以这里回读
/// 一次确认；不一致就写一次，仍不一致则拒绝继续后续破坏性步骤（WinRE 注册等）。
/// 只动 `default` 一项，不动 `displayorder`——菜单顺序由 `boot_menu::apply`
/// 按用户方案统一写定。
#[cfg(windows)]
fn assert_boot_default_points_at_target(
    efi_root: &Path,
    target_letter: char,
    log: &Path,
) -> Result<(), TaskError> {
    let store = efi_root.join("EFI\\Microsoft\\Boot\\BCD");
    let store_arg = store.to_string_lossy().into_owned();
    // 目标卷的 loader：按分区找，不按"bcdboot 建了哪个"猜。
    let all = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let loader = crate::boot_menu::loader_identifier_for_partition(&all, target_letter)?
        .ok_or_else(|| errc(ErrCode::BcdbootLoaderMissing))?;
    // `/enum {bootmgr} /v`：/v 才输出 GUID 而非 {bootmgr}/{default} 别名，
    // 解析器与别名比对都依赖这一点（2026-10-13 实测确认）。
    let read_default = |log: &Path| -> Result<String, TaskError> {
        let text = capture_logged(
            "bcdedit.exe",
            &["/store", &store_arg, "/enum", "{bootmgr}", "/v"],
            log,
        )?;
        parse_boot_manager_state(&text)
            .map(|state| state.default)
            .ok_or_else(|| errc(ErrCode::PostBcdbootBootMgrUnparsable))
    };
    let current = read_default(log)?;
    if current.eq_ignore_ascii_case(&loader) {
        append_log(log, &crate::i18n::msg_tpl("default_switched", &[loader.as_str()]))?;
        return Ok(());
    }
    append_log(
        log,
        &crate::i18n::msg_tpl(
            "default_switching",
            &[&format!("{current}").as_str(), loader.as_str()],
        ),
    )?;
    run_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/default", &loader],
        log,
    )?;
    let after = read_default(log)?;
    if !after.eq_ignore_ascii_case(&loader) {
        return Err(errc(ErrCode::DefaultSwitchFailed));
    }
    append_log(log, crate::i18n::msg("default_already_switched"))?;
    Ok(())
}

/// 保留仍存在的菜单项并追加第二系统；不复活 BCDBoot 删除的旧编号。
///
/// default 归属**不在**本函数处理：它由 boot_menu::apply 调
/// `boot_menu::decide_default` 决定（2026-10-15 用户规则：一律指向本次还原的
/// 系统；该规则 v2.6.4 起已无分支）。
#[cfg(windows)]
fn preserve_primary_boot_manager(
    efi_root: &Path,
    previous: &BcdBootManagerState,
    secondary_identifier: &str,
    log: &Path,
) -> Result<(), TaskError> {
    let store = efi_root.join("EFI\\Microsoft\\Boot\\BCD");
    let store_arg = store.to_string_lossy().into_owned();
    let live = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let expected_order = secondary_boot_order(previous, secondary_identifier, &live)?;
    // **不再在这里写 default**（2026-10-15 用户规则修订）：default 归属的唯一
    // 权威是 boot_menu::apply（调 decide_default），两个场景都指向本次
    // 还原出来的系统。此前这里把它写回 `previous.default`，随后又被 apply 覆盖，
    // 属于自相矛盾的中间态。本函数继续负责的是"原菜单项一个不少 + 追加新项 +
    // 恢复休眠引用"这三件事。
    if let Some(resume_object) = &previous.resume_object {
        // BCDBoot 会把 {bootmgr}.resumeobject 换成新 loader 的恢复程序；快照里
        // 有记录才恢复——旧 BCD 可能本来就没有该字段，不能凭空造一个出来。
        run_logged(
            "bcdedit.exe",
            &[
                "/store",
                &store_arg,
                "/set",
                "{bootmgr}",
                "resumeobject",
                resume_object,
            ],
            log,
        )?;
    } else if parse_boot_manager_state(&live)
        .and_then(|state| state.resume_object)
        .is_some()
    {
        // 快照里**没有** resumeobject，而 BCDBoot 为新系统凭空加了一个（它是新 loader
        // 的兄弟对象，GUID 只差末位）。必须删掉：
        //   ① 留着会让休眠恢复走进第二系统；
        //   ② 下面的三项校验把 (live=Some, 快照=None) 判为失配，于是任务在
        //      BootRepaired 永久失败，而且**每次重试都会同样失败**——
        //      2026-10-14 阶段 3 实测就栽在这里：default 与 displayorder 都已正确
        //      恢复，唯一失配项就是这个 BCDBoot 新增的 resumeobject。
        run_logged(
            "bcdedit.exe",
            &[
                "/store",
                &store_arg,
                "/deletevalue",
                "{bootmgr}",
                "resumeobject",
            ],
            log,
        )?;
    }
    let mut display_args = vec![
        "/store".to_string(),
        store_arg.clone(),
        "/displayorder".to_string(),
    ];
    display_args.extend(expected_order.iter().cloned());
    let display_refs: Vec<&str> = display_args.iter().map(String::as_str).collect();
    run_logged("bcdedit.exe", &display_refs, log)?;
    let verified_text = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let verified = parse_boot_manager_state(&verified_text)
        .ok_or_else(|| err("Boot Manager state could not be read after preserving order"))?;
    let resume_object_matches = match (&verified.resume_object, &previous.resume_object) {
        (None, None) => true,
        (Some(actual), Some(expected)) => actual.eq_ignore_ascii_case(expected),
        _ => false,
    };
    // default 不再属于本函数的验收范围（归 boot_menu::apply，见上方注释）。
    if !resume_object_matches
        || verified.display_order.len() != expected_order.len()
        || verified
            .display_order
            .iter()
            .zip(&expected_order)
            .any(|(actual, expected)| !actual.eq_ignore_ascii_case(expected))
    {
        return Err(err("Secondary display order could not be preserved"));
    }
    cleanup_orphaned_secondary_resume_objects(previous, &verified_text, log)?;
    append_log(
        log,
        &format!(
            "Restored {} original display-order entries and appended secondary loader {secondary_identifier} (default left to the boot-menu plan)",
            expected_order.len().saturating_sub(1),
        ),
    )?;
    Ok(())
}

#[cfg(windows)]
fn cleanup_orphaned_secondary_resume_objects(
    previous: &BcdBootManagerState,
    live: &str,
    log: &Path,
) -> Result<(), TaskError> {
    let normalized_live = live.replace("\r\n", "\n");
    let live_blocks: Vec<&str> = normalized_live.split("\n\n").collect();
    let live_ids: std::collections::BTreeSet<String> = live_blocks
        .iter()
        .filter_map(|block| block.lines().find_map(bcd_identifier_from_line))
        .map(str::to_ascii_lowercase)
        .collect();
    let live_resume_refs: std::collections::BTreeSet<String> = live_blocks
        .iter()
        .flat_map(|block| block.lines())
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .to_ascii_lowercase()
                .starts_with("resumeobject")
                .then(|| bcd_value_from_line(trimmed).map(str::to_ascii_lowercase))
                .flatten()
        })
        .collect();
    for (loader, resume) in &previous.loader_resume_objects {
        if live_ids.contains(&loader.to_ascii_lowercase())
            || live_resume_refs.contains(&resume.to_ascii_lowercase())
        {
            continue;
        }
        let guid = crate::text_parsing::require_guid(resume, "remove orphan resume object")
            .map_err(|message| err(&message))?;
        // 清理孤儿 resume object 是**收尾性质**的动作：目标条目可能已被 bcdboot 或
        // 早前步骤移除，此时 bcdedit 以 exit code 1 报「找不到元素」——目的本已达成
        // （它就该不存在），绝不能因此把整任务判失败。
        // 2026-10-14 实测（任务 0fa52c1d）：create-secondary 的格式化 / 镜像应用 /
        // bcdboot / WinRE 注册 / 菜单命名 / default 恢复**全部成功**，仅此处因目标
        // GUID 已不存在而失败 → 任务被判失败并「保留 boot entry 与载荷」→ 下次启动
        // bootsequence 仍指向失效项报 0xc000000e，被迫进 PE 人工清引导。
        match run_logged("bcdedit.exe", &["/delete", &guid, "/f"], log) {
            Ok(()) => append_log(
                log,
                &format!("Removed orphan resume object {guid} for replaced loader {loader}"),
            )?,
            Err(error) => append_log(
                log,
                &format!(
                    "Orphan resume object {guid} for replaced loader {loader} is already absent (deletion skipped): {error}"
                ),
            )?,
        }
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn secondary_boot_order(
    previous: &BcdBootManagerState,
    secondary: &str,
    live: &str,
) -> Result<Vec<String>, TaskError> {
    let normalized = live.replace("\r\n", "\n");
    let objects: std::collections::BTreeSet<String> = normalized
        .split("\n\n")
        .filter_map(|block| block.lines().find_map(bcd_identifier_from_line))
        .map(str::to_ascii_lowercase)
        .collect();
    if !objects.contains(&previous.default.to_ascii_lowercase())
        || !objects.contains(&secondary.to_ascii_lowercase())
    {
        return Err(errc(ErrCode::LoaderMissingRefuseMenu));
    }
    let mut order: Vec<_> = previous
        .display_order
        .iter()
        .filter(|id| {
            objects.contains(&id.to_ascii_lowercase()) && !id.eq_ignore_ascii_case(secondary)
        })
        .cloned()
        .collect();
    if order.is_empty() {
        order.push(previous.default.clone());
    }
    order.push(secondary.to_owned());
    Ok(order)
}

/// Parse the minimal Boot Manager state required to preserve the existing
/// loader. The BCD field labels may be localized, but the value tokens are
/// invariant braced identifiers.
#[cfg(any(windows, test))]
fn parse_boot_manager_state(output: &str) -> Option<BcdBootManagerState> {
    // `/enum all` includes a firmware boot-manager object that also has a
    // `displayorder`. Scope the parser to the Windows Boot Manager object;
    // otherwise a secondary restore could put firmware entries in the
    // Windows loader menu.
    let output = output.replace("\r\n", "\n");
    let loader_resume_objects = output
        .split("\n\n")
        .filter_map(|block| {
            let lower = block.to_ascii_lowercase();
            if !lower.contains("\\windows\\system32\\winload.efi") {
                return None;
            }
            let loader = block.lines().find_map(bcd_identifier_from_line)?;
            let resume = block.lines().find_map(|line| {
                let trimmed = line.trim();
                trimmed
                    .to_ascii_lowercase()
                    .starts_with("resumeobject")
                    .then(|| bcd_value_from_line(trimmed).map(str::to_owned))
                    .flatten()
            })?;
            Some((loader.to_owned(), resume))
        })
        .collect();
    let boot_manager = output.split("\n\n").find(|block| {
        block
            .lines()
            .find_map(bcd_identifier_from_line)
            .is_some_and(|identifier| {
                identifier.eq_ignore_ascii_case("{9dea862c-5cdd-4e70-acc1-f32b344d4795}")
            })
    })?;
    let mut default = None;
    let mut resume_object = None;
    let mut display_order = Vec::new();
    let mut reading_display_order = false;
    for line in boot_manager.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("default") || trimmed.starts_with("默认") {
            default = bcd_value_from_line(trimmed).map(str::to_owned);
            reading_display_order = false;
            continue;
        }
        if lower.starts_with("resumeobject") {
            resume_object = bcd_value_from_line(trimmed).map(str::to_owned);
            reading_display_order = false;
            continue;
        }
        let key = trimmed.split_whitespace().next().unwrap_or("");
        if key.eq_ignore_ascii_case("displayorder") || key == "显示顺序" {
            reading_display_order = true;
        } else if !trimmed.starts_with('{') {
            // 只有纯 GUID 续行属于启动菜单；toolsdisplayorder 等后续字段必须隔离。
            reading_display_order = false;
        }
        if reading_display_order {
            if let Some(value) = bcd_value_from_line(trimmed) {
                display_order.push(value.to_owned());
            } else if !trimmed.is_empty() {
                reading_display_order = false;
            }
        }
    }
    Some(BcdBootManagerState {
        default: default?,
        display_order,
        resume_object,
        loader_resume_objects,
    })
}

/// Accept GUIDs and BCDEdit's well-known aliases, but nothing that could turn
/// captured BCD text into an argument injection.
#[cfg(any(windows, test))]
fn bcd_value_from_line(line: &str) -> Option<&str> {
    let start = line.find('{')?;
    let end = line[start..].find('}')? + start + 1;
    let value = &line[start..end];
    (value.len() >= 3
        && value[1..value.len() - 1]
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-'))
    .then_some(value)
}

#[cfg(windows)]
fn verify_bcd_target(efi_root: &Path, target_root: &Path, log: &Path) -> Result<(), TaskError> {
    let store = efi_root.join("EFI\\Microsoft\\Boot\\BCD");
    if !store.is_file() {
        return Err(err("BCD store is missing after BCDBoot"));
    }
    let target_letter = target_root
        .to_string_lossy()
        .chars()
        .next()
        .ok_or_else(|| err("restore target has no drive letter for BCD verification"))?
        .to_ascii_lowercase();
    let needle = format!("partition={target_letter}:");
    let store_arg = store.to_string_lossy().into_owned();
    let output = capture_logged(
        "bcdedit.exe",
        &["/store", &store_arg, "/enum", "all", "/v"],
        log,
    )?;
    let matched = output.lines().any(|line| {
        let lower = line.trim().to_ascii_lowercase();
        (lower.starts_with("device")
            || lower.starts_with("osdevice")
            || lower.starts_with("设备")
            || lower.starts_with("os 设备"))
            && lower.contains(&needle)
    });
    if !matched {
        return Err(err(&format!(
            "BCDBoot completed but no Windows loader points to the selected target {target_letter}:"
        )));
    }
    append_log(
        log,
        &format!("Verified BCD device/osdevice points to target {target_letter}:"),
    )?;
    Ok(())
}

#[cfg_attr(not(windows), allow(dead_code))]
fn stream_to_log<R: Read>(
    label: &str,
    stream: R,
    sink: Arc<Mutex<std::fs::File>>,
) -> Result<(), TaskError> {
    let mut reader = BufReader::new(stream);
    let mut line: Vec<u8> = Vec::new();
    let mut saw_cr = false;
    let emit = |line: &[u8], sink: &Arc<Mutex<std::fs::File>>| -> Result<(), TaskError> {
        // DISM/bcdboot 在本地化主机上使用 Windows 控制台代码页（如 CP936/GBK）；
        // stdout 不一定是 UTF-8。使用 decode_windows_bytes 进行代码页感知解码，
        // 彻底避免中文输出被截断损坏成 \u{FFFD}（锟斤拷乱码）。
        let text = crate::text_parsing::decode_windows_bytes(line);
        let mut file = sink
            .lock()
            .map_err(|_| err("recovery log lock was poisoned"))?;
        writeln!(file, "[{label}] {text}")?;
        file.flush()?;
        Ok(())
    };
    loop {
        let mut byte = [0u8; 1];
        if reader.read(&mut byte)? == 0 {
            if !line.is_empty() {
                emit(&line, &sink)?;
            }
            break;
        }
        match byte[0] {
            b'\n' => {
                if saw_cr {
                    // \r\n: the \r already flushed this logical line; ignore the \n.
                    saw_cr = false;
                    line.clear();
                } else {
                    emit(&line, &sink)?;
                    line.clear();
                }
            }
            b'\r' => {
                // DISM 进度条用 \r 原地刷新（不带 \n）。把每次 \r 当作一行边界实时
                // 落盘，进度窗口才能看到中间百分比；否则整段进度会卡在管道缓冲里，
                // 直到 DISM 结束吐出 \n 才一次性落盘，窗口进度条全程冻在 0%。
                emit(&line, &sink)?;
                line.clear();
                saw_cr = true;
            }
            other => {
                line.push(other);
                saw_cr = false;
            }
        }
    }
    Ok(())
}

fn run_command(program: &str, args: Vec<String>) -> Result<(), TaskError> {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(err(&format!("{program} failed with {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bcd_identifier_from_line, claim_boot_resume_attempt, diskpart_format_script,
        parse_boot_manager_state, parse_recover_options, split_workspace_root_rel,
        stage_resumable_after_interruption,
    };

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
        assert_eq!(task.operation, super::Operation::CreateSecondary);
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

    #[test]
    fn dism_carriage_return_progress_streams_each_update() {
        // 回归：DISM 用 \r 原地刷新进度条（不带 \n）。旧实现用 read_until(b'\n')，
        // 导致所有 [= xx% =] 卡在管道缓冲、直到结束才落盘，进度窗口全程冻在 0%。
        // 现在每次 \r 都应作为一行边界实时写入日志。
        use std::io::Cursor;
        use std::sync::{Arc, Mutex};

        let dir = std::env::temp_dir().join("br_stream_test");
        let _ = std::fs::create_dir_all(&dir);
        let log_path = dir.join("recovery.log");
        let _ = std::fs::remove_file(&log_path);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap();
        let sink = Arc::new(Mutex::new(file));
        let data: Vec<u8> = b"[=   10.0% =]\r[=   20.0% =]\r[=  100.0% =]\nDone\n".to_vec();
        super::stream_to_log("stdout", Cursor::new(data), Arc::clone(&sink)).unwrap();
        drop(sink);
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(
            content.contains("10.0%"),
            "missing first percent update: {content}"
        );
        assert!(
            content.contains("20.0%"),
            "missing second percent update: {content}"
        );
        assert!(
            content.contains("100.0%"),
            "missing final percent update: {content}"
        );
        assert!(content.contains("Done"), "missing trailing line: {content}");
        // \r\n 不应产生重复空行。
        assert!(
            !content.contains("[stdout] \n[stdout] "),
            "carriage-return+newline doubled a line: {content}"
        );
        let _ = std::fs::remove_file(&log_path);
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
