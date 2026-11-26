//! 图形界面任务派发纯决策与命令行参数规划模块 (`gui_dispatch_plan.rs`)。
//!
//! 从 `native_gui/task_executor.rs` 提取纯 Rust 规划逻辑，无 Windows API 依赖，
//! 使得 macOS、Linux CI 与 Windows 上均可通过单元测试完整覆盖 A/B/C/备份 × 在线/离线 × 格式化矩阵。

#![cfg_attr(not(windows), allow(dead_code))]

/// GUI 入口键 → CLI `prepare --operation` 值。
/// B 复用 `restore-existing`；C 复用 `create-secondary`；其余入口原样保留。
pub(crate) fn cli_operation(gui: &str) -> &'static str {
    match gui {
        "restore-data" => "restore-existing",
        "create-secondary" => "create-secondary",
        "backup" => "backup",
        "restore-existing" => "restore-existing",
        "install-pe-entry" => "install-pe-entry",
        "probe" => "probe",
        _ => "backup",
    }
}

/// 是否「镜像还原类」入口（A 还原系统到 C / B 还原数据到非 C / C 还原系统到非 C）。
pub(crate) fn is_image_restore_entry(gui: &str) -> bool {
    matches!(
        gui,
        "restore-existing" | "restore-data" | "create-secondary"
    )
}

/// 统一把盘符规格（例如 `"C"` / `"c:"` / `"C:\\"`）规范成单个大写盘符字母串（`"C"`）。
pub(crate) fn normalize_drive_upper(drive: &str) -> String {
    drive
        .trim()
        .trim_end_matches('\\')
        .trim_end_matches(':')
        .to_ascii_uppercase()
}

/// 只有三个还原入口且取消勾选「格式化目标分区」时才需要给 `prepare` 传 `--skip-format`。
pub(crate) fn should_append_skip_format(operation: &str, format_target: bool) -> bool {
    matches!(
        operation,
        "restore-existing" | "restore-data" | "create-secondary"
    ) && !format_target
}

/// GUI 任务派发分流输入。
pub(crate) struct DispatchInputs<'a> {
    pub operation: &'a str,
    pub source_drive: &'a str,
    pub target_drive: &'a str,
    pub system_drive: &'a str,
    pub channel_online: bool,
    pub format_target: bool,
    pub image_name: &'a str,
    pub boot_menu_name: &'a str,
}

/// GUI 任务校验通过后的高层次分流结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DispatchRoute {
    /// 目标或源为当前活动系统，需弹出 PE / WinRE / 取消 选择框。
    SystemDriveChoiceRequired,
    /// 数据卷还原选了离线通道，需弹出 PE / WinRE / 取消 选择框。
    OfflineDataChoiceRequired { format_target: bool },
    /// 无需弹 PE/RE 选择框，直接下发提权 `prepare`（例如离线 C 入口或 `probe`）。
    PrepareDirect,
    /// 走在线同步通道，不重启进恢复环境。
    Online {
        cli_op: &'static str,
        affected_upper: String,
        system_upper: String,
        format_target: bool,
        online_image_name: String,
    },
}

/// 计算 GUI 创建任务后的执行通道分流。
pub(crate) fn plan_gui_dispatch(inputs: &DispatchInputs<'_>) -> DispatchRoute {
    let system_upper = normalize_drive_upper(inputs.system_drive);
    if matches!(inputs.operation, "backup" | "restore-existing") {
        let affected = if inputs.operation == "backup" {
            inputs.source_drive
        } else {
            inputs.target_drive
        };
        let affected_upper = normalize_drive_upper(affected);
        let needs_offline = if inputs.operation == "backup" {
            affected_upper == system_upper
        } else {
            true
        };
        if needs_offline {
            return DispatchRoute::SystemDriveChoiceRequired;
        }
        return DispatchRoute::Online {
            cli_op: inputs.operation.into_online_op(),
            affected_upper,
            system_upper,
            format_target: false,
            online_image_name: inputs.image_name.trim().to_string(),
        };
    }

    if matches!(inputs.operation, "restore-data" | "create-secondary") {
        if inputs.channel_online {
            let online_image_name = if inputs.operation == "create-secondary" {
                inputs.boot_menu_name.trim().to_string()
            } else {
                String::new()
            };
            return DispatchRoute::Online {
                cli_op: cli_operation(inputs.operation),
                affected_upper: normalize_drive_upper(inputs.target_drive),
                system_upper,
                format_target: inputs.format_target,
                online_image_name,
            };
        }
        if inputs.operation == "restore-data" {
            return DispatchRoute::OfflineDataChoiceRequired {
                format_target: inputs.format_target,
            };
        }
    }

    DispatchRoute::PrepareDirect
}

trait OnlineOpExt {
    fn into_online_op(self) -> &'static str;
}

impl OnlineOpExt for &str {
    fn into_online_op(self) -> &'static str {
        cli_operation(self)
    }
}

/// 构造提权 `prepare` 子命令参数向量所需的全部输入。
pub(crate) struct PrepareArgInputs<'a> {
    pub operation: &'a str,
    pub source_drive: &'a str,
    pub target_drive: &'a str,
    pub boot_menu_name: &'a str,
    pub image_path: &'a str,
    pub wim_index: &'a str,
    pub compress: &'a str,
    pub image_name: &'a str,
    pub keep_indexes: Option<u32>,
    pub force_restore_hash: bool,
    pub verify_hash: bool,
    pub format_target: bool,
    pub fail_threshold: u32,
    pub fail_action_env: &'a str,
    pub fail_pause_max_wait_secs: u32,
    pub dism_timeout_secs: u32,
    pub english: bool,
}

/// 为提权 `prepare` 进程构造参数列表（不含程序路径第 0 项）。
pub(crate) fn build_prepare_arguments(inputs: &PrepareArgInputs<'_>) -> Vec<String> {
    let mut arguments = vec![
        "prepare".to_string(),
        "--operation".to_string(),
        cli_operation(inputs.operation).to_string(),
        "--source-drive".to_string(),
        inputs.source_drive.to_string(),
        "--target-drive".to_string(),
        inputs.target_drive.to_string(),
        "--boot-menu-name".to_string(),
        inputs.boot_menu_name.to_string(),
    ];
    if inputs.operation != "probe" {
        arguments.extend([
            "--image-path".to_string(),
            inputs.image_path.to_string(),
            "--wim-index".to_string(),
            inputs.wim_index.to_string(),
        ]);
    }
    if inputs.operation == "backup" {
        arguments.extend(["--compress".to_string(), inputs.compress.to_string()]);
        arguments.extend(["--image-name".to_string(), inputs.image_name.to_string()]);
        if let Some(keep) = inputs.keep_indexes {
            arguments.extend(["--keep-indexes".to_string(), keep.to_string()]);
        }
    }
    if is_image_restore_entry(inputs.operation) {
        arguments.push("--allow-destructive".to_string());
        if inputs.force_restore_hash {
            arguments.push("--force-restore-hash".to_string());
        }
        if inputs.verify_hash {
            arguments.push("--verify-hash".to_string());
        }
        if should_append_skip_format(inputs.operation, inputs.format_target) {
            arguments.push("--skip-format".to_string());
        }
    }
    if inputs.operation == "probe" {
        arguments.push("--no-reboot".to_string());
    }
    arguments.extend([
        "--fail-threshold".to_string(),
        inputs.fail_threshold.to_string(),
        "--fail-action".to_string(),
        inputs.fail_action_env.to_string(),
        "--fail-pause-max-wait-secs".to_string(),
        inputs.fail_pause_max_wait_secs.to_string(),
        "--dism-timeout-secs".to_string(),
        inputs.dism_timeout_secs.to_string(),
        "--ui-language".to_string(),
        if inputs.english {
            "en".to_string()
        } else {
            "zh".to_string()
        },
    ]);
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_dispatch(operation: &str) -> DispatchInputs<'_> {
        DispatchInputs {
            operation,
            source_drive: "C:",
            target_drive: "F:",
            system_drive: "C:",
            channel_online: true,
            format_target: true,
            image_name: "win11-img",
            boot_menu_name: "Second Win11",
        }
    }

    #[test]
    fn operation_mapping_and_restore_entry_classification() {
        assert_eq!(cli_operation("restore-data"), "restore-existing");
        assert_eq!(cli_operation("create-secondary"), "create-secondary");
        assert_eq!(cli_operation("restore-existing"), "restore-existing");
        assert_eq!(cli_operation("backup"), "backup");
        assert_eq!(cli_operation("probe"), "probe");
        assert!(is_image_restore_entry("restore-existing"));
        assert!(is_image_restore_entry("restore-data"));
        assert!(is_image_restore_entry("create-secondary"));
        assert!(!is_image_restore_entry("backup"));
        assert!(!is_image_restore_entry("install-pe-entry"));
        assert!(!is_image_restore_entry("probe"));
    }

    #[test]
    fn dispatch_routes_all_entries_and_channels_consistently() {
        // 备份活动系统盘（兼容带冒号/反斜杠写法）必须进离线选择框。
        let mut sys_backup = base_dispatch("backup");
        sys_backup.source_drive = "c:\\";
        assert_eq!(
            plan_gui_dispatch(&sys_backup),
            DispatchRoute::SystemDriveChoiceRequired
        );

        // 备份非系统盘必须分流到在线通道。
        let mut data_backup = base_dispatch("backup");
        data_backup.source_drive = "F:";
        data_backup.image_name = " data-snap ";
        assert_eq!(
            plan_gui_dispatch(&data_backup),
            DispatchRoute::Online {
                cli_op: "backup",
                affected_upper: "F".into(),
                system_upper: "C".into(),
                format_target: false,
                online_image_name: "data-snap".into(),
            }
        );

        // A 入口锁定系统盘、始终需要 PE/RE 选择框。
        assert_eq!(
            plan_gui_dispatch(&base_dispatch("restore-existing")),
            DispatchRoute::SystemDriveChoiceRequired
        );

        // B 入口：在线保留 format_target，且不携带启动菜单名。
        let mut b_online = base_dispatch("restore-data");
        b_online.channel_online = true;
        b_online.format_target = false;
        assert_eq!(
            plan_gui_dispatch(&b_online),
            DispatchRoute::Online {
                cli_op: "restore-existing",
                affected_upper: "F".into(),
                system_upper: "C".into(),
                format_target: false,
                online_image_name: String::new(),
            }
        );

        // B 入口离线：带 format_target 语义进离线选择框。
        let mut b_offline = b_online;
        b_offline.channel_online = false;
        assert_eq!(
            plan_gui_dispatch(&b_offline),
            DispatchRoute::OfflineDataChoiceRequired {
                format_target: false
            }
        );

        // C 入口在线与离线：在线传递菜单名，离线直接进 prepare。
        let mut c_online = base_dispatch("create-secondary");
        c_online.boot_menu_name = " Windows 11 Secondary ";
        assert_eq!(
            plan_gui_dispatch(&c_online),
            DispatchRoute::Online {
                cli_op: "create-secondary",
                affected_upper: "F".into(),
                system_upper: "C".into(),
                format_target: true,
                online_image_name: "Windows 11 Secondary".into(),
            }
        );

        let mut c_offline = base_dispatch("create-secondary");
        c_offline.channel_online = false;
        assert_eq!(
            plan_gui_dispatch(&c_offline),
            DispatchRoute::PrepareDirect
        );

        assert_eq!(
            plan_gui_dispatch(&base_dispatch("probe")),
            DispatchRoute::PrepareDirect
        );
    }

    #[test]
    fn build_prepare_arguments_covers_backup_skip_format_and_probe() {
        let backup_args = build_prepare_arguments(&PrepareArgInputs {
            operation: "backup",
            source_drive: "C:",
            target_drive: "H:",
            boot_menu_name: "",
            image_path: r"H:\BRImages\win.wim",
            wim_index: "1",
            compress: "fast",
            image_name: "sys-1",
            keep_indexes: Some(3),
            force_restore_hash: false,
            verify_hash: true,
            format_target: false,
            fail_threshold: 5,
            fail_action_env: "pause",
            fail_pause_max_wait_secs: 600,
            dism_timeout_secs: 1800,
            english: true,
        });
        assert!(backup_args.windows(2).any(|w| w == ["--operation", "backup"]));
        assert!(backup_args.windows(2).any(|w| w == ["--compress", "fast"]));
        assert!(backup_args.windows(2).any(|w| w == ["--keep-indexes", "3"]));
        assert!(backup_args.windows(2).any(|w| w == ["--ui-language", "en"]));
        assert!(!backup_args.iter().any(|a| a == "--allow-destructive"));
        assert!(!backup_args.iter().any(|a| a == "--skip-format"));

        let data_overlay_args = build_prepare_arguments(&PrepareArgInputs {
            operation: "restore-data",
            source_drive: "F:",
            target_drive: "F:",
            boot_menu_name: "",
            image_path: r"H:\BRImages\data.wim",
            wim_index: "2",
            compress: "",
            image_name: "",
            keep_indexes: None,
            force_restore_hash: true,
            verify_hash: true,
            format_target: false,
            fail_threshold: 3,
            fail_action_env: "auto-escape",
            fail_pause_max_wait_secs: 0,
            dism_timeout_secs: 0,
            english: false,
        });
        assert!(data_overlay_args.windows(2).any(|w| w == ["--operation", "restore-existing"]));
        assert!(data_overlay_args.iter().any(|a| a == "--allow-destructive"));
        assert!(data_overlay_args.iter().any(|a| a == "--force-restore-hash"));
        assert!(data_overlay_args.iter().any(|a| a == "--verify-hash"));
        assert!(data_overlay_args.iter().any(|a| a == "--skip-format"));
        assert!(data_overlay_args.windows(2).any(|w| w == ["--ui-language", "zh"]));

        let secondary_format_args = build_prepare_arguments(&PrepareArgInputs {
            operation: "create-secondary",
            source_drive: "C:",
            target_drive: "E:",
            boot_menu_name: "AltOS",
            image_path: r"H:\BRImages\win.wim",
            wim_index: "1",
            compress: "",
            image_name: "",
            keep_indexes: None,
            force_restore_hash: false,
            verify_hash: false,
            format_target: true,
            fail_threshold: 3,
            fail_action_env: "auto-escape",
            fail_pause_max_wait_secs: 0,
            dism_timeout_secs: 0,
            english: false,
        });
        assert!(secondary_format_args.windows(2).any(|w| w == ["--operation", "create-secondary"]));
        assert!(secondary_format_args.iter().any(|a| a == "--allow-destructive"));
        assert!(!secondary_format_args.iter().any(|a| a == "--skip-format"));

        let probe_args = build_prepare_arguments(&PrepareArgInputs {
            operation: "probe",
            source_drive: "C:",
            target_drive: "H:",
            boot_menu_name: "",
            image_path: "",
            wim_index: "",
            compress: "",
            image_name: "",
            keep_indexes: None,
            force_restore_hash: false,
            verify_hash: false,
            format_target: true,
            fail_threshold: 3,
            fail_action_env: "auto-escape",
            fail_pause_max_wait_secs: 0,
            dism_timeout_secs: 0,
            english: false,
        });
        assert!(probe_args.iter().any(|a| a == "--no-reboot"));
        assert!(!probe_args.iter().any(|a| a == "--image-path"));
    }
}
