//! 还原期间编辑引导方案，映像应用完成后才写入 BCD。

use backuprestore_core::{TaskError, VolumeIdentity};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::path::{Path, PathBuf};

pub(crate) const TIMEOUT_MAX: u32 = 999;
pub(crate) const TIMEOUT_DEFAULT: u32 = 10;
pub(crate) const NAME_MAX_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BootMenuEntry {
    pub drive_letter: char,
    pub display_name: String,
    pub selected: bool,
    pub is_restore_target: bool,
    pub volume: VolumeIdentity,
    /// 该卷在 BCD 里**已有** loader 时，它当前的 description。
    /// `Some` 表示这是既有条目：默认必须保留原名字，只有用户主动改过才写回；
    /// `None` 表示还没有条目（新建），此时 `display_name` 是待写入的默认名。
    /// 没有这个字段时，apply 会对所有选中项无条件 set description，
    /// 把用户给已有系统起的名字覆盖成 "Windows (C:)" 这类默认值。
    #[serde(default)]
    pub existing_name: Option<String>,
    /// 该卷的 loader 当前是 `{bootmgr}` 的默认启动项。
    ///
    /// 这一项同样不能取消：取消后 apply 会把 `/displayorder` 写成不含它的列表，
    /// 默认项随之漂移到刚还原的系统，而在线路径的验收又要求默认项不变——
    /// 结果是 BCD 已被改坏、操作却报失败且无回滚。在能取消之前就拦住。
    #[serde(default)]
    pub is_current_default: bool,
}

/// 还原完成后 `{bootmgr}.default` 的归属策略——**default 决策的唯一权威表述**
/// （2026-10-15 用户规则化，docs/20261015-0725 S1）。
///
/// 此前这个决策分散在四处、各自推断互不知晓：bcdboot 的 `/d` 取舍
/// （main.rs）、`assert_boot_default_preserved` 回读自愈、
/// `preserve_primary_boot_manager` 快照恢复、以及 apply 末尾的兜底
/// （`selected.contains(&previous.default)`，不在就静默落到还原目标）。
/// 现在由 main.rs/在线路径按已知的「目标是不是第二系统」显式设定并随方案落盘，
/// apply 只执行不推断；后三者降级为该决策的执行与校验细节。
///
/// 用户规则（与镜像从哪个分区备份而来无关，只看目标性质）：
/// - 还原本系统（目标=活动系统分区）→ default 指向还原后的自己；
/// - 新增第二系统（目标=其它分区）→ default 保持原主系统，新项只追加到菜单末尾。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DefaultPolicy {
    /// 重建唯一系统：default 指向本次还原的目标。镜像自认的分区（第一分区或
    /// 第三分区启动后自认 C:）不影响这一结论——这就是本机的主系统。
    PointAtRestoreTarget,
    /// 新增第二系统：default 保持原有主系统不动。
    PreserveCurrentDefault,
}

impl Default for DefaultPolicy {
    /// 缺省取保守侧：没有显式设定时绝不擅自把 default 改指到还原目标。
    fn default() -> Self {
        Self::PreserveCurrentDefault
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BootMenuPlan {
    pub entries: Vec<BootMenuEntry>,
    pub timeout_seconds: u32,
    /// default 归属策略。`#[serde(default)]` 让旧版本落盘的方案（没有这一字段）
    /// 能继续读取，取保守缺省；正常流程由调用方在 prepare 后立即显式设定。
    #[serde(default)]
    pub default_policy: DefaultPolicy,
}

impl BootMenuPlan {
    /// 该条目是否被钉住（不可取消勾选）：本次还原的目标，或当前的默认启动项。
    fn is_pinned(entry: &BootMenuEntry) -> bool {
        entry.is_restore_target || entry.is_current_default
    }
    pub(crate) fn toggle(&mut self, index: usize) -> bool {
        let Some(entry) = self.entries.get_mut(index) else {
            return false;
        };
        if Self::is_pinned(entry) {
            return false;
        }
        entry.selected = !entry.selected;
        true
    }
    pub(crate) fn move_up(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.entries.len() {
            return false;
        }
        self.entries.swap(index - 1, index);
        true
    }
    pub(crate) fn move_down(&mut self, index: usize) -> bool {
        if index >= self.entries.len().saturating_sub(1) {
            return false;
        }
        self.entries.swap(index, index + 1);
        true
    }
    /// 本次还原目标在菜单里的下标。默认顺序下它在最后，但用户可上移，
    /// 所以调用方一律用本方法定位，不要硬编码 0。
    pub(crate) fn restore_target_index(&self) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.is_restore_target)
    }
    pub(crate) fn rename(&mut self, index: usize, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty()
            || trimmed.chars().any(char::is_control)
            || index >= self.entries.len()
        {
            return false;
        }
        self.entries[index].display_name = trimmed.chars().take(NAME_MAX_CHARS).collect();
        true
    }
    pub(crate) fn validate(&self) -> Result<(), TaskError> {
        if self.entries.iter().filter(|e| e.is_restore_target).count() != 1
            || self
                .entries
                .iter()
                .any(|e| e.is_restore_target && !e.selected)
        {
            return Err(crate::err("本次还原系统必须且只能出现一次，不能取消"));
        }
        if self.timeout_seconds > TIMEOUT_MAX {
            return Err(crate::err("启动菜单等待时间超出范围"));
        }
        // 当前默认启动项（通常是主系统）不能被取消：它一旦从 displayorder 里消失，
        // 默认项就会漂移到刚还原的系统。toggle 已拦住交互路径，这里再守一道，
        // 防止落盘的方案被改过后绕过去。
        if self
            .entries
            .iter()
            .any(|e| e.is_current_default && !e.selected)
        {
            return Err(crate::err("当前默认启动的系统不能从启动菜单中移除"));
        }
        // 名字重复会让启动菜单出现两个同名项，用户无法区分该选哪个。
        for (i, entry) in self.entries.iter().enumerate() {
            if self.entries[..i]
                .iter()
                .any(|e| e.selected && entry.selected && e.display_name == entry.display_name)
            {
                return Err(crate::err("启动菜单出现同名项，请改名以便区分"));
            }
        }
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.display_name.trim().is_empty()
                || entry.display_name.chars().any(char::is_control)
                || entry.display_name.chars().count() > NAME_MAX_CHARS
                || !entry.volume.is_complete()
            {
                return Err(crate::err("引导方案名称或卷身份无效"));
            }
            if self.entries[..i]
                .iter()
                .any(|e| e.volume.same_partition(&entry.volume))
            {
                return Err(crate::err("引导方案重复系统分区"));
            }
        }
        Ok(())
    }
}

/// 新启动项命名撞名消解（S5，docs/20261015-0725）。
///
/// 拟用的名字与现有任一启动菜单项重名时，在末尾追加本地时间后缀
/// `-YYYYMMDDHHMM`；同一分钟内已有同基名项时退到秒，再退到秒+序号，
/// 保证绝不返回重名。基名超长时先截断，保证结果不超过 `NAME_MAX_CHARS`。
///
/// 为什么消解而不是报错：`validate()` 对同名项是硬失败，而重名在真实场景
/// 很常见——用镜像索引名（备份时间戳）当菜单名时二次还原、同一镜像装两个
/// 第二系统。与其让用户每次自己想新名字（还会把「为什么建不了任务」归因错
/// 地方），不如按用户要求的粒度自动避让，文本框回显后仍可手改。
pub(crate) fn resolve_menu_name_collision(
    preferred: &str,
    existing: &[String],
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let trimmed = preferred.trim();
    let collides = |candidate: &str| existing.iter().any(|name| name == candidate);
    if trimmed.is_empty() || !collides(trimmed) {
        return trimmed.chars().take(NAME_MAX_CHARS).collect();
    }
    for attempt in 0..100u32 {
        let suffix = match attempt {
            0 => now.format("%Y%m%d%H%M").to_string(),
            1 => now.format("%Y%m%d%H%M%S").to_string(),
            serial => format!("{}-{serial}", now.format("%Y%m%d%H%M%S")),
        };
        let dashed = format!("-{suffix}");
        let keep = NAME_MAX_CHARS.saturating_sub(dashed.chars().count());
        let candidate = format!("{}{dashed}", trimmed.chars().take(keep).collect::<String>());
        if !collides(&candidate) {
            return candidate;
        }
    }
    trimmed.chars().take(NAME_MAX_CHARS).collect()
}

/// 枚举当前 BCD 里所有 loader 的 description，用于识别"哪些卷已有条目、原名叫什么"。
/// 失败只返回 None（调用方退化成"无既有名字"），不阻断还原。
#[cfg(windows)]
fn existing_boot_menu_names(log: &Path) -> Option<String> {
    let esp = crate::windows_prepare::esp_identity_without_drive_letter().ok()?;
    let letter = crate::windows_prepare::ensure_volume_mounted(&esp, 'S', log).ok()?;
    let store = format!("{letter}:\\EFI\\Microsoft\\Boot\\BCD");
    crate::capture_logged(
        "bcdedit.exe",
        &["/store", &store, "/enum", "all", "/v"],
        log,
    )
    .ok()
}

/// 读当前 BCD 里所有分区 loader 的描述（S5 新启动项撞名预检的 live 数据源）。
/// 读不到（ESP 不可达等）返回空表：调用方退化成「不消解」，由 validate 的
/// 同名守卫或 apply 侧兜底，不阻断还原。
#[cfg(windows)]
pub(crate) fn existing_loader_descriptions(log: &Path) -> Vec<String> {
    existing_boot_menu_names(log)
        .map(|output| all_loader_descriptions(&output))
        .unwrap_or_default()
}

/// 判断一个 bcdedit 对象块是不是「分区 loader」（不指定分区）。
///
/// 判据：`path` 指向 winload.efi、有设备类字段、且不带 `ramdiskdevice`
/// （ramdisk 选项也有 winload 路径特征，但不是启动菜单里可选的系统项）。
/// `block_is_loader_for` 在其上追加分区针；`all_loader_descriptions`
/// 直接用它列全量菜单项名字。
fn block_is_partition_loader(block: &str) -> bool {
    let lower = block.to_ascii_lowercase();
    let lines: Vec<&str> = lower.lines().map(str::trim).collect();
    // ramdisk 条目：设备字段是 ramdiskdevice，不是分区加载器，必须先排除。
    if lines.iter().any(|line| line.starts_with("ramdiskdevice")) {
        return false;
    }
    let is_loader = lines.iter().any(|line| {
        (line.starts_with("path") || line.starts_with("路径")) && line.ends_with("\\winload.efi")
    });
    let has_device = lines.iter().any(|line| {
        line.starts_with("device")
            || line.starts_with("osdevice")
            || line.starts_with("设备")
            || line.starts_with("os 设备")
            || line.starts_with("操作系统设备")
    });
    is_loader && has_device
}

/// 判断一个 bcdedit 对象块是否是"指向该分区的 Windows 加载器"。
///
/// 判据在 `block_is_partition_loader` 之上追加"至少一个设备字段指向该分区"。
///
/// 这里刻意用"至少一个设备字段"而不是"device 与 osdevice 都要有"：
/// 本地化字段名无法穷举（中文 bcdedit 的 osdevice 实测写法是「os 设备」——
/// 带空格，见 main.rs 里已实战验证的解析器；曾误写成「操作系统设备」），
/// 一旦某个拼法猜错，AND 语义会让该语言下**所有** loader 都匹配不上，
/// 引导修复直接全线失效。OR 语义下猜错一个标签只是少一项证据，仍能工作。
/// 行首匹配也用 starts_with 而非精确相等，兼容「os 设备」这种含空格的键名。
fn block_is_loader_for(block: &str, needle: &str) -> bool {
    if !block_is_partition_loader(block) {
        return false;
    }
    block.to_ascii_lowercase().lines().any(|line| {
        let line = line.trim();
        (line.starts_with("device")
            || line.starts_with("osdevice")
            || line.starts_with("设备")
            || line.starts_with("os 设备")
            || line.starts_with("操作系统设备"))
            && line.contains(needle)
    })
}

/// 从 PE 头部字节里取出资源目录（`RT_VERSION`）的 RVA 与大小。
///
/// 只解析到头信息，不碰资源内容，所以调用方只需读文件前若干 KB。
/// PE32 与 PE32+ 的数据目录位置不同，靠 optional header 的 magic 区分。
fn pe_resource_directory(head: &[u8]) -> Option<(u32, u32)> {
    if head.len() < 0x40 || &head[0..2] != b"MZ" {
        return None;
    }
    let pe_offset = u32::from_le_bytes(head.get(0x3C..0x40)?.try_into().ok()?) as usize;
    let pe = head.get(pe_offset..pe_offset + 24)?;
    if &pe[0..4] != b"PE\0\0" {
        return None;
    }
    let optional_size = u16::from_le_bytes(pe.get(20..22)?.try_into().ok()?) as usize;
    let optional = head.get(pe_offset + 24..pe_offset + 24 + optional_size)?;
    if optional.len() < 2 {
        return None;
    }
    let magic = u16::from_le_bytes(optional[0..2].try_into().ok()?);
    // PE32 的数据目录在 optional header 偏移 96，PE32+ 在 112。
    let directory = match magic {
        0x10b => 96,
        0x20b => 112,
        _ => return None,
    };
    // 资源目录是第 3 个数据目录（下标 2），每项 8 字节。
    let entry = optional.get(directory + 16..directory + 24)?;
    Some((
        u32::from_le_bytes(entry[0..4].try_into().ok()?),
        u32::from_le_bytes(entry[4..8].try_into().ok()?),
    ))
}

/// 按节表把 RVA 换算成文件偏移。找不到所在节返回 None。
fn pe_rva_to_offset(head: &[u8], rva: u32) -> Option<usize> {
    let pe_offset = u32::from_le_bytes(head.get(0x3C..0x40)?.try_into().ok()?) as usize;
    let pe = head.get(pe_offset..pe_offset + 24)?;
    if &pe[0..4] != b"PE\0\0" {
        return None;
    }
    let section_count = u16::from_le_bytes(pe.get(6..8)?.try_into().ok()?) as usize;
    let optional_size = u16::from_le_bytes(pe.get(20..22)?.try_into().ok()?) as usize;
    let table = pe_offset + 24 + optional_size;
    for index in 0..section_count {
        let section = head.get(table + index * 40..table + index * 40 + 40)?;
        let virtual_size = u32::from_le_bytes(section[8..12].try_into().ok()?);
        let virtual_address = u32::from_le_bytes(section[12..16].try_into().ok()?);
        let raw_size = u32::from_le_bytes(section[16..20].try_into().ok()?);
        let raw_pointer = u32::from_le_bytes(section[20..24].try_into().ok()?);
        let span = virtual_size.max(raw_size);
        if rva >= virtual_address && rva < virtual_address + span {
            return Some(raw_pointer as usize + (rva - virtual_address) as usize);
        }
    }
    None
}

/// 在资源目录树里向下找类型 16（`RT_VERSION`）的叶子数据入口。
///
/// 目录是三层：类型 → 名字 → 语言。每层条目 `(id/name 标志, 偏移)`，
/// 偏移最高位为 1 表示指向下一层目录，为 0 表示指向数据入口。
///
/// `base` 既是层号也是深度上限：PE 规范只有三层，超过就认定目录畸形。
/// 必须有这个上限 + "子节点偏移必须前进"的检查：这些字节来自目标卷上的文件，
/// 一个损坏或被构造的 ntoskrnl.exe 只要让某层条目指回自身，无界递归就会
/// 栈溢出并 abort 整个进程——而这段代码跑在格式化之前的 discover 里，
/// 等于让一个坏文件直接终止还原，而不是退化成 "Windows (X:)" 兜底名。
fn pe_version_resource_offset(resource: &[u8], base: usize, node: usize) -> Option<usize> {
    if base > 2 {
        return None;
    }
    let header = resource.get(node..node + 16)?;
    let named = u16::from_le_bytes(header[12..14].try_into().ok()?) as usize;
    let identified = u16::from_le_bytes(header[14..16].try_into().ok()?) as usize;
    let entries = named + identified;
    for index in 0..entries {
        let entry = resource.get(node + 16 + index * 8..node + 16 + index * 8 + 8)?;
        let name_or_id = u32::from_le_bytes(entry[0..4].try_into().ok()?);
        let offset = u32::from_le_bytes(entry[4..8].try_into().ok()?);
        // 顶层只认类型 16；下层（名字/语言层）取第一个即可，版本资源只有一个。
        if base == 0 && name_or_id != 16 {
            continue;
        }
        if offset & 0x8000_0000 != 0 {
            let child = (offset & 0x7FFF_FFFF) as usize;
            // 子节点必须严格向前，否则自指/回指的畸形目录会无限递归。
            if child <= node {
                continue;
            }
            if let Some(found) = pe_version_resource_offset(resource, base + 1, child) {
                return Some(found);
            }
        } else {
            let data = resource.get(offset as usize..offset as usize + 16)?;
            let to_rva = u32::from_le_bytes(data[0..4].try_into().ok()?);
            let size = u32::from_le_bytes(data[4..8].try_into().ok()?);
            if size >= 4 {
                return Some(to_rva as usize);
            }
        }
    }
    None
}

/// 从 `VS_VERSIONINFO` 里取出文件版本四元组 (major, minor, build, ubr)。
fn pe_version_quad(data: &[u8]) -> Option<(u16, u16, u16, u16)> {
    // 头 6 字节 + UTF-16 "VS_VERSION_INFO" + 空字符，再 4 字节对齐。
    if data.len() < 6 {
        return None;
    }
    let value_length = u16::from_le_bytes(data[2..4].try_into().ok()?) as usize;
    let mut cursor = 6;
    let mut units = 0usize;
    while cursor + 1 < data.len() {
        let unit = u16::from_le_bytes(data[cursor..cursor + 2].try_into().ok()?);
        cursor += 2;
        units += 1;
        if unit == 0 {
            break;
        }
    }
    if units == 0 {
        return None;
    }
    // 对齐到 4 字节边界，紧接着就是 VS_FIXEDFILEINFO。
    cursor = (cursor + 3) & !3;
    if value_length < 0x34 {
        return None;
    }
    let fixed = data.get(cursor..cursor + 0x34)?;
    if u32::from_le_bytes(fixed[0..4].try_into().ok()?) != 0xFEEF_04BD {
        return None;
    }
    let ms = u32::from_le_bytes(fixed[8..12].try_into().ok()?);
    let ls = u32::from_le_bytes(fixed[12..16].try_into().ok()?);
    Some((
        (ms >> 16) as u16,
        (ms & 0xFFFF) as u16,
        (ls >> 16) as u16,
        (ls & 0xFFFF) as u16,
    ))
}

/// 读某卷的系统 build 号，用于给"硬盘上有 Windows 但启动菜单没条目"的卷起名。
///
/// 名字必须带版本号：否则多个未登记系统会得到 `Windows (E:)` 这类只差盘符的名字，
/// 用户照样分不清；用户明确要求 `Windows 11 26100` 这种形式。
/// 数据源选 `ntoskrnl.exe` 的 PE 版本而不是注册表：纯文件读取、不需要管理员、
/// 不 load/unload hive（卸载失败会占用句柄，影响后续所有操作），WinRE/PE 里同样可用。
#[cfg(windows)]
fn volume_build_tag(root: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let path = root.join("Windows").join("System32").join("ntoskrnl.exe");
    let mut file = std::fs::File::open(&path).ok()?;
    // 头 + 节表远小于 8KB（ntoskrnl 也只要几百字节），先只读这些。
    let mut head = vec![0u8; 8192];
    let read = file.read(&mut head).ok()?;
    head.truncate(read);
    let (resource_rva, _) = pe_resource_directory(&head)?;
    if resource_rva == 0 {
        return None;
    }
    let resource_offset = pe_rva_to_offset(&head, resource_rva)?;
    let leaf_rva = {
        let mut buffer = vec![0u8; 64 * 1024];
        file.seek(SeekFrom::Start(resource_offset as u64)).ok()?;
        let read = file.read(&mut buffer).ok()?;
        buffer.truncate(read);
        pe_version_resource_offset(&buffer, 0, 0)?
    };
    let data_offset = pe_rva_to_offset(&head, leaf_rva as u32)?;
    file.seek(SeekFrom::Start(data_offset as u64)).ok()?;
    let mut data = vec![0u8; 4096];
    let read = file.read(&mut data).ok()?;
    data.truncate(read);
    let (_major, _minor, build, ubr) = pe_version_quad(&data)?;
    // build >= 22000 是 Windows 11；更早的是 Windows 10。
    let product = if build >= 22000 {
        "Windows 11"
    } else {
        "Windows 10"
    };
    Some(format!("{product} {build}.{ubr}"))
}

/// 读取某分区 loader 当前的 description（启动菜单里显示的名字）。
///
/// 返回 `None` 表示该分区**没有** loader（将新建），此时才该由我们起名。
/// 既有条目的名字必须原样保留，否则会把用户自己起的名字覆盖掉。
///
/// 本地化标签无法穷举，所以这里列出已知的几种拼法（「说明」「描述」），
/// 且**读不到名字不等于没有条目**——调用方据 `block_is_loader_for` 判定条目存在性，
/// 本函数返回 None 时 apply 会走"按需写入"而不是"覆盖"，不会误删用户的名字。
pub(crate) fn loader_description_for_partition(output: &str, letter: char) -> Option<String> {
    let needle = format!("partition={}: ", letter.to_ascii_lowercase());
    let needle = needle.trim_end();
    for block in output.replace("\r\n", "\n").split("\n\n") {
        if !block_is_loader_for(block, needle) {
            continue;
        }
        // description 的值是原文，不能走小写副本：中文名与大小写混排都会被抹掉。
        for line in block.lines() {
            let trimmed = line.trim();
            let Some((key, value)) = trimmed.split_once(char::is_whitespace) else {
                continue;
            };
            let key_lower = key.to_ascii_lowercase();
            if matches!(key_lower.as_str(), "description" | "说明" | "描述") {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// 从 `bcdedit /enum all /v` 输出里提取**所有分区 loader** 的 description。
///
/// 与 `loader_description_for_partition` 的分工：那个按分区找名字（给既有
/// 条目保名），这个列全量名字（给新条目避让，S5 撞名消解的输入）。
/// 读不到名字的块直接跳过——列证宁可少，不要把空串当成一个候选名。
pub(crate) fn all_loader_descriptions(output: &str) -> Vec<String> {
    let mut names = Vec::new();
    for block in output.replace("\r\n", "\n").split("\n\n") {
        if !block_is_partition_loader(block) {
            continue;
        }
        for line in block.lines() {
            let trimmed = line.trim();
            let Some((key, value)) = trimmed.split_once(char::is_whitespace) else {
                continue;
            };
            let key_lower = key.to_ascii_lowercase();
            if matches!(key_lower.as_str(), "description" | "说明" | "描述") {
                let name = value.trim().to_string();
                if !name.is_empty() {
                    names.push(name);
                }
                break;
            }
        }
    }
    names
}

/// 对象必须唯一；同卷存在多个加载器时拒绝按枚举顺序猜测。
pub(crate) fn loader_identifier_for_partition(
    output: &str,
    letter: char,
) -> Result<Option<String>, TaskError> {
    let needle = format!("partition={}: ", letter.to_ascii_lowercase());
    let needle = needle.trim_end();
    let mut found = Vec::new();
    for block in output.replace("\r\n", "\n").split("\n\n") {
        if !block_is_loader_for(block, needle) {
            continue;
        }
        if let Some(id) = block.lines().find_map(crate::bcd_identifier_from_line) {
            found.push(id.to_string());
        }
    }
    if found.len() > 1 {
        return Err(crate::err("同一系统分区有多个引导项，拒绝猜测"));
    }
    Ok(found.pop())
}

pub(crate) fn render_entries(plan: &BootMenuPlan, cursor: usize) -> String {
    plan.entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            format!(
                "{} [{}] {}. {} ({}:){}",
                if i == cursor { ">" } else { " " },
                if e.selected { "√" } else { " " },
                i + 1,
                e.display_name,
                e.drive_letter,
                if e.is_restore_target {
                    " [当前正在还原中的系统·Step 4生效]"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// 落盘的引导方案：带上 task_id 防止拿上一个任务的候选来改本次还原。
#[cfg(windows)]
#[derive(Debug, Serialize, Deserialize)]
struct SavedPlan {
    task_id: String,
    plan: BootMenuPlan,
}

#[cfg(windows)]
pub(crate) struct BootMenuSession {
    state: std::sync::Mutex<SessionState>,
    path: PathBuf,
    task_id: String,
    log: PathBuf,
}
#[cfg(windows)]
struct SessionState {
    plan: BootMenuPlan,
    editable: bool,
    error: Option<String>,
}

#[cfg(windows)]
impl BootMenuSession {
    /// `suggested_target_name` 是调用方从镜像元数据里拿到的版本化名字
    /// （如 `Windows 11 Pro 26100.9549`），只用于给"本次还原的目标"起名；
    /// 传 None 时退化成读目标卷版本或通用名。已有 loader 的条目不受它影响。
    pub(crate) fn prepare(
        target: &VolumeIdentity,
        task_id: &str,
        dir: &Path,
        log: &Path,
        suggested_target_name: Option<&str>,
    ) -> Result<std::sync::Arc<Self>, TaskError> {
        let path = dir.join("boot-menu-plan.json");
        // 不用 try_exists()：它在 Windows 上对"拒绝访问"和"不存在"都报错，
        // 而这里只是想知道有没有上次留下的方案。直接读，读不到就当没有。
        let saved = backuprestore_core::read_json::<SavedPlan>(&path).ok();
        let mut plan = if let Some(saved) = saved {
            if saved.task_id != task_id
                || !saved
                    .plan
                    .entries
                    .iter()
                    .any(|e| e.is_restore_target && e.volume.same_partition(target))
            {
                return Err(crate::err("引导方案不属于当前还原任务"));
            }
            saved.plan
        } else {
            let mut fresh = discover(target, log)?;
            // 只有"本次还原的目标"这一项采用调用方给的版本化名字；
            // 其它条目的名字来自 BCD 现值或各卷自身版本，不能被它覆盖。
            if let Some(name) = suggested_target_name
                && let Some(entry) = fresh
                    .entries
                    .iter_mut()
                    .find(|entry| entry.is_restore_target)
                && entry.existing_name.is_none()
            {
                entry.display_name = name.trim().chars().take(NAME_MAX_CHARS).collect();
            }
            // S5 撞名消解（方案内兜底）：目标名与其它条目重名时追加
            // -YYYYMMDDHHMM。GUI 侧与离线 prepare 侧已各对照 live BCD 消解过
            // 一次（幂等）；这里对照「本方案其它条目」——覆盖"卷上有 Windows
            // 但 BCD 里没有条目"的候选：那种卷不在 live BCD 描述里，前两处
            // 消解都看不到它，而它确实会进最终菜单。不消解的代价是 apply 的
            // validate 在 STEP 4 才以「同名项」报错——那时已格式化已应用。
            if let Some(position) = fresh.restore_target_index() {
                let others: Vec<String> = fresh
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != position)
                    .map(|(_, entry)| entry.display_name.clone())
                    .collect();
                let target_name = fresh.entries[position].display_name.clone();
                let resolved =
                    resolve_menu_name_collision(&target_name, &others, chrono::Local::now());
                if resolved != target_name {
                    crate::append_log(
                        log,
                        &format!("启动菜单目标名与其它菜单项重名，已消解：{target_name} -> {resolved}"),
                    )?;
                    fresh.entries[position].display_name = resolved;
                }
            }
            fresh
        };
        for entry in &mut plan.entries {
            let letter = crate::windows_prepare::ensure_volume_mounted(
                &entry.volume,
                entry.drive_letter,
                log,
            )?;
            let actual = crate::windows_prepare::volume_identity(letter)?;
            entry.volume = crate::pe_safety::rebind_after_boot(&entry.volume, &actual, false)?;
            entry.drive_letter = letter;
        }
        plan.validate()?;
        backuprestore_core::write_json_atomic(
            &path,
            &SavedPlan {
                task_id: task_id.into(),
                plan: plan.clone(),
            },
        )?;
        crate::append_log(
            log,
            &format!(
                "引导修复方案已准备：{} 个系统；本次目标必选；无倒计时",
                plan.entries.len()
            ),
        )?;
        Ok(std::sync::Arc::new(Self {
            state: std::sync::Mutex::new(SessionState {
                plan,
                editable: true,
                error: None,
            }),
            path,
            task_id: task_id.into(),
            log: log.into(),
        }))
    }
    pub(crate) fn snapshot(&self) -> (BootMenuPlan, bool) {
        let state = self.state.lock().unwrap();
        (state.plan.clone(), state.editable)
    }
    pub(crate) fn edit(&self, change: impl FnOnce(&mut BootMenuPlan)) {
        let mut state = self.state.lock().unwrap();
        if !state.editable {
            return;
        }
        let mut next = state.plan.clone();
        change(&mut next);
        let result = next.validate().and_then(|_| {
            backuprestore_core::write_json_atomic(
                &self.path,
                &SavedPlan {
                    task_id: self.task_id.clone(),
                    plan: next.clone(),
                },
            )
        });
        match result {
            Ok(()) => {
                state.plan = next;
                let _ = crate::append_log(&self.log, "引导修复选择已保存，恢复继续运行");
            }
            Err(error) => {
                state.error = Some(error.to_string());
                let _ = crate::append_log(&self.log, &format!("引导修复方案保存失败：{error}"));
            }
        }
    }
    pub(crate) fn freeze(&self) -> Result<BootMenuPlan, TaskError> {
        let mut state = self.state.lock().unwrap();
        state.editable = false;
        if let Some(error) = &state.error {
            return Err(crate::err(error));
        }
        state.plan.validate()?;
        Ok(state.plan.clone())
    }
    /// 设定 default 归属策略——「单一权威」的写入入口（docs/20261015-0725 S1）：
    /// main.rs/在线路径在 prepare 之后按「目标是不是第二系统」显式调用，
    /// apply 不再自行推断。设定随方案落盘，续跑任务重启后仍然有效。
    pub(crate) fn set_default_policy(&self, policy: DefaultPolicy) {
        let mut state = self.state.lock().unwrap();
        state.plan.default_policy = policy;
        let saved = backuprestore_core::write_json_atomic(
            &self.path,
            &SavedPlan {
                task_id: self.task_id.clone(),
                plan: state.plan.clone(),
            },
        );
        match saved {
            Ok(()) => {
                let _ = crate::append_log(&self.log, &format!("默认启动项策略：{policy:?}"));
            }
            Err(error) => {
                let _ =
                    crate::append_log(&self.log, &format!("默认启动项策略落盘失败：{error}"));
            }
        }
    }
}

#[cfg(windows)]
fn discover(target: &VolumeIdentity, log: &Path) -> Result<BootMenuPlan, TaskError> {
    let target_letter = target
        .drive_letter
        .ok_or_else(|| crate::err("还原目标盘符缺失"))?;
    // 默认顺序沿用既有约定：原有系统在前，本次还原的目标追加在后。
    // （早先版本把目标放在首位，导致启动菜单第一项变成新系统、而默认启动项仍是
    // 主系统，两者不一致会让用户困惑；现在默认与 run10 验收过的行为一致，
    // 用户想置前可用「上移」自己调整。）
    //
    // 候选卷要先全部挂上盘符，再统一读 BCD：没有盘符的 Windows 卷在 bcdedit
    // 里显示成 `device partition=\Device\HarddiskVolume4`，按盘符去查必然查不到，
    // 于是被误判成"没有条目"，apply 就会覆盖掉用户给它起的名字。
    let mut mounted = Vec::new();
    let paths = crate::windows_prepare::native_volume_paths()?;
    for path in paths {
        let identity = match crate::windows_prepare::volume_identity_at_path(&path, path.clone()) {
            Ok(identity) => identity,
            Err(error) => {
                crate::append_log(log, &format!("引导候选卷身份不可读，跳过：{path}；{error}"))?;
                continue;
            }
        };
        if identity.is_reserved_partition()
            || identity.same_partition(target)
            || identity.drive_letter == Some('X')
        {
            continue;
        }
        let root = Path::new(&path);
        // 探测失败（而不是"探测结果是否"）时跳过该候选卷，不让整个还原失败。
        //
        // 实机（2026-10-12 在线还原）踩过：枚举到当前运行的系统卷 C: 时，
        // probe_file 里的 fs::symlink_metadata 对活动卷根报共享违例
        // （ERROR_SHARING_VIOLATION），而 probe_file 把它当 Err 传播。
        // 此前这段只在 WinRE 路径运行，候选里从来不含活动卷，所以没暴露。
        // C: 本来就在启动菜单里，跳过它不影响功能，但能读到的卷仍要正常登记。
        let is_windows = match crate::pe_safety::target_is_windows_volume(root) {
            Ok(is_windows) => is_windows,
            Err(error) => {
                crate::append_log(log, &format!("引导候选卷探测失败，跳过：{path}；{error}"))?;
                continue;
            }
        };
        let has_loader =
            match crate::pe_safety::probe_file(root, &["Windows", "System32", "winload.efi"]) {
                Ok(has_loader) => has_loader,
                Err(error) => {
                    crate::append_log(log, &format!("引导候选卷探测失败，跳过：{path}；{error}"))?;
                    continue;
                }
            };
        if !is_windows || !has_loader {
            continue;
        }
        // 常规系统必须含离线恢复组件；硬盘版 PE 保留现有入口，不重新登记。
        let has_reagentc =
            match crate::pe_safety::probe_file(root, &["Windows", "System32", "reagentc.exe"]) {
                Ok(has_reagentc) => has_reagentc,
                Err(error) => {
                    crate::append_log(log, &format!("引导候选卷探测失败，跳过：{path}；{error}"))?;
                    continue;
                }
            };
        if !has_reagentc {
            crate::append_log(
                log,
                &format!("卷缺少常规 Windows 恢复组件，保留现有入口：{path}"),
            )?;
            continue;
        }
        let letter = crate::windows_prepare::ensure_volume_mounted(&identity, 'U', log)?;
        let actual = crate::windows_prepare::volume_identity(letter)?;
        let volume = crate::pe_safety::rebind_after_boot(&identity, &actual, false)?;
        mounted.push((letter, volume, path.clone()));
    }
    // 所有候选卷都挂好盘符之后才读 BCD，这样 bcdedit 报的是 `partition=X:`
    // 而不是 \Device\HarddiskVolumeN，既有条目才查得到。
    // 读不到（ESP 不可达等）不阻断：退化成"没有既有名字"，apply 按需写入。
    let bcd_all = existing_boot_menu_names(log);
    let default_id = bcd_all
        .as_deref()
        .and_then(crate::parse_boot_manager_state)
        .map(|state| state.default);
    let mut entries = Vec::new();
    for (letter, volume, path) in mounted {
        let root = Path::new(&path);
        // 该卷已有 loader 就用它当前的名字，不要另起一个——用户可能特意命名过。
        let existing = bcd_all
            .as_deref()
            .and_then(|output| loader_description_for_partition(output, letter));
        // 这一项是不是当前的默认启动系统：是就钉住，不允许取消勾选
        // （取消会让默认项漂移到刚还原的系统）。
        let is_current_default = match (&bcd_all, &default_id) {
            (Some(output), Some(default)) => loader_identifier_for_partition(output, letter)?
                .is_some_and(|id| id.eq_ignore_ascii_case(default)),
            _ => false,
        };
        // 没有条目的（硬盘上有 Windows 但启动菜单里没有它）：按用户要求起一个
        // 带版本号的名字，避免多个未登记系统只差盘符、用户分不清。
        // 读不到版本才退化成带盘符的名字，保证永不为空。
        let display_name = existing.clone().unwrap_or_else(|| {
            volume_build_tag(root).unwrap_or_else(|| format!("Windows ({letter}:)"))
        });
        entries.push(BootMenuEntry {
            drive_letter: letter,
            display_name,
            selected: true,
            is_restore_target: false,
            volume,
            existing_name: existing,
            is_current_default,
        });
    }
    // 还原目标放在最后：保持「原有系统在前」的默认顺序，且它是必选项不能取消。
    // 目标卷此刻正要被格式化，BCD 里不可能有它的 loader，所以 existing_name 为 None
    // ——这正是"该由我们起名"的那一类。
    entries.push(BootMenuEntry {
        drive_letter: target_letter,
        display_name: "本次还原的 Windows".into(),
        selected: true,
        is_restore_target: true,
        volume: target.clone(),
        existing_name: None,
        is_current_default: false,
    });
    // 默认名可能撞车（两个同补丁级别的未登记系统会得到同一个版本串）。
    // 撞了就补盘符区分，否则启动菜单里出现两个一模一样的项，用户没法选。
    // validate 会拒绝重名，所以这里必须在返回前消解掉。
    for index in 0..entries.len() {
        let name = entries[index].display_name.clone();
        let duplicated = entries[..index].iter().any(|e| e.display_name == name);
        if duplicated {
            let letter = entries[index].drive_letter;
            let disambiguated = format!("{name} ({letter}:)");
            entries[index].display_name = disambiguated.chars().take(NAME_MAX_CHARS).collect();
            crate::append_log(
                log,
                &format!(
                    "引导菜单默认名重复，已加盘符区分：{name} -> {}",
                    entries[index].display_name
                ),
            )?;
        }
    }
    Ok(BootMenuPlan {
        entries,
        timeout_seconds: TIMEOUT_DEFAULT,
        // discover 不知道场景，先取保守缺省；main.rs/在线路径在 prepare 返回后
        // 按目标性质立即显式设定（DefaultPolicy 的文档即规则原文）。
        default_policy: DefaultPolicy::default(),
    })
}

#[cfg(windows)]
pub(crate) fn apply(
    plan: &BootMenuPlan,
    esp: &Path,
    log: &Path,
    resume: Option<&crate::boot_entry::ReBootEntry>,
) -> Result<(), TaskError> {
    plan.validate()?;
    // 盘符是临时参数、不是身份。从方案准备好到真正写入 BCD 之间，目标卷可能被
    // 重新挂到别的盘符（在线路径 STEP 3.5 就挂到 T:），所以这里必须按稳定 GUID
    // 重新取当前盘符。用 prepare 时记的旧盘符去找 loader，会一个都找不到——
    // 这正是 2026-10-12 在线路径接入后第四步必败的根因。
    let mut plan = plan.clone();
    for entry in &mut plan.entries {
        let letter =
            crate::windows_prepare::ensure_volume_mounted(&entry.volume, entry.drive_letter, log)?;
        let actual = crate::windows_prepare::volume_identity(letter)?;
        entry.volume = crate::pe_safety::rebind_after_boot(&entry.volume, &actual, false)?;
        entry.drive_letter = letter;
    }
    let store = esp
        .join("EFI\\Microsoft\\Boot\\BCD")
        .to_string_lossy()
        .into_owned();
    let mut output = crate::capture_logged(
        "bcdedit.exe",
        &["/store", &store, "/enum", "all", "/v"],
        log,
    )?;
    let previous = crate::parse_boot_manager_state(&output)
        .ok_or_else(|| crate::err("引导修复前启动管理器无效"))?;
    for entry in plan.entries.iter().filter(|e| e.selected) {
        let actual = crate::windows_prepare::volume_identity(entry.drive_letter)?;
        crate::pe_safety::verify_binding(&entry.volume, &actual, false)?;
        if !crate::pe_safety::target_is_windows_volume(&PathBuf::from(format!(
            "{}:\\",
            entry.drive_letter
        )))? {
            return Err(crate::err("引导修复时所选系统已不存在"));
        }
        if loader_identifier_for_partition(&output, entry.drive_letter)?.is_none() {
            if let Some(re) = resume {
                crate::boot_entry::rearm(re, log)?;
            }
            crate::run_logged(
                "bcdboot.exe",
                &[
                    &format!("{}:\\Windows", entry.drive_letter),
                    "/s",
                    &esp.to_string_lossy(),
                    "/f",
                    "UEFI",
                    // /d 保留现有默认启动项：这里是给"用户勾选但还没有 loader 的
                    // 系统"补建条目，无论如何都不该顺手改默认启动项。本函数末尾
                    // 自己会按方案决定 default（原 default 仍在菜单则保留）。
                    "/d",
                    "/addlast",
                ],
                log,
            )?;
            if let Some(re) = resume {
                crate::boot_entry::rearm(re, log)?;
            }
            output = crate::capture_logged(
                "bcdedit.exe",
                &["/store", &store, "/enum", "all", "/v"],
                log,
            )?;
        }
    }
    let mut selected = Vec::new();
    let mut candidates = Vec::new();
    let mut target_id = None;
    for entry in &plan.entries {
        let id = loader_identifier_for_partition(&output, entry.drive_letter)?;
        if let Some(id) = &id {
            candidates.push(id.clone());
        }
        if !entry.selected {
            continue;
        }
        let id = id.ok_or_else(|| crate::err("引导修复后找不到系统加载器"))?;
        // 名字只在两种情况下写：该条目是本次新建的（原本没有 loader），
        // 或用户在弹窗里主动改过名。既有条目且名字没变就一个字都不写——
        // 否则会把用户给已有系统起的名字覆盖成默认值。
        let renamed = entry.existing_name.as_deref() != Some(entry.display_name.as_str());
        if entry.existing_name.is_none() || renamed {
            crate::run_logged(
                "bcdedit.exe",
                &[
                    "/store",
                    &store,
                    "/set",
                    &id,
                    "description",
                    &entry.display_name,
                ],
                log,
            )?;
            crate::append_log(
                log,
                &format!(
                    "引导菜单名称写入：{} -> {}",
                    entry.existing_name.as_deref().unwrap_or("（新建）"),
                    entry.display_name
                ),
            )?;
        } else {
            crate::append_log(
                log,
                &format!("引导菜单名称保持不变：{}", entry.display_name),
            )?;
        }
        if entry.is_restore_target {
            target_id = Some(id.clone());
        }
        selected.push(id);
    }
    // 不在常规 Windows 候选中的入口（包括硬盘版 PE）保留在原菜单中。
    for id in &previous.display_order {
        if !candidates.contains(id) && output.contains(id) && !selected.contains(id) {
            selected.push(id.clone());
        }
    }
    // default 归属的唯一决策点（2026-10-15 用户规则化，docs/20261015-0725 S1）：
    // 策略由 main.rs/在线路径按「目标是不是第二系统」显式设定并随方案落盘，
    // 这里只执行、不推断，且每条分支都写日志——A 场景尤其需要显式留下
    // 「default 指向还原后的自己」，避免事后只能从 BCD 现状反推当时的意图。
    let (default, policy_note) = match plan.default_policy {
        DefaultPolicy::PointAtRestoreTarget => {
            // 重建唯一系统：default 指向还原后的自己。注意这覆盖了一种旧行为——
            // 旧代码在「原 default 恰好还在菜单里」时会保留原值，若用户还原前
            // 的默认项是另一套系统（如第二系统），还原后 default 仍指那套，
            // 与「还原本系统后默认进自己」的规则相悖。镜像从哪个分区备份而来
            // 不影响这一结论。
            let id = target_id
                .clone()
                .ok_or_else(|| crate::err("缺少必选目标加载器"))?;
            (id, "策略=指向还原目标（重建唯一系统）".to_string())
        }
        DefaultPolicy::PreserveCurrentDefault => {
            if selected.contains(&previous.default) {
                (
                    previous.default.clone(),
                    "原默认项仍在最终菜单，保留".to_string(),
                )
            } else {
                // 第二系统场景下原默认项不在菜单=异常：主系统 loader 没被探测进
                // 菜单，或落盘方案被改过绕过了 validate 的勾选守卫。最终行为仍是
                // 落到还原目标（与旧代码一致），但先留一道 WARN——2026-10-13
                // 事故正是这种「静默漂移」：default 留在半成品第二系统上，用户
                // 每次开机被拖进去。是否升级为直接报错拒绝见 docs/20261015-0725
                // S3，边界待用户裁定；裁定前不改行为，只加留痕。
                crate::append_log(
                    log,
                    &format!(
                        "WARN：原默认项 {} 不在最终菜单，default 将落到还原目标",
                        previous.default
                    ),
                )?;
                (
                    target_id
                        .clone()
                        .ok_or_else(|| crate::err("缺少必选目标加载器"))?,
                    "原默认项不在菜单，落到还原目标（已 WARN）".to_string(),
                )
            }
        }
    };
    crate::append_log(log, &format!("默认启动项决策：{default}；{policy_note}"))?;
    crate::run_logged(
        "bcdedit.exe",
        &["/store", &store, "/default", &default],
        log,
    )?;
    if let Some(id) = &previous.resume_object {
        crate::run_logged(
            "bcdedit.exe",
            &["/store", &store, "/set", "{bootmgr}", "resumeobject", id],
            log,
        )?;
    } else {
        // 快照里没有 resumeobject 时必须删掉 bcdboot 新写的值，否则回读校验
        // （resume_object 必须等于快照值 None）判定不一致而报失败，
        // 且休眠恢复会指向错误的系统。删除失败是良性的（本来就没有）。
        let _ = crate::run_logged(
            "bcdedit.exe",
            &[
                "/store",
                &store,
                "/deletevalue",
                "{bootmgr}",
                "resumeobject",
            ],
            log,
        );
    }
    let mut args = vec!["/store", store.as_str(), "/displayorder"];
    args.extend(selected.iter().map(String::as_str));
    crate::run_logged("bcdedit.exe", &args, log)?;
    crate::run_logged(
        "bcdedit.exe",
        &[
            "/store",
            &store,
            "/timeout",
            &plan.timeout_seconds.to_string(),
        ],
        log,
    )?;
    let verify = crate::capture_logged(
        "bcdedit.exe",
        &["/store", &store, "/enum", "all", "/v"],
        log,
    )?;
    let actual = crate::parse_boot_manager_state(&verify)
        .ok_or_else(|| crate::err("无法回读引导修复结果"))?;
    let timeout = verify.lines().any(|line| {
        line.split_whitespace().collect::<Vec<_>>()
            == ["timeout", &plan.timeout_seconds.to_string()]
    });
    if actual.display_order != selected
        || actual.default != default
        || actual.resume_object != previous.resume_object
        || !timeout
    {
        return Err(crate::err(
            "引导修复顺序、默认项、休眠引用或显示时间回读不一致",
        ));
    }
    for entry in plan.entries.iter().filter(|e| e.selected) {
        let id = loader_identifier_for_partition(&verify, entry.drive_letter)?
            .ok_or_else(|| crate::err("回读缺少所选系统"))?;
        let object =
            crate::capture_logged("bcdedit.exe", &["/store", &store, "/enum", &id, "/v"], log)?;
        // 只校验我们写过名字的条目；故意保留原名的条目回读必然是旧名字，
        // 用 display_name 去比会永远不相等（那不是缺陷，是设计）。
        let renamed = entry.existing_name.as_deref() != Some(entry.display_name.as_str());
        if entry.existing_name.is_none() || renamed {
            // 中文 bcdedit 用「说明」而不是 description，两种都要认。
            let shown = object
                .lines()
                .filter_map(|line| line.trim().split_once(char::is_whitespace))
                .find(|(key, _)| matches!(*key, "description" | "说明"))
                .map(|(_, value)| value.trim());
            if shown != Some(entry.display_name.as_str()) {
                return Err(crate::err("引导菜单名称回读不一致"));
            }
        }
    }
    crate::append_log(
        log,
        &format!(
            "引导修复完成：菜单 {} 项，显示 {} 秒；现有 PE 入口保留",
            selected.len(),
            plan.timeout_seconds
        ),
    )
}

#[cfg(windows)]
#[path = "boot_menu_window.rs"]
mod window;
#[cfg(windows)]
pub(crate) use window::{handle_key, open_editor};

#[cfg(test)]
mod tests {
    use super::*;
    use backuprestore_core::VolumeIdentity;

    /// 构造完整卷身份（`is_complete` 要求全部稳定字段都在）。
    fn identity(disk: u8, partition: u8, letter: char) -> VolumeIdentity {
        VolumeIdentity {
            disk_guid: format!("{{dg-{disk}}}"),
            partition_guid: format!("{{pg-{partition}}}"),
            volume_guid: format!("{{vg-{disk}-{partition}}}"),
            partition_type_guid: "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}".into(),
            disk_number: Some(u32::from(disk)),
            partition_number: Some(u32::from(partition)),
            partition_offset: u64::from(partition) * 1_000_000,
            partition_size: 500_000_000_000,
            filesystem: "NTFS".into(),
            volume_serial: format!("{disk:08x}{partition:08x}"),
            drive_letter: Some(letter),
        }
    }

    /// 默认顺序与 `discover()` 一致：原有系统在前，本次还原的目标在最后。
    /// 首项模拟"BCD 里已有条目且用户起过名"（existing_name = Some），
    /// 末项模拟"还没有条目、该由我们起名"（existing_name = None）。
    /// 首项刻意**不是**当前默认项，以便测试勾选的正常路径；
    /// 需要测"默认项被钉住"的用例自行把 is_current_default 置 true。
    fn plan() -> BootMenuPlan {
        BootMenuPlan {
            entries: vec![
                BootMenuEntry {
                    drive_letter: 'C',
                    display_name: "我的工作系统".into(),
                    selected: true,
                    is_restore_target: false,
                    volume: identity(0, 1, 'C'),
                    existing_name: Some("我的工作系统".into()),
                    is_current_default: false,
                },
                BootMenuEntry {
                    drive_letter: 'E',
                    display_name: "本次还原的 Windows".into(),
                    selected: true,
                    is_restore_target: true,
                    volume: identity(2, 5, 'E'),
                    existing_name: None,
                    is_current_default: false,
                },
            ],
            timeout_seconds: TIMEOUT_DEFAULT,
            default_policy: DefaultPolicy::PointAtRestoreTarget,
        }
    }

    #[test]
    fn restore_target_index_finds_the_mandatory_entry() {
        // 默认顺序里目标在最后；用户上移后仍必须定位正确，故禁止硬编码下标 0。
        let p = plan();
        assert_eq!(p.restore_target_index(), Some(1));
        let mut moved = plan();
        assert!(moved.move_up(1), "目标可被上移");
        assert_eq!(moved.restore_target_index(), Some(0), "上移后下标要跟着变");
        let mut empty = plan();
        empty.entries.clear();
        assert_eq!(empty.restore_target_index(), None);
    }

    #[test]
    fn restore_target_cannot_be_unselected() {
        let mut p = plan();
        assert!(!p.toggle(1), "还原目标是必选项，勾选必须被拒绝");
        assert!(p.entries[1].selected, "必选项状态不能被改变");
        assert!(p.toggle(0), "其它系统可以取消");
        assert!(!p.entries[0].selected);
        assert!(p.toggle(0));
        assert!(p.entries[0].selected, "再次勾选应恢复选中");
        assert!(!p.toggle(9), "越界下标不得 panic");
    }

    #[test]
    fn ordering_moves_entries_within_bounds() {
        let mut p = plan();
        assert!(!p.move_up(0), "首项上移应无效");
        assert!(p.move_down(0), "首项下移应生效");
        assert_eq!(p.entries[0].drive_letter, 'E', "下移后首项应变成原第二项");
        assert!(p.move_up(1));
        assert_eq!(p.entries[0].drive_letter, 'C', "上移后应还原");
        assert!(!p.move_down(1), "末项下移应无效");
        assert!(!p.move_down(9), "越界下标不得 panic");
    }

    #[test]
    fn rename_trims_and_rejects_unusable_names() {
        let mut p = plan();
        assert!(p.rename(1, "  第二系统  "));
        assert_eq!(p.entries[1].display_name, "第二系统", "首尾空白应被去掉");
        assert!(!p.rename(1, "   "), "纯空白名称必须拒绝");
        assert!(!p.rename(1, "带\x07控制字符"), "控制字符必须拒绝");
        assert!(!p.rename(9, "越界"), "越界下标不得 panic");
        let long = "长".repeat(NAME_MAX_CHARS + 5);
        assert!(p.rename(1, &long));
        assert_eq!(
            p.entries[1].display_name.chars().count(),
            NAME_MAX_CHARS,
            "超长名称应截断"
        );
    }

    #[test]
    fn validate_requires_exactly_one_selected_target() {
        let mut p = plan();
        p.validate().expect("默认方案应通过");
        p.entries.remove(1);
        p.validate().expect_err("缺少还原目标必须拒绝");
        let mut p = plan();
        p.entries[1].selected = false;
        p.validate().expect_err("还原目标被取消必须拒绝");
        let mut p = plan();
        p.entries[0].is_restore_target = true;
        p.validate().expect_err("两个必选项必须拒绝");
    }

    #[test]
    fn validate_rejects_incomplete_identity_and_duplicates() {
        let mut p = plan();
        p.entries[1].volume.volume_guid = String::new();
        p.validate().expect_err("卷身份不完整必须拒绝");
        let mut p = plan();
        p.entries[1].volume.disk_guid = p.entries[0].volume.disk_guid.clone();
        p.entries[1].volume.partition_guid = p.entries[0].volume.partition_guid.clone();
        p.validate().expect_err("重复系统分区必须拒绝");
        let mut p = plan();
        p.entries[1].display_name = String::new();
        p.validate().expect_err("空名称必须拒绝");
        let mut p = plan();
        p.timeout_seconds = TIMEOUT_MAX + 1;
        p.validate().expect_err("菜单时间超上限必须拒绝");
        let mut p = plan();
        p.timeout_seconds = TIMEOUT_MAX;
        p.validate().expect("恰好上限应通过");
    }

    #[test]
    fn loader_description_reads_the_current_menu_name() {
        let output = "Windows Boot Loader\r\nidentifier {11111111-2222-3333-4444-555555555555}\r\ndescription 我的工作系统\r\npath \\Windows\\system32\\winload.efi\r\ndevice partition=C:\r\nosdevice partition=C:\r\n\r\n";
        assert_eq!(
            loader_description_for_partition(output, 'c').as_deref(),
            Some("我的工作系统"),
            "必须原样读回当前名字，供 apply 判断要不要改写"
        );
        assert_eq!(
            loader_description_for_partition(output, 'e'),
            None,
            "该分区没有 loader，返回 None 表示该由我们起名"
        );
    }

    #[test]
    fn loader_description_accepts_localized_label() {
        // 中文 bcdedit 用「说明」而不是 description。
        let output = "Windows Boot Loader\r\n标识符 {aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}\r\n说明 Windows 11 专业版 26100\r\n路径 \\Windows\\system32\\winload.efi\r\n设备 partition=C:\r\n操作系统设备 partition=C:\r\n\r\n";
        assert_eq!(
            loader_description_for_partition(output, 'C').as_deref(),
            Some("Windows 11 专业版 26100")
        );
    }

    #[test]
    fn loader_description_ignores_ramdisk_and_resume_objects() {
        // 两类必须排除的对象，形态取自仓库里已验证的真实夹具
        // （main.rs::secondary_loader_selection_ignores_resume_object）：
        // - ramdisk 条目：设备字段是 ramdiskdevice，不是分区加载器；
        // - 休眠恢复对象：path 是 winresume.efi 而非 winload.efi，且只带 device。
        //   正因为 path 不同，判据对设备字段用 OR 语义是安全的——不必强求
        //   device 与 osdevice 都在（那样会在本地化标签猜错时漏掉全部 loader）。
        let output = "Windows Boot Loader\r\nidentifier {11111111-1111-1111-1111-111111111111}\r\ndescription 恢复环境\r\npath \\Windows\\system32\\winload.efi\r\nramdiskdevice partition=E:\r\n\r\nWindows Resume Application\r\nidentifier {22222222-2222-2222-2222-222222222222}\r\ndescription 休眠恢复\r\npath \\Windows\\system32\\winresume.efi\r\ndevice partition=C:\r\n\r\n";
        assert_eq!(
            loader_description_for_partition(output, 'e'),
            None,
            "ramdisk 条目不是分区加载器"
        );
        assert_eq!(
            loader_description_for_partition(output, 'c'),
            None,
            "winresume.efi 的休眠恢复对象不是系统加载器"
        );
    }

    #[test]
    fn loader_lookup_survives_a_missing_osdevice_label() {
        // 只有 device、没有 osdevice（本地化标签未被识别时就是这种观感）：
        // 必须仍然能认出这是 C: 的加载器，否则该语言下引导修复全线失效。
        let output = "Windows Boot Loader\r\nidentifier {33333333-3333-4333-8333-333333333333}\r\ndescription 我的系统\r\npath \\Windows\\system32\\winload.efi\r\ndevice partition=C:\r\n\r\n";
        assert_eq!(
            loader_description_for_partition(output, 'c').as_deref(),
            Some("我的系统")
        );
        assert_eq!(
            loader_identifier_for_partition(output, 'c')
                .unwrap()
                .as_deref(),
            Some("{33333333-3333-4333-8333-333333333333}")
        );
    }

    #[test]
    fn loader_lookup_accepts_chinese_os_device_with_space() {
        // 中文 bcdedit 的 osdevice 实测写法是「os 设备」（带空格），
        // 用 split_once(whitespace) 取键名会把它截成 "os"，必须用 starts_with 兼容。
        let output = "Windows Boot Loader\r\n标识符 {44444444-4444-4444-8444-444444444444}\r\n说明 中文系统\r\n路径 \\Windows\\system32\\winload.efi\r\n设备 partition=C:\r\nos 设备 partition=C:\r\n\r\n";
        assert_eq!(
            loader_description_for_partition(output, 'c').as_deref(),
            Some("中文系统")
        );
    }

    /// 手工搭一个最小的 PE 头 + 资源目录 + `VS_VERSIONINFO`，
    /// 用来验证版本号解析不依赖真实 ntoskrnl.exe（那个 10MB，不能进仓库）。
    fn minimal_pe_with_version(build: u16, ubr: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x400];
        // DOS 头：MZ + e_lfanew 指向 0x80。
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        // PE 签名 + COFF 头（section 数 1，optional header 大小 0xE0）。
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        bytes[0x94..0x96].copy_from_slice(&0xE0u16.to_le_bytes());
        // Optional header：PE32+ magic，数据目录第 3 项（资源）RVA=0x200。
        let optional = 0x80 + 24;
        bytes[optional..optional + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        let directory = optional + 112 + 16;
        bytes[directory..directory + 4].copy_from_slice(&0x200u32.to_le_bytes());
        bytes[directory + 4..directory + 8].copy_from_slice(&0x100u32.to_le_bytes());
        // 节表：一个节，VirtualAddress 0x200 / RawPointer 0x200。
        let table = optional + 0xE0;
        bytes[table + 8..table + 12].copy_from_slice(&0x100u32.to_le_bytes());
        bytes[table + 12..table + 16].copy_from_slice(&0x200u32.to_le_bytes());
        bytes[table + 16..table + 20].copy_from_slice(&0x100u32.to_le_bytes());
        bytes[table + 20..table + 24].copy_from_slice(&0x200u32.to_le_bytes());
        // 资源目录（位于 0x200）：类型层，一个条目 id=16 指向名字层（偏移 0x30）。
        let resource = 0x200usize;
        bytes[resource + 14..resource + 16].copy_from_slice(&1u16.to_le_bytes());
        bytes[resource + 16..resource + 20].copy_from_slice(&16u32.to_le_bytes());
        bytes[resource + 20..resource + 24].copy_from_slice(&0x8000_0030u32.to_le_bytes());
        // 名字层（0x230 = resource + 0x30）：一个条目指向叶子数据（偏移 0x60）。
        let name_layer = resource + 0x30;
        bytes[name_layer + 14..name_layer + 16].copy_from_slice(&1u16.to_le_bytes());
        bytes[name_layer + 16..name_layer + 20].copy_from_slice(&1u32.to_le_bytes());
        bytes[name_layer + 20..name_layer + 24].copy_from_slice(&0x60u32.to_le_bytes());
        // 叶子数据入口（resource + 0x60）：OffsetToData=0x300，Size=0x60。
        let leaf = resource + 0x60;
        bytes[leaf..leaf + 4].copy_from_slice(&0x300u32.to_le_bytes());
        bytes[leaf + 4..leaf + 8].copy_from_slice(&0x60u32.to_le_bytes());
        // VS_VERSIONINFO（位于 0x300 = 节内 RVA 0x100 → 同偏移）。
        let version = 0x300usize;
        bytes[version + 2..version + 4].copy_from_slice(&0x34u16.to_le_bytes());
        // "VS_VERSION_INFO" 共 15 个字符；连同结尾空字符占 16 个 u16 = 32 字节。
        for (index, unit) in "VS_VERSION_INFO".encode_utf16().enumerate() {
            let at = version + 6 + index * 2;
            bytes[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        // 字符串后对齐到 4 字节边界，紧接着就是 VS_FIXEDFILEINFO。
        let fixed = (version + 6 + 32 + 3) & !3;
        bytes[fixed..fixed + 4].copy_from_slice(&0xFEEF_04BDu32.to_le_bytes());
        let ms: u32 = (10u32 << 16) | 0;
        let ls: u32 = (u32::from(build) << 16) | u32::from(ubr);
        bytes[fixed + 8..fixed + 12].copy_from_slice(&ms.to_le_bytes());
        bytes[fixed + 12..fixed + 16].copy_from_slice(&ls.to_le_bytes());
        bytes
    }

    #[test]
    fn pe_version_quad_reads_major_build_and_ubr() {
        let data = minimal_pe_with_version(26100, 9549);
        assert_eq!(
            pe_version_quad(&data[0x300..]),
            Some((10, 0, 26100, 9549)),
            "必须读出 build 与 UBR，菜单名才能区分 26100.9549 与 26100.6751"
        );
    }

    #[test]
    fn pe_resource_directory_handles_pe32_plus_header() {
        let data = minimal_pe_with_version(22000, 1);
        assert_eq!(pe_resource_directory(&data), Some((0x200, 0x100)));
        // 不是 PE 文件必须安全返回 None，不能 panic。
        assert_eq!(pe_resource_directory(b"not a pe file at all"), None);
        assert_eq!(pe_resource_directory(&[]), None);
    }

    #[test]
    fn pe_rva_to_offset_walks_the_section_table() {
        let data = minimal_pe_with_version(26100, 1);
        assert_eq!(pe_rva_to_offset(&data, 0x200), Some(0x200));
        assert_eq!(
            pe_rva_to_offset(&data, 0x9000),
            None,
            "不在任何节里的 RVA 必须返回 None"
        );
    }

    #[test]
    fn pe_version_resource_offset_finds_the_type_16_leaf() {
        let data = minimal_pe_with_version(26100, 1);
        assert_eq!(
            pe_version_resource_offset(&data[0x200..], 0, 0),
            Some(0x300),
            "类型 16（RT_VERSION）的叶子必须被找到，否则读不到版本"
        );
    }

    #[test]
    fn current_default_entry_cannot_be_unselected() {
        // 取消当前默认启动项会让默认项漂移到刚还原的系统，而在线路径的验收
        // 又要求默认项不变 —— 结果是 BCD 已改坏却报失败且无回滚。必须在此拦住。
        let mut p = plan();
        p.entries[0].is_current_default = true;
        assert!(!p.toggle(0), "当前默认启动项必须拒绝取消");
        assert!(p.entries[0].selected, "状态不能被改变");
        // 即使绕过 toggle（例如落盘的方案被改过），validate 也要拒绝。
        p.entries[0].selected = false;
        p.validate()
            .expect_err("默认项未选中的方案必须被 validate 拒绝");
    }

    #[test]
    fn duplicate_menu_names_are_rejected() {
        // 两个同名项在启动菜单里无法区分，用户不知道该选哪个。
        let mut p = plan();
        p.entries[1].display_name = p.entries[0].display_name.clone();
        p.validate().expect_err("同名项必须被拒绝");
        // 未勾选的项不参与重名判定（它不会进菜单）。
        let mut q = plan();
        q.entries[0].selected = false;
        q.entries[1].display_name = q.entries[0].display_name.clone();
        q.validate().expect("未勾选项不该触发重名拒绝");
    }

    #[test]
    fn existing_name_keeps_original_and_only_new_entries_get_named() {
        // 这是命名契约的核心：既有条目名字没变时一个字都不写，
        // 用户改过名或条目是新建的才写。
        let p = plan();
        assert_eq!(
            p.entries[0].existing_name.as_deref(),
            Some("我的工作系统"),
            "既有条目的原名必须被记录下来"
        );
        assert_eq!(
            p.entries[0].existing_name.as_deref(),
            Some(p.entries[0].display_name.as_str()),
            "没改过名时两者相等 —— apply 据此跳过写入，保留原名字"
        );
        assert!(
            p.entries[1].existing_name.is_none(),
            "还原目标此刻没有 loader，属于该由我们起名的那一类"
        );
    }

    #[test]
    fn renamed_entry_is_detected_even_after_reorder() {
        // 用户改名后 existing_name 与 display_name 不再相等，与排序无关。
        let mut p = plan();
        p.rename(0, "主力机");
        assert_ne!(
            p.entries[0].existing_name.as_deref(),
            Some(p.entries[0].display_name.as_str()),
            "改名后必须能被识别为「需要写回」"
        );
        assert!(p.move_down(0), "调整顺序不应影响改名判定");
        assert_ne!(
            p.entries[1].existing_name.as_deref(),
            Some(p.entries[1].display_name.as_str()),
            "上移/下移后改名判定仍然成立"
        );
    }

    #[test]
    fn versioned_names_satisfy_the_menu_constraints() {
        // 不自己编造名字再断言它们不同（那样的断言永不失败）。
        // 这里验证的是真实契约：版本化名字必须能通过 validate 的全部约束，
        // 而撞名必须被拒绝——discover 返回前的去重逻辑就是据此设计的。
        let mut p = plan();
        p.entries[0].display_name = "Windows 11 26100.9549".into();
        p.entries[0].existing_name = Some("Windows 11 26100.9549".into());
        p.entries[1].display_name = "Windows 11 Pro 26100.9549".into();
        p.validate().expect("版本化名字应通过全部校验");
        for entry in &p.entries {
            assert!(!entry.display_name.trim().is_empty());
            assert!(entry.display_name.chars().count() <= NAME_MAX_CHARS);
            assert!(!entry.display_name.chars().any(char::is_control));
        }
        // 同补丁级别的两个未登记系统会生成同一个版本串，必须被拒绝，
        // 从而迫使 discover 加盘符区分。
        let mut collided = p.clone();
        collided.entries[1].display_name = collided.entries[0].display_name.clone();
        collided
            .validate()
            .expect_err("相同版本串必须被当作重名拒绝");
        // 加盘符区分后应恢复合法。
        let letter = collided.entries[1].drive_letter;
        collided.entries[1].display_name =
            format!("{} ({letter}:)", collided.entries[0].display_name);
        collided.validate().expect("加盘符区分后应通过");
    }

    #[test]
    fn loader_identifier_finds_the_unique_loader() {
        // 两个对象块：只有 C 分区的那个同时满足 winload.efi + 设备字段。
        let output = "\
Windows Boot Manager\r\nidentifier {bootmgr}\r\n\r\nWindows Boot Loader\r\nidentifier {11111111-2222-3333-4444-555555555555}\r\npath \\Windows\\system32\\winload.efi\r\ndevice partition=C:\r\nosdevice partition=C:\r\n\r\n";
        assert_eq!(
            loader_identifier_for_partition(output, 'c')
                .unwrap()
                .as_deref(),
            Some("{11111111-2222-3333-4444-555555555555}")
        );
        assert_eq!(loader_identifier_for_partition(output, 'd').unwrap(), None);
    }

    #[test]
    fn loader_identifier_accepts_localized_device_labels() {
        // 中文 Windows 的 bcdedit 用「设备」「操作系统设备」标签，字段名解析不能因此失效。
        let output = "Windows Boot Loader\r\n标识符 {aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}\r\n路径 \\Windows\\system32\\winload.efi\r\n设备 partition=C:\r\n操作系统设备 partition=C:\r\n\r\n";
        assert_eq!(
            loader_identifier_for_partition(output, 'C')
                .unwrap()
                .as_deref(),
            Some("{aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}")
        );
    }

    #[test]
    fn loader_identifier_refuses_to_guess_between_two_loaders() {
        // 同卷两个加载器：按枚举顺序猜会把用户引导到错误系统，必须报错。
        let output = "Windows Boot Loader\r\nidentifier {11111111-1111-1111-1111-111111111111}\r\npath \\Windows\\system32\\winload.efi\r\ndevice partition=C:\r\nosdevice partition=C:\r\n\r\nWindows Boot Loader\r\nidentifier {22222222-2222-2222-2222-222222222222}\r\npath \\Windows\\system32\\winload.efi\r\ndevice partition=C:\r\nosdevice partition=C:\r\n\r\n";
        assert!(loader_identifier_for_partition(output, 'c').is_err());
    }

    #[test]
    fn loader_identifier_ignores_ramdisk_and_incomplete_objects() {
        // ramdisk 选项与 resume 对象也带 winload 路径特征，但设备字段不是分区加载器。
        let output = "Windows Boot Loader\r\nidentifier {ramdisk}\r\npath \\Windows\\system32\\winload.efi\r\nramdiskdevice partition=E:\r\n\r\nWindows Boot Loader\r\nidentifier {no-osdevice}\r\npath \\Windows\\system32\\winload.efi\r\ndevice partition=C:\r\n\r\n";
        assert_eq!(loader_identifier_for_partition(output, 'e').unwrap(), None);
        assert_eq!(loader_identifier_for_partition(output, 'c').unwrap(), None);
    }

    #[test]
    fn render_entries_marks_cursor_target_and_selection() {
        // 光标落在下标 1，即默认顺序里的还原目标（原有系统在前）。
        let rendered = render_entries(&plan(), 1);
        let lines: Vec<&str> = rendered.split("\r\n").collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with(' '), "非焦点行用空格占位保持排版稳定");
        assert!(lines[0].contains("[√]"), "选中项显示勾选");
        assert!(lines[0].contains("(C:)"), "首行是原有系统");
        assert!(!lines[0].contains("必选"), "非目标行不得标注必选");
        assert!(lines[1].starts_with('>'), "光标行以 > 指示");
        assert!(
            lines[1].contains("[当前正在还原中的系统·Step 4生效]"),
            "还原目标必须标注"
        );
        assert!(
            lines[1].ends_with("[当前正在还原中的系统·Step 4生效]"),
            "必选标注在行尾"
        );
        assert!(lines[1].contains("(E:)"), "目标行带盘符");
    }

    #[test]
    fn all_loader_descriptions_lists_every_menu_entry() {
        // 混排四种对象：两个分区 loader（英文/中文标签）、一个 ramdisk 选项、
        // 一个休眠恢复对象。只有前两个算「启动菜单里的系统项」。
        let output = "Windows Boot Loader\r\nidentifier {11111111-2222-3333-4444-555555555555}\r\ndescription 本次还原的 Windows\r\npath \\Windows\\system32\\winload.efi\r\ndevice partition=C:\r\n\r\nWindows Boot Loader\r\n标识符 {aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}\r\n说明 Windows 11 专业版 26100\r\n路径 \\Windows\\system32\\winload.efi\r\n设备 partition=E:\r\n操作系统设备 partition=E:\r\n\r\nWindows Boot Loader\r\nidentifier {ramdisk}\r\ndescription 恢复环境\r\npath \\Windows\\system32\\winload.efi\r\nramdiskdevice partition=F:\r\n\r\nWindows Resume Application\r\nidentifier {22222222-2222-2222-2222-222222222222}\r\ndescription 休眠恢复\r\npath \\Windows\\system32\\winresume.efi\r\ndevice partition=C:\r\n\r\n";
        assert_eq!(
            all_loader_descriptions(output),
            vec!["本次还原的 Windows".to_string(), "Windows 11 专业版 26100".to_string()],
            "列全量菜单名供撞名避让；ramdisk 与 resume 对象不算菜单项"
        );
    }

    #[test]
    fn resolve_menu_name_keeps_unique_names_untouched() {
        let now = chrono::Local::now();
        let existing = vec!["本次还原的 Windows".to_string()];
        assert_eq!(
            resolve_menu_name_collision("2026-10-14 21:40", &existing, now),
            "2026-10-14 21:40",
            "不撞名时原样返回（trim 后）"
        );
        assert_eq!(
            resolve_menu_name_collision("  2026-10-14 21:40  ", &[], now),
            "2026-10-14 21:40",
            "无现有条目时同样不动，顺便 trim"
        );
    }

    #[test]
    fn resolve_menu_name_appends_local_timestamp_on_collision() {
        let now = chrono::Local::now();
        let expected_suffix = format!("-{}", now.format("%Y%m%d%H%M"));
        let existing = vec!["2026-10-14 21:40".to_string()];
        let resolved = resolve_menu_name_collision("2026-10-14 21:40", &existing, now);
        assert_eq!(resolved, format!("2026-10-14 21:40{expected_suffix}"));
        assert!(!existing.contains(&resolved), "消解结果不得仍与现有项重名");
    }

    #[test]
    fn resolve_menu_name_escalates_when_suffixed_name_also_collides() {
        let now = chrono::Local::now();
        let minute = now.format("%Y%m%d%H%M").to_string();
        let second = now.format("%Y%m%d%H%M%S").to_string();
        // 同一分钟内已存在带分钟后缀和秒后缀的两项，只能退到秒+序号。
        let existing = vec![
            "备份".to_string(),
            format!("备份-{minute}"),
            format!("备份-{second}"),
            format!("备份-{second}-2"),
        ];
        let resolved = resolve_menu_name_collision("备份", &existing, now);
        assert_eq!(resolved, format!("备份-{second}-3"));
        assert!(resolved.chars().count() <= NAME_MAX_CHARS);
    }

    #[test]
    fn resolve_menu_name_truncates_base_to_fit_limit() {
        let now = chrono::Local::now();
        let long: String = "甲".repeat(NAME_MAX_CHARS + 10);
        let existing = vec![long.clone()];
        let resolved = resolve_menu_name_collision(&long, &existing, now);
        assert_eq!(resolved.chars().count(), NAME_MAX_CHARS, "必须截到上限");
        assert!(
            resolved.ends_with(&format!("-{}", now.format("%Y%m%d%H%M"))),
            "后缀完整保留，截断发生在基名段"
        );
    }
}
