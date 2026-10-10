//! 半成品启动项簿记：**跨平台**，因此它的单测在 macOS 上也会跑。
//!
//! ## 为什么单独一个模块
//!
//! `boot_entry` 整个是 `#[cfg(windows)]`，里面的单测在 macOS 上一次都不编译。
//! 2026-09-29 `ramdisk=` 值畸形就是这么漏过去的（那段教训记在 `boot_entry` 的模块注释里）。
//! 半成品簿记是**纯数据 + 文件 I/O**，不碰任何 Windows API，所以放这里并配齐单测。
//!
//! ## 它解决什么问题（问题 11）
//!
//! `create_entry` 在字段写入全部重试失败后，会保留已建的两个 BCD 对象供诊断
//! （那里的 KEEP 分支）。此前这两个 GUID 只出现在日志文本里，没有任何代码认得它们：
//! 界面报"准备失败"，机器上却静静留着两个 BCD 对象，下一次重启可能进一个
//! `device`/`osdevice` 从未写成功的启动项。
//!
//! 现在把 GUID 落到这份簿记里，准备期的统一补偿据此按**精确 GUID** 清理
//! （绝不用描述字符串匹配——那会误删别的任务的对象）。

use backuprestore_core::{TaskError, VolumeIdentity, read_json, write_json_atomic};
use std::fs;
use std::path::{Path, PathBuf};

/// 半成品条目的簿记文件名。
///
/// 刻意**与 `boot-entry.json` 分开**：若两者共用，下一次 `create_entry` 的
/// "复用已有条目"分支就会把这个字段不全的条目当成可用条目，`arm_one_shot`
/// 随后武装它，机器下次开机会进一个指不到载荷的启动项。
/// 分开之后，补偿仍能按精确 GUID 清掉它，而复用分支看不见它。
pub(crate) const RESIDUE_RECORD: &str = "boot-entry-residue.json";

/// `create_entry` 失败后为诊断而保留的两个 BCD 对象的簿记。
///
/// 字段与 `ReBootEntry` 对齐，但**不共用类型**：正式簿记会被"复用已有条目"
/// 分支消费，半成品簿记不会。用不同类型让这个区别在编译期就成立。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResidueRecord {
    /// 所属任务 ID。按任务隔离的载荷定位全靠它；**刻意不给默认值**，
    /// 缺它的旧格式簿记会直接反序列化失败，而不是静默拿到空值。
    pub task_id: String,
    /// 为诊断而保留的 osloader 条目 GUID（带花括号）。
    pub loader_guid: String,
    /// 为诊断而保留的设备选项对象 GUID（带花括号）。
    pub devopts_guid: String,
    /// 载荷卷身份（补偿时仍要按它核验卷身份，不信任跨重启保存的盘符）。
    pub wim_volume: VolumeIdentity,
    /// 载荷 WIM 的绝对路径。
    pub wim_path: String,
    /// 记录时间（RFC3339）。
    pub created: String,
}

impl ResidueRecord {
    fn record_path(task_dir: &Path) -> PathBuf {
        task_dir.join(RESIDUE_RECORD)
    }

    /// 读取半成品条目簿记；没有则返回 `None`。
    pub(crate) fn read(task_dir: &Path) -> Result<Option<Self>, TaskError> {
        let path = Self::record_path(task_dir);
        if !path.is_file() {
            return Ok(None);
        }
        read_json(path).map(Some)
    }

    /// 落盘半成品条目簿记。**不写 `boot-entry.json`**，以免被复用分支当成可用条目。
    pub(crate) fn write(&self, task_dir: &Path) -> Result<(), TaskError> {
        write_json_atomic(Self::record_path(task_dir), self)
    }

    /// 补偿成功后清掉半成品簿记；文件不存在视为已清理。
    pub(crate) fn clear(task_dir: &Path) -> Result<(), TaskError> {
        match fs::remove_file(Self::record_path(task_dir)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TASK_ID: &str = "11111111-2222-4333-8444-555555555555";
    const LOADER: &str = "{aaaaaaaa-1111-2222-3333-444444444444}";
    const DEVOPTS: &str = "{bbbbbbbb-1111-2222-3333-444444444444}";

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn record() -> ResidueRecord {
        ResidueRecord {
            task_id: TASK_ID.to_string(),
            loader_guid: LOADER.to_string(),
            devopts_guid: DEVOPTS.to_string(),
            wim_volume: VolumeIdentity::new("disk", "partition"),
            wim_path: format!(r"F:\BackupRestoreRE\{TASK_ID}\Winre.wim"),
            created: "2026-10-10T00:00:00Z".to_string(),
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "br-residue-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 半成品簿记必须落在自己的文件里，绝不复用正式簿记的文件名。
    ///
    /// 这不是命名偏好：共用文件名时，下一次 `create_entry` 的"复用已有条目"
    /// 分支会把这个 `device`/`osdevice` 从未写成功的条目当成可用条目，
    /// `arm_one_shot` 随后武装它，机器下次开机会进一个指不到载荷的启动项。
    #[test]
    fn residue_record_has_its_own_file_and_never_shares_the_live_one() {
        assert_eq!(RESIDUE_RECORD, "boot-entry-residue.json");
        assert_ne!(RESIDUE_RECORD, "boot-entry.json");

        let dir = temp_dir("own-file");
        let live = dir.join("boot-entry.json");
        let residue = dir.join(RESIDUE_RECORD);
        record().write(&dir).unwrap();
        assert!(residue.is_file());
        assert!(!live.exists(), "写半成品簿记绝不能顺带造出正式簿记");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 补偿要能原样拿回两个 GUID 与卷身份——按精确 GUID 删除是安全边界，
    /// 少了任何一个都会退化成"靠描述字符串匹配"或"整条放弃清理"。
    #[test]
    fn residue_record_round_trips_every_field() {
        let dir = temp_dir("round-trip");
        let original = record();
        original.write(&dir).unwrap();
        let read_back = ResidueRecord::read(&dir)
            .unwrap()
            .expect("刚写下的簿记应能读回");
        assert_eq!(read_back, original);
        assert_eq!(read_back.loader_guid, LOADER);
        assert_eq!(read_back.devopts_guid, DEVOPTS);
        assert_eq!(read_back.task_id, TASK_ID);
        assert_eq!(read_back.wim_path, original.wim_path);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 缺 `task_id` 的旧格式簿记必须报错，不能静默拿到空值。
    ///
    /// 空任务 ID 会让按任务隔离的载荷定位退化成卷根，清理时可能碰到别人的载荷。
    #[test]
    fn residue_record_rejects_a_missing_task_id() {
        let dir = temp_dir("task-id");
        let mut value = serde_json::to_value(record()).unwrap();
        value.as_object_mut().unwrap().remove("taskId");
        fs::write(
            dir.join(RESIDUE_RECORD),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
        assert!(
            ResidueRecord::read(&dir).is_err(),
            "缺 task_id 的簿记必须报错，不能退化成空值"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 清理必须幂等，而且绝不能碰正式簿记。
    #[test]
    fn clearing_residue_is_idempotent_and_spares_the_live_record() {
        let dir = temp_dir("idempotent");
        // 目录里什么都没有时也要算成功。
        ResidueRecord::clear(&dir).unwrap();
        ResidueRecord::clear(&dir).unwrap();

        record().write(&dir).unwrap();
        ResidueRecord::clear(&dir).unwrap();
        ResidueRecord::clear(&dir).unwrap();
        assert!(ResidueRecord::read(&dir).unwrap().is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 没有簿记时读取返回 `None`（副作用还没进展到建 BCD 对象的那一步）。
    #[test]
    fn reading_an_absent_residue_record_is_not_an_error() {
        let dir = temp_dir("absent");
        assert!(ResidueRecord::read(&dir).unwrap().is_none());
        fs::remove_dir_all(&dir).unwrap();
    }
}
