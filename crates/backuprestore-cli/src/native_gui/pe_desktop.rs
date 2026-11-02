//! PE 恢复桌面（2026-11-02 从 `native_gui.rs` 拆出）。
//!
//! 在 WinRE / WinPE 环境下通过 `--pe-desktop` 启动的独立全屏桌面。
//! 包含：PE 桌面主界面、7 个大卡片按钮交互、时钟刷新、软硬件信息查看、
//! 以及安全的「返回 Windows」BCD 引导还原逻辑。

use super::*;
use crate::recovery_progress::SafeLock;
use crate::{append_log, err, launch_background_console};
use backuprestore_core::TaskError;

/// State for the PE recovery-desktop window. The operation the technician
/// picks is carried out through `PE_EXIT_TAB` (an atomic) so the window can be
/// destroyed safely inside WM_COMMAND before the message loop finishes.
struct PeDesktopState {
    clock: Hwnd,
    fonts: [Handle; 3],
    /// PE 恢复桌面按钮的气泡提示（tooltip）窗口句柄。
    tooltip: Hwnd,
}

/// Exit selection shared between the PE desktop window procedure and
/// `run_pe_desktop`: 0 = no operation, 1 = backup, 2 = restore,
/// 3 = secondary system. A plain atomic avoids any use-after-free when the
/// window is destroyed inside WM_COMMAND.
static PE_EXIT_TAB: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Guard so a double click on "返回 Windows" cannot run two BCD rewrites at
/// once while the window stays alive during the exit sequence.
static PE_EXITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe fn update_pe_clock(state: &PeDesktopState) {
    let mut time = SystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    GetLocalTime(&mut time);
    let text = format!("{:02}:{:02}:{:02}", time.hour, time.minute, time.second);
    set_text(state.clock, &text);
}

/// Restore the boot manager default to `{current}` and reboot, so a PE session
/// launched through a temporary default does not trap the machine in PE. The
/// EFI system partition is located by enumerating volumes, mounted to `S:`,
/// and the BCD entry is rewritten with `bcdedit /store`.
/// Reboot the PE session. `ExitWindowsEx` needs shutdown privileges that the
/// PE shell may lack, so fall back to `wpeutil.exe reboot` (the PE-native
/// restart tool) exactly like the restart card does.
unsafe fn pe_reboot(hwnd: Hwnd) {
    if ExitWindowsEx(EWX_REBOOT, 0) == 0 {
        let wpe = wide("wpeutil.exe");
        let argument = wide("reboot");
        ShellExecuteW(
            hwnd,
            null(),
            wpe.as_ptr(),
            argument.as_ptr(),
            null(),
            SW_SHOW,
        );
    }
}

/// Best-effort diagnostics written via the **verified** ESP volume path
/// (`\\?\Volume{GUID}\exit-pe.log`, survives unmount and reboot) plus a copy
/// in that same volume's log subdirectory, and `X:\exit-pe.log` (PE RAM disk),
/// so a failed exit attempt can be diagnosed from the next Windows session.
///
/// 此前这里还会盲写 `S:\BackupRestore\logs\exit-pe.log`，等于假设 `S:` 就是 ESP。
/// `S:` 被数据卷占用、或 ESP 挂在别的盘符时，本项目的日志会落到一个无关卷上，
/// 而且读日志的人无法分辨。现在所有 ESP 侧路径都从调用方传进来的已核验卷根派生
/// （见 `text_parsing::esp_log_path_on`），卷根拿不到时就只写 RAM 盘，不猜盘符。
///
/// Writes go through CreateFileW/WriteFile directly because std::fs writes to
/// mounted FAT volumes failed silently inside PE. Returns per-path failure
/// details (including GetLastError) so the caller can surface them during
/// development.
fn write_pe_exit_log(entries: &[String], esp_volume_path: Option<&str>) -> Vec<String> {
    let text = entries.join("\r\n");
    let mut failures: Vec<String> = Vec::new();
    // 优先写卷路径（\\?\Volume{GUID}\ 独立于盘符，PE 内最可靠，重启后仍保留在
    // ESP 上），再写同一个已核验卷上的日志子目录副本，最后写 X:（PE RAM 盘，
    // 重启即丢，仅供当次排查）。
    // 不猜盘符：卷根拿不到时宁可只留 RAM 盘那一份，也不往 S:/Q: 盲写——
    // PE 里 Q: 通常不存在（Win11 侧盘符漂移），S: 也可能是别的卷。
    let mut paths: Vec<String> = Vec::new();
    if let Some(vp) = esp_volume_path {
        let mut full = vp.to_string();
        if !full.ends_with('\\') {
            full.push('\\');
        }
        full.push_str("exit-pe.log");
        paths.push(full);
        paths.push(crate::text_parsing::esp_log_path_on(vp, "exit-pe.log"));
    }
    paths.push("X:\\exit-pe.log".to_string());
    for path in paths {
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let file = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0x40000000, // GENERIC_WRITE
                1,          // FILE_SHARE_READ
                null_mut(),
                2,    // CREATE_ALWAYS
                0x80, // FILE_ATTRIBUTE_NORMAL
                null_mut(),
            )
        };
        if file as isize == -1 || file.is_null() {
            failures.push(format!("create {} failed, err={}", path, unsafe {
                GetLastError()
            }));
            continue;
        }
        let mut written: u32 = 0;
        let ok = unsafe {
            WriteFile(
                file,
                text.as_ptr() as *const c_void,
                text.len() as u32,
                &mut written,
                null_mut(),
            )
        };
        if ok == 0 {
            failures.push(format!("write {} failed, err={}", path, unsafe {
                GetLastError()
            }));
        }
        unsafe {
            CloseHandle(file);
        }
    }
    failures
}

/// Development aid: show the full exit diagnostics in a message box right
/// before rebooting, because log files inside PE are unreliable (volume-path
/// and drive-letter writes both failed in the Parallels PE session).
fn show_diag_dialog(hwnd: Hwnd, diag: &[String]) {
    // 自动点击模式：跳过诊断弹窗（无输入通道，弹窗会阻塞自动流程）。
    if PE_AUTO_CLICK.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let text = diag.join("\r\n");
    let msg: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let cap: Vec<u16> = "BackupRestore exit diag"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        MessageBoxW(hwnd, msg.as_ptr(), cap.as_ptr(), 0x40); // MB_ICONINFORMATION
    }
}

/// Restore the boot manager default to `{current}` and reboot, so a PE session
/// launched through a temporary default does not trap the machine in PE. The
/// EFI system partition is located by enumerating volumes, mounted at a free
/// drive letter (S: preferred, then T:..Z:), and the BCD entry is rewritten by
/// running `bcdedit.exe` synchronously with a captured exit code.
unsafe fn exit_pe_to_windows(hwnd: Hwnd) {
    if PE_EXITING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let mut diag: Vec<String> = Vec::new();
    diag.push("exit_pe_to_windows start".to_string());
    let mut esp_volume_path: Option<String> = None;
    let mut volume = [0u16; 512];
    let handle = FindFirstVolumeW(volume.as_mut_ptr(), 512);
    if handle as isize == -1 || handle.is_null() {
        diag.push("FindFirstVolumeW failed".to_string());
        let _ = write_pe_exit_log(&diag, None);
        show_diag_dialog(hwnd, &diag);
        PE_EXITING.store(false, std::sync::atomic::Ordering::SeqCst);
        return;
    }
    let mut found_bcd = false;
    // 现场认不出 Windows 条目时，若 default 不是 PE 条目，仍可以照着现状重启
    // （bootmgr 会回落到 displayorder 第一项）。与 `found_bcd` 分开记，免得
    // 「改成功了」和「没改但能安全重启」两种结局混成一个布尔。
    let mut reboot_without_target = false;
    loop {
        let length = volume.iter().position(|&unit| unit == 0).unwrap_or(0);
        let volume_path = String::from_utf16_lossy(&volume[..length]);
        let mut volume_name = [0u16; 64];
        let mut filesystem = [0u16; 64];
        let mut serial = 0u32;
        let mut max_component = 0u32;
        let mut flags = 0u32;
        let queried = GetVolumeInformationW(
            wide(&volume_path).as_ptr(),
            volume_name.as_mut_ptr(),
            64,
            &mut serial,
            &mut max_component,
            &mut flags,
            filesystem.as_mut_ptr(),
            64,
        );
        if queried != 0 {
            let fs_length = filesystem.iter().position(|&unit| unit == 0).unwrap_or(0);
            let fs = String::from_utf16_lossy(&filesystem[..fs_length]).to_ascii_uppercase();
            diag.push(format!(
                "volume: {} fs: {}",
                volume_path.trim_end_matches('\\'),
                fs
            ));
            if fs == "FAT" || fs == "FAT32" {
                diag.push(format!(
                    "FAT volume: {}",
                    volume_path.trim_end_matches('\\')
                ));
                // Try S: then T:..Z: for a free mount point (X: is the PE RAM disk).
                let mut mounted_letter: Option<u16> = None;
                for letter in ['S', 'T', 'U', 'V', 'W', 'Y', 'Z'] {
                    let mut mount = [0u16; 4];
                    mount[0] = letter as u16;
                    mount[1] = ':' as u16;
                    mount[2] = '\\' as u16;
                    if SetVolumeMountPointW(mount.as_ptr(), wide(&volume_path).as_ptr()) != 0 {
                        mounted_letter = Some(letter as u16);
                        diag.push(format!("mounted at {}:", letter));
                        break;
                    } else {
                        diag.push(format!(
                            "mount {}: failed, last error: {}",
                            letter,
                            GetLastError()
                        ));
                    }
                }
                if let Some(letter) = mounted_letter {
                    // 盘符显式转换：`letter as u8 as char` 在 u16 >255 时会静默截断成
                    // 错盘符（当前 'S'/'T'/'Z' 恰好 <256 是偶然正确）。转换不出来就
                    // 给一个不可能存在的盘符，让下面的 GetFileAttributesW 自然失败，
                    // 同时留诊断信息——本函数无返回值，不引入新的控制流。
                    let letter_char = char::from_u32(u32::from(letter))
                        .filter(|c| c.is_ascii_alphabetic())
                        .unwrap_or('?');
                    if letter_char == '?' {
                        diag.push(format!("ESP letter {letter} is not an ASCII drive letter"));
                    }
                    let bcd_path = format!("{letter_char}:\\EFI\\Microsoft\\Boot\\BCD");
                    if GetFileAttributesW(wide(&bcd_path).as_ptr()) != u32::MAX {
                        diag.push("BCD file found".to_string());
                        esp_volume_path = Some(volume_path.clone());
                        // **现查**「真正的 Windows 启动项」，不再信赖 ESP 上那个
                        // `pe-exit-guid.txt`。
                        //
                        // 2026-10-29 实测的教训：那个文件是部署/测试时一次性写下的，
                        // **之后没有任何机制刷新它**（本机停在 2026-09-26 09:21），
                        // 于是 `default` 被写成一条早已删除的悬空 GUID；机器还能进
                        // Windows，全靠 bootmgr 回落到 `displayorder` 第一项——是运气。
                        //
                        // `{current}` 在这里不能用：PE 里它解析成 PE 条目本身，而不是
                        // 我们要回去的那个 Windows。所以枚举全部条目，按
                        // 「非 winpe + path 是 winload.efi + 该盘符上文件真的存在」
                        // 认出候选，多条时按启动菜单顺序取第一条。
                        let enum_all_file =
                            std::env::temp_dir().join("backuprestore-exit-enum-all.txt");
                        let enum_all_code = run_cmd_to_file(
                            &format!("bcdedit.exe /store \"{bcd_path}\" /enum all /v"),
                            Some(&enum_all_file),
                        );
                        let enum_all_text = std::fs::read(&enum_all_file)
                            .map(|bytes| decode_console_bytes(&bytes))
                            .unwrap_or_default();
                        diag.push(format!("enum all exit code: {enum_all_code}"));
                        let win_guid = crate::text_parsing::pick_windows_loader(
                            &enum_all_text,
                            |letter| {
                                let probe = format!("{letter}:\\Windows\\System32\\winload.efi");
                                GetFileAttributesW(wide(&probe).as_ptr()) != u32::MAX
                            },
                        );
                        for entry in crate::text_parsing::bcd_loader_entries(&enum_all_text) {
                            diag.push(format!(
                                "enum candidate: {} winpe={} device={:?} path={}",
                                entry.identifier, entry.is_winpe, entry.device_partition, entry.loader_path
                            ));
                        }
                        match &win_guid {
                            Some(guid) => diag.push(format!("live Windows loader: {guid}")),
                            None => {
                                diag.push("no Windows loader identified by live query".to_string())
                            }
                        }
                        if let Some(guid) = win_guid {
                            // Split the operations so a harmless
                            // `deletevalue ... Element not found` cannot hide a
                            // failed default restore behind the final exit code.
                            let set_command = format!(
                                "bcdedit.exe /store \"{bcd_path}\" /set {{bootmgr}} default \"{guid}\""
                            );
                            let set_code = run_cmd_to_file(&set_command, None);
                            diag.push(format!("set default exit code: {set_code}"));

                            // A stale bootsequence overrides default and would
                            // boot PE forever. Missing is already success, so
                            // verify the resulting enum instead of trusting the
                            // delete command's localized error code.
                            let delete_command = format!(
                                "bcdedit.exe /store \"{bcd_path}\" /deletevalue {{bootmgr}} bootsequence"
                            );
                            let delete_code = run_cmd_to_file(&delete_command, None);
                            diag.push(format!("delete bootsequence exit code: {delete_code}"));
                            let enum_command =
                                format!("bcdedit.exe /store \"{bcd_path}\" /enum {{bootmgr}}");
                            let enum_file = std::env::temp_dir().join("backuprestore-exit-bcd.txt");
                            let enum_code = run_cmd_to_file(&enum_command, Some(&enum_file));
                            let enum_text = std::fs::read(&enum_file)
                                .map(|bytes| decode_console_bytes(&bytes))
                                .unwrap_or_default();
                            let bootsequence_present =
                                crate::text_parsing::bcd_output_has_bootsequence(&enum_text);
                            diag.push(format!(
                                "verify bootsequence: enum exit={enum_code}, present={bootsequence_present}"
                            ));
                            if set_code.is_success()
                                && enum_code.is_success()
                                && !bootsequence_present
                            {
                                found_bcd = true;
                            } else {
                                diag.push("BCD restore verification failed".to_string());
                            }
                        } else {
                            // 认不出 Windows 条目时**绝不编一个 GUID 写进去**——那正是
                            // 上一版把 default 写成悬空值的原因。但也不能什么都不做：
                            // 若当前 default 指着一个 PE 条目（进 PE 就是靠它），直接
                            // 重启会又回 PE。所以分两种：
                            //   ① default 是 PE 条目 → 不重启，留在 PE 并弹诊断，让人自己
                            //      从启动菜单选 Windows（此时菜单里 Windows 一定在）；
                            //   ② default 不是 PE 条目 → 清掉 bootsequence 后照常重启，
                            //      行为与现状（bootmgr 回落 displayorder 第一项）一致。
                            let delete_command = format!(
                                "bcdedit.exe /store \"{bcd_path}\" /deletevalue {{bootmgr}} bootsequence"
                            );
                            let delete_code = run_cmd_to_file(&delete_command, None);
                            diag.push(format!(
                                "no target identified: delete bootsequence exit code: {delete_code}"
                            ));
                            let default_is_pe =
                                crate::text_parsing::bcd_boot_manager_default(&enum_all_text)
                                    .is_some_and(|default| {
                                        crate::text_parsing::bcd_loader_entries(&enum_all_text)
                                            .iter()
                                            .any(|entry| {
                                                entry.is_winpe
                                                    && entry
                                                        .identifier
                                                        .eq_ignore_ascii_case(&default)
                                            })
                                    });
                            if default_is_pe {
                                diag.push(
                                    "default still points at a PE entry: staying in PE instead of \
                                     rebooting back into it"
                                        .to_string(),
                                );
                            } else {
                                diag.push(
                                    "default is not a PE entry: rebooting without changing default"
                                        .to_string(),
                                );
                                reboot_without_target = true;
                            }
                        }
                    } else {
                        diag.push("BCD file NOT found at mount point".to_string());
                    }
                    // Write diagnostics while the ESP is still mounted (volume
                    // path also survives unmount, so this is belt and braces).
                    diag.push(format!("volume exit sequence done, success={found_bcd}"));
                    let write_failures = write_pe_exit_log(&diag, Some(&volume_path));
                    for f in &write_failures {
                        diag.push(f.clone());
                    }
                    let mut unmount = [letter, ':' as u16, '\\' as u16, 0];
                    DeleteVolumeMountPointW(unmount.as_mut_ptr());
                    if found_bcd || reboot_without_target {
                        break;
                    }
                } else {
                    diag.push("no free mount point".to_string());
                }
            }
        }
        if FindNextVolumeW(handle, volume.as_mut_ptr(), 512) == 0 {
            break;
        }
    }
    FindVolumeClose(handle);
    // Post-unmount best effort: volume path still works after unmount, plus
    // Q: and X: (PE RAM disk).
    let _ = write_pe_exit_log(&diag, esp_volume_path.as_deref());
    if found_bcd || reboot_without_target {
        diag.push(format!(
            "exit sequence done, rebooting (default restored={found_bcd}, reboot without target={reboot_without_target})"
        ));
        let _ = write_pe_exit_log(&diag, esp_volume_path.as_deref());
        pe_reboot(hwnd);
    } else {
        diag.push("exit sequence failed, staying in PE".to_string());
        let _ = write_pe_exit_log(&diag, esp_volume_path.as_deref());
        show_diag_dialog(hwnd, &diag);
        PE_EXITING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// PE 桌面"备份/还原/第二系统"对话框的全局状态。pe_dialog() 在创建窗口
/// 前写入卷列表与默认值；对话框 WM_CREATE 读取建控件；OK 按钮把用户
/// 输入写回 PE_DLG_OUT 后销毁窗口。
static PE_DLG_VOLUMES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static PE_DLG_DEFAULTS: std::sync::Mutex<(String, String, bool, String)> =
    std::sync::Mutex::new((String::new(), String::new(), false, String::new()));
static PE_DLG_OUT: std::sync::Mutex<Option<(String, String, String)>> = std::sync::Mutex::new(None);

/// PE 内时间戳（YYYYMMDD-HHMMSS），用于自动镜像文件名。
fn pe_timestamp() -> String {
    let mut t: SystemTime = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// 枚举 PE 内所有带盘符卷（排除 X: PE RAM 盘与 S: ESP 挂载点），
/// 返回 "盘符|卷标|文件系统|容量" 列表供对话框下拉。
fn pe_list_volumes() -> Vec<String> {
    let mut list = Vec::new();
    unsafe {
        let mask = GetLogicalDrives();
        for i in 0..26 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let d = (b'A' + i) as char;
            if d == 'X' || d == 'S' {
                continue; // PE RAM 盘与 ESP 不参与选卷
            }
            let root: Vec<u16> = format!("{d}:\\").encode_utf16().chain(Some(0)).collect();
            let mut label = [0u16; 128];
            let mut fs = [0u16; 64];
            let mut serial = 0u32;
            let mut maxlen = 0u32;
            let mut flags = 0u32;
            let ok = GetVolumeInformationW(
                root.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                &mut serial,
                &mut maxlen,
                &mut flags,
                fs.as_mut_ptr(),
                fs.len() as u32,
            );
            let mut total: u64 = 0;
            let mut free: u64 = 0;
            let _ = GetDiskFreeSpaceExW(root.as_ptr(), null_mut(), &mut total, &mut free);
            let total_gb = total as f64 / (1024.0 * 1024.0 * 1024.0);
            let label_s = if ok != 0 {
                String::from_utf16_lossy(&label[..label.iter().position(|&c| c == 0).unwrap_or(0)])
            } else {
                String::new()
            };
            let fs_s = if ok != 0 {
                String::from_utf16_lossy(&fs[..fs.iter().position(|&c| c == 0).unwrap_or(0)])
            } else {
                String::new()
            };
            list.push(format!("{d}:|{label_s}|{fs_s}|{total_gb:.1}GB"));
        }
    }
    list
}

/// 默认镜像保存盘：第一个非系统、非 PE/ESP 的卷。
fn pe_default_image_drive(system: &str) -> String {
    for vol in pe_list_volumes() {
        let d = vol
            .split('|')
            .next()
            .unwrap_or("")
            .trim_end_matches(':')
            .to_string();
        let du = d.to_ascii_uppercase();
        if du.is_empty() || du == system.trim_end_matches(':').to_ascii_uppercase() {
            continue;
        }
        return du;
    }
    "H".to_string()
}

unsafe extern "system" fn window_proc_pe_dialog(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    if message == WM_CREATE {
        let (default_path, default_name, show_name, warning) = {
            let guard = PE_DLG_DEFAULTS.safe_lock();
            guard.clone()
        };
        let width = 440;
        // 标签 + 卷下拉
        create_control(
            hwnd,
            "STATIC",
            "目标卷（PE 盘符）：",
            0,
            16,
            14,
            300,
            20,
            ID_PE_DLG_LABEL1,
        );
        let combo = create_control(
            hwnd,
            "COMBOBOX",
            "",
            CBS_DROPDOWNLIST | WS_TABSTOP,
            16,
            34,
            width - 32,
            160,
            ID_PE_DLG_COMBO,
        );
        let volumes = {
            let guard = PE_DLG_VOLUMES.safe_lock();
            guard.clone()
        };
        for vol in &volumes {
            add_combo_item(combo, vol);
        }
        SendMessageW(combo, CB_SETCURSEL, 0, 0);
        SendMessageW(combo, CB_SETDROPPEDWIDTH, 300, 0);
        // 镜像路径
        create_control(
            hwnd,
            "STATIC",
            "镜像路径：",
            0,
            16,
            70,
            300,
            20,
            ID_PE_DLG_LABEL2,
        );
        let edit = create_control(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP | ES_AUTOHSCROLL,
            16,
            90,
            width - 32,
            24,
            ID_PE_DLG_EDIT,
        );
        if !default_path.is_empty() {
            let value = wide(&default_path);
            SetWindowTextW(edit, value.as_ptr());
        }
        // 菜单名称（仅第二系统显示）
        if show_name {
            create_control(
                hwnd,
                "STATIC",
                if get_text(hwnd).contains("还原系统") {
                    "镜像索引："
                } else {
                    "菜单名称："
                },
                0,
                16,
                128,
                300,
                20,
                ID_PE_DLG_LABEL3,
            );
            let name_edit = create_control(
                hwnd,
                "EDIT",
                "",
                WS_TABSTOP | ES_AUTOHSCROLL,
                16,
                148,
                width - 32,
                24,
                ID_PE_DLG_NAME,
            );
            if !default_name.is_empty() {
                let value = wide(&default_name);
                SetWindowTextW(name_edit, value.as_ptr());
            }
        }
        // 确定/取消
        create_control(
            hwnd,
            "BUTTON",
            "执行",
            WS_TABSTOP,
            16,
            236,
            120,
            32,
            ID_PE_DLG_OK,
        );
        create_control(
            hwnd,
            "BUTTON",
            "取消",
            WS_TABSTOP,
            152,
            236,
            120,
            32,
            ID_PE_DLG_CANCEL,
        );
        // 警告/说明（红色小字，位于按钮上方）
        let warn_y = if show_name { 196 } else { 180 };
        let warn = create_control(hwnd, "STATIC", "", 0, 16, warn_y, 420, 36, ID_PE_DLG_WARN);
        if !warning.is_empty() {
            let value = wide(&warning);
            SetWindowTextW(warn, value.as_ptr());
        }
        return 0;
    }
    if message == WM_COMMAND {
        let control_id = w_param & 0xffff;
        if control_id == ID_PE_DLG_OK || control_id == ID_PE_DLG_CANCEL {
            if control_id == ID_PE_DLG_OK {
                let combo = GetDlgItem(hwnd, ID_PE_DLG_COMBO as i32);
                let edit = GetDlgItem(hwnd, ID_PE_DLG_EDIT as i32);
                let name_edit = GetDlgItem(hwnd, ID_PE_DLG_NAME as i32);
                let drive = get_text(combo);
                let path = get_text(edit);
                let name = if name_edit.is_null() {
                    String::new()
                } else {
                    get_text(name_edit)
                };
                let mut guard = PE_DLG_OUT.safe_lock();
                *guard = Some((drive, path, name));
            } else {
                let mut guard = PE_DLG_OUT.safe_lock();
                *guard = None;
            }
            DestroyWindow(hwnd);
            return 0;
        }
    }
    DefWindowProcW(hwnd, message, w_param, l_param)
}

/// PE 内任务对话框：卷下拉 + 镜像路径 + （可选）菜单名称 + 执行/取消。
/// 返回 (卷项, 路径, 名称)；取消返回 None。
unsafe fn pe_dialog(
    owner: Hwnd,
    title: &str,
    show_name: bool,
    warning: &str,
    default_path: &str,
    default_name: &str,
) -> Option<(String, String, String)> {
    unsafe {
        let instance = GetModuleHandleW(null());
        let class_name = wide("BackupRestorePeDialog");
        let class = WndClassExW {
            cb_size: size_of::<WndClassExW>() as u32,
            style: 0,
            wnd_proc: Some(window_proc_pe_dialog),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: instance,
            h_icon: null_mut(),
            h_cursor: null_mut(),
            h_brush: (6usize) as HBrush,
            menu_name: null(),
            class_name: class_name.as_ptr(),
            h_icon_sm: null_mut(),
        };
        let _ = RegisterClassExW(&class); // 已注册则忽略
        // 准备全局状态
        {
            let mut vols = PE_DLG_VOLUMES.safe_lock();
            *vols = pe_list_volumes();
        }
        {
            let mut defs = PE_DLG_DEFAULTS.safe_lock();
            *defs = (
                default_path.to_string(),
                default_name.to_string(),
                show_name,
                warning.to_string(),
            );
        }
        {
            let mut out = PE_DLG_OUT.safe_lock();
            *out = None;
        }
        // ★ 2.6.79：PE 内对话框尺寸按 DPI 缩放，否则 200% 下它只有屏幕的 1/4 大。
        let width = crate::dpi::s(440);
        // 对话框总高需预留 WS_CAPTION 标题栏（约 28-30px），否则底部
        // 「执行/取消」按钮会超出客户区被裁剪，PE 内显示不完整。
        let height = if show_name {
            crate::dpi::s(324)
        } else {
            crate::dpi::s(312)
        };
        let x = ((GetSystemMetrics(SM_CXSCREEN) - width) / 2).max(0);
        let y = ((GetSystemMetrics(SM_CYSCREEN) - height) / 2).max(0);
        let wtitle = wide(title);
        let dlg = CreateWindowExW(
            0,
            class_name.as_ptr(),
            wtitle.as_ptr(),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            x,
            y,
            width,
            height,
            owner,
            null_mut(),
            instance,
            null_mut(),
        );
        if dlg.is_null() {
            return None;
        }
        // 模态消息循环：PeekMessageW 轮询，对话框销毁即返回
        let mut msg: Msg = std::mem::zeroed();
        loop {
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return None;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
                if IsWindow(dlg) == 0 {
                    let guard = PE_DLG_OUT.safe_lock();
                    return guard.clone();
                }
            }
            Sleep(20);
        }
    }
}

/// Convert captured console bytes through the OEM code page the child process
/// actually wrote with. Only reached when the bytes are neither UTF-16 nor
/// valid UTF-8, i.e. GBK/CP437 output from `diskpart`, `ipconfig`, `reg`,
/// `manage-bde` or `systeminfo` on a localized WinRE image.
fn sysinfo_oem_to_string(bytes: &[u8]) -> Option<String> {
    crate::text_parsing::decode_code_page(bytes, 1)
}

/// Decode one probe's bytes: UTF-16LE, then strict UTF-8, then the console OEM
/// code page. The final fallback keeps the old lossy behaviour so a probe never
/// disappears just because the conversion failed.
fn decode_console_bytes(bytes: &[u8]) -> String {
    match crate::text_parsing::plan_console_bytes(bytes) {
        crate::text_parsing::ConsoleBytes::Utf16 => decode_bcdedit_bytes(bytes),
        crate::text_parsing::ConsoleBytes::Utf8(text) => text,
        crate::text_parsing::ConsoleBytes::Oem => sysinfo_oem_to_string(bytes)
            .unwrap_or_else(|| String::from_utf8_lossy(bytes).to_string()),
    }
}

/// Read one probe's output file back as text, tolerating the mixed encodings
/// the console tools emit and collapsing a failed probe to an empty string so
/// the caller renders the visible "unavailable" note instead of raw garbage.
fn sysinfo_probe(command: &str, out_dir: &std::path::Path, name: &str) -> String {
    let out_file = out_dir.join(format!("sysinfo-{name}.txt"));
    let _ = std::fs::remove_file(&out_file);
    // Every probe is read-only. A non-zero exit is normal in WinRE (no
    // BitLocker, no display driver, …) so the exit code is not treated as
    // fatal; only genuinely empty output becomes "unavailable".
    let code = run_cmd_to_file_timeout(command, Some(&out_file), 20000);
    let bytes = match std::fs::read(&out_file) {
        Ok(bytes) => bytes,
        Err(_) => return String::new(),
    };
    let _ = std::fs::remove_file(&out_file);
    if matches!(code, CmdOutcome::SpawnFailed(_)) && bytes.is_empty() {
        return String::new();
    }
    let text = decode_console_bytes(&bytes);
    text.trim_end().to_string()
}

/// `reg query "<key>"` probe that keeps only the key's own value rows.
///
/// The bare listing also prints every subkey path; on
/// `...NT\CurrentVersion` that is dozens of lines of noise which pushed the
/// report past the edit control's text limit in the live WinRE run.
fn sysinfo_reg_probe(key: &str, out_dir: &std::path::Path, name: &str) -> String {
    let raw = sysinfo_probe(&crate::text_parsing::registry_query_all(key), out_dir, name);
    crate::text_parsing::reg_key_values(&raw)
}

/// Collect every read-only source for the information window.
fn collect_system_info_sources(
    out_dir: &std::path::Path,
) -> crate::text_parsing::SystemInfoSources {
    let systeminfo = sysinfo_probe("systeminfo", out_dir, "systeminfo");
    crate::text_parsing::SystemInfoSources {
        hostname: sysinfo_probe("hostname", out_dir, "hostname"),
        systeminfo: crate::text_parsing::systeminfo_cpu_memory(&systeminfo),
        os_registry: sysinfo_reg_probe(
            r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            out_dir,
            "os",
        ),
        bios_registry: sysinfo_reg_probe(r"HKLM\HARDWARE\DESCRIPTION\System\BIOS", out_dir, "bios"),
        disks: sysinfo_probe("echo list disk | diskpart", out_dir, "disks"),
        volumes: sysinfo_probe("echo list volume | diskpart", out_dir, "volumes"),
        display_devices: sysinfo_probe(
            r#"pnputil /enum-devices /class Display /connected"#,
            out_dir,
            "display",
        ),
        resolution: sysinfo_screen_resolution(),
        network: sysinfo_probe("ipconfig /all", out_dir, "network"),
        recovery: sysinfo_probe("reagentc /info", out_dir, "reagentc"),
        bitlocker: sysinfo_probe("manage-bde -status", out_dir, "bitlocker"),
        secure_boot: sysinfo_probe(
            r#"reg query "HKLM\SYSTEM\CurrentControlSet\Control\SecureBoot\State" /v UEFISecureBootEnabled"#,
            out_dir,
            "secureboot",
        ),
    }
}

/// Current desktop resolution straight from the Win32 metrics, so the report
/// has a display figure even when no driver-level probe answers.
fn sysinfo_screen_resolution() -> String {
    unsafe {
        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);
        if width <= 0 || height <= 0 {
            return String::new();
        }
        format!("{width}x{height}")
    }
}

/// `WinRE` when the running environment is the recovery environment, `WinPE`
/// for a plain preinstallation image, otherwise the installed Windows.
///
/// The footer used to be hard-coded to `WinPE`, which mislabelled every run
/// that arrived through `reagentc /boottore`. WinRE is the PE image plus
/// `\Windows\System32\Recovery\ReAgent.xml` / `winre.jpg`-style recovery
/// scaffolding, and its `SystemRoot` lives on the RAM disk (`X:`), so those
/// two facts together separate the three cases without running a command.
fn sysinfo_environment_label() -> &'static str {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| String::from(r"C:\Windows"));
    let on_ramdisk = system_root
        .get(..1)
        .is_some_and(|drive| drive.eq_ignore_ascii_case("X"));
    if !on_ramdisk {
        return "Windows";
    }
    let recovery_markers = [
        r"X:\Windows\System32\Recovery\ReAgent.xml",
        r"X:\sources\recovery\RecEnv.exe",
        r"X:\Windows\System32\RecEnv.exe",
    ];
    if recovery_markers
        .iter()
        .any(|marker| std::path::Path::new(marker).exists())
    {
        "WinRE"
    } else {
        "WinPE"
    }
}

/// Show the read-only software/hardware information window.
///
/// Collection happens before the window is created so the text box is filled
/// in one shot; every probe is independently optional.
unsafe fn pe_system_info_from_desktop(parent: Hwnd) {
    let out_dir = std::env::temp_dir();
    let sources = collect_system_info_sources(&out_dir);
    let collected_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let report = crate::text_parsing::format_system_info_report(
        &sources,
        sysinfo_environment_label(),
        std::env::consts::ARCH,
        &collected_at,
    );
    show_system_info_window(parent, &report);
}

/// Read-only information window: one multiline edit filling the client area,
/// both scroll bars, parent disabled while it is up.
unsafe fn show_system_info_window(parent: Hwnd, report: &str) {
    let instance = GetModuleHandleW(null());
    if instance.is_null() {
        show_message(parent, report, "软硬件信息", MB_OK);
        return;
    }
    let class_name = wide("BackupRestoreSysInfo");
    let class = WndClassExW {
        cb_size: size_of::<WndClassExW>() as u32,
        style: 0,
        wnd_proc: Some(system_info_window_proc),
        cb_cls_extra: 0,
        cb_wnd_extra: 0,
        h_instance: instance,
        h_icon: null_mut(),
        h_cursor: null_mut(),
        h_brush: CreateSolidBrush(PE_BACKGROUND),
        menu_name: null(),
        class_name: class_name.as_ptr(),
        h_icon_sm: null_mut(),
    };
    // A second click re-registers the same class; that returns 0 and is not an
    // error, so the result is deliberately not treated as fatal.
    RegisterClassExW(&class);

    let screen_width = GetSystemMetrics(SM_CXSCREEN).max(640);
    let screen_height = GetSystemMetrics(SM_CYSCREEN).max(480);
    let width = (screen_width * 3 / 4).clamp(560, 1100);
    let height = (screen_height * 3 / 4).clamp(420, 820);
    let title = wide("软硬件信息 - BackupRestore");
    let window = CreateWindowExW(
        0,
        class_name.as_ptr(),
        title.as_ptr(),
        WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_VISIBLE,
        (screen_width - width) / 2,
        (screen_height - height) / 2,
        width,
        height,
        parent,
        null_mut(),
        instance,
        null_mut(),
    );
    if window.is_null() {
        // Falling back to a message box keeps the button useful rather than
        // doing nothing at all if the window cannot be created.
        show_message(parent, report, "软硬件信息", MB_OK);
        return;
    }

    // The text is deliberately NOT passed to CreateWindowExW: a multiline EDIT
    // is created with the default 32,767 character limit, and the report
    // routinely exceeds it, which truncated the 按钮边界 section in the live
    // WinRE run. Create it empty, lift the limit, then fill it.
    let text_box = create_control(
        window,
        "EDIT",
        "",
        WS_TABSTOP
            | WS_BORDER
            | ES_MULTILINE
            | ES_AUTOVSCROLL
            | ES_AUTOHSCROLL
            | ES_READONLY
            | WS_VSCROLL
            | WS_HSCROLL,
        8,
        8,
        width - 32,
        height - 56,
        ID_PE_SYSINFO_TEXT,
    );
    // WinRE images do not necessarily ship Consolas, and the fallback lacks
    // Chinese glyphs. The stock GUI font is localized with the image, so the
    // report remains readable without assuming a font file exists.
    let font = gui_font();
    if !text_box.is_null() {
        if !font.is_null() {
            SendMessageW(text_box, WM_SETFONT, font as WParam, 1);
        }
        // 0 means "as large as the control can hold" for multiline edits.
        SendMessageW(text_box, EM_SETLIMITTEXT, 0, 0);
        // set_text normalizes lone LF to CRLF; the EDIT control renders a bare
        // LF as an invisible control character, which glued the report header
        // into one unreadable row.
        set_text(text_box, report);
    }
    ShowWindow(window, SW_SHOW);
    if !text_box.is_null() {
        // Focus immediately so PageDown/Ctrl+End/the wheel scroll the report
        // without first having to Tab into the text box.
        SetFocus(text_box);
    }

    // Modal by hand, following `pe_dialog`: poll with PeekMessageW and stop as
    // soon as the window is gone. A blocking GetMessageW is wrong here twice
    // over: it would hang once the last message is drained, and -- seen live
    // in WinRE -- a quit message left in the queue when `IsWindow` short
    // circuits the loop condition let the desktop's own loop consume it right
    // after, which ended the recovery desktop and returned to Windows just
    // because the user closed this read-only report with Alt+F4.
    EnableWindow(parent, 0);
    let mut message: Msg = std::mem::zeroed();
    // One message per iteration: an empty queue only means "idle", so checking
    // IsWindow after every drain is what ends the loop, and the WM_QUIT branch
    // cannot re-feed itself.
    loop {
        if PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) == 0 {
            if IsWindow(window) == 0 {
                break;
            }
            Sleep(20);
            continue;
        }
        if message.message == WM_QUIT {
            // The quit belongs to the desktop, not to this report window: hand
            // it back so a real exit request still reaches the desktop loop,
            // then stop being modal.
            PostQuitMessage(0);
            break;
        }
        TranslateMessage(&message);
        DispatchMessageW(&message);
        if IsWindow(window) == 0 {
            break;
        }
    }
    EnableWindow(parent, 1);
    SetForegroundWindow(parent);
}

unsafe extern "system" fn system_info_window_proc(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    match message {
        WM_SIZE => {
            // Keep the text box filling the client area so resizing the window
            // actually gains reading space.
            let edit = GetDlgItem(hwnd, ID_PE_SYSINFO_TEXT as i32);
            if !edit.is_null() {
                let width = (l_param & 0xffff) as i32;
                let height = ((l_param >> 16) & 0xffff) as i32;
                // ★ 2.6.79：内边距随 DPI 缩放；width/height 来自父窗口，已是物理像素。
                MoveWindow(
                    edit,
                    crate::dpi::s(8),
                    crate::dpi::s(8),
                    width - crate::dpi::s(16),
                    height - crate::dpi::s(16),
                    1,
                );
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        // No WM_DESTROY handler on purpose: this window shares its thread with
        // the recovery desktop's message loop, so PostQuitMessage here would
        // take the desktop down with it. See `show_system_info_window`.
        _ => DefWindowProcW(hwnd, message, w_param, l_param),
    }
}

/// PE 桌面「备份系统」：PE 内直接 dism 捕获，不再跳主 GUI。
/// PE 三个界面入口共用类型化请求，不再拼接逐行破坏命令。
unsafe fn pe_backup_from_desktop(hwnd: Hwnd) {
    pe_run_interactive(hwnd, "backup");
}
unsafe fn pe_restore_from_desktop(hwnd: Hwnd) {
    pe_run_interactive(hwnd, "restore");
}
unsafe fn pe_secondary_from_desktop(hwnd: Hwnd) {
    pe_run_interactive(hwnd, "secondary");
}

unsafe fn pe_run_interactive(hwnd: Hwnd, operation: &str) {
    let (title, warning, default_name) = match operation {
        "backup" => (
            "备份系统 - BackupRestore",
            "捕获所选卷；已有镜像使用候选副本追加，不删除原件。",
            "",
        ),
        "secondary" => (
            "安装第二系统 - BackupRestore",
            "安装到指定目标；必须另外确认需要保留的主系统。",
            "Windows 备份",
        ),
        _ => (
            "还原系统 - BackupRestore",
            "仅接受系统镜像；先校验再格式化，失败即停止。",
            "1",
        ),
    };
    let default_wim = if operation == "backup" {
        format!(
            "{}:\\pe-bk-{}.wim",
            pe_default_image_drive(""),
            pe_timestamp()
        )
    } else {
        String::new()
    };
    let Some((drive_item, image, name)) = pe_dialog(
        hwnd,
        title,
        operation != "backup",
        warning,
        &default_wim,
        default_name,
    ) else {
        return;
    };
    let drive = drive_item
        .split('|')
        .next()
        .unwrap_or("")
        .trim_end_matches(':');
    let main = if operation == "secondary" {
        let Some((item, _, _)) = pe_dialog(
            hwnd,
            "选择需保留的主系统",
            false,
            "此处选择已有主系统卷，仅记录保护身份，不写入该卷。",
            "",
            "",
        ) else {
            return;
        };
        item.chars().next()
    } else {
        None
    };
    let index = if operation == "restore" {
        name.trim().parse::<u32>()
    } else {
        Ok(1)
    };
    let request = index
        .map_err(|_| crate::errc(ErrCode::ImageIndexNotPositive))
        .and_then(|index| {
            // overlay_no_acl=false：PE 桌面三入口恒格式化，无覆盖式组合（任务 #110）。
            crate::pe_operation::new_request(operation, drive, image.trim(), index, main, &name, false)
        });
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            show_message(
                hwnd,
                &error.to_string(),
                "参数校验失败",
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    };
    if operation != "backup" {
        let ask = show_message(
            hwnd,
            &format!(
                "确认将 {} 的索引 {} 还原到卷 {}:？\n此操作将格式化目标卷。输入或角色校验失败时不会格式化。",
                image.trim(),
                request.index,
                drive
            ),
            "还原确认",
            MB_YESNO | MB_ICONWARNING,
        );
        if ask != IDYES {
            return;
        }
    }
    EnableWindow(hwnd, 0);
    let result = crate::pe_operation::execute(&request);
    EnableWindow(hwnd, 1);
    match result {
        Ok(detail) => {
            let title = if operation == "backup" {
                "备份完成"
            } else {
                "还原完成"
            };
            show_message(hwnd, &detail, title, MB_OK | MB_ICONINFORMATION);
            if show_message(
                hwnd,
                "操作已完成，返回 Windows 吗？",
                "BackupRestore",
                MB_YESNO | MB_ICONQUESTION,
            ) == IDYES
            {
                pe_reboot(hwnd);
            }
        }
        Err(error) => {
            show_message(
                hwnd,
                &format!("操作未完成，已停止后续写入。\n{error}"),
                "操作失败",
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

unsafe extern "system" fn window_proc_pe(
    hwnd: Hwnd,
    message: u32,
    w_param: WParam,
    l_param: LParam,
) -> LResult {
    if message == WM_CREATE {
        // ★ 2.6.89：PE 恢复桌面也换成微软雅黑常规字重，与主界面统一。
        //   原来这里用 "Segoe UI"，字重 700 / 600 / 500 三级做层次。
        //   但实测本项目的 PE 镜像里 **Segoe UI 只登记了 Bold(700) / Light(300) /
        //   Semilight(350) / Italic，没有常规体 `segoeui.ttf`**，于是：
        //     700 → 命中真粗体；
        //     600 → 按最接近挑到 Bold（600 比 350 更靠近 700）；
        //     500 → 按最接近挑到 Semilight(350)。
        //   也就是三级字重全是错位的，代码写的和实际画的不是一回事。
        //   微软雅黑在本 PE 镜像里是齐的（`msyh.ttc` 已登记），换成它之后三档字号
        //   用同一个真常规体，层次改由**字号**（44 / 30 / 18）承担。
        //   证据见 `docs/20261030-1520-PE镜像字体核查-微软雅黑可用但SegoeUI缺常规体-v2.6.89-DS41F.md`。
        let face = wide("Microsoft YaHei");
        // ★ 2.6.79：恢复桌面上的大字号同样按 DPI 缩放，与布局坐标同比放大。
        let title_font = CreateFontW(
            -crate::dpi::s(44),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            134,
            0,
            0,
            0,
            0,
            face.as_ptr(),
        );
        let card_font = CreateFontW(
            -crate::dpi::s(30),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            134,
            0,
            0,
            0,
            0,
            face.as_ptr(),
        );
        let bar_font = CreateFontW(
            -crate::dpi::s(18),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            134,
            0,
            0,
            0,
            0,
            face.as_ptr(),
        );
        let width = GetSystemMetrics(SM_CXSCREEN).max(640);
        let height = GetSystemMetrics(SM_CYSCREEN).max(480);

        let title = create_control(
            hwnd,
            "STATIC",
            &format!("BackupRestore 恢复桌面  v{PROGRAM_VERSION}"),
            SS_CENTER,
            0,
            36,
            width,
            64,
            ID_PE_TITLE,
        );
        let card_width = 280;
        let card_height = 108;
        let gap = 36;
        let grid_width = card_width * 3 + gap * 2;
        let start_x = ((width - grid_width) / 2).max(0);
        // 七个入口排成 3 列 3 行（末行只占 1 个），行高按 3 行计算才不会
        // 让最后一行压到底栏上。
        let start_y = ((height - (card_height * 3 + gap * 2 + 150)) / 2).max(16) + 24;
        let cards: [(usize, &str, i32, i32); 7] = [
            (ID_PE_BACKUP, "备份系统", 0, 0),
            (ID_PE_RESTORE, "还原系统", 1, 0),
            (ID_PE_SECONDARY, "安装第二系统", 2, 0),
            (ID_PE_CMD, "命令提示符", 0, 1),
            (ID_PE_EXIT, "返回 Windows", 1, 1),
            // 「重启」已合并进「返回 Windows」，此位改为打开完整主程序 GUI
            (ID_PE_MAIN_GUI, "打开完整程序", 2, 1),
            (ID_PE_SYSINFO, "软硬件信息", 0, 2),
        ];
        let mut card_controls = Vec::with_capacity(cards.len());
        for (id, text, column, row) in cards {
            let x = start_x + column * (card_width + gap);
            let y = start_y + row * (card_height + gap);
            card_controls.push(create_control(
                hwnd,
                "BUTTON",
                text,
                WS_TABSTOP,
                x,
                y,
                card_width,
                card_height,
                id,
            ));
        }
        let bar_y = height - 44;
        let version = create_control(
            hwnd,
            "STATIC",
            // 底栏原先固定写死 WinPE，经 reagentc /boottore 进入 WinRE 时标签不实；
            // 改为实际探测到的环境。
            &format!(
                "BackupRestore v{PROGRAM_VERSION}  |  {}",
                sysinfo_environment_label()
            ),
            0,
            16,
            bar_y,
            360,
            30,
            ID_PE_VERSION,
        );
        let clock = create_control(
            hwnd,
            "STATIC",
            "--:--:--",
            SS_CENTER,
            width - 220,
            bar_y,
            200,
            30,
            ID_PE_CLOCK,
        );

        SendMessageW(title, WM_SETFONT, title_font as WParam, 1);
        for control in &card_controls {
            SendMessageW(*control, WM_SETFONT, card_font as WParam, 1);
        }
        SendMessageW(version, WM_SETFONT, bar_font as WParam, 1);
        SendMessageW(clock, WM_SETFONT, bar_font as WParam, 1);

        // PE 桌面包卡按钮的气泡提示：鼠标悬停显示各按钮用途说明。
        let tooltip = CreateWindowExW(
            WS_EX_TOPMOST,
            wide("tooltips_class32").as_ptr(),
            null(),
            WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
            0,
            0,
            0,
            0,
            hwnd,
            null_mut(),
            null_mut(),
            null_mut(),
        );
        if !tooltip.is_null() {
            for &(id, text) in &[
                (
                    ID_PE_BACKUP,
                    "备份系统：把选中的源分区制作成 WIM 镜像（离线 DISM 捕获），完成后返回 Windows。",
                ),
                (
                    ID_PE_RESTORE,
                    "还原系统：格式化目标分区并把镜像写回、重建引导（危险操作，请先确认目标分区与重要数据）。",
                ),
                (
                    ID_PE_SECONDARY,
                    "安装第二系统：把镜像部署到另一个分区并新增启动项，保留现有 Windows。",
                ),
                (
                    ID_PE_CMD,
                    "命令提示符：打开 PE 环境里的 cmd，可手动执行维护命令。",
                ),
                (
                    ID_PE_EXIT,
                    "返回 Windows：恢复引导启动到当前 Windows 并自动重启。",
                ),
                (
                    ID_PE_MAIN_GUI,
                    "打开完整程序：启动完整的多页签界面（与 Windows 下相同），可执行更多操作。",
                ),
                (
                    ID_PE_SYSINFO,
                    "软硬件信息：只读采集本机操作系统、CPU/内存、主板固件、显示、存储、网络与恢复安全状态，不修改任何配置。",
                ),
            ] {
                let control = GetDlgItem(hwnd, id as i32);
                add_tooltip(tooltip, hwnd, control, text);
            }
        }

        let state = Box::new(PeDesktopState {
            clock,
            fonts: [title_font, card_font, bar_font],
            tooltip,
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        SetTimer(hwnd, PE_TIMER_ID, 1000, None);
        return 0;
    }
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PeDesktopState;
    if !state_ptr.is_null() {
        let state = &mut *state_ptr;
        if message == WM_COMMAND {
            let control_id = w_param & 0xffff;
            match control_id {
                ID_PE_BACKUP => {
                    // PE 内直接执行备份（对话框确认后 dism 捕获，不再跳主 GUI）
                    pe_backup_from_desktop(hwnd);
                    return 0;
                }
                ID_PE_RESTORE => {
                    // PE 内直接执行还原（格式化+Apply+BCDBoot）
                    pe_restore_from_desktop(hwnd);
                    return 0;
                }
                ID_PE_SECONDARY => {
                    // PE 内直接安装第二系统（Apply+BCD 追加）
                    pe_secondary_from_desktop(hwnd);
                    return 0;
                }
                ID_PE_EXIT => {
                    // Return to the main Windows installation: find the EFI
                    // system partition that carries BCD, mount it, restore the
                    // boot manager default to {current}, then reboot. Without
                    // this step a PE set as the default would loop back into
                    // PE forever.
                    exit_pe_to_windows(hwnd);
                    return 0;
                }
                ID_PE_SYSINFO => {
                    // 只读采集并展示软硬件信息，不修改系统、不启动任务。
                    pe_system_info_from_desktop(hwnd);
                    return 0;
                }
                ID_PE_CMD => {
                    let cmd = wide("cmd.exe");
                    ShellExecuteW(hwnd, null(), cmd.as_ptr(), null(), null(), SW_SHOW);
                    return 0;
                }
                ID_PE_MAIN_GUI => {
                    // 打开完整主程序 GUI（多 tab 界面，与 Windows 下相同）。
                    // 注意：当前 exe 名为 Recovery.exe，不满足 main.rs
                    // should_launch_gui 的 "BackupRestore" 检查，无参数启动
                    // 不会进 GUI；必须带 --tab 1（备份页）直接进入。
                    let executable = std::env::current_exe().unwrap_or_else(|_| {
                        std::path::PathBuf::from("X:\\Windows\\System32\\Recovery.exe")
                    });
                    let exe_wide = wide(&executable.to_string_lossy());
                    let argument = wide("--tab 1");
                    ShellExecuteW(
                        hwnd,
                        null(),
                        exe_wide.as_ptr(),
                        argument.as_ptr(),
                        null(),
                        SW_SHOW,
                    );
                    return 0;
                }
                _ => {}
            }
        }
        if message == WM_TIMER && w_param == PE_TIMER_ID {
            update_pe_clock(state);
            return 0;
        }
        if message == WM_CTLCOLORSTATIC || message == WM_CTLCOLORBTN {
            let hdc = l_param as Handle;
            SetBkMode(hdc, 1);
            SetTextColor(hdc, PE_TITLE_TEXT);
            return PE_BACKGROUND as LResult;
        }
        if message == WM_DESTROY {
            KillTimer(hwnd, PE_TIMER_ID);
            if !state.tooltip.is_null() {
                DestroyWindow(state.tooltip);
            }
            for font in state.fonts {
                if !font.is_null() {
                    DeleteObject(font);
                }
            }
            drop(Box::from_raw(state_ptr));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            PostQuitMessage(0);
            return 0;
        }
    }
    DefWindowProcW(hwnd, message, w_param, l_param)
}

/// 自动请求只领取一次；全部成功才返回重启许可，失败保留证据。
/// 旧文本任务保留原件，绝不解释为可重放命令。
fn pe_task_execute() -> bool {
    let result = crate::pe_operation::execute_pending();
    match result {
        Ok(completed) => completed,
        Err(error) => {
            unsafe {
                show_message(
                    null_mut(),
                    &format!(
                        "自动任务未执行或未完成：{error}\n已保留任务与状态，请检查后重新安排。"
                    ),
                    "PE 自动任务已停止",
                    MB_OK | MB_ICONERROR,
                );
            }
            false
        }
    }
}

pub(crate) unsafe fn run_pe_desktop() -> Result<Option<usize>, TaskError> {
    unsafe {
        let common_controls = InitCommonControlsEx {
            size: size_of::<InitCommonControlsEx>() as u32,
            classes: ICC_WIN95_CLASSES,
        };
        InitCommonControlsEx(&common_controls);
        if !is_elevated() {
            relaunch_elevated()?;
            return Ok(None);
        }
        close_previous_gui_windows();
        // 领取类型化请求；失败停留桌面，无请求则等待用户操作。
        let auto_reboot = pe_task_execute();
        if auto_reboot {
            pe_reboot_with_fallback();
            return Ok(None);
        }
        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            return Err(err("GetModuleHandleW failed"));
        }
        let background = CreateSolidBrush(PE_BACKGROUND);
        let class_name = wide("BackupRestorePeDesktop");
        let class = WndClassExW {
            cb_size: size_of::<WndClassExW>() as u32,
            style: 0,
            wnd_proc: Some(window_proc_pe),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: instance,
            h_icon: null_mut(),
            h_cursor: null_mut(),
            h_brush: background,
            menu_name: null(),
            class_name: class_name.as_ptr(),
            h_icon_sm: null_mut(),
        };
        if RegisterClassExW(&class) == 0 {
            return Err(err("RegisterClassExW failed (PE desktop)"));
        }
        PE_EXIT_TAB.store(0, std::sync::atomic::Ordering::SeqCst);
        let width = GetSystemMetrics(SM_CXSCREEN).max(640);
        let height = GetSystemMetrics(SM_CYSCREEN).max(480);
        let title = wide(&format!(
            "BackupRestore - PE Recovery Desktop v{PROGRAM_VERSION}"
        ));
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            width,
            height,
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        );
        if window.is_null() {
            // 【2026-10-17 第二轮核查，逃生 C】PE 是 **shell-free** 的
            // （`winpeshl.ini` 直接拉起本程序）：建窗失败后直接 `return Err`
            // 的话，屏幕上什么都没有、没有任务栏也没有命令行，而一次性启动项
            // 仍指向 PE —— 硬断电重启又会再进 PE，真正的死循环。
            // 这里先开一个最大化的命令提示符当手动出口，再让用户选去留。
            let _ = append_log(
                &std::path::PathBuf::from(r"X:\BackupRestore-pe-desktop.log"),
                "PE desktop: CreateWindowExW failed; offering a console escape hatch",
            );
            let _ = launch_background_console();
            let answer = message_box_standalone(
                "PE 恢复桌面创建窗口失败。\n\
                 已打开一个**最大化**的命令提示符窗口，可用它手动处理\n\
                 （例如 bcdedit /deletevalue {bootmgr} bootsequence、wpeutil reboot）。\n\n\
                 是 = 立即重启（wpeutil reboot）\n\
                 否 = 留在 PE，用命令提示符手动处理\n\n\
                 PE desktop failed to create its window. A maximized console has been\n\
                 opened for manual recovery. Yes = reboot now; No = stay in PE.",
                "PE 桌面启动失败 / PE desktop failed to start",
                MB_YESNO | MB_ICONERROR,
            );
            if answer == IDYES {
                pe_reboot_with_fallback();
            }
            return Err(err("CreateWindowExW failed (PE desktop)"));
        }
        ShowWindow(window, SW_SHOW);
        // 旧 pe-click 文本未绑定目标身份，不再自动授权破坏性操作。
        let mut message = Msg {
            hwnd: null_mut(),
            message: 0,
            w_param: 0,
            l_param: 0,
            time: 0,
            point: Point { x: 0, y: 0 },
        };
        loop {
            let result = GetMessageW(&mut message, null_mut(), 0, 0);
            if result <= 0 {
                break;
            }
            // PE 桌面按钮同样支持 Tab 遍历 / 回车触发 / 方向键。
            if IsDialogMessageW(window, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let exit_tab = match PE_EXIT_TAB.load(std::sync::atomic::Ordering::SeqCst) {
            1 => Some(1),
            2 => Some(2),
            3 => Some(3),
            _ => None,
        };
        Ok(exit_tab)
    }
}
