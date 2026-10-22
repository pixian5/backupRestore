//! wimlib `--no-acls` 覆盖式应用通道（2026-10-22 用户裁定「换 wimlib
//! --no-acls」，任务 #110）。
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
//! `docs/20261022-1427-wimlib-no-acls覆盖式还原POC与DISM对照-Step-5-Preview.md`）：
//! 1. `--no-acls` 整个跳过 SD 还原 → 对 RestoreDirMetadata 失败**结构性免疫**
//!    （该失败模式在代码路径上不可能发生）；
//! 2. wimlib 自行获取 SeRestore/SeBackup 提权（E5：禁用提权的父进程下
//!    wimlib 仍 exit 0，同父进程的 icacls 子进程被拒 exit 5）→ **RE/PE 上下文
//!    同样可用**，而这正是 DISM 会败的环境。
//!
//! **代价**（GUI 确认框明示）：镜像里的目录权限（ACL/owner）不还原，MSIX
//! 应用的权限可能异常。
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

use backuprestore_core::TaskError;
use std::path::{Path, PathBuf};

/// wimlib 官方 ARM64 二进制的两个文件名（与 `winre_payload::OPTIONAL_RUNTIME_FILES`
/// 和 `build-win.sh --deploy` 的拷贝清单保持一致）。
pub(crate) const WIMLIB_EXE: &str = "wimlib-imagex.exe";
pub(crate) const WIMLIB_DLL: &str = "libwim-15.dll";

/// 构造 `wimlib-imagex apply` 的参数（纯函数，单测锁死形态与顺序）。
///
/// 参数顺序即官方 CLI 契约：`apply <wim> <image_num_or_name> <dest_dir>`，
/// `--no-acls` 是跳过安全描述符还原的开关（等价于 wimgapi 的
/// `WIM_FLAG_NO_DIRACL | WIM_FLAG_NO_FILEACL`，DISM.exe 未暴露）。
/// 引号转义由 `windows_command::run_program` 的 `quote` 统一处理
/// （已正确处理尾部反斜杠，见其单测 `quoted_arguments_keep_trailing_slash`）。
pub(crate) fn wimlib_args(image: &Path, index: u32, apply_root: &Path) -> Vec<String> {
    vec![
        "apply".to_string(),
        image.display().to_string(),
        index.to_string(),
        apply_root.display().to_string(),
        "--no-acls".to_string(),
    ]
}

/// 在 `executable_dir` 定位 `wimlib-imagex.exe`；缺失时给出**可操作**的明确报错
/// （指出需要哪两个文件、应该放哪里），不静默回退 DISM。
pub(crate) fn locate_wimlib(executable_dir: &Path) -> Result<PathBuf, TaskError> {
    let exe = executable_dir.join(WIMLIB_EXE);
    if !exe.is_file() {
        return Err(crate::err(&format!(
            "覆盖式应用（--no-acls）需要 wimlib 二进制：未找到 {}（{WIMLIB_DLL} 须与其同目录）。\
             请把 wimlib 官方 Windows ARM64 预编译包的两个文件放进程序目录后重试；\
             不会回退 DISM（该组合下 DISM 必然失败）。",
            exe.display()
        )));
    }
    Ok(exe)
}

/// 经统一进程通道执行 `wimlib-imagex apply <wim> <index> <dir> --no-acls`。
///
/// - 执行通道复用 `windows_command::run_program`（引号转义、Job 对象进程树
///   隔离、超时、日志落盘），与 DISM 同等待遇；
/// - 超时用 `dism_timeout::wait_ms()`：wimlib apply 71GiB 实测约 22 分钟，
///   与 DISM 同属「长任务」，共用同一个可配置上限（默认仍是不限制）；
/// - `executable_dir` 传调用方解析好的程序目录（在线态=程序同目录；
///   RE 态=current_exe 所在目录，即注入后的 System32）。
#[cfg(windows)]
pub(crate) fn apply_no_acl(
    executable_dir: &Path,
    image: &Path,
    index: u32,
    apply_root: &Path,
    log: &Path,
) -> Result<(), TaskError> {
    use crate::windows_command;
    let exe = locate_wimlib(executable_dir)?;
    let args = wimlib_args(image, index, apply_root);
    crate::append_log(
        log,
        &format!(
            "wimlib 开始（--no-acls 覆盖式应用）：{} {}",
            exe.display(),
            args.join(" ")
        ),
    )?;
    let outcome = windows_command::run_program(
        &exe.to_string_lossy(),
        &args,
        Some(log),
        crate::dism_timeout::wait_ms(),
    );
    crate::append_log(log, &format!("wimlib 结果：{outcome}"))?;
    if !outcome.is_success() {
        return Err(crate::err(&format!("wimlib apply failed: {outcome}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wimlib_args_have_exact_shape() {
        // 锁死参数形态与顺序：apply <wim> <index> <dir> --no-acls。
        // 尾部反斜杠的盘符路径保持原样——quote 会在 run_program 里正确转义。
        let args = wimlib_args(Path::new("H:\\BRImages\\br-38sys.wim"), 3, Path::new("F:\\"));
        assert_eq!(
            args,
            vec![
                "apply".to_string(),
                "H:\\BRImages\\br-38sys.wim".to_string(),
                "3".to_string(),
                "F:\\".to_string(),
                "--no-acls".to_string(),
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
        std::fs::write(dir.join(WIMLIB_EXE), b"stub").unwrap();
        assert_eq!(locate_wimlib(&dir).unwrap(), dir.join(WIMLIB_EXE));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
