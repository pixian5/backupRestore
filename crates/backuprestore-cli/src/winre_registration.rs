//! 系统还原收尾：启用目标系统的恢复环境，并核验实际 BCD 关联。
use backuprestore_core::TaskError;

#[cfg(all(test, windows))]
#[path = "winre_registration_fault_tests.rs"]
mod fault_tests;

#[derive(Debug, PartialEq, Eq)]
struct Info {
    enabled: bool,
    location: Option<String>,
    recovery_id: Option<String>,
}

fn parse_info(text: &str) -> Result<Info, TaskError> {
    let states: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (label, value) = line.split_once(':')?;
            if !label.to_ascii_lowercase().contains("windows re") {
                return None;
            }
            match value.trim().to_ascii_lowercase().as_str() {
                "enabled" => Some(true),
                "disabled" => Some(false),
                _ => None,
            }
        })
        .collect();
    if states.len() != 1 {
        return Err(crate::err("无法唯一确认 WinRE 启用状态"));
    }
    let locations: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (_, value) = line.split_once(':')?;
            let value = value.trim().to_ascii_lowercase();
            value
                .starts_with("\\\\?\\globalroot\\device\\harddisk")
                .then_some(value)
        })
        .collect();
    if locations.len() > 1 {
        return Err(crate::err("WinRE 注册位置不唯一"));
    }
    let identifiers: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (label, value) = line.split_once(':')?;
            label.contains("BCD").then_some(value.trim())
        })
        .collect();
    let recovery_id = match identifiers.as_slice() {
        [value]
            if value.len() == 36
                && crate::bcd_identifier_from_line(&format!("{{{value}}}")).is_some() =>
        {
            Some(format!("{{{}}}", value.to_ascii_lowercase()))
        }
        [] if !states[0] => None,
        _ => return Err(crate::err("WinRE 注册标识符缺失或不唯一")),
    };
    Ok(Info {
        enabled: states[0],
        location: locations.into_iter().next(),
        recovery_id,
    })
}

fn field(block: &str, name: &str) -> Result<String, TaskError> {
    let values: Vec<_> = block
        .lines()
        .filter_map(|line| {
            let mut parts = line.trim().splitn(2, char::is_whitespace);
            parts
                .next()?
                .eq_ignore_ascii_case(name)
                .then(|| parts.next().unwrap_or("").trim().to_ascii_lowercase())
        })
        .collect();
    if values.len() != 1 {
        return Err(crate::err(&format!("BCD 字段缺失或重复：{name}")));
    }
    Ok(values[0].clone())
}

fn object<'a>(text: &'a str, id: &str) -> Result<&'a str, TaskError> {
    let blocks: Vec<_> = text
        .split("\n\n")
        .filter(|block| {
            block
                .lines()
                .find_map(|line| {
                    let key = line.split_whitespace().next()?;
                    if key.eq_ignore_ascii_case("identifier") || key == "标识符" {
                        crate::bcd_identifier_from_line(line)
                    } else {
                        None
                    }
                })
                .is_some_and(|value| value.eq_ignore_ascii_case(id))
        })
        .collect();
    if blocks.len() != 1 {
        return Err(crate::err("BCD 对象缺失或重复"));
    }
    Ok(blocks[0])
}

fn verify(
    info: &Info,
    text: &str,
    loader: &str,
    letter: char,
    location: &str,
) -> Result<(), TaskError> {
    if !info.enabled || info.location.as_deref() != Some(location) {
        return Err(crate::err("WinRE 未启用或注册位置与目标分区不符"));
    }
    if !crate::pe_safety::target_loader(text, letter)?.eq_ignore_ascii_case(loader) {
        return Err(crate::err("WinRE 关联的目标系统加载器已变化"));
    }
    let text = text.replace("\r\n", "\n");
    let os = object(&text, loader)?;
    let recovery = field(os, "recoverysequence")?;
    if crate::bcd_identifier_from_line(&recovery) != Some(recovery.as_str()) {
        return Err(crate::err("系统恢复关联不是唯一 GUID"));
    }
    if field(os, "recoveryenabled")? != "yes" {
        return Err(crate::err("系统恢复启动未启用"));
    }
    if info.recovery_id.as_deref() != Some(recovery.as_str()) {
        return Err(crate::err("BCD 恢复关联与 WinRE 注册标识符不一致"));
    }
    verify_recovery_object(&text, &recovery, letter)
}

fn verify_recovery_object(text: &str, recovery: &str, letter: char) -> Result<(), TaskError> {
    let re = object(text, recovery)?;
    let device = field(re, "device")?;
    let prefix = format!(
        "ramdisk=[{}:]\\recovery\\windowsre\\winre.wim,",
        letter.to_ascii_lowercase()
    );
    let options = device
        .strip_prefix(&prefix)
        .ok_or_else(|| crate::err("恢复加载器未指向目标原始 Winre.wim"))?;
    if crate::bcd_identifier_from_line(options) != Some(options)
        || field(re, "osdevice")? != device
        || field(re, "path")? != "\\windows\\system32\\winload.efi"
        || field(re, "winpe")? != "yes"
    {
        return Err(crate::err("恢复加载器设备或执行模式不完整"));
    }
    let ramdisk = object(text, options)?;
    if field(ramdisk, "ramdisksdidevice")? != format!("partition={}:", letter.to_ascii_lowercase())
        || field(ramdisk, "ramdisksdipath")? != "\\recovery\\windowsre\\boot.sdi"
    {
        return Err(crate::err("恢复内存盘启动文件不在目标注册目录"));
    }
    Ok(())
}

trait Backend {
    fn preflight(&mut self) -> Result<(), TaskError>;
    fn info(&mut self) -> Result<Info, TaskError>;
    fn register(&mut self) -> Result<(), TaskError>;
    fn enable(&mut self) -> Result<(), TaskError>;
    fn reconnect(&mut self, info: &Info) -> Result<(), TaskError>;
    fn verify(&mut self, info: &Info) -> Result<(), TaskError>;
}

fn run(backend: &mut impl Backend) -> Result<(), TaskError> {
    backend.preflight()?;
    let before = backend.info()?;
    // BCDBoot 可重建系统加载器；注册仍启用并不代表新加载器已有恢复关联。
    if !before.enabled {
        backend.register()?;
        backend.enable()?;
    } else {
        backend.reconnect(&before)?;
    }
    let after = backend.info()?;
    backend.verify(&after)
}

#[cfg(windows)]
pub(crate) fn repair(
    target: &std::path::Path,
    efi: &std::path::Path,
    expected: &backuprestore_core::VolumeIdentity,
    log: &std::path::Path,
) -> Result<(), TaskError> {
    use crate::{append_log, capture_logged, pe_safety, windows_prepare as wp};
    use std::{fs, path::Path};
    struct WindowsBackend<'a> {
        target: &'a Path,
        efi: &'a Path,
        log: &'a Path,
        expected: &'a backuprestore_core::VolumeIdentity,
        letter: char,
        loader: String,
        location: String,
        hashes: Vec<(std::path::PathBuf, String)>,
        original_sdi: Option<Vec<u8>>,
    }
    impl WindowsBackend<'_> {
        fn bcd(&self, active: bool) -> Result<String, TaskError> {
            let store = self.efi.join("EFI\\Microsoft\\Boot\\BCD");
            let path = store.to_string_lossy();
            let args = if active {
                vec!["/enum", "all", "/v"]
            } else {
                vec!["/store", &path, "/enum", "all", "/v"]
            };
            capture_logged("bcdedit.exe", &args, self.log)
        }
        fn reagent(&self, args: &[&str]) -> Result<String, TaskError> {
            let program = self.target.join("Windows\\System32\\reagentc.exe");
            // 详细诊断直接落到任务盘；不能依赖重启即消失的 X: 默认日志。
            let diagnostic = self.log.with_extension("reagent.log");
            let diagnostic = diagnostic.to_string_lossy();
            let mut args = args.to_vec();
            args.extend(["/logpath", &diagnostic]);
            capture_logged(&program.to_string_lossy(), &args, self.log)
        }
        fn identity(&self) -> Result<(), TaskError> {
            pe_safety::verify_binding(self.expected, &wp::volume_identity(self.letter)?, false)
        }
    }
    impl Backend for WindowsBackend<'_> {
        fn preflight(&mut self) -> Result<(), TaskError> {
            capture_logged(
                "reg.exe",
                &["query", "HKLM\\SYSTEM\\CurrentControlSet\\Control\\MiniNT"],
                self.log,
            )
            .map_err(|_| crate::err("离线 WinRE 注册只允许在 PE 中执行"))?;
            // 自定义 WinPE 入口可能绕过 wpeinit；让系统工具读取本次真实固件类型。
            capture_logged("wpeutil.exe", &["UpdateBootInfo"], self.log)?;
            let firmware = capture_logged(
                "reg.exe",
                &[
                    "query",
                    "HKLM\\SYSTEM\\CurrentControlSet\\Control",
                    "/v",
                    "PEFirmwareType",
                ],
                self.log,
            )?;
            if !firmware.lines().any(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                fields == ["PEFirmwareType", "REG_DWORD", "0x2"]
            }) {
                return Err(crate::err("WinRE 注册要求已确认的 UEFI 固件环境"));
            }
            self.identity()?;
            for parts in [
                vec!["Windows", "System32", "reagentc.exe"],
                vec!["Windows", "System32", "config", "SYSTEM"],
            ] {
                if !pe_safety::probe_file(self.target, &parts)? {
                    return Err(crate::err("目标缺少离线注册组件"));
                }
            }
            self.loader = pe_safety::target_loader(&self.bcd(false)?, self.letter)?;
            // reagentc 没有 /store，必须先确认活动存储中的同一目标，禁止误关联别的系统。
            if pe_safety::target_loader(&self.bcd(true)?, self.letter)? != self.loader {
                return Err(crate::err("指定 ESP 与活动 BCD 的目标加载器不一致"));
            }
            self.location = format!(
                "\\\\?\\globalroot\\device\\harddisk{}\\partition{}\\recovery\\windowsre",
                self.expected
                    .disk_number
                    .ok_or_else(|| crate::err("目标磁盘编号缺失"))?,
                self.expected
                    .partition_number
                    .ok_or_else(|| crate::err("目标分区编号缺失"))?
            );
            for (name, source) in [
                (
                    "Winre.wim",
                    vec!["Windows", "System32", "Recovery", "Winre.wim"],
                ),
                (
                    "boot.sdi",
                    vec!["Windows", "Boot", "DVD", "EFI", "boot.sdi"],
                ),
            ] {
                let parts = ["Recovery", "WindowsRE", name];
                let destination = self.target.join("Recovery\\WindowsRE").join(name);
                // 仅使用本次还原镜像内的原件；绝不拿任务注入镜像作为系统 WinRE。
                if !pe_safety::probe_file(self.target, &parts)? {
                    if !pe_safety::probe_file(self.target, &source)? {
                        return Err(crate::err(&format!("还原镜像缺少恢复原件：{name}")));
                    }
                    let source = source
                        .iter()
                        .fold(self.target.to_path_buf(), |path, item| path.join(item));
                    let hash = crate::sha256_file(&source)?;
                    fs::create_dir_all(destination.parent().expect("恢复目录"))?;
                    fs::copy(&source, &destination)?;
                    if crate::sha256_file(&destination)? != hash {
                        return Err(crate::err("恢复原件复制校验失败"));
                    }
                }
                if fs::metadata(&destination)?.len() == 0 {
                    return Err(crate::err("恢复原件为空"));
                }
                self.hashes
                    .push((destination.clone(), crate::sha256_file(&destination)?));
                if name == "boot.sdi" {
                    self.original_sdi = Some(fs::read(&destination)?);
                }
            }
            append_log(
                self.log,
                &format!(
                    "WinRE registration target={} loader={} location={}",
                    self.letter, self.loader, self.location
                ),
            )
        }
        fn info(&mut self) -> Result<Info, TaskError> {
            let windows = self.target.join("Windows");
            parse_info(&self.reagent(&["/info", "/target", &windows.to_string_lossy()])?)
        }
        fn register(&mut self) -> Result<(), TaskError> {
            self.identity()?;
            let windows = self.target.join("Windows");
            let recovery = self.target.join("Recovery\\WindowsRE");
            self.reagent(&[
                "/setreimage",
                "/path",
                &recovery.to_string_lossy(),
                "/target",
                &windows.to_string_lossy(),
            ])?;
            Ok(())
        }
        fn enable(&mut self) -> Result<(), TaskError> {
            self.identity()?;
            self.reagent(&["/enable", "/osguid", &self.loader])?;
            Ok(())
        }
        fn reconnect(&mut self, info: &Info) -> Result<(), TaskError> {
            self.identity()?;
            if info.location.as_deref() != Some(self.location.as_str()) {
                return Err(crate::err("已启用 WinRE 不在目标分区，拒绝重接"));
            }
            let recovery = info
                .recovery_id
                .as_deref()
                .ok_or_else(|| crate::err("WinRE 注册标识符缺失"))?;
            let mut missing = false;
            // 两个存储都必须拥有经过完整核验的原件链；不能凭注册状态臆造恢复对象。
            for active in [false, true] {
                let text = self.bcd(active)?.replace("\r\n", "\n");
                verify_recovery_object(&text, recovery, self.letter)?;
                let os = object(&text, &self.loader)?;
                let links: Vec<_> = os
                    .lines()
                    .filter(|line| {
                        line.split_whitespace()
                            .next()
                            .is_some_and(|key| key.eq_ignore_ascii_case("recoverysequence"))
                    })
                    .collect();
                if links.is_empty() {
                    missing = true;
                } else if field(os, "recoverysequence")? != recovery {
                    return Err(crate::err("系统已有不同的恢复关联，拒绝覆盖"));
                }
                missing |= field(os, "recoveryenabled").ok().as_deref() != Some("yes");
            }
            if missing {
                self.identity()?;
                let store = self.efi.join("EFI\\Microsoft\\Boot\\BCD");
                for (key, value) in [("recoverysequence", recovery), ("recoveryenabled", "Yes")] {
                    capture_logged(
                        "bcdedit.exe",
                        &[
                            "/store",
                            &store.to_string_lossy(),
                            "/set",
                            &self.loader,
                            key,
                            value,
                        ],
                        self.log,
                    )?;
                }
                append_log(
                    self.log,
                    "WinRE existing registration reconnected to repaired system loader",
                )?;
            }
            Ok(())
        }
        fn verify(&mut self, info: &Info) -> Result<(), TaskError> {
            self.identity()?;
            verify(
                info,
                &self.bcd(false)?,
                &self.loader,
                self.letter,
                &self.location,
            )?;
            verify(
                info,
                &self.bcd(true)?,
                &self.loader,
                self.letter,
                &self.location,
            )?;
            for (file, hash) in &self.hashes {
                if crate::sha256_file(file)? != *hash {
                    return Err(crate::err("注册过程改变了恢复原件"));
                }
            }
            append_log(
                self.log,
                "WinRE registration verified: Enabled; target location, recoverysequence, ramdisk and original hashes match",
            )
        }
    }
    let letter = target
        .to_string_lossy()
        .chars()
        .next()
        .filter(char::is_ascii_alphabetic)
        .ok_or_else(|| crate::err("目标盘符无效"))?;
    let mut backend = WindowsBackend {
        target,
        efi,
        log,
        expected,
        letter,
        loader: String::new(),
        location: String::new(),
        hashes: Vec::new(),
        original_sdi: None,
    };
    let result = run(&mut backend);
    if let Err(error) = result {
        // reagentc 在启用失败时可能删除已经存在的 boot.sdi，不能让失败破坏原件。
        if let Some(bytes) = &backend.original_sdi {
            let rollback = (|| -> Result<(), TaskError> {
                backend.identity()?;
                pe_safety::probe_file(target, &["Recovery", "WindowsRE", "boot.sdi"])?;
                let path = target.join("Recovery\\WindowsRE\\boot.sdi");
                if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
                    fs::write(&path, bytes)?;
                    if fs::read(&path)? != *bytes {
                        return Err(crate::err("WinRE 启用失败后的 boot.sdi 恢复校验失败"));
                    }
                    append_log(
                        log,
                        "WinRE registration failed; original boot.sdi restored and verified",
                    )?;
                }
                Ok(())
            })();
            if let Err(rollback) = rollback {
                return Err(crate::err(&format!("{error}; {rollback}")));
            }
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const OS: &str = "{11111111-1111-1111-1111-111111111111}";
    const RE: &str = "{22222222-2222-2222-2222-222222222222}";
    const RD: &str = "{33333333-3333-3333-3333-333333333333}";
    const LOC: &str = "\\\\?\\globalroot\\device\\harddisk1\\partition2\\recovery\\windowsre";
    /// 需外部新快照与专用测试卷；常规测试不执行任何 BCD 写入。
    #[cfg(windows)]
    #[test]
    #[ignore = "需要真实 PE、全新 VM 快照和独立注册夹具"]
    fn offline_registration_on_test_partition() {
        use crate::{capture_logged, windows_prepare as wp};
        use std::{env, path::PathBuf};
        assert_eq!(
            env::var("BR_WINRE_TEST_ACK").as_deref(),
            Ok("fresh-snapshot-test-volume-only")
        );
        let letter = env::var("BR_WINRE_TEST_TARGET")
            .unwrap()
            .chars()
            .next()
            .unwrap();
        let primary = env::var("BR_WINRE_TEST_PRIMARY")
            .unwrap()
            .chars()
            .next()
            .unwrap();
        assert_ne!(letter, primary);
        let target = PathBuf::from(format!("{letter}:\\"));
        assert_eq!(
            std::fs::read_to_string(target.join("BRRE-215-fixture.marker")).unwrap(),
            "offline-registration-fixture"
        );
        let expected = wp::volume_identity(letter).unwrap();
        assert!(
            expected
                .partition_guid
                .eq_ignore_ascii_case(&env::var("BR_WINRE_TEST_GUID").unwrap())
        );
        let efi = PathBuf::from(env::var("BR_WINRE_TEST_EFI").unwrap());
        let log = PathBuf::from(env::var("BR_WINRE_TEST_LOG").unwrap());
        capture_logged(
            "reg.exe",
            &["query", "HKLM\\SYSTEM\\CurrentControlSet\\Control\\MiniNT"],
            &log,
        )
        .unwrap();
        let before = capture_logged("bcdedit.exe", &["/enum", "all", "/v"], &log).unwrap();
        let manager = crate::parse_boot_manager_state(&before).unwrap();
        let primary_id = crate::pe_safety::target_loader(&before, primary).unwrap();
        let copy = capture_logged(
            "bcdedit.exe",
            &[
                "/copy",
                &primary_id,
                "/d",
                "BackupRestore registration fixture",
            ],
            &log,
        )
        .unwrap();
        let id = crate::bcd_identifier_from_line(&copy).unwrap().to_string();
        let result = (|| -> Result<(), TaskError> {
            let partition = format!("partition={letter}:");
            for key in ["device", "osdevice"] {
                capture_logged("bcdedit.exe", &["/set", &id, key, &partition], &log)?;
            }
            capture_logged("bcdedit.exe", &["/displayorder", &id, "/remove"], &log)?;
            // 夹具加载器不继承主系统恢复关联；只对新建对象操作。
            if field(
                object(&before.replace("\r\n", "\n"), &primary_id)?,
                "recoverysequence",
            )
            .is_ok()
            {
                capture_logged(
                    "bcdedit.exe",
                    &["/deletevalue", &id, "recoverysequence"],
                    &log,
                )?;
            }
            // 克隆项不能共享主系统的休眠恢复对象，否则 reagentc /enable
            // 会沿 resumeobject 修改主系统的恢复开关，污染测试基线。
            if field(
                object(&before.replace("\r\n", "\n"), &primary_id)?,
                "resumeobject",
            )
            .is_ok()
            {
                capture_logged("bcdedit.exe", &["/deletevalue", &id, "resumeobject"], &log)?;
            }
            fault_tests::preflight_failures(&target, &efi, &expected, &id, &log)?;
            repair(&target, &efi, &expected, &log)?;
            fault_tests::registered_failures(&target, &efi, &expected, &id, &primary_id, &log)?;
            repair(&target, &efi, &expected, &log)?;
            // 重现实际 C: 故障：注册保持启用，但 BCDBoot 新加载器丢失恢复关联。
            capture_logged(
                "bcdedit.exe",
                &["/deletevalue", &id, "recoverysequence"],
                &log,
            )?;
            capture_logged("bcdedit.exe", &["/set", &id, "recoveryenabled", "No"], &log)?;
            repair(&target, &efi, &expected, &log)?;
            Ok(())
        })();
        // 删除本次新建且指向测试卷的恢复对象及克隆加载器，保留原 BCD 对象。
        let after = capture_logged("bcdedit.exe", &["/enum", "all", "/v"], &log)
            .unwrap()
            .replace("\r\n", "\n");
        for block in after.split("\n\n") {
            let Some(created) = block.lines().find_map(crate::bcd_identifier_from_line) else {
                continue;
            };
            if before.contains(created) || created == id {
                continue;
            }
            let lower = block.to_ascii_lowercase();
            if lower.contains(&format!("ramdisk=[{}:]", letter.to_ascii_lowercase()))
                || field(block, "ramdisksdidevice").ok().as_deref()
                    == Some(&format!("partition={}:", letter.to_ascii_lowercase()))
            {
                capture_logged("bcdedit.exe", &["/delete", created], &log).unwrap();
            }
        }
        capture_logged("bcdedit.exe", &["/delete", &id], &log).unwrap();
        let final_bcd = capture_logged("bcdedit.exe", &["/enum", "all", "/v"], &log).unwrap();
        assert_eq!(crate::parse_boot_manager_state(&final_bcd), Some(manager));
        assert_eq!(final_bcd, before, "测试改变了原有启动对象，必须恢复原基线");
        result.unwrap();
    }
    fn bcd() -> String {
        format!(
            "identifier {OS}\ndevice partition=P:\nosdevice partition=P:\npath \\Windows\\system32\\winload.efi\nrecoverysequence {RE}\nrecoveryenabled Yes\n\nidentifier {RE}\ndevice ramdisk=[P:]\\Recovery\\WindowsRE\\Winre.wim,{RD}\nosdevice ramdisk=[P:]\\Recovery\\WindowsRE\\Winre.wim,{RD}\npath \\windows\\system32\\winload.efi\nwinpe Yes\n\nidentifier {RD}\nramdisksdidevice partition=P:\nramdisksdipath \\Recovery\\WindowsRE\\boot.sdi\n"
        )
    }
    #[test]
    fn info_requires_unique_known_status() {
        assert!(!parse_info("Windows RE 状态: Disabled").unwrap().enabled);
        let valid = format!(
            "Windows RE status: Enabled\nWindows RE location: {LOC}\nBCD identifier: {}",
            RE.trim_matches(['{', '}'])
        );
        assert_eq!(
            parse_info(&valid).unwrap(),
            Info {
                enabled: true,
                location: Some(LOC.into()),
                recovery_id: Some(RE.into())
            }
        );
        for invalid in [
            "成功",
            "Windows RE status: unknown",
            "Windows RE status: Enabled\nWindows RE status: Disabled",
            "Windows RE status: Enabled\nBCD identifier: malformed",
            "Windows RE status: Enabled",
        ] {
            assert!(parse_info(invalid).is_err());
        }
        assert!(
            parse_info(&format!(
                "{valid}\nBCD identifier: {}",
                RE.trim_matches(['{', '}'])
            ))
            .is_err()
        );
    }
    #[test]
    fn binding_requires_complete_exact_chain() {
        let info = Info {
            enabled: true,
            location: Some(LOC.into()),
            recovery_id: Some(RE.into()),
        };
        assert!(verify(&info, &bcd(), OS, 'P', LOC).is_ok());
        assert!(
            verify(
                &info,
                &bcd().replace("identifier", "标识符").replace('\n', "\r\n"),
                OS,
                'P',
                LOC
            )
            .is_ok()
        );
        for bad in [
            bcd().replace("recoveryenabled Yes", "recoveryenabled No"),
            bcd().replace("[P:]", "[C:]"),
            bcd().replace(
                "ramdisksdidevice partition=P:",
                "ramdisksdidevice partition=C:",
            ),
            bcd().replace("winpe Yes", "winpe No"),
            bcd().replace("boot.sdi", "other.sdi"),
            bcd().replace("recoverysequence", "description"),
            format!("{}\n\nidentifier {RD}\n", bcd()),
            bcd().replace(
                "recoveryenabled Yes",
                "recoveryenabled Yes\nrecoveryenabled Yes",
            ),
        ] {
            assert!(verify(&info, &bad, OS, 'P', LOC).is_err(), "{bad}");
        }
        assert!(
            verify(
                &Info {
                    enabled: false,
                    location: Some(LOC.into()),
                    recovery_id: Some(RE.into())
                },
                &bcd(),
                OS,
                'P',
                LOC
            )
            .is_err()
        );
        assert!(verify(&info, &bcd(), OS, 'P', "other").is_err());
        assert!(
            verify(
                &Info {
                    recovery_id: Some(OS.into()),
                    ..info
                },
                &bcd(),
                OS,
                'P',
                LOC
            )
            .is_err()
        );
    }
    struct Fake {
        calls: Vec<&'static str>,
        fail: &'static str,
        enabled: bool,
        false_success: bool,
    }
    impl Fake {
        fn step(&mut self, name: &'static str) -> Result<(), TaskError> {
            self.calls.push(name);
            if self.fail == name {
                Err(crate::err("注入故障"))
            } else {
                Ok(())
            }
        }
    }
    impl Backend for Fake {
        fn preflight(&mut self) -> Result<(), TaskError> {
            self.step("preflight")
        }
        fn info(&mut self) -> Result<Info, TaskError> {
            self.step("info")?;
            Ok(Info {
                enabled: self.enabled,
                location: None,
                recovery_id: None,
            })
        }
        fn register(&mut self) -> Result<(), TaskError> {
            self.step("register")
        }
        fn enable(&mut self) -> Result<(), TaskError> {
            self.step("enable")?;
            self.enabled = !self.false_success;
            Ok(())
        }
        fn reconnect(&mut self, _info: &Info) -> Result<(), TaskError> {
            self.step("reconnect")
        }
        fn verify(&mut self, info: &Info) -> Result<(), TaskError> {
            self.step("verify")?;
            if info.enabled {
                Ok(())
            } else {
                Err(crate::err("仍未启用"))
            }
        }
    }
    #[test]
    fn registration_stops_on_each_failure_and_rejects_false_success() {
        for fail in ["preflight", "info", "register", "enable", "verify"] {
            let mut fake = Fake {
                calls: Vec::new(),
                fail,
                enabled: false,
                false_success: false,
            };
            assert!(run(&mut fake).is_err());
            assert_eq!(fake.calls.last(), Some(&fail));
        }
        let mut fake = Fake {
            calls: Vec::new(),
            fail: "",
            enabled: false,
            false_success: true,
        };
        assert!(run(&mut fake).is_err());
        let mut fake = Fake {
            enabled: true,
            fail: "reconnect",
            ..fake
        };
        assert!(run(&mut fake).is_err());
        assert_eq!(fake.calls.last(), Some(&"reconnect"));
    }
    #[test]
    fn registration_is_idempotent_and_always_rereads() {
        let mut fake = Fake {
            calls: Vec::new(),
            fail: "",
            enabled: false,
            false_success: false,
        };
        run(&mut fake).unwrap();
        assert_eq!(
            fake.calls,
            ["preflight", "info", "register", "enable", "info", "verify"]
        );
        fake.calls.clear();
        run(&mut fake).unwrap();
        assert_eq!(
            fake.calls,
            ["preflight", "info", "reconnect", "info", "verify"]
        );
    }
}
