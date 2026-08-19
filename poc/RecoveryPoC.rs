#![cfg_attr(windows, windows_subsystem = "windows")]

use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // X: is the temporary WinRE RAM disk; the first argument is the persistent volume.
    let volume = std::env::args().nth(1).unwrap_or_else(|| "C:".to_string());
    let marker_path = format!(r"{volume}\WinRE-PoC");
    let marker_dir = Path::new(&marker_path);
    let marker = marker_dir.join("recovery-started.txt");
    let _ = create_dir_all(marker_dir);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&marker) {
        let _ = writeln!(file, "RecoveryPoC started; unix_seconds={timestamp}");
    }

    let _ = std::process::Command::new(r"X:\Windows\System32\wpeutil.exe")
        .args(["UpdateBootInfo"])
        .status();
}
