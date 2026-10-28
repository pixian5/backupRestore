//! wimlib 覆盖式应用通道（任务 #110；2026-10-22 用户裁定「换 wimlib」，
//! 2026-10-23 用户裁定产品化「保留安全描述符还原」，去掉 `--no-acls`）。
//!
//! **为什么存在**：DISM `/Apply-Image` 做「覆盖式应用」（取消格式化，不删卷上
//! 既有文件）打到已含 Windows 的目标卷时，要还原镜像里
//! `Program Files\WindowsApps\...`（MSIX 应用包目录，owner =
//! `NT SERVICE\TrustedInstaller`、带 MSIX 专用 ACL）的**目录 SD / owner**；
//! 系统拒绝对这类目录改写元数据 → `RestoreDirMetadata 0x80070005` → DISM
//! 立即整体放弃（exit 5，事务性，不留半成品）。格式化能过是因为受保护目录被
//! 清空，DISM 只需「新建」而非「改写」。
//!
//! **wimlib 的两个决定性性质**（POC 实证，见
//! `docs/20261022-1427-wimlib-no-acls覆盖式还原概念验证与DISM对照-Step-5-Preview.md`
//! 与 `docs/20261022-2308-wimlib保留访问控制列表覆盖式还原概念验证-Step-5-Preview.md`）：
//! 1. wimlib 自行获取 SeRestore/SeBackup 提权（E5：禁用提权的父进程下
//!    wimlib 仍 exit 0，同父进程的 icacls 子进程被拒 exit 5）→ **RE/PE 上下文
//!    同样可用**，而这正是 DISM 会败的环境；
//! 2. 该提权**同时覆盖文件写入与安全描述符还原**（2026-10-23 概念验证：
//!    特权禁用上下文 + TrustedInstaller 属主保护目录，wimlib 把 owner/DACL/
//!    protected 完整还原为镜像 SD，SDDL 逐字节一致；全量 71GiB / 634,091
//!    条目 exit 0 零警告，判据样本目录/文件 SD 与镜像参考逐字节一致，
//!    含 `WIN://SYSAPPID` 条件 ACE + app package SID + TrustedInstaller FA
//!    的原生 MSIX ACL）。因此镜像里的目录权限（ACL/owner）**完整还原**，
//!    个别受保护目录可能保留卷上现有权限。
//!
//! **历史**：v2.6.25–v2.6.26 曾带 `--no-acls`（跳过 SD 还原，对
//! RestoreDirMetadata 失败结构性免疫）作为双保险；2026-10-23 概念验证证明
//! 提权覆盖 SD 还原后，用户裁定产品化「保留访问控制列表」，参数去掉。
//!
//! **定位规则**：在线态 = 程序同目录（`executable_dir`，随包部署）；RE 态 =
//! `winre_payload` 注入到 WinRE 的 `Windows\System32`（RE 内 `executable_dir`
//! 即 System32，`current_exe` 也在那里）。`libwim-15.dll` 必须与 exe 同目录。
//!
//! **许可**：wimlib GPLv3+ / libwim LGPLv2.1+（Windows 构建可选 LGPL）。以
//! **独立进程**调用官方预编译二进制、不链接库，属合法聚合，不传染产品代码。
//!
//! **缺失时明确报错，绝不静默回退 DISM**——否则退化成旧的硬拒/失败行为，
//! 等于没做。
//!
//! **另一个方向也有兜底（D1，2026-10-24）**：走了 DISM 却失败的场景
//! （`fallback_after_dism_failure`）会按判据自动降级到本通道重试一次。
//! 两个方向合起来才是完整的双保险：路由期「该用 wimlib 就用 wimlib」，
//! 执行期「DISM 倒了 wimlib 接得上」。设计见
//! `docs/20261024-1636-DISM失败自动降级wimlib与错误分类详细设计-Step-5-Preview.md`。

use crate::error_code::ErrCode;
use backuprestore_core::TaskError;
use std::path::{Path, PathBuf};

/// wimlib 官方 ARM64 二进制的两个文件名（与 `winre_payload::OPTIONAL_RUNTIME_FILES`
/// 和 `build-win.sh --deploy` 的拷贝清单保持一致）。
pub(crate) const WIMLIB_EXE: &str = "wimlib-imagex.exe";
pub(crate) const WIMLIB_DLL: &str = "libwim-15.dll";

/// 构造 `wimlib-imagex apply` 的参数（纯函数，单测锁死形态与顺序）。
///
/// 参数顺序即官方 CLI 契约：`apply <wim> <image_num_or_name> <dest_dir>`。
/// 不带 `--no-acls`：安全描述符（ACL/owner）随镜像完整还原——wimlib 自行
/// 获取 SeRestore/SeBackup 提权，还原受保护目录的 SD 不被拒（2026-10-23
/// 概念验证，tiny + 全量两级）。
/// 引号转义由 `windows_command::run_program` 的 `quote` 统一处理
/// （已正确处理尾部反斜杠，见其单测 `quoted_arguments_keep_trailing_slash`）。
pub(crate) fn wimlib_args(image: &Path, index: u32, apply_root: &Path) -> Vec<String> {
    vec![
        "apply".to_string(),
        image.display().to_string(),
        index.to_string(),
        apply_root.display().to_string(),
    ]
}

/// 在 `executable_dir` 定位 `wimlib-imagex.exe`；缺失时给出**可操作**的明确报错
/// （指出需要哪两个文件、应该放哪里），不静默回退 DISM。
///
/// **两个文件都校验**：`wimlib-imagex.exe` 与 `libwim-15.dll` 是**两个独立部署件**
/// （恢复环境载荷注入与部署脚本各自分开拷贝），缺任一个 wimlib 都启动不了。
/// 只查主程序而不查依赖库是此前的缺口——动态库缺失时 Windows 加载器直接让进程
/// 退出，报出来的错误既不点名缺哪个文件、也不落在我们的日志里，现场无法归因。
/// 分开报错就是为了「漏拷了哪一个」一眼可辨。
pub(crate) fn locate_wimlib(executable_dir: &Path) -> Result<PathBuf, TaskError> {
    let exe = executable_dir.join(WIMLIB_EXE);
    if !exe.is_file() {
        return Err(crate::errc_with(
            ErrCode::WimlibBinaryMissing,
            &[&exe.display().to_string(), WIMLIB_DLL],
        ));
    }
    let dll = executable_dir.join(WIMLIB_DLL);
    if !dll.is_file() {
        return Err(crate::errc_with(
            ErrCode::WimlibDllMissing,
            &[&exe.display().to_string(), &dll.display().to_string()],
        ));
    }
    Ok(exe)
}

/// 经统一进程通道执行 `wimlib-imagex apply <wim> <index> <dir>`（还原安全描述符）。
///
/// - 执行通道复用 `windows_command::run_program`（引号转义、Job 对象进程树
///   隔离、超时、日志落盘），与 DISM 同等待遇；
/// - 超时由调用方显式传入 `wait_ms`：常规路径传 `dism_timeout::wait_ms()`
///   （wimlib apply 71GiB 实测约 8–22 分钟，与 DISM 同属「长任务」，共用同一个
///   可配置上限，默认仍是不限制）；**降级路径传 `windows_command::INFINITE`**
///   ——用户设短上限的意图是「杀死挂死的 DISM」，不是「这个任务只许跑 N 秒」，
///   继承被玩坏的上限会把降级后的 wimlib 同样秒杀，降级失去意义
///   （见 `fallback_after_dism_failure` 的文档）；
/// - `executable_dir` 传调用方解析好的程序目录（在线态=程序同目录；
///   RE 态=current_exe 所在目录，即注入后的 System32）。
#[cfg(windows)]
pub(crate) fn apply_overlay(
    executable_dir: &Path,
    image: &Path,
    index: u32,
    apply_root: &Path,
    log: &Path,
    wait_ms: u32,
) -> Result<(), TaskError> {
    use crate::windows_command;
    let exe = locate_wimlib(executable_dir)?;
    let args = wimlib_args(image, index, apply_root);
    crate::append_log(
        log,
        &crate::i18n::msg_tpl(
            "wimlib_start",
            &[&exe.display().to_string(), &args.join(" ")],
        ),
    )?;
    let outcome = windows_command::run_program(&exe.to_string_lossy(), &args, Some(log), wait_ms);
    crate::append_log(log, &crate::i18n::msg_tpl("wimlib_result", &[&outcome.to_string()]))?;
    if !outcome.is_success() {
        // 失败必须带输出尾部（与在线链 `dism()` 同一契约）：实机第三层双失败
        // 时 wimlib 的真实报错（`[ERROR] Can't open "…" for deletion …
        // (status=c0000043)`）只落在日志里，Err 只带退出码会让
        // `failure_policy::classify_apply_failure` 拿到空文本 → 归 Unknown，
        // 双失败的第二条因果丢失归因能力。
        let tail = crate::text_parsing::tail_text(log, 8192);
        return Err(crate::errc_with(
            ErrCode::WimlibApplyFailed,
            &[&format!("{outcome}"), &tail],
        ));
    }
    Ok(())
}

/// DISM apply 失败后的**自动降级编排**（D1，2026-10-24）。
///
/// 设计：`docs/20261024-1636-DISM失败自动降级wimlib与错误分类详细设计-Step-5-Preview.md`。
/// 判据与文案的单一来源在 `failure_policy`（纯函数 + macOS 单测），本函数只做
/// 编排：分类 → 判据 → 记日志 → 以**不限制**超时重跑 wimlib → 结果合成。
/// 四个 apply 接线点（RE 主链 / PE / 在线数据卷分支 / 在线第二系统分支）
/// 共用本函数——各写一遍就是 failure_policy 模块注释里写过的「文案分叉」前科。
///
/// **为什么降级路径传不限制超时**：`dism_timeout::wait_ms()` 是 DISM 与 wimlib
/// 共用的可配上限，用户设短上限的意图是「杀死挂死的 DISM」而不是「这个任务
/// 只许跑 N 秒」。若降级后还继承同一个被玩坏的上限，wimlib 会被同样秒杀，
/// 降级失去意义（wimlib apply 71GiB 实测 8–22 分钟，与 DISM 同属长任务）。
///
/// 错误合成策略：无论是否降级，失败回执都带**中文因果 + 原始错误全文**；
/// 双失败时两级因果都带上（DISM 的 + wimlib 的）——两条因果比一条更有归因价值。
#[cfg(windows)]
pub(crate) fn fallback_after_dism_failure(
    dism_error: &TaskError,
    log: &Path,
    image: &Path,
    index: u32,
    apply_root: &Path,
) -> Result<(), TaskError> {
    let dism_text = dism_error.to_string();
    let kind = crate::failure_policy::classify_apply_failure(&dism_text);
    let cause = kind.cause();
    // 无论是否降级，分类行都落日志——失败回执的可归因性本身就是收益。
    crate::append_log(
        log,
        &crate::i18n::msg_tpl("apply_classified", &[&format!("{kind:?}"), cause]),
    )?;
    let executable_dir = std::env::current_exe()
        .map_err(|error| crate::errc_with(ErrCode::ProgramDirUnresolvableFallback, &[&error.to_string()]))?
        .parent()
        .ok_or_else(|| crate::errc(ErrCode::ProgramDirResolveFailedFallback))?
        .to_path_buf();
    let located = locate_wimlib(&executable_dir);
    if !crate::failure_policy::decide_fallback(kind, located.is_ok()) {
        // 两种不降级：ImageCorrupt（换引擎必然同样失败）/ wimlib 缺件（无法降级）。
        let reason = match &located {
            Err(missing) => crate::i18n::msg_tpl("fallback_skip_missing", &[&missing.to_string()]),
            Ok(_) => crate::i18n::msg("fallback_skip_corrupt").to_string(),
        };
        crate::append_log(log, &format!("[WARN] {reason}"))?;
        return Err(crate::errc_with(
            ErrCode::DismApplyFailedWithReason,
            &[cause, &reason, &dism_text],
        ));
    }
    crate::append_log(log, &crate::i18n::msg_tpl("fallback_start", &[cause]))?;
    match apply_overlay(
        &executable_dir,
        image,
        index,
        apply_root,
        log,
        crate::windows_command::INFINITE,
    ) {
        Ok(()) => {
            crate::append_log(log, crate::i18n::msg("fallback_ok"))?;
            Ok(())
        }
        Err(wimlib_error) => {
            let wimlib_kind =
                crate::failure_policy::classify_apply_failure(&wimlib_error.to_string());
            Err(crate::errc_with(
                ErrCode::FallbackAlsoFailed,
                &[
                    cause,
                    wimlib_kind.cause(),
                    &dism_text,
                    &wimlib_error.to_string(),
                ],
            ))
        }
    }
}

/// 从一份 PE 镜像里取出「判定启动配置是否合格」所需的两个事实。
///
/// 返回 `(shell 原文, Recovery.exe 是否在镜像里)`；`shell` 为 `None` 表示镜像里
/// 没有 `Windows/System32/winpeshl.ini`。
///
/// 为什么用 wimlib 而不是 DISM：只需要读**一个几百字节的文件**，DISM 得先挂载
/// 整个镜像（400 MB 起、要临时目录、要清理），代价与收益完全不成比例。
///
/// ★ 两个实测坑写死在实现里：
/// 1. `wimlib-imagex extract` 取**单个文件**时会把它**拍平**到 `--dest-dir` 根下，
///    不保留中间目录（2026-10-29 实测：文件落在 `<dest>\winpeshl.ini`，而不是
///    `<dest>\Windows\System32\winpeshl.ini`）。因此两处路径都要找。
/// 2. 文件不存在时 `extract` 的退出码是 **49**（"The path does not exist in the
///    WIM image"），而不是 0/1 之类——所以先用 `dir` 判断存在性，再决定要不要
///    提取，避免把「镜像损坏」误报成「缺这个文件」。
#[cfg(windows)]
pub(crate) fn probe_pe_image_shell(
    executable_dir: &Path,
    image: &Path,
    work_dir: &Path,
    log: &Path,
) -> Result<(Option<String>, bool), TaskError> {
    /// 探测是**装前自检**，不是任务主链：卡住就应当放弃探测，而不是跟着卡。
    const PROBE_TIMEOUT_MS: u32 = 120_000;
    const SYSTEM32: &str = "/Windows/System32";

    let exe = locate_wimlib(executable_dir)?;
    std::fs::create_dir_all(work_dir)?;

    let listing = work_dir.join("pe-image-listing.txt");
    let listing_args = vec![
        "dir".to_string(),
        image.display().to_string(),
        "1".to_string(),
        format!("--path={SYSTEM32}"),
    ];
    let outcome = crate::windows_command::run_program(
        &exe.to_string_lossy(),
        &listing_args,
        Some(&listing),
        PROBE_TIMEOUT_MS,
    );
    crate::append_log(
        log,
        &format!(
            "PE shell probe: wimlib dir exit={outcome} image={}",
            image.display()
        ),
    )?;
    if !outcome.is_success() {
        let tail = crate::text_parsing::tail_text(&listing, 2048);
        return Err(crate::err(&format!(
            "wimlib dir failed while reading the PE image: {outcome}; {tail}"
        )));
    }
    let entries = std::fs::read(&listing)
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .unwrap_or_default();
    let has = |name: &str| {
        entries.lines().any(|line| {
            let normalized = line.trim().replace('\\', "/").to_ascii_lowercase();
            normalized.ends_with(&format!("/{}", name.to_ascii_lowercase()))
        })
    };
    let shell_present = has("winpeshl.ini");
    let recovery_present = has("Recovery.exe");
    if !shell_present {
        let _ = std::fs::remove_file(&listing);
        return Ok((None, recovery_present));
    }

    let extract_args = vec![
        "extract".to_string(),
        image.display().to_string(),
        "1".to_string(),
        format!("{SYSTEM32}/winpeshl.ini"),
        format!("--dest-dir={}", work_dir.display()),
    ];
    let outcome = crate::windows_command::run_program(
        &exe.to_string_lossy(),
        &extract_args,
        Some(&listing),
        PROBE_TIMEOUT_MS,
    );
    crate::append_log(
        log,
        &format!("PE shell probe: wimlib extract exit={outcome}"),
    )?;
    if !outcome.is_success() {
        let tail = crate::text_parsing::tail_text(&listing, 2048);
        return Err(crate::err(&format!(
            "wimlib extract failed while reading winpeshl.ini: {outcome}; {tail}"
        )));
    }
    // 先找拍平后的路径，再找保留目录层级的路径（见上面的坑 1）。
    let candidates = [
        work_dir.join("winpeshl.ini"),
        work_dir.join("Windows").join("System32").join("winpeshl.ini"),
    ];
    let shell = candidates
        .iter()
        .find(|path| path.is_file())
        .map(|path| {
            std::fs::read(path)
                .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
                .unwrap_or_default()
        });
    Ok((shell, recovery_present))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wimlib_args_have_exact_shape() {
        // 锁死参数形态与顺序：apply <wim> <index> <dir>（不带 --no-acls，
        // 安全描述符随镜像还原——2026-10-23 用户裁定产品化保留访问控制列表）。
        // 尾部反斜杠的盘符路径保持原样——quote 会在 run_program 里正确转义。
        let args = wimlib_args(Path::new("H:\\BRImages\\br-38sys.wim"), 3, Path::new("F:\\"));
        assert_eq!(
            args,
            vec![
                "apply".to_string(),
                "H:\\BRImages\\br-38sys.wim".to_string(),
                "3".to_string(),
                "F:\\".to_string(),
            ]
        );
    }

    #[test]
    fn locate_wimlib_reports_missing_binary_with_actionable_message() {
        let dir = std::env::temp_dir().join(format!("br-wimlib-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let error = locate_wimlib(&dir).unwrap_err().to_string();
        // 报错必须点名两个文件与「不回退 DISM」的语义，让部署缺口可归因。
        assert!(error.contains(WIMLIB_EXE), "{error}");
        assert!(error.contains(WIMLIB_DLL), "{error}");
        assert!(error.contains("不会回退 DISM"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn locate_wimlib_accepts_present_binary() {
        let dir = std::env::temp_dir().join(format!("br-wimlib-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 主程序与依赖库必须同时就位——部署清单是分开拷这两个文件的。
        std::fs::write(dir.join(WIMLIB_EXE), b"stub").unwrap();
        std::fs::write(dir.join(WIMLIB_DLL), b"stub").unwrap();
        assert_eq!(locate_wimlib(&dir).unwrap(), dir.join(WIMLIB_EXE));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn locate_wimlib_rejects_exe_without_companion_dll() {
        let dir = std::env::temp_dir().join(format!("br-wimlib-nodll-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 只放主程序、不放依赖库——这是部署漏拷一个文件时最常见的形态。
        // 不校验的话 wimlib 启动即被系统终止，报错里既不会点名缺哪个文件、
        // 也不会落进我们的日志，现场只能看到「wimlib apply failed」而无法归因。
        std::fs::write(dir.join(WIMLIB_EXE), b"stub").unwrap();
        let error = locate_wimlib(&dir).unwrap_err().to_string();
        assert!(error.contains(WIMLIB_DLL), "{error}");
        assert!(error.contains("不会回退 DISM"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
