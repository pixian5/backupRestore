先做 V1，不要擅自扩展成完整 Ghost/PE 工具。

# Windows 一键系统备份还原 V1——完整开发需求

## 1. 项目目标

开发一个 Windows 桌面程序，实现：

> **在正常 Windows 中点击“备份”或“还原”，程序自动准备恢复任务，重启进入 Windows 自带 WinRE（Windows Recovery Environment），WinRE 自动启动本程序的恢复组件，在离线环境下执行系统备份/还原，完成后自动修复 Windows 启动项并重新启动进入正常 Windows。**

最终用户体验：

```text
正常 Windows
    ↓
打开 BackupRestore.exe
    ↓
点击「系统备份」或「系统还原」
    ↓
程序自动准备
    ↓
自动重启
    ↓
自动进入 WinRE
    ↓
自动启动 Recovery.exe
    ↓
自动执行备份/还原
    ↓
完成
    ↓
自动修复 BCD
    ↓
自动重启
    ↓
正常 Windows
```

用户**不需要制作 U 盘、不需要手动进入 BIOS、不需要手动选择 WinRE、不需要手动运行命令。**

------

# 2. V1 的明确范围

V1 只支持：

- Windows 10/11 x64
- UEFI
- GPT
- Windows 系统分区备份与还原（默认单系统流程）
- 可选：在已有空闲 NTFS 分区上建立第二个 Windows，形成双系统
- 普通 NTFS Windows 系统分区
- WIM 镜像
- Windows 自带 WinRE
- DISM 作为镜像备份/还原引擎
- BCDBoot/BCD 作为启动管理工具

双系统是可选模式，不是 V1 的前置条件。用户可以只使用单系统备份/还原，
也可以主动选择“建立第二个 Windows”。选择双系统模式后，程序可以把经
过验证的 WIM 应用到用户明确选择的已有空闲/可覆盖 NTFS 分区，并在不
删除现有启动项的前提下创建第二个 Windows 启动项。V1 不自动压缩现有
分区、不自动重新分区，也不覆盖已有 Windows 分区来“腾出”空间。目标
分区必须经过最终身份复核，且用户必须确认显示的磁盘 GUID、分区 GUID、
大小和启动菜单名称。

V1 暂时不支持：

- Legacy BIOS / MBR
- 已有多系统环境的自动迁移、批量管理和分区重构
- 动态磁盘
- Storage Spaces
- RAID 特殊场景
- 网络启动
- 网络备份
- 增量备份
- 差异备份
- 自研镜像格式
- 自研文件系统驱动
- 自研 UEFI Bootloader
- 自带完整 WinPE
- 自研磁盘分区引擎

V1 的双系统模式只负责“在现有环境中可选地增加一个 Windows”。它不是完整
多系统管理器，不负责迁移、重排或批量管理用户已有的多个系统。建立完成
后必须保留已有 Windows 的 EFI/BCD 启动项，新系统使用独立的分区 GUID
和独立的启动项。

**不要为了“以后可能需要”而扩大 V1 范围。**

------

# 3. 程序结构

项目分成两个主要程序。

```text
BackupRestore.exe
```

运行在正常 Windows 中。

负责：

- GUI
- 检查系统环境
- 创建备份任务
- 创建还原任务
- 管理备份文件
- 检查 WinRE
- 准备 Recovery 环境
- 设置一次性启动
- 重启 Windows

`BackupRestore.exe` 还负责枚举所有 Windows 安装、展示磁盘/分区身份，
以及为第二个 Windows 生成启动项计划。它不能把当前盘符当作分区身份。

以及：

```text
Recovery.exe
```

运行在 WinRE 中。

负责：

- 读取任务
- 检测磁盘
- 找到 Windows 分区
- 找到备份 WIM
- 执行 DISM
- 处理 EFI 分区
- 执行 BCDBoot
- 写日志
- 清理任务
- 重启 Windows

Recovery 环境中的启动器负责启动 `Recovery.exe`。启动器、Recovery 程序、
任务文件和原始 WinRE 的哈希必须作为同一个恢复载荷进行版本校验。

------

# 4. 推荐技术栈

Windows 主程序、Recovery.exe： 使用Rust；

------

# 5. 目录设计

程序目录就是工作目录，允许放在任意普通卷的普通目录中，例如：

```text
D:\Tools\BackupRestore\
├── BackupRestore.exe
├── Recovery.exe
├── tasks\<taskId>\
└── logs\
```

不再使用 `C:\ProgramData`、注册表或用户可见的工作目录卷选择。任务、日志、
WinRE 载荷、状态和 BCD 快照都写入该程序目录。创建任务时记录程序目录所在
卷的 GUID、磁盘/分区身份以及相对卷根路径；WinRE 根据这些身份重新挂载该
卷，再从相对路径读取 `tasks\<taskId>`。每次任务使用独立目录，任务提交采用
“临时文件写入、刷新、原子改名”的方式。

单系统还原或新增第二系统如果程序目录所在卷等于目标卷，只显示阻止提示并
停止，不创建任务、不修改 WinRE/BCD、不请求重启。用户必须手动移动整个
程序目录后重新运行；程序不会自动复制或迁移文件。备份不覆盖源卷，因此
程序目录与备份源位于同一卷仍然允许。

------

# 6. 备份文件设计

备份不能放在 Windows 系统分区。

例如用户选择：

```text
D:\MyBackup\
```

目录：

```text
D:\MyBackup\
│
├── Windows.wim
├── metadata.json
└── backup.log
```

metadata 示例：

```json
{
  "version": 1,
  "type": "wim",
  "created": "2026-08-13T00:00:00",
  "computer": "DESKTOP-XXXX",
  "windowsEdition": "Windows 11 Pro",
  "architecture": "amd64",
  "windowsBuild": "22631",
  "wimIndex": 1,
  "imageSha256": "...",
  "imageSize": 0,
  "source": {
    "diskGuid": "...",
    "partitionGuid": "...",
    "partitionOffset": 0,
    "partitionSize": 0
  }
}
```

路径只用于显示和最后一次访问，不能用于身份判断。备份完成后先写入
`.partial` 文件，执行 WIM 完整性检查并计算 SHA-256，最后才改名为正式
镜像并生成元数据。元数据还必须记录捕获时的 Windows 版本、架构、分区
大小、已用空间、卷序列号和创建程序版本。

以后可以支持多个备份：

```text
D:\MyBackup\
│
├── 2026-08-13\
│   ├── Windows.wim
│   └── metadata.json
│
├── 2026-08-20\
│   ├── Windows.wim
│   └── metadata.json
```

------

# 7. 系统检测

BackupRestore.exe 启动时检测：

### Windows

确认：

```text
Windows 10/11
x64
```

### 启动方式

确认：

```text
UEFI
```

### 分区方式

确认：

```text
GPT
```

### Windows 分区

找到当前 Windows 所在分区。

不要简单假设：

```text
C:\
```

虽然正常 Windows 中通常是 C:，但程序内部应该通过可靠方式确认。

### Windows 阶段先自行确定当前系统所在盘符作为默认，用户可以选择。“卷 GUID”作为内部标识，**不要把 `C:` / `D:` 当成真正的分区身份。PE 阶段通过卷 GUID 优先定位**

Windows 阶段必须以管理员权限运行，并建立完整磁盘清单：磁盘 GPT
UniqueId、分区 GPT UniqueId、分区类型 GUID、起始偏移、长度、文件系统、
卷序列号和当前盘符。Recovery 阶段重新扫描并逐项比对这些字段；盘符仅
作为 DISM/BCDBoot 的临时参数。

程序必须枚举当前系统和其他可识别的 Windows 安装。普通还原默认目标是
当前系统分区；只有用户主动选择建立双系统时，目标才可以是明确选定的
已有空闲/可覆盖分区。程序不得默认把其他 Windows 分区当成目标。

------

# 8. WinRE 检测

执行：

```cmd
reagentc /info
```

或者使用 Windows API/系统配置读取 WinRE 状态。

必须确认：

```text
WinRE enabled
WinRE image exists
WinRE 可以正常启动
```

如果 WinRE 不存在或者 Disabled：

显示：

```text
无法使用 Windows 恢复环境。

请先启用 WinRE。
```

V1 可以不负责自动修复 WinRE。

V1 采用“暂存 WinRE 副本”的路线：保留已注册的原始 `winre.wim` 及其
SHA-256，不直接永久修改它；为每个任务从当前 WinRE 生成副本，在副本中
放入启动器、`Recovery.exe`、任务载荷和版本清单，再通过临时启动配置
进入该副本。启动器必须自动运行 `Recovery.exe`，任务完成或失败后删除
临时启动配置并回到原有 Windows 启动项。

如果 Phase 0 无法在 Win11 ARM64 和 Win11 x64 测试机上证明“自动进入
暂存 WinRE 并启动 Recovery.exe”，不得以手动命令行作为 V1 的隐式替代，
而应将自动恢复链标记为阻塞项。Windows 更新后必须重新检查原始 WinRE
哈希并重新生成暂存副本，不能继续使用旧载荷。

------

# 9. 核心设计：任务系统

这是整个项目非常重要的一部分。

不要让 BackupRestore.exe 和 Recovery.exe 依靠命令行参数硬编码传递信息。

应该设计一个带版本和身份快照的任务文件：

```text
task.json
```

例如：

```json
{
  "taskId": "uuid",
  "version": 1,
  "operation": "restore-existing",
  "image": {
    "volume": {"diskGuid": "...", "partitionGuid": "..."},
    "relativePath": "MyBackup\\2026-08-13\\Windows.wim",
    "sha256": "...",
    "index": 1
  },
  "target": {
    "diskGuid": "...",
    "partitionGuid": "...",
    "partitionOffset": 0,
    "partitionSize": 0,
    "role": "existing-windows"
  },
  "bootPlan": {"mode": "return-existing", "previousBcdHash": "..."},
  "created": "2026-08-13T00:00:00",
  "bootOnce": true,
  "status": "pending"
}
```

备份任务：

```json
{
  "taskId": "uuid",
  "version": 1,
  "operation": "backup",
  "source": {"diskGuid": "...", "partitionGuid": "..."},
  "destination": {
    "volume": {"diskGuid": "...", "partitionGuid": "..."},
    "relativePath": "MyBackup\\2026-08-13\\Windows.wim"
  },
  "compression": "max",
  "checkIntegrity": true,
  "status": "pending"
}
```

用户主动选择建立双系统时，`operation` 使用 `create-secondary`，`target.role` 为
`new-windows`，`bootPlan.mode` 为 `add-secondary`，并记录用户确认的
启动菜单名称。GUI/PowerShell 接收用户选择的 Windows 绝对镜像路径（例如
`B:\BackupRestore\Windows.wim`），同时在任务中记录镜像卷身份；为适应 WinRE
换盘符，任务还可保存经过校验的卷内相对路径，但绝不能把可变盘符作为唯一定位方式。

Recovery.exe 启动后：

```text
读取 task.json
    ↓
验证任务
    ↓
执行 operation
```

------

# 10. 任务文件必须放在不会被覆盖的位置

**不能把 task.json 放在即将覆盖的目标分区中，然后直接覆盖该分区。**

因为还原过程中 Windows 分区会被覆盖。

任务文件应该放在程序目录所在的普通卷中；该卷必须与还原目标不同。程序
目录移动到另一卷后，下一次运行自动使用新目录，不依赖旧路径或注册表：

```text
程序目录\
└── tasks\<taskId>\
    ├── task.json
    ├── status.json
    ├── Recovery.log
    └── manifest.json
```

任务目录必须位于不会被目标分区覆盖的位置，且至少保留一份正常启动
所需的失败结果。Recovery 载荷使用同样的 `taskId` 和清单，启动器只能
执行清单中哈希匹配的 `Recovery.exe`。

------

# 11. 用户点击“备份”

GUI：

```text
┌──────────────────────────────┐
│      Windows 系统备份         │
│                              │
│  备份位置                    │
│  D:\MyBackup\                │
│                              │
│       [开始备份]             │
└──────────────────────────────┘
```

点击以后：

```text
检查磁盘
    ↓
检查备份空间和备份卷身份
    ↓
检查 WinRE
    ↓
创建 backup task
    ↓
设置一次性进入恢复环境
    ↓
重启
```

------

# 12. WinRE 中执行备份

Recovery.exe 启动。

自动读取：

```text
JSON文件
```

假设备份：

```text
D:\
```

备份位置：

```text
E:\MyBackup\当前时间.wim
```

执行：

```cmd
dism /Capture-Image ^
 /ImageFile:E:\MyBackup\当前时间.wim ^
 /CaptureDir:D:\ ^
 /Name:"Windows Backup" ^
 /Compress:max ^
 /CheckIntegrity
```

Recovery.exe 必须实时获取 DISM 输出并显示进度。

示例中的盘符仅为说明。Recovery 阶段必须先按卷身份解析实际盘符，确认
目标卷不是镜像卷，再将解析出的盘符传给 DISM。备份镜像必须写入
`.partial` 文件；完成后执行 `/CheckIntegrity`、WIM 信息读取和 SHA-256
校验，校验成功后再原子改名为正式镜像。

------

# 13. 备份完成

执行：

```text
DISM 完成
    ↓
验证 WIM
    ↓
生成 metadata.json
    ↓
写入完成状态
    ↓
保留 backup task 和 status 结果，待正常 Windows 读取并确认后归档
    ↓
恢复正常启动
    ↓
重启
```

用户重新进入 Windows。

GUI 显示：

```text
备份成功

Windows 11 Pro
备份时间：2026-08-13
镜像：当前时间.wim
大小：XX GB
```

------

# 14. 用户点击“还原”

GUI：

用户选择镜像后，程序默认进入“还原当前 Windows”模式并展示自动识别的
当前 Windows 分区。用户可以主动切换到“建立第二个 Windows”模式，再选
择一个已有的空闲/可覆盖 NTFS 分区作为第二系统目标；不允许选择 EFI、
MSR、Recovery 分区或镜像所在分区。

```text
┌──────────────────────────────┐
│       Windows 系统还原        │
│                              │
│  2026-08-13                  │
│  Windows 11 Pro              │
│  Windows.wim                 │
│                              │
│       [开始还原]             │
└──────────────────────────────┘
```

点击以后弹出警告：

```text
即将还原 Windows。

所选目标分区中的数据将被备份镜像覆盖。
请确认重要数据已经备份。
镜像路径
要还原的目标分区
[取消]    [确认还原]
```

确认以后：

```text
验证 WIM
    ↓
验证备份元数据
    ↓
再次显示并确认磁盘 GUID、分区 GUID、偏移、大小和启动菜单名称
    ↓
创建 restore task
    ↓
配置下一次启动进入 WinRE
    ↓
重启
```

------

# 15. 一次性启动

目标：

```text
Windows
   ↓
下一次启动 WinRE
   ↓
完成后自动恢复正常 Windows
```

**不要修改 Windows 默认启动项永久指向 WinRE。**

应该使用一次性启动机制。候选实现是记录原始 BCD 快照，并使用经 Phase 0
验证的 `bootsequence`/WinRE 启动方式；不得假定写入一个 Recovery BCD 项
就一定会自动进入 WinRE。恢复失败时必须能够按快照恢复原来的默认项。

也就是说：

```text
BootSequence
```

完成后自动恢复正常启动。

如果使用 BCD API/BCDEdit，必须确保：

```text
正常 Windows
    ↓
一次性 Recovery
    ↓
Recovery 完成
    ↓
正常 Windows
```

而不是：

```text
Windows
 ↓
WinRE
 ↓
WinRE
 ↓
……
```

------

# 16. WinRE 自动启动 Recovery.exe

这是整个项目的核心实现之一。

目标：

```text
进入 WinRE
    ↓
自动执行
    ↓
Recovery.exe
```

用户不能需要手动点击：

```text
高级选项
→ 命令提示符
→ xxx.exe
```

必须自动启动、执行恢复。

V1 的候选实现是为当前注册的 `winre.wim` 创建任务专用副本，在副本中
放入启动器和 `Recovery.exe`，再通过临时启动配置加载这个副本。原始
`winre.wim` 必须先备份并计算哈希。只有在虚拟机中验证以下完整链路后，
该候选实现才能成为正式实现：

```text
Windows -> 临时 WinRE 副本 -> 启动器 -> Recovery.exe -> 返回原系统
```

要求：

1. 不破坏正常 Windows Recovery 功能
2. 不永久修改用户注册的原始 WinRE；任务副本必须可删除、可重建
3. Windows 更新后不能轻易导致系统无法启动
4. 必须有失败恢复方案
5. Recovery.exe 必须能够自动启动
6. Recovery.exe 完成后必须恢复正常 Windows 启动

如果不能在 Win11 ARM64 和 Win11 x64 环境中证明自动入口，V1 自动流程
不得以“用户手动打开命令提示符”替代，自动恢复链应标记为未完成。

------

# 17. Recovery.exe 启动以后

首先：

```text
显示：

系统恢复
正在准备……
```

然后：

```text
读取 task.json
```

验证：

```text
task version
operation
卷身份、绝对镜像路径（以及供 WinRE 重挂载使用的受校验卷内路径）和目标身份
WIM existence
WIM validity
image SHA-256
载荷清单和 Recovery.exe SHA-256
```

如果任务损坏：

```text
恢复任务无效。
请重新启动 Windows。
```

------

# 18. 还原前检测

在 Windows 下，点击还原前必须检测：

```text
Windows 分区
EFI 分区
备份 WIM
磁盘容量
磁盘类型
GPT
```

并确认：

```text
目标分区可容纳镜像展开后的文件，并满足元数据记录的最小分区大小
```

WIM 文件大小不是还原所需空间。最小空间必须来自捕获元数据中的已用
空间、系统保留空间和预留余量，并在 Recovery 中以实际目标分区大小再
次检查。对于普通单系统还原，目标分区必须是当前 Windows 分区；对于用户
主动选择的 `create-secondary`，目标分区必须不是镜像所在分区，且不能
包含需要保留的 Windows 安装。

如果不足：

```text
目标磁盘空间不足。
```

停止还原。

------

# 19. 还原 WIM

找到：

```text
Windows.wim
```

执行：

```cmd
dism /Apply-Image ^
 /ImageFile:E:\MyBackup\Windows.wim ^
 /Index:1 ^
 /ApplyDir:C:\
```

`C:` 和 `E:` 只是假设盘符。Recovery 必须先解析目标卷和镜像卷，再传入
实际盘符。还原策略分两种：

- `restore-existing`：默认单系统流程。最终身份复核通过后，只格式化并
  应用到当前 Windows 分区；绝不操作整块磁盘、EFI、MSR、Recovery 或镜像
  分区。
- `create-secondary`：仅在用户主动选择双系统模式后使用，只格式化用户
  确认的第二系统目标分区，保留原有 Windows 分区、原有 EFI/BCD 和所有
  Recovery 分区。

格式化是不可逆阶段，必须在 `target-erased` 状态写盘成功后才允许执行，
并在界面和日志中明确记录目标的磁盘 GUID、分区 GUID、起始偏移和大小。



------

# 20. 还原前处理 Windows 分区

不要直接：

```text
format C:
```

必须：

```text
识别物理磁盘
    ↓
识别 GPT
    ↓
识别 EFI System Partition
    ↓
识别所有 Windows 分区和目标角色
    ↓
最终复核后只处理目标分区
```

必须保护：

```text
Recovery Partition
Backup Partition
```

除非用户明确要求重新分区。

------

# 21. 还原 EFI

WIM 只负责 Windows 系统分区。

还原完成以后必须重新建立 Windows 启动环境。

找到：

```text
EFI System Partition
```

执行：

```cmd
bcdboot <resolved-windows>:\Windows /s <resolved-efi>:\ /f UEFI
```

然后检查：

```text
EFI\Microsoft\Boot\bootmgfw.efi
```

和 BCD 是否存在。

单系统还原必须验证原有 Windows loader 能启动。双系统建立必须新增一个
指向第二系统分区的 loader，并设置用户确认的显示名称；不能删除或覆盖
原有 loader。写入后重新读取 BCD，验证每个 loader 的 `device`、`osdevice`
和 `path` 均指向预期分区，且默认项与用户选择一致。

------

# 22. 还原成功后的清理

必须：

```text
保留 restore task 和 status 结果，待正常 Windows 读取并确认后再归档
```

删除任务专用的临时启动配置和 WinRE 副本：

```text
Recovery BCD Entry（仅限本任务创建的项）
```

确保：

```text
下一次启动 = 正常 Windows
```

然后：

```cmd
shutdown /r /t 0
```

------

# 23. Recovery.exe 必须有状态机制

不能出现：

```text
还原到 80%
电脑断电
```

然后下一次开机不知道发生过什么。

建议：

```text
task.json
status.json
```

状态：

```text
prepared
boot-requested
recovery-started
preflight
target-erased
image-applied
boot-repaired
success
failed
```

例如：

```json
{
  "operation": "restore-existing",
  "status": "image-applied",
  "progress": 78
}
```

完成：

```json
{
  "operation": "restore-existing",
  "status": "success"
}
```

失败：

```json
{
  "operation": "restore",
  "status": "failed",
  "errorCode": 123
}
```

------

# 24. 日志

必须有完整日志。

例如：

```text
Recovery.log
```

记录：

```text
2026-08-13 01:00:01 Starting Recovery
2026-08-13 01:00:02 Task loaded
2026-08-13 01:00:03 Windows volume resolved from partition GUID = <resolved-letter>
2026-08-13 01:00:04 Backup image resolved from volume GUID = <resolved-path>
2026-08-13 01:00:05 Checking WIM
2026-08-13 01:00:10 Apply image started
2026-08-13 01:15:23 Apply image completed
2026-08-13 01:15:30 EFI = S:
2026-08-13 01:15:31 BCDBoot completed
2026-08-13 01:15:32 Recovery completed
```

失败时记录：

```text
DISM exit code
BCDBoot exit code
错误消息
当前磁盘
当前分区
当前任务
```

------

# 25. 断电/异常情况

必须设计异常保护。

例如还原过程中：

```text
突然断电
```

下次启动仍然要能够：

```text
进入 Recovery
    ↓
发现 restore task 处于未完成状态
    ↓
判断系统当前状态
    ↓
提示恢复失败/继续恢复
```

V1 可以采取比较简单的策略：

```text
如果 restore task 处于 `recovery-started`、`preflight` 或 `target-erased`
    ↓
再次进入 Recovery
    ↓
读取最后一个持久化阶段和目标分区状态
    ↓
在身份与镜像校验通过后重新执行完整 Apply-Image
```

`target-erased` 之后不得仅凭旧状态自动判定安全；必须把失败标记
为可恢复或不可恢复，并保留现场日志。每次状态变更都要原子写入并刷新到
任务所在的非目标卷。WIM Apply 可以重新执行，但只能在这些检查全部通过
后执行。

------

# 26. BitLocker

V1 要求：

```text
检测 BitLocker
```

如果源卷、目标卷、镜像卷或 EFI 相关卷开启 BitLocker，且 Recovery 无法
在不泄露密钥的前提下访问：

```text
提示：

当前系统启用了 BitLocker。

V1 暂不支持自动处理 BitLocker，
请先暂停/关闭 BitLocker，并确认目标卷在 Recovery 中可访问后再进行备份还原。
```

**不要偷偷修改 BitLocker 状态。**

V1 不自动暂停、解锁或修改保护状态。以后 V2 再做：

```text
Suspend-BitLocker
Unlock
Recovery Key
TPM
```

------

# 27. GUI 最简单设计

首页：

```text
┌────────────────────────────────────┐
│        My Windows Backup           │
│                                    │
│   当前系统                         │
│   Windows 11 Pro x64               │
│   当前模式：单系统备份/还原       │
│                                    │
│  ┌────────────┐  ┌────────────┐   │
│  │  系统备份   │  │  系统还原   │   │
│  └────────────┘  └────────────┘   │
│                                    │
│                                    │
└────────────────────────────────────┘
```

还原页面：

```text
┌────────────────────────────────────┐
│             系统还原                │
│                                    │
│  备份时间：2026-08-13              │
│  Windows：Windows 11 Pro           │
│  镜像大小：82.4 GB                 │
│   备份位置：D:\MyBackup\镜像.wim      │
│   目标分区：当前 Windows 分区          │
│  ⚠提示：当前系统分区将被清空并覆盖    │
│                                    │
│             [开始还原]              │
└────────────────────────────────────┘
```

还原页面必须提供明确的模式选择：默认是“还原当前 Windows”，另有“建立
第二个 Windows（可选）”入口。只有第二种模式显示第二系统目标分区和启动
菜单名称，不得让普通单系统还原流程承担双系统配置。

备份页面：

```
┌────────────────────────────────────┐
│             备份                │
│                                    │
│  备份时间：2026-08-13              │
│  Windows：Windows 11 Pro           │
│  镜像大小：82.4 GB                 │
│   备份位置：D:\MyBackup\镜像.wim      │    
│   源分区：磁盘 GUID / 分区 GUID        │
│  ⚠提示：镜像不得位于源分区           │
│                                    │
│             [开始备份]              │
└────────────────────────────────────┘
```



------

# 28. 最重要的安全原则

程序必须：

### 绝不能：

```text
误格式化备份盘
误格式化 EFI
误格式化 Recovery
误删除其他分区
永久修改默认 Windows 启动项
在没有确认的情况下开始还原
```

### 必须：

```text
二次确认
磁盘识别
分区识别
WIM 验证
任务状态
日志
失败处理
启动恢复
显示并确认磁盘 GUID、分区 GUID、偏移和大小
用户选择双系统模式时保留原有 Windows 启动项
镜像与目标分区不得位于同一分区
原子状态写入和失败结果归档
```

------

# 29. 开发顺序

让 AI **严格按照这个顺序开发**，不要一次生成全部代码。

### Phase 0：恢复入口 PoC

先在 Parallels 的 Win11 ARM64 虚拟机中验证基础恢复入口：

```text
Windows -> 临时 WinRE 副本 -> 自动启动 Recovery.exe -> 正常 Windows
```

再在 Win11 x64 虚拟机中重复。只读检测已经确认当前 ARM64 虚拟机为
UEFI/GPT，包含 Recovery、EFI、MSR、Windows 和 Recovery 分区；本次测试
未取得管理员令牌，因此 `reagentc`、BCD 和 BitLocker 的状态仍需在提升
权限的 Guest Tools 会话中补验。Phase 0 未通过前，不得开始真实格式化或
真实还原。

### Phase 1

先实现：

```text
检测 UEFI/GPT
检测 Windows 分区
检测 WinRE
检测 EFI
枚举所有 Windows 安装
枚举可作为第二系统的已有 NTFS 分区
记录磁盘/分区身份快照
```

------

### Phase 2

实现：

```text
WIM 信息读取
DISM Capture
DISM Apply
```

先不用自动重启，也不执行格式化。

------

### Phase 3

实现：

```text
创建 task.json
读取 task.json
状态管理
日志
```

------

### Phase 4

实现：

```text
BCD 管理
一次性启动
```

先测试：

```text
Windows
 ↓
WinRE
 ↓
正常返回 Windows
```

**不要一上来测试真实还原。**

还要验证：现有 BCD 默认项保持不变、临时启动失败可回滚、Recovery
完成后不会循环进入 WinRE。

------

### Phase 5

实现：

```text
WinRE
 ↓
自动启动 Recovery.exe
```

先让：

```text
Recovery.exe
```

只显示：

```text
Hello Recovery
```

确认自动启动成功。

------

### Phase 6

加入：

```text
DISM Capture
```

测试：

```text
Windows
 ↓
WinRE
 ↓
Recovery.exe
 ↓
备份
 ↓
Windows
```

------

### Phase 7

加入：

```text
DISM Apply
BCDBoot
```

先测试默认的 `restore-existing` 单系统流程。只有实现用户主动选择双系统
模式后，再额外测试 `create-secondary`：原 Windows 可启动、第二 Windows
可启动、启动菜单名称正确、删除/失败时原启动项不丢失。双系统测试不是
单系统备份/还原的前置条件。

------

# 30. 最终验收标准

开发完成以后，必须满足：

### 备份

```text
Windows
 ↓
点击「备份」
 ↓
自动重启
 ↓
自动进入 WinRE
 ↓
自动启动 Recovery.exe
 ↓
自动创建 Windows.wim
 ↓
自动重启
 ↓
Windows 正常启动
```

### 还原

```text
Windows
 ↓
点击「还原」
 ↓
确认
 ↓
自动重启
 ↓
自动进入 WinRE
 ↓
自动启动 Recovery.exe
 ↓
自动找到 Windows.wim
 ↓
自动还原
 ↓
自动修复 EFI/BCD
 ↓
自动重启
 ↓
Windows 正常启动
```

默认验收为单系统还原。双系统是用户主动选择时的附加验收：

### 可选：建立双系统

```text
Windows
 ↓
选择 WIM 和已有空闲/可覆盖 NTFS 分区
 ↓
确认磁盘 GUID、分区 GUID、大小和启动菜单名称
 ↓
自动重启并进入 Recovery.exe
 ↓
仅清空目标分区并应用 Windows.wim
 ↓
保留原 EFI/BCD，新增第二 Windows 启动项
 ↓
验证两个 loader 的 device/osdevice/path
 ↓
启动菜单可选择两个 Windows
```

验收还必须覆盖：镜像损坏、镜像所在卷不可用、镜像与目标同分区、空间
不足、目标身份变化、EFI/Recovery 误选、BCD 修复失败、断电发生在每个
阶段、WinRE 自动入口失败、临时启动循环、Secure Boot、BitLocker、
Windows 10/11 不同构建版本，以及 ARM64 测试环境与 x64 发布包的架构拒绝。

## 32. 本次 Parallels 测试记录

测试环境：Parallels Desktop 27，`Windows 11.pvm`，Windows 11 ARM64，
UEFI、GPT、Secure Boot 开启、TPM 开启、单块约 256 GB 虚拟磁盘。

已完成的只读验证（用于基础检测和恢复入口 PoC，不代表必须启用双系统）：

- 虚拟机可以通过 `prlctl` 启动，Guest Tools 可以执行 Windows 命令。
- 系统报告为 Windows 11 ARM64，Build 26200。
- 磁盘报告为 GPT，包含 Recovery、EFI System、MSR、Windows 和第二个
  Recovery 分区；Windows 当前盘符为 `C:`。
- 未执行格式化、Apply-Image、BCDBoot 写入或 WinRE 修改。

未完成的验证：Guest Tools 使用普通用户令牌，`reagentc /info`、BCD 和
BitLocker 查询被 Windows 拒绝访问。因此该 VM 结果不能证明管理员权限下
的 WinRE/BCD/BitLocker 行为，也不能证明 x64 兼容性。后续必须使用提升的
Guest Tools 会话补验，并准备 Win11 x64 虚拟机完成发布架构验收。

### WinRE 自动启动 PoC 结果

已在该 VM 完成真实重启测试。管理员对注册在 Recovery 分区的 `winre.wim`
制作临时副本，调用 `reagentc /boottore` 后重启，VM 确实自动进入 Windows
RE。第一轮仅替换 `startnet.cmd`，WinRE 停在标准“选择一个选项”界面，且
没有执行脚本，证明 `startnet.cmd` 不是标准 WinRE UI 的自动入口。

第二轮在任务专用 WIM 的 `Windows\System32\winpeshl.ini` 中配置：

```ini
[LaunchApps]
%SYSTEMROOT%\System32\RecoveryPoC.cmd
%SYSTEMROOT%\System32\recenv.exe
```

重启后 `RecoveryPoC.cmd` 在持久化 Windows 卷写入
`C:\WinRE-PoC\recovery-started.txt`，内容为 `RecoveryPoC.cmd started`，随后
自动返回正常 Windows。该标记证明链路成立：

```text
Windows -> reagentc /boottore -> staged WinRE -> winpeshl.ini
        -> RecoveryPoC.cmd -> recenv.exe -> Windows
```

PoC 结束后已将注册 WinRE 恢复为原始 WIM；原始与恢复后 SHA-256 都是
`396B9E18F46C26174724F53A0F9B8751E273DC86F76FAFF7FA4E691699E53CC7`，且
`reagentc /info` 显示 WinRE 仍为 Enabled。正式实现应使用任务专用 WIM、
启动器和完整性清单，先恢复已注册的原始 WIM，再执行实际 Recovery 程序。

整个过程中：

> **用户不需要按 F12、不需要进入 BIOS、不需要选择 WinRE、不需要输入命令、不需要手动启动 Recovery.exe。**

------

# 33. 最终项目应该是什么

最终不是：

> “一个调用 DISM 的 GUI”。

而是一个完整的**Windows 一键系统备份/还原程序**：

```text
                    ┌──────────────────┐
                    │ BackupRestore.exe│
                    └────────┬─────────┘
                             │
                    ┌────────┴────────┐
                    │                 │
                  Backup       Restore / Dual System
                    │                 │
                    └────────┬────────┘
                             │
                       Recovery Task
                             │
                             ▼
                         BCD / Boot
                             │
                             ▼
                     Staged WinRE
                             │
                             ▼
                       Recovery.exe
                             │
                  ┌──────────┴──────────┐
                  │                     │
               Capture                Apply
                  │                     │
                  ▼                     ▼
              Windows.wim          Windows.wim
                                        │
                                        ▼
                                    BCDBoot
                                        │
                                        ▼
                                     Windows
```

**最关键的一点：V1 不要自己制作 WinPE。直接利用 WinRE；不自己实现镜像算法，直接利用 DISM；不自己实现 Bootloader，利用 Windows BCD/BCDBoot。**

等 V1 稳定以后，再考虑把 WinRE 替换成自己的 WinPE，甚至进一步做增量备份、整盘备份、BitLocker 支持等。
