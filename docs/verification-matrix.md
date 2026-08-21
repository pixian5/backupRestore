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
| x64/ARM64 架构隔离 | `windows/build-windows.ps1`、`build-manifest.json`、`Assert-PackageArchitecture` | ARM64 二进制已在 VM 启动并通过 hash/schema 检查，WinRE 运行待验证 | x64/ARM64 各自产物在对应 Guest 启动并拒绝错架构 |
| 当前/候选 Windows 分区枚举 | `BackupRestore.Gui.ps1:Get-EnvironmentText` | 代码已覆盖 | GUI 实盘显示并人工核对 GUID |
| 盘符不是身份 | `VolumeIdentity`、`verify_task_identity_env`、`mount_env_volume` | 代码已覆盖 | 改盘符或更换卷后任务必须拒绝 |
| probe 任务/源卷相同 | `recover-env` 按卷 GUID 扫描已有挂载，任务/源同卷时复用实际盘符 | 实机已验证（Win11 ARM64 probe） | 任务 `c12026c0-6a9e-4093-8a8b-2971968a31f7` 在 WinRE 记录 `TASK volume already mounted at C:; reusing it`，随后为 `success` |
| EFI/MSR/Recovery 保护 | core `is_reserved_partition`、PowerShell、`Recovery.cmd` | 离线已验证 | 实盘尝试选中三类分区都被拒绝 |
| 备份 `.partial`、WIM 校验、SHA-256、metadata | `recover_windows` backup、`BackupMetadata`、原生输出按字节日志 | `v0.4.6` 代码已包含修复，实机待重跑 | DISM Capture 后检查 WIM、metadata 和 hash；旧 `v0.3.6` 任务曾被本地代码页日志解析缺陷阻断 |
| 镜像容量不是目标容量 | metadata `required_target_size`、PowerShell/Recovery 双重检查 | 离线已验证 | 小目标卷在格式化前拒绝 |
| BitLocker 不自动修改 | PowerShell `Get-BitLockerVolume` 检查 | 代码已覆盖 | 开启保护的源/镜像/目标任务拒绝且状态不变 |
| 临时 WinRE 副本和原始 hash | `BackupRestore.ps1`、manifest、`WinreRestoreGuard` | 实机已验证（Win11 ARM64 probe） | 任务原始与重启返回 Windows 后注册 `Winre.wim` 均为 `0cbc86b44994065c7295f0322df670cf0b6c9e4a7be5099cfd962ddec956fda1` |
| WinRE 自动启动 Recovery.exe | `winpeshl.ini`、`RecoveryLauncher.cmd` | 实机已验证（Win11 ARM64 probe） | `Recovery-launcher.log` 记录 `Recovery.exe present` 与 `starting Recovery.exe`；`recovery.log` 记录 Recovery.exe 从 env 启动 |
| 一次性启动后返回正常 Windows | `reagentc /boottore`、清理/重启路径 | 实机已验证（Win11 ARM64 probe） | `recovery.log` 记录 `wpeutil.exe reboot`；VM 屏幕和 Guest Tools 均确认已回到正常 Windows，未出现 WinRE 循环 |
| 单系统还原 | `restore-existing`、DiskPart format、Apply-Image、BCDBoot `/v` | 实机待验证 | 任务 `985ab31b-e9f1-4c64-a49d-7b044e5f8cde` 已在 `S:` 完成格式化与 Apply，旧 BCDBoot 返回 193；普通令牌 `/v` 证明隔离 EFI 写入需高完整性（0x5），仍须管理员 `x` 高完整性重跑并检查 `E:\EFI\Microsoft\Boot\bootmgfw.efi`、BCD、回归启动 |
| 双系统还原 | `create-secondary`、`/addlast`、BCD menu name | 实机待验证 | 原 loader 和新 loader 的 device/osdevice/path 均正确 |
| 断电恢复 | `Stage`、`recover_windows` resume 分支 | 代码已覆盖 | 每个阶段断电后快照恢复并检查状态 |
| BCD 失败回滚 | BCD snapshot、`restore_bcd_snapshot` | 代码已覆盖 | 模拟 BCDBoot 失败后原 BCD hash 恢复 |
| Rust Win32 GUI 单窗口和二次确认 | `crates/backuprestore-cli/src/native_gui.rs`、`BackupRestore.exe` | `v0.4.5` ARM64 Windows 进程烟测已通过（窗口标题 `BackupRestore - Rust GUI`）；`v0.4.6` 本机离线测试通过 | 真实环境/镜像/任务状态按钮、管理员 UAC、长路径和日志刷新仍待 VM 验收 |
| 任务结果不虚报 | `last-task.json`、结果页文案、`status.json` | 实机已验证（Win11 ARM64 probe） | `status.json` 为 `success` 仅出现在原始 WinRE hash 恢复校验之后；日志顺序可复核 |
| 网络/工具链下载规则 | `~/.codex/skills/pixian-dev-workflow/SKILL.md` | 流程已覆盖 | 每个大下载前保留网络检查和授权证据 |

## 当前强制实机顺序

1. ARM64 自动 probe 已实机通过。任何修改 WinRE 载荷、启动器、盘符策略或清理逻辑后，先重复 `probe -NoReboot`，再在新快照中重复自动 probe。
2. 下一项在独立快照中验证备份；先检查任务卷、镜像卷和剩余容量，不足时停止而不是创建不完整 WIM。
3. 最后按默认单系统还原、可选双系统还原顺序执行，并在每项前确认快照、目标分区身份和破坏性授权范围。
4. 每次实机测试结束后导出最小压缩证据：最终 `status.json`、`Recovery.log`、`prepare.log`、前后 WinRE/BCD hash 和关键截图；恢复快照后再开始下一项。

当前进度和用户明确的下载授权规则见 [project-status.md](project-status.md)。
