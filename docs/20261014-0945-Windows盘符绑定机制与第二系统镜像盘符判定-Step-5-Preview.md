# Windows 盘符绑定机制，与第二系统「镜像自认盘符」的判定（2026-10-14 09:45，v2.4.8）

> 模型：Step-5-Preview｜触发：用户“搞迷糊了”——问新增第二系统时如何判断镜像是从 C 盘
> 还是别的分区备份出来的、有没有必要区分、Windows 底层怎么实现、以及两个具体场景能不能做。
>
> 本文是**机制说明**，不改动任何代码。

## 0. 结论先行

1. **盘符不是分区的物理属性**，它存在每套 Windows 自己的注册表里
   （`HKLM\SYSTEM\MountedDevices`）。同一个物理分区，在两个系统的注册表里可以叫不同字母，
   同一时刻只有一个系统在运行，所以「两个都叫 C:」不冲突。
2. **判定镜像“自认哪个盘符”的权威依据是离线 SOFTWARE hive 里的 `SystemRoot`**，
   不是“它物理上在哪个分区”。物理分区信息（副档里的 partition_size / 卷身份）只用于
   容量预检和身份校验。
3. **必须区分，但区分的是“镜像自认什么字母”而不是“来自哪个分区”**。不修 → 新系统的
   `C:` 仍指向原物理分区（往往是主系统）→ 启动后加载错误的 Windows 目录 → 崩溃循环
   （2026-10-12 实测：LogonUI / dwm 崩溃循环，见 `docs/20261012-1435`）。
4. **本项目的策略是“迁就镜像”**：不去改写镜像里的 `SystemRoot`（代价极高），而是把物理
   分区绑成镜像期望的那个字母。实现是 `secondary_registry::repair()`。
5. **程序全程不用盘符锁定目标**，用 `disk_number/partition_number + partition_guid +
   volume_serial` 身份——因为 WinRE 里盘符会重排（02:12 实测 T:→D:、H:→E:）。

## 1. Windows 底层：盘符是怎么来的

启动时序决定了「盘符什么时候才存在」：

| 阶段 | 用什么定位系统分区 | 盘符是否已存在 |
| --- | --- | --- |
| bootmgr 读 BCD | 设备路径 / 分区 GUID（`\Device\HarddiskVolumeN`、ramdisk 设备路径） | 不存在 |
| winload 加载内核 | 同上 | 不存在 |
| 内核读 SYSTEM hive | — | **建立盘符映射** |
| 服务 / 程序访问 `C:\...` | 走盘符 | 此时才有效 |

也就是说：**BCD 那一层根本不依赖盘符**（本项目 `boot_entry.rs` 写的 ramdisk 规格是设备路径
形式，正是这个道理）；盘符只在系统起来之后才起作用。所以「改盘符」这件事，改的是
系统内部的映射表，与能不能启动无关，只与启动后能不能正确访问自己的文件有关。

两个关键注册表位置：

```
HKLM\SYSTEM\MountedDevices
    \DosDevices\C:   REG_BINARY   DMIO:ID:<GPT 分区 GUID 的小端字节>
    \DosDevices\E:   REG_BINARY   DMIO:ID:<另一个分区的 GUID>
HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion
    SystemRoot       REG_SZ       C:\Windows        ← 这个系统"自认"装在哪个盘符
```

- GPT 盘：`MountedDevices` 的值是字符串 `DMIO:ID:` 后接**分区 GUID 的 little-endian 字节**
  （见 `secondary_registry::gpt_mapping`，`uuid::Uuid::to_bytes_le()`）。MBR 盘则是磁盘签名
  + 分区偏移，格式不同。
- `SystemRoot` 是**只读的事实**：成千上万条服务路径、程序安装记录、用户配置都写死了
  `C:\...`。改它会牵动全注册表，Windows 官方也不支持（sysprep 之后仍可能残留）。

## 2. 本项目怎么做：`secondary_registry::repair()`

源码 `crates/backuprestore-cli/src/secondary_registry.rs`，只在「新增第二系统」路径调用
（离线 `main.rs:2685`、在线 `online_operation.rs:486`）：

| 步骤 | 实现 | 源码 |
| --- | --- | --- |
| ① 读期望字母 | `reg load` 离线 SOFTWARE hive，读 `SystemRoot`（如 `C:\Windows`）→ `source_letter = 'C'` | `system_letter()` 4-27 |
| ② 算分区标识 | `DMIO:ID:` + 目标分区 GUID 的小端字节 | `gpt_mapping()` 29-37 |
| ③ 改写映射 | `reg load` 离线 SYSTEM hive → `MountedDevices` 里**删掉指向该分区的其他字母**，再写 `\DosDevices\C: = DMIO:ID:<GUID>` | 153-178 |
| ④ 回读校验 | 该分区在映射表里必须**恰好出现一次**且名字正确；其他卷的映射不得丢失 | 179-191 |

配套的安全细节：

- **备份原 hive**：改之前把 `SYSTEM`/`SOFTWARE` 及事务日志族复制到
  `Windows\System32\config\BackupRestore-secondary-original\`，并校验 sha256（118-138）。
- **拒绝动当前系统**：`if running.eq_ignore_ascii_case(&format!("{letter}:"))` →
  报「拒绝修改当前系统的离线配置」（97-99）。所以**不能拿它改正在运行的系统**。
- **保留其他卷映射**：只删“指向目标分区且名字不对”的旧映射，别的卷映射原样保留
  （因此新系统里可能残留原环境其他卷的字母，无害）。

## 3. 四个问题的逐条回答

### Q1：新增第二系统时，怎么判断镜像是从 C 盘还是别的分区备份出来的？

**不看“从哪个物理分区来”，只看镜像里那个系统自认什么字母**——读离线 SOFTWARE hive 的
`SystemRoot`。物理分区信息在还原时已经变了（目标被格式化成新卷），拿它当依据没有意义。

### Q2：有必要区分吗？

**有**。不区分（即不调 `repair`）的后果：镜像里的 `MountedDevices` 把 `C:` 绑在原物理分区
上，应用到新分区后，新系统的 `SystemRoot=C:\Windows` 会指向**主系统的** Windows 目录 →
加载错误的驱动与配置 → 登录界面崩溃循环或 `0x0000007B INACCESSIBLE_BOOT_DEVICE`。
这正是 2026-10-12 修的那个缺陷（`docs/20261012-1435`）：在线路径漏调 `repair()` 导致
第二系统桌面崩溃循环。

### Q3：当前系统 C 盘 → 备份出来就是 C 盘？当前系统在 E 盘 → 还原出来也是 E 盘？能改吗？

- **备份**：DISM 是卷级捕获，把分区内容（含两套 hive）原样带进镜像，所以「自认字母」
  被原样保留。
- **还原（第二系统）**：本项目让它**沿用原字母**——镜像自认 `C:` → 目标分区在新系统里
  也叫 `C:`；镜像自认 `E:` → 也叫 `E:`。
  ⚠️ 所以“还原出来是 E 盘”要分两层看：**物理上**它落在你选的目标分区；**字母上**它沿用
  镜像自认的字母。这两件事互不相干。
- **能改吗**：技术上能（改 `SystemRoot` + 全注册表路径），代价极高且 Windows 官方不支持。
  Ghost / Clonezilla / 本项目统一选择“迁就镜像”。本项目**没有**提供「改成别的字母」的选项。

### Q4a：现在系统在 C，想新增第二系统到 E——能吗？

**能，且这是标准用法**。走「新增第二系统」：源卷 = 当前系统（C:），目标 = E:。

```
格式化 E: → DISM Apply → repair（读 SystemRoot=C:\Windows）
  → 把 E: 分区在新系统的 hive 里绑成 C:
→ bcdboot /d /addlast（保留主系统默认启动项）+ 菜单命名 + WinRE repair_secondary
```

结果：第二系统启动后**自认 C:**；而你在主系统里看那个分区是 E:。两者不冲突（见 §0.1）。

### Q4b：现在系统物理上在 E 分区（但它自认 C:），能备份出来还原到“我当前的 C 盘”吗？

先澄清“当前的 C 盘”指谁：

- **如果指你现在正在运行的这个系统自己** → 那就是「还原它自己」= `restore-existing`
  （`source == target`），可以，走在线覆盖或离线覆盖都行。
- **如果指主系统视角下的 C:（另一个 Windows 所在的分区）** → 那是把镜像覆盖到另一个
  Windows 分区上：目标会被格式化并写入镜像，`repair` 把它绑成镜像自认的字母（C:）。
  也可以，但那个分区上原有的 Windows 就没了。

关键点：程序**不在乎“物理分区叫 E 还是 C”**——它用 `disk/partition + GUID + 序列号`
锁定目标（不用盘符，因为 WinRE 里盘符重排）。唯一相关的是镜像自认的字母，由 `repair`
负责对齐。

约束回顾：`restore-existing` 要求 `source == target`；`create-secondary` 要求
`source != target`；`repair` 拒绝修改当前运行系统的 hive。

## 4. 一句话记忆

> **物理分区是“房子”，盘符是“门牌号”，门牌号写在这家系统自己的户口本（MountedDevices）
> 里。搬家（克隆/还原）时，正确的做法是把门牌号重新钉到新房子上，而不是逼这家人改姓。**

## 5. 相关文档

- `docs/20261012-1435`（在线路径漏调 repair 导致第二系统崩溃循环，现象侧）
- `docs/20261014-0146`（在线还原端到端，数据卷分支不格式化不修引导）
- `docs/20261014-0212`（RE 还原 round-trip + RE 启动模式说明）
