//! 操作页签布局与切换控制器
//!
//! 负责：
//! - 操作入口定义与语义归一（A 还原系统至 C / B 还原数据 / C 新增第二系统 / PE 安装 / 备份）；
//! - 系统盘、EFI 引导卷及当前程序所在卷识别探测；
//! - 页签布局计算、实测文字算宽、控件可见性与引导文案切换；
//! - 主窗口控件重排（）与窗口重绘。

use super::*;

pub(crate) unsafe fn selected_operation(state: &State) -> &'static str {
    match state.operation_index {
        0 => "backup",
        // 三入口拆分：1=A 还原系统到 C 盘（沿用 restore-existing），
        // 2=B 还原数据到非 C 盘，3=C 还原系统到非 C 盘。
        1 => "restore-existing",
        2 => "restore-data",
        3 => "create-secondary",
        4 => "install-pe-entry",
        _ => "backup",
    }
}

/// GUI 入口键 → CLI `prepare --operation` 值。
/// B 复用 restore-existing（prepare 链要求 source==target，RE 内数据卷分支不碰引导）；
/// C 复用 create-secondary（目标≠活动系统、需菜单名、NewWindows 收尾）——
/// 2.4.9 起收尾按「目标性质」路由，operation 只表达语义，这里仅做入口归一。
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

/// 是否「镜像还原类」入口（A/B/C + 新增第二系统）：

/// 共用 WIM 索引必选、破坏性确认、副档校验、--allow-destructive 等闸门。
pub(crate) fn is_image_restore_entry(gui: &str) -> bool {
    matches!(
        gui,
        "restore-existing" | "restore-data" | "create-secondary"
    )
}

/// 「格式化目标分区」复选框的入口集合：A/B/C 三个还原入口。
/// set_operation_visibility 的可见性列表与 layout_operation 的重排条件
/// **必须共用本函数**：2026-10-17 修复的 C 页遮挡 bug 即因
/// v2.6.4 合并入口时可见性列表把 restore-system-other 改名 create-secondary、
/// 而 layout 的排除条件 `!= "create-secondary"` 漏改——复选框显示却不参与
/// 排版，停在后创建控件的下层被按钮行/详情框盖住。单一来源杜绝再次漂移。
pub(crate) fn is_format_target_entry(gui: &str) -> bool {
    matches!(
        gui,
        "restore-existing" | "restore-data" | "create-secondary"
    )
}

/// 当前活动系统盘符（`%SystemDrive%`，无冒号大写）。
/// GUI 侧只用于锁定 A 的目标、B/C 的目标预检——真正的卷身份比对在
/// prepare 的 P0 安全闸里做，这里不做安全承诺。
pub(crate) fn active_system_letter() -> String {
    std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".to_string())
        .trim_end_matches('\\')
        .trim_end_matches(':')
        .to_ascii_uppercase()
}

/// 当前活动系统盘符（`%SystemDrive%`，大写字母），**取不到就返回 `None`**。
///
/// 与 `active_system_letter` 的区别：那个在环境变量缺失时退回 `"C"`（只为界面
/// 显示一个盘符，错了无害）；这个用于**安全判定**，取不到时必须如实说"不知道"，
/// 绝不能退回字母 C——写死 C 会把"系统盘不是 C"的机器漏掉、把"C 是数据盘"的
/// 机器误拦，正是 `install_pe_harddisk` 里那处 `drive_char == 'C'` 的老问题。
pub(crate) fn system_drive_letter() -> Option<char> {
    std::env::var("SystemDrive")
        .ok()
        .and_then(|value| {
            value
                .trim_end_matches('\\')
                .trim_end_matches(':')
                .chars()
                .next()
        })
        .map(|letter| letter.to_ascii_uppercase())
}

/// 该卷是不是 EFI 系统分区（装引导文件的那个小分区）。
///
/// 判据是分区类型 GUID `EFI_TYPE`，不是盘符。
pub(crate) fn is_efi_system_partition(drive: Option<&DriveInfo>) -> bool {
    drive
        .map(|drive| {
            drive
                .partition_type_guid
                .eq_ignore_ascii_case(crate::windows_prepare::EFI_TYPE)
        })
        .unwrap_or(false)
}

/// 在运行本程序的那个卷上？程序所在卷不能被当成 PE 安装目标——装进去会覆盖
/// 正在运行的程序自身。
pub(crate) fn is_program_drive(drive_char: char, executable_dir: &std::path::Path) -> bool {
    match executable_dir.to_string_lossy().chars().next() {
        Some(letter) => letter.eq_ignore_ascii_case(&drive_char),
        // 拿不到程序路径时不猜：既不能误判成"是"，也不能假设成 C。
        None => false,
    }
}

pub(crate) unsafe fn set_operation_tabs(state: &State, language: Language) {
    for (index, (button, key)) in state
        .controls
        .operation_tabs
        .iter()
        .zip([
            "backup", "restore_a", "restore_b", "secondary", "pe",
        ])
        .enumerate()
    {
        set_text(*button, ui_text(language, key));
        append_gui_log(
            state,
            &format!("PE dev: tab[{index}] key={key} hwnd={button:?} text_set"),
        );
        SendMessageW(
            *button,
            BM_SETCHECK,
            if index == state.operation_index {
                BST_CHECKED
            } else {
                BST_UNCHECKED
            },
            0,
        );
    }
}

/// 「操作模式」那一排入口按钮（五个「按下去像页签」的单选按钮）的横向排布。
///
/// ## 为什么必须实测文字宽度，不能写死
///
/// ★ 2.6.83 修缺陷。这五个按钮的宽度是**创建时写死**的
/// （56 / 120 / 120 / 150 / 72，x 从 90 起每隔 3px 排下去），那是按 14 号字
/// 的中文文案估出来的。用户能在设置里把字号调到 16 / 18 之后，三个问题同时出现：
///
/// 1. 文字整体变宽，「新增非 C 盘第二系统」这种 9 字长文案直接顶出按钮矩形，
///    相邻两个按钮的字贴在一起，看上去像一串挤在一块的标签；
/// 2. 左边「操作模式」这个标签只有 62px 宽，18 号字下四个汉字要 70 多像素，
///    末字被切掉；
/// 3. 英文界面同理，而且更严重——「Add second Windows system off C:」
///    比任何中文文案都长。
///
/// 所以这里改成：**让 GDI 按当前字体把每段文案真正量一遍**（`measure_text_width`），
/// 加左右内边距当按钮宽度，再依次往右排。文案随语言切换、字号随用户选择变，
/// 量出来的宽度自然跟着变，不需要再维护一张「哪个语言多宽」的表。
///
/// 这一行的高度（32）不随字号变：18 号字行距是 21，装进 32 仍有余量，
/// 下面的控件 y 坐标链（`layout_operation` 里从 y=100 起）也就不必跟着改。
pub(crate) unsafe fn layout_operation_tabs(state: &State) {
    /// 文字左右各留的空白：按钮边框 + 一点视觉呼吸。太窄会让字贴着边框。
    const TAB_SIDE_PADDING: i32 = 9;
    // 注：`measure_text_width` 走的是 `GetTextExtentPoint32W`，量出来的就是
    // 真实绘制宽度（另加 4 px 防 ClearType 取整），这里只加视觉内边距。
    /// 相邻两个按钮之间的空隙。
    const TAB_GAP: i32 = 3;
    /// 最小宽度：「备份」这种两字文案也至少要有这么宽，否则看着不像按钮。
    const TAB_MIN_WIDTH: i32 = 56;
    const TOP: i32 = 60;
    const HEIGHT: i32 = 32;
    /// 「操作模式」标签与第一个按钮之间的空隙。
    const LABEL_GAP: i32 = 8;

    let move_to = |hwnd: Hwnd, x: i32, width: i32| {
        if !hwnd.is_null() {
            MoveWindow(
                hwnd,
                crate::dpi::s(x),
                crate::dpi::s(TOP),
                crate::dpi::s(width),
                crate::dpi::s(HEIGHT),
                1,
            );
        }
    };

    let mut x = 20;
    // 先排「操作模式」标签：它的宽度也按当前文案实测（中文 4 字 / 英文 "Operation"
    // 差别很大），不再固定 62。控件 id 2001 是 `WM_CREATE` 里创建的那个 STATIC。
    let label = GetDlgItem(state.root, 2001);
    if !label.is_null() {
        let text = get_text(label);
        let width = (measure_text_width(&text) + TAB_SIDE_PADDING * 2).max(40);
        move_to(label, x, width);
        x += width + LABEL_GAP;
    }
    for button in state.controls.operation_tabs.iter() {
        let text = get_text(*button);
        let width = (measure_text_width(&text) + TAB_SIDE_PADDING * 2).max(TAB_MIN_WIDTH);
        move_to(*button, x, width);
        x += width + TAB_GAP;
    }
    // 只在右边缘真的变了才记日志：`layout_operation` 每次 WM_SIZE 都会跑，
    // 无条件下日志会在拖拽窗口时把 GUI 日志刷满。
    let right_edge = x - TAB_GAP;
    static LAST_EDGE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
    if LAST_EDGE.swap(right_edge, std::sync::atomic::Ordering::SeqCst) != right_edge {
        append_gui_log(
            state,
            &format!("layout: operation tabs right edge={right_edge} (96dpi base)"),
        );
    }
}

pub(crate) fn operation_display(language: Language, operation: &str) -> &'static str {
    match (language, operation) {
        (Language::Chinese, "probe") => "探测（仅检查）",
        (Language::Chinese, "backup") => "备份",
        (Language::Chinese, "restore-existing") => "还原系统到 C 盘",
        (Language::Chinese, "restore-data") => "还原数据到非 C 盘",
        (Language::Chinese, "create-secondary") => "新增非 C 盘第二系统",
        (Language::Chinese, "install-pe-entry") => "安装 PE",
        (Language::English, "probe") => "Inspect",
        (Language::English, "backup") => "Backup",
        (Language::English, "restore-existing") => "Restore system to C:",
        (Language::English, "restore-data") => "Restore data to non-C:",
        (Language::English, "create-secondary") => "Add second system (non-C:)",
        (Language::English, "install-pe-entry") => "Install PE",
        _ => "",
    }
}

pub(crate) fn operation_hint_key(operation: &str) -> &'static str {
    match operation {
        "backup" => "backup_hint",
        "restore-existing" => "restore_hint",
        "restore-data" => "restore_b_hint",
        "create-secondary" => "secondary_hint",
        "install-pe-entry" => "pe_hint",
        _ => "backup_hint",
    }
}

pub(crate) unsafe fn set_operation_guidance(state: &State) {
    let language = selected_language(state);
    set_text(
        state.controls.status,
        ui_text(language, operation_hint_key(selected_operation(state))),
    );
}

pub(crate) unsafe fn set_operation_visibility(state: &State) {
    let operation = selected_operation(state);
    let show_image = operation != "probe";
    let show_target = matches!(
        operation,
        "restore-existing" | "restore-data" | "create-secondary"
            | "install-pe-entry"
    );
    // A/B/C/新增第二系统都要手选 WIM 索引 + 副档校验复选框。
    let show_index = is_image_restore_entry(operation);
    // 源卷行只在「备份」出现（选备份来源）。还原类一律隐藏：A/B 源=目标、
    // C 源恒为当前系统盘（active_system_letter()，创建任务处读取，不给用户
    // 改的机会），显示一个改不了的卷只会误导（v2.6.13 起 C 页移除源卷行）。
    let show_source = operation == "backup";
    // WIM 压缩方式（fast/none）只对备份首次创建有意义，仅「备份」tab 显示。
    let show_compress = operation == "backup";
    // 「第二系统名称」在「新增第二系统」与 C（他盘系统还原，离线 prepare 必填）
    // 显示；「安装 PE」tab 有自己的「PE 启动项名称」输入框，若此处也显示 menu，
    // 其布局位置（field_x+500, secondary_y）会恰好覆盖右下角「创建快捷方式」按钮。
    let show_menu = matches!(operation, "create-secondary");
    // B/C 入口显示「在线/离线」通道单选；A 必离线（无选择），备份走智能分流。
    let show_channel = matches!(operation, "restore-data" | "create-secondary");
    let set_visible = |hwnd: Hwnd, visible: bool| {
        ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
    };
    let set_child_visible = |id: i32, visible: bool| {
        set_visible(GetDlgItem(state.root, id), visible);
    };

    set_visible(state.controls.source, show_source);
    set_visible(state.controls.source_details, show_source);
    set_child_visible(2003, show_source);

    set_visible(state.controls.image, show_image);
    set_child_visible(ID_BROWSE_IMAGE as i32, show_image);
    set_child_visible(2004, show_image);

    set_visible(state.controls.index, show_index);
    set_visible(state.controls.verify_hash, show_index);
    set_child_visible(2007, show_index);
    set_visible(state.controls.menu, show_menu);
    set_child_visible(2008, show_menu);
    set_visible(state.controls.compress, show_compress);
    set_child_visible(2014, show_compress);
    // 备份索引名与保留最近 N 个：仅「备份」tab 显示（压缩率行下方新行）。
    set_visible(state.controls.index_name, show_compress);
    set_visible(state.controls.keep, show_compress);
    set_child_visible(2015, show_compress);
    set_child_visible(2016, show_compress);

    set_visible(state.controls.target, show_target);
    set_visible(state.controls.target_details, show_target);
    set_child_visible(2005, show_target);
    // A（还原系统到 C 盘）：目标锁定为当前系统盘，禁用下拉防误选；
    // 界面仍显示实际盘符（select_operation 会自动选中 %SystemDrive%）。
    EnableWindow(
        state.controls.target,
        if operation == "restore-existing" { 0 } else { 1 },
    );
    // B/C 的通道单选；「格式化目标分区」复选框在三个还原入口（A/B/C）都显示
    //（2026-10-14 用户拍板：所有还原都要有格式化选项，在线离线全通道生效）。
    set_child_visible(ID_CHANNEL_ONLINE as i32, show_channel);
    set_child_visible(ID_CHANNEL_OFFLINE as i32, show_channel);
    set_child_visible(ID_FORMAT_TARGET as i32, is_format_target_entry(operation));
    // 「重启进入 PE」「创建快捷方式」只在 安装 PE tab 显示
    set_child_visible(ID_PE_REBOOT_MAIN as i32, operation == "install-pe-entry");
    set_child_visible(ID_PE_SHORTCUT as i32, operation == "install-pe-entry");
    // 启动方式单选与目录行只在「安装 PE」tab 显示；硬盘启动模式下隐藏目录路径行
    let pe_mode_ram = IsDlgButtonChecked(state.root, ID_PE_MODE_RAM as i32) != 0;
    let pe_visible = operation == "install-pe-entry";
    append_gui_log(
        state,
        &format!("PE visibility: op={operation} pe_visible={pe_visible} ram_checked={pe_mode_ram}"),
    );
    set_child_visible(ID_PE_MODE_RAM as i32, pe_visible);
    set_child_visible(ID_PE_MODE_DISK as i32, pe_visible);
    // 「PE 启动项名称」输入框两种模式通用，在「安装 PE」tab 始终显示
    set_child_visible(ID_PE_NAME_LABEL as i32, pe_visible);
    set_child_visible(ID_PE_NAME_EDIT as i32, pe_visible);
    // PE 目录路径仅 RAM disk 模式需要（WIM 复制目录），硬盘启动模式隐藏
    set_child_visible(ID_PE_DIR_LABEL as i32, pe_visible && pe_mode_ram);
    set_child_visible(ID_PE_DIR_EDIT as i32, pe_visible && pe_mode_ram);
    set_child_visible(ID_PE_DIR_BROWSE as i32, pe_visible && pe_mode_ram);
}

/// 顶栏操作按钮（查看日志 / 备份文件管理… / 取消待重启任务 / 设置… / 语言）
/// 独立成一行，放在客户区顶部这条原本空着的窄带里。
///
/// ★ 2026-10-26 修缺陷：此前这 5 个控件与「操作模式」的入口行**同在 y=60**，
/// 两组横向总宽约 90+650(入口) + 4 按钮+语言约 750 = 远超任何窗口宽度，于是
/// 顶栏按钮被画在入口按钮上面。实机 GetWindowRect 量到 **6 对重叠**，最严重的是
/// 「新增非 C 盘第二系统」被「查看日志」盖掉 107×24 px（占该按钮宽度 71%）。
/// 当时的注释写着「最小窗口 1020 宽下两者不重叠」，而它只对「入口排到 ~741、
/// 语言栏从右边算起」成立——没算后来加进来的四个顶栏按钮。教训：两排控件共用
/// 一个 y 时，必须按**最坏宽度**（最小窗口 + 最长语言）核算，不能只看各自的起点。
pub(crate) const TOP_BAR_Y: i32 = 18;

/// 按文字实测给控件定宽：不少于 `floor`，并在测量值上加 12 px 余量。
///
/// ★ 2.6.92：主窗口原先有一批「按 14 号字调出来的固定宽度」控件（顶栏按钮、
///   右侧小标签、按钮行），字号调到 18 号时文字会被切——最严重的是英文
///   `Secondary boot name`：控件 100、文字要 189，切掉 89 像素。
///   字号是用户在设置里可调的，所以宽度必须跟着文字走，不能写死。
///   100% 与 200% 缩放下都成立（`measure_text_width` 返回的是 96dpi 基准值，
///   `reposition` 再统一乘系数）。
pub(crate) unsafe fn fit_width(text: &str, floor: i32) -> i32 {
    (measure_text_width(text) + 12).max(floor)
}

pub(crate) unsafe fn layout_operation(state: &State) {
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 1020,
        bottom: 760,
    };
    GetClientRect(state.root, &mut rect);
    // ★ 2.6.79：把实际客户区换算回 96dpi 基准空间，下面这一大段重排常量
    //   （右对齐的「客户宽 − 20 − 总宽」之类）一概不动，仍按 96dpi 书写；
    //   最终由 `reposition` 统一乘系数输出物理像素。
    //   为什么必须先换算：客户区读出来已经是放大后的值，若在物理量上再整体乘系数，
    //   右对齐会算出「(2040−20−241)×2」这种翻倍的错误位置。
    //   100% 下 `unscale` 是恒等，结果与改动前逐像素相同。
    let client_width = crate::dpi::unscale(rect.right - rect.left).max(1020);
    // 「操作模式」那一排页签的横向排布：宽度按当前字号/语言实测，见函数注释。
    // 放在这里而不是只在语言切换时调用，是因为**改字号**也要重排
    // （`apply_font_size` 也是走 `layout_operation`）。
    layout_operation_tabs(state);
    // 2.6.92：下面多处要按文字实测定宽，语言在这里取一次。
    let language = selected_language(state);
    let field_x = 180;
    let field_width = (client_width - field_x - 24).max(700);
    // ★ 高度按「**最坏行数 × 行距**」算，不能按某一页的行数算。
    // ★ 2.6.80：基准字号 12→14，行距随之 **14 px → 17 px**（tmHeight 14 + tmExternalLeading 3）。
    //   原行距 = SimSun 9pt 的 tmHeight 12 + tmExternalLeading 2 = 14 px（2026-10-26 实测：
    //   按 tmHeight 12 算会得出「9 行只要 108 px、装得下」，而屏幕上最后一行被切成半截）。
    // 最坏情况 = 「还原数据到非 C 盘」页的目标卷详情 **10 行**（该页设备详情有一行
    // 954 px > 可用 950 px，软换行多出一行）→ 需 140 px；EM_GETRECT 实测文本区比控件
    // 高少 4 px（上下边框各 2）→ 148 留 8 px 余量。其余页 9 行（=126 px）都在其内。
    // ★ 往 `drive_details` 加字段就要同步这里，否则又是一次「最后一行看不到」。
    // ★ 2.6.81：按**当前字号**算（最坏 10 行 + 8 余量），不再写死 178。
    //   字号可在设置里改，行距随之变，这里必须跟着变，否则最后一行被切。
    let details_height = crate::dpi::text_block_height(10, 8);
    let row_gap = 8;
    // 行可见性：源卷行仅「备份」显示（A/B/C 隐藏——C 源恒为当前系统盘，
    // v2.6.13 起从 C 页移除），通道单选行仅 B/C 显示。隐藏的行不占位，
    // 目标行相应上移。
    let layout_op = selected_operation(state);
    let source_visible = layout_op == "backup";
    let target_visible = matches!(
        layout_op,
        "restore-existing" | "restore-data" | "create-secondary"
            | "install-pe-entry"
    );
    let image_visible = layout_op != "probe";
    let index_visible = is_image_restore_entry(layout_op);
    let channel_visible = matches!(layout_op, "restore-data" | "create-secondary");
    let source_combo_y = 100;
    let source_details_y = source_combo_y + 36;
    let target_combo_y = if source_visible {
        source_details_y + details_height + row_gap
    } else {
        source_combo_y
    };
    let target_details_y = target_combo_y + 36;
    // 通道单选行：目标卷详情框下方；三个还原入口再加一行「格式化目标分区」复选框
    let channel_y = target_details_y + details_height + 8;
    let show_format_target = is_format_target_entry(layout_op);
    let format_y = channel_y + 26;
    let last_details_y = if show_format_target {
        format_y + 24
    } else if channel_visible {
        channel_y + 24
    } else if target_visible {
        target_details_y + details_height
    } else {
        source_details_y + details_height
    };
    // 「安装 PE」tab 的启动方式单选占三行（约 90px：单选行 + 启动项名行 + 目录行），后续 status 下移
    let pe_extra_y = if selected_operation(state) == "install-pe-entry" {
        90
    } else {
        0
    };
    // 状态提示框的高度：9 行文字 + 边框与余量，随字号变。
    // ★ 必须在这里先算出来：下面每一行的 y 都要让过它。
    //   2.6.80 把基准字号 12→14 之后这个高度从 148 涨到 178，但 `image_y` 仍按
    //   「status_y + 168」排（168 是「148 + 20 空隙」的老值），于是状态框把
    //   「镜像绝对路径」那一行的顶部压住了 10 个基准像素（200% 下 20 物理像素）。
    //   2026-10-30 用户截图指出「镜像绝对路径这一行顶部被稍微遮挡了」。
    //   教训与 2.6.83 同源：**高度一改，靠它算出来的每一个 y 都得重新审视**。
    let status_height = crate::dpi::text_block_height(9, 25);
    let status_y = last_details_y + 16 + pe_extra_y;
    // 状态框与下一行之间的空隙。原来是 20（= 168 − 148）；现在按实测高度算，
    // 8 已经够呼吸，也少往下推一些，免得整窗底部贴到屏幕边。
    let status_gap = 8;
    let image_y = status_y + status_height + status_gap;
    let secondary_y = image_y + 34;
    // C（还原系统到非 C 盘）与「新增第二系统」的 verify_hash 复选框在
    // secondary_y+32（底缘 +56）——按钮行必须让过它，留 8px 间隙。
    // （2.5.2 及此前按钮行在 +40，与复选框重叠 16px，「严格对比哈希」被挡住。）
    let buttons_y = if index_visible {
        if matches!(
            selected_operation(state),
            "create-secondary"
        ) {
            secondary_y + 64
        } else {
            secondary_y + 40
        }
    } else if selected_operation(state) == "backup" {
        // 备份时 secondary_y 行独占「压缩率」下拉，下一行是「索引名 + 保留最近 N 个」，
        // 按钮行再下移 30px 避免遮挡。
        secondary_y + 40 + 30
    } else if image_visible {
        image_y + 40
    } else {
        // 「探测」页没有镜像行，按钮直接跟在状态框下面——同样必须让过实测高度。
        status_y + status_height + status_gap
    };

    let reposition = |hwnd: Hwnd, x: i32, y: i32, width: i32, height: i32| {
        if !hwnd.is_null() {
            // ★ 2.6.79：入参都是 96dpi 基准值（含由 `client_width` 推出的结果），
            //   这里统一换算成物理像素。这一处覆盖本函数全部动态重排，
            //   所以 530 行常量不需要逐个改写。
            MoveWindow(
                hwnd,
                crate::dpi::s(x),
                crate::dpi::s(y),
                crate::dpi::s(width),
                crate::dpi::s(height),
                1,
            );
        }
    };
    // 语言栏靠右显示：标签(55) + 间距(6) + combo(180)，combo 右缘 = 客户宽 - 20。
    // 创建时的静态坐标只对最小宽 1020 成立，这里按实际客户宽动态右对齐
    //（覆盖最大化/还原/拖拽改尺寸等所有窗口状态）。
    // ★ 2.6.92：语言标签与下面四个顶栏按钮的宽度改成按文字实测——原来是按 14 号字
    //   调出来的固定值（55 / 90 / 150 / 140 / 120），18 号字下分别被切
    //   （最严重 `Cancel pending reboot`：150 装 198）。
    //   顶栏是**右对齐链条**，加宽只会把整条链往左推，不会压到邻接控件。
    let lang_label_w = fit_width(ui_text(language, "language"), 55);
    let lang_combo_w = 180;
    let lang_total = lang_label_w + 6 + lang_combo_w;
    reposition(
        GetDlgItem(state.root, ID_LANGUAGE_LABEL as i32),
        client_width - 20 - lang_total,
        TOP_BAR_Y + 7,
        lang_label_w,
        18,
    );
    reposition(
        state.controls.language,
        client_width - 20 - lang_combo_w,
        TOP_BAR_Y,
        lang_combo_w,
        300,
    );
    // 「设置…」按钮紧靠语言栏左侧、同排右对齐（客户宽变化时一起跟随）。
    let settings_w = fit_width(ui_text(language, "settings"), 90);
    let settings_x = client_width - 20 - lang_total - 8 - settings_w;
    reposition(
        GetDlgItem(state.root, ID_SETTINGS as i32),
        settings_x,
        TOP_BAR_Y,
        settings_w,
        24,
    );
    // 「取消待重启任务」在「设置…」左侧，同排右对齐。
    // 2026-10-17 教训：这个按钮最初漏了这条重排，停留在创建坐标 760，把语言
    // 标签和 combo 压掉半个 —— 顶栏三个右对齐控件必须一起进 layout_operation。
    let cancel_w = fit_width(ui_text(language, "cancel_pending"), 150);
    let cancel_x = settings_x - 8 - cancel_w;
    reposition(
        GetDlgItem(state.root, ID_CANCEL_PENDING as i32),
        cancel_x,
        TOP_BAR_Y,
        cancel_w,
        24,
    );
    // 「备份文件管理…」在「取消待重启任务」左侧，同排右对齐（同一链条，
    // 漏了重排就会压在邻接控件上——2026-10-17 的教训对每个新顶栏按钮都成立）。
    let image_admin_w = fit_width(ui_text(language, "image_admin"), 140);
    let image_admin_x = cancel_x - 8 - image_admin_w;
    reposition(
        GetDlgItem(state.root, ID_IMAGE_ADMIN as i32),
        image_admin_x,
        TOP_BAR_Y,
        image_admin_w,
        24,
    );
    // 「查看日志」在同一链条的最左端（同一个教训：新顶栏按钮必须进这条重排链）。
    let view_logs_w = fit_width(ui_text(language, "view_logs"), 120);
    reposition(
        GetDlgItem(state.root, ID_VIEW_LOGS as i32),
        image_admin_x - 8 - view_logs_w,
        TOP_BAR_Y,
        view_logs_w,
        24,
    );
    reposition(
        state.controls.source,
        field_x,
        source_combo_y,
        field_width,
        220,
    );
    reposition(
        state.controls.source_details,
        field_x,
        source_details_y,
        field_width,
        details_height,
    );
    reposition(
        state.controls.target,
        field_x,
        target_combo_y,
        field_width,
        220,
    );
    reposition(
        state.controls.target_details,
        field_x,
        target_details_y,
        field_width,
        details_height,
    );
    // B/C 入口的通道单选行：在线（左）/ 离线（右），文案随语言刷新，
    // 双保险处理「两个单选都未选中」的异常态（同 PE 模式单选的做法）。
    if channel_visible {
        let online_hwnd = GetDlgItem(state.root, ID_CHANNEL_ONLINE as i32);
        let offline_hwnd = GetDlgItem(state.root, ID_CHANNEL_OFFLINE as i32);
        let online_checked = IsDlgButtonChecked(state.root, ID_CHANNEL_ONLINE as i32) != 0;
        let offline_checked = IsDlgButtonChecked(state.root, ID_CHANNEL_OFFLINE as i32) != 0;
        if !online_checked && !offline_checked {
            SendMessageW(online_hwnd, BM_SETCHECK, BST_CHECKED, 0);
            append_gui_log(
                state,
                "channel layout: both radios unchecked -> reset to online",
            );
        }
        let language = selected_language(state);
        set_text(
            online_hwnd,
            if language == Language::English {
                "Online (no reboot)"
            } else {
                "在线执行（不重启）"
            },
        );
        set_text(
            offline_hwnd,
            if language == Language::English {
                "Offline (reboot into PE/RE)"
            } else {
                "离线执行（重启进 PE/RE）"
            },
        );
        reposition(online_hwnd, field_x, channel_y, 300, 22);
        reposition(offline_hwnd, field_x + 320, channel_y, 360, 22);
    }
    // 三个还原入口的「格式化目标分区」复选框：通道行下方一行（A 无通道行时
    // 仍按同一相对位置排布），文案随语言刷新。
    if show_format_target {
        let format_hwnd = GetDlgItem(state.root, ID_FORMAT_TARGET as i32);
        set_text(
            format_hwnd,
            if selected_language(state) == Language::English {
                "Format target volume (erase, then write the image)"
            } else {
                "格式化目标分区（清空目标卷后写入镜像）"
            },
        );
        let format_row_y = if channel_visible { format_y } else { channel_y };
        reposition(format_hwnd, field_x, format_row_y, 460, 22);
    }
    // ★ 同样按「最坏行数 × 行距」算，并且**以实测可用高度反推**：
    // 2.6.80：行距 17 px，提示文案最长 9 行（restore 页）→ 153 px；实测该控件
    // EM_GETRECT 的文本区比控件高度少 14 px（178 → availH 164），故 178 之后余量 11 px。
    // 更长的内容（PE 页）靠 WS_VSCROLL 滚动——**宁可滚，也不要切半行**。
    // 此前 64 px 只够 4.2 行，第 5 行被切成半截。
    // 9 行 + 25（文本区比控件高少 14px，另留 11px 余量）；同样随字号变化。
    // 高度在函数上半段就已经算好了（`status_height`）——下面几行的 y 都靠它定位，
    // 所以不能只在这儿算。
    reposition(state.controls.status, 20, status_y, client_width - 40, status_height);

    // 「安装 PE」tab：行1 RAM disk 单选 + PE 目录路径（完整路径，同排）；
    // 行2 硬盘启动单选；行3 PE 启动项名称（两种模式通用）
    if selected_operation(state) == "install-pe-entry" {
        let mode_y = last_details_y + 18;
        let pe_ram_checked = {
            let ram_checked = IsDlgButtonChecked(state.root, ID_PE_MODE_RAM as i32) != 0;
            let disk_checked = IsDlgButtonChecked(state.root, ID_PE_MODE_DISK as i32) != 0;
            if !ram_checked && !disk_checked {
                // 双保险：任何路径（切换 tab 等）导致两个单选都未选中时，
                // 恢复默认选中 RAM disk，避免目录行被隐藏、模式语义丢失。
                SendMessageW(
                    GetDlgItem(state.root, ID_PE_MODE_RAM as i32),
                    BM_SETCHECK,
                    BST_CHECKED,
                    0,
                );
                append_gui_log(
                    state,
                    "PE layout: both radios unchecked -> reset to RAM disk",
                );
                true
            } else {
                ram_checked
            }
        };
        append_gui_log(
            state,
            &format!(
                "PE layout: last_details_y={last_details_y} mode_y={mode_y} ram_checked={pe_ram_checked}"
            ),
        );
        set_text(
            GetDlgItem(state.root, ID_PE_MODE_RAM as i32),
            if selected_language(state) == Language::English {
                "RAM disk (no partition)"
            } else {
                "RAM disk（不占分区）"
            },
        );
        set_text(
            GetDlgItem(state.root, ID_PE_MODE_DISK as i32),
            if selected_language(state) == Language::English {
                "Hard disk boot (own partition)"
            } else {
                "硬盘启动（独立分区）"
            },
        );
        set_text(
            GetDlgItem(state.root, ID_PE_DIR_LABEL as i32),
            if selected_language(state) == Language::English {
                "PE folder path"
            } else {
                "PE 目录路径"
            },
        );
        reposition(
            GetDlgItem(state.root, ID_PE_MODE_RAM as i32),
            field_x,
            mode_y,
            260,
            22,
        );
        // 行2：硬盘启动单选
        reposition(
            GetDlgItem(state.root, ID_PE_MODE_DISK as i32),
            field_x,
            mode_y + 28,
            280,
            22,
        );
        // 「PE 目录路径」输入框：填完整路径（如 C:\BackupRestorePE，盘符可
        // 任意合法目录，不限于 C）。WIM 复制到该路径的 sources\boot.wim。
        // 为空时按语言+当前目标卷填默认完整路径；已有内容（含完整路径）原样保留。
        let dir_edit_hwnd = GetDlgItem(state.root, ID_PE_DIR_EDIT as i32);
        let dir_text = get_text(dir_edit_hwnd);
        let dir_trimmed = dir_text.trim();
        if dir_trimmed.is_empty() {
            // 程序所在盘符：取到就排除它，取不到（程序路径异常）就不排除任何卷，
            // 绝不退回字母 C——那会把一台系统盘恰好是 C 的机器上所有其它卷都排除掉。
            let exe_drive = state
                .executable_dir
                .to_string_lossy()
                .chars()
                .next()
                .map(|letter| letter.to_ascii_uppercase());
            let default_drive = state
                .drives
                .iter()
                .map(|d| d.letter.to_ascii_uppercase())
                .find(|letter| {
                    letter
                        .chars()
                        .next()
                        .map(|c| Some(c) != exe_drive)
                        .unwrap_or(true)
                })
                .unwrap_or_else(|| {
                    std::env::var("SystemDrive")
                        .unwrap_or_else(|_| "C:".to_string())
                        .trim()
                        .trim_end_matches(':')
                        .to_ascii_uppercase()
                });
            set_text(dir_edit_hwnd, &format!("{default_drive}:\\BackupRestorePE"));
        }
        // ★ 2.6.92：标签与浏览按钮改按实测宽度（18 号字下固定 110/80 会切）。
        let pe_dir_label_text = if language == Language::English {
            "PE folder path"
        } else {
            "PE 目录路径"
        };
        let pe_dir_label_w = fit_width(pe_dir_label_text, 110);
        reposition(
            GetDlgItem(state.root, ID_PE_DIR_LABEL as i32),
            field_x + 270,
            mode_y,
            pe_dir_label_w,
            22,
        );
        reposition(
            GetDlgItem(state.root, ID_PE_DIR_EDIT as i32),
            field_x + 270 + pe_dir_label_w + 10,
            mode_y,
            200,
            24,
        );
        // 浏览按钮紧贴目录输入框右侧（输入框 380..580，按钮 586..666）
        // ★ 2.6.80：按钮宽 60 → 80，给 14px 微软雅黑 Bold 的「浏览…」留出余量。
        reposition(
            GetDlgItem(state.root, ID_PE_DIR_BROWSE as i32),
            field_x + 270 + pe_dir_label_w + 10 + 200 + 6,
            mode_y,
            fit_width(ui_text(language, "pe_dir_browse"), 80),
            24,
        );
        // 行3：「PE 启动项名称」（两种模式通用，始终显示）
        // ★ 2.6.97：文案提到变量里，下面的 reposition 要拿它按实测定宽。
        let pe_name_label_text = if selected_language(state) == Language::English {
            "Boot entry name"
        } else {
            "PE 启动项名称"
        };
        set_text(
            GetDlgItem(state.root, ID_PE_NAME_LABEL as i32),
            pe_name_label_text,
        );
        // 输入框为空时按语言+当前模式填默认名；若仍是另一模式的默认名
        // （用户未自定义），切换模式时跟随更新（用户自定义后保留）。
        let name_edit_hwnd = GetDlgItem(state.root, ID_PE_NAME_EDIT as i32);
        let default_name = pe_entry_description(selected_language(state), pe_ram_checked);
        let current_name = get_text(name_edit_hwnd).trim().to_string();
        if current_name.is_empty() {
            set_text(name_edit_hwnd, &default_name);
        } else {
            let other_default = pe_entry_description(selected_language(state), !pe_ram_checked);
            if current_name == other_default {
                set_text(name_edit_hwnd, &default_name);
            }
        }
        // ★ 2.6.97：标签固定 130 装不下，18 号英文 `Boot entry name` 要 144。
        //   按实测加宽，输入框跟着右移。
        let pe_name_label_w = fit_width(pe_name_label_text, 130);
        reposition(
            GetDlgItem(state.root, ID_PE_NAME_LABEL as i32),
            field_x,
            mode_y + 56,
            pe_name_label_w,
            22,
        );
        reposition(
            GetDlgItem(state.root, ID_PE_NAME_EDIT as i32),
            field_x + pe_name_label_w + 10,
            mode_y + 54,
            300,
            24,
        );
        // 硬盘模式下隐藏 PE 目录路径（完整路径）行，仅 RAM disk 显示
        let dir_label_hwnd = GetDlgItem(state.root, ID_PE_DIR_LABEL as i32);
        let label_visible = if pe_ram_checked { SW_SHOW } else { SW_HIDE };
        let edit_visible = if pe_ram_checked { SW_SHOW } else { SW_HIDE };
        ShowWindow(dir_label_hwnd, label_visible);
        ShowWindow(dir_edit_hwnd, edit_visible);
        ShowWindow(
            GetDlgItem(state.root, ID_PE_DIR_BROWSE as i32),
            edit_visible,
        );
    }

    // ★ 2.6.80：基准字号 12→14 且改用微软雅黑 Bold 之后，浏览按钮的文字变宽，
    //   原来「间距 10 + 按钮宽 80」在视觉上贴到输入框右端（用户反馈重叠）。
    //   这里把间距加到 14、按钮宽加到 106，并相应缩短输入框，右缘仍与
    //   `field_x + field_width` 对齐（10+80=90 与 14+106=120 的差额由输入框吸收）。
    let image_width = (field_width - 120).max(370);
    reposition(state.controls.image, field_x, image_y, image_width, 24);
    reposition(
        GetDlgItem(state.root, ID_BROWSE_IMAGE as i32),
        field_x + image_width + 14,
        image_y,
        106,
        24,
    );
    // 「单系统还原」与「新增第二系统」tab 的 WIM 索引与校验复选框布局：
    // C（还原系统到非 C 盘）与「新增第二系统」一样带菜单名输入框，用同一布局。
    let (index_width, verify_hash_x, verify_hash_y) =
        if matches!(
            selected_operation(state),
            "create-secondary"
        ) {
            (380, field_x, secondary_y + 32)
        } else {
            (
                field_width - 320,
                field_x + field_width - 300,
                secondary_y + 2,
            )
        };
    reposition(state.controls.index, field_x, secondary_y, index_width, 220);
    reposition(
        state.controls.verify_hash,
        verify_hash_x,
        verify_hash_y,
        300,
        24,
    );
    // ★ 2.6.92：标签加宽后输入框跟着右移（否则英文 18 号下标签会压到输入框上）。
    reposition(
        state.controls.menu,
        field_x + 390 + fit_width(ui_text(language, "menu"), 100) + 10,
        secondary_y,
        (field_width - 500).max(300),
        24,
    );
    // 「备份」tab 的压缩率下拉：WIM 索引/第二系统名称行（secondary_y）在
    // 备份模式下控件均隐藏，压缩率独占该行：标签在左侧，下拉框在右侧。
    reposition(GetDlgItem(state.root, 2014), 20, secondary_y + 2, 150, 24);
    reposition(state.controls.compress, field_x, secondary_y, 280, 220);

    // ★ 2.6.92：这两个标签的宽度不能加（右边紧邻输入框，列宽固定 150），
    //   但**高度**得按实测算：18 号字下「目标卷（镜像位置）」在 150 宽内会折成两行，
    //   写死 40 会把第二行切掉（实测 h=40 / need=48，切 8 像素）。
    // ★ 2.6.94：这里**不能**用 `measure_text_height` 算——布局这一段跑在
    //   「字体尺寸已经改、但控件字体还没应用」的窗口期，量出来的是旧字号下的行数
    //   （18 号字下实测量到 1 行 → 高度算成 40，而真实排版是 2 行 48，切掉半行）。
    //   改成按「最坏 2 行 × 当前行距 + 8」直接算：行距本身跟随字号，与测量无关。
    //   中英文在 150 宽内最多占 2 行（中文「目标卷（镜像位置）」9 字 = 162 > 150）。
    let label_h = crate::dpi::text_block_height(2, 8).max(40);
    let source_label_h = label_h;
    let target_label_h = label_h;
    reposition(
        GetDlgItem(state.root, 2003),
        20,
        source_combo_y + 2,
        150,
        source_label_h,
    );
    reposition(
        GetDlgItem(state.root, 2005),
        20,
        target_combo_y + 2,
        150,
        target_label_h,
    );
    reposition(GetDlgItem(state.root, 2004), 20, image_y + 2, 150, 24);
    reposition(GetDlgItem(state.root, 2007), 20, secondary_y + 2, 150, 24);
    // ★ 2.6.92：同上——固定 100 宽在 18 号字下装不下
    //   中文「第二系统名称」108 / 英文 `Secondary boot name` 189。
    reposition(
        GetDlgItem(state.root, 2008),
        field_x + 500 - 110,
        secondary_y + 2,
        fit_width(ui_text(language, "menu"), 100),
        24,
    );
    // 备份 tab 第二行：索引名（左）+ 保留最近 N 个（右），位于压缩率行下方。
    let index_name_y = secondary_y + 32;
    reposition(GetDlgItem(state.root, 2015), 20, index_name_y, 130, 24);
    reposition(
        state.controls.index_name,
        field_x,
        index_name_y - 2,
        280,
        24,
    );
    // ★ 2.6.92：这个标签原来固定 100 宽（14 号字调出来的），18 号字下
    //   中文「保留最近 N 个」要 115、英文 `Keep latest N` 要 115 都被切。
    //   标签按实测加宽后，右侧的计数框跟着右移，保证不压在一起。
    let keep_label_x = field_x + 320;
    let keep_label_w = fit_width(ui_text(language, "keep"), 100);
    reposition(
        GetDlgItem(state.root, 2016),
        keep_label_x,
        index_name_y,
        keep_label_w,
        24,
    );
    reposition(
        state.controls.keep,
        keep_label_x + keep_label_w + 10,
        index_name_y - 2,
        80,
        24,
    );
    // ★ 2.6.92：这排按钮原来是固定 x（20/150/280/410）+ 固定宽（120/120/120/140），
    //   18 号字下「刷新环境」120 装 178、「刷新任务状态」140 装 160 都被切。
    //   改成按实测宽度**顺序排布**：宽度跟文字走，后面的按钮自然右移，
    //   不再需要手工维护 x 常量（以前加一个按钮就要重算整排）。
    let mut buttons_x = 20;
    for (id, key, floor) in [
        (ID_REFRESH, "refresh", 120),
        (ID_READ_IMAGE, "read_image", 120),
        (ID_CREATE_TASK, "create_task", 120),
        (ID_REFRESH_TASK, "refresh_task", 140),
    ] {
        let w = fit_width(ui_text(language, key), floor);
        reposition(GetDlgItem(state.root, id as i32), buttons_x, buttons_y, w, 28);
        buttons_x += w + 10;
    }
    // 「安装 PE」tab 专属按钮（重启进入 PE / 创建快捷方式）：
    // 创建时使用固定坐标（560/710, 630），这里与通用按钮行同 y 对齐，
    // 避免窗口/布局变化时与右侧控件错位或遮挡。
    if selected_operation(state) == "install-pe-entry" {
        // ★ 2.6.97：这两个按钮原来写死 560 / 710——按钮行改成「按实测宽度顺序排布」之后，
        //   18 号英文下最后一个按钮（Refresh task status）会伸到 486 基准像素以右，
        //   实测与「重启进入 PE」重叠 208×56。改成接着按钮行末尾排。
        let pe_reboot_w = fit_width(ui_text(language, "pe_reboot_main"), 140);
        let pe_reboot_x = buttons_x.max(560);
        reposition(
            GetDlgItem(state.root, ID_PE_REBOOT_MAIN as i32),
            pe_reboot_x,
            buttons_y,
            pe_reboot_w,
            28,
        );
        reposition(
            GetDlgItem(state.root, ID_PE_SHORTCUT as i32),
            pe_reboot_x + pe_reboot_w + 10,
            buttons_y,
            fit_width(ui_text(language, "pe_shortcut"), 140),
            28,
        );
    }
}

pub(crate) unsafe fn set_volume_labels(state: &State) {
    let language = selected_language(state);
    let operation = selected_operation(state);
    // A 的目标标签带实际盘符（%SystemDrive%），动态拼接；源卷行在 A/B/C 隐藏，
    // 标签文案仅作回退占位。Keep the labels short enough for the narrow role
    // column. The detail boxes below carry the complete safety explanation.
    let (source, target): (String, String) = match (language, operation) {
        (Language::Chinese, "backup") => {
            ("源卷（备份来源）".into(), "目标卷（镜像位置）".into())
        }
        (Language::Chinese, "restore-existing") => (
            "源卷（不使用）".into(),
            format!("目标卷（当前系统盘 {}:）", active_system_letter()),
        ),
        (Language::Chinese, "restore-data") => {
            ("源卷（不使用）".into(), "目标卷（数据卷）".into())
        }
        (Language::Chinese, "create-secondary") => (
            "源卷（不使用）".into(),
            "目标卷（第二系统位置）".into(),
        ),
        (Language::Chinese, "install-pe-entry") => {
            ("源卷（不使用）".into(), "目标卷（PE 安装位置）".into())
        }
        (Language::Chinese, _) => ("源卷（检查对象）".into(), "目标卷（不使用）".into()),
        (Language::English, "backup") => ("Source (backup)".into(), "Target (image)".into()),
        (Language::English, "restore-existing") => (
            "Source (unused)".into(),
            format!("Target (system {}:)", active_system_letter()),
        ),
        (Language::English, "restore-data") => {
            ("Source (unused)".into(), "Target (data)".into())
        }
        (Language::English, "create-secondary") => (
            "Source (unused)".into(),
            "Target (second system)".into(),
        ),
        (Language::English, "install-pe-entry") => {
            ("Source (unused)".into(), "Target (PE install)".into())
        }
        (Language::English, _) => ("Source (inspect)".into(), "Target (unused)".into()),
    };
    set_child_text(state.root, 2003, &source);
    set_child_text(state.root, 2005, &target);
}

pub(crate) unsafe fn select_operation(state: &mut State, index: usize) {
    // 【2026-10-18 缺陷修复 M2】两个「当前选中项」都必须在**改 operation_index
    // 之前**读取。combo 里的项是**旧入口**填充的，而 `selected_drive_letter`
    // 按**当前** operation_index 决定用不用过滤视图来解释索引——顺序一颠倒，
    // 就是用新视图去读旧内容，选中项被静默移位（从 C 切到 A 会把用户选的
    // F: 读成 E:）。源卷同理：此前重填时 source 传 `None`，等于把备份页的
    // 源卷静默重置为第 0 项，切走再切回可能备份到错误的分区。
    let keep_target = selected_drive_letter(state, state.controls.target);
    let keep_source = selected_drive_letter(state, state.controls.source);
    state.operation_index = index.min(4);
    set_operation_tabs(state, selected_language(state));
    let operation = selected_operation(state);
    if operation == "restore-existing" {
        // A（还原系统到 C 盘）：目标锁定 %SystemDrive%，自动选中（下拉已禁用）。
        // 旧版「源卷选择复制到目标」逻辑随源卷行隐藏一并移除。
        let letter = active_system_letter();
        select_drive(state, state.controls.target, &letter);
    }
    if matches!(operation, "restore-data" | "create-secondary") {
        // B/C：每次进入入口时通道复位为「在线」默认——不跨入口保留上次的
        // 离线选择，防止用户在 C 选了离线后切到 B 忘改而意外重启格式化。
        SendMessageW(
            GetDlgItem(state.root, ID_CHANNEL_ONLINE as i32),
            BM_SETCHECK,
            BST_CHECKED,
            0,
        );
        SendMessageW(
            GetDlgItem(state.root, ID_CHANNEL_OFFLINE as i32),
            BM_SETCHECK,
            BST_UNCHECKED,
            0,
        );
    }
    // C 入口目标 combo 排除当前系统盘；离开 C 回别处时恢复全量（A 页要能
    // 选中系统盘）。combo 内容只在 set_drive_items 填充时定死、tab 切换
    // 本身不重填——这里按「当前入口期望项数 vs 实际项数」校验，不符即带
    // 选中重填（v2.6.13；少了这步，从备份 tab 进 C 会残留全量枚举，系统盘
    // 又出现在下拉里）。
    if target_combo_expected_count(state)
        != SendMessageW(state.controls.target, CB_GETCOUNT, 0, 0) as usize
    {
        append_gui_log(
            state,
            &format!(
                "target combo refilled: op={operation} keep={} keep_source={}",
                keep_target.clone().unwrap_or_else(|| "<none>".into()),
                keep_source.clone().unwrap_or_else(|| "<none>".into())
            ),
        );
        // 源卷选中项一并传下去（M2）：不传就是「静默重置为第 0 项」。
        set_drive_items(state, [None, keep_source, keep_target]);
    }
    if matches!(operation, "restore-data" | "create-secondary") {
        // 目标若还停在初始默认的系统盘（B/C 都禁止的目标），自动跳到第一个
        // 合法卷：B=非系统且非 Windows 卷；C=非系统卷。找不到就保持原样，
        // 由「创建任务」时的目标闸门给出明确报错（不做静默猜测）。
        let system = active_system_letter();
        if let Some(current) = selected_drive_letter(state, state.controls.target)
            && current.eq_ignore_ascii_case(&system)
            && let Some(candidate) = state.drives.iter().find(|drive| {
                !drive.letter.eq_ignore_ascii_case(&system)
                    && (operation == "create-secondary" || !drive.has_windows_installation)
            })
        {
            append_gui_log(
                state,
                &format!(
                    "target default moved off system drive: {} -> {}",
                    current, candidate.letter
                ),
            );
            select_drive(state, state.controls.target, &candidate.letter);
        }
    }
    set_operation_visibility(state);
    set_volume_labels(state);
    layout_operation(state);
    set_operation_guidance(state);
    set_drive_details(state);
    // S5：切到 C/第二系统 tab 时，若菜单名还是自动态就用当前索引的 WIM 名预填。
    prefill_menu_name_from_image(state);
    // 整窗强制重绘（历史 bug 双保险）：layout_operation 会对十余个控件做
    // MoveWindow，快速连续切换 tab 时重绘风暴容易让个别控件区域残留上一
    // tab 的文本（表现为「PE tab 镜像标签行叠出上一 tab 的 status 文本」）。
    // 在全部布局与文案就绪后同步重绘整窗，彻底擦除残留像素。
    InvalidateRect(state.root, null(), 1);
    UpdateWindow(state.root);
}

