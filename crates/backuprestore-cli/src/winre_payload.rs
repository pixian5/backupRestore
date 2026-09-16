//! WinRE shell template and payload contract.
//!
//! The normal WinRE task image and the standalone PE desktop have different
//! entry points. Keep the WinRE side in one module so the package preflight,
//! task staging, WIM injection, and post-injection verification cannot drift.

use backuprestore_core::{TaskError, sha256_file, verify_sha256};
use std::fs;
use std::path::Path;

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

/// Validate and stage all static files from the package into a task payload.
pub fn stage_static_payload(executable_dir: &Path, payload: &Path) -> Result<(), TaskError> {
    // Keep the boot contract inside the Rust binary. A deployment must not
    // fail because an optional source-tree template was omitted from a VM.
    fs::write(payload.join(WINRE_SHELL_PAYLOAD_NAME), EXPECTED_WINRE_SHELL)?;
    validate_winre_shell(&payload.join(WINRE_SHELL_PAYLOAD_NAME))?;
    copy_required(
        &executable_dir.join("Recovery.exe"),
        &payload.join("Recovery.exe"),
        "WinRE package payload",
    )?;
    for name in OPTIONAL_RUNTIME_FILES {
        let source = executable_dir.join(name);
        if source.is_file() {
            copy_required(&source, &payload.join(name), "WinRE runtime payload")?;
        }
    }
    Ok(())
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
    use super::{EXPECTED_WINRE_SHELL, inject_winre_payload, stage_static_payload};
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

        stage_static_payload(&package, &payload).unwrap();

        assert_eq!(
            fs::read_to_string(payload.join("winpeshl.ini")).unwrap(),
            EXPECTED_WINRE_SHELL
        );
        assert!(payload.join("Recovery.exe").is_file());
        assert!(!payload.join("BackupRestore.exe").exists());
        assert!(!payload.join("RecoveryLauncher.cmd").exists());
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
        stage_static_payload(&package, &payload).unwrap();
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
        stage_static_payload(&package, &payload).unwrap();
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
