//! PE 写任务：稳定身份、完整预检、持久执行意图及失败阻断。
use crate::{
    pe_safety::{self, RestoreBackend, RestoreIntent},
    windows_command, windows_prepare as wp,
};
use backuprestore_core::{TaskError, VolumeIdentity};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
};

fn queue_root() -> Result<PathBuf, TaskError> {
    Ok(root(&wp::esp_identity_without_drive_letter()?)?
        .join("BackupRestore")
        .join("pe-requests"))
}

pub(crate) fn enqueue(request: &Request) -> Result<(), TaskError> {
    let _lock = WriteLock::acquire()?;
    let dir = queue_root()?;
    fs::create_dir_all(&dir)?;
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|s| s == "json") && !path.with_extension("claimed").exists()
        {
            return Err(crate::err("已有待执行 PE 请求，请先处理原任务"));
        }
    }
    let path = dir.join(format!("{}.json", request.id));
    let bytes = serde_json::to_vec_pretty(request)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    if fs::read(&path)? != bytes {
        return Err(crate::err("任务写后回读不一致"));
    }
    Ok(())
}

pub(crate) fn execute_pending() -> Result<bool, TaskError> {
    let _lock = WriteLock::acquire()?;
    let dir = queue_root()?;
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let mut pending = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|s| s == "json") && !path.with_extension("claimed").exists()
        {
            pending.push(path);
        }
    }
    if pending.is_empty() {
        return Ok(false);
    }
    if pending.len() != 1 {
        return Err(crate::err("多个待执行请求冲突，拒绝猜测"));
    }
    let bytes = pe_safety::claim_request(&pending[0])?;
    let request: Request = serde_json::from_slice(&bytes)?;
    let result = execute(&request);
    // 只有全部成功才允许返回；失败保留已领取状态，后续启动不能再次格式化。
    let outcome = pending[0].with_extension("result");
    let detail = match &result {
        Ok(detail) => format!("success\n{detail}"),
        Err(error) => format!("failed\n{error}"),
    };
    fs::write(&outcome, detail)?;
    result.map(|_| true)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Request {
    pub version: u32,
    pub id: String,
    pub operation: String,
    pub target: VolumeIdentity,
    pub image_volume: VolumeIdentity,
    pub image_relative: String,
    pub index: u32,
    pub main: Option<VolumeIdentity>,
    pub name: String,
}

pub(crate) fn new_request(
    operation: &str,
    drive: &str,
    image: &str,
    index: u32,
    main: Option<char>,
    name: &str,
) -> Result<Request, TaskError> {
    let letter = drive_letter(drive)?;
    let image = image.replace('/', "\\");
    backuprestore_core::validate_absolute_path(&image)?;
    let bytes = image.as_bytes();
    if bytes.len() < 4 || bytes[1] != b':' || bytes[2] != b'\\' || !bytes[0].is_ascii_alphabetic() {
        return Err(crate::err("镜像必须位于本地卷的绝对路径"));
    }
    let image_relative = image[3..].to_string();
    backuprestore_core::validate_relative_path(&image_relative)?;
    if index == 0 {
        return Err(crate::err("镜像索引必须大于零"));
    }
    if !["backup", "restore", "secondary", "data"].contains(&operation) {
        return Err(crate::err("未知 PE 操作"));
    }
    Ok(Request {
        version: 1,
        id: format!(
            "{}-{}",
            std::process::id(),
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .ok_or_else(|| crate::err("无法生成任务编号"))?
        ),
        operation: operation.into(),
        target: wp::volume_identity(letter)?,
        image_volume: wp::volume_identity(bytes[0] as char)?,
        image_relative,
        index,
        main: main.map(wp::volume_identity).transpose()?,
        name: name.into(),
    })
}

fn drive_letter(value: &str) -> Result<char, TaskError> {
    let value = value.trim().trim_end_matches(':');
    if value.len() != 1 || !value.as_bytes()[0].is_ascii_alphabetic() {
        return Err(crate::err("盘符无效"));
    }
    Ok(value.as_bytes()[0].to_ascii_uppercase() as char)
}
fn root(identity: &VolumeIdentity) -> Result<PathBuf, TaskError> {
    identity
        .volume_path()
        .map(PathBuf::from)
        .ok_or_else(|| crate::err("卷缺少稳定 GUID 路径"))
}

/// 跨启动允许编号改变，但分区身份、类型、几何和原序列号必须匹配。
fn locate(expected: &VolumeIdentity) -> Result<VolumeIdentity, TaskError> {
    let mut actual = wp::refresh_identity(expected)?;
    let mut stable = expected.clone();
    stable.disk_number = actual.disk_number;
    stable.partition_number = actual.partition_number;
    pe_safety::verify_binding(&stable, &actual, true)?;
    actual.drive_letter = None;
    Ok(actual)
}

/// 所有进程共用内核互斥；进程异常退出自动释放，持久任务领取记录仍阻止重放。
struct WriteLock(*mut std::ffi::c_void);
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(
        attributes: *const std::ffi::c_void,
        owner: i32,
        name: *const u16,
    ) -> *mut std::ffi::c_void;
    fn WaitForSingleObject(handle: *mut std::ffi::c_void, timeout: u32) -> u32;
    fn ReleaseMutex(handle: *mut std::ffi::c_void) -> i32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}
impl WriteLock {
    fn acquire() -> Result<Self, TaskError> {
        let name: Vec<u16> = "Global\\BackupRestore-PE-Write"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        if !matches!(unsafe { WaitForSingleObject(handle, 0) }, 0 | 0x80) {
            unsafe {
                CloseHandle(handle);
            }
            return Err(crate::err("已有 PE 写任务运行，拒绝并发执行"));
        }
        Ok(Self(handle))
    }
}
impl Drop for WriteLock {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

pub(crate) fn execute(request: &Request) -> Result<String, TaskError> {
    let _lock = WriteLock::acquire()?;
    if request.version != 1
        || !["backup", "restore", "secondary", "data"].contains(&request.operation.as_str())
    {
        return Err(crate::err("不支持的 PE 任务版本或动作"));
    }
    backuprestore_core::validate_relative_path(&request.image_relative)?;
    backuprestore_core::validate_relative_path(&request.id)?;
    if request.id.contains(['\\', '/']) {
        return Err(crate::err("任务编号不能含路径"));
    }
    let image_volume = locate(&request.image_volume)?;
    let image = root(&image_volume)?.join(&request.image_relative);
    let parent = image.parent().ok_or_else(|| crate::err("镜像没有父目录"))?;
    fs::read_dir(parent)?;
    // 拒绝目录重解析，防止镜像或日志实际落入待擦除卷。
    let components = pe_safety::relative_components(&request.image_relative)?;
    if request.operation != "backup" {
        if !pe_safety::probe_file(&root(&image_volume)?, &components)? {
            return Err(crate::err("镜像文件不存在，未执行任何格式化"));
        }
    } else {
        let mut path = root(&image_volume)?;
        use std::os::windows::fs::MetadataExt;
        for part in components.iter().take(components.len().saturating_sub(1)) {
            path.push(part);
            if fs::symlink_metadata(&path)?.file_attributes() & 0x400 != 0 {
                return Err(crate::err("备份目录不能经过重解析点"));
            }
        }
    }
    let dir = parent.join(format!(".br-pe-{}", request.id));
    fs::create_dir(&dir)?;
    backuprestore_core::write_json_atomic(dir.join("request.json"), request)?;
    let progress = ProgressGuard(crate::recovery_progress::spawn(
        dir.join("operation.log"),
        Some(&request.operation),
    ));
    let mut backend = Engine {
        request: request.clone(),
        image,
        dir,
        target: locate(&request.target)?,
        image_volume,
        image_handle: None,
        image_hash: String::new(),
        esp: None,
        after: None,
        command_number: 0,
        progress: progress.0.clone(),
        boot_manager: None,
    };
    let result = if request.operation == "backup" {
        backend.backup()
    } else {
        pe_safety::run_restore(&mut backend)
    };
    let detail = format!("日志与执行状态：{}", backend.dir.display());
    result
        .map(|()| detail.clone())
        .map_err(|error| crate::err(&format!("{error}\n{detail}")))
}

struct Engine {
    request: Request,
    image: PathBuf,
    dir: PathBuf,
    target: VolumeIdentity,
    image_volume: VolumeIdentity,
    image_handle: Option<File>,
    image_hash: String,
    esp: Option<VolumeIdentity>,
    after: Option<VolumeIdentity>,
    command_number: u32,
    progress: std::sync::Arc<crate::recovery_progress::ProgressShared>,
    boot_manager: Option<crate::BcdBootManagerState>,
}

struct ProgressGuard(std::sync::Arc<crate::recovery_progress::ProgressShared>);
impl Drop for ProgressGuard {
    fn drop(&mut self) {
        crate::recovery_progress::request_close(&self.0);
    }
}

pub(crate) fn decode_output(bytes: &[u8]) -> Result<String, TaskError> {
    match crate::text_parsing::plan_console_bytes(bytes) {
        crate::text_parsing::ConsoleBytes::Utf8(text) => Ok(text),
        crate::text_parsing::ConsoleBytes::Utf16 => {
            let bytes = bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes);
            if !bytes.len().is_multiple_of(2) {
                return Err(crate::err("UTF-16 输出被截断"));
            }
            let words: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            String::from_utf16(&words).map_err(|_| crate::err("UTF-16 输出解码失败"))
        }
        crate::text_parsing::ConsoleBytes::Oem => crate::text_parsing::decode_code_page(bytes, 1)
            .ok_or_else(|| crate::err("控制台输出解码失败")),
    }
}

impl Engine {
    fn log(&self, text: &str) -> Result<(), TaskError> {
        crate::append_log(&self.dir.join("operation.log"), text)
    }
    fn command(&mut self, program: &str, args: &[String], long: bool) -> Result<String, TaskError> {
        self.command_number += 1;
        let out = self
            .dir
            .join(format!("command-{}.out", self.command_number));
        let tool =
            PathBuf::from(std::env::var("SystemRoot").map_err(|_| crate::err("SystemRoot 未知"))?)
                .join("System32")
                .join(program);
        self.log(&format!("命令开始：{} {args:?}", tool.display()))?;
        self.progress.switch_log_path(out.clone(), 0);
        let outcome = windows_command::run_program(
            &tool.to_string_lossy(),
            args,
            Some(&out),
            if long {
                windows_command::INFINITE
            } else {
                60_000
            },
        );
        self.log(&format!("命令退出：{outcome}；原始输出：{}", out.display()))?;
        let text = decode_output(&fs::read(&out)?)?;
        self.log(&text)?;
        self.progress
            .switch_log_path(self.dir.join("operation.log"), 0);
        if !outcome.is_success() {
            return Err(crate::err(&format!("{program} 失败：{outcome}\n{text}")));
        }
        Ok(text)
    }
    fn protect(&mut self) -> Result<(), TaskError> {
        let mut protected = vec![self.image_volume.clone()];
        let system = std::env::var("SystemDrive").map_err(|_| crate::err("运行卷未知"))?;
        let runtime = drive_letter(&system)?;
        if self.target.drive_letter == Some(runtime) {
            return Err(crate::err("不能写入当前运行卷"));
        }
        if runtime != 'X' {
            protected.push(wp::volume_identity(runtime)?);
        }
        let executable = std::env::current_exe()?;
        let executable = executable.to_string_lossy();
        if executable.as_bytes().get(1) == Some(&b':') {
            let letter = executable.as_bytes()[0] as char;
            if letter != runtime {
                protected.push(wp::volume_identity(letter)?);
            }
        }
        if runtime == 'X' && self.request.operation != "backup" {
            let text = self.command("bcdedit.exe", &["/enum".into(), "{current}".into()], false)?;
            let start = text
                .find("ramdisk=[")
                .ok_or_else(|| crate::err("当前恢复载荷来源未知，拒绝格式化"))?
                + 9;
            let end = text[start..]
                .find(']')
                .ok_or_else(|| crate::err("恢复载荷卷无法解析"))?
                + start;
            let letter = drive_letter(&text[start..end])?;
            protected.push(wp::volume_identity(letter)?);
        }
        let main = self.request.main.as_ref().map(locate).transpose()?;
        if let Some(main) = &main
            && self.intent() == RestoreIntent::SecondaryInstall
            && !pe_safety::probe_file(&root(main)?, &["Windows", "System32", "config", "SYSTEM"])?
        {
            return Err(crate::err("需保留的主系统卷缺少系统证据"));
        }
        pe_safety::check_target(&self.target, &protected, main.as_ref(), self.intent())
    }
    fn recheck(&self, formatted: bool) -> Result<VolumeIdentity, TaskError> {
        let expected = if formatted {
            self.after.as_ref().unwrap_or(&self.target)
        } else {
            &self.target
        };
        let actual = wp::refresh_identity(expected)?;
        pe_safety::verify_binding(expected, &actual, true)?;
        let image = wp::refresh_identity(&self.image_volume)?;
        pe_safety::verify_binding(&self.image_volume, &image, true)?;
        Ok(actual)
    }
    fn backup(&mut self) -> Result<(), TaskError> {
        self.protect()?;
        self.persist("capture-intent")?;
        let letter = wp::ensure_volume_mounted(&self.target, 'T', &self.dir.join("operation.log"))?;
        // 委托已有候选事务；不删除现有镜像、不就地追加、无十分钟硬时限。
        let params = crate::online_operation::OnlineOpParams {
            operation: "backup".into(),
            source_drive: letter.to_string(),
            target_drive: String::new(),
            image_path: self.image.to_string_lossy().into_owned(),
            index: "1".into(),
            compress: "fast".into(),
            image_name: "PE".into(),
            keep_indexes: None,
        };
        let (_, free) = wp::disk_free_space(letter)?;
        let used = self.target.partition_size.saturating_sub(free);
        self.recheck(false)?;
        let result = crate::online_operation::capture(
            &params,
            &root(&self.target)?,
            self.target.clone(),
            used,
            &self.dir.join("operation.log"),
        );
        match result {
            Ok(()) => {
                self.recheck(false)?;
                self.persist("success")
            }
            Err(error) => {
                self.persist(&format!("failed: {error}"))?;
                Err(error)
            }
        }
    }
}

impl Engine {
    fn check_image(&mut self) -> Result<(), TaskError> {
        self.protect()?;
        // 文件句柄拒绝共享写入/删除，持续持有至本次任务结束，避免预检后被替换。
        self.image_handle = Some(
            OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&self.image)?,
        );
        self.image_hash = backuprestore_core::sha256_file(&self.image)?;
        let info = self.command(
            "dism.exe",
            &[
                "/English".into(),
                "/Get-WimInfo".into(),
                format!("/WimFile:{}", self.image.display()),
                format!("/Index:{}", self.request.index),
            ],
            false,
        )?;
        let expanded = info
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                if key.trim() != "Size" {
                    return None;
                }
                value
                    .trim()
                    .strip_suffix("bytes")
                    .map(str::trim)?
                    .replace(',', "")
                    .parse::<u64>()
                    .ok()
            })
            .ok_or_else(|| crate::err("镜像展开容量未知，拒绝写入"))?;
        let mut required = expanded
            .checked_add((expanded / 10).max(64 * 1024 * 1024))
            .ok_or_else(|| crate::err("镜像容量溢出"))?;
        let sidecar = backuprestore_core::image_metadata::index_metadata_path(
            &self.image,
            self.request.index,
        )?;
        match fs::read(&sidecar) {
            Ok(bytes) => {
                let metadata: backuprestore_core::BackupMetadata = serde_json::from_slice(&bytes)?;
                if metadata.wim_index != self.request.index
                    || metadata.image_sha256 != self.image_hash
                {
                    return Err(crate::err("副档索引或摘要不匹配"));
                }
                required = required
                    .max(metadata.required_target_size())
                    .max(metadata.source.partition_size);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::read_dir(sidecar.parent().unwrap())?;
            }
            Err(error) => return Err(error.into()),
        }
        if self.target.partition_size < required {
            return Err(crate::err(&format!(
                "目标容量不足：{} < {required}",
                self.target.partition_size
            )));
        }
        if self.intent() == RestoreIntent::DataOverlay {
            let letter =
                wp::ensure_volume_mounted(&self.target, 'T', &self.dir.join("operation.log"))?;
            pe_safety::verify_binding(&self.target, &wp::volume_identity(letter)?, true)?;
            if wp::disk_free_space(letter)?.1 < required {
                return Err(crate::err("数据覆盖目标可用空间不足"));
            }
        }
        if self.intent() != RestoreIntent::DataOverlay {
            let listing = self.command(
                "dism.exe",
                &[
                    "/English".into(),
                    "/List-Image".into(),
                    format!("/ImageFile:{}", self.image.display()),
                    format!("/Index:{}", self.request.index),
                ],
                true,
            )?;
            let paths: Vec<String> = listing
                .lines()
                .map(|s| s.trim().replace('/', "\\").to_ascii_lowercase())
                .collect();
            for required in [
                "\\windows\\system32\\config\\system",
                "\\windows\\system32\\winload.efi",
                "\\windows\\system32\\ntoskrnl.exe",
            ] {
                if !paths.iter().any(|p| p == required) {
                    return Err(crate::err(&format!(
                        "镜像缺少系统文件 {required}；不能按系统还原成功"
                    )));
                }
            }
        }
        self.log(&format!(
            "预检通过：目标={:?}；镜像摘要={}；索引={}；意图={:?}",
            self.target,
            self.image_hash,
            self.request.index,
            self.intent()
        ))
    }
}

impl RestoreBackend for Engine {
    fn intent(&self) -> RestoreIntent {
        match self.request.operation.as_str() {
            "secondary" => RestoreIntent::SecondaryInstall,
            "data" => RestoreIntent::DataOverlay,
            _ => RestoreIntent::SystemReplace,
        }
    }
    fn preflight(&mut self) -> Result<(), TaskError> {
        self.check_image()?;
        if self.intent() != RestoreIntent::DataOverlay {
            let esp = wp::esp_identity_without_drive_letter()?;
            let store = root(&esp)?
                .join("EFI\\Microsoft\\Boot\\BCD")
                .to_string_lossy()
                .into_owned();
            let before = self.command(
                "bcdedit.exe",
                &[
                    "/store".into(),
                    store.clone(),
                    "/enum".into(),
                    "{bootmgr}".into(),
                    "/v".into(),
                ],
                false,
            )?;
            self.boot_manager = Some(
                crate::parse_boot_manager_state(&before)
                    .ok_or_else(|| crate::err("启动管理器状态未知，拒绝格式化"))?,
            );
            fs::copy(&store, self.dir.join("bcd-before"))?;
            fs::write(self.dir.join("bootmgr-before.txt"), &before)?;
            self.esp = Some(esp);
        }
        Ok(())
    }
    fn persist(&mut self, stage: &str) -> Result<(), TaskError> {
        backuprestore_core::write_json_atomic(
            self.dir.join("runtime.json"),
            &serde_json::json!({"version":1,"request":self.request.id,"stage":stage,"target":self.target,"imageHash":self.image_hash}),
        )?;
        self.log(&format!("状态：{stage}"))
    }
    fn format(&mut self) -> Result<(), TaskError> {
        let actual = self.recheck(false)?;
        let script = crate::diskpart_format_script(actual.disk_number, actual.partition_number)?;
        let path = self.dir.join("format.txt");
        fs::write(&path, script)?;
        self.command(
            "diskpart.exe",
            &["/s".into(), path.to_string_lossy().into_owned()],
            true,
        )?;
        let after = wp::refresh_identity(&actual)?;
        backuprestore_core::operation_safety::verify_formatted_volume(&actual, &after)?;
        fs::read_dir(root(&after)?)?;
        self.after = Some(after);
        Ok(())
    }
    fn apply(&mut self) -> Result<(), TaskError> {
        let actual = self.recheck(self.intent() != RestoreIntent::DataOverlay)?;
        self.command(
            "dism.exe",
            &[
                "/English".into(),
                "/Apply-Image".into(),
                format!("/ImageFile:{}", self.image.display()),
                format!("/Index:{}", self.request.index),
                format!("/ApplyDir:{}", root(&actual)?.display()),
                "/CheckIntegrity".into(),
                "/Verify".into(),
            ],
            true,
        )?;
        Ok(())
    }
    fn verify(&mut self) -> Result<(), TaskError> {
        let actual = self.recheck(self.intent() != RestoreIntent::DataOverlay)?;
        fs::read_dir(root(&actual)?)?;
        if self.intent() != RestoreIntent::DataOverlay {
            for components in [
                vec!["Windows", "System32", "config", "SYSTEM"],
                vec!["Windows", "System32", "winload.efi"],
                vec!["Windows", "System32", "ntoskrnl.exe"],
            ] {
                if !pe_safety::probe_file(&root(&actual)?, &components)? {
                    return Err(crate::err("系统文件验证失败，不能降级为数据还原成功"));
                }
            }
        }
        Ok(())
    }
    fn boot(&mut self) -> Result<(), TaskError> {
        let esp = self
            .esp
            .clone()
            .ok_or_else(|| crate::err("引导许可缺少 ESP"))?;
        pe_safety::verify_binding(&esp, &wp::refresh_identity(&esp)?, true)?;
        let target = self.recheck(true)?;
        let log = self.dir.join("operation.log");
        let esp_letter = wp::ensure_volume_mounted(&esp, 'S', &log)?;
        let target_letter = wp::ensure_volume_mounted(&target, 'T', &log)?;
        pe_safety::verify_binding(&esp, &wp::volume_identity(esp_letter)?, true)?;
        pe_safety::verify_binding(&target, &wp::volume_identity(target_letter)?, true)?;
        let store = format!("{esp_letter}:\\EFI\\Microsoft\\Boot\\BCD");
        let before = self.command(
            "bcdedit.exe",
            &[
                "/store".into(),
                store.clone(),
                "/enum".into(),
                "{bootmgr}".into(),
                "/v".into(),
            ],
            false,
        )?;
        let old = self
            .boot_manager
            .clone()
            .ok_or_else(|| crate::err("缺少格式化前引导状态"))?;
        if crate::parse_boot_manager_state(&before).as_ref() != Some(&old) {
            return Err(crate::err("预检后引导状态发生变化，停止引导写入"));
        }
        self.command(
            "bcdboot.exe",
            &[
                format!("{target_letter}:\\Windows"),
                "/s".into(),
                format!("{esp_letter}:"),
                "/f".into(),
                "UEFI".into(),
                "/d".into(),
            ],
            true,
        )?;
        let after = self.command(
            "bcdedit.exe",
            &[
                "/store".into(),
                store.clone(),
                "/enum".into(),
                "all".into(),
                "/v".into(),
            ],
            false,
        )?;
        let loader = pe_safety::target_loader(&after, target_letter)?;
        if self.intent() == RestoreIntent::SecondaryInstall {
            self.command(
                "bcdedit.exe",
                &[
                    "/store".into(),
                    store.clone(),
                    "/set".into(),
                    loader.clone(),
                    "description".into(),
                    self.request.name.clone(),
                ],
                false,
            )?;
            let mut order = old.display_order.clone();
            if !order.iter().any(|id| id.eq_ignore_ascii_case(&loader)) {
                order.push(loader);
            }
            let mut args = vec!["/store".into(), store.clone(), "/displayorder".into()];
            args.extend(order.clone());
            self.command("bcdedit.exe", &args, false)?;
            self.command(
                "bcdedit.exe",
                &[
                    "/store".into(),
                    store.clone(),
                    "/set".into(),
                    "{bootmgr}".into(),
                    "default".into(),
                    old.default.clone(),
                ],
                false,
            )?;
            let current = self.command(
                "bcdedit.exe",
                &[
                    "/store".into(),
                    store,
                    "/enum".into(),
                    "{bootmgr}".into(),
                    "/v".into(),
                ],
                false,
            )?;
            if crate::parse_boot_manager_state(&current)
                != Some(crate::BcdBootManagerState {
                    default: old.default,
                    display_order: order,
                    resume_object: old.resume_object,
                    loader_resume_objects: old.loader_resume_objects,
                })
            {
                return Err(crate::err("第二系统默认项或菜单顺序不符合约定，拒绝报成功"));
            }
        }
        crate::winre_registration::repair(
            &std::path::PathBuf::from(format!("{target_letter}:\\")),
            &std::path::PathBuf::from(format!("{esp_letter}:\\")),
            &target,
            &log,
        )?;
        self.log("引导与恢复环境关联已回读；系统实际可启动性需另行启动验收")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// 真实格式化、DISM 应用和文件验证；在引导边界注入失败，不修改实际 BCD。
    struct FailBeforeBoot(Engine);
    impl RestoreBackend for FailBeforeBoot {
        fn intent(&self) -> RestoreIntent {
            self.0.intent()
        }
        fn preflight(&mut self) -> Result<(), TaskError> {
            // 本替身不执行引导写入，故仅使用生产代码的完整镜像/目标预检。
            self.0.check_image()
        }
        fn persist(&mut self, stage: &str) -> Result<(), TaskError> {
            self.0.persist(stage)
        }
        fn format(&mut self) -> Result<(), TaskError> {
            self.0.format()
        }
        fn apply(&mut self) -> Result<(), TaskError> {
            self.0.apply()
        }
        fn verify(&mut self) -> Result<(), TaskError> {
            self.0.verify()
        }
        fn boot(&mut self) -> Result<(), TaskError> {
            Err(crate::err("TEST_INJECTED_BOOT_FAILURE"))
        }
    }

    /// 即使断言失败也按本测试的完整路径卸载；不依据盘符寻找并卸载其他虚拟盘。
    struct OwnedVhd(PathBuf);
    impl Drop for OwnedVhd {
        fn drop(&mut self) {
            let script = self.0.with_extension("detach.txt");
            if fs::write(
                &script,
                format!(
                    "select vdisk file=\"{}\"\r\ndetach vdisk\r\n",
                    self.0.display()
                ),
            )
            .is_ok()
            {
                let result = windows_command::run_program(
                    "diskpart.exe",
                    &["/s".into(), script.to_string_lossy().into_owned()],
                    Some(&self.0.with_extension("detach.log")),
                    60_000,
                );
                eprintln!("测试盘卸载：{result}；{}", self.0.display());
            }
        }
    }

    #[test]
    fn console_output_keeps_chinese_and_rejects_truncated_utf16() {
        assert_eq!(decode_output("操作成功".as_bytes()).unwrap(), "操作成功");
        assert_eq!(
            decode_output(&[0xff, 0xfe, 0x2d, 0x4e, 0x87, 0x65]).unwrap(),
            "中文"
        );
        assert!(decode_output(&[0xff, 0xfe, 0x2d]).is_err());
    }

    /// 只创建本轮专有的 128 MiB 虚拟盘；真实系统卷仅作为测试文件目录。
    #[test]
    #[ignore = "需要 Windows 管理员；显式创建一次性 VHD 验证真实 DISM 及无写入拒绝"]
    fn disposable_vhd_preflight_and_index_two_apply() {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetLogicalDrives() -> u32;
        }
        let letters = unsafe { GetLogicalDrives() };
        assert_ne!(letters, 0, "必须能枚举盘符");
        assert_eq!(letters & (1 << (b'V' - b'A')), 0, "V: 已占用，绝不覆盖");
        let dir = std::env::temp_dir().join(format!(
            "br-pe-v212-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&dir).unwrap();
        let vhd = dir.join("disposable.vhdx");
        let guard = OwnedVhd(vhd.clone());
        let script = dir.join("create.txt");
        fs::write(&script, format!("create vdisk file=\"{}\" maximum=128 type=expandable\r\nselect vdisk file=\"{}\"\r\nattach vdisk\r\nconvert gpt\r\ncreate partition primary\r\nformat fs=ntfs quick label=BRPE212\r\nassign letter=V\r\n",vhd.display(),vhd.display())).unwrap();
        let log = dir.join("fixture.log");
        assert!(
            windows_command::run_program(
                "diskpart.exe",
                &["/s".into(), script.to_string_lossy().into_owned()],
                Some(&log),
                60_000
            )
            .is_success()
        );
        let target = wp::volume_identity('V').unwrap();
        assert!(target.is_complete());
        assert!(target.partition_size <= 128 * 1024 * 1024);
        assert_eq!(target.filesystem, "NTFS");
        fs::write(
            "V:\\sentinel.txt",
            "must survive rejected tasks and data overlay",
        )
        .unwrap();
        let source = dir.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("payload.txt"), "first").unwrap();
        let image = dir.join("two-index.wim");
        assert!(
            windows_command::run_program(
                "dism.exe",
                &[
                    "/English".into(),
                    "/Capture-Image".into(),
                    format!("/ImageFile:{}", image.display()),
                    format!("/CaptureDir:{}", source.display()),
                    "/Name:first".into(),
                    "/Compress:fast".into()
                ],
                Some(&log),
                windows_command::INFINITE
            )
            .is_success()
        );
        fs::write(source.join("payload.txt"), "second").unwrap();
        assert!(
            windows_command::run_program(
                "dism.exe",
                &[
                    "/English".into(),
                    "/Append-Image".into(),
                    format!("/ImageFile:{}", image.display()),
                    format!("/CaptureDir:{}", source.display()),
                    "/Name:second".into()
                ],
                Some(&log),
                windows_command::INFINITE
            )
            .is_success()
        );
        let before = wp::volume_identity('V').unwrap();
        let request = new_request("data", "V", image.to_str().unwrap(), 2, None, "").unwrap();
        execute(&request).unwrap();
        assert_eq!(fs::read_to_string("V:\\payload.txt").unwrap(), "second");
        assert!(Path::new("V:\\sentinel.txt").is_file());
        assert_eq!(
            wp::volume_identity('V').unwrap().volume_serial,
            before.volume_serial
        );
        for (name, mode, index) in [
            ("missing.wim", "restore", 1),
            ("two-index.wim", "restore", 999),
            ("two-index.wim", "restore", 1),
        ] {
            let request =
                new_request(mode, "V", dir.join(name).to_str().unwrap(), index, None, "").unwrap();
            assert!(execute(&request).is_err());
            assert!(Path::new("V:\\sentinel.txt").is_file());
            assert_eq!(
                wp::volume_identity('V').unwrap().volume_serial,
                before.volume_serial
            );
        }
        fs::copy(&image, "V:\\on-target.wim").unwrap();
        let conflict = new_request("restore", "V", "V:\\on-target.wim", 1, None, "").unwrap();
        assert!(execute(&conflict).is_err());
        assert!(Path::new("V:\\sentinel.txt").is_file());
        let backup = new_request(
            "backup",
            "V",
            dir.join("capture.wim").to_str().unwrap(),
            1,
            None,
            "",
        )
        .unwrap();
        execute(&backup).unwrap();
        assert!(dir.join("capture.wim").is_file());
        // 合成系统标记仅用于安全步骤测试，不能宣称该镜像能启动。
        fs::create_dir_all(source.join("Windows\\System32\\config")).unwrap();
        for name in ["config\\SYSTEM", "winload.efi", "ntoskrnl.exe"] {
            fs::write(
                source.join("Windows\\System32").join(name),
                "synthetic test fixture",
            )
            .unwrap();
        }
        let system_image = dir.join("synthetic.wim");
        assert!(
            windows_command::run_program(
                "dism.exe",
                &[
                    "/English".into(),
                    "/Capture-Image".into(),
                    format!("/ImageFile:{}", system_image.display()),
                    format!("/CaptureDir:{}", source.display()),
                    "/Name:synthetic".into(),
                ],
                Some(&log),
                windows_command::INFINITE
            )
            .is_success()
        );
        let request =
            new_request("restore", "V", system_image.to_str().unwrap(), 1, None, "").unwrap();
        let state_dir = dir.join("format-apply-failure");
        fs::create_dir(&state_dir).unwrap();
        let progress = ProgressGuard(crate::recovery_progress::spawn(
            state_dir.join("operation.log"),
            Some("restore"),
        ));
        let mut backend = FailBeforeBoot(Engine {
            target: locate(&request.target).unwrap(),
            image_volume: locate(&request.image_volume).unwrap(),
            request,
            image: system_image,
            dir: state_dir.clone(),
            image_handle: None,
            image_hash: String::new(),
            esp: None,
            after: None,
            command_number: 0,
            progress: progress.0.clone(),
            boot_manager: None,
        });
        let error = pe_safety::run_restore(&mut backend).unwrap_err();
        assert!(
            error.to_string().contains("TEST_INJECTED_BOOT_FAILURE"),
            "{error}"
        );
        assert!(!Path::new("V:\\sentinel.txt").exists());
        assert_ne!(
            wp::volume_identity('V').unwrap().volume_serial,
            before.volume_serial
        );
        assert_eq!(fs::read_to_string("V:\\payload.txt").unwrap(), "second");
        let state: serde_json::Value =
            serde_json::from_slice(&fs::read(state_dir.join("runtime.json")).unwrap()).unwrap();
        assert!(state["stage"].as_str().unwrap().starts_with("failed:"));
        drop(backend);
        drop(progress);
        // 保留全部文件和证据；只卸载本测试刚创建、路径已绑定的虚拟盘。
        let detach = dir.join("detach.txt");
        fs::write(
            &detach,
            format!(
                "select vdisk file=\"{}\"\r\ndetach vdisk\r\n",
                vhd.display()
            ),
        )
        .unwrap();
        assert!(
            windows_command::run_program(
                "diskpart.exe",
                &["/s".into(), detach.to_string_lossy().into_owned()],
                Some(&log),
                60_000
            )
            .is_success()
        );
        assert_eq!(unsafe { GetLogicalDrives() } & (1 << (b'V' - b'A')), 0);
        std::mem::forget(guard);
        println!("PE_SAFETY_NATIVE_PASS evidence={}", dir.display());
    }
}
