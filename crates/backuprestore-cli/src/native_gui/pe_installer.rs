//! PE 引导项安装器与引导链管理
//!
//! 负责：
//! - 内存启动（RAMDisk）与硬盘启动（Flat / Harddisk）PE 引导项部署与 BCD 注册；
//! - 校验 PE 镜像与 shell 配置（）；
//! - 生成一键进入 PE 快捷方式（）；
//! - 引导项命令审计日志记录（）；
//! - 重启至 PE 及独立无 GUI 重启入口（ /  / ）。

use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use super::*;
use backuprestore_core::TaskError;

pub(super) unsafe fn install_pe_entry(state: &State) {
    let mode_ram = IsDlgButtonChecked(state.root, ID_PE_MODE_RAM as i32) != 0;
    if mode_ram {
        install_pe_ramdisk(state);
    } else {
        install_pe_harddisk(state);
    }
}

/// 启动项名称：按程序当前语言 + 启动模式（避免中英混合）。
pub(super) fn pe_entry_description(language: Language, mode_ram: bool) -> String {
    if mode_ram {
        if language == Language::Chinese {
            "Windows PE (BackupRestore) 内存启动".to_string()
        } else {
            "Windows PE (BackupRestore) RAM".to_string()
        }
    } else if language == Language::Chinese {
        "Windows PE (BackupRestore) 硬盘启动".to_string()
    } else {
        "Windows PE (BackupRestore) Disk".to_string()
    }
}

/// 解析「PE 目录路径」输入框的完整路径（如 C:\BackupRestorePE 或 D:\a\PE）。
/// 返回 (盘符字符, 相对根目录路径，如 "\BackupRestorePE" / "\a\PE")。
/// 非法（缺盘符、根目录、含非法字符）返回 None。
pub(super) fn split_pe_dir_path(raw: &str) -> Option<(char, String)> {
    let path = raw.trim().trim_end_matches('\\').trim_end_matches('/');
    if path.len() < 3 {
        return None;
    }
    let bytes = path.as_bytes();
    let drive = bytes[0].to_ascii_uppercase();
    if !drive.is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return None;
    }
    if path[3..].is_empty() {
        // 根目录（如 C:\），不允许：会把 PE 文件散落到盘根
        return None;
    }
    for ch in path[3..].chars() {
        if matches!(ch, '*' | '?' | '<' | '>' | '|' | '"') {
            return None;
        }
    }
    let rel = format!("\\{}", &path[3..]);
    Some((drive as char, rel))
}

/// RAM disk 模式：把 boot.wim 复制到 `{卷}:\{目录}`（不占独立分区），
/// bootmgr 通过 {ramdiskoptions} + boot.sdi 从内存启动。
/// 装 PE 之前先看一眼**这个镜像自己带的启动配置**对不对——**只警告，不拦**
/// （2026-10-29 用户拍板：`只警告，允许继续装`）。
///
/// 背景（实机踩过）：产品装 PE 是纯铺设器——格式化 + `dism /Apply-Image` + 建
/// BCD 项，**一个字节都不改镜像里的文件**。而「PE 启动后自动弹出恢复桌面」靠的是
/// 镜像自带的 `Windows\System32\winpeshl.ini`。若那份配置指向不存在的程序，装完
/// 重启就是「黑一下又回 Windows」，19 秒一轮无限循环，而且**从外面看不出原因**
/// （要进 PE 分区数日志条数才判得出）。机器一秒能查的事，不该让人查几小时。
///
/// 返回 `true` = 继续安装，`false` = 用户选择先不装。
///
/// ★ 探测本身失败（缺 wimlib、镜像正被占用、wimlib 报错）**一律不拦**：这是附加
/// 保护，不是安装的前置条件；把它做成硬门禁会让「装 PE」在缺 wimlib 的机器上
/// 整体不可用，而那种机器本来也能正常装 PE。
pub(super) unsafe fn confirm_pe_image_shell(state: &State, image_path: &str) -> bool {
    let language = selected_language(state);
    let work_dir = state.executable_dir.join("_pe-shell-check");
    let log = crate::log_root::logs_dir().join("pe-shell-check.log");
    let probe = crate::wimlib_apply::probe_pe_image_shell(
        &state.executable_dir,
        Path::new(image_path),
        &work_dir,
        &log,
    );
    let _ = std::fs::remove_dir_all(&work_dir);
    let (shell, recovery_exe_present) = match probe {
        Ok(facts) => facts,
        Err(error) => {
            append_gui_log(
                state,
                &format!("PE image shell check skipped (probe failed): {error}"),
            );
            return true;
        }
    };
    let Some(problem) = crate::text_parsing::judge_pe_shell(&crate::text_parsing::PeShellFacts {
        shell: shell.as_deref(),
        recovery_exe_present,
    }) else {
        append_gui_log(state, "PE image shell check passed");
        return true;
    };
    append_gui_log(
        state,
        &format!(
            "PE image shell check: {} (image={image_path} log={})",
            problem.describe(),
            log.display()
        ),
    );
    let (code_str, detail) = match &problem {
        crate::text_parsing::PeShellProblem::Missing => (
            crate::error_code::ErrCode::PeImageMissingShellConfig.code(),
            if language == Language::English {
                "[BR-PE-059] The image has no Windows\\System32\\winpeshl.ini, so WinPE would stop at a bare command prompt."
                    .to_string()
            } else {
                "[BR-PE-059] 镜像里没有 Windows\\System32\\winpeshl.ini 这个文件，PE 启动后只会停在一个黑命令行窗口。"
                    .to_string()
            },
        ),
        crate::text_parsing::PeShellProblem::Mismatch { bytes, actual } => (
            crate::error_code::ErrCode::PeImageShellTargetNotFound.code(),
            if language == Language::English {
                format!(
                    "[BR-PE-060] That file is {bytes} bytes and is not the expected shell (expected 61 bytes: one line, Recovery.exe,--pe-desktop). It reads: {actual}"
                )
            } else {
                format!(
                    "[BR-PE-060] 这个文件是 {bytes} 字节，内容不是产品期望的那一份（期望 61 字节、只有一行 Recovery.exe,--pe-desktop）。它的实际内容是：{actual}"
                )
            },
        ),
        crate::text_parsing::PeShellProblem::RecoveryExeMissing => (
            crate::error_code::ErrCode::PeImageShellTargetNotFound.code(),
            if language == Language::English {
                "[BR-PE-060] The shell file is correct, but the Recovery.exe it points at is not inside this image."
                    .to_string()
            } else {
                "[BR-PE-060] 启动配置写对了，但它指向的 Recovery.exe 不在这个镜像里。".to_string()
            },
        ),
    };
    append_gui_log(
        state,
        &format!("PE image shell check code=[{code_str}]"),
    );
    if TEST_AUTO_CONFIRM.load(std::sync::atomic::Ordering::SeqCst) {
        append_gui_log(
            state,
            "test hook: PE image shell warning auto-continued",
        );
        return true;
    }
    let message = if language == Language::English {
        format!(
            "This PE image's startup configuration is not what the product expects.\n\n{detail}\n\nInstalling it anyway will most likely give you a screen that flashes black and returns to Windows, over and over, every ~19 seconds. The product does not modify the image, so the install itself would still succeed.\n\nContinue installing anyway?"
        )
    } else {
        format!(
            "这个 PE 镜像的启动配置不对。\n\n{detail}\n\n照这样装下去，重启选 PE 之后大概率会「黑一下又回到 Windows」，而且反复循环（约 19 秒一轮）。安装动作本身会成功——产品只是把镜像原样铺到分区，不改里面的任何文件。\n\n仍然继续安装吗？"
        )
    };
    let answer = show_message(
        state.root,
        &message,
        if language == Language::English {
            "PE image self-check"
        } else {
            "PE 镜像自检"
        },
        MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
    );
    if answer != IDYES {
        append_gui_log(
            state,
            "GUI action cancelled: PE image shell warning declined by user",
        );
        return false;
    }
    append_gui_log(
        state,
        "PE image shell warning acknowledged; continuing by user choice",
    );
    true
}

pub(super) unsafe fn install_pe_ramdisk(state: &State) {
    let language = selected_language(state);
    let image_path = get_text(state.controls.image).trim().to_string();
    if !PathBuf::from(&image_path).is_file() {
        append_gui_log(state, "GUI action blocked: PE WIM file does not exist");
        show_message(
            state.root,
            &if language == Language::English {
                format!("PE WIM file not found: {image_path}")
            } else {
                format!("PE 镜像文件不存在：{image_path}")
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
    // 读「PE 目录路径」：完整路径（如 C:\BackupRestorePE，盘符任意合法目录）。
    // RAM disk 模式把 boot.wim 复制到该路径的 sources\boot.wim，不占独立分区。
    let dir_path = {
        let raw = get_text(GetDlgItem(state.root, ID_PE_DIR_EDIT as i32))
            .trim()
            .to_string();
        if raw.trim().is_empty() {
            // 布局层已填默认值，这里防御性兜底
            let system = std::env::var("SystemDrive")
                .unwrap_or_else(|_| "C:".to_string())
                .trim()
                .trim_end_matches(':')
                .to_ascii_uppercase();
            format!("{system}:\\BackupRestorePE")
        } else {
            raw
        }
    };
    let (target_char, rel_path) = match split_pe_dir_path(&dir_path) {
        Some(value) => value,
        None => {
            append_gui_log(state, "GUI action blocked: invalid PE directory path");
            show_message(
                state.root,
                &if language == Language::English {
                    format!(
                        "Invalid PE directory path \"{dir_path}\". Enter a full path like C:\\BackupRestorePE (any drive letter is allowed); it must not be a drive root."
                    )
                } else {
                    format!(
                        "PE 目录路径不合法：\"{dir_path}\"。请填写完整路径（如 C:\\BackupRestorePE，盘符可以是任意合法目录），不能是盘符根目录。"
                    )
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
    };
    // 启动项名称：读共用的「PE 启动项名称」输入框，空则按语言+模式用默认名
    let entry_name = {
        let typed = get_text(GetDlgItem(state.root, ID_PE_NAME_EDIT as i32))
            .trim()
            .to_string();
        if typed.is_empty() {
            pe_entry_description(language, true)
        } else {
            typed
        }
    };
    if target_char
        == state
            .executable_dir
            .to_string_lossy()
            .chars()
            .next()
            .unwrap_or(' ')
    {
        append_gui_log(
            state,
            "GUI action blocked: target volume is the program volume",
        );
        show_message(
            state.root,
            if language == Language::English {
                "The target volume must differ from the volume that runs this program."
            } else {
                "目标卷不能与运行本程序的卷相同。"
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
    // 装之前先看镜像自带的启动配置（只警告，不拦）。放在破坏性确认**之前**：
    // 能改主意的时候给信息，比格式化完再讲有用。
    if !confirm_pe_image_shell(state, &image_path) {
        return;
    }
    // Destructive-ish confirmation: overwrites <target>:\<dir>\sources\boot.wim
    // and modifies the boot configuration. Nothing runs before this confirmation.
    let answer = if TEST_AUTO_CONFIRM.load(std::sync::atomic::Ordering::SeqCst) {
        append_gui_log(state, "test hook: RAM disk confirmation auto-accepted");
        IDYES
    } else {
        show_message(
            state.root,
            &if language == Language::English {
                format!(
                    "Install PE (RAM disk) to {dir_path}?\n\n- Copy the PE WIM to {dir_path}\\sources\\boot.wim\n- Ensure {dir_path}\\boot\\boot.sdi\n- Add boot entry \"{entry_name}\" to the boot menu\n\nNo extra partition is used. The current Windows default boot is NOT changed. Continue?"
                )
            } else {
                format!(
                    "以 RAM disk 方式安装 PE 到 {dir_path}？\n\n- 复制 PE 镜像到 {dir_path}\\sources\\boot.wim\n- 确保 {dir_path}\\boot\\boot.sdi 存在\n- 在启动菜单新增「{entry_name}」启动项\n\n不占用独立分区。不修改当前 Windows 默认启动。是否继续？"
                )
            },
            if language == Language::English {
                "Install PE"
            } else {
                "安装 PE"
            },
            MB_YESNO | MB_ICONWARNING,
        )
    };
    if answer != IDYES {
        append_gui_log(
            state,
            "GUI action cancelled: PE install confirmation declined",
        );
        return;
    }
    append_gui_log(
        state,
        &format!("PE RAM disk install started: wim={image_path} target={dir_path}"),
    );
    let target_root = format!("{dir_path}\\");
    // 1. Copy the PE WIM into <target>:\<dir>\sources\boot.wim, preserving an
    //    existing file as .stock on first install.
    let sources_dir = format!("{target_root}sources");
    if !PathBuf::from(&sources_dir).is_dir() {
        let _ = log_fs(
            state,
            &format!("创建 PE 源目录 {sources_dir}"),
            std::fs::create_dir_all(&sources_dir),
        );
    }
    let wim_dest = PathBuf::from(format!("{sources_dir}\\boot.wim"));
    if wim_dest.is_file() {
        let stock = PathBuf::from(format!("{sources_dir}\\boot.wim.stock"));
        if !stock.is_file() {
            let _ = log_fs(
                state,
                &format!("备份既有 boot.wim 到 {}", stock.display()),
                std::fs::copy(&wim_dest, &stock),
            );
            append_gui_log(
                state,
                "PE install: existing boot.wim kept as boot.wim.stock",
            );
        }
    }
    if let Err(error) = std::fs::copy(&image_path, &wim_dest) {
        append_gui_log(state, &format!("PE install failed: copy boot.wim: {error}"));
        show_message(
            state.root,
            &format!(
                "{}: {error}",
                if language == Language::English {
                    "Failed to copy the PE WIM"
                } else {
                    "复制 PE 镜像失败"
                }
            ),
            if language == Language::English {
                "PE install failed"
            } else {
                "安装失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 2. Ensure <target>:\boot\boot.sdi exists (ramdisk SDI template).
    let boot_dir = format!("{target_root}boot");
    let sdi_dest = PathBuf::from(format!("{boot_dir}\\boot.sdi"));
    if !sdi_dest.is_file() {
        let _ = log_fs(
            state,
            &format!("创建 PE 引导目录 {boot_dir}"),
            std::fs::create_dir_all(&boot_dir),
        );
        let mut sdi_source: Option<PathBuf> = None;
        for candidate in [
            "C:\\WinPE_arm64\\media\\boot\\boot.sdi",
            "C:\\Program Files (x86)\\Windows Kits\\10\\Assessment and Deployment Kit\\Windows Preinstallation Environment\\arm64\\media\\boot\\boot.sdi",
        ] {
            if PathBuf::from(candidate).is_file() {
                sdi_source = Some(PathBuf::from(candidate));
                break;
            }
        }
        if let Some(src) = sdi_source
            && let Err(error) = std::fs::copy(&src, &sdi_dest)
        {
            append_gui_log(state, &format!("PE install failed: copy boot.sdi: {error}"));
        }
        if !sdi_dest.is_file() {
            append_gui_log(
                state,
                "PE install blocked: <target>:\\<dir>\\boot\\boot.sdi is missing and no ADK copy was found",
            );
            show_message(
                state.root,
                &if language == Language::English {
                    format!(
                        "{dir_path}\\boot\\boot.sdi is missing and no ADK copy could be found. Place the Windows PE boot.sdi on the target volume and try again."
                    )
                } else {
                    format!(
                        "{dir_path}\\boot\\boot.sdi 不存在，且未找到 ADK 副本。请将 WinPE 的 boot.sdi 放到目标卷后重试。"
                    )
                },
                if language == Language::English {
                    "PE install failed"
                } else {
                    "安装失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    }
    // 3. BCD: reuse the system {ramdiskoptions} entry (bcdedit on this
    //    platform rejects /create /application ramdisk), repoint it at the
    //    target volume/folder, then create the PE osloader entry and append
    //    it to the display order.
    let guid_out = state.executable_dir.join("_pe-bcd-guid.txt");
    let guid_out = guid_out.to_string_lossy().to_string();
    // 【2026-10-16 审查修订，日志缺口 8】bcdedit 的输出目录先建好：下面 GUID
    // 解析失败与每个 /set 步骤失败都要把原始输出落到这里，而不是进 NUL。
    let bcd_out_dir = crate::log_root::logs_dir().to_path_buf();
    let _ = log_fs(
        state,
        &format!("创建日志输出目录 {}", &bcd_out_dir.display()),
        std::fs::create_dir_all(&bcd_out_dir),
    );
    let _ = std::fs::remove_file(&guid_out);
    let ram_guid_path = "{ramdiskoptions}".to_string();
    let mut steps = vec![
        format!("bcdedit.exe /set {ram_guid_path} ramdisksdidevice partition={target_char}:"),
        format!("bcdedit.exe /set {ram_guid_path} ramdisksdipath {rel_path}\\boot\\boot.sdi"),
    ];
    let os_guid = {
        let _ = std::fs::remove_file(&guid_out);
        let code = run_cmd_to_file(
            &format!("bcdedit.exe /create /d \"{entry_name}\" /application osloader"),
            Some(std::path::Path::new(&guid_out)),
        );
        if !code.is_success() {
            append_gui_log(
                state,
                &format!("PE install failed: bcdedit create osloader code={code}"),
            );
            show_message(
                state.root,
                &if language == Language::English {
                    format!("bcdedit failed to create the PE boot entry (code {code}).")
                } else {
                    format!("bcdedit 创建 PE 启动项失败（退出码 {code}）。")
                },
                if language == Language::English {
                    "PE install failed"
                } else {
                    "安装失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
        match extract_bcd_guid(&guid_out) {
            Some(guid) => guid,
            None => {
                // 【2026-10-16 审查修订，日志缺口 8】把 bcdedit 的原始输出落盘：
                // 此前解析失败只弹一句"无法读取 GUID"，输出本身没进任何地方，
                // 事后无从判断是 bcdedit 没输出、还是输出格式变了（后者才是
                // 真正需要修的地方）。
                let raw = std::fs::read(&guid_out)
                    .map(|bytes| decode_bcdedit_bytes(&bytes))
                    .unwrap_or_default();
                let _ = log_fs(
                    state,
                    "留存 bcdedit 原始输出",
                    std::fs::copy(&guid_out, bcd_out_dir.join("pe-install-bcdedit-create.out")),
                );
                append_gui_log(
                    state,
                    &format!("PE install failed: could not parse GUID from bcdedit output: {raw}"),
                );
                show_message(
                    state.root,
                    if language == Language::English {
                        "Could not read the created PE boot entry GUID."
                    } else {
                        "无法读取新建的 PE 启动项 GUID。"
                    },
                    if language == Language::English {
                        "PE install failed"
                    } else {
                        "安装失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        }
    };
    let os_guid_path = format!("{{{}}}", os_guid);
    let ramdisk_device =
        format!("ramdisk=[{target_char}:]{rel_path}\\sources\\boot.wim,{ram_guid_path}");
    steps.push(format!(
        "bcdedit.exe /set {os_guid_path} device {ramdisk_device}"
    ));
    steps.push(format!(
        "bcdedit.exe /set {os_guid_path} osdevice {ramdisk_device}"
    ));
    steps.push(format!("bcdedit.exe /set {os_guid_path} winpe yes"));
    steps.push(format!("bcdedit.exe /set {os_guid_path} detecthal yes"));
    steps.push(format!(
        "bcdedit.exe /set {os_guid_path} systemroot \\windows"
    ));
    steps.push(format!("bcdedit.exe /set {os_guid_path} nx OptIn"));
    steps.push(format!(
        "bcdedit.exe /set {os_guid_path} description \"{entry_name}\""
    ));
    steps.push(format!("bcdedit.exe /displayorder {os_guid_path} /addlast"));
    let mut failed_step: Option<String> = None;
    // 【2026-10-16 审查修订，日志缺口 8】每个 bcdedit 步骤的输出落到独立文件：
    // 此前 `run_cmd_to_file(step, None)` 把三流全部送进 NUL，失败时只剩一个
    // 退出码，而 bcdedit 的真实原因（"拒绝访问"/"找不到元素"/GUID 拼错）
    // 恰恰写在输出里。文件放在程序目录，与 `_pe-bcd-guid.txt` 同一处。
    let bcd_out_dir = crate::log_root::logs_dir().to_path_buf();
    let _ = log_fs(
        state,
        &format!("创建日志输出目录 {}", &bcd_out_dir.display()),
        std::fs::create_dir_all(&bcd_out_dir),
    );
    for (index, step) in steps.iter().enumerate() {
        let out_file = bcd_out_dir.join(format!("pe-install-bcdedit-{index}.out"));
        let _ = std::fs::remove_file(&out_file);
        let code = run_cmd_to_file(step, Some(&out_file));
        let detail = std::fs::read(&out_file)
            .map(|bytes| decode_bcdedit_bytes(&bytes))
            .unwrap_or_default();
        let detail = detail.trim();
        append_gui_log(
            state,
            &format!(
                "PE install bcdedit: {step} -> {code}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    crate::i18n::msg_tpl("pe_output_suffix", &[&detail])
                }
            ),
        );
        if !code.is_success() {
            failed_step = Some(if detail.is_empty() {
                step.clone()
            } else {
                format!("{step}\n{detail}")
            });
            break;
        }
    }
    if let Some(step) = failed_step {
        show_message(
            state.root,
            &format!(
                "{}: {}",
                if language == Language::English {
                    "bcdedit step failed"
                } else {
                    "bcdedit 步骤失败"
                },
                step
            ),
            if language == Language::English {
                "PE install failed"
            } else {
                "安装失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let _ = std::fs::remove_file(&guid_out);
    // 记录 PE 启动项 GUID（裸 GUID），供「重启进入 PE」按钮设置 bootsequence 使用。
    let guid_file = state.executable_dir.join("pe-entry-guid.txt");
    let _ = std::fs::write(&guid_file, os_guid.trim());
    append_gui_log(
        state,
        &format!(
            "GUI action completed: PE installed (RAM disk wim -> {dir_path}\\sources\\boot.wim, BCD entry {os_guid_path})"
        ),
    );
    show_message(
        state.root,
        &if language == Language::English {
            format!(
                "PE (RAM disk) installed to {dir_path}.\n\nThe boot menu now has \"{entry_name}\". Reboot and choose it from the menu to enter the recovery desktop. The current Windows default boot is unchanged."
            )
        } else {
            format!(
                "PE（RAM disk）已安装到 {dir_path}。\n\n启动菜单已新增「{entry_name}」。重启后从菜单选择即可进入恢复桌面。当前 Windows 默认启动未改动。"
            )
        },
        if language == Language::English {
            "Install complete"
        } else {
            "安装完成"
        },
        MB_OK,
    );
}

/// 硬盘启动模式：把 boot.wim Apply 到目标独立分区（先格式化），
/// BCD 建 osloader 条目并带 winpe/detecthal 标志，直接从分区引导 PE。
pub(super) unsafe fn install_pe_harddisk(state: &State) {
    let language = selected_language(state);
    let image_path = get_text(state.controls.image).trim().to_string();
    if !PathBuf::from(&image_path).is_file() {
        append_gui_log(state, "GUI action blocked: PE WIM file does not exist");
        show_message(
            state.root,
            &if language == Language::English {
                format!("PE WIM file not found: {image_path}")
            } else {
                format!("PE 镜像文件不存在：{image_path}")
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
    let target_drive = match selected_drive_letter(state, state.controls.target) {
        Some(value) => value,
        None => {
            append_gui_log(state, "GUI action blocked: no target partition selected");
            show_message(
                state.root,
                if language == Language::English {
                    "Select the target partition for the PE install."
                } else {
                    "请先选择 PE 安装的目标分区。"
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
    };
    let drive_char = match target_drive.chars().next() {
        Some(letter) => letter.to_ascii_uppercase(),
        // 取不到目标盘符就拒绝，不退回字母 C——那会凭空造出一个"合法目标"。
        None => {
            append_gui_log(state, "GUI action blocked: target drive letter unavailable");
            show_message(
                state.root,
                if language == Language::English {
                    "Could not determine the target drive letter."
                } else {
                    "无法确定目标分区的盘符。"
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
    };
    // 目标分区安全校验：非系统卷、非程序所在卷、非 ESP、可用空间 >= 2GB。
    //
    // 2026-10-16：系统盘改按 `%SystemDrive%` 判、ESP 改按分区类型 GUID 判，
    // 不再写死 `'C'` / `'S'`。此前那两种写法在「系统盘不是 C」「ESP 不在 S」
    // 的机器上拦不住真正的危险目标，在「C 或 S 其实是数据盘」的机器上又会误拦。
    let target_info = state
        .drives
        .iter()
        .find(|drive| drive.letter.eq_ignore_ascii_case(&drive_char.to_string()));
    let mut is_system_volume = false;
    let mut free_bytes: u64 = 0;
    if let Some(drive) = target_info {
        is_system_volume = drive.has_windows_installation;
        free_bytes = drive.free_bytes.unwrap_or(0);
    }
    let on_system_drive = system_drive_letter() == Some(drive_char);
    let on_esp = is_efi_system_partition(target_info);
    let on_program_drive = is_program_drive(drive_char, &state.executable_dir);
    if on_system_drive || on_esp || on_program_drive {
        append_gui_log(
            state,
            &format!(
                "GUI action blocked: target {drive_char}: is system={on_system_drive} esp={on_esp} program={on_program_drive}"
            ),
        );
        show_message(
            state.root,
            if language == Language::English {
                "The target partition cannot be the system drive, the ESP or the volume that runs this program."
            } else {
                "目标分区不能是系统盘、ESP 或运行本程序的卷。"
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
    if is_system_volume {
        append_gui_log(
            state,
            "GUI action blocked: target partition contains a Windows installation",
        );
        show_message(
            state.root,
            if language == Language::English {
                "The target partition contains a Windows installation. Choose an empty or dedicated partition."
            } else {
                "目标分区包含 Windows 系统，请选择空分区或专用分区。"
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
    if free_bytes < 2 * 1024 * 1024 * 1024 {
        append_gui_log(
            state,
            "GUI action blocked: target partition free space < 2GB",
        );
        show_message(
            state.root,
            &if language == Language::English {
                format!(
                    "Target partition {drive_char}: free space is below 2 GB ({}).",
                    format_bytes(Some(free_bytes))
                )
            } else {
                format!(
                    "目标分区 {drive_char}: 可用空间不足 2GB（{}）。",
                    format_bytes(Some(free_bytes))
                )
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
    // 启动项名称：读共用的「PE 启动项名称」输入框，空则按语言+模式用默认名
    let entry_name = {
        let typed = get_text(GetDlgItem(state.root, ID_PE_NAME_EDIT as i32))
            .trim()
            .to_string();
        if typed.is_empty() {
            pe_entry_description(language, false)
        } else {
            typed
        }
    };
    // 装之前先看镜像自带的启动配置（只警告，不拦）。放在格式化确认之前。
    if !confirm_pe_image_shell(state, &image_path) {
        return;
    }
    // 破坏性确认：目标分区将被格式化 + 写入 PE 系统
    let answer = if TEST_AUTO_CONFIRM.load(std::sync::atomic::Ordering::SeqCst) {
        append_gui_log(state, "test hook: hard disk confirmation auto-accepted");
        IDYES
    } else {
        show_message(
            state.root,
            &if language == Language::English {
                format!(
                    "Install PE (hard disk boot) to partition {drive_char}:?\n\n- The partition WILL BE FORMATTED (all data on it is lost)\n- Apply the PE WIM to {drive_char}:\\\n- Add boot entry \"{entry_name}\" to the boot menu\n\nThe current Windows default boot is NOT changed. Continue?"
                )
            } else {
                format!(
                    "以硬盘启动方式安装 PE 到分区 {drive_char}:？\n\n- 该分区将被格式化（数据全部丢失！）\n- 将 PE 镜像展开到 {drive_char}:\\\n- 在启动菜单新增「{entry_name}」启动项\n\n不修改当前 Windows 默认启动。是否继续？"
                )
            },
            if language == Language::English {
                "Install PE"
            } else {
                "安装 PE"
            },
            MB_YESNO | MB_ICONWARNING,
        )
    };
    if answer != IDYES {
        append_gui_log(
            state,
            "GUI action cancelled: PE hard disk install confirmation declined",
        );
        return;
    }
    append_gui_log(
        state,
        &format!("PE hard disk install started: wim={image_path} partition={drive_char}:"),
    );
    // 不能仅依赖 diskpart 退出码：按物理分区选择、保存输出，并核对新文件系统序列号。
    let format_log = crate::log_root::logs_dir().join("pe-format.log");
    let format_result = (|| -> Result<(), backuprestore_core::TaskError> {
        let before = crate::windows_prepare::volume_identity(drive_char)?;
        if before.volume_serial.is_empty() {
            return Err(crate::err(
                "cannot verify format without original volume serial",
            ));
        }
        let script = crate::diskpart_format_script(before.disk_number, before.partition_number)?;
        let script_file = state.executable_dir.join("_pe-format.txt");
        std::fs::write(&script_file, &script)?;
        crate::append_log(
            &format_log,
            &crate::i18n::msg_tpl("pe_fmt_before", &[&format!("{before:?}"), script.as_str()]),
        )?;
        let result = crate::windows_command::run_program(
            "diskpart.exe",
            &["/s".into(), script_file.to_string_lossy().into_owned()],
            Some(&format_log),
            300000,
        );
        crate::append_log(&format_log, &crate::i18n::msg_tpl("pe_diskpart_result", &[&format!("{result}")]))?;
        if !result.is_success() {
            return Err(crate::err(&format!("diskpart failed: {result}")));
        }
        let after = crate::windows_prepare::volume_identity(drive_char)?;
        crate::append_log(&format_log, &crate::i18n::msg_tpl("pe_fmt_after", &[&format!("{after:?}")]))?;
        backuprestore_core::operation_safety::verify_formatted_volume(&before, &after)?;
        if !Path::new(&format!("{drive_char}:\\")).is_dir() {
            return Err(crate::err("formatted volume root inaccessible"));
        }
        std::fs::remove_file(script_file)?;
        crate::append_log(
            &format_log,
            crate::i18n::msg("pe_fmt_verified"),
        )
    })();
    if let Err(error) = format_result {
        append_gui_log(
            state,
            &format!(
                "PE format failed; refusing apply: {error}; log={}",
                format_log.display()
            ),
        );
        show_message(
            state.root,
            &format!(
                "无法确认格式化成功，已停止展开镜像。\n{error}\n日志：{}",
                format_log.display()
            ),
            "安装失败",
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 2. Apply PE WIM（索引 1 = WinPE）到分区
    // 【2026-10-16 审查修订，卡死 3】等待上限可配置（`dism_timeout`），默认仍
    // INFINITE。此前硬编码 INFINITE，且这条调用链从 `WM_COMMAND` **同步**执行
    // （`4367 install_pe_entry` → `4779 install_pe_harddisk`，不开线程）：
    // DISM 挂死则主窗口消息循环被阻塞，整窗「未响应」、无进度、无取消，
    // 只能任务管理器杀进程。
    let apply_wait_ms = crate::dism_timeout::wait_ms();
    let apply_code = run_cmd_to_file_timeout(
        &format!(
            "dism /Apply-Image /ImageFile:\"{image_path}\" /Index:1 /ApplyDir:{drive_char}:\\"
        ),
        None,
        apply_wait_ms,
    );
    if !apply_code.is_success() {
        append_gui_log(
            state,
            &format!("PE hard disk install failed: dism apply code={apply_code}"),
        );
        show_message(
            state.root,
            &if language == Language::English {
                format!("Failed to apply the PE WIM to {drive_char}:\\ (code {apply_code}).")
            } else {
                format!("将 PE 镜像展开到 {drive_char}:\\ 失败（退出码 {apply_code}）。")
            },
            if language == Language::English {
                "PE install failed"
            } else {
                "安装失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    // 3. BCD：建 osloader 条目（winpe/detecthal 标志齐全），加到显示顺序
    // 【2026-10-16 审查修订，日志缺口 8】bcdedit 的输出目录先建好：下面
    // GUID 解析失败与每个 /set 步骤失败都要把原始输出落到这里，而不是进 NUL。
    let bcd_out_dir = crate::log_root::logs_dir().to_path_buf();
    let _ = log_fs(
        state,
        &format!("创建日志输出目录 {}", &bcd_out_dir.display()),
        std::fs::create_dir_all(&bcd_out_dir),
    );
    let guid_out = state
        .executable_dir
        .join("_pe-bcd-guid.txt")
        .to_string_lossy()
        .to_string();
    let _ = std::fs::remove_file(&guid_out);
    let os_guid = {
        let code = run_cmd_to_file(
            &format!("bcdedit.exe /create /d \"{entry_name}\" /application osloader"),
            Some(std::path::Path::new(&guid_out)),
        );
        if !code.is_success() {
            append_gui_log(
                state,
                &format!("PE hard disk install failed: bcdedit create code={code}"),
            );
            show_message(
                state.root,
                &if language == Language::English {
                    format!("bcdedit failed to create the PE boot entry (code {code}).")
                } else {
                    format!("bcdedit 创建 PE 启动项失败（退出码 {code}）。")
                },
                if language == Language::English {
                    "PE install failed"
                } else {
                    "安装失败"
                },
                MB_OK | MB_ICONERROR,
            );
            return;
        }
        match extract_bcd_guid(&guid_out) {
            Some(guid) => guid,
            None => {
                // 【2026-10-16 审查修订，日志缺口 8】同上：原始输出落盘 + 写进
                // GUI 日志，事后能区分"bcdedit 没输出"与"输出格式变了"。
                let raw = std::fs::read(&guid_out)
                    .map(|bytes| decode_bcdedit_bytes(&bytes))
                    .unwrap_or_default();
                let _ = log_fs(
                    state,
                    "留存 bcdedit 原始输出",
                    std::fs::copy(&guid_out, bcd_out_dir.join("pe-hdd-bcdedit-create.out")),
                );
                append_gui_log(
                    state,
                    &format!(
                        "PE hard disk install failed: could not parse GUID from bcdedit output: {raw}"
                    ),
                );
                show_message(
                    state.root,
                    if language == Language::English {
                        "Could not read the created PE boot entry GUID."
                    } else {
                        "无法读取新建的 PE 启动项 GUID。"
                    },
                    if language == Language::English {
                        "PE install failed"
                    } else {
                        "安装失败"
                    },
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        }
    };
    let os_guid_path = format!("{{{}}}", os_guid);
    let steps = vec![
        format!("bcdedit.exe /set {os_guid_path} device partition={drive_char}:"),
        format!("bcdedit.exe /set {os_guid_path} osdevice partition={drive_char}:"),
        format!("bcdedit.exe /set {os_guid_path} path \\Windows\\system32\\winload.efi"),
        format!("bcdedit.exe /set {os_guid_path} systemroot \\Windows"),
        format!("bcdedit.exe /set {os_guid_path} winpe yes"),
        format!("bcdedit.exe /set {os_guid_path} detecthal yes"),
        format!("bcdedit.exe /set {os_guid_path} nx OptIn"),
        format!("bcdedit.exe /set {os_guid_path} description \"{entry_name}\""),
        format!("bcdedit.exe /displayorder {os_guid_path} /addlast"),
    ];
    let mut failed_step: Option<String> = None;
    // 【2026-10-16 审查修订，日志缺口 8】bcdedit 的输出落盘而不是进 NUL：
    // 此前 `run_cmd_to_file(step, None)` 把三流全送 NUL，失败只剩退出码，
    // 而 bcdedit 的真实原因（拒绝访问 / 找不到元素 / GUID 拼错）写在输出里。
    let bcd_out_dir = crate::log_root::logs_dir().to_path_buf();
    let _ = log_fs(
        state,
        &format!("创建日志输出目录 {}", &bcd_out_dir.display()),
        std::fs::create_dir_all(&bcd_out_dir),
    );
    for (index, step) in steps.iter().enumerate() {
        let out_file = bcd_out_dir.join(format!("pe-hdd-bcdedit-{index}.out"));
        let _ = std::fs::remove_file(&out_file);
        let code = run_cmd_to_file(step, Some(&out_file));
        let detail = std::fs::read(&out_file)
            .map(|bytes| decode_bcdedit_bytes(&bytes))
            .unwrap_or_default();
        let detail = detail.trim();
        append_gui_log(
            state,
            &format!(
                "PE hard disk bcdedit: {step} -> {code}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    crate::i18n::msg_tpl("pe_output_suffix", &[&detail])
                }
            ),
        );
        if !code.is_success() {
            failed_step = Some(if detail.is_empty() {
                step.clone()
            } else {
                format!("{step}\n{detail}")
            });
            break;
        }
    }
    if let Some(step) = failed_step {
        show_message(
            state.root,
            &format!(
                "{}: {}",
                if language == Language::English {
                    "bcdedit step failed"
                } else {
                    "bcdedit 步骤失败"
                },
                step
            ),
            if language == Language::English {
                "PE install failed"
            } else {
                "安装失败"
            },
            MB_OK | MB_ICONERROR,
        );
        return;
    }
    let _ = std::fs::remove_file(&guid_out);
    // 记录 PE 启动项 GUID，供「重启进入 PE」按钮使用
    let guid_file = state.executable_dir.join("pe-entry-guid.txt");
    let _ = std::fs::write(&guid_file, os_guid.trim());
    append_gui_log(
        state,
        &format!(
            "GUI action completed: PE installed (hard disk -> {drive_char}:\\, BCD entry {os_guid_path})"
        ),
    );
    show_message(
        state.root,
        &if language == Language::English {
            format!(
                "PE (hard disk boot) installed to partition {drive_char}:.\n\nThe boot menu now has \"{entry_name}\". Reboot and choose it from the menu to enter the recovery desktop. The current Windows default boot is unchanged."
            )
        } else {
            format!(
                "PE（硬盘启动）已安装到分区 {drive_char}:。\n\n启动菜单已新增「{entry_name}」。重启后从菜单选择即可进入恢复桌面。当前 Windows 默认启动未改动。"
            )
        },
        if language == Language::English {
            "Install complete"
        } else {
            "安装完成"
        },
        MB_OK,
    );
}

/// Parse the first `{xxxxxxxx-....}` GUID from a bcdedit output file and
/// return it WITHOUT the surrounding braces (callers wrap with `{}` as
/// needed). Decoding and scanning live in `text_parsing` so both are covered
/// by the offline test suite.
fn extract_bcd_guid(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    first_braced_guid(&decode_bcdedit_bytes(&bytes))
}

/// Get the current user's shell desktop, including known-folder redirection.
unsafe fn current_desktop_dir() -> Option<PathBuf> {
    let mut path = [0_u16; 260];
    if SHGetFolderPathW(
        null_mut(),
        CSIDL_DESKTOPDIRECTORY,
        null_mut(),
        SHGFP_TYPE_CURRENT,
        path.as_mut_ptr(),
    ) != 0
    {
        return None;
    }
    let length = path.iter().position(|value| *value == 0)?;
    (length > 0).then(|| PathBuf::from(String::from_utf16_lossy(&path[..length])))
}

/// Windows opens file URLs from a .url shell shortcut without a script host.
fn file_url(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let mut encoded = String::new();
    for byte in normalized.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~' | b'/' | b':')
        {
            encoded.push(*byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    format!("file:///{encoded}")
}

/// 「创建快捷方式」：在用户桌面创建指向 BackupRestore.exe 本身的 Windows
/// Internet shortcut（.url）。它由 Shell 直接解析，不依赖 PowerShell、WScript 或
/// 控制台窗口；双击会打开主 GUI。
pub(super) unsafe fn pe_create_shortcut(state: &State) {
    let language = selected_language(state);
    let executable =
        std::env::current_exe().unwrap_or_else(|_| state.executable_dir.join("BackupRestore.exe"));
    let mut directories = Vec::new();
    if let Some(desktop) = current_desktop_dir() {
        directories.push(desktop);
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        directories.push(PathBuf::from(profile).join("Desktop"));
    }
    // Parallels can redirect the user desktop to the shared macOS desktop.
    directories.push(PathBuf::from(r"C:\Mac\Home\Desktop"));
    directories.retain(|directory| directory.is_dir());
    directories.sort();
    directories.dedup();

    let contents = format!(
        "[InternetShortcut]\r\nURL={}\r\nIconFile={}\r\nIconIndex=0\r\n",
        file_url(&executable),
        executable.display(),
    );
    let mut created = Vec::new();
    let mut failures = Vec::new();
    for directory in directories {
        let shortcut = directory.join("BackupRestore.url");
        match fs::write(&shortcut, &contents) {
            Ok(()) => created.push(shortcut),
            Err(error) => failures.push(format!("{}: {error}", shortcut.display())),
        }
    }
    append_gui_log(
        state,
        &format!(
            "PE create shortcut: created=[{}] failures=[{}]",
            created
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("; "),
            failures.join("; ")
        ),
    );
    if !created.is_empty() {
        show_message(
            state.root,
            if language == Language::English {
                "Desktop shortcut \"BackupRestore\" created. Double-click it to open the program."
            } else {
                "已创建桌面快捷方式「BackupRestore」。双击即可打开程序。"
            },
            if language == Language::English {
                "Shortcut created"
            } else {
                "快捷方式已创建"
            },
            MB_OK | MB_ICONINFORMATION,
        );
    } else {
        show_message(
            state.root,
            &if language == Language::English {
                "Failed to create a desktop shortcut. No desktop directory was writable."
                    .to_string()
            } else {
                "创建桌面快捷方式失败：没有可写入的桌面目录。".to_string()
            },
            if language == Language::English {
                "Failed"
            } else {
                "创建失败"
            },
            MB_OK | MB_ICONERROR,
        );
    }
}

/// 「改引导 + 立刻重启」这类动作的统一执行入口。
///
/// 为什么要有它（2026-10-17 第二轮核查，日志缺口 3）：三处设置
/// `bootsequence` 的入口此前都只留「命令 + 退出码」两行（快捷方式那条连这两行
/// 都没有），而 **bcdedit 的真实输出**（元素未找到 / 拒绝访问 / 参数无效）
/// 一个字都没留下 —— 偏偏这几处的后果是**马上重启这台机器**。
///
/// 日志路径统一取 `log_root::logs_dir()`（2026-10-18 用户要求：不放 PE、
/// 不放目标分区）——`state` 只用于决定日志行要不要同步进 GUI 状态栏。
pub(super) unsafe fn run_boot_command_logged(state: Option<&State>, label: &str, command: &str) -> CmdOutcome {
    let logs = crate::log_root::logs_dir();
    // 这两个 helper 可能在没有 `State` 的入口里跑（桌面快捷方式 / PE 桌面），
    // 建目录失败就由随后的写日志失败如实暴露，不在这里假装记一笔。
    let _ = std::fs::create_dir_all(logs);
    let out = logs.join("boot-sequence.log");
    let code = run_cmd_to_file(command, Some(&out));
    let message = format!(
        "{label}: {command} -> exit code={code}（bcdedit 完整输出：{}）",
        out.display()
    );
    match state {
        Some(state) => append_gui_log(state, &message),
        None => {
            // PE/RE 里这一行会由 `session_log` 自动镜像到持久化卷。
            let _ = super::append_log(&logs.join("gui.log"), &message);
        }
    }
    code
}

/// PE 桌面里的重启：先 `wpeutil`（PE 原生），失败再 `shutdown.exe`，
/// 两条都失败**必须**告诉用户。
///
/// 旧代码是 `let _ = run_cmd_to_file("wpeutil.exe reboot", None);`：
/// 没日志、没兜底、失败后 PE 桌面毫无反应，用户面对一台卡住的机器不知道
/// 发生了什么（2026-10-17 第二轮核查，日志缺口 4）。
pub(super) unsafe fn pe_reboot_with_fallback() {
    let logs = crate::log_root::logs_dir();
    // 这两个 helper 可能在没有 `State` 的入口里跑（桌面快捷方式 / PE 桌面），
    // 建目录失败就由随后的写日志失败如实暴露，不在这里假装记一笔。
    let _ = std::fs::create_dir_all(logs);
    let out = logs.join("pe-reboot.log");
    let first = run_cmd_to_file("wpeutil.exe reboot", Some(&out));
    let _ = super::append_log(
        &logs.join("gui.log"),
        &format!("pe_reboot: wpeutil.exe reboot -> exit code={first}"),
    );
    if first.is_success() {
        return;
    }
    let second = run_cmd_to_file("shutdown.exe /r /t 0 /f", None);
    let _ = super::append_log(
        &logs.join("gui.log"),
        &format!("pe_reboot: shutdown.exe fallback -> exit code={second}"),
    );
    if second.is_success() {
        return;
    }
    message_box_standalone(
        "自动重启失败：wpeutil 与 shutdown.exe 都失败了。\n\
         请在命令提示符里手动执行 wpeutil reboot，或长按电源键重启。\n\n\
         Auto-reboot failed: both wpeutil and shutdown.exe failed. \
         Run 'wpeutil reboot' manually or power-cycle the machine.",
        "重启失败 / Reboot failed",
        MB_OK | MB_ICONERROR,
    );
}

/// 记下「原本被 `let _ =` 吞掉」的文件系统操作。
///
/// 为什么要有它（2026-10-17 第二轮核查，日志缺口 5）：PE 安装期间多处
/// `fs::copy` / `create_dir_all` 的失败被直接丢弃，故障要延后到后面的步骤才炸，
/// 而日志里看不出「到底是哪一步没拷成」。
unsafe fn log_fs<T>(state: &State, what: &str, result: std::io::Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            append_gui_log(state, &crate::i18n::msg_tpl("op_failed", &[what, &error.to_string()]));
            None
        }
    }
}

/// 设置一次性 PE 启动项并重启，进入恢复桌面等待用户选择操作。
pub(super) unsafe fn pe_reboot_to_pe(state: &State) {
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
                "No PE entry installed yet. Open the Install PE tab, choose a target volume and a PE WIM, then click Create Task first."
            } else {
                "尚未安装 PE。请先在「安装 PE」页选择目标卷和 PE 镜像，点击「创建任务」完成安装。"
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
    // 单纯进入恢复桌面，无需写入可执行文本任务。
    let command = format!("bcdedit.exe /set {{bootmgr}} bootsequence {{{guid}}}");
    let code = run_boot_command_logged(Some(state), "PE reboot to PE", &command);
    if code.is_success() {
        // 配置成功：立即自动重启进入 PE（无需用户手动重启）
        append_gui_log(state, "PE reboot to PE: bootsequence set, auto reboot now");
        if ExitWindowsEx(EWX_REBOOT, 0) == 0 {
            // ExitWindowsEx 失败（缺关机权限等）时回退 shutdown.exe /r /t 0
            let fallback = run_cmd_to_file("shutdown.exe /r /t 0 /f", None);
            append_gui_log(
                state,
                &format!(
                    "PE reboot to PE: ExitWindowsEx failed, shutdown.exe fallback code={fallback}"
                ),
            );
            if !fallback.is_success() {
                // 两条路都失败：提示用户手动重启
                show_message(
                    state.root,
                    &if language == Language::English {
                        "PE boot sequence set, but auto-reboot failed (ExitWindowsEx and shutdown.exe both failed). Please restart manually.".to_string()
                    } else {
                        "已设置 PE 一次性启动项，但自动重启失败（ExitWindowsEx 与 shutdown.exe 均失败），请手动重启进入 PE。".to_string()
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
                    "Failed to set PE boot sequence (bcdedit code {code}). Run as administrator."
                )
            } else {
                format!("配置失败（bcdedit 退出码 {code}）。请确认以管理员身份运行。")
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

/// 桌面快捷方式入口（`--pe-reboot`）：无主窗口，设置 bootsequence 指向
/// 已安装的 PE，用无父窗口消息框提示结果。非提升进程先提权重启。
pub unsafe fn pe_reboot_standalone() -> Result<(), TaskError> {
    if !is_elevated() {
        relaunch_elevated()?;
        return Ok(());
    }
    let executable_dir = std::env::current_exe()
        .map_err(|error| crate::err(&format!("cannot resolve current executable: {error}")))?
        .parent()
        .map(|parent| parent.to_path_buf())
        .unwrap_or_default();
    let guid_file = executable_dir.join("pe-entry-guid.txt");
    let guid = std::fs::read_to_string(&guid_file)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let Some(guid) = guid else {
        let text = wide(
            "尚未安装 PE。请先打开 BackupRestore，在「安装 PE」页安装后再使用本快捷方式。\n\nPE is not installed yet. Install it from the Install PE tab first.",
        );
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            wide("未安装 PE").as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
        return Ok(());
    };
    let command = format!("bcdedit.exe /set {{bootmgr}} bootsequence {{{guid}}}");
    // 这个入口**没有主窗口**（桌面快捷方式）：此前连一行日志都没有，
    // 进程退出后完全看不出这台机器被设置过一次性启动项。
    let code = run_boot_command_logged(None, "pe-reboot standalone", &command);
    if code.is_success() {
        let text = wide(
            "已设置一次性启动项。重启后自动进入 PE 恢复桌面；PE 启动时会自动清除该启动项，之后重启正常回到 Windows。\n\nBoot sequence set. Reboot to enter the PE recovery desktop automatically.",
        );
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            wide("设置成功").as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    } else {
        let text = wide(&format!(
            "设置 bootsequence 失败（bcdedit 退出码 {code}）。请确认以管理员身份运行。\n\nFailed to set boot sequence (exit code {code})."
        ));
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            wide("设置失败").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
    Ok(())
}
