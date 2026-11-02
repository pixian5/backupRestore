//! 驱动器、WIM 镜像及引导项名称选择控制器
//!
//! 负责：
//! - 驱动器枚举、下拉框项填充、角色描述与详细参数格式化；
//! - 目标卷过滤视图（排除当前系统卷）与真实索引安全换算；
//! - WIM 镜像索引列表解析、格式化与下拉框交互；
//! - 注册表 Windows 版本探测与备份索引名/引导菜单名默认值智能生成；
//! - BCD 既有条目名称预检与重名规避。

use super::*;

/// C 入口（新增非 C 盘第二系统）目标卷枚举视图：排除当前系统盘。
///
/// combo 索引与 `state.drives` 索引的默认映射是恒等的；过滤开启后所有
/// CB_GETCURSEL/CB_SETCURSEL 都必须经本视图换算，否则 combo 索引会错位到
/// 别的卷——「选了 E: 实际写到 F:」比不过滤更危险。返回 None = 不过滤
/// （恒等映射，其余入口保持原行为）。
pub(crate) unsafe fn target_combo_indices(state: &State) -> Option<Vec<usize>> {
    if selected_operation(state) != "create-secondary" {
        return None;
    }
    let system = active_system_letter();
    Some(
        state
            .drives
            .iter()
            .enumerate()
            .filter(|(_, drive)| !drive.letter.eq_ignore_ascii_case(&system))
            .map(|(index, _)| index)
            .collect(),
    )
}

/// 目标 combo 在当前入口下的期望项数（与 set_drive_items 填充结果对称）：
/// 入口视图空/无卷时下拉里是 1 个占位提示项，否则等于视图大小。
pub(crate) unsafe fn target_combo_expected_count(state: &State) -> usize {
    match target_combo_indices(state) {
        Some(view) if !view.is_empty() => view.len(),
        Some(_) => 1,
        None if state.drives.is_empty() => 1,
        None => state.drives.len(),
    }
}

/// 盘符 → target combo 项索引（经过滤视图换算；view=None 直查 drives）。
pub(crate) fn drive_combo_position(view: Option<&[usize]>, state: &State, letter: &str) -> Option<usize> {
    match view {
        Some(view) => view
            .iter()
            .copied()
            .position(|index| state.drives[index].letter.eq_ignore_ascii_case(letter)),
        None => state
            .drives
            .iter()
            .position(|drive| drive.letter.eq_ignore_ascii_case(letter)),
    }
}

pub(crate) unsafe fn selected_drive_letter(state: &State, control: Hwnd) -> Option<String> {
    let index = combo_selection(control)?;
    let drive_index = if control == state.controls.target {
        match target_combo_indices(state) {
            Some(view) => *view.get(index)?,
            None => index,
        }
    } else {
        index
    };
    state.drives.get(drive_index).map(|drive| drive.letter.clone())
}

pub(crate) fn drive_display(drive: &DriveInfo, language: Language) -> String {
    let label = if drive.label.is_empty() {
        if language == Language::English {
            "no label"
        } else {
            "无卷标"
        }
    } else {
        &drive.label
    };
    let disk = drive
        .disk_number
        .map(|number| number.to_string())
        .unwrap_or_else(|| "?".to_string());
    let partition = drive
        .partition_number
        .map(|number| number.to_string())
        .unwrap_or_else(|| "?".to_string());
    let windows_marker = if drive.has_windows_installation {
        " | Windows"
    } else {
        ""
    };
    if language == Language::English {
        format!(
            "{}: | {} | {} | total {} | free {} | disk {}/partition {}{}",
            drive.letter,
            drive.filesystem,
            label,
            format_bytes(drive.size_bytes),
            format_bytes(drive.free_bytes),
            disk,
            partition,
            windows_marker,
        )
    } else {
        format!(
            "{}: | {} | 卷标 {} | 总容量 {} | 可用 {} | 磁盘 {}/分区 {}{}",
            drive.letter,
            drive.filesystem,
            label,
            format_bytes(drive.size_bytes),
            format_bytes(drive.free_bytes),
            disk,
            partition,
            windows_marker,
        )
    }
}

pub(crate) fn drive_details(drive: &DriveInfo, language: Language) -> String {
    let disk = drive
        .disk_number
        .map(|number| number.to_string())
        .unwrap_or_else(|| "?".to_string());
    let partition = drive
        .partition_number
        .map(|number| number.to_string())
        .unwrap_or_else(|| "?".to_string());
    if language == Language::English {
        format!(
            "{}: {}\nFile system: {}\r\nWindows installation: {}\r\nTotal: {} | Free: {}\r\nDisk/partition: {}/{}\r\nPartition type: {}\r\nVolume GUID: {}",
            drive.letter,
            if drive.label.is_empty() {
                "no label"
            } else {
                &drive.label
            },
            drive.filesystem,
            if drive.has_windows_installation {
                "yes"
            } else {
                "no"
            },
            format_bytes(drive.size_bytes),
            format_bytes(drive.free_bytes),
            disk,
            partition,
            drive.partition_type_guid,
            drive.volume_guid,
        )
    } else {
        format!(
            "{}: {}\n文件系统：{}\r\nWindows 安装：{}\r\n总容量：{} | 可用：{}\r\n磁盘/分区：{}/{}\r\n分区类型：{}\r\n卷 GUID：{}",
            drive.letter,
            if drive.label.is_empty() {
                "无卷标"
            } else {
                &drive.label
            },
            drive.filesystem,
            if drive.has_windows_installation {
                "是"
            } else {
                "否"
            },
            format_bytes(drive.size_bytes),
            format_bytes(drive.free_bytes),
            disk,
            partition,
            drive.partition_type_guid,
            drive.volume_guid,
        )
    }
}

pub(crate) fn drive_role_description(role: &str, operation: &str, language: Language) -> &'static str {
    match (language, role, operation) {
        (Language::Chinese, "source", "backup") => {
            "源卷用途：备份时从这里捕获 Windows 分区；镜像不能保存到这个卷。"
        }
        (Language::Chinese, "source", "restore-existing") => {
            "源卷用途：当前要替换的 Windows 分区；单系统还原会强制目标卷与它相同。"
        }
        (Language::Chinese, "source", _) => {
            "源卷用途：探测时只核验当前 Windows 分区身份；不会备份、还原或写入。"
        }
        (Language::Chinese, "target", "restore-existing") => {
            "目标卷用途：锁定为当前正在运行的系统盘；离线格式化后写回镜像，重建唯一系统。"
        }
        (Language::Chinese, "target", "restore-data") => {
            "目标卷用途：接收数据卷镜像。默认勾选「格式化目标分区」=清空后写入（卷序列号改变）；取消勾选=覆盖式应用（多余文件保留）。不能选当前系统盘或已有 Windows 的分区。"
        }
        (Language::Chinese, "target", "create-secondary") => {
            "目标卷用途：将被格式化并写入完整 Windows 作为第二系统，新增启动菜单项，完成后默认启动项切换为这台新系统（原系统仍可选）。下拉列表已排除当前系统盘（C:）。"
        }
        (Language::Chinese, "target", "backup") => {
            "目标卷用途：备份模式不会写入此卷；镜像保存位置由“镜像绝对路径”决定。"
        }
        (Language::Chinese, "target", _) => {
            "目标卷用途：探测模式只核验身份；不会备份、还原、格式化或写入。"
        }
        (Language::English, "source", "backup") => {
            "Purpose: backup captures the Windows partition here; the image cannot be stored on this volume."
        }
        (Language::English, "source", "restore-existing") => {
            "Purpose: current Windows partition to replace; single-system restore forces target to match it."
        }
        (Language::English, "source", _) => {
            "Purpose: probe only checks the current Windows volume identity; it never writes the volume."
        }
        (Language::English, "target", "restore-existing") => {
            "Purpose: locked to the running system drive; formatted offline and rewritten, rebuilding the only Windows."
        }
        (Language::English, "target", "restore-data") => {
            "Purpose: receives the data-volume image. 'Format target volume' checked (default) = erase then write (serial changes); unchecked = overlay apply. Never the system drive or an existing Windows partition."
        }
        (Language::English, "target", "create-secondary") => {
            "Purpose: formatted and written as a second Windows with a new boot entry; after the restore the default boot entry switches to this new system (the previous system stays selectable). The dropdown already excludes the current system drive (C:)."
        }
        (Language::English, "target", "backup") => {
            "Purpose: backup does not write this volume; image storage comes from Image absolute path."
        }
        (Language::English, "target", _) => {
            "Purpose: probe only checks identity; it never backs up, restores, formats or writes this volume."
        }
        _ => "",
    }
}

pub(crate) unsafe fn set_drive_details(state: &State) {
    let language = selected_language(state);
    let selected = [
        (
            "source",
            selected_drive_letter(state, state.controls.source),
        ),
        (
            "target",
            selected_drive_letter(state, state.controls.target),
        ),
    ];
    let detail_controls = [state.controls.source_details, state.controls.target_details];
    let operation = selected_operation(state);
    for (role, letter) in selected {
        let index = match role {
            "source" => 0,
            _ => 1,
        };
        let text = letter
            .and_then(|letter| state.drives.iter().find(|item| item.letter == letter))
            .map(|drive| {
                format!(
                    "{}\n\n{}",
                    drive_role_description(role, operation, language),
                    drive_details(drive, language)
                )
            })
            .unwrap_or_else(|| {
                if language == Language::English {
                    format!(
                        "{}\n\nNo volume selected.",
                        drive_role_description(role, operation, language)
                    )
                } else {
                    format!(
                        "{}\n\n未选择卷。",
                        drive_role_description(role, operation, language)
                    )
                }
            });
        set_text(detail_controls[index], &text);
    }
}

pub(crate) unsafe fn set_drive_items(state: &State, desired: [Option<String>; 3]) {
    // 源卷：全量枚举（备份入口选备份来源；还原类不读它的选择）。
    reset_combo(state.controls.source);
    if state.drives.is_empty() {
        add_combo_item(
            state.controls.source,
            if selected_language(state) == Language::English {
                "No eligible mounted volume"
            } else {
                "没有可选择的已挂载卷"
            },
        );
        SendMessageW(state.controls.source, CB_SETCURSEL, 0, 0);
    } else {
        for drive in &state.drives {
            add_combo_item(
                state.controls.source,
                &drive_display(drive, selected_language(state)),
            );
        }
    }
    // 目标卷：C 入口排除当前系统盘（combo 索引经 target_combo_indices 换算）。
    let target_view = target_combo_indices(state);
    let target_positions: Vec<usize> = match &target_view {
        Some(view) if !view.is_empty() => view.clone(),
        // 唯一卷恰是当前系统盘：明确告知排除规则，不留静默空 combo。
        Some(_) => Vec::new(),
        None => (0..state.drives.len()).collect(),
    };
    reset_combo(state.controls.target);
    if state.drives.is_empty() {
        add_combo_item(
            state.controls.target,
            if selected_language(state) == Language::English {
                "No eligible mounted volume"
            } else {
                "没有可选择的已挂载卷"
            },
        );
        SendMessageW(state.controls.target, CB_SETCURSEL, 0, 0);
    } else if target_positions.is_empty() {
        add_combo_item(
            state.controls.target,
            if selected_language(state) == Language::English {
                "No eligible target (current system drive excluded)"
            } else {
                "没有可选目标卷（已排除当前系统盘）"
            },
        );
        SendMessageW(state.controls.target, CB_SETCURSEL, 0, 0);
    } else {
        for &drive_index in &target_positions {
            add_combo_item(
                state.controls.target,
                &drive_display(&state.drives[drive_index], selected_language(state)),
            );
        }
    }
    for (control, wanted, view) in [
        (state.controls.source, desired[1].clone(), None),
        (state.controls.target, desired[2].clone(), target_view.as_deref()),
    ] {
        let position = wanted
            .as_deref()
            .and_then(|letter| drive_combo_position(view, state, letter))
            .unwrap_or(0);
        SendMessageW(control, CB_SETCURSEL, position as usize, 0);
    }
}

pub(crate) unsafe fn select_drive(state: &State, control: Hwnd, letter: &str) {
    // 【2026-10-18 缺陷修复 H1】此前目标分支写作
    // `target_combo_indices(state).as_deref().and_then(|view| …)`：视图为 `None`
    // （除 C 入口外的**所有**入口，含 A）时 `and_then` 直接得到 `None`，
    // `CB_SETCURSEL` 从不发出。A 入口「自动选中当前系统盘」的意图因此落空——
    // 下拉停在上一个入口选中的盘，界面却写着「目标卷（当前系统盘 C:）」，
    // **显示卷 ≠ 操作卷**（prepare 只要求 source==target 即放行 → 格式化错盘）。
    // 现在与 `drive_combo_position` 完全同源：视图在就按视图换算（视图里找不到
    // 就保持不动，绝不回退全量索引），视图为 None 就用全量索引。
    let position = if control == state.controls.target {
        let view = target_combo_indices(state);
        drive_combo_position(view.as_deref(), state, letter)
    } else {
        drive_combo_position(None, state, letter)
    };
    if let Some(position) = position {
        SendMessageW(control, CB_SETCURSEL, position as usize, 0);
    }
}

pub(crate) unsafe fn selected_wim_index(state: &State) -> Option<u32> {
    if let Some(image) = state.wim_images.get(combo_index(state.controls.index)) {
        return Some(image.index);
    }
    get_text(state.controls.index)
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
}

pub(crate) fn parse_drive_infos(output: &str) -> Result<Vec<DriveInfo>, String> {
    let value: serde_json::Value = serde_json::from_str(output)
        .map_err(|error| format!("volume metadata JSON parse failed: {error}"))?;
    let items = if let Some(items) = value.as_array() {
        items.clone()
    } else if value.get("letter").is_some() {
        vec![value]
    } else {
        return Err("volume metadata did not return a volume object or array".to_string());
    };
    let mut drives = Vec::with_capacity(items.len());
    for item in items {
        let letter = json_text(&item, "letter")
            .trim()
            .trim_end_matches(':')
            .to_ascii_uppercase();
        if letter.len() != 1 || !letter.as_bytes()[0].is_ascii_alphabetic() {
            continue;
        }
        drives.push(DriveInfo {
            letter,
            label: json_text(&item, "label"),
            filesystem: json_text(&item, "filesystem"),
            size_bytes: json_text(&item, "sizeBytes").parse::<u64>().ok(),
            free_bytes: json_text(&item, "freeBytes").parse::<u64>().ok(),
            volume_guid: json_text(&item, "volumeGuid"),
            disk_number: json_text(&item, "diskNumber").parse::<u32>().ok(),
            partition_number: json_text(&item, "partitionNumber").parse::<u32>().ok(),
            partition_type_guid: json_text(&item, "partitionTypeGuid"),
            has_windows_installation: json_text(&item, "hasWindowsInstallation")
                .eq_ignore_ascii_case("true"),
        });
    }
    drives.sort_by(|left, right| left.letter.cmp(&right.letter));
    drives.dedup_by(|left, right| left.letter == right.letter);
    if drives.is_empty() {
        return Err("no eligible mounted volumes were returned".to_string());
    }
    Ok(drives)
}

pub(crate) fn report_images_value(report: &serde_json::Value) -> serde_json::Value {
    report
        .get("images")
        .cloned()
        .or_else(|| {
            if report.get("ImageIndex").is_some() || report.get("imageIndex").is_some() {
                Some(report.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()))
}

pub(crate) fn wim_display(image: &WimImageInfo, language: Language) -> String {
    let name = if image.name.is_empty() {
        if language == Language::English {
            "(unnamed)"
        } else {
            "（未命名）"
        }
    } else {
        &image.name
    };
    let description = if image.description.is_empty() {
        if language == Language::English {
            "no description"
        } else {
            "无描述"
        }
    } else {
        &image.description
    };
    let version = if image.version.is_empty() {
        "?"
    } else {
        &image.version
    };
    let architecture = if image.architecture.is_empty() {
        "?"
    } else {
        &image.architecture
    };
    let edition = if image.edition.is_empty() {
        "?"
    } else {
        &image.edition
    };
    let install_type = if image.installation_type.is_empty() {
        "?"
    } else {
        &image.installation_type
    };
    if language == Language::English {
        format!(
            "Index {} | {} | {} | Version {} | Arch {} | Edition {} | Install {} | Size {}",
            image.index,
            name,
            description,
            version,
            architecture,
            edition,
            install_type,
            format_bytes(image.size_bytes),
        )
    } else {
        format!(
            "索引 {} | {} | {} | 版本 {} | 架构 {} | 版本类型 {} | 大小 {} | 安装类型 {}",
            image.index,
            name,
            description,
            version,
            architecture,
            edition,
            format_bytes(image.size_bytes),
            install_type,
        )
    }
}

pub(crate) unsafe fn set_wim_items(state: &State) {
    let language = selected_language(state);
    let selected = selected_wim_index(state);
    reset_combo(state.controls.index);
    if state.wim_images.is_empty() {
        add_combo_item(
            state.controls.index,
            if language == Language::English {
                "No image metadata loaded (click Read image)"
            } else {
                "尚未读取镜像信息（请点击“读取镜像”）"
            },
        );
        SendMessageW(state.controls.index, CB_SETCURSEL, 0, 0);
        return;
    }
    let mut selected_position = 0usize;
    for (position, image) in state.wim_images.iter().enumerate() {
        if selected == Some(image.index) {
            selected_position = position;
        }
        add_combo_item(state.controls.index, &wim_display(image, language));
    }
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 1020,
        bottom: 760,
    };
    GetClientRect(state.root, &mut rect);
    // The open list must expose each index's full metadata, even when the
    // selected one-line control shares a row with a secondary-system name.
    // ★ 2.6.79：客户区换算回 96dpi 基准再减常量，最后整体缩放成物理像素。
    let dropdown_width =
        crate::dpi::s((crate::dpi::unscale(rect.right - rect.left) - 204).max(440)) as usize;
    SendMessageW(state.controls.index, CB_SETDROPPEDWIDTH, dropdown_width, 0);
    SendMessageW(state.controls.index, CB_SETCURSEL, selected_position, 0);
}

/// S5：把选中 WIM 索引的名字预填到「启动菜单名称」文本框。
///
/// 名字取自 DISM 的 ImageName——我们自己的备份是索引名（如「2026-10-14
/// 21:40」），第三方 WIM 是「Windows 11 Pro」之类。只在用户没有手动改过时
/// 覆盖（`menu_name_touched`）：用户意图优先于自动预填。
pub(crate) unsafe fn prefill_menu_name_from_image(state: &mut State) {
    if state.menu_name_touched {
        return;
    }
    if !matches!(
        selected_operation(state),
        "create-secondary"
    ) {
        return;
    }
    // 先取owned名字再写文本框：image 的借用不能跨过 set_menu_name 的 &mut。
    let name = state
        .wim_images
        .get(combo_index(state.controls.index))
        .map(|image| image.name.trim().to_string());
    let Some(name) = name else {
        return;
    };
    if name.is_empty() {
        return;
    }
    set_menu_name(state, &name);
}

/// 程序侧写菜单名（区别于用户输入）：同步记录自动值，WM_COMMAND 的
/// EN_CHANGE 据此区分「程序预填」与「用户手改」——SetWindowText 自己也会
/// 触发 EN_CHANGE，没有这个对照就会把程序自己的预填误判成用户输入。
///
/// 顺序要紧：必须先写 `menu_name_auto` 再 `set_text`。EDIT 的 WM_SETTEXT
/// 会同步（SendMessage）向父窗发 EN_CHANGE，处理函数在同一调用栈上重入；
/// 若照「先写文本后记自动值」的直觉顺序，重入时 auto 还是上一个值（启动
/// 首次甚至是 None），当前文本已变 → 差值非空 → `menu_name_touched` 被
/// 永久置 true，此后读取镜像/切索引的预填全部被跳过（2026-10-15 实机
/// 复现：菜单框永远停留在语言默认值，S5 预填整个失效）。
pub(crate) unsafe fn set_menu_name(state: &mut State, value: &str) {
    state.menu_name_auto = Some(value.to_string());
    set_text(state.controls.menu, value);
}

/// 读 HKLM 下的 REG_SZ 值。读不到或类型不符返回 `None`——版本信息只是给索引名
/// 起个好名字，缺了要有降级方案，绝不能因此阻断建窗。
pub(crate) unsafe fn reg_read_string(sub_key: &str, value: &str) -> Option<String> {
    let key = wide(sub_key);
    let name = wide(value);
    let mut buffer = vec![0u16; 512];
    let mut size = (buffer.len() * size_of::<u16>()) as u32;
    let status = RegGetValueW(
        HKEY_LOCAL_MACHINE,
        key.as_ptr(),
        name.as_ptr(),
        RRF_RT_REG_SZ,
        null_mut(),
        buffer.as_mut_ptr().cast(),
        &mut size,
    );
    if status != 0 {
        return None;
    }
    // 缓冲区未被填满的部分是 0，去掉尾部 NUL 再 trim。
    let text = String::from_utf16_lossy(&buffer)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

/// 读 HKLM 下的 REG_DWORD 值（`UBR` 补丁级别就是 DWORD）。
pub(crate) unsafe fn reg_read_dword(sub_key: &str, value: &str) -> Option<u32> {
    let key = wide(sub_key);
    let name = wide(value);
    let mut data: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let status = RegGetValueW(
        HKEY_LOCAL_MACHINE,
        key.as_ptr(),
        name.as_ptr(),
        RRF_RT_REG_DWORD,
        null_mut(),
        (&mut data as *mut u32).cast(),
        &mut size,
    );
    (status == 0).then_some(data)
}

/// `ProductName` 去掉尾部 edition 词：`Windows 11 Pro` → `Windows 11`，
/// `Windows Server 2022 Datacenter` → `Windows Server 2022`。
/// 词数不足三个时不拆（避免把 `Windows 11` 砍成 `Windows`）。
pub(crate) fn trim_edition(product: &str) -> String {
    let words: Vec<&str> = product.split_whitespace().collect();
    if words.len() >= 3 {
        words[..words.len() - 1].join(" ")
    } else {
        product.trim().to_string()
    }
}

/// 当前系统的版本串，如 `Windows 11 26200.9550`。
///
/// ⚠️ 主版本**不能只信 `ProductName`**：实测本机是 Windows 11 25H2（build
/// 26200.9550、DisplayVersion 25H2），注册表 `ProductName` 却仍写着
/// `Windows 10 Pro`——跨版本升级/预览通道常见。所以桌面版按 build 号判定
/// （≥22000 = Windows 11，≥10240 = Windows 10），这也是通行做法；
/// Server 分支不受影响（ProductName 里的 "Server" 优先，且它的 build 号
/// 与桌面版不同源，不能套桌面版阈值）。
pub(crate) unsafe fn current_windows_version_tag() -> Option<String> {
    let product = reg_read_string(CURRENT_VERSION_KEY, "ProductName")?;
    let build = reg_read_string(CURRENT_VERSION_KEY, "CurrentBuildNumber");
    let build_number = build
        .as_deref()
        .and_then(|value| value.trim().parse::<u32>().ok());
    let ubr = reg_read_dword(CURRENT_VERSION_KEY, "UBR");
    let family = if product.to_ascii_lowercase().contains("server") {
        trim_edition(&product)
    } else {
        match build_number {
            Some(number) if number >= 22000 => "Windows 11".to_string(),
            Some(number) if number >= 10240 => "Windows 10".to_string(),
            // build 读不到才退回 ProductName：宁可信息旧一点，也不要凭空造。
            _ => trim_edition(&product),
        }
    };
    let version = match (build, ubr) {
        (Some(build), Some(ubr)) => format!("{build}.{ubr}"),
        (Some(build), None) => build,
        // 连 build 都读不到就只给产品名，不再拼半个版本号。
        (None, _) => return Some(family),
    };
    Some(format!("{family} {version}"))
}

/// 备份索引名默认值：`系统版本 + 年月日`，例 `Windows 11 26200.9550 20261015`
/// （2026-10-15 用户需求）。
///
/// **只在源卷就是当前系统盘时才带版本**：别的卷的版本要挂离线 hive 才读得到，
/// 代价高；而把当前系统的版本安在别的卷头上是误导——索引名写进 WIM 就改不了。
/// 那种情况退化成带盘符的名字，信息少但不假。
pub(crate) unsafe fn default_backup_index_name(source_is_system: bool, letter: Option<&str>) -> String {
    let mut now = SystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    GetLocalTime(&mut now);
    let date = format!("{:04}{:02}{:02}", now.year, now.month, now.day);
    if !source_is_system {
        return match letter {
            Some(letter) => format!("Windows ({letter}:) {date}"),
            None => format!("Windows {date}"),
        };
    }
    match current_windows_version_tag() {
        Some(tag) => format!("{tag} {date}"),
        None => format!("Windows {date}"),
    }
}

/// 程序侧写备份索引名：同步记录自动值，EN_CHANGE 据此区分「程序预填」与
/// 「用户手改」——理由与顺序要求完全同 `set_menu_name`（先记 auto 再写文本）。
pub(crate) unsafe fn set_index_name(state: &mut State, value: &str) {
    state.index_name_auto = Some(value.to_string());
    set_text(state.controls.index_name, value);
}

/// 源卷确定后重算备份索引名；用户手改过就再也不动它。
pub(crate) unsafe fn refresh_backup_index_name(state: &mut State) {
    if state.index_name_touched {
        return;
    }
    let letter = selected_drive_letter(state, state.controls.source);
    // `%SystemDrive%` 取不到时 `system` 是 None，此时如实判定为"不是系统卷"
    // 而不是退回字母 C——备份索引名的默认值会因此降级成带盘符的形式
    // （`Windows (F:) 日期`），宁可少写也不要写错。
    let system = system_drive_letter();
    let source_is_system = letter
        .as_deref()
        .and_then(|letter| letter.chars().next())
        .and_then(|letter| system.map(|system| letter.to_ascii_uppercase() == system))
        .unwrap_or(false);
    let value = default_backup_index_name(source_is_system, letter.as_deref());
    if state.index_name_auto.as_deref() == Some(value.as_str()) {
        return;
    }
    set_index_name(state, &value);
}

/// S5：读当前 BCD 里所有启动项名字（撞名预检的数据源）。
///
/// 走 CLI 子命令 `list-boot-entries`：`bcdedit /enum` 是读操作，普通权限即可，
/// 而 GUI 常态未提权（package 自启副本）。读不到返回空表——按「无法预检」
/// 处理并记日志，承重消解在提权后的 prepare 侧（docs/20261015-0725）。
pub(crate) fn existing_boot_entry_names(state: &State) -> Vec<String> {
    match rust_cli_output(&["list-boot-entries"]) {
        Ok(output) => {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&output) else {
                append_gui_log(state, "boot entries JSON parse failed; precheck skipped");
                return Vec::new();
            };
            // 【2026-10-16 审查修订，日志缺口 10】`bcdedit /enum` 失败时 CLI 会在
            // JSON 里带一个 `error` 字段。此前失败与"真的没有启动项"都表现为空
            // 列表，撞名预检会**静默失效**——名字重名要到任务创建时才炸。
            if let Some(error) = value.get("error").and_then(|item| item.as_str()) {
                append_gui_log(
                    state,
                    &format!("boot entries probe failed; collision precheck skipped: {error}"),
                );
            }
            value
                .get("descriptions")
                .and_then(|items| items.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        }
        Err(error) => {
            append_gui_log(
                state,
                &format!("boot entries unavailable; collision precheck skipped: {error}"),
            );
            Vec::new()
        }
    }
}

/// 压缩方式下拉显示文本。只保留「压缩/不压缩」两项，不再暴露 max/LZX。
/// 索引 0/1 固定对应 DISM 术语 fast/none（映射见 text_parsing）。
pub(crate) fn compress_level_labels(language: Language) -> [&'static str; 2] {
    if language == Language::Chinese {
        ["压缩", "不压缩"]
    } else {
        ["Compression", "No compression"]
    }
}

