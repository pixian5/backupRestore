//! 任务生命周期校验与状态流转引擎 (Task Lifecycle Validation & State Transition).
//!
//! 负责任务在联机准备阶段、WinRE 恢复阶段以及第二系统安装等不同场景下的
//! 参数完整性、分区互斥安全性（防止自毁覆盖）、引导模式一致性以及状态机跃迁。

use chrono::Utc;

use crate::capacity_plan::canonical_compression;
use crate::stage::{default_progress, Stage};
use crate::task_spec::{BootMode, TargetRole};
use crate::task_store::{
    require_complete, validate_absolute_path, validate_relative_path, validate_sha256_text,
    validate_task_id,
};
use crate::{Operation, StatusRecord, Task, TaskError, TASK_VERSION};

impl Task {
    /// 全面校验任务配置参数与分区边界互斥安全性。
    ///
    /// 无论联机准备端还是 WinRE 恢复端，都必须通过此校验才能落盘或动盘。
    pub fn validate(&self) -> Result<(), TaskError> {
        if self.version != TASK_VERSION {
            return Err(TaskError::coded(
                "BR-CORE-001",
                format!(
                    "unsupported task version {} (expected {})",
                    self.version, TASK_VERSION
                ),
            ));
        }
        validate_task_id(&self.task_id)?;
        if !self.boot_once {
            return Err(TaskError::coded(
                "BR-CORE-002",
                "recovery tasks must use one-time boot",
            ));
        }
        if self.boot_plan.boot_sequence_requested && self.boot_plan.previous_bcd_sha256.is_none() {
            return Err(TaskError::coded(
                "BR-CORE-003",
                "boot sequence requires a previous BCD hash for rollback",
            ));
        }
        if let Some(hash) = self.boot_plan.previous_bcd_sha256.as_deref() {
            validate_sha256_text("previous BCD", hash)?;
        }
        if let Some(compression) = self.compress.as_deref() {
            canonical_compression(compression)?;
        }
        if let Some(payload) = self.payload.as_ref() {
            if payload.task_id != self.task_id {
                return Err(TaskError::coded(
                    "BR-CORE-004",
                    "payload task id does not match task id",
                ));
            }
            for (name, hash) in [
                ("recovery entry point", payload.recovery_sha256.as_str()),
                ("task", payload.task_sha256.as_str()),
                ("original WinRE", payload.original_winre_sha256.as_str()),
                ("staged WinRE", payload.staged_winre_sha256.as_str()),
            ] {
                validate_sha256_text(name, hash)?;
            }
        }
        // 用户接受例外必须始终绑在本任务的镜像记录上。任务文件被改动时，
        // 这里要让一个「对不上自己镜像」的例外直接失败，而不是变成通用放行。
        if let Some(accepted) = self.image_acceptance.as_ref() {
            let image = self
                .image
                .as_ref()
                .ok_or_else(|| TaskError::coded("BR-CORE-005", "image acceptance has no image"))?;
            if accepted.index != image.index {
                return Err(TaskError::coded(
                    "BR-CORE-006",
                    "image acceptance index does not match the task image index",
                ));
            }
            if accepted.accepted_size_bytes != image.size_bytes {
                return Err(TaskError::coded(
                    "BR-CORE-007",
                    "image acceptance size does not match the task image size",
                ));
            }
            // 空哈希表示"仅大小"模式下的接受，不覆盖哈希判据；非空则必须与任务
            // 镜像哈希一致，否则离线侧会拿一个无人确认过的值当作已授权内容。
            if !accepted.accepted_sha256.is_empty() {
                validate_sha256_text("accepted image", &accepted.accepted_sha256)?;
                if !accepted.accepted_sha256.eq_ignore_ascii_case(&image.sha256) {
                    return Err(TaskError::coded(
                        "BR-CORE-008",
                        "image acceptance hash does not match the task image hash",
                    ));
                }
            }
            if let Some(sidecar) = accepted.superseded_sidecar_sha256.as_deref() {
                validate_sha256_text("superseded sidecar", sidecar)?;
            }
        }
        match self.operation {
            Operation::Probe => {
                let source = self.source.as_ref().ok_or_else(|| missing("source"))?;
                require_complete("probe source", source)?;
                if self.image.is_some() || self.destination.is_some() || self.target.is_some() {
                    return Err(TaskError::coded(
                        "BR-CORE-009",
                        "probe task cannot contain image, destination or target",
                    ));
                }
            }
            Operation::Backup => {
                let source = self.source.as_ref().ok_or_else(|| missing("source"))?;
                let dest = self
                    .destination
                    .as_ref()
                    .ok_or_else(|| missing("destination"))?;
                require_complete("source", source)?;
                require_complete("destination volume", &dest.volume)?;
                if source.same_partition(&dest.volume) {
                    return Err(TaskError::coded(
                        "BR-CORE-010",
                        "backup destination must differ from source partition",
                    ));
                }
                if dest.volume.is_reserved_partition() {
                    return Err(TaskError::coded(
                        "BR-CORE-011",
                        "EFI/MSR/Recovery partitions cannot store backup images",
                    ));
                }
                if let Some(path) = dest.absolute_path.as_deref() {
                    validate_absolute_path(path)?;
                }
                validate_relative_path(&dest.relative_path)?;
                if self.image.is_some() || self.target.is_some() {
                    return Err(TaskError::coded(
                        "BR-CORE-012",
                        "backup task cannot contain image or target",
                    ));
                }
            }
            Operation::RestoreExisting | Operation::CreateSecondary => {
                let source = self.source.as_ref().ok_or_else(|| missing("source"))?;
                let image = self.image.as_ref().ok_or_else(|| missing("image"))?;
                let target = self.target.as_ref().ok_or_else(|| missing("target"))?;
                require_complete("source", source)?;
                require_complete("image volume", &image.volume)?;
                require_complete("target volume", &target.volume)?;
                if source.same_partition(&image.volume) {
                    return Err(TaskError::coded(
                        "BR-CORE-013",
                        "image volume must differ from source volume",
                    ));
                }
                if image.volume.is_reserved_partition() {
                    return Err(TaskError::coded(
                        "BR-CORE-014",
                        "EFI/MSR/Recovery partitions cannot store restore images",
                    ));
                }
                if let Some(path) = image.absolute_path.as_deref() {
                    validate_absolute_path(path)?;
                }
                if image.sha256.len() != 64
                    || !image.sha256.chars().all(|c| c.is_ascii_hexdigit())
                {
                    return Err(TaskError::coded(
                        "BR-CORE-015",
                        "image sha256 must be 64 hex characters",
                    ));
                }
                if image.index == 0 {
                    return Err(TaskError::coded(
                        "BR-CORE-016",
                        "WIM index must be greater than zero",
                    ));
                }
                if image.size_bytes == 0 {
                    return Err(TaskError::coded(
                        "BR-CORE-017",
                        "restore image size must be greater than zero",
                    ));
                }
                if target.minimum_size_bytes == 0 {
                    return Err(TaskError::coded(
                        "BR-CORE-018",
                        "restore target minimum size must be greater than zero",
                    ));
                }
                validate_relative_path(&image.relative_path)?;
                if image.volume.same_partition(&target.volume) {
                    return Err(TaskError::coded(
                        "BR-CORE-019",
                        "image volume must differ from restore target",
                    ));
                }
                if target.volume.is_reserved_partition() {
                    return Err(TaskError::coded(
                        "BR-CORE-020",
                        "EFI/MSR/Recovery partitions cannot be restore targets",
                    ));
                }
                match self.operation {
                    Operation::RestoreExisting => {
                        if !source.same_partition(&target.volume) {
                            return Err(TaskError::coded(
                                "BR-CORE-021",
                                "restore-existing target must be the existing Windows source volume",
                            ));
                        }
                        if target.role != TargetRole::ExistingWindows {
                            return Err(TaskError::coded(
                                "BR-CORE-022",
                                "restore-existing target role must be existing-windows",
                            ));
                        }
                        if !matches!(self.boot_plan.mode, BootMode::ReturnExisting) {
                            return Err(TaskError::coded(
                                "BR-CORE-023",
                                "restore-existing must return to the existing boot entry",
                            ));
                        }
                    }
                    Operation::CreateSecondary => {
                        if source.same_partition(&target.volume) {
                            return Err(TaskError::coded(
                                "BR-CORE-024",
                                "secondary target must differ from the existing Windows source",
                            ));
                        }
                        if target.role != TargetRole::NewWindows {
                            return Err(TaskError::coded(
                                "BR-CORE-025",
                                "create-secondary target role must be new-windows",
                            ));
                        }
                        if target
                            .boot_menu_name
                            .as_deref()
                            .unwrap_or("")
                            .trim()
                            .is_empty()
                        {
                            return Err(TaskError::coded(
                                "BR-CORE-026",
                                "create-secondary requires a boot menu name",
                            ));
                        }
                        if !valid_menu_name(target.boot_menu_name.as_deref().unwrap_or("")) {
                            return Err(TaskError::coded(
                                "BR-CORE-027",
                                "create-secondary boot menu name contains control characters or is too long",
                            ));
                        }
                        if !matches!(self.boot_plan.mode, BootMode::AddSecondary) {
                            return Err(TaskError::coded(
                                "BR-CORE-028",
                                "create-secondary must use add-secondary boot mode",
                            ));
                        }
                        if self
                            .boot_plan
                            .menu_name
                            .as_deref()
                            .unwrap_or("")
                            .trim()
                            .is_empty()
                        {
                            return Err(TaskError::coded(
                                "BR-CORE-029",
                                "create-secondary requires a boot plan menu name",
                            ));
                        }
                        if !valid_menu_name(self.boot_plan.menu_name.as_deref().unwrap_or("")) {
                            return Err(TaskError::coded(
                                "BR-CORE-030",
                                "create-secondary boot plan menu name contains control characters or is too long",
                            ));
                        }
                    }
                    Operation::Probe | Operation::Backup => unreachable!(),
                }
            }
        }
        let workspace_volume = self
            .workspace_volume
            .as_ref()
            .ok_or_else(|| missing("workspace volume"))?;
        require_complete("workspace volume", workspace_volume)?;
        if workspace_volume.is_reserved_partition() {
            return Err(TaskError::coded(
                "BR-CORE-031",
                "EFI/MSR/Recovery partitions cannot store workspace files",
            ));
        }
        if let Some(target) = self.target.as_ref()
            && workspace_volume.same_partition(&target.volume)
        {
            return Err(TaskError::coded(
                "BR-CORE-032",
                "workspace volume must differ from restore target",
            ));
        }
        Ok(())
    }

    /// 按照合法状态机推进当前任务阶段，并生成最新状态流水记录。
    pub fn transition(&mut self, next: Stage) -> Result<StatusRecord, TaskError> {
        if !self.status.can_transition_to(next) {
            return Err(TaskError::InvalidTransition {
                from: self.status,
                to: next,
            });
        }
        self.status = next;
        Ok(StatusRecord {
            task_id: self.task_id.clone(),
            operation: self.operation,
            stage: next,
            progress: default_progress(next),
            updated: Utc::now(),
            error_code: None,
            error_code_text: None,
            error: None,
        })
    }
}

pub fn missing(name: &str) -> TaskError {
    TaskError::coded("BR-CORE-033", format!("missing {name}"))
}

pub fn valid_menu_name(value: &str) -> bool {
    value.chars().count() <= 256 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capacity_plan::{ensure_capacity, validate_operation};
    use crate::task_spec::{BackupMetadata, BootPlan, DestinationSpec, ImageAcceptance, ImageSpec, TargetSpec};
    use crate::volume_role::VolumeIdentity;

    fn identity(value: &str, size: u64) -> VolumeIdentity {
        let mut i = VolumeIdentity::new("disk", value);
        i.volume_guid = format!("volume-{value}");
        i.partition_type_guid = "{00000000-0000-0000-0000-000000000000}".into();
        i.disk_number = Some(0);
        i.partition_number = Some(1);
        i.partition_size = size;
        i.filesystem = "NTFS".into();
        i
    }

    fn backup_task() -> Task {
        let mut task = Task::new(
            Operation::Backup,
            BootPlan {
                mode: BootMode::ReturnExisting,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: None,
                boot_sequence_requested: true,
            },
        );
        task.source = Some(identity("source", 100));
        task.workspace_volume = Some(identity("task", 100));
        task.destination = Some(DestinationSpec {
            volume: identity("image", 100),
            absolute_path: None,
            relative_path: "MyBackup/Windows.wim".into(),
        });
        task
    }

    fn restore_task_with_image(sha256: &str, size_bytes: u64, index: u32) -> Task {
        let mut task = Task::new(
            Operation::RestoreExisting,
            BootPlan {
                mode: BootMode::ReturnExisting,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: None,
                boot_sequence_requested: true,
            },
        );
        task.source = Some(identity("source", 200));
        task.workspace_volume = Some(identity("workspace", 200));
        task.image = Some(ImageSpec {
            volume: identity("image", 400),
            absolute_path: None,
            relative_path: "backup.wim".into(),
            sha256: sha256.into(),
            size_bytes,
            index,
            name: None,
        });
        task.target = Some(TargetSpec {
            volume: identity("source", 200),
            role: TargetRole::ExistingWindows,
            boot_menu_name: None,
            minimum_size_bytes: 100,
            is_active_system: true,
            format_target: true,
            overlay_no_acl: false,
        });
        task.verify_hash = Some(true);
        task
    }

    #[test]
    fn backup_rejects_same_partition() {
        let mut task = backup_task();
        task.destination.as_mut().unwrap().volume.partition_guid = "source".into();
        assert!(task.validate().is_err());
    }

    #[test]
    fn task_validation_checks_compression() {
        let mut task = backup_task();
        task.compress = Some("fast".into());
        assert!(task.validate().is_ok());
        task.compress = Some("none".into());
        assert!(task.validate().is_ok());
        task.compress = Some("max".into());
        assert!(task.validate().is_err());
        task.compress = Some("unknown".into());
        assert!(task.validate().is_err());
    }

    #[test]
    fn arbitrary_drive_letters_are_temporary_hints() {
        let mut task = backup_task();
        task.source.as_mut().unwrap().drive_letter = Some('S');
        task.workspace_volume.as_mut().unwrap().drive_letter = Some('T');
        task.destination.as_mut().unwrap().volume.drive_letter = Some('B');
        assert!(task.validate().is_ok());

        let mut moved = task.clone();
        moved.source.as_mut().unwrap().drive_letter = Some('D');
        moved.destination.as_mut().unwrap().volume.drive_letter = Some('I');
        assert!(moved.validate().is_ok());
        assert!(
            task.source
                .as_ref()
                .unwrap()
                .same_partition(moved.source.as_ref().unwrap())
        );
    }

    #[test]
    fn workspace_volume_rejects_reserved_and_target_overlap() {
        let mut task = backup_task();
        task.workspace_volume.as_mut().unwrap().partition_type_guid =
            "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}".into();
        assert!(task.validate().is_err());

        let mut task = backup_task();
        task.workspace_volume.as_mut().unwrap().partition_guid = "source".into();
        assert!(task.validate().is_ok());

        let mut task = backup_task();
        task.workspace_volume.as_mut().unwrap().partition_guid = "image".into();
        assert!(task.validate().is_ok());

        let mut task = Task::new(
            Operation::RestoreExisting,
            BootPlan {
                mode: BootMode::ReturnExisting,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: None,
                boot_sequence_requested: true,
            },
        );
        task.source = Some(identity("source", 200));
        task.workspace_volume = Some(identity("source", 200));
        task.image = Some(ImageSpec {
            volume: identity("image", 100),
            absolute_path: None,
            relative_path: "backup.wim".into(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            index: 1,
            name: None,
        });
        task.target = Some(TargetSpec {
            volume: identity("source", 200),
            role: TargetRole::ExistingWindows,
            boot_menu_name: None,
            minimum_size_bytes: 100,
            is_active_system: true,
            format_target: true,
            overlay_no_acl: false,
        });
        assert!(task.validate().is_err());
    }

    #[test]
    fn accepted_image_survives_the_reboot_and_still_detects_later_tampering() {
        let actual = "b".repeat(64);
        let mut task = restore_task_with_image(&actual, 4096, 2);
        task.image_acceptance = Some(ImageAcceptance {
            accepted_sha256: actual.clone(),
            accepted_size_bytes: 4096,
            index: 2,
            superseded_sidecar_sha256: Some("c".repeat(64)),
            accepted_at: Utc::now(),
        });
        task.validate().unwrap();
        let accepted = task.image_acceptance.as_ref().unwrap();
        assert!(accepted.covers(2, &actual, 4096));
        assert!(!accepted.covers(2, &"d".repeat(64), 4096));
        assert!(!accepted.covers(2, &actual, 4097));
        assert!(!accepted.covers(1, &actual, 4096));
    }

    #[test]
    fn image_acceptance_must_match_the_task_image_record() {
        let actual = "b".repeat(64);
        let base = restore_task_with_image(&actual, 4096, 2);
        let accepted = ImageAcceptance {
            accepted_sha256: actual.clone(),
            accepted_size_bytes: 4096,
            index: 2,
            superseded_sidecar_sha256: None,
            accepted_at: Utc::now(),
        };

        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            index: 1,
            ..accepted.clone()
        });
        assert!(task.validate().is_err());

        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            accepted_size_bytes: 8192,
            ..accepted.clone()
        });
        assert!(task.validate().is_err());

        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            accepted_sha256: "e".repeat(64),
            ..accepted.clone()
        });
        assert!(task.validate().is_err());

        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            accepted_sha256: "not-a-hash".into(),
            ..accepted.clone()
        });
        assert!(task.validate().is_err());

        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            superseded_sidecar_sha256: Some("zz".into()),
            ..accepted.clone()
        });
        assert!(task.validate().is_err());

        let mut task = base.clone();
        task.image_acceptance = Some(ImageAcceptance {
            accepted_sha256: String::new(),
            ..accepted.clone()
        });
        task.validate().unwrap();
        assert!(!task.image_acceptance.as_ref().unwrap().covers(2, &actual, 4096));

        let mut task = backup_task();
        task.image_acceptance = Some(accepted);
        assert!(task.validate().is_err());
    }

    #[test]
    fn task_rejects_incomplete_workspace_identity() {
        let mut task = backup_task();
        task.workspace_volume
            .as_mut()
            .unwrap()
            .partition_type_guid
            .clear();
        assert!(task.validate().is_err());
        task.workspace_volume.as_mut().unwrap().partition_type_guid =
            "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}".into();
        task.workspace_volume.as_mut().unwrap().volume_guid.clear();
        assert!(task.validate().is_err());
    }

    #[test]
    fn menu_name_rejects_controls_and_unbounded_input() {
        assert!(valid_menu_name("Windows Backup"));
        assert!(!valid_menu_name("Windows\nBackup"));
        assert!(!valid_menu_name(&"x".repeat(257)));
    }

    #[test]
    fn reserved_volumes_and_dot_paths_are_rejected() {
        let mut backup = backup_task();
        backup
            .destination
            .as_mut()
            .unwrap()
            .volume
            .partition_type_guid = "{de94bba4-06d1-4d40-a16a-bfd50179d6ac}".into();
        assert!(backup.validate().is_err());
        assert!(validate_relative_path(".").is_err());
        assert!(validate_relative_path(".\\Windows.wim").is_err());

        let source = identity("source", 100);
        let mut image = identity("image", 100);
        image.partition_type_guid = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}".into();
        let target = identity("source", 100);
        let mut task = Task::new(
            Operation::RestoreExisting,
            BootPlan {
                mode: BootMode::ReturnExisting,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: None,
                boot_sequence_requested: true,
            },
        );
        task.source = Some(source);
        task.workspace_volume = Some(identity("workspace", 200));
        task.image = Some(ImageSpec {
            volume: image,
            absolute_path: None,
            relative_path: "backup.wim".into(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            index: 1,
            name: None,
        });
        task.target = Some(TargetSpec {
            volume: target,
            role: TargetRole::ExistingWindows,
            boot_menu_name: None,
            minimum_size_bytes: 100,
            is_active_system: true,
            format_target: true,
            overlay_no_acl: false,
        });
        assert!(task.validate().is_err());
    }

    #[test]
    fn absolute_image_paths_require_a_drive_root() {
        assert!(validate_absolute_path(r"B:\Backups\Windows.wim").is_ok());
        assert!(validate_absolute_path(r"B:/Backups/Windows.wim").is_ok());
        assert!(validate_absolute_path(r"Backups\Windows.wim").is_err());
        assert!(validate_absolute_path(r"B:\Backups\..\Windows.wim").is_err());
        assert!(validate_absolute_path(r"B:\").is_err());

        let mut task = backup_task();
        task.destination.as_mut().unwrap().absolute_path = Some("backup.wim".into());
        assert!(task.validate().is_err());
        task.destination.as_mut().unwrap().absolute_path = Some(r"B:\Backups\Windows.wim".into());
        assert!(task.validate().is_ok());
    }

    #[test]
    fn restore_rejects_image_on_target_and_reserved_target() {
        let source = identity("source", 100);
        let image = identity("image", 100);
        assert!(validate_operation(Operation::RestoreExisting, &source, &image, &image).is_err());
        let mut efi = identity("efi", 100);
        efi.partition_type_guid = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}".into();
        assert!(validate_operation(Operation::RestoreExisting, &source, &image, &efi).is_err());
    }

    #[test]
    fn secondary_requires_explicit_name_and_mode() {
        let mut task = Task::new(
            Operation::CreateSecondary,
            BootPlan {
                mode: BootMode::AddSecondary,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: Some("Windows 11 Backup".into()),
                boot_sequence_requested: true,
            },
        );
        task.image = Some(ImageSpec {
            volume: identity("image", 100),
            absolute_path: None,
            relative_path: "backup.wim".into(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            index: 1,
            name: None,
        });
        task.target = Some(TargetSpec {
            volume: identity("target", 200),
            role: TargetRole::NewWindows,
            boot_menu_name: Some("Windows 11 Backup".into()),
            minimum_size_bytes: 100,
            is_active_system: false,
            format_target: true,
            overlay_no_acl: false,
        });
        task.source = Some(identity("source", 200));
        task.workspace_volume = Some(identity("workspace", 200));
        assert!(task.validate().is_ok());
        task.target.as_mut().unwrap().boot_menu_name = None;
        assert!(task.validate().is_err());
    }

    #[test]
    fn restore_requires_source_size_and_target_capacity() {
        let mut task = Task::new(
            Operation::RestoreExisting,
            BootPlan {
                mode: BootMode::ReturnExisting,
                previous_bcd_sha256: Some("a".repeat(64)),
                menu_name: None,
                boot_sequence_requested: true,
            },
        );
        task.source = Some(identity("source", 200));
        task.workspace_volume = Some(identity("workspace", 200));
        task.image = Some(ImageSpec {
            volume: identity("image", 100),
            absolute_path: None,
            relative_path: "backup.wim".into(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            index: 1,
            name: None,
        });
        task.target = Some(TargetSpec {
            volume: identity("source", 200),
            role: TargetRole::ExistingWindows,
            boot_menu_name: None,
            minimum_size_bytes: 100,
            is_active_system: true,
            format_target: true,
            overlay_no_acl: false,
        });
        assert!(task.validate().is_ok());
        task.source = None;
        assert!(task.validate().is_err());
        task.source = Some(identity("source", 200));
        task.image.as_mut().unwrap().size_bytes = 0;
        assert!(task.validate().is_err());
        task.image.as_mut().unwrap().size_bytes = 1;
        task.target.as_mut().unwrap().minimum_size_bytes = 0;
        assert!(task.validate().is_err());
    }

    #[test]
    fn path_and_capacity_guards() {
        assert!(validate_relative_path("x\\y\\z.wim").is_ok());
        assert!(validate_relative_path("..\\evil.wim").is_err());
        let metadata = BackupMetadata {
            version: 1,
            image_type: "wim".into(),
            created: Utc::now(),
            started: None,
            duration_secs: None,
            bytes_per_sec: None,
            computer: "test".into(),
            windows_edition: "Pro".into(),
            architecture: "amd64".into(),
            windows_build: "1".into(),
            wim_index: 1,
            image_sha256: "a".repeat(64),
            image_size: 1,
            source: identity("source", 100),
            captured_used_bytes: 80,
            reserved_bytes: 20,
            minimum_target_size: 90,
            volume_serial: "x".into(),
            program_version: "0.0.0".into(),
            image_name: None,
        };
        assert_eq!(metadata.required_target_size(), 100);
        assert!(ensure_capacity(99, &metadata).is_err());
        assert!(ensure_capacity(100, &metadata).is_ok());
    }

    #[test]
    fn task_transition_lifecycle() {
        let mut task = backup_task();
        assert_eq!(task.status, Stage::Prepared);

        let rec = task.transition(Stage::BootRequested).unwrap();
        assert_eq!(rec.stage, Stage::BootRequested);
        assert_eq!(task.status, Stage::BootRequested);

        // 非法状态跃迁应返回 InvalidTransition 错误
        let err = task.transition(Stage::RecoveryComplete).unwrap_err();
        assert!(matches!(err, TaskError::InvalidTransition { .. }));
        assert_eq!(err.code(), Some("BR-TSK-028"));
    }
}
