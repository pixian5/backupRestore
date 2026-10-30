//! 图形界面文案表（2026-10-26 从 `native_gui.rs` 抽出）。
//!
//! 为什么单独成文件：这是图形界面里**唯一能在非 Windows 主机上单测**的部分——
//! 纯函数 + 静态表，不碰任何 Win32 句柄。窗口过程与布局必须靠交叉编译加实机点击
//! 验证，文案表却是纯数据，抽出来就能用单测守住「每个键都有中英两版、没有空串、
//! 未知键回落」这类回归。
//!
//! ★ 调用点不用改：`native_gui.rs` 用 `pub(crate) use` 重导出这三个名字
//! （沿用 `pe_safety` 的既有套路），所以 `Language::Chinese` / `ui_text(..)` 的
//! 写法一行都没动。

// 非 Windows 主机上 `native_gui` 不参与编译，这三个名字暂时没有调用者。
#![cfg_attr(not(windows), allow(dead_code))]

pub(crate) fn tooltip_text(language: Language, key: &str) -> &'static str {
    match (language, key) {
        (Language::Chinese, "probe") => "探测：仅校验任务、WinRE 和卷身份，不写入磁盘、不重启。",
        (Language::Chinese, "backup") => "备份：进入 WinRE 后用 DISM 捕获源卷到镜像路径。",
        (Language::Chinese, "restore") => {
            "单系统还原：格式化目标卷并写入镜像，目标必须是当前系统卷。"
        }
        (Language::Chinese, "secondary") => {
            "新增非 C 盘第二系统：把完整 Windows 镜像部署到另一个分区并新增启动菜单项，还原完成后默认启动项切换为这台新系统（原系统仍保留在菜单中可选）。默认在线执行（格式化目标并修复引导，不重启）；离线则重启进恢复环境完成。目标不能是当前系统盘。"
        }
        (Language::Chinese, "restore_a") => {
            "还原系统到 C 盘：目标锁定为当前系统盘（显示实际盘符），必须离线执行——重启进入 PE/RE 完成格式化与还原；破坏性确认与环境确认两道确认后才会开始。"
        }
        (Language::Chinese, "restore_b") => {
            "还原数据到非 C 盘：把数据卷镜像还原到非系统分区。默认在线执行、不格式化（覆盖式应用，卷序列号不变）；切到离线则重启进 PE/RE，先 diskpart 格式化再还原。目标不能是当前系统盘。"
        }
        (Language::Chinese, "channel_online") => {
            "在线执行：不重启，直接在当前系统里完成（B 按「格式化目标分区」选项决定是否先清空目标卷；C 恒格式化并修复引导）。"
        }
        (Language::Chinese, "channel_offline") => {
            "离线执行：创建任务并重启进入 PE/Windows RE 完成（目标卷都会被格式化后写入镜像）。"
        }
        (Language::Chinese, "format_target") => {
            "勾选：先用 diskpart 快速格式化目标卷并校验成功（分区不变、卷序列号改变），再写入镜像——还原的完整语义（默认）。\n不勾：覆盖式应用，不格式化（目标卷上镜像没有的文件会保留，卷序列号不变）。\n在线离线通道均生效；离线时该选择会随任务落盘、恢复环境内执行。"
        }
        (Language::Chinese, "source") => "选择备份来源或当前 Windows 分区；身份按 GUID 校验。",
        (Language::Chinese, "target") => "还原时将被格式化的目标分区；备份和探测模式不使用。",
        (Language::Chinese, "source_details") => {
            "显示所选源卷的详细信息：卷标、文件系统、容量、分区位置与卷 GUID（身份校验按 GUID 而非盘符，同一分区换盘符仍能认出）。"
        }
        (Language::Chinese, "target_details") => {
            "显示所选目标卷的详细信息：卷标、文件系统、剩余空间、分区位置与卷 GUID。还原/新增第二系统前请确认这就是要操作的分区——目标卷会被格式化，数据全部丢失。"
        }
        (Language::Chinese, "status") => {
            "实时显示当前环境信息（Windows 版本、恢复环境状态、可用卷数量）与最近一次操作的结果提示；操作前后的校验结论也显示在这里。"
        }
        (Language::Chinese, "image") => {
            "镜像文件必须是绝对路径，例如 B:\\BackupRestore\\Windows.wim。"
        }
        (Language::Chinese, "index") => "选择 WIM 索引；下拉项显示索引及详细元数据。",
        (Language::Chinese, "menu") => "第二系统在 Windows 启动菜单中显示的名称。",
        (Language::Chinese, "compress") => {
            "备份镜像压缩方式（仅首次创建 WIM 时生效）：\n• 压缩（默认/推荐）：使用 fast 压缩，速度和体积平衡\n• 不压缩：WIM 最大（约等于源数据量），备份最快\n增量备份说明：镜像已存在时追加为新索引，压缩方式沿用 WIM 首次创建时的设置；压缩方式不影响能否增量备份。"
        }
        (Language::Chinese, "index_name") => {
            "备份索引名（写入 WIM 的 Name 字段）。默认是程序启动时间，可修改；追加备份时用于区分历史版本。"
        }
        (Language::Chinese, "keep") => {
            "保留最近 N 个索引：备份追加成功后自动删除更旧的索引（0=不清理）。删除不可恢复，请谨慎设置。"
        }
        (Language::Chinese, "verify_hash") => {
            "勾选时全量计算镜像 SHA-256 哈希比对；不勾选时仅对比文件物理大小（毫秒级极速，推荐）。"
        }
        (Language::Chinese, "refresh") => "刷新 Windows、WinRE 和可用卷信息。",
        (Language::Chinese, "read_image") => "只读解析 WIM 索引、哈希和元数据。",
        (Language::Chinese, "create_task") => "创建任务；还原操作会先显示确认对话框。",
        (Language::Chinese, "refresh_task") => "读取程序目录中的最近任务状态。",
        (Language::Chinese, "browse") => {
            "打开文件选择框，浏览并选择镜像文件（.wim）的保存或来源路径。"
        }
        (Language::Chinese, "pe_dir_browse") => {
            "浏览并选择 PE 存放目录路径，PE 镜像（boot.wim）将复制到该目录。"
        }
        (Language::Chinese, "pe_reboot_main") => {
            "部署完成后立即重启进入已安装的 PE 环境，验证 PE 能否正常启动。注意：Parallels 等虚拟机的 bootsequence 可能不被固件消费，真实硬件路径有效。"
        }
        (Language::Chinese, "pe_shortcut") => {
            "在桌面创建「打开 安装 PE」快捷方式，方便以后手动进入已安装的 PE 环境。"
        }
        (Language::Chinese, "pe_mode_ram") => {
            "RAM disk 模式（适合临时验证）：PE 镜像临时复制到内存盘启动，不占用独立分区、不改变现有分区布局。"
        }
        (Language::Chinese, "pe_mode_disk") => {
            "硬盘启动模式（更稳定）：把 PE 部署到独立分区并从该分区启动，适合长期保留；需要提供一个空闲分区。⚠ 该分区将被格式化，上面的数据全部丢失！"
        }
        (Language::Chinese, "pe_dir_edit") => {
            "PE 目录路径：PE 镜像（boot.wim）存放的文件夹；请输入完整路径（盘符可为任意合法目录，不限于 C）。RAM disk 模式下 WIM 会复制到此路径下。"
        }
        (Language::Chinese, "language") => {
            "切换界面语言（中文 / English），切换后所有标签、按钮与说明文字随之更新。"
        }
        (Language::Chinese, "settings") => {
            "程序设置：恢复环境连续失败多少次后如何处理。\n• 自动清理并重启回正常 Windows（默认，等于原行为）；\n• 暂停在恢复环境，由你按按钮手动清理。\n设置保存在程序目录的 BackupRestore.settings.ini，对之后创建的任务生效。"
        }
        (Language::Chinese, "cancel_pending") => {
            "处置「上次没跑完」的还原任务：放弃并清理（清掉一次性启动项、按终态收口、保留证据），或查看它当前停在哪一步。\n没有这类任务时按钮只提示，不做任何改动。"
        }
        (Language::Chinese, "view_logs") => {
            "查看日志：列出日志根目录下最近修改的日志文件（本轮任务日志排最前），可用记事本打开，也可直接打开日志目录。\n纯只读，不改任何文件。"
        }
        (Language::English, "probe") => {
            "Inspect: validate task, WinRE and volume identities; no disk write or reboot."
        }
        (Language::English, "backup") => {
            "Backup: enter WinRE and capture the selected source volume with DISM."
        }
        (Language::English, "restore") => "Restore: format the target volume and apply the image.",
        (Language::English, "secondary") => {
            "Add second system (non-C:): deploy a full Windows image to another partition with a new boot entry; after the restore the default boot entry switches to this new system (the previous system stays selectable in the menu). Online by default; target must not be the system drive."
        }
        (Language::English, "restore_a") => {
            "Restore system to C:: target is locked to the running system drive; must run offline (PE/RE) and requires two confirmations."
        }
        (Language::English, "restore_b") => {
            "Restore data to non-C:: apply a data-volume image without touching boot. Online by default (no format); offline formats via diskpart. Target must not be the system drive."
        }
        (Language::English, "channel_online") => {
            "Online: run inside the current Windows session without reboot (B honors the 'Format target volume' option; C always formats and repairs boot)."
        }
        (Language::English, "channel_offline") => {
            "Offline: schedule a task and reboot into PE/RE (target volume is formatted)."
        }
        (Language::English, "format_target") => {
            "Checked (default): quick-format the target via diskpart and verify success (same partition, volume serial changed) before applying the image.\nUnchecked: overlay apply without formatting (files not in the image remain; volume serial unchanged).\nApplies to both online and offline channels; offline the choice is persisted with the task and honored inside the recovery environment."
        }
        (Language::English, "source") => {
            "Select the backup source or current Windows volume; GUID identity is verified."
        }
        (Language::English, "target") => {
            "Restore target to be formatted; unused by backup and inspect."
        }
        (Language::English, "source_details") => {
            "Detailed information of the selected source volume: label, file system, size, partition location and volume GUID (identity is matched by GUID, not drive letter, so a partition is still recognized after its letter changes)."
        }
        (Language::English, "target_details") => {
            "Detailed information of the selected target volume: label, file system, free space, partition location and volume GUID. Confirm this is the right partition before restoring / adding a second system - the target volume is formatted and all data on it is lost."
        }
        (Language::English, "status") => {
            "Live environment information (Windows build, recovery environment status, number of eligible volumes) plus the result of the most recent action; validation verdicts before/after operations also appear here."
        }
        (Language::English, "image") => {
            "The image file must be an absolute path, such as B:\\BackupRestore\\Windows.wim."
        }
        (Language::English, "index") => "Select a WIM index; each item shows detailed metadata.",
        (Language::English, "menu") => "Name shown for the second system in the Windows boot menu.",
        (Language::English, "compress") => {
            "Backup WIM compression (applies only when the WIM is first created):\n• Compression (default/recommended): uses fast compression for a balanced speed and size\n• No compression: largest WIM (approximately the source size), fastest backup\nIncremental notes: appending to an existing WIM keeps the compression set at first creation; compression does not affect whether incremental backup is available."
        }
        (Language::English, "index_name") => {
            "Backup image name (written to the WIM Name field). Defaults to program start time; editable. Appended backups use it to distinguish history."
        }
        (Language::English, "keep") => {
            "Keep latest N indexes: after a successful append, older indexes are deleted (0 = keep all). Deletion is irreversible; set with care."
        }
        (Language::English, "verify_hash") => {
            "When checked, computes full SHA-256 hash before restoring (slower). When unchecked, compares file size only (fast, recommended)."
        }
        (Language::English, "refresh") => "Refresh Windows, WinRE and eligible volume information.",
        (Language::English, "read_image") => {
            "Read WIM indexes, hash and metadata without modifying the image."
        }
        (Language::English, "create_task") => {
            "Create the task; restore operations require confirmation."
        }
        (Language::English, "refresh_task") => {
            "Read the latest task status from the program directory."
        }
        (Language::English, "browse") => {
            "Open a file dialog to pick the .wim image file (save or source path)."
        }
        (Language::English, "pe_dir_browse") => {
            "Browse for the PE storage folder; the PE image (boot.wim) will be copied there."
        }
        (Language::English, "pe_reboot_main") => {
            "Reboot immediately into the PE environment after deploy to verify it boots. Note: bootsequence may not be consumed by Parallels firmware; the real-hardware path works."
        }
        (Language::English, "pe_shortcut") => {
            "Create an 'Open Installed PE' shortcut on the desktop for easy entry later."
        }
        (Language::English, "pe_mode_ram") => {
            "RAM disk mode (good for quick testing): the PE image is loaded into a memory drive, no partition used, existing layout untouched."
        }
        (Language::English, "pe_mode_disk") => {
            "Hard-disk mode (more stable): deploy PE to its own partition and boot from it; requires a spare partition. WARNING: that partition WILL BE FORMATTED (all data on it is lost)."
        }
        (Language::English, "pe_dir_edit") => {
            "PE folder path: where the PE image (boot.wim) is stored; enter a full path (any valid drive or folder, not only C). In RAM disk mode the WIM is copied here."
        }
        (Language::English, "language") => {
            "Switch the UI language (Chinese / English); all labels, buttons and help text update immediately."
        }
        (Language::English, "settings") => {
            "Settings: what to do after the recovery environment fails N times in a row.\n• Clean up and reboot to Windows automatically (default, same as before);\n• Pause in the recovery environment and clean up manually.\nSaved to BackupRestore.settings.ini next to the program; applies to tasks created afterwards."
        }
        (Language::English, "cancel_pending") => {
            "Handle an unfinished recovery task: abandon and clean it up (clears the one-shot boot entry, marks it terminal, keeps the evidence), or just see which stage it stopped at. With nothing pending the button only reports that."
        }
        (Language::English, "view_logs") => {
            "View logs: lists the most recently modified log files under the log root (the current run's task log comes first); open one in Notepad or open the log folder directly.\nRead-only - nothing is modified."
        }
        (Language::Chinese, "image_admin") => {
            "管理备份文件：列出镜像卷（默认 H:\\BRImages）下的 WIM 与各索引，可删除最旧的索引只保留最近 N 个。删除不可撤销，操作前会逐项确认。"
        }
        (Language::English, "image_admin") => {
            "Manage backup files: list the WIMs on the image volume (default H:\\BRImages) with their indexes, and delete the oldest indexes keeping only the latest N. Deleting cannot be undone; each delete is confirmed item by item."
        }
        _ => "",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Language {
    Chinese,
    English,
}

pub(crate) fn ui_text(language: Language, key: &str) -> &'static str {
    match (language, key) {
        (Language::Chinese, "operation") => "操作模式",
        (Language::Chinese, "source") => "源卷",
        (Language::Chinese, "image") => "镜像绝对路径",
        (Language::Chinese, "target") => "目标卷",
        (Language::Chinese, "index") => "WIM 索引",
        (Language::Chinese, "menu") => "第二系统名称",
        (Language::Chinese, "compress") => "压缩率",
        (Language::Chinese, "index_name") => "索引名",
        (Language::Chinese, "keep") => "保留最近 N 个",
        (Language::Chinese, "verify_hash") => "严格对比哈希（默认仅对比大小）",
        (Language::Chinese, "language") => "语言",
        (Language::Chinese, "settings") => "设置…",
        // ★ 省略号只给「点下去还要收集参数」的按钮（Windows 约定）。
        //   下面两个只弹展示/确认框，不收集任何信息，故**不带**省略号（2026-10-26 规范化）。
        (Language::Chinese, "cancel_pending") => "取消待重启任务",
        (Language::Chinese, "image_admin") => "备份文件管理…",
        (Language::Chinese, "view_logs") => "查看日志",
        (Language::Chinese, "refresh") => "刷新环境",
        (Language::Chinese, "read_image") => "读取镜像",
        (Language::Chinese, "browse") => "浏览…",
        (Language::Chinese, "create_task") => "创建任务",
        (Language::Chinese, "refresh_task") => "刷新任务状态",
        (Language::Chinese, "initial_status") => {
            "先点击“刷新环境”确认 Windows、恢复环境和卷身份。默认模式为备份。"
        }
        (Language::Chinese, "probe") => "探测（仅检查）",
        (Language::Chinese, "backup") => "备份",
        // 三个还原入口用用户拍板的全称（2026-10-14：文案不得改动）。
        (Language::Chinese, "restore") => "还原系统到 C 盘",
        (Language::Chinese, "restore_a") => "还原系统到 C 盘",
        (Language::Chinese, "restore_b") => "还原数据到非 C 盘",
        (Language::Chinese, "secondary") => "新增非 C 盘第二系统",
        (Language::Chinese, "probe_hint") => {
            "「探测」是全程无破坏的安全检查模式，适合首次使用先验证环境：\n\n\
             • 刷新并校验当前 Windows 系统、恢复环境（WinRE / PE 入口）以及各卷身份，身份按磁盘+分区 GUID 校验，不依赖盘符；\n\
             • 点「创建任务」只会生成并校验任务文件与恢复环境载荷，验证环境搭建是否正确；\n\
             • 全程不会备份、不会还原、不会格式化、不会改动引导项、不会重启。\n\n\
             建议：第一次使用本程序先切换到「探测」运行一次，确认系统、WinRE、卷都识别正常，再使用其它功能。"
        }
        (Language::Chinese, "backup_hint") => {
            "「备份」把选中的源分区制作成 WIM 镜像（使用 DISM 捕获）：\n\n\
             • 源卷：要备份的分区，默认当前系统分区；镜像路径：用于保存 .wim 的绝对路径；\n\
             • 压缩方式：「压缩」使用 fast，速度和体积平衡；「不压缩」最快但文件最大；\n\
             • 可设置「索引名」与「保留最近 N 个」管理历史版本；镜像已存在时自动追加为新索引；\n\
             • 流程：准备完成后自动进入恢复环境（PE / WinRE），离线捕获源卷，完成后自动返回 Windows。\n\n\
             说明：系统分区必须在离线环境备份；数据卷可在当前系统在线直接备份。"
        }
        (Language::Chinese, "restore_hint") => {
            "「还原系统到 C 盘」把镜像系统写回当前系统盘并重建引导：\n\n\
             • 目标卷：锁定为当前正在运行的系统盘（显示实际盘符，不可改选）；\n\
             • 镜像与索引：要还原的 WIM 及其中索引；还原会覆盖系统盘上的全部数据；\n\
             • 格式化目标分区（默认勾选）：格式化成功校验后才写入；取消勾选=覆盖式应用（旧文件残留）；\n\
             • 流程：破坏性确认 → 选择 PE/RE → 重启离线（按选项格式化）→ DISM 应用镜像 → BCDBoot 重建引导项；\n\
             • 适用场景：系统损坏、无法开机时把备份恢复回本机。\n\n\
             安全提示：目标无法更换，还原前请确认镜像正确、重要数据已另存，操作不可中途取消。"
        }
        (Language::Chinese, "restore_b_hint") => {
            "「还原数据到非 C 盘」把数据卷镜像还原到非系统分区，不碰引导：\n\n\
             • 目标卷：要接收数据的数据分区（不能是当前系统盘，也不能是已有 Windows 的分区）；\n\
             • 格式化目标分区（默认勾选）：先 diskpart 快速格式化并校验成功（分区不变、卷序列号改变），再写入镜像；取消勾选=覆盖式应用、卷上多余文件保留；\n\
             • 通道：在线=不重启直接执行；离线=重启进 PE/RE（格式化选项同样生效，随任务落盘）；\n\
             • 镜像与索引：要还原的 WIM 及其中索引；\n\
             • 适用场景：数据盘损坏或迁移时恢复数据。\n\n\
             安全提示：勾选格式化会清空目标卷上的全部数据；不勾选时同名文件也会被镜像内容覆盖。"
        }
        (Language::Chinese, "secondary_hint") => {
            "「新增非 C 盘第二系统」在保留当前 Windows 的前提下，把镜像部署到另一个分区，成为可启动的第二系统：\n\n\
             • 目标卷：另选一个分区（不能是当前系统盘），将被格式化并写入完整 Windows 镜像；\n\
             • 启动项名称：新系统在 Windows 启动菜单中显示的名称，可用中文；\n\
             • 通道差异：在线=不重启，直接在当前系统里格式化目标并修复引导；离线=重启进恢复环境完成；\n\
             • 效果：完成后默认启动项切换到这台新系统，开机菜单里仍可自由选择进入原系统，适合双系统共存。\n\n\
             安全提示：目标卷数据会被覆盖，部署前请确认该分区上没有需要保留的内容。"
        }
        (Language::Chinese, "pe") => "安装 PE",
        (Language::Chinese, "pe_hint") => {
            "「安装 PE」部署一个自包含的 PE 备份/恢复环境，作为开机可选启动项：\n\n\
             • 启动方式：RAM disk（不占分区，镜像临时复制到内存盘启动）或硬盘启动（独立分区，更稳定）；\n\
             • ⚠ 硬盘启动会先格式化所选独立分区（该分区数据全部丢失），请提前确认分区上没有要保留的内容；\n\
             • PE 目录路径 / 目标卷：指定放置 PE 镜像（boot.wim）的目录或分区；启动项名称可自定义；\n\
             • 部署后新增 BCD 启动项「Windows PE (BackupRestore)」，不改变当前 Windows 默认启动；\n\
             • 用途：当系统无法启动时，可进入 PE 离线备份、还原、安装第二系统、打开完整程序或返回 Windows；\n\
             • 勾选「重启进入 PE」可验证 PE 能否正常启动；PE 使用程序自带的标准 PE 镜像（非 WinRE）。"
        }
        (Language::English, "operation") => "Operation",
        (Language::English, "source") => "Windows source",
        // 2.6.91：原为 "Image absolute path"，18 号字下需要 178 基准像素，
        // 而标签列统一宽 150 → 文字被切掉 28 像素（100% 缩放下实测）。
        // 中文「镜像绝对路径」只要 108，装得下；只有英文串超宽，故缩短。
        (Language::English, "image") => "Image path",
        (Language::English, "target") => "Restore target",
        (Language::English, "index") => "WIM index",
        (Language::English, "menu") => "Secondary boot name",
        (Language::English, "compress") => "Compression",
        (Language::English, "index_name") => "Image name",
        (Language::English, "keep") => "Keep latest N",
        (Language::English, "verify_hash") => "Verify hash (default: size only)",
        (Language::English, "language") => "Language",
        // 省略号统一用单字符 U+2026（与中文一致）；此前四个用 "..." 而 browse 用 "…"，
        // 同一排两套写法是可见的不一致。
        (Language::English, "settings") => "Settings…",
        (Language::English, "cancel_pending") => "Cancel pending reboot",
        (Language::English, "image_admin") => "Backup files…",
        (Language::English, "view_logs") => "View logs",
        (Language::English, "refresh") => "Refresh environment",
        (Language::English, "read_image") => "Read image",
        (Language::English, "browse") => "Browse…",
        (Language::English, "create_task") => "Create task",
        (Language::English, "refresh_task") => "Refresh task status",
        (Language::English, "initial_status") => {
            "Click Refresh environment to inspect Windows, WinRE and volume identities. Default mode is backup."
        }
        (Language::English, "probe") => "Inspect",
        (Language::English, "backup") => "Backup",
        (Language::English, "restore") => "Restore C:",
        (Language::English, "restore_a") => "Restore C:",
        (Language::English, "restore_b") => "Restore data",
        (Language::English, "secondary") => "Add 2nd system (non-C:)",
        (Language::English, "probe_hint") => {
            "Inspect is a completely non-destructive safety check; run it first to validate your environment.\n\n• Refreshes and verifies the current Windows, recovery environment (WinRE / PE entry) and every volume, identified by disk+partition GUID instead of drive letter;\n• 'Create task' only writes and validates the task files and WinRE payloads to confirm the setup;\n• It never backs up, restores, formats, touches boot entries, or reboots.\n\nTip: first-time users should switch to Inspect and run once to confirm Windows, WinRE and volumes are recognized before using other features."
        }
        (Language::English, "backup_hint") => {
            "Backup captures the selected source partition into a WIM image (via DISM).\n\n• Source volume: the partition to back up (defaults to the system partition); Image path: absolute path for the .wim;\n• Compression: \"Compression\" uses fast for balanced speed and size; \"No compression\" is fastest but largest;\n• 'Image name' and 'Keep latest N' manage history; when the image already exists a new index is appended automatically;\n• Flow: after preparation it enters recovery (PE / WinRE), captures the source volume offline, then returns to Windows.\n\nNote: the system partition must be backed up offline; data volumes can be backed up online in the current system."
        }
        (Language::English, "restore_hint") => {
            "Restore C: writes an image system back to the running system drive and rebuilds the boot loader.\n\n• Target volume: locked to the running system drive (actual letter shown, not selectable);\n• Image & index: the WIM and its index to restore; all data on the system drive is overwritten;\n• Flow: destructive confirmation → choose PE/RE → reboot, format the target offline → DISM Apply → BCDBoot rebuilds the boot entry;\n• Use case: recover this machine from a backup when the system is damaged.\n\nSafety: the target cannot be changed; confirm the image is correct and important data is stored elsewhere. The operation cannot be cancelled midway."
        }
        (Language::English, "restore_b_hint") => {
            "Restore data applies a data-volume image to a non-system partition without touching boot.\n\n• Target volume: a data partition (never the system drive, nor a partition that already has Windows);\n• Format target volume (checked by default): quick-format via diskpart, verify success (same partition, serial changed), then write the image; unchecked = overlay apply, extra files remain;\n• Channel: online = no reboot; offline = reboot into PE/RE (always formats there);\n• Image & index: the WIM and its index to restore;\n• Use case: recover or migrate data volumes.\n\nSafety: formatting erases everything on the target; even without it, same-name files are overwritten."
        }
        (Language::English, "secondary_hint") => {
            "Add second system (non-C:) deploys a full Windows image to another partition as a bootable second system while keeping the current Windows.\n\n• Target volume: another partition (never the system drive); it is formatted and receives the image;\n• Boot name: the name shown in the Windows boot menu; Chinese is supported;\n• Channel difference: online = no reboot, format and boot-repair inside the current session; offline = reboot into the recovery environment;\n• Result: after the restore the default boot entry switches to this new system, and the boot menu still lets you choose either system; ideal for dual-boot.\n\nSafety: the target volume is overwritten; confirm it holds nothing you need to keep."
        }
        (Language::English, "pe") => "Install PE",
        (Language::English, "pe_hint") => {
            "Install PE deploys a self-contained PE backup/restore environment as an optional boot entry.\n\n• Boot mode: RAM disk (no partition; image loaded into a memory drive) or hard-disk boot (own partition, more stable);\n• WARNING: hard-disk boot FORMATS the chosen partition first (all data on it is lost) — confirm it holds nothing you need;\n• PE folder path / target volume: where the PE image (boot.wim) is placed; boot name is customizable;\n• Adds a BCD entry \"Windows PE (BackupRestore)\" without changing the current Windows default boot;\n• Purpose: when the system cannot boot, enter PE to back up, restore, install a second system, open the full program, or return to Windows;\n• Enable 'Reboot into PE' to verify it boots; PE uses the bundled standard PE image (not WinRE)."
        }
        _ => "",
    }
}

/// 文案表的完整性由**源码自身**校验，不维护第二份键列表：
/// 手工列的键清单迟早会与表脱同步，那时测试还在绿、界面已经露中文。
#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = include_str!("gui_text.rs");
    const UNKNOWN_KEY: &str = "no-such-key-exists";

    /// 两张表各有自己的键集（tooltip 只覆盖部分控件），所以按函数切开分别校验。
    fn sections() -> (&'static str, &'static str) {
        let split = SOURCE
            .find("pub(crate) fn ui_text(")
            .expect("ui_text must exist");
        (&SOURCE[..split], &SOURCE[split..])
    }

    fn keys_in(section: &str, language: &str) -> Vec<String> {
        let needle = format!("(Language::{language}, \"");
        let mut rest = section;
        let mut out: Vec<String> = Vec::new();
        while let Some(position) = rest.find(&needle) {
            rest = &rest[position + needle.len()..];
            match rest.find('"') {
                Some(end) => {
                    out.push(rest[..end].to_string());
                    rest = &rest[end..];
                }
                None => break,
            }
        }
        out.sort();
        out
    }

    #[test]
    fn every_key_has_both_chinese_and_english() {
        let (tooltips, labels) = sections();
        for (name, section) in [("tooltip_text", tooltips), ("ui_text", labels)] {
            let zh = keys_in(section, "Chinese");
            let en = keys_in(section, "English");
            assert!(!zh.is_empty(), "{name}: 一条键都没抓到");
            // 漏一条英文 = 该界面在英文下露中文；反之亦然。
            assert_eq!(zh, en, "{name}: 中英文案表的键集合必须完全一致");
        }
    }

    #[test]
    fn no_empty_translation_and_unknown_keys_fall_back_to_empty() {
        let (tooltips, labels) = sections();
        for key in keys_in(tooltips, "Chinese") {
            assert!(!tooltip_text(Language::Chinese, &key).is_empty(), "zh tooltip 为空：{key}");
            assert!(!tooltip_text(Language::English, &key).is_empty(), "en tooltip 为空：{key}");
        }
        for key in keys_in(labels, "Chinese") {
            assert!(!ui_text(Language::Chinese, &key).is_empty(), "zh 文案为空：{key}");
            assert!(!ui_text(Language::English, &key).is_empty(), "en 文案为空：{key}");
        }
        assert_eq!(ui_text(Language::Chinese, UNKNOWN_KEY), "");
        assert_eq!(ui_text(Language::English, UNKNOWN_KEY), "");
        assert_eq!(tooltip_text(Language::Chinese, UNKNOWN_KEY), "");
        assert_eq!(tooltip_text(Language::English, UNKNOWN_KEY), "");
    }
}
