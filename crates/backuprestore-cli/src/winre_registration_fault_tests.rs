//! 真实 PE 中的注册失败矩阵，仅由显式忽略测试在专用夹具上调用。
use super::*;
use crate::{capture_logged, sha256_file};
use backuprestore_core::VolumeIdentity;
use std::{fs, path::Path};

fn bcd(log: &Path) -> Result<String, TaskError> {
    capture_logged("bcdedit.exe", &["/enum", "all", "/v"], log)
}

/// 还原测试字段必须保留原始路径大小写，不能复用用于语义比较的小写解析值。
fn original_field(block: &str, name: &str) -> Result<String, TaskError> {
    field(block, name)?;
    block
        .lines()
        .find_map(|line| {
            let mut parts = line.trim().splitn(2, char::is_whitespace);
            (parts.next()?.eq_ignore_ascii_case(name))
                .then(|| parts.next().unwrap_or("").trim().to_string())
        })
        .ok_or_else(|| crate::err("测试字段原始文本缺失"))
}

/// 失败必须命中指定闸门；损坏启动链还可在工具虚假成功后的回读阶段拒绝。
fn rejected(
    name: &str,
    target: &Path,
    efi: &Path,
    expected: &VolumeIdentity,
    message: &str,
    allow_false_enable: bool,
    log: &Path,
) -> Result<(), TaskError> {
    let case_log = log.with_file_name(format!("fault-{name}.log"));
    if case_log.exists() {
        return Err(crate::err("故障测试日志已存在，拒绝复用旧证据"));
    }
    let before = bcd(log)?;
    let error = repair(target, efi, expected, &case_log)
        .err()
        .ok_or_else(|| crate::err(&format!("{name}: 故障被错误地报告为成功")))?;
    let false_enable = allow_false_enable
        && error
            .to_string()
            .contains("WinRE 未启用或注册位置与目标分区不符");
    if !error.to_string().contains(message) && !false_enable {
        return Err(crate::err(&format!("{name}: 未命中预期闸门：{error}")));
    }
    if bcd(log)? != before {
        return Err(crate::err(&format!("{name}: 失败修改了活动 BCD")));
    }
    let text = fs::read_to_string(&case_log)?;
    let commands_match = if false_enable {
        // 真实 reagentc 可在损坏的现有恢复对象上返回退出码 0，前后却都是 Disabled。
        // 必须证明执行过两个命令且最终仍被回读拒绝，不能把任意失败放进白名单。
        text.contains("/setreimage")
            && text.contains("/enable")
            && text.matches("Disabled").count() >= 2
    } else {
        !text.contains("/setreimage") && !text.contains("/enable")
    };
    if !commands_match || text.contains("registration verified:") {
        return Err(crate::err(&format!("{name}: 越过了注册闸门或错误报告成功")));
    }
    println!("FAULT_PASS {name}: {error}; active BCD unchanged; false_enable={false_enable}");
    Ok(())
}

/// 临时隐藏夹具文件；测试失败或提前返回时也恢复原件，不删除备份。
struct HiddenFiles(Vec<(std::path::PathBuf, std::path::PathBuf, String)>);

impl HiddenFiles {
    fn hide(paths: &[std::path::PathBuf]) -> Result<Self, TaskError> {
        let mut guard = Self(Vec::new());
        for path in paths {
            if path.try_exists()? {
                let backup = path.with_extension("br-fault-backup");
                if backup.try_exists()? {
                    return Err(crate::err("夹具原件备份已存在，拒绝覆盖"));
                }
                let hash = sha256_file(path)?;
                fs::rename(path, &backup)?;
                guard.0.push((path.clone(), backup, hash));
            }
        }
        Ok(guard)
    }

    fn restore(&mut self) -> Result<(), TaskError> {
        while let Some((path, backup, hash)) = self.0.last() {
            // Windows 的重命名不覆盖已有文件；这里只删除本测试生成的空占位。
            if path.try_exists()? {
                if fs::metadata(path)?.len() != 0 {
                    return Err(crate::err("原件路径出现非空文件，保留备份供人工核查"));
                }
                fs::remove_file(path)?;
            }
            fs::rename(backup, path)?;
            if sha256_file(path)? != *hash {
                return Err(crate::err("故障测试恢复原件摘要不符"));
            }
            self.0.pop();
        }
        Ok(())
    }
}

impl Drop for HiddenFiles {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("FAULT_FIXTURE_RESTORE_FAILED: {error}");
        }
    }
}

pub(super) fn preflight_failures(
    target: &Path,
    efi: &Path,
    expected: &VolumeIdentity,
    loader: &str,
    log: &Path,
) -> Result<(), TaskError> {
    let mut wrong = expected.clone();
    wrong.partition_guid = "{00000000-0000-0000-0000-000000000001}".into();
    rejected("identity-mismatch", target, efi, &wrong, "身份", false, log)?;

    for (name, paths, message, empty) in [
        (
            "missing-wim",
            [
                "Recovery\\WindowsRE\\Winre.wim",
                "Windows\\System32\\Recovery\\Winre.wim",
            ],
            "还原镜像缺少恢复原件：Winre.wim",
            false,
        ),
        (
            "empty-wim",
            [
                "Recovery\\WindowsRE\\Winre.wim",
                "Windows\\System32\\Recovery\\Winre.wim",
            ],
            "恢复原件为空",
            true,
        ),
        (
            "missing-sdi",
            [
                "Recovery\\WindowsRE\\boot.sdi",
                "Windows\\Boot\\DVD\\EFI\\boot.sdi",
            ],
            "还原镜像缺少恢复原件：boot.sdi",
            false,
        ),
        (
            "empty-sdi",
            [
                "Recovery\\WindowsRE\\boot.sdi",
                "Windows\\Boot\\DVD\\EFI\\boot.sdi",
            ],
            "恢复原件为空",
            true,
        ),
    ] {
        let paths = paths.map(|path| target.join(path));
        let mut hidden = HiddenFiles::hide(&paths)?;
        if empty {
            fs::write(&paths[0], [])?;
        }
        let result = rejected(name, target, efi, expected, message, false, log);
        hidden.restore()?;
        result?;
    }

    // 使用任务目录里的独立 BCD 副本制造不同加载器，不覆盖真实 ESP。
    let shadow = log
        .parent()
        .ok_or_else(|| crate::err("测试日志缺少目录"))?
        .join("fault-shadow-efi");
    let store = shadow.join("EFI\\Microsoft\\Boot\\BCD");
    if shadow.try_exists()? {
        return Err(crate::err("故障测试 BCD 副本已存在"));
    }
    fs::create_dir_all(
        store
            .parent()
            .ok_or_else(|| crate::err("测试存储缺少目录"))?,
    )?;
    let store_text = store.to_string_lossy();
    capture_logged("bcdedit.exe", &["/export", &store_text], log)?;
    capture_logged(
        "bcdedit.exe",
        &[
            "/store",
            &store_text,
            "/copy",
            loader,
            "/d",
            "BackupRestore fault shadow",
        ],
        log,
    )?;
    capture_logged(
        "bcdedit.exe",
        &["/store", &store_text, "/delete", loader],
        log,
    )?;
    // BCD 是注册表配置单元，系统工具仅枚举它也可能改变配置单元文件的内部表示。
    // 必须比较全部对象与字段；文件摘要只能用来诊断，不能判定配置发生写入。
    let before = capture_logged(
        "bcdedit.exe",
        &["/store", &store_text, "/enum", "all", "/v"],
        log,
    )?;
    rejected(
        "bcd-store-mismatch",
        target,
        &shadow,
        expected,
        "目标加载器不一致",
        false,
        log,
    )?;
    let after = capture_logged(
        "bcdedit.exe",
        &["/store", &store_text, "/enum", "all", "/v"],
        log,
    )?;
    if before != after {
        return Err(crate::err("失败修改了指定 BCD 副本的对象或字段"));
    }
    Ok(())
}

pub(super) fn registered_failures(
    target: &Path,
    efi: &Path,
    expected: &VolumeIdentity,
    loader: &str,
    primary: &str,
    log: &Path,
) -> Result<(), TaskError> {
    let before = bcd(log)?.replace("\r\n", "\n");
    let recovery = field(object(&before, loader)?, "recoverysequence")?;
    let original_link = field(object(&before, primary)?, "recoverysequence")?;
    if recovery == original_link {
        return Err(crate::err("测试卷意外关联主系统恢复对象"));
    }
    let device = field(object(&before, &recovery)?, "device")?;
    let options = device
        .rsplit_once(',')
        .ok_or_else(|| crate::err("测试恢复对象缺少内存盘选项"))?
        .1;
    // PE 会重新分配盘符；制造错误设备类型时仍使用现场已确认的测试卷。
    let wrong_device = format!(
        "partition={}",
        target.to_string_lossy().trim_end_matches('\\')
    );
    let hashes = ["Winre.wim", "boot.sdi"].map(|name| {
        let path = target.join("Recovery\\WindowsRE").join(name);
        sha256_file(&path).map(|hash| (path, hash))
    });
    let hashes = hashes.into_iter().collect::<Result<Vec<_>, _>>()?;
    for (name, id, key, value, message) in [
        (
            "conflicting-link",
            loader,
            "recoverysequence",
            original_link.as_str(),
            "不同的恢复关联",
        ),
        (
            "wrong-wim-device",
            recovery.as_str(),
            "device",
            wrong_device.as_str(),
            "未指向目标原始",
        ),
        (
            "wrong-sdi-path",
            options,
            "ramdisksdipath",
            "\\missing\\boot.sdi",
            "启动文件不在目标注册目录",
        ),
    ] {
        let original = original_field(object(&before, id)?, key)?;
        capture_logged("bcdedit.exe", &["/set", id, key, value], log)?;
        let result = rejected(
            name,
            target,
            efi,
            expected,
            message,
            matches!(name, "wrong-wim-device" | "wrong-sdi-path"),
            log,
        );
        // 每个失败场景立即恢复本测试拥有的对象，再核验整个 BCD 与恢复原件。
        capture_logged("bcdedit.exe", &["/set", id, key, &original], log)?;
        result?;
        if bcd(log)?.replace("\r\n", "\n") != before {
            return Err(crate::err("注入后没有完整恢复测试对象"));
        }
        for (path, hash) in &hashes {
            if sha256_file(path)? != *hash {
                return Err(crate::err("恢复原件被失败场景改变"));
            }
        }
    }
    Ok(())
}
