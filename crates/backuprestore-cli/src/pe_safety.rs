//! PE 的写盘契约与步骤编排；不依赖 Windows，故障测试直接覆盖真实编排。
use backuprestore_core::{TaskError, VolumeIdentity};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RestoreIntent {
    SystemReplace,
    SecondaryInstall,
    DataOverlay,
}

/// 同时拆解两种 Windows 分隔符，逐层拒绝可改变解析语义的路径组件。
pub(crate) fn relative_components(value: &str) -> Result<Vec<&str>, TaskError> {
    let parts: Vec<_> = value.split(['\\', '/']).collect();
    if parts.iter().any(|part| {
        part.is_empty()
            || *part == "."
            || *part == ".."
            || part.contains(':')
            || part.ends_with(['.', ' '])
            || part.chars().any(char::is_control)
    }) {
        return Err(crate::err("镜像相对路径包含无效组件"));
    }
    Ok(parts)
}

/// 只接受同一对象内的精确设备字段；描述文字和重复字段不能冒充加载器证据。
pub(crate) fn target_loader(text: &str, letter: char) -> Result<String, TaskError> {
    let text = text.replace("\r\n", "\n");
    let device = format!("partition={letter}:").to_ascii_lowercase();
    let mut found = Vec::new();
    for block in text.split("\n\n") {
        let field = |key: &str| {
            let values: Vec<_> = block
                .lines()
                .filter_map(|line| {
                    let mut parts = line.trim().splitn(2, char::is_whitespace);
                    crate::text_parsing::bcd_field_name_is(parts.next()?, key)
                        .then(|| parts.next().unwrap_or("").trim())
                })
                .collect();
            (values.len() == 1).then(|| values[0].to_ascii_lowercase())
        };
        if field("device").as_deref() == Some(&device)
            && field("osdevice").as_deref() == Some(&device)
            && field("path").as_deref() == Some("\\windows\\system32\\winload.efi")
        {
            let id = block
                .lines()
                .find_map(crate::bcd_identifier_from_line)
                .ok_or_else(|| crate::err("目标加载器缺少对象 GUID"))?;
            found.push(id.to_string());
        }
    }
    if found.len() != 1 {
        return Err(crate::err("目标引导加载器不唯一或设备字段不完整"));
    }
    Ok(found.remove(0))
}

pub(crate) fn verify_binding(
    expected: &VolumeIdentity,
    actual: &VolumeIdentity,
    serial: bool,
) -> Result<(), TaskError> {
    if !expected.is_complete()
        || !actual.is_complete()
        || !expected.same_partition(actual)
        || !expected
            .partition_type_guid
            .eq_ignore_ascii_case(&actual.partition_type_guid)
        || expected.partition_offset != actual.partition_offset
        || expected.partition_size != actual.partition_size
        || expected.disk_number != actual.disk_number
        || expected.partition_number != actual.partition_number
        || (serial
            && (expected.volume_serial.is_empty()
                || expected.volume_serial != actual.volume_serial))
    {
        return Err(crate::err("卷身份、现场编号或几何已变化，拒绝写入"));
    }
    Ok(())
}

/// 跨启动只重新绑定现场地址；稳定身份、几何和文件系统仍须全部吻合。
/// 返回值用于本次操作，后续写入继续由 verify_binding 拒绝编号再次变化。
pub(crate) fn rebind_after_boot(
    expected: &VolumeIdentity,
    actual: &VolumeIdentity,
    serial: bool,
) -> Result<VolumeIdentity, TaskError> {
    if !expected.is_complete() {
        return Err(crate::err("准备期卷身份不完整，拒绝重新绑定"));
    }
    let mut rebound = expected.clone();
    rebound.disk_number = actual.disk_number;
    rebound.partition_number = actual.partition_number;
    rebound.drive_letter = actual.drive_letter;
    verify_binding(&rebound, actual, serial)?;
    if !expected.filesystem.eq_ignore_ascii_case(&actual.filesystem)
        || !crate::text_parsing::same_volume(&expected.volume_guid, &actual.volume_guid)
    {
        return Err(crate::err("跨启动卷身份或文件系统变化，拒绝重新绑定"));
    }
    Ok(rebound)
}

pub(crate) fn check_target(
    target: &VolumeIdentity,
    protected: &[VolumeIdentity],
    main: Option<&VolumeIdentity>,
    intent: RestoreIntent,
) -> Result<(), TaskError> {
    if !target.is_complete()
        || target.is_reserved_partition()
        || !target
            .partition_type_guid
            .eq_ignore_ascii_case("{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}")
        || !target.filesystem.eq_ignore_ascii_case("NTFS")
    {
        return Err(crate::err("目标必须是身份完整的 GPT/NTFS 基本数据分区"));
    }
    for role in protected {
        if !role.is_complete() || role.same_partition(target) {
            return Err(crate::err(
                "目标与镜像、工作区、运行程序或恢复载荷冲突，或必需角色未知",
            ));
        }
    }
    if intent == RestoreIntent::SecondaryInstall {
        let main = main.ok_or_else(|| crate::err("第二系统任务缺少已确认的主系统身份"))?;
        if !main.is_complete() || main.same_partition(target) {
            return Err(crate::err("第二系统不能覆盖主系统，确认不能豁免角色禁区"));
        }
    }
    Ok(())
}

/// 逐级检查目录，明确区分不存在、错误和重解析路径；未知不能降级为非系统。
pub(crate) fn probe_file(root: &Path, components: &[&str]) -> Result<bool, TaskError> {
    probe_file_inner(root, components, true)
}

/// 目标卷是否为完整的 Windows 安装：以 SYSTEM hive 是否存在为判据，
/// 与离线侧分叉 bcdboot 的判据保持一致（见 `main.rs` 恢复流程）。
/// 复用 `probe_file` 以继承"拒绝重解析点穿越"的保护。
/// 放在本模块是因为 `online_operation` 是 `#[cfg(windows)]`，这里可在本机单测。
pub(crate) fn target_is_windows_volume(root: &Path) -> Result<bool, TaskError> {
    probe_file(root, &["Windows", "System32", "config", "SYSTEM"])
}

/// GUI 中英双语文案的语言维度（CLI 输出固定中文）。
/// 定义在本模块是因为这里是 `cfg(any(windows, test))`、macOS 单测可直达；
/// 使用者：`failure_policy`（设置窗口标签/详情/解析错误）、`dism_timeout`（提示语）。
/// 注意：覆盖式还原判据（下方 `overlay_restore_on_windows_volume`）#110 起收敛为
/// 纯谓词，不再需要语言维度——它的旧双语文案是硬拒时代的，已随硬拒一起退役。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageLanguage {
    Chinese,
    English,
}

/// 「覆盖式应用（取消格式化）」打到**已含 Windows 的目标卷**的判据
/// （准备期命中；用户 2026-10-16 拍板立项，2026-10-22 #110 起改路由）。
///
/// 返回 `true` = 命中「已含 Windows + 取消格式化」组合。
/// 判据抽成纯函数以便单测锁定行为，与 `boot_menu::decide_default` 同一取向
/// （行为固化到可测函数，不散落在分支里）。
///
/// **精确根因**（PE 内对照实验钉死，见 `docs/20261016-1413` §2 与
/// `docs/20261016-1836`）：DISM 覆盖式应用要把镜像里 `Program Files\WindowsApps\...`
/// （MSIX 应用包目录，owner = `NT SERVICE\TrustedInstaller`、带 MSIX 专用 ACL）的
/// **目录元数据 / Security Descriptor / owner** 还原到卷上；该系统受 TrustedInstaller
/// 保护、拒绝对这类目录改写 owner/ACL → `RestoreDirMetadata:(2418): Access is denied`
/// → `0x80070005` → DISM **立即整体放弃**（不继续，exit 5）。
///
/// 旁证（PE 内实测）：该目录下 `md` 建子目录**成功**、`rd` 删除**成功**（普通数据写入
/// 允许），**只有"改写既有受保护目录的元数据"被拒**。格式化能过是因为受保护目录被
/// 清空，DISM 只需**新建**（允许）而非**改写**。
///
/// 该组合与 RE/在线/PE 环境无关、与 DISM 版本无关（PE 里同镜像同样失败）；
/// `--skip-format` 只对**数据卷**（无 Windows 的卷，如 2026-10-14 验证的 T:）有效。
/// 判据用「目标卷当前是否含 Windows」而非「是否活动系统卷」：任何既有 Windows 卷
/// 都带 TrustedInstaller 保护的目录，都会命中同一根因。
///
/// **#110 行为变迁（2026-10-22）**：用户裁定「换 wimlib --no-acls」后，本判据从
/// 「硬拒（返回错误文案）」改为「路由（置 `overlay_no_acl=true`，恢复期走 wimlib
/// `--no-acls` 通道，跳过 SD 还原 → 结构性免疫 0x80070005）」。随之删除的两样东西：
/// ① `MessageLanguage` 语言维度与中英双语文案——#110 起三个调用点（prepare 离线 /
///   在线链 / GUI 预检）都只取布尔值，旧文案在发布件里从未输出（被优化器 GC）；
///   GUI 确认框自带双语文案，语义是「告知代价并请示」而非旧硬拒文案的「指导用户
///   改参数」，有意不再强行单一来源（R3 文案分叉护栏随硬拒时代一起退役）。
/// ② 旧硬拒时代「请勾选格式化目标分区」的操作引导。
/// 根因分析与四象限判据不变，由下方单测继续锁定。
pub(crate) fn overlay_restore_on_windows_volume(
    target_contains_windows: bool,
    format_target: bool,
) -> bool {
    target_contains_windows && !format_target
}

/// 活动 BCD 可能由系统独占；这里只验证类型和祖先，内容可读性由 bcdedit 回读验证。
#[cfg(windows)]
pub(crate) fn probe_file_metadata(root: &Path, components: &[&str]) -> Result<bool, TaskError> {
    probe_file_inner(root, components, false)
}

fn probe_file_inner(root: &Path, components: &[&str], open: bool) -> Result<bool, TaskError> {
    let root_info = fs::symlink_metadata(root)?;
    if !root_info.is_dir() || root_info.file_type().is_symlink() {
        return Err(crate::err("探测卷根不可用或存在重解析"));
    }
    let mut path = root.to_path_buf();
    for (index, part) in components.iter().enumerate() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(info) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if info.file_attributes() & 0x400 != 0 {
                        return Err(crate::err("安全探测不跟随重解析点"));
                    }
                }
                if info.file_type().is_symlink() {
                    return Err(crate::err("安全探测不跟随链接"));
                }
                if index + 1 == components.len() {
                    if !info.is_file() {
                        return Err(crate::err("必要文件路径不是普通文件"));
                    }
                    if open {
                        fs::File::open(&path)?;
                    }
                    return Ok(true);
                }
                if !info.is_dir() {
                    return Err(crate::err("探测祖先路径不是目录"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // 再验证祖先仍可达，避免卷离线产生的 NotFound 被当成缺文件。
                fs::read_dir(path.parent().ok_or_else(|| crate::err("探测父路径缺失"))?)?;
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(crate::err("探测路径为空"))
}

/// 领取记录采用独占创建并刷新；记录存在（包括上轮崩溃留下的半记录）就拒绝重放。
pub(crate) fn claim_request(request: &Path) -> Result<Vec<u8>, TaskError> {
    let bytes = fs::read(request)?;
    let mut claim = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(request.with_extension("claimed"))?;
    claim.write_all(&bytes)?;
    claim.sync_all()?;
    Ok(bytes)
}

pub(crate) trait RestoreBackend {
    fn intent(&self) -> RestoreIntent;
    fn preflight(&mut self) -> Result<(), TaskError>;
    fn persist(&mut self, stage: &str) -> Result<(), TaskError>;
    fn format(&mut self) -> Result<(), TaskError>;
    fn apply(&mut self) -> Result<(), TaskError>;
    fn verify(&mut self) -> Result<(), TaskError>;
    fn boot(&mut self) -> Result<(), TaskError>;
}

pub(crate) fn run_restore(backend: &mut impl RestoreBackend) -> Result<(), TaskError> {
    let result = (|| {
        backend.preflight()?;
        if backend.intent() != RestoreIntent::DataOverlay {
            backend.persist("format-intent")?;
            backend.format()?;
            backend.persist("format-verified")?;
        }
        backend.persist("apply-intent")?;
        backend.apply()?;
        backend.verify()?;
        backend.persist("apply-verified")?;
        if backend.intent() != RestoreIntent::DataOverlay {
            backend.persist("boot-intent")?;
            backend.boot()?;
            backend.persist("boot-verified")?;
        }
        backend.persist("success")
    })();
    if let Err(ref error) = result {
        // 失败状态若也无法落盘，原先意图记录仍会阻止自动重放；不能改成成功。
        if let Err(write_error) = backend.persist(&format!("failed: {error}")) {
            return Err(crate::err(&format!(
                "{error}；失败状态落盘也失败：{write_error}"
            )));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loader_proof_requires_exact_unique_fields() {
        let good = "标识符 {11111111-1111-1111-1111-111111111111}\ndevice partition=T:\nosdevice partition=T:\npath \\Windows\\system32\\winload.efi";
        assert!(target_loader(good, 'T').is_ok());
        assert!(target_loader(&good.replace("osdevice", "description"), 'T').is_err());
        assert!(
            target_loader(
                &good.replace("osdevice partition=T:", "osdevice partition=T:\\other"),
                'T'
            )
            .is_err()
        );
        assert!(target_loader(&format!("{good}\ndevice partition=T:"), 'T').is_err());
        assert!(target_loader(&format!("{good}\n\n{good}"), 'T').is_err());
    }
    #[test]
    fn mixed_separators_cannot_skip_ancestor_checks() {
        assert_eq!(
            relative_components("folder/junction\\image.wim").unwrap(),
            vec!["folder", "junction", "image.wim"]
        );
        for path in ["a/../b", "a/./b", "/root", "a//b", "a/b:", "a./b", "a /b"] {
            assert!(relative_components(path).is_err(), "{path}");
        }
    }
    fn identity(partition: &str) -> VolumeIdentity {
        VolumeIdentity {
            disk_guid: "{11111111-1111-1111-1111-111111111111}".into(),
            partition_guid: partition.into(),
            volume_guid: "{22222222-2222-2222-2222-222222222222}".into(),
            partition_type_guid: "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}".into(),
            disk_number: Some(1),
            partition_number: Some(2),
            partition_offset: 1048576,
            partition_size: 128 * 1024 * 1024,
            filesystem: "NTFS".into(),
            volume_serial: "serial".into(),
            drive_letter: Some('V'),
        }
    }
    #[test]
    fn protected_roles_and_unknown_main_cannot_be_overridden() {
        let target = identity("target");
        let main = identity("main");
        assert!(check_target(&target, &[], Some(&main), RestoreIntent::SecondaryInstall).is_ok());
        assert!(check_target(&target, &[], None, RestoreIntent::SecondaryInstall).is_err());
        assert!(
            check_target(&target, &[], Some(&target), RestoreIntent::SecondaryInstall).is_err()
        );
        assert!(
            check_target(
                &target,
                std::slice::from_ref(&target),
                None,
                RestoreIntent::SystemReplace
            )
            .is_err()
        );
        let mut unknown = main.clone();
        unknown.partition_type_guid.clear();
        assert!(check_target(&target, &[unknown], None, RestoreIntent::SystemReplace).is_err());
        for kind in [
            "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}",
            "{de94bba4-06d1-4d40-a16a-bfd50179d6ac}",
            "unknown",
        ] {
            let mut reserved = target.clone();
            reserved.partition_type_guid = kind.into();
            assert!(check_target(&reserved, &[], None, RestoreIntent::SystemReplace).is_err());
        }
    }
    #[test]
    fn checked_numbers_cannot_change_before_write() {
        let before = identity("target");
        let mut after = before.clone();
        after.disk_number = Some(7);
        assert!(verify_binding(&before, &after, true).is_err());
        after = before.clone();
        after.partition_number = Some(4);
        assert!(verify_binding(&before, &after, true).is_err());
        after = before.clone();
        after.volume_serial = "new".into();
        assert!(verify_binding(&before, &after, true).is_err());
        assert!(verify_binding(&before, &after, false).is_ok());
    }

    #[test]
    fn boot_rebinding_keeps_stable_identity_and_pins_new_addresses() {
        let before = identity("target");
        let mut current = before.clone();
        current.disk_number = Some(7);
        current.partition_number = Some(4);
        current.drive_letter = Some('Q');
        let rebound = rebind_after_boot(&before, &current, true).unwrap();
        assert_eq!(rebound.disk_number, Some(7));
        assert_eq!(rebound.partition_number, Some(4));
        assert!(verify_binding(&rebound, &current, true).is_ok());
        let script =
            crate::diskpart_format_script(rebound.disk_number, rebound.partition_number).unwrap();
        assert!(script.starts_with("select disk 7\r\nselect partition 4\r\n"));
        current.disk_number = Some(8);
        assert!(verify_binding(&rebound, &current, true).is_err());
        for mutate in [
            |v: &mut VolumeIdentity| v.disk_guid = "foreign".into(),
            |v: &mut VolumeIdentity| v.partition_guid = "foreign".into(),
            |v: &mut VolumeIdentity| v.volume_guid = "foreign".into(),
            |v: &mut VolumeIdentity| v.partition_type_guid = "foreign".into(),
            |v: &mut VolumeIdentity| v.partition_offset += 4096,
            |v: &mut VolumeIdentity| v.partition_size += 4096,
            |v: &mut VolumeIdentity| v.filesystem = "FAT32".into(),
            |v: &mut VolumeIdentity| v.volume_serial = "foreign".into(),
            |v: &mut VolumeIdentity| v.disk_number = None,
        ] {
            let mut foreign = current.clone();
            mutate(&mut foreign);
            assert!(rebind_after_boot(&before, &foreign, true).is_err());
        }
    }
    struct Recorder {
        calls: Vec<String>,
        fail: &'static str,
        intent: RestoreIntent,
    }
    impl Recorder {
        fn call(&mut self, name: &str) -> Result<(), TaskError> {
            self.calls.push(name.into());
            if self.fail == name {
                Err(crate::err("注入失败"))
            } else {
                Ok(())
            }
        }
    }
    impl RestoreBackend for Recorder {
        fn intent(&self) -> RestoreIntent {
            self.intent
        }
        fn preflight(&mut self) -> Result<(), TaskError> {
            self.call("preflight")
        }
        fn persist(&mut self, s: &str) -> Result<(), TaskError> {
            self.call(s)
        }
        fn format(&mut self) -> Result<(), TaskError> {
            self.call("format")
        }
        fn apply(&mut self) -> Result<(), TaskError> {
            self.call("apply")
        }
        fn verify(&mut self) -> Result<(), TaskError> {
            self.call("verify")
        }
        fn boot(&mut self) -> Result<(), TaskError> {
            self.call("boot")
        }
    }
    #[test]
    fn every_failure_stops_all_following_writes() {
        let sequence = [
            "preflight",
            "format-intent",
            "format",
            "format-verified",
            "apply-intent",
            "apply",
            "verify",
            "apply-verified",
            "boot-intent",
            "boot",
            "boot-verified",
            "success",
        ];
        for (index, fail) in sequence.iter().enumerate() {
            let mut backend = Recorder {
                calls: vec![],
                fail,
                intent: RestoreIntent::SystemReplace,
            };
            assert!(run_restore(&mut backend).is_err());
            assert_eq!(&backend.calls[..index + 1], &sequence[..index + 1]);
            assert_eq!(backend.calls.len(), index + 2);
            assert!(backend.calls.last().unwrap().starts_with("failed:"));
        }
    }
    #[test]
    fn data_overlay_never_formats_or_repairs_boot() {
        let mut backend = Recorder {
            calls: vec![],
            fail: "",
            intent: RestoreIntent::DataOverlay,
        };
        run_restore(&mut backend).unwrap();
        assert!(
            !backend
                .calls
                .iter()
                .any(|s| s.contains("format") || s.contains("boot"))
        );
        assert_eq!(backend.calls.last().unwrap(), "success");
    }
    #[test]
    fn failed_or_completed_request_is_not_replayed() {
        let dir = std::env::temp_dir().join(format!(
            "br-claim-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("request.json");
        fs::write(&path, b"request").unwrap();
        assert_eq!(claim_request(&path).unwrap(), b"request");
        assert!(claim_request(&path).is_err());
        fs::write(&path, b"changed").unwrap();
        assert!(claim_request(&path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn missing_root_and_non_file_are_unknown_not_absent() {
        let dir = std::env::temp_dir().join(format!("br-probe-{}", std::process::id()));
        fs::create_dir_all(dir.join("directory")).unwrap();
        assert!(probe_file(&dir.join("offline"), &["SYSTEM"]).is_err());
        assert!(probe_file(&dir, &["directory"]).is_err());
        assert!(!probe_file(&dir, &["absent"]).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    // 锁住"目标是不是另一个 Windows 卷"的判据：只有真正存在 SYSTEM hive 才算，
    // 空目录或只有目录骨架的数据卷不能被误拒（在线还原必须放行）。
    #[test]
    fn windows_volume_detected_only_with_system_hive() {
        let dir = std::env::temp_dir().join(format!("br-winvol-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Windows").join("System32").join("config")).unwrap();
        assert!(!target_is_windows_volume(&dir).unwrap());

        fs::write(
            dir.join("Windows")
                .join("System32")
                .join("config")
                .join("SYSTEM"),
            b"hive",
        )
        .unwrap();
        assert!(target_is_windows_volume(&dir).unwrap());

        let empty = std::env::temp_dir().join(format!("br-winvol-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&empty);
        fs::create_dir_all(&empty).unwrap();
        assert!(!target_is_windows_volume(&empty).unwrap());

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&empty);
    }

    /// 「覆盖式应用打到含 Windows 的目标卷」判据（用户 2026-10-16 拍板立项，
    /// 2026-10-22 #110 起由硬拒改路由）。四象限全锁：只有「含 Windows +
    /// 取消格式化」命中，其余三格必须放行。
    #[test]
    fn overlay_route_quadrant_is_exact() {
        // 含 Windows + 取消格式化 = 命中（恢复期路由到 wimlib --no-acls）。
        assert!(overlay_restore_on_windows_volume(true, false));
        // 勾选格式化 = DISM 只需「新建」受保护目录（允许），必须放行。
        assert!(!overlay_restore_on_windows_volume(true, true));
        // 数据卷没有 WindowsApps，覆盖式应用合法（2026-10-14 已在 T: 验证）。
        assert!(!overlay_restore_on_windows_volume(false, false));
        assert!(!overlay_restore_on_windows_volume(false, true));
    }
}
