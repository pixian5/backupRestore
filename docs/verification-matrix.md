# V1 需求与证据矩阵

这份矩阵防止把静态代码检查误报为 Windows/WinRE 实机成功。状态只允许使用：

- **代码已覆盖**：源代码和离线测试已覆盖，仍可能需要实机确认；
- **离线已验证**：本机命令已经通过，但不等同于 Windows 运行；
- **实机待验证**：必须在管理员 Windows 11 ARM64 快照中执行；
- **实机已验证**：有对应 Windows VM 日志、持久化状态和回归证据；适用范围必须写明，不能外推到未执行的破坏性流程；
- **不在 V1**：需求明确排除。

| 需求 | 代码证据 | 当前状态 | 实机验收证据 |
|---|---|---|---|
| Windows 10/11、UEFI、GPT | `windows_prepare.rs`、`VolumeIdentity` | Rust prepare 已在 ARM64 编译；完整架构/UEFI拒绝待补测 | `prepare.log` 中 OS、WinRE、GPT 身份检查通过 |
| x64/ARM64 架构隔离 | `windows/build-windows.ps1`、`build-manifest.json`、`Assert-PackageArchitecture` | `v0.4.8` ARM64 二进制已在 VM 启动并通过 manifest/`Recovery.exe hash` 检查，WinRE 运行待验证 | x64/ARM64 各自产物在对应 Guest 启动并拒绝错架构 |
| 当前/候选 Windows 分区枚举 | Rust `discover_drives`、`Refresh environment` | 代码已覆盖 | GUI 实盘显示下拉项的文件系统、容量、剩余空间和 GUID |
| 盘符不是身份 | `VolumeIdentity`、`verify_task_identity_env`、`mount_env_volume` | 代码已覆盖 | 改盘符或更换卷后任务必须拒绝 |
| Recovery 盘符冲突安全边界 | `identity_from_diskpart`、`ensure_volume_mounted` | **代码已覆盖；v1.0.1 ARM64 重建通过** | 已挂载卷先按实际磁盘/分区身份复用；固定 `R:` 被占用或 DiskPart 失败时不会读取错误卷，实机故障注入待验证 |
| 镜像使用绝对路径 | `ImagePath`、`validate_absolute_path`、task `absolutePath`、`IMAGE_ABSOLUTE_PATH` | `v0.5.0` 代码和 ARM64 参数帮助已验证；Recovery 仍按 GUID 重挂载后使用卷内路径 | Windows GUI 选择 `B:\...\Windows.wim`，换盘符后必须仍解析到同一镜像卷 |
| probe 任务/源卷相同 | `recover-env` 按卷 GUID 扫描已有挂载，任务/源同卷时复用实际盘符 | 实机已验证（Win11 ARM64 probe） | 任务 `c12026c0-6a9e-4093-8a8b-2971968a31f7` 在 WinRE 记录 `TASK volume already mounted at C:; reusing it`，随后为 `success` |
| EFI/MSR/Recovery 保护 | core `is_reserved_partition`、Rust `windows_prepare`、Rust Recovery | Rust 代码与 ARM64 原生 GPT 枚举已验证 | 实盘尝试选中三类分区都被拒绝 |
| 备份 `.partial`、WIM 校验、SHA-256、metadata | `recover_windows` backup、`BackupMetadata`、原生输出按字节日志 | 实机已验证（隔离源卷/镜像卷）；容量拒绝分支已验证 | `v0.4.7` 任务 `ff6b645b-b9e4-4b4e-945a-1fb406923b0d` 为 `success`；WIM `c08c4e7a9628ead708802ca880f46932a4cb6e0547f0cad735bf79ef29711b30` 与 metadata 一致。新一轮 C: -> B: 因 24.0 GB < 250.4 GB 被拒绝，`.partial` 不存在 |
| 镜像容量不是目标容量 | metadata `required_target_size`、Rust prepare/Recovery 双重检查 | 离线已验证 | 小目标卷在格式化前拒绝 |
| BitLocker 不自动修改 | Rust `manage-bde.exe -status` 只读检查 | 代码已覆盖 | 开启保护的源/镜像/目标任务拒绝且状态不变 |
| 临时 WinRE 副本和原始 hash | Rust `prepare`、manifest、`WinreRestoreGuard` | 旧流程实机已验证；本轮 Rust prepare 已完成无重启 probe，自动重启待复验 | 任务原始与重启返回 Windows 后注册 `Winre.wim` hash 一致 |
| WinRE 自动启动 Recovery.exe | `winpeshl.ini`、`RecoveryLauncher.cmd` | 实机已验证（Win11 ARM64 probe） | `Recovery-launcher.log` 记录 `Recovery.exe present` 与 `starting Recovery.exe`；`recovery.log` 记录 Recovery.exe 从 env 启动 |
| 一次性启动后返回正常 Windows | `reagentc /boottore`、清理/重启路径 | 实机已验证（Win11 ARM64 probe） | `recovery.log` 记录 `wpeutil.exe reboot`；VM 屏幕和 Guest Tools 均确认已回到正常 Windows，未出现 WinRE 循环 |
| Rust prepare 自动进入 WinRE | Rust `prepare`、`RecoveryLauncher.cmd`、Rust `recover-env` | **实机已验证（v0.8.0 ARM64 probe）** | 任务 `dcff7126-aa6b-4a5b-910c-d5acbbcbdebe` 从 T: 程序目录完成自动 probe；`status.json=success`，原始 WinRE hash 已恢复。 |
| 单系统还原 | `restore-existing`、DiskPart format、Apply-Image、BCDBoot `/v` | **实机已验证（非 C、多索引 Index 2、ARM64）** | 任务 `375f4422-7c17-4397-9560-6c83d7ca9ff4` 为 `success`；U: 目标 SYSTEM 与 fixture 源 hash 相同；E: BCD、`bootmgfw.efi`、`bootaa64.efi` 存在；Recovery 日志确认 `/Index:2` 和 `bcdboot ... /s Z:`。不证明从 E: 实际重启进入 U: |
| Rust prepare 多索引单系统还原 | Rust `prepare`、Rust `recover-env`、`restore-existing` | **实机已验证（v0.8.2 ARM64）** | 任务 `fcfdd192-61e0-4b14-b04f-9734dcd26e48` 从 B: 工作目录使用 T: Index 2 还原 U:，DiskPart/DISM/BCDBoot/WinRE 清理完成；U: SYSTEM hash 为 `A70A0D…CC550`，E: BCD 默认 loader 指向 U:。 |
| 独立 EFI 实际引导（开发测试） | Parallels `hdd2` 首启动、EFI E:、U: 已 Apply Windows | **实机失败（ARM64，已复测）**；不属于普通 GUI/V1 发布门槛 | E: BCD 默认 loader `device/osdevice=partition=U:`，`bcdboot U:\Windows /s E: /f UEFI /v` 成功；关闭 Secure Boot、并替换 E: bootmgfw 为 U: 同哈希版本后，hdd2 仍进入 Recovery `0xc0430001`。启动顺序和 Secure Boot 已恢复。 |
| 双系统还原 | `create-secondary`、`/addlast`、BCD menu name | 实机待验证 | 原 loader 和新 loader 的 device/osdevice/path 均正确 |
| 断电恢复 | `Stage`、`recover_windows` resume 分支 | 代码已覆盖 | 每个阶段断电后快照恢复并检查状态 |
| BCD 失败回滚 | BCD snapshot、`restore_bcd_snapshot` | 代码已覆盖 | 模拟 BCDBoot 失败后原 BCD hash 恢复 |
| Rust Win32 GUI 单窗口、二次确认和多语言 | `crates/backuprestore-cli/src/native_gui.rs`、`BackupRestore.exe` | **实机已验证（GUI 范围）**：`v0.6.5` 客户区动态排版在每次标签切换后重排，详情框 112 高度、行距 8、状态框 64；逐页实际矩形确认探测/备份/单系统还原/第二系统的字段显隐、镜像/按钮不重叠，第二系统按钮位于客户区内；v1.1.0 启动会正常关闭旧 GUI 并保留唯一最新实例 | v1.1.0 ARM64 实测 6 个历史 GUI 已收敛为 1 个响应中的 `BackupRestore - Rust GUI v1.1.0` 窗口。发布验收还要求包目录、`build-manifest.json` 与 GUI 标题版本三者一致；不包括 UAC、任务创建、WinRE 或磁盘写入。 |
| WIM 多索引读取与详细下拉显示 | `parse_wim_images`、`parse_dism_images`、`report_images_value` | **实机已验证（v1.0.9 ARM64）** | 前台 GUI 用 `--open-image C:\BackupRestoreBuild\multi-index-gui-v1.0.8.wim` 读取同源双索引 WIM；状态确认 2 个索引，默认下拉项显示索引 1、名称和描述。截图在 `.test-artifacts/root-captures/v1.0.9-open-image-c-drive.png`。 |
| GUI 管理员令牌与 WIM 读取 | `is_elevated`、`relaunch_elevated`、`ShellExecuteW("runas")` | **实机已验证（v0.7.4 ARM64）** | GUI 自提升后读取 WIM 成功，不再出现 DISM 740；最新 GUI 以高完整性用户进程运行 |
| 英文标签可见性 | `ui_text`、`operation_display` | **实机已验证（v0.7.3 ARM64）** | 英文模式四个顶部标签显示 `Inspect / Backup / Restore / Second system`，无裁剪；详细行为由说明框显示 |
| 英文 UI 纯净性 | `ui_text(Language::English, "language")` | **实机已验证（v0.7.3 ARM64）** | 英语模式显示纯 `Language`，不残留中文；中文模式仍显示 `语言` |
| 中文任务状态编码 | Rust Win32 文本控件与 `read_json` BOM 兼容 | **代码已覆盖；旧包实机已验证** | 当前 Rust GUI 不再经由 PowerShell 输出；旧 v0.7.4 证据仅证明历史中文显示结果 |
| 非 C 指定分区真实备份 | `BackupRestore.exe prepare -SourceDrive U -ImagePath T:\...`、Recovery 状态机 | **实机已验证（ARM64）** | 任务 `bc1660b8-6863-495d-b333-cd168f9a5c41` 最终 success；U: fixture 未格式化，WIM/metadata/hash 已记录 |
| 多索引 WIM 导出 | DISM Export-Image | **实机已验证（ARM64）** | T: `non-c-u-multi\Windows.wim` 显示索引 1 和 2；Index 2 已用于任务 `375f4422-7c17-4397-9560-6c83d7ca9ff4` 还原 |
| 隔离测试 EFI 选择 | `BackupRestore.exe prepare --test-efi-drive`、Rust EFI 校验、Recovery EFI=Z: | **实机已验证（ARM64）** | 独立 EFI E: GUID 写入任务 env，WinRE 使用 Z: 临时挂载，避免镜像卷 E: 冲突；参数仅用于开发测试，普通 GUI 不暴露。 |
| 工作目录所在卷与镜像卷重叠拒绝 | `native_gui.rs:create_task`、Rust `prepare`、core `Task::validate` | 离线已验证；ARM64 GUI 待验证 | GUI 在创建任务前按卷身份尽早拒绝，Rust prepare 在任何 BCD/WinRE 写入前按 GUID 拒绝，核心校验作为防御纵深 |
| 任务结果不虚报 | `last-task.json`、结果页文案、`status.json` | 实机已验证（Win11 ARM64 probe） | 自动 probe 任务 `c12026c0-6a9e-4093-8a8b-2971968a31f7` 的 `status.json` 为 `success` 仅出现在原始 WinRE hash 恢复校验之后；新 `-NoReboot` 任务 `760fcfe1-392d-4142-94af-b830f4286296` 保持 `prepared`，dry-run 不会伪造成功 |
| 网络/工具链下载规则 | `~/.codex/skills/pixian-dev-workflow/SKILL.md` | 流程已覆盖 | 每个大下载前保留网络检查和授权证据 |

## 当前强制实机顺序

1. ARM64 自动 probe 已实机通过。任何修改 WinRE 载荷、启动器、盘符策略或清理逻辑后，先重复 `probe -NoReboot`，再在新快照中重复自动 probe。
2. 下一项在独立快照中验证备份；先检查工作目录所在卷、镜像卷和剩余容量，不足时停止而不是创建不完整 WIM。
3. 最后按默认单系统还原、可选双系统还原顺序执行，并在每项前确认快照、目标分区身份和破坏性授权范围。
4. 每次实机测试结束后导出最小压缩证据：最终 `status.json`、`Recovery.log`、`prepare.log`、前后 WinRE/BCD hash 和关键截图；恢复快照后再开始下一项。

当前进度和用户明确的下载授权规则见 [project-status.md](project-status.md)。
