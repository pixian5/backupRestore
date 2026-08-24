# V1 需求与证据矩阵

这份矩阵防止把静态代码检查误报为 Windows/WinRE 实机成功。状态只允许使用：

- **代码已覆盖**：源代码和离线测试已覆盖，仍可能需要实机确认；
- **离线已验证**：本机命令已经通过，但不等同于 Windows 运行；
- **实机待验证**：必须在管理员 Windows 11 ARM64 快照中执行；
- **实机已验证**：有对应 Windows VM 日志、持久化状态和回归证据；适用范围必须写明，不能外推到未执行的破坏性流程；
- **不在 V1**：需求明确排除。

| 需求 | 代码证据 | 当前状态 | 实机验收证据 |
|---|---|---|---|
| Windows 10/11、UEFI、GPT | `windows/BackupRestore.ps1:Assert-SystemEnvironment` | 代码已覆盖 | `prepare.log` 中 OS、firmware、GPT 检查通过 |
| x64/ARM64 架构隔离 | `windows/build-windows.ps1`、`build-manifest.json`、`Assert-PackageArchitecture` | `v0.4.8` ARM64 二进制已在 VM 启动并通过 manifest/`Recovery.exe hash` 检查，WinRE 运行待验证 | x64/ARM64 各自产物在对应 Guest 启动并拒绝错架构 |
| 当前/候选 Windows 分区枚举 | Rust `discover_drives`、`Refresh environment` | 代码已覆盖 | GUI 实盘显示下拉项的文件系统、容量、剩余空间和 GUID |
| 盘符不是身份 | `VolumeIdentity`、`verify_task_identity_env`、`mount_env_volume` | 代码已覆盖 | 改盘符或更换卷后任务必须拒绝 |
| 镜像使用绝对路径 | `ImagePath`、`validate_absolute_path`、task `absolutePath`、`IMAGE_ABSOLUTE_PATH` | `v0.5.0` 代码和 ARM64 参数帮助已验证；Recovery 仍按 GUID 重挂载后使用卷内路径 | Windows GUI 选择 `B:\...\Windows.wim`，换盘符后必须仍解析到同一镜像卷 |
| probe 任务/源卷相同 | `recover-env` 按卷 GUID 扫描已有挂载，任务/源同卷时复用实际盘符 | 实机已验证（Win11 ARM64 probe） | 任务 `c12026c0-6a9e-4093-8a8b-2971968a31f7` 在 WinRE 记录 `TASK volume already mounted at C:; reusing it`，随后为 `success` |
| EFI/MSR/Recovery 保护 | core `is_reserved_partition`、PowerShell、`Recovery.cmd` | 离线已验证 | 实盘尝试选中三类分区都被拒绝 |
| 备份 `.partial`、WIM 校验、SHA-256、metadata | `recover_windows` backup、`BackupMetadata`、原生输出按字节日志 | 实机已验证（隔离源卷/镜像卷）；容量拒绝分支已验证 | `v0.4.7` 任务 `ff6b645b-b9e4-4b4e-945a-1fb406923b0d` 为 `success`；WIM `c08c4e7a9628ead708802ca880f46932a4cb6e0547f0cad735bf79ef29711b30` 与 metadata 一致。新一轮 C: -> B: 因 24.0 GB < 250.4 GB 被拒绝，`.partial` 不存在 |
| 镜像容量不是目标容量 | metadata `required_target_size`、PowerShell/Recovery 双重检查 | 离线已验证 | 小目标卷在格式化前拒绝 |
| BitLocker 不自动修改 | PowerShell `Get-BitLockerVolume` 检查 | 代码已覆盖 | 开启保护的源/镜像/目标任务拒绝且状态不变 |
| 临时 WinRE 副本和原始 hash | `BackupRestore.ps1`、manifest、`WinreRestoreGuard` | 实机已验证（Win11 ARM64 probe） | 任务原始与重启返回 Windows 后注册 `Winre.wim` 均为 `0cbc86b44994065c7295f0322df670cf0b6c9e4a7be5099cfd962ddec956fda1` |
| WinRE 自动启动 Recovery.exe | `winpeshl.ini`、`RecoveryLauncher.cmd` | 实机已验证（Win11 ARM64 probe） | `Recovery-launcher.log` 记录 `Recovery.exe present` 与 `starting Recovery.exe`；`recovery.log` 记录 Recovery.exe 从 env 启动 |
| 一次性启动后返回正常 Windows | `reagentc /boottore`、清理/重启路径 | 实机已验证（Win11 ARM64 probe） | `recovery.log` 记录 `wpeutil.exe reboot`；VM 屏幕和 Guest Tools 均确认已回到正常 Windows，未出现 WinRE 循环 |
| 单系统还原 | `restore-existing`、DiskPart format、Apply-Image、BCDBoot `/v` | 实机已验证（隔离直接恢复；未做该 EFI 的实际重启） | `v0.4.7` 任务 `821eb6af-f13c-46b2-8c1c-af1aa8345e42` 为 `success`；`S:` 的 SYSTEM、EFI_EX、BOOTRES、Fonts、bootstr 恢复，`E:\EFI\Microsoft\Boot\bootmgfw.efi`、`bootaa64.efi`、BCD 存在；管理员 `bcdedit /store ... /enum all /v` 返回 0 |
| 双系统还原 | `create-secondary`、`/addlast`、BCD menu name | 实机待验证 | 原 loader 和新 loader 的 device/osdevice/path 均正确 |
| 断电恢复 | `Stage`、`recover_windows` resume 分支 | 代码已覆盖 | 每个阶段断电后快照恢复并检查状态 |
| BCD 失败回滚 | BCD snapshot、`restore_bcd_snapshot` | 代码已覆盖 | 模拟 BCDBoot 失败后原 BCD hash 恢复 |
| Rust Win32 GUI 单窗口、二次确认和多语言 | `crates/backuprestore-cli/src/native_gui.rs`、`BackupRestore.exe` | `v0.5.3` ARM64 包已在 VM 启动；Rust 是唯一桌面前端；无控制台子系统、中文模式提示、WIM 索引下拉框和隐藏管理员读取已通过真实窗口点测；盘符详细下拉框和隐藏 PowerShell 待新包验收 | `BackupRestore - Rust GUI` 窗口真实显示；读取 `B:\BackupRestore\Windows.wim` 后显示索引 1/Windows Backup/162.9 MiB；操作模式与卷选择器均由 Rust 控件提供 |
| 任务结果不虚报 | `last-task.json`、结果页文案、`status.json` | 实机已验证（Win11 ARM64 probe） | 自动 probe 任务 `c12026c0-6a9e-4093-8a8b-2971968a31f7` 的 `status.json` 为 `success` 仅出现在原始 WinRE hash 恢复校验之后；新 `-NoReboot` 任务 `760fcfe1-392d-4142-94af-b830f4286296` 保持 `prepared`，dry-run 不会伪造成功 |
| 网络/工具链下载规则 | `~/.codex/skills/pixian-dev-workflow/SKILL.md` | 流程已覆盖 | 每个大下载前保留网络检查和授权证据 |

## 当前强制实机顺序

1. ARM64 自动 probe 已实机通过。任何修改 WinRE 载荷、启动器、盘符策略或清理逻辑后，先重复 `probe -NoReboot`，再在新快照中重复自动 probe。
2. 下一项在独立快照中验证备份；先检查任务卷、镜像卷和剩余容量，不足时停止而不是创建不完整 WIM。
3. 最后按默认单系统还原、可选双系统还原顺序执行，并在每项前确认快照、目标分区身份和破坏性授权范围。
4. 每次实机测试结束后导出最小压缩证据：最终 `status.json`、`Recovery.log`、`prepare.log`、前后 WinRE/BCD hash 和关键截图；恢复快照后再开始下一项。

当前进度和用户明确的下载授权规则见 [project-status.md](project-status.md)。
