//! 任务创建、在线/离线调度与生命周期执行器
//!
//! 负责：
//! - 任务创建入口前置校验与交互确认（）；
//! - 在线备份/还原后台线程派发与互斥门禁（, , ）；
//! - 离线/PE 任务调度与一次性引导配置（）；
//! - 待续跑任务检测、弹窗决策与待重启任务取消（, ）；
//! - 独立模态提示窗口（）。

use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use super::*;
/// 读取最近一次 launcher-errors.log 里的错误信息（用于 prepare 异常退出感知）。
/// 与写方（`main::record_launch_error` / `windows_prepare` 的 UAC 前置）
/// 同源：都走 `log_root::logs_dir()`。写的是 CLI 进程、读的是 GUI 进程，
/// 两边各解析一次——策略确定才拿得到同一份文件，否则 GUI 读到空气。
fn read_latest_launcher_error() -> Option<String> {
    let log_path = crate::log_root::logs_dir().join("launcher-errors.log");
    let content = std::fs::read_to_string(log_path).ok()?;
    content
        .lines()
        .rev()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(|s| s.to_string())
}

/// 返回 `Some(IDYES)` 表示用户明确要求清理未完成任务；返回 `Some(IDNO)` 或
/// `None` 表示不能/不应继续。只读扫描失败也阻断，不猜测当前是否安全。
unsafe fn incomplete_task_choice(
    state: &State,
    language: Language,
) -> Option<(i32, backuprestore_core::Task)> {
    let store = backuprestore_core::TaskStore::new(&state.executable_dir);
    let tasks = match store.incomplete_tasks() {
        Ok(tasks) => tasks,
        Err(error) => {
            append_gui_log(state, &format!("incomplete task scan failed: {error}"));
            show_message(
                state.root,
                &format!("无法检查已有未完成任务：{error}"),
                if language == Language::English {
                    "Cannot inspect tasks"
                } else {
                    "无法检查任务"
                },
                MB_OK | MB_ICONERROR,
            );
            return None;
        }
    };
    if tasks.is_empty() {
        return None;
    }
    let details = tasks
        .iter()
        .map(|task| {
            let target = task
                .target
                .as_ref()
                .map(|spec| spec.volume.partition_guid.as_str())
                .unwrap_or("(无目标卷)");
            format!(
                "{} / {:?} / {:?} / target={target}",
                task.task_id, task.operation, task.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    append_gui_log(
        state,
        &format!(
            "incomplete task scan: count={} details={details}",
            tasks.len()
        ),
    );
    let text = if language == Language::English {
        format!(
            "An unfinished recovery task already exists:\n\n{details}\n\nYes = resume the oldest task, No = abandon and clean it (only before target writes), Cancel = do not create a new task."
        )
    } else {
        format!(
            "检测到尚未终结的任务：\n\n{details}\n\n选择“是”继续最早的旧任务；选择“否”放弃并清理（仅限未写入目标卷的任务）；选择“取消”不创建新任务。"
        )
    };
    let title = if language == Language::English {
        "Unfinished task exists"
    } else {
        "存在未完成任务"
    };
    let Some(task) = tasks.into_iter().next() else {
        return None;
    };
    Some((
        show_message(
            state.root,
            &format!("{text}\n\n是=继续最早的旧任务；否=放弃并清理；取消=不创建新任务。"),
            title,
            MB_YESNOCANCEL | MB_ICONWARNING,
        ),
        task,
    ))
}

/// 清理用户明确放弃的未完成任务：只删除任务目录里的恢复工作树，并将任务标记为
/// Failed 保留诊断记录。若任务已经进入破坏性阶段，拒绝 GUI 直接清理——这种任务
/// 必须先走恢复/续跑，避免把“目标已擦除但还没应用镜像”的材料删掉。
unsafe fn cleanup_incomplete_tasks_from_gui(
    state: &State,
    language: Language,
) -> Result<(), String> {
    let store = backuprestore_core::TaskStore::new(&state.executable_dir);
    let tasks = store
        .incomplete_tasks()
        .map_err(|error| error.to_string())?;
    // 先完整预检，避免多个任务中后面的危险任务导致前面的任务已经被部分清理。
    if let Some(task) = tasks
        .iter()
        .find(|task| !task.status.can_abandon_before_target_write())
    {
        return Err(format!(
            "任务 {} 已进入 {:?} 阶段，可能已修改目标卷；不能从新任务对话框直接清理。请先从恢复入口续跑或人工处理。",
            task.task_id, task.status
        ));
    }
    for task in tasks {
        let dir = store
            .task_dir(&task.task_id)
            .map_err(|error| error.to_string())?;
        let log = dir.join("recovery.log");
        append_log(
            &log,
            &format!(
                "User explicitly abandoned task from new-task dialog; operation={:?} stage={:?}",
                task.operation, task.status
            ),
        )
        .map_err(|error| error.to_string())?;
        let mut failed = task.clone();
        store
            .write_failure(&mut failed, 2, "用户在创建新任务前明确放弃旧任务")
            .map_err(|error| error.to_string())?;
        store
            .cleanup_task_artifacts(&failed.task_id)
            .map_err(|error| error.to_string())?;
        append_gui_log(
            state,
            &format!(
                "incomplete task abandoned and artifacts cleaned: {}",
                failed.task_id
            ),
        );
    }
    let _ = language;
    Ok(())
}

/// 启动一个需要管理员权限的旧任务续跑；`resume` 会验证载荷、重武装一次性
/// 启动项，然后把同一任务送回 WinRE。这里只允许用户明确选择了某个 task ID。
unsafe fn launch_explicit_resume(
    state: &State,
    task: &backuprestore_core::Task,
    language: Language,
) -> bool {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            append_gui_log(state, &format!("resume launch failed: {error}"));
            return false;
        }
    };
    let args = [
        "resume".to_string(),
        state.executable_dir.to_string_lossy().into_owned(),
        task.task_id.clone(),
    ];
    let params = args
        .iter()
        .map(|argument| crate::text_parsing::quote_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let executable_wide = wide(&executable.to_string_lossy());
    let verb = wide("runas");
    let params_wide = wide(&params);
    let mut info = ShellExecuteInfoW {
        cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
        f_mask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: state.root,
        lp_verb: verb.as_ptr(),
        lp_file: executable_wide.as_ptr(),
        lp_parameters: params_wide.as_ptr(),
        lp_directory: null(),
        n_show: SW_SHOWNORMAL,
        h_inst_app: null_mut(),
        lp_id_list: null_mut(),
        lp_class: null(),
        hkey_class: null_mut(),
        dw_hot_key: 0,
        h_icon: null_mut(),
        h_process: null_mut(),
    };
    if ShellExecuteExW(&mut info) == 0 {
        append_gui_log(
            state,
            &format!("resume launch failed: Windows error {}", GetLastError()),
        );
        return false;
    }
    if !info.h_process.is_null() {
        let wait = WaitForSingleObject(info.h_process, 1500);
        if wait == 0 {
            let mut code = 0_u32;
            GetExitCodeProcess(info.h_process, &mut code);
            CloseHandle(info.h_process);
            if code != 0 {
                append_gui_log(state, &format!("resume launch exited early: code={code}"));
                return false;
            }
        } else {
            CloseHandle(info.h_process);
        }
    }
    append_gui_log(
        state,
        &format!("explicit resume launched: task={}", task.task_id),
    );
    let text = if language == Language::English {
        "The unfinished task was sent to Windows RE for explicit resumption."
    } else {
        "已将旧任务送入 Windows RE，继续执行原任务。"
    };
    show_message(
        state.root,
        text,
        if language == Language::English {
            "Resume task"
        } else {
            "继续旧任务"
        },
        MB_OK | MB_ICONINFORMATION,
    );
    true
}

pub(crate) unsafe fn create_task(state: &mut State) {
    if ONLINE_BUSY.is_busy() {
        show_message(
            state.root,
            "已有在线任务正在执行，请等待完成。",
            "任务运行中",
            MB_OK | MB_ICONWARNING,
        );
        return;
    }
    let language = selected_language(state);
    let operation = selected_operation(state).to_string();
    append_gui_log(
        state,
        &format!("GUI action started: create task; operation={operation}"),
    );
    // The PE recovery install is a direct in-process operation (copy WIM +
    // BCD entry), not a WinRE recovery task, so handle it before any of the
    // restore-specific validation below.
    if operation == "install-pe-entry" {
        install_pe_entry(state);
        return;
    }
    // 在任何破坏性确认、镜像校验、提权或 prepare 之前检查未终结任务。
    // 载荷目录按任务 ID 隔离只能防文件覆盖；同一工作区的 EFI bootsequence
    // 仍是单资源。发现旧任务时让用户选择：继续原任务 / 清理并放弃旧任务 / 取消。
    // “继续”只继续最早的任务；“清理”必须经过用户明确确认，不能静默删恢复材料。
    if let Some((choice, task)) = incomplete_task_choice(state, language) {
        if choice == IDYES {
            if launch_explicit_resume(state, &task, language) {
                return;
            }
            return;
        }
        if choice != IDNO {
            append_gui_log(state, "GUI action cancelled: incomplete task remains");
            return;
        }
        if let Err(error) = cleanup_incomplete_tasks_from_gui(state, language) {
            append_gui_log(
                state,
                &format!("GUI action blocked: incomplete task cleanup failed: {error}"),
            );
            show_message(
                state.root,
                &error,
                if language == Language::English {
                    "Cleanup failed"
                } else {
                    "清理失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    }
    let btn_task = GetDlgItem(state.root, ID_CREATE_TASK as i32);
    let current_btn_text = get_text(btn_task);
    if current_btn_text == "选择镜像位置" || current_btn_text == "Choose image location" {
        browse_image(state);
        let updated = get_text(state.controls.image).trim().to_string();
        if backuprestore_core::validate_absolute_path(&updated).is_ok() {
            set_text(
                btn_task,
                if language == Language::English {
                    "Create task"
                } else {
                    "创建任务"
                },
            );
            if is_image_restore_entry(&operation) && Path::new(&updated).is_file() {
                read_image(state);
            }
        }
        return;
    }
    let image_path = get_text(state.controls.image).trim().to_string();
    if operation != "probe"
        && let Err(error) = backuprestore_core::validate_absolute_path(&image_path)
    {
        let kind = if image_path.is_empty() { "empty" } else { "invalid" };
        append_gui_log(
            state,
            &format!(
                "GUI image path check: kind={kind} detail={error}; guidance dialog opened"
            ),
        );
        set_text(
            state.controls.status,
            &if language == Language::English {
                if image_path.is_empty() {
                    "No image path entered. Choose [Choose image location] to pick a file, or [Cancel] to return.".to_string()
                } else {
                    "The image absolute path is invalid. Choose [Choose image location] to pick a file, or [Cancel] to return.".to_string()
                }
            } else if image_path.is_empty() {
                "尚未输入镜像绝对路径。请点击【选择镜像位置】选择文件，或点击【取消】返回。".to_string()
            } else {
                "镜像绝对路径无效。请点击【选择镜像位置】选择文件，或点击【取消】返回。".to_string()
            },
        );
        // 空路径与非法路径分别用不同文案引导，均提供【选择镜像位置】/【取消】
        // （用户 2026-10-18 要求：不要弹错误图标报错，改成提示 + 浏览 + 取消）。
        let choice = ask_invalid_image_path(state.root, language, image_path.is_empty());
        append_gui_log(
            state,
            &format!("GUI image path guidance: user choice={choice} (1=browse 0=cancel)"),
        );
        if choice == 1 {
            // 用户点击【选择镜像位置】：立即弹出文件浏览窗口
            browse_image(state);
            let updated = get_text(state.controls.image).trim().to_string();
            let valid = backuprestore_core::validate_absolute_path(&updated).is_ok();
            append_gui_log(
                state,
                &format!("GUI browse returned: updated={updated} valid={valid}"),
            );
            if valid {
                set_text(
                    btn_task,
                    if language == Language::English {
                        "Create task"
                    } else {
                        "创建任务"
                    },
                );
                if is_image_restore_entry(&operation) && Path::new(&updated).is_file() {
                    read_image(state);
                }
            }
            SetFocus(state.controls.image);
        } else {
            // 用户点击【取消】：将主界面【创建任务】按钮文本修改为【选择镜像位置】
            set_text(
                btn_task,
                if language == Language::English {
                    "Choose image location"
                } else {
                    "选择镜像位置"
                },
            );
            SetFocus(state.controls.image);
        }
        return;
    }
    // 路径合法时确保按钮文本为【创建任务】
    set_text(
        btn_task,
        if language == Language::English {
            "Create task"
        } else {
            "创建任务"
        },
    );
    let index = selected_wim_index(state);
    let index = if is_image_restore_entry(&operation) && index.is_none() {
        if Path::new(&image_path).is_file() {
            read_image(state);
        }
        let recheck = selected_wim_index(state);
        match recheck {
            Some(value) => value.to_string(),
            None => {
                append_gui_log(
                    state,
                    "GUI action blocked: restore requested without a selected WIM index",
                );
                show_message(
                    state.root,
                    if language == Language::English {
                        "Read the WIM first, then select a valid image index from the dropdown."
                    } else {
                        "请先读取 WIM，再从下拉框选择有效的镜像索引。"
                    },
                    if language == Language::English {
                        "Validation failed"
                    } else {
                        "参数校验失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        }
    } else {
        index.unwrap_or(1).to_string()
    };
    let selected_or_error = |control: Hwnd, label: &str| {
        selected_drive_letter(state, control)
            .ok_or_else(|| format!("{label}必须从下拉框选择一个可用卷。"))
    };
    let workspace_drive = state
        .executable_dir
        .to_string_lossy()
        .chars()
        .next()
        .map(|value| value.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    // 三入口拆分：先取目标卷（A 锁定系统盘 / B 数据卷 / C 第二系统位置），
    // 源卷按入口归一——A/B 的 prepare 链要求 source==target（restore-existing），
    // C 的语义是「保留当前系统、装另一个分区」（create-secondary 的 source）。
    // 源卷下拉只在备份/新增第二系统显示，A/B/C 不再读它的选择。
    // 探测页不显示目标卷行（`set_operation_visibility` 隐藏了它），下拉里根本
    // 不存在「用户的选择」。探测不写入任何卷，这个值只是交给 prepare 的参数，
    // 取不到就退回当前系统盘——不能让一个用不到的字段挡住环境自检。
    let target_drive = if operation == "probe" {
        selected_drive_letter(state, state.controls.target).unwrap_or_else(active_system_letter)
    } else {
        match selected_or_error(
            state.controls.target,
            if language == Language::English {
                "Restore target"
            } else {
                "目标卷"
            },
        ) {
            Ok(value) => value,
            Err(error) => {
                append_gui_log(state, &format!("GUI action blocked: {error}"));
                show_message(
                    state.root,
                    &error,
                    if language == Language::English {
                        "Validation failed"
                    } else {
                        "参数校验失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        }
    };
    // 【2026-10-18 缺陷修复 H1 第二层：A 入口目标硬闸】
    // 光修好「自动选中」不够——H1 的成因正是 UI 自动选中**静默失效**而下游没有
    // 任何一道断言兜住：A 页文案写死「目标卷（当前系统盘 X:）」、下拉被禁用，
    // prepare 侧又只要求 `source == target` 就放行。于是只要选中项不是系统盘，
    // 整条链就会格式化那个盘并灌入镜像。这里按入口性质断言到底：
    // restore-existing 的目标必须就是当前运行的系统盘，否则直接拦下。
    if operation == "restore-existing"
        && !target_drive.eq_ignore_ascii_case(&active_system_letter())
    {
        let message = if language == Language::English {
            format!(
                "The 'Restore C:' entry must target the currently running system drive ({}:), \
                 but {} is selected. Refusing to continue — nothing was changed. \
                 (Pick the 'Restore C:' entry only when you intend to overwrite the running system.)",
                active_system_letter(),
                target_drive
            )
        } else {
            format!(
                "「还原系统到 C 盘」入口的目标必须是当前正在运行的系统盘（{}:），\n\
                 而当前选中的是 {}。已拒绝执行，未做任何更改。",
                active_system_letter(),
                target_drive
            )
        };
        append_gui_log(
            state,
            &format!(
                "GUI action blocked: restore-existing target {} != active system drive {}",
                target_drive,
                active_system_letter()
            ),
        );
        show_message(
            state.root,
            &message,
            if language == Language::English {
                "Restore blocked"
            } else {
                "还原已停止"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let source_drive = match operation.as_str() {
        "restore-existing" | "restore-data" => target_drive.clone(),
        "create-secondary" => active_system_letter(),
        // 探测页同样不显示源卷行，与上面目标卷同理退回当前系统盘。
        "probe" => {
            selected_drive_letter(state, state.controls.source).unwrap_or_else(active_system_letter)
        }
        _ => match selected_or_error(
            state.controls.source,
            if language == Language::English {
                "Source volume"
            } else {
                "源卷"
            },
        ) {
            Ok(value) => value,
            Err(error) => {
                append_gui_log(state, &format!("GUI action blocked: {error}"));
                show_message(
                    state.root,
                    &error,
                    if language == Language::English {
                        "Validation failed"
                    } else {
                        "参数校验失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        },
    };
    // Keep this as the first restore-specific guard: no image parsing,
    // confirmation, elevation or preparation is reached when the program
    // directory would be overwritten.
    let workspace_identity = state
        .drives
        .iter()
        .find(|drive| drive.letter.eq_ignore_ascii_case(&workspace_drive));
    let target_identity = state
        .drives
        .iter()
        .find(|drive| drive.letter.eq_ignore_ascii_case(&target_drive));
    if is_image_restore_entry(&operation)
        && same_partition_by_gui_identity(workspace_identity, target_identity)
    {
        append_gui_log(
            state,
            "GUI action blocked: program workspace volume matches restore target",
        );
        show_message(
            state.root,
            if language == Language::English {
                "Cannot start restore: the program directory is on the same volume as the restore target. Move the entire BackupRestore folder to another volume and run it again. No task, WinRE, boot configuration or reboot was requested."
            } else {
                "无法开始还原：程序目录位于目标分区所在卷。请手动将整个 BackupRestore 文件夹移动到其他分区后重新运行。未创建任务，未修改 WinRE，未修改启动配置，未请求重启。"
            },
            if language == Language::English {
                "Restore blocked"
            } else {
                "还原已停止"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // ===== 三入口拆分后的 GUI 侧目标闸门（prepare 的 P0 安全闸仍是最终防线）：
    // B/C 的目标不能是当前系统盘——B 是数据还原、C 是第二系统，覆盖运行中的
    // 系统盘分别属于 A 入口和明确的误操作。
    if matches!(operation.as_str(), "restore-data" | "create-secondary")
        && target_drive.eq_ignore_ascii_case(&active_system_letter())
    {
        let message = if language == Language::English {
            format!(
                "The selected target is the currently running system drive ({}:). Restoring data or a second system onto it is not allowed; use the 'Restore C:' entry to restore the running system. Nothing was changed.",
                active_system_letter()
            )
        } else {
            format!(
                "所选目标是当前正在运行的系统盘（{}:）。还原数据到非 C 盘与还原系统到非 C 盘不允许指向它；要还原当前系统请使用「还原系统到 C 盘」入口。未做任何更改。",
                active_system_letter()
            )
        };
        append_gui_log(
            state,
            "GUI action blocked: restore-data/create-secondary target is the active system drive",
        );
        show_message(
            state.root,
            &message,
            if language == Language::English {
                "Restore blocked"
            } else {
                "还原已停止"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // B（还原数据到非 C 盘）的目标不能是已有 Windows 的分区：数据通道不修复引导，
    // 把数据镜像盖在 Windows 分区上只会得到一个坏系统。指引改用 C/新增第二系统。
    if operation == "restore-data"
        && let Some(target_info) = state
            .drives
            .iter()
            .find(|drive| drive.letter.eq_ignore_ascii_case(&target_drive))
        && target_info.has_windows_installation
    {
        append_gui_log(
            state,
            "GUI action blocked: restore-data target already contains a Windows installation",
        );
        show_message(
            state.root,
            if language == Language::English {
                "The selected target already contains a Windows installation. The data-restore channel does not repair boot configuration; use 'Restore other' or 'Second system' to restore a system. Nothing was changed."
            } else {
                "所选目标分区上已有 Windows 安装。还原数据到非 C 盘的通道不处理引导；要还原系统请改用「还原系统到非 C 盘」或「新增第二系统」。未做任何更改。"
            },
            if language == Language::English {
                "Restore blocked"
            } else {
                "还原已停止"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 【准备期硬拒，GUI 早期版】「系统卷 + 取消格式化（覆盖式应用）」是确定性失败组合。
    // A 入口（还原系统到 C 盘）目标必然是带 Windows 的系统卷；此时取消勾选「格式化
    // 目标分区」= 让 DISM 去改写 C: 上 `Program Files\WindowsApps\...`（MSIX 包目录，
    // owner=TrustedInstaller）的目录元数据，必然被拒（RestoreDirMetadata
    // 0x80070005）→ RE 内 exit 5。
    // 权威闸门在 prepare 的 `validate_operation_inputs`（离线）与
    // `restore_to_windows_volume`（在线），这里只做「不让用户白跑一趟提权+重启」的前置拦截。
    // 详见 docs/20261016-1413 §2、docs/20261016-1836。
    //
    // 【2026-10-16 接手复核修正（R3 文案分叉）】此前本处**自己复制了一份等价条件与
    // 中英双语硬编码文案**，与 `pe_safety` 的判据并存 —— 判据能对、文案会漂
    // （项目 P3 前科）。彼时改为只调用同一个纯函数（判据 + 旧硬拒文案单一来源）。
    // 【2026-10-22 #110】硬拒改确认框后，旧硬拒文案退役（发布件里从未输出），
    // `pe_safety::overlay_restore_on_windows_volume` 收敛为纯谓词；本处确认框
    // 文案语义是「告知 wimlib 通道代价并请示」，与旧硬拒文案的「指导用户改参数」
    // 不同，有意只此一处（无分叉对象）。
    if is_image_restore_entry(&operation) {
        let format_target_checked = IsDlgButtonChecked(state.root, ID_FORMAT_TARGET as i32) != 0;
        let target_has_windows = state
            .drives
            .iter()
            .find(|drive| drive.letter.eq_ignore_ascii_case(&target_drive))
            .is_some_and(|drive| drive.has_windows_installation);
        if crate::pe_safety::overlay_restore_on_windows_volume(
            target_has_windows,
            format_target_checked,
        ) {
            // 【2026-10-22 任务 #110：硬拒改确认】命中「已含 Windows + 取消
            // 格式化」不再直接拦截：DISM 该组合必然 RestoreDirMetadata
            // 0x80070005（docs/20261016-1413 §2），用户裁定「换 wimlib」
            // 后改走 wimlib 通道（自行获取 SeRestore/SeBackup 提权，POC 见
            // docs/20261022-1427）。【2026-10-23 产品化】概念验证
            // （docs/20261022-2308）证明该提权同时覆盖安全描述符还原 →
            // 镜像目录权限（ACL/owner）完整还原，个别受保护目录可能保留卷上
            // 现有权限。确认框明示该语义；确认后放行进入正常任务流——
            // prepare/在线链内部按同一判据路由，已无硬拒点；取消则维持原
            // 拦截行为（什么都不做）。
            append_gui_log(
                state,
                "GUI overlay restore onto a volume containing Windows: asking the user to confirm the wimlib route",
            );
            let notice = if language == Language::English {
                "The target volume already contains Windows. Overlay apply without formatting will use the wimlib channel: directory permissions (ACL/owner) from the image are restored in full; a few protected directories may keep their existing permissions.\n\nContinue?"
            } else {
                "目标分区已含 Windows。取消格式化的覆盖式应用将走 wimlib 通道：镜像中的目录权限（属主与访问控制列表）将完整还原，个别受保护目录可能保留卷上现有权限。\n\n是否继续？"
            };
            let answer = show_message(
                state.root,
                notice,
                if language == Language::English {
                    "Confirm overlay restore (wimlib channel)"
                } else {
                    "确认覆盖式还原（wimlib 通道）"
                },
                MB_YESNO | MB_ICONWARNING,
            );
            if answer != IDYES {
                append_gui_log(
                    state,
                    "GUI action blocked: overlay restore (wimlib route) declined by the user",
                );
                return;
            }
            append_gui_log(
                state,
                "GUI overlay restore: the user confirmed the wimlib route",
            );
        }
    }
    // C（还原系统到非 C 盘）走 create-secondary 语义，离线 prepare 要求启动菜单名必填；
    // 在线通道同样用它给新启动项命名。空名在这里拦下，避免提权后才发现。
    if operation == "create-secondary" && get_text(state.controls.menu).trim().is_empty() {
        append_gui_log(state, "GUI action blocked: create-secondary requires a boot menu name");
        show_message(
            state.root,
            if language == Language::English {
                "Enter the boot menu name for the new system (shown in the Windows boot menu) before creating the task."
            } else {
                "请先填写启动菜单名称（新系统在 Windows 启动菜单中的显示名）再创建任务。"
            },
            if language == Language::English {
                "Validation failed"
            } else {
                "参数校验失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let backup_will_append = operation == "backup" && PathBuf::from(&image_path).is_file();
    let image_drive = image_path
        .chars()
        .next()
        .map(|value| value.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    if operation != "probe" && image_drive == source_drive {
        append_gui_log(
            state,
            "GUI action blocked: image volume matches source volume",
        );
        show_message(
            state.root,
            if language == Language::English {
                "Image volume must differ from the Windows source volume."
            } else {
                "镜像卷不能与 Windows 源卷相同。"
            },
            if language == Language::English {
                "Validation failed"
            } else {
                "参数校验失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // S5：新启动项撞名消解。与现有任一启动菜单项重名时追加 -YYYYMMDDHHMM，
    // 回写文本框并告知——用户可继续手改后再创建。数据源是未提权的
    // bcdedit /enum（list-boot-entries 子命令）；读不到就跳过，提权后的
    // prepare 侧还有一道同样的消解兜底（docs/20261015-0725）。
    if matches!(
        operation.as_str(),
        "create-secondary"
    ) {
        let typed = get_text(state.controls.menu).trim().to_string();
        let existing = existing_boot_entry_names(state);
        let resolved =
            crate::boot_menu::resolve_menu_name_collision(&typed, &existing, chrono::Local::now());
        if resolved != typed {
            append_gui_log(
                state,
                &format!("boot menu name collision resolved: {typed} -> {resolved}"),
            );
            set_menu_name(state, &resolved);
            // 消解结果已是既定名：标上手改，避免随后切索引又被 WIM 名覆盖。
            state.menu_name_touched = true;
            show_message(
                state.root,
                &if language == Language::English {
                    format!(
                        "The boot menu name \"{typed}\" is already in use by an existing boot entry. It has been changed to \"{resolved}\". You can edit it further before creating the task."
                    )
                } else {
                    format!(
                        "启动菜单名称「{typed}」与现有启动项重名，已自动改为「{resolved}」。可继续手动修改后再创建任务。"
                    )
                },
                if language == Language::English {
                    "Boot menu name adjusted"
                } else {
                    "启动菜单名称已调整"
                },
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }
    let operation_label = operation_display(language, &operation);
    // C/第二系统入口把最终菜单名写进破坏性确认摘要：消解后的名字必须让用户
    // 在点「是」之前看见，而不是建完任务才发现启动项叫了别的名字。
    let menu_summary = if matches!(
        operation.as_str(),
        "create-secondary"
    ) {
        let menu = get_text(state.controls.menu).trim().to_string();
        if language == Language::English {
            format!("\nBoot menu name: {menu}")
        } else {
            format!("\n启动菜单名：{menu}")
        }
    } else {
        String::new()
    };
    let (target_line, warning) =
        crate::failure_policy::destructive_confirm_parts(
            &operation,
            message_language(language),
            &target_drive,
        );
    let summary = if language == Language::English {
        format!(
            "Mode: {operation_label}\nSource volume: {source_drive}\nImage: {image_path}{target_line}\nWIM index: {index}{menu_summary}",
        )
    } else {
        format!(
            "模式：{operation_label}\n源卷：{source_drive}\n镜像绝对路径：{image_path}{target_line}\nWIM 索引：{index}{menu_summary}",
        )
    };
    // 测试钩子（--test-hook + auto_install）下自动接受确认框，与 PE 安装确认同策略：
    // 自动化验收环境无法精确点击弹窗按钮，卡在这里等于流程永远走不下去。
    // 探测入口不弹这道确认：它承诺「不写盘、不重启」，弹「破坏性确认」与自身
    // 语义直接冲突，还会让首次使用者以为自检有风险——而该入口的存在意义恰恰是
    // 让人放心地先自检一遍（需求 §7/§8）。备份与三个还原入口的行为一字不动。
    let destructive_answer = if operation == "probe" {
        append_gui_log(
            state,
            "probe: destructive confirmation skipped (no disk write, no reboot)",
        );
        IDYES
    } else if TEST_AUTO_CONFIRM.load(std::sync::atomic::Ordering::SeqCst) {
        append_gui_log(state, "test hook: destructive confirmation auto-accepted");
        IDYES
    } else {
        show_message(
            state.root,
            &format!("{summary}\n\n{warning}"),
            if language == Language::English {
                "Destructive confirmation"
            } else {
                "破坏性确认"
            },
            MB_YESNO | MB_ICONWARNING,
        )
    };
    if is_image_restore_entry(&operation) && destructive_answer != IDYES {
        append_gui_log(
            state,
            &format!("GUI action cancelled: destructive confirmation; operation={operation}"),
        );
        set_text(
            state.controls.status,
            if language == Language::English {
                "The destructive task was cancelled."
            } else {
                "用户取消了破坏性任务。"
            },
        );
        return;
    }
    // ===== 还原前档案（.index-N.metadata.json）检查：缺失或哈希不匹配时
    // 弹窗提示，用户确认后仍可还原（传 --force-restore-hash 跳过校验）=====
    // 档案丢失不阻止还原：用户可能移动/删除了 wim 旁的档案文件。
    let mut force_restore_hash = false;
    let mut verify_hash_enabled = false;
    if is_image_restore_entry(&operation) && PathBuf::from(&image_path).is_file() {
        let verify_hash = unsafe {
            SendMessageW(state.controls.verify_hash, BM_GETCHECK, 0, 0) == BST_CHECKED as isize
        };
        verify_hash_enabled = verify_hash;
        let index_number = index.parse::<u32>().unwrap_or(1);
        let metadata_check = crate::read_index_metadata(Path::new(&image_path), index_number);
        let hash_ok = match &metadata_check {
            Ok(metadata) => {
                // 优化：优先比对文件物理大小；如果用户勾选了“严格对比哈希”，才流式计算大文件 SHA-256
                // 彻底消除默认情况下在主 UI 消息循环线程同步流式读取 58GB 导致窗口未响应（卡死）1~2 分钟的问题。
                if let Ok(file_meta) = std::fs::metadata(&image_path) {
                    if metadata.image_size > 0 && file_meta.len() != metadata.image_size {
                        false
                    } else if verify_hash {
                        backuprestore_core::sha256_file(&image_path)
                            .map(|actual| actual.eq_ignore_ascii_case(&metadata.image_sha256))
                            .unwrap_or(false)
                    } else {
                        true
                    }
                } else {
                    false
                }
            }
            Err(_) => false,
        };
        let (prompt, title) = if metadata_check.is_err() {
            (
                (
                    if language == Language::English {
                        "No backup metadata file was found next to this image (it may have been moved or deleted).\n\nSkipping the integrity check and continuing may restore a wrong or damaged image.\n\nContinue anyway?"
                    } else {
                        "未在此镜像旁找到备份档案文件（可能被移动或删除）。\n\n跳过完整性校验直接还原，可能还原到错误或损坏的镜像。\n\n是否仍要继续还原？"
                    }
                )
                .to_string(),
                (
                    if language == Language::English {
                        "Backup metadata missing"
                    } else {
                        "备份档案缺失"
                    }
                )
                .to_string(),
            )
        } else if !hash_ok {
            (
                (
                    if language == Language::English {
                        "The backup metadata does not match this image file (the image may have been modified or damaged).\n\nContinue anyway?"
                    } else {
                        "备份档案与镜像不匹配（镜像可能被修改或损坏）。\n\n是否仍要还原？"
                    }
                )
                .to_string(),
                (
                    if language == Language::English {
                        "Image hash mismatch"
                    } else {
                        "镜像校验不匹配"
                    }
                )
                .to_string(),
            )
        } else {
            (String::new(), String::new())
        };
        // 测试钩子下同样自动接受（否则档案缺失弹窗会卡住自动验收）。
        let metadata_answer = if TEST_AUTO_CONFIRM.load(std::sync::atomic::Ordering::SeqCst) {
            append_gui_log(state, "test hook: metadata confirmation auto-accepted");
            IDYES
        } else if prompt.is_empty() {
            IDYES
        } else {
            show_message(state.root, &prompt, &title, MB_YESNO | MB_ICONWARNING)
        };
        if !prompt.is_empty() && metadata_answer != IDYES
        {
            append_gui_log(
                state,
                &format!("restore cancelled: metadata check declined; operation={operation}"),
            );
            return;
        }
        force_restore_hash = metadata_check.is_err() || !hash_ok;
        // ===== A（还原系统到 C 盘）容量预检加强：目标锁定为系统盘、无法换盘纠错，
        // 容量不足必须在重启前拦下（进 PE 后才发现等于白重启一次）。
        // required_target_size = max(备份时源分区大小, 已用字节+保留量)，
        // 与目标分区总大小比对（还原会先格式化，比较对象是总容量而非空闲）。
        if operation == "restore-existing"
            && let Ok(metadata) = &metadata_check
            && let Some(target_size) = state
                .drives
                .iter()
                .find(|drive| drive.letter.eq_ignore_ascii_case(&target_drive))
                .and_then(|drive| drive.size_bytes)
            && metadata.required_target_size() > target_size
        {
            let required_gb = metadata.required_target_size() / 1_073_741_824;
            let actual_gb = target_size / 1_073_741_824;
            append_gui_log(
                state,
                &format!(
                    "GUI action blocked: restore target too small; required={} bytes actual={target_size} bytes",
                    metadata.required_target_size()
                ),
            );
            show_message(
                state.root,
                &if language == Language::English {
                    format!(
                        "The system drive ({target_drive}:) is smaller than this image requires (needs about {required_gb} GiB, the partition is {actual_gb} GiB). The target is locked to the system drive and cannot be changed; use a smaller image or a larger disk. Nothing was changed."
                    )
                } else {
                    format!(
                        "系统盘（{target_drive}:）容量小于该镜像的要求（约需 {required_gb} GiB，当前分区 {actual_gb} GiB）。目标锁定为系统盘无法更换；请换用更小的镜像或更大的磁盘。未做任何更改。"
                    )
                },
                if language == Language::English {
                    "Restore blocked"
                } else {
                    "还原已停止"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    }
    // ===== 智能分流：备份/还原目标为「当前活动系统」→ 弹窗选 PE/RE/取消 =====
    // 判断用「当前活动系统」（%SystemDrive%），不是「任何含 Windows 的卷」：
    // 双系统时另一个 Windows 卷并未运行，可直接在线备份/还原。
    let compress = if operation == "backup" {
        // 两项下拉映射为 DISM 术语：压缩=fast、不压缩=none。
        match compression_from_ui_index(combo_selection(state.controls.compress)) {
            Ok(level) => level.to_string(),
            Err(error) => {
                append_gui_log(state, &format!("invalid compression selection: {error}"));
                return;
            }
        }
    } else {
        String::new()
    };
    // 备份索引名（默认程序启动时间，用户可改）与保留最近 N 个索引（0=不清理）。
    let image_name = if operation == "backup" {
        get_text(state.controls.index_name).trim().to_string()
    } else {
        String::new()
    };
    let keep_indexes = if operation == "backup" {
        get_text(state.controls.keep)
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|value| *value > 0)
    } else {
        None
    };
    if matches!(operation.as_str(), "backup" | "restore-existing") {
        let system_upper = std::env::var("SystemDrive")
            .unwrap_or_else(|_| "C:".to_string())
            .trim_end_matches('\\')
            .trim_end_matches(':')
            .to_ascii_uppercase();
        // 待处理分区：备份=被捕获的源卷（镜像保存卷只是存放位置，不影响是否离线）；
        // 还原=被覆盖的目标卷。
        let affected = if operation == "backup" {
            &source_drive
        } else {
            &target_drive
        };
        let affected_upper = affected.trim_end_matches(':').to_ascii_uppercase();
        // A（还原系统到 C 盘）目标锁定系统盘、必离线；备份仍按「是否当前活动系统」分流。
        let needs_offline = if operation == "backup" {
            affected_upper == system_upper
        } else {
            true
        };
        if needs_offline {
            // 当前活动系统：必须离线处理，弹窗让用户选 PE / RE / 取消。
            // 测试钩子配置了 system_drive_choice 时直接采用（跳过弹窗）。
            let choice = match state.test_drive_choice {
                Some(c) => c,
                None => ask_system_drive_handler(state.root, language),
            };
            append_gui_log(
                state,
                &format!("system drive choice returned: {choice} (0=cancel 1=PE 2=RE)"),
            );
            match choice {
                1 => {
                    append_gui_log(
                        state,
                        "system drive operation: user chose PE (schedule PE task)",
                    );
                    schedule_pe_task(
                        state,
                        &operation,
                        &source_drive,
                        &target_drive,
                        &image_path,
                        &index,
                        &compress,
                    );
                    return;
                }
                2 => {
                    // 进入 Windows RE：先弹确认框（明确告知后续动作），
                    // 确认后再走管理员 prepare → WinRE 任务链，避免"只关弹窗无反应"。
                    // 测试钩子指定了 choice 时跳过确认框（自动化环境无法精确点击）。
                    if state.test_drive_choice.is_none() {
                        // 文案按实际操作区分：此前写死「备份」，还原/新增第二系统时
                        // 也在说"执行备份"，用户会以为点错了。
                        // 操作名走 operation_display 的显示名，不能直接用内部键：2026-09-30
                        // 实机确认框显示成「将创建backup任务」，中文界面里混进英文内部键。
                        let operation_text = operation_display(language, &operation);
                        let (body, caption) = if language == Language::English {
                            (
                                format!(
                                    "A {operation_text} task will be created and prepared with administrator rights, then the system will restart into the recovery environment to run it. Continue?"
                                ),
                                "Enter the recovery environment".to_string(),
                            )
                        } else {
                            (
                                format!(
                                    "将创建「{operation_text}」任务并以管理员权限准备，准备完成后系统会重启进入恢复环境执行。是否继续？"
                                ),
                                "进入恢复环境".to_string(),
                            )
                        };
                        let confirm =
                            show_message(state.root, &body, &caption, MB_YESNO | MB_ICONQUESTION);
                        append_gui_log(
                            state,
                            &format!(
                                "system drive operation: Windows RE confirm result={confirm} (6=yes)"
                            ),
                        );
                        if confirm != IDYES {
                            append_gui_log(
                                state,
                                "system drive operation: Windows RE confirm declined",
                            );
                            return;
                        }
                    }
                    append_gui_log(
                        state,
                        "system drive operation: user chose Windows RE (prepare chain)",
                    );
                }
                _ => {
                    append_gui_log(state, "system drive operation cancelled by user");
                    return;
                }
            }
        } else {
            // 非当前活动系统（数据盘 / 未运行的第二系统）：在线直接执行，不重启
            append_gui_log(
                state,
                &format!("online operation: affected={affected_upper} system={system_upper}"),
            );
            run_online_operation(
                state,
                &operation,
                &source_drive,
                &target_drive,
                &image_path,
                &index,
                &compress,
                &image_name,
                keep_indexes,
                false,
            );
            return;
        }
    }
    // ===== B/C 入口按「通道单选」分流（默认在线；离线 = 重启进 PE/RE）=====
    if matches!(operation.as_str(), "restore-data" | "create-secondary") {
        let online = IsDlgButtonChecked(state.root, ID_CHANNEL_ONLINE as i32) != 0;
        append_gui_log(
            state,
            &format!("channel split: op={operation} online={online}"),
        );
        if online {
            // 在线：按「格式化目标分区」选项决定是否清空目标卷（默认勾选）——
            // B=数据卷覆盖/格式化应用；C=格式化+应用+修复引导（online_operation
            // 的 create-secondary 分支，同样尊重格式化选项）。
            // C 的启动菜单名通过 image_name 传给在线通道——restore_to_windows_volume
            // 会优先用它命名新启动项（用户命名 > 镜像版本名自动推断）。
            let online_image_name = if operation == "create-secondary" {
                get_text(state.controls.menu).trim().to_string()
            } else {
                String::new()
            };
            let format_target =
                IsDlgButtonChecked(state.root, ID_FORMAT_TARGET as i32) != 0;
            append_gui_log(
                state,
                &format!("online params: format_target={format_target}"),
            );
            run_online_operation(
                state,
                cli_operation(&operation),
                &source_drive,
                &target_drive,
                &image_path,
                &index,
                &compress,
                &online_image_name,
                keep_indexes,
                format_target,
            );
            return;
        }
        // 离线 B：目标是数据卷（不是系统盘），PE/RE 弹窗用通用文案；
        // PE=排队 PE 任务，RE=确认后走提权 prepare 链。
        if operation == "restore-data" {
            CHOICE_GENERIC_OFFLINE.store(true, std::sync::atomic::Ordering::SeqCst);
            let choice = match state.test_drive_choice {
                Some(c) => c,
                None => ask_system_drive_handler(state.root, language),
            };
            // 弹窗在 WM_CREATE 里已 swap(false) 消费；测试钩子跳过弹窗时手动复位，
            // 防止标志泄漏到下一次系统盘弹窗（文案错成通用版）。
            CHOICE_GENERIC_OFFLINE.store(false, std::sync::atomic::Ordering::SeqCst);
            append_gui_log(
                state,
                &format!("offline data restore choice: {choice} (0=cancel 1=PE 2=RE)"),
            );
            match choice {
                1 => {
                    append_gui_log(
                        state,
                        "offline data restore: user chose PE (schedule PE task)",
                    );
                    schedule_pe_task(
                        state,
                        cli_operation(&operation),
                        &source_drive,
                        &target_drive,
                        &image_path,
                        &index,
                        &compress,
                    );
                    return;
                }
                2 => {
                    // 进入 Windows RE：与 A/备份相同的确认框机制
                    if state.test_drive_choice.is_none() {
                        let operation_text = operation_display(language, &operation);
                        let (body, caption) = if language == Language::English {
                            (
                                format!(
                                    "A {operation_text} task will be created and prepared with administrator rights, then the system will restart into the recovery environment to run it. Continue?"
                                ),
                                "Enter the recovery environment".to_string(),
                            )
                        } else {
                            (
                                format!(
                                    "将创建「{operation_text}」任务并以管理员权限准备，准备完成后系统会重启进入恢复环境执行。是否继续？"
                                ),
                                "进入恢复环境".to_string(),
                            )
                        };
                        let confirm =
                            show_message(state.root, &body, &caption, MB_YESNO | MB_ICONQUESTION);
                        append_gui_log(
                            state,
                            &format!(
                                "offline data restore: Windows RE confirm result={confirm} (6=yes)"
                            ),
                        );
                        if confirm != IDYES {
                            append_gui_log(
                                state,
                                "offline data restore: Windows RE confirm declined",
                            );
                            return;
                        }
                    }
                    append_gui_log(
                        state,
                        "offline data restore: user chose Windows RE (prepare chain)",
                    );
                }
                _ => {
                    append_gui_log(state, "offline data restore cancelled by user");
                    return;
                }
            }
        }
        // 离线 C（还原系统到非 C 盘）：与「新增第二系统」完全一致的离线链——
        // destructive 确认已在上方完成，直接落到下面的提权 prepare。
    }
    let executable =
        std::env::current_exe().unwrap_or_else(|_| state.executable_dir.join("BackupRestore.exe"));
    let mut arguments = vec![
        "prepare".to_string(),
        // GUI 入口归一成 CLI operation：B→restore-existing、C→create-secondary
        //（2.4.9 起收尾按目标性质路由，CLI 侧无需感知三入口细分）。
        "--operation".to_string(),
        cli_operation(&operation).to_string(),
        "--source-drive".to_string(),
        source_drive.clone(),
        "--target-drive".to_string(),
        target_drive.clone(),
        "--boot-menu-name".to_string(),
        get_text(state.controls.menu),
    ];
    if operation != "probe" {
        arguments.extend([
            "--image-path".to_string(),
            image_path,
            "--wim-index".to_string(),
            index,
        ]);
    }
    if operation == "backup" {
        // 压缩下拉：值即 fast/none（DISM 术语，语言无关，已在上方分流处解析）。
        arguments.extend(["--compress".to_string(), compress]);
        // 备份索引名（用户输入，默认程序启动时间）与保留最近 N 个索引。
        arguments.extend(["--image-name".to_string(), image_name]);
        if let Some(keep) = keep_indexes {
            arguments.extend(["--keep-indexes".to_string(), keep.to_string()]);
        }
    }
    if is_image_restore_entry(&operation) {
        arguments.push("--allow-destructive".to_string());
        // 档案缺失/哈希不匹配且用户已确认 → 跳过哈希校验。
        if force_restore_hash {
            arguments.push("--force-restore-hash".to_string());
        }
        if verify_hash_enabled {
            arguments.push("--verify-hash".to_string());
        }
        // 格式化选项（离线链路）：三个还原入口取消勾选「格式化目标分区」时，
        // prepare 落盘 format_target=false，RE 恢复链做覆盖式应用（跳过 diskpart）。
        // 默认勾选不传参（=格式化，历史行为）。
        if matches!(
            operation.as_str(),
            "restore-existing" | "restore-data" | "create-secondary"
        ) && IsDlgButtonChecked(state.root, ID_FORMAT_TARGET as i32) == 0
        {
            append_gui_log(state, "prepare args: --skip-format (overlay restore)");
            arguments.push("--skip-format".to_string());
        }
    }
    if operation == "probe" {
        arguments.push("--no-reboot".to_string());
    }
    // 恢复环境失败处理策略 + 界面语言（2026-10-16 用户拍板做成设置项）。
    // 显式传给 prepare，由它落盘进 `RecoveryTask.env`——这样「任务创建时用的是
    // 哪套策略」被固定在任务里，事后改设置不会改变已有任务的行为。
    arguments.extend([
        "--fail-threshold".to_string(),
        state.failure_policy.threshold.to_string(),
        "--fail-action".to_string(),
        state.failure_policy.action.env_name().to_string(),
        // `pause` 策略的最长等待秒数（0 = 一直等 = 旧版行为）。
        "--fail-pause-max-wait-secs".to_string(),
        state.failure_policy.pause_max_wait_secs.to_string(),
        // DISM 等待上限（2026-10-16 审查补上）：显式传给 prepare，由它落盘进
        // `RecoveryTask.env`，恢复环境在入口灌进进程环境变量。0 = 不限制。
        "--dism-timeout-secs".to_string(),
        state.dism_timeout_secs.to_string(),
        "--ui-language".to_string(),
        if language == Language::English {
            "en".to_string()
        } else {
            "zh".to_string()
        },
    ]);
    let executable_wide = wide(&executable.to_string_lossy());
    let params = arguments
        .iter()
        .map(|argument| quote_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let runas = wide("runas");
    let params = wide(&params);
    append_gui_log(
        state,
        &format!("elevated prepare: launching (visible) operation={operation} params={params:?}"),
    );
    let mut exec_info = ShellExecuteInfoW {
        cb_size: size_of::<ShellExecuteInfoW>() as u32,
        f_mask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: state.root,
        lp_verb: runas.as_ptr(),
        lp_file: executable_wide.as_ptr(),
        lp_parameters: params.as_ptr(),
        lp_directory: null(),
        n_show: SW_SHOWNORMAL,
        h_inst_app: null_mut(),
        lp_id_list: null_mut(),
        lp_class: null(),
        hkey_class: null_mut(),
        dw_hot_key: 0,
        h_icon: null_mut(),
        h_process: null_mut(),
    };
    let ok = ShellExecuteExW(&mut exec_info);
    if ok == 0 {
        let err = GetLastError();
        append_gui_log(
            state,
            &format!(
                "GUI action failed: elevated prepare launch; operation={operation}; ShellExecuteExW failed err={err}"
            ),
        );
        let err_msg = if language == Language::English {
            format!(
                "Could not start the elevated preparation script (Windows error {err}). No task, WinRE or reboot was requested."
            )
        } else {
            format!(
                "无法启动管理员准备脚本（Windows 错误码 {err}）。未创建任务、未修改 WinRE、未请求重启。"
            )
        };
        set_text(state.controls.status, &err_msg);
        show_message(
            state.root,
            &err_msg,
            if language == Language::English {
                "Preparation launch failed"
            } else {
                "准备启动失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    append_gui_log(
        state,
        &format!(
            "elevated prepare: ShellExecuteExW succeeded; h_process={:?}",
            exec_info.h_process
        ),
    );
    if !exec_info.h_process.is_null() {
        // 等待 1500 毫秒，检测 prepare 进程是否因参数校验或安全规则立即异常退出。
        // 如果 1500ms 内退出，说明前置校验失败，必须立即读取 launcher-errors.log 弹窗报错；
        // 如果 1500ms 后仍在运行，说明校验通过，正在正常执行 DISM 挂载与 BCD 准备。
        let wait_result = WaitForSingleObject(exec_info.h_process, 1500);
        if wait_result == 0 {
            let mut exit_code: u32 = 0;
            GetExitCodeProcess(exec_info.h_process, &mut exit_code);
            CloseHandle(exec_info.h_process);
            if exit_code != 0 {
                let last_error = read_latest_launcher_error().unwrap_or_else(|| {
                    if language == Language::English {
                        format!("Preparation process terminated abnormally with exit code {exit_code}")
                    } else {
                        format!("管理员准备进程异常退出，退出码：{exit_code}")
                    }
                });
                append_gui_log(
                    state,
                    &format!(
                        "elevated prepare failed early: exit_code={exit_code} error={last_error}"
                    ),
                );
                let display_err = if language == Language::English {
                    format!("Preparation failed (exit code {exit_code}):\n\n{last_error}")
                } else {
                    format!("准备失败（退出码 {exit_code}）：\n\n{last_error}")
                };
                set_text(state.controls.status, &display_err);
                show_message(
                    state.root,
                    &display_err,
                    if language == Language::English {
                        "Preparation Failed"
                    } else {
                        "任务准备失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        } else {
            CloseHandle(exec_info.h_process);
        }
    }
    append_gui_log(
        state,
        &format!(
            "GUI action delegated: elevated prepare launched; operation={operation}; source={source_drive}; target={target_drive}"
        ),
    );
    set_text(
        state.controls.status,
        if operation == "probe" {
            if language == Language::English {
                "Probe preparation started with -NoReboot. Check status.json and logs; no backup, restore or reboot will run."
            } else {
                "已启动探测准备流程（不重启）。请查看任务状态和日志；不会备份、还原或格式化。"
            }
        } else if backup_will_append {
            if language == Language::English {
                "Elevated preparation started. The existing WIM will receive a new index; existing indexes remain unchanged until the append candidate is verified."
            } else {
                "已启动管理员准备流程。现有 WIM 将追加一个新索引；候选镜像验证完成前，原有索引不会被修改。"
            }
        } else if language == Language::English {
            "Elevated preparation started. Check status.json, prepare.log and Recovery.log; this is not recovery success."
        } else {
            "已启动管理员准备脚本。请在任务结果中查看 status.json、prepare.log 和 Recovery.log；这不是恢复成功证明。"
        },
    );
}

// ===================== 智能分流：进入 PE / 在线直接执行 =====================

/// 将绑定稳定卷身份和镜像索引的请求持久写入 ESP，成功后才安排一次性 PE 启动。
pub(crate) unsafe fn schedule_pe_task(
    state: &State,
    operation: &str,
    source_drive: &str,
    target_drive: &str,
    image_path: &str,
    _index: &str,
    _compress: &str,
) {
    let language = selected_language(state);
    let guid_file = state.executable_dir.join("pe-entry-guid.txt");
    let guid = std::fs::read_to_string(&guid_file)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let Some(guid) = guid else {
        show_message(
            state.root,
            if language == Language::English {
                "No PE entry installed yet. Open the Install PE tab, install the PE first, then retry."
            } else {
                "尚未安装 PE。请先在「安装 PE」页完成 PE 安装，再重试。"
            },
            if language == Language::English {
                "PE entry missing"
            } else {
                "未安装 PE"
            },
            MB_OK | MB_ICONINFORMATION,
        );
        return;
    };
    let request = (|| -> Result<(), backuprestore_core::TaskError> {
        let mode = if operation == "backup" {
            "backup"
        } else {
            "restore"
        };
        let drive = if mode == "backup" {
            source_drive
        } else {
            target_drive
        };
        let index = _index
            .parse::<u32>()
            .map_err(|_| crate::errc(ErrCode::ImageIndexInvalid))?;
        // overlay_no_acl=false：PE 三入口（backup/restore/secondary）恒格式化，
        // 不存在「覆盖式应用到已含 Windows 卷」组合（任务 #110）。
        let request = crate::pe_operation::new_request(mode, drive, image_path, index, None, "", false)?;
        crate::pe_operation::enqueue(&request)
    })();
    let write_ok = request.is_ok();
    if let Err(error) = request {
        show_message(
            state.root,
            &error.to_string(),
            "任务写入失败，未修改启动项",
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let command = format!("bcdedit.exe /set {{bootmgr}} bootsequence {{{guid}}}");
    let code = run_boot_command_logged(Some(state), "schedule_pe_task", &command);
    if code.is_success() && write_ok {
        append_gui_log(state, "schedule_pe_task: bootsequence set, auto reboot now");
        if ExitWindowsEx(EWX_REBOOT, 0) == 0 {
            let fallback = run_cmd_to_file("shutdown.exe /r /t 0 /f", None);
            append_gui_log(
                state,
                &format!("schedule_pe_task: shutdown.exe fallback code={fallback}"),
            );
            if !fallback.is_success() {
                show_message(
                    state.root,
                    if language == Language::English {
                        "PE task configured and boot sequence set, but auto-reboot failed. Please restart manually."
                    } else {
                        "已写入 PE 任务配置并设置一次性启动项，但自动重启失败，请手动重启进入 PE。"
                    },
                    if language == Language::English {
                        "Auto-reboot failed"
                    } else {
                        "自动重启失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
            }
        }
    } else {
        show_message(
            state.root,
            &if language == Language::English {
                format!(
                    "Failed to configure PE task (bcdedit code {code}, config write {write_ok}). Run as administrator."
                )
            } else {
                format!(
                    "配置失败（bcdedit 退出码 {code}，配置写入 {write_ok}）。请确认以管理员身份运行。"
                )
            },
            if language == Language::English {
                "Failed"
            } else {
                "设置失败"
            },
            MB_OK | MB_ICONERROR,
        );
    }
}

use crate::online_operation::{OnlineOpParams, OnlineResult};
pub(crate) static ONLINE_RESULT: std::sync::Mutex<Option<OnlineResult>> = std::sync::Mutex::new(None);
pub(crate) static ONLINE_BUSY: backuprestore_core::operation_safety::OperationGate =
    backuprestore_core::operation_safety::OperationGate::new();

#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn run_online_operation(
    state: &State,
    operation: &str,
    source_drive: &str,
    target_drive: &str,
    image_path: &str,
    index: &str,
    compress: &str,
    image_name: &str,
    keep_indexes: Option<u32>,
    format_target: bool,
) {
    if !ONLINE_BUSY.try_start() {
        show_message(
            state.root,
            "已有在线任务正在执行或结果尚未处理，请等待完成。",
            "任务运行中",
            MB_OK | MB_ICONWARNING,
        );
        return;
    }
    let params = OnlineOpParams {
        operation: operation.into(),
        source_drive: source_drive.into(),
        target_drive: target_drive.into(),
        image_path: image_path.into(),
        index: index.into(),
        compress: compress.into(),
        image_name: image_name.into(),
        keep_indexes,
        format_target,
    };
    let root = state.root as usize;
    EnableWindow(GetDlgItem(state.root, ID_CREATE_TASK as i32), 0);
    // 忙标志一直保持到主窗口取走结果；旧结果不会被新任务清空。
    let spawn = std::thread::Builder::new()
        .name("online-operation".into())
        .spawn(move || {
            // payload 一并带进结果：2026-10-16 审查发现原实现是
            // `unwrap_or_else(|_| …)`，把 panic 载荷（断言消息、文件行号）整个
            // 丢掉，只留一句「线程异常退出」。windows subsystem 没有控制台，
            // 而 `install_panic_logging` 装的 hook 也已把同一份内容写进
            // `<exe>\logs\panic.log`，两边互为兜底。
            let result = std::panic::catch_unwind(|| crate::online_operation::execute(&params))
                .unwrap_or_else(|payload| {
                    let detail = crate::panic_payload_text(payload.as_ref());
                    OnlineResult {
                        success: false,
                        detail: format!(
                            "在线执行线程异常退出（{detail}）。完整调用栈见 \
                             <程序目录>\\logs\\panic.log；请检查本轮日志和候选目录；不能判定为成功。"
                        ),
                        // panic 发生时连 `execute` 的第一行都没走完，没有本轮日志。
                        log_path: None,
                        // panic 不是业务失败，没有稳定码（归因看 panic.log）。
                        error_code: None,
                    }
                });
            *ONLINE_RESULT.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            unsafe {
                PostMessageW(root as Hwnd, WM_APP_ONLINE_DONE, 0, 0);
            }
        });
    if let Err(error) = spawn {
        ONLINE_BUSY.finish();
        EnableWindow(GetDlgItem(state.root, ID_CREATE_TASK as i32), 1);
        show_message(
            state.root,
            &format!("无法启动在线线程：{error}"),
            "启动失败",
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 消息投递失败时由定时器兜底；结果存在则处理一次，不通过文本猜测是否成功。
    SetTimer(state.root, 9002, 1000, None);
    set_text(
        state.controls.status,
        "在线任务执行中；请勿关闭程序。进度窗口显示当前阶段，完成后报告结果。",
    );
    append_gui_log(
        state,
        &format!("online started: op={operation} source={source_drive} target={target_drive}"),
    );
}

/// 用户对待续跑任务的选择。
///
/// 为什么要有它（2026-10-17 第二轮核查，逃生 A）：此前 GUI 一启动就把
/// `resume_pending_boot_task` 的返回值当开关用——有任务就 `shutdown /r`，
/// **窗口都没创建完机器就重启了**，界面上也没有任何叫停手段。现在先问。
pub(crate) enum PendingResumeChoice {
    /// 立即重启继续这个任务
    Reboot,
    /// 本次不重启，任务保留
    Defer,
}

/// 启动早期（还没有主窗口）弹一个框问用户怎么处理待续跑任务。
///
/// 三选一而不是「是/否」：这正是缺口所在——旧逻辑里用户**没有第三个选项**，
/// 想停下这台机器只能去手敲 `abandon-task`，而它在目标卷已擦除后还会拒绝。
pub(crate) unsafe fn ask_pending_resume(pending: &crate::PendingResume) -> PendingResumeChoice {
    let text = format!(
        "检测到上次没跑完的还原任务：\n\n\
         任务：{}\n\
         操作：{:?}\n\
         停在：{:?}\n\n\
         是   = 立即重启，继续这个任务\n\
         否 / 取消 / 直接关窗 = 本次不重启（任务与启动项都保留，以后再处理）\n\
         想放弃这个任务：进主窗口，用顶栏「取消待重启任务」\n\n\
         Detected an unfinished recovery task.\n\
         Yes = reboot now and continue it; No / Cancel / closing the box = not this time \
         (task and boot entry are kept). To abandon it, use the top-bar \
         \"Cancel pending reboot task...\" in the main window.",
        pending.task_id, pending.operation, pending.stage
    );
    let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "待续跑的还原任务 / Unfinished recovery task"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let answer = MessageBoxW(
        null_mut(),
        text.as_ptr(),
        title.as_ptr(),
        MB_YESNOCANCEL | MB_ICONWARNING | MB_SETFOREGROUND,
    );
    match answer {
        IDYES => PendingResumeChoice::Reboot,
        // 「否」「取消」与**直接关窗**都按「本次不重启」处理：
        // 不重启永远是最保守的那个分支 —— 绝不能把「没看清楚就关了窗口」
        // 当成同意重启，更不能当成同意放弃。放弃要走主窗口顶栏那个按钮，
        // 那里有独立的二次确认。
        _ => PendingResumeChoice::Defer,
    }
}

/// 没有主窗口时用的消息框（启动早期 / PE 桌面建窗失败时）。
/// 返回 `MessageBoxW` 的返回值，调用方据此判断用户选了哪个按钮。
pub(crate) unsafe fn message_box_standalone(text: &str, title: &str, flags: u32) -> i32 {
    let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    MessageBoxW(
        null_mut(),
        text.as_ptr(),
        title.as_ptr(),
        flags | MB_SETFOREGROUND,
    )
}

/// 顶栏「取消待重启任务」：GUI 里唯一一个「让机器别再重启」的出口。
///
/// 复用 `abandon-task` 那条已经实机验收过的收口路径，不另写清理逻辑；
/// 目标卷已擦除时它会拒绝，那正是既定的安全边界，如实转达给用户。
pub(crate) unsafe fn cancel_pending_task(state: &mut State) {
    let language = selected_language(state);
    let pending = match crate::find_pending_boot_task() {
        Ok(pending) => pending,
        Err(error) => {
            append_gui_log(state, &format!("cancel_pending: scan failed: {error}"));
            show_message(
                state.root,
                &format!("扫描任务失败：{error}"),
                if language == Language::English {
                    "Scan failed"
                } else {
                    "扫描失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    };
    let Some(pending) = pending else {
        append_gui_log(state, "cancel_pending: no pending boot task");
        show_message(
            state.root,
            if language == Language::English {
                "No unfinished recovery task is waiting for a reboot."
            } else {
                "当前没有等待重启的未完成任务。"
            },
            if language == Language::English {
                "Nothing pending"
            } else {
                "没有待重启任务"
            },
            MB_OK | MB_ICONINFORMATION,
        );
        return;
    };
    let prompt = format!(
        "任务：{}\n操作：{:?}\n停在：{:?}\n\n\
         放弃这个任务并清理？（清掉它的一次性启动项、按终态收口，任务目录与日志等证据保留）\n\n\
         Abandon this task and clean it up? (one-shot boot entry cleared, task marked terminal, logs kept)",
        pending.task_id, pending.operation, pending.stage
    );
    let answer = show_message(
        state.root,
        &prompt,
        if language == Language::English {
            "Abandon pending task?"
        } else {
            "放弃待重启任务？"
        },
        MB_YESNO | MB_ICONWARNING | MB_SETFOREGROUND,
    );
    if answer != IDYES {
        append_gui_log(state, "cancel_pending: user declined; nothing changed");
        return;
    }
    match crate::abandon_pending_task(&pending) {
        Ok(()) => {
            append_gui_log(
                state,
                &format!("cancel_pending: abandoned task {}", pending.task_id),
            );
            show_message(
                state.root,
                if language == Language::English {
                    "Task abandoned and cleaned up. This machine will no longer reboot into the recovery environment for it."
                } else {
                    "已放弃并清理该任务。本机不会再为它重启进恢复环境。"
                },
                if language == Language::English {
                    "Done"
                } else {
                    "已完成"
                },
                MB_OK | MB_ICONINFORMATION,
            );
        }
        Err(error) => {
            append_gui_log(
                state,
                &format!("cancel_pending: abandon failed for {}: {error}", pending.task_id),
            );
            show_message(
                state.root,
                &format!(
                    "放弃失败：{error}\n\n（目标卷已擦除的任务按设计不予清理；\
                     可查看该任务的 prepare.log，或用 resume 命令继续。）"
                ),
                if language == Language::English {
                    "Abandon failed"
                } else {
                    "放弃失败"
                },
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

