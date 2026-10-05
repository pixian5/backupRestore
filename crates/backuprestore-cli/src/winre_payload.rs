//! WinRE shell template and payload contract.
//!
//! The normal WinRE task image and the standalone PE desktop have different
//! entry points. Keep the WinRE side in one module so the package preflight,
//! task staging, WIM injection, and post-injection verification cannot drift.

use backuprestore_core::{TaskError, sha256_file, verify_sha256};
use std::fs;
use std::path::{Path, PathBuf};

const WINRE_SHELL_PAYLOAD_NAME: &str = "winpeshl.ini";
const EXPECTED_WINRE_SHELL: &str = concat!(
    "[LaunchApps]\n",
    "%SYSTEMROOT%\\System32\\Recovery.exe,recover-env %SYSTEMROOT%\\System32\\RecoveryTask.env\n"
);

const WINRE_GENERATED_PAYLOAD_FILES: &[&str] = &["RecoveryTask.env", "task.json"];
const OPTIONAL_RUNTIME_FILES: &[&str] = &["VCRUNTIME140.dll", "VCRUNTIME140_1.dll"];
const OBSOLETE_WINRE_FILES: &[&str] = &[
    "BackupRestore.exe",
    "RecoveryLauncher.cmd",
    "winpeshl-boot.cmd",
];

fn err(message: impl Into<String>) -> TaskError {
    TaskError::Invalid(message.into())
}

fn normalize_text(text: &str) -> String {
    text.trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .trim_end()
        .to_owned()
}

fn required_payload_names() -> impl Iterator<Item = &'static str> {
    [WINRE_SHELL_PAYLOAD_NAME, "Recovery.exe"]
        .into_iter()
        .chain(WINRE_GENERATED_PAYLOAD_FILES.iter().copied())
}

fn validate_winre_shell(path: &Path) -> Result<(), TaskError> {
    let contents = fs::read_to_string(path)?;
    if normalize_text(&contents) != normalize_text(EXPECTED_WINRE_SHELL) {
        return Err(err(format!(
            "WinRE shell must directly launch Recovery.exe recover-env: {}",
            path.display()
        )));
    }
    Ok(())
}

fn copy_required(source: &Path, destination: &Path, role: &str) -> Result<(), TaskError> {
    if !source.is_file() {
        return Err(err(format!(
            "required {role} is missing: {}",
            source.display()
        )));
    }
    fs::copy(source, destination)?;
    let expected = sha256_file(source)?;
    verify_sha256(destination, &expected)?;
    Ok(())
}

/// 判断两个路径是否指向同一个文件（离线侧 `executable_dir` 就是 System32，
/// 此时 current_exe 与同目录的 `Recovery.exe` 本来就是同一个文件）。
fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(a), Ok(b)) => a == b,
        _ => left == right,
    }
}

/// 决定载荷里 `Recovery.exe` 的来源文件，并在发现旧件时返回一条告警文案。
///
/// 载荷里的 `Recovery.exe` 与产品主程序按契约**就是同一份二进制的两个名字**
/// （`build-win.sh` 把同一个 BackupRestore.exe 复制成两份）。
///
/// 实机踩过的坑（2026-09-30 任务 247a7169）：部署只更新了包目录，运行目录
/// `H:` 下的 `Recovery.exe` 还是好几个版本前的旧件（c6e11439…），于是
/// 「改了 BackupRestore.exe 不等于改了 PE 里跑的 Recovery.exe」——离线侧
/// 实际执行的是旧代码，镜像元数据里 `programVersion` 甚至写着 1.8.7，
/// 而任务清单的 `recoverySha256` 也把旧哈希记成了"正确值"，静态检查看不出问题。
///
/// 因此这里改为**优先用正在运行的可执行文件**当载荷来源：同哈希时沿用同目录
/// 的 `Recovery.exe`（行为不变），一旦不同就用自己并把两个哈希写进日志，
/// 让旧件永远不可能被烘焙进 WIM。
pub fn resolve_recovery_payload_source(
    executable_dir: &Path,
    current_exe: Option<&Path>,
) -> Result<(PathBuf, Option<String>), TaskError> {
    let sibling = executable_dir.join("Recovery.exe");
    let Some(current) = current_exe.filter(|path| path.is_file()) else {
        return Ok((sibling, None));
    };
    if same_file(current, &sibling) {
        return Ok((sibling, None));
    }
    if !sibling.is_file() {
        let hash = sha256_file(current)?;
        return Ok((
            current.to_path_buf(),
            Some(format!(
                "payload Recovery.exe is missing at {}; using the running executable {} (sha256={hash})",
                sibling.display(),
                current.display()
            )),
        ));
    }
    let current_hash = sha256_file(current)?;
    let sibling_hash = sha256_file(&sibling)?;
    if current_hash == sibling_hash {
        return Ok((sibling, None));
    }
    Ok((
        current.to_path_buf(),
        Some(format!(
            "payload Recovery.exe is stale: {} sha256={sibling_hash} != running executable {} sha256={current_hash}; staging the running executable instead",
            sibling.display(),
            current.display()
        )),
    ))
}

/// Validate and stage all static files from the package into a task payload.
///
/// 返回值是可选告警：非 `None` 表示同目录的 `Recovery.exe` 是旧件或缺失，
/// 已改用正在运行的可执行文件，调用方必须把这条写进准备日志。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn stage_static_payload(
    executable_dir: &Path,
    payload: &Path,
) -> Result<Option<String>, TaskError> {
    let current = std::env::current_exe().ok();
    stage_static_payload_with_exe(executable_dir, payload, current.as_deref())
}

/// `stage_static_payload` 的可注入版本：显式传入"正在运行的可执行文件"，
/// 供单测在任意平台构造旧件/缺失两种场景。
pub fn stage_static_payload_with_exe(
    executable_dir: &Path,
    payload: &Path,
    current_exe: Option<&Path>,
) -> Result<Option<String>, TaskError> {
    // Keep the boot contract inside the Rust binary. A deployment must not
    // fail because an optional source-tree template was omitted from a VM.
    fs::write(payload.join(WINRE_SHELL_PAYLOAD_NAME), EXPECTED_WINRE_SHELL)?;
    validate_winre_shell(&payload.join(WINRE_SHELL_PAYLOAD_NAME))?;
    let (recovery_source, warning) = resolve_recovery_payload_source(executable_dir, current_exe)?;
    copy_required(
        &recovery_source,
        &payload.join("Recovery.exe"),
        "WinRE package payload",
    )?;
    for name in OPTIONAL_RUNTIME_FILES {
        let source = executable_dir.join(name);
        if source.is_file() {
            copy_required(&source, &payload.join(name), "WinRE runtime payload")?;
        }
    }
    Ok(warning)
}

fn verify_payload_contract(payload: &Path) -> Result<(), TaskError> {
    let shell = payload.join(WINRE_SHELL_PAYLOAD_NAME);
    validate_winre_shell(&shell)?;
    for name in required_payload_names() {
        let path = payload.join(name);
        if !path.is_file() {
            return Err(err(format!(
                "required WinRE task payload is missing: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

/// Copy the complete task payload into an already mounted WinRE WIM and verify
/// every required file byte-for-byte before DISM commits the image.
pub fn inject_winre_payload(mount: &Path, payload: &Path) -> Result<(), TaskError> {
    verify_payload_contract(payload)?;
    let system32 = mount.join("Windows").join("System32");
    if !system32.is_dir() {
        return Err(err("mounted WinRE has no Windows\\System32"));
    }

    for name in required_payload_names() {
        copy_required(
            &payload.join(name),
            &system32.join(name),
            "WinRE WIM payload",
        )?;
    }
    for name in OPTIONAL_RUNTIME_FILES {
        let source = payload.join(name);
        if source.is_file() {
            copy_required(&source, &system32.join(name), "WinRE WIM runtime")?;
        }
    }
    for name in OBSOLETE_WINRE_FILES {
        let stale = system32.join(name);
        if stale.is_file() {
            fs::remove_file(stale)?;
        }
    }

    verify_payload_contract(&system32)?;
    for name in OPTIONAL_RUNTIME_FILES {
        let source = payload.join(name);
        if source.is_file() {
            let expected = sha256_file(&source)?;
            verify_sha256(system32.join(name), &expected)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        EXPECTED_WINRE_SHELL, inject_winre_payload, resolve_recovery_payload_source,
        stage_static_payload_with_exe,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "backuprestore-winre-payload-{label}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn stages_only_the_direct_rust_winre_entry() {
        let root = temp_dir("stage");
        let package = root.join("package");
        let payload = root.join("payload");
        fs::create_dir_all(&package).unwrap();
        fs::create_dir_all(&payload).unwrap();
        fs::write(package.join("Recovery.exe"), b"recovery").unwrap();

        stage_static_payload_with_exe(&package, &payload, None).unwrap();

        assert_eq!(
            fs::read_to_string(payload.join("winpeshl.ini")).unwrap(),
            EXPECTED_WINRE_SHELL
        );
        assert!(payload.join("Recovery.exe").is_file());
        assert!(!payload.join("BackupRestore.exe").exists());
        assert!(!payload.join("RecoveryLauncher.cmd").exists());
        fs::remove_dir_all(root).unwrap();
    }

    // 旧件场景回归锁：同目录 Recovery.exe 与正在运行的可执行文件哈希不同时，
    // 必须改用正在运行的那一份，并给出带两个哈希的告警。
    #[test]
    fn stale_sibling_recovery_falls_back_to_the_running_executable() {
        let root = temp_dir("stale");
        let package = root.join("package");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("Recovery.exe"), b"old-build").unwrap();
        let running = root.join("BackupRestore.exe");
        fs::write(&running, b"new-build").unwrap();

        let (source, warning) =
            resolve_recovery_payload_source(&package, Some(running.as_path())).unwrap();

        assert_eq!(source, running);
        let warning = warning.expect("stale payload must warn");
        assert!(warning.contains("stale"), "{warning}");
        assert!(warning.contains("Recovery.exe"), "{warning}");
        fs::remove_dir_all(root).unwrap();
    }

    // 同哈希时行为不变：仍沿用同目录的 Recovery.exe，且不告警。
    #[test]
    fn matching_sibling_recovery_is_kept_without_warning() {
        let root = temp_dir("match");
        let package = root.join("package");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("Recovery.exe"), b"same-build").unwrap();
        let running = root.join("BackupRestore.exe");
        fs::write(&running, b"same-build").unwrap();

        let (source, warning) =
            resolve_recovery_payload_source(&package, Some(running.as_path())).unwrap();

        assert_eq!(source, package.join("Recovery.exe"));
        assert!(warning.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    // 同目录没有 Recovery.exe 时也能顶上，避免仅因部署漏拷一个名字就无法准备任务。
    #[test]
    fn missing_sibling_recovery_uses_the_running_executable() {
        let root = temp_dir("absent");
        let package = root.join("package");
        fs::create_dir_all(&package).unwrap();
        let running = root.join("BackupRestore.exe");
        fs::write(&running, b"only-build").unwrap();

        let (source, warning) =
            resolve_recovery_payload_source(&package, Some(running.as_path())).unwrap();

        assert_eq!(source, running);
        assert!(
            warning
                .expect("missing payload must warn")
                .contains("missing")
        );
        fs::remove_dir_all(root).unwrap();
    }

    // 取不到 current_exe 时退回旧行为，不改变离线侧（executable_dir 即 System32）语义。
    #[test]
    fn without_a_running_executable_the_sibling_is_used() {
        let root = temp_dir("noexe");
        let package = root.join("package");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("Recovery.exe"), b"recovery").unwrap();

        let (source, warning) = resolve_recovery_payload_source(&package, None).unwrap();

        assert_eq!(source, package.join("Recovery.exe"));
        assert!(warning.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_template_is_the_direct_rust_contract() {
        let repository_template = include_str!("../../../windows/winre-winpeshl.ini");
        assert_eq!(
            super::normalize_text(repository_template),
            super::normalize_text(EXPECTED_WINRE_SHELL)
        );
    }

    #[test]
    fn ignores_external_shell_files_and_generates_the_winre_contract() {
        let root = temp_dir("bad-template");
        let package = root.join("package");
        let payload = root.join("payload");
        fs::create_dir_all(&package).unwrap();
        fs::create_dir_all(&payload).unwrap();
        fs::write(
            package.join("winre-winpeshl.ini"),
            "[LaunchApps]\n%SYSTEMROOT%\\System32\\Recovery.exe,--pe-desktop\n",
        )
        .unwrap();
        fs::write(package.join("Recovery.exe"), b"recovery").unwrap();
        stage_static_payload_with_exe(&package, &payload, None).unwrap();
        assert_eq!(
            fs::read_to_string(payload.join("winpeshl.ini")).unwrap(),
            EXPECTED_WINRE_SHELL
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn injection_copies_and_verifies_the_complete_contract() {
        let root = temp_dir("inject");
        let package = root.join("package");
        let payload = root.join("payload");
        let mount = root.join("mount");
        fs::create_dir_all(&package).unwrap();
        fs::create_dir_all(&payload).unwrap();
        fs::create_dir_all(mount.join("Windows").join("System32")).unwrap();
        fs::write(package.join("Recovery.exe"), b"recovery").unwrap();
        stage_static_payload_with_exe(&package, &payload, None).unwrap();
        fs::write(payload.join("RecoveryTask.env"), b"TASK_ID=test").unwrap();
        fs::write(payload.join("task.json"), b"{}").unwrap();
        let system32 = mount.join("Windows").join("System32");
        fs::write(system32.join("RecoveryLauncher.cmd"), b"obsolete").unwrap();
        fs::write(system32.join("winpeshl-boot.cmd"), b"obsolete").unwrap();

        inject_winre_payload(&mount, &payload).unwrap();

        for name in [
            "winpeshl.ini",
            "Recovery.exe",
            "RecoveryTask.env",
            "task.json",
        ] {
            assert_eq!(
                fs::read(payload.join(name)).unwrap(),
                fs::read(system32.join(name)).unwrap()
            );
        }
        assert!(!system32.join("RecoveryLauncher.cmd").exists());
        assert!(!system32.join("winpeshl-boot.cmd").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn injection_rejects_an_incomplete_task_payload() {
        let root = temp_dir("missing");
        let payload = root.join("payload");
        let mount = root.join("mount");
        fs::create_dir_all(&payload).unwrap();
        fs::create_dir_all(mount.join("Windows").join("System32")).unwrap();
        fs::write(payload.join("winpeshl.ini"), EXPECTED_WINRE_SHELL).unwrap();
        fs::write(payload.join("Recovery.exe"), b"recovery").unwrap();
        fs::write(payload.join("task.json"), b"{}").unwrap();

        let error = inject_winre_payload(&mount, &payload).unwrap_err();
        assert!(error.to_string().contains("RecoveryTask.env"));
        fs::remove_dir_all(root).unwrap();
    }
}
