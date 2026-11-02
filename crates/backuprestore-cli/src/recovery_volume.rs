//! 恢复环境（PE / WinRE）下的卷挂载、盘符分配、身份复核与分区格式化模块。
//!
//! 包含：
//! - `RecoveryTask.env` 键值读取与跨重启卷身份比对；
//! - 卷挂载、候选盘符分配（`mountvol` / `diskpart`）；
//! - 跨重启动态盘符寻址（`resolve_volume_letter`, `letter_for_partition`）；
//! - 目标分区 DiskPart 快速格式化与安全核验；
//! - EFI 分区及系统镜像卷解析。

#[cfg(windows)]
use std::collections::BTreeMap;
#[cfg(windows)]
use std::fs::{self, OpenOptions};
#[cfg(windows)]
use std::io::Read;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
use backuprestore_core::{Operation, Stage, Task, TaskStore, VolumeIdentity};
use backuprestore_core::TaskError;

#[cfg(windows)]
use crate::error_code::ErrCode;
#[cfg(windows)]
use crate::error_code::errc_with;
#[cfg(windows)]
use crate::error_code::errc;
#[cfg(windows)]
use crate::process_runner::run_logged;
#[cfg(windows)]
use crate::text_parsing::{
    MountedVolumes, VolumeMountQuery, classify_mountvol_output, parse_mountvol_listing,
};
#[cfg(windows)]
use crate::{append_log, windows_command, windows_prepare, pe_safety};
use crate::err;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
pub(crate) fn read_env_file(path: impl AsRef<Path>) -> Result<BTreeMap<String, String>, TaskError> {
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
pub(crate) fn env_required(values: &BTreeMap<String, String>, key: &str) -> Result<String, TaskError> {
    values
        .get(key)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| err(&format!("RecoveryTask.env is missing {key}")))
}

#[cfg(windows)]
pub(crate) fn env_u32(values: &BTreeMap<String, String>, key: &str) -> Result<u32, TaskError> {
    env_required(values, key)?
        .parse::<u32>()
        .map_err(|_| err(&format!("RecoveryTask.env {key} is not a number")))
}

#[cfg(windows)]
pub(crate) fn env_u64(values: &BTreeMap<String, String>, key: &str) -> Result<u64, TaskError> {
    env_required(values, key)?
        .parse::<u64>()
        .map_err(|_| err(&format!("RecoveryTask.env {key} is not a number")))
}

#[cfg(windows)]
pub(crate) fn env_optional(values: &BTreeMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

#[cfg(windows)]
pub(crate) fn env_optional_u64(values: &BTreeMap<String, String>, key: &str) -> Option<u64> {
    env_optional(values, key).and_then(|value| value.parse::<u64>().ok())
}

#[cfg(windows)]
pub(crate) fn verify_task_identity_env(
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
pub(crate) fn verify_image_absolute_path(
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
pub(crate) fn mount_env_volume(
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
pub(crate) fn assign_volume_letter(
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
    // mountvol 的输出落盘：用 `run_program` 带作业对象约束进程树、支持超时，且把输出写进文件。
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

/// Snapshot every current volume→letter mount by running `mountvol` with no arguments.
#[cfg(windows)]
pub(crate) fn enumerate_mounted_volumes(log: &Path) -> Result<Vec<(String, Vec<char>)>, TaskError> {
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

/// Candidate letters for a role, best first.
#[cfg(windows)]
pub(crate) fn mount_letter_candidates(
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
pub(crate) fn verify_live_volume_identity(
    letter: char,
    values: &BTreeMap<String, String>,
    prefix: &str,
    allow_reformatted_serial: bool,
) -> Result<(), TaskError> {
    let live = crate::windows_prepare::volume_identity_with_known_guid(
        letter,
        env_required(values, &format!("{prefix}_VOLUME_GUID"))?,
    )?;
    verify_env_volume_identity(&live, values, prefix, allow_reformatted_serial)
}

#[cfg(windows)]
pub(crate) fn verify_env_volume_identity(
    live: &VolumeIdentity,
    values: &BTreeMap<String, String>,
    prefix: &str,
    allow_reformatted_serial: bool,
) -> Result<(), TaskError> {
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
#[cfg(windows)]
pub(crate) fn query_mounted_volume(letter: char, log: &Path) -> Result<VolumeMountQuery, TaskError> {
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
pub(crate) fn verify_mounted_volume(letter: char, expected: &str, log: &Path) -> Result<(), TaskError> {
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
pub(crate) fn native_windows_architecture() -> String {
    let reported = [
        std::env::var("PROCESSOR_ARCHITEW6432").ok(),
        std::env::var("PROCESSOR_ARCHITECTURE").ok(),
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

/// 记录的身份与现场身份是否指向**同一个卷**。
#[cfg(windows)]
pub(crate) fn volume_identity_matches(
    recorded: &VolumeIdentity,
    live: &VolumeIdentity,
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
    !recorded.disk_guid.is_empty()
        && recorded.partition_offset > 0
        && recorded.disk_guid.eq_ignore_ascii_case(&live.disk_guid)
        && recorded.partition_offset == live.partition_offset
        && recorded.partition_size == live.partition_size
}

/// 把任务记录的卷身份解析成**本次运行里真实可用**的盘符。
#[cfg(windows)]
pub(crate) fn resolve_volume_letter(
    volume: &VolumeIdentity,
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
pub(crate) fn assign_letter_by_volume_guid(
    volume: &VolumeIdentity,
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
pub(crate) fn resolve_volume_root(
    volume: &VolumeIdentity,
    log: &Path,
) -> Result<PathBuf, TaskError> {
    Ok(PathBuf::from(format!(
        "{}:\\",
        resolve_volume_letter(volume, log)?
    )))
}

#[cfg(windows)]
pub(crate) fn resolve_efi_root_for_recovery(
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
pub(crate) fn verify_partition_identity_after_format(
    expected: &VolumeIdentity,
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
pub(crate) fn diskpart_format_script(
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
pub(crate) fn format_target_partition(
    store: &TaskStore,
    task: &Task,
    target: &VolumeIdentity,
    log: &Path,
) -> Result<(), TaskError> {
    let letter = target
        .drive_letter
        .ok_or_else(|| errc(ErrCode::FormatTargetLetterMissing))?;
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
pub(crate) fn resolve_volume_path(
    volume: &VolumeIdentity,
    relative: &str,
    log: &Path,
) -> Result<PathBuf, TaskError> {
    Ok(resolve_volume_root(volume, log)?
        .join(relative.replace('\\', std::path::MAIN_SEPARATOR_STR)))
}

#[cfg(windows)]
pub(crate) fn find_efi_root(override_root: Option<&Path>) -> Result<PathBuf, TaskError> {
    override_root
        .map(Path::to_path_buf)
        .ok_or_else(|| err("EFI root must be explicitly mounted before recovery"))
}
