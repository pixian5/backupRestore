# V1 需求与证据矩阵

这份矩阵防止把静态代码检查误报为 Windows/WinRE 实机成功。状态只允许使用：

- **代码已覆盖**：源代码和离线测试已覆盖，仍可能需要实机确认；
- **离线已验证**：本机命令已经通过，但不等同于 Windows 运行；
- **实机待验证**：必须在管理员 Windows 11 ARM64 快照中执行；
- **不在 V1**：需求明确排除。

| 需求 | 代码证据 | 当前状态 | 实机验收证据 |
|---|---|---|---|
| Windows 10/11、UEFI、GPT | `windows/BackupRestore.ps1:Assert-SystemEnvironment` | 代码已覆盖 | `prepare.log` 中 OS、firmware、GPT 检查通过 |
| x64/ARM64 架构隔离 | `windows/build-windows.ps1`、`build-manifest.json`、`Assert-PackageArchitecture` | ARM64 二进制已在 VM 启动并通过 hash/schema 检查，WinRE 运行待验证 | x64/ARM64 各自产物在对应 Guest 启动并拒绝错架构 |
| 当前/候选 Windows 分区枚举 | `BackupRestore.Gui.ps1:Get-EnvironmentText` | 代码已覆盖 | GUI 实盘显示并人工核对 GUID |
| 盘符不是身份 | `VolumeIdentity`、`verify_task_identity_env`、`mount_env_volume` | 代码已覆盖 | 改盘符或更换卷后任务必须拒绝 |
| probe 任务/源卷相同 | `recover-env` 复用已验证的 `T:` 挂载 | 代码已覆盖 | 同一分区 probe 不再重复分配 `S:`，并留下成功状态和日志 |
| EFI/MSR/Recovery 保护 | core `is_reserved_partition`、PowerShell、`Recovery.cmd` | 离线已验证 | 实盘尝试选中三类分区都被拒绝 |
| 备份 `.partial`、WIM 校验、SHA-256、metadata | `recover_windows` backup、`BackupMetadata` | 代码已覆盖 | DISM Capture 后检查 WIM、metadata 和 hash |
| 镜像容量不是目标容量 | metadata `required_target_size`、PowerShell/Recovery 双重检查 | 离线已验证 | 小目标卷在格式化前拒绝 |
| BitLocker 不自动修改 | PowerShell `Get-BitLockerVolume` 检查 | 代码已覆盖 | 开启保护的源/镜像/目标任务拒绝且状态不变 |
| 临时 WinRE 副本和原始 hash | `BackupRestore.ps1`、manifest、`WinreRestoreGuard` | 代码已覆盖 | 任务前后原始 WinRE hash 相同 |
| WinRE 自动启动 Recovery.exe | `winpeshl.ini`、`RecoveryLauncher.cmd` | 实机待验证 | Windows -> 重启 -> WinRE 后持久化 marker 和 `Recovery.log` |
| 一次性启动后返回正常 Windows | `reagentc /boottore`、清理/重启路径 | 实机待验证 | 重启次数、BCD 默认项、无 WinRE 循环 |
| 单系统还原 | `restore-existing`、format、Apply-Image、BCDBoot | 实机待验证 | 快照中系统启动、用户文件按预期覆盖 |
| 双系统还原 | `create-secondary`、`/addlast`、BCD menu name | 实机待验证 | 原 loader 和新 loader 的 device/osdevice/path 均正确 |
| 断电恢复 | `Stage`、`recover_windows` resume 分支 | 代码已覆盖 | 每个阶段断电后快照恢复并检查状态 |
| BCD 失败回滚 | BCD snapshot、`restore_bcd_snapshot` | 代码已覆盖 | 模拟 BCDBoot 失败后原 BCD hash 恢复 |
| GUI 三页面和二次确认 | `BackupRestore.Gui.ps1` | 离线已验证（AST） | WPF 真实操作、UAC、长路径和日志刷新 |
| 任务结果不虚报 | `last-task.json`、结果页文案、`status.json` | 代码已覆盖 | 正常 Windows 读取 WinRE 最终状态 |
| 网络/工具链下载规则 | `~/.codex/skills/pixian-dev-workflow/SKILL.md` | 流程已覆盖 | 每个大下载前保留网络检查和授权证据 |

## 当前强制实机顺序

1. 先只做 `probe -NoReboot`，不格式化、不 Apply、不重启；确认载荷完整性。
2. 在 VM 快照上验证 Windows -> WinRE -> Recovery.exe 自动入口，并把持久化 marker 写在任务卷，不能写 WinRE 的 `X:` RAM 盘。
3. 再验证备份；最后按默认单系统还原、可选双系统还原顺序执行。
4. 每次实机测试结束后导出最小压缩证据：最终 `status.json`、`Recovery.log`、`prepare.log`、前后 WinRE/BCD hash 和关键截图；恢复快照后再开始下一项。

当前进度和用户明确的下载授权规则见 [project-status.md](project-status.md)。
