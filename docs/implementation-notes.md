# V1 实现说明

## 已实现的安全骨架

- `backuprestore-core` 提供统一的任务 JSON、卷身份、WIM 哈希/大小校验、容量校验、BitLocker 拒绝策略、原子 `task.json` / `status.json` 写入和状态机。
- 备份、已有 Windows 还原、创建第二 Windows 三种任务都要求源/镜像/目标分区身份明确；镜像与目标、源与目标不能相同，EFI/MSR/Recovery 分区不能作为目标。
- 恢复状态在准备、启动、WinRE、预检、捕获、擦除、应用、修复引导、成功/失败之间单向流转；准备阶段失败也会落盘失败状态，避免“无状态卡死”。
- Windows 准备脚本检查 Windows 10/11、ARM64（同时保留 x64 配置）、UEFI/GPT、WinRE、BitLocker 状态，保存原始 WinRE 与 BCD，并用 SHA-256 清单保护 WinRE payload。
- `winpeshl.ini` 现在自动进入哈希校验的 `RecoveryLauncher.cmd`；真实任务优先启动 `Recovery.exe recover-env`，没有 exe 时只允许 `probe` 兼容脚本运行。
- WinRE 执行路径按磁盘号/分区号和卷 GUID 重新挂载固定临时盘符，并逐项比对任务 JSON 与 RecoveryTask.env 的磁盘/分区/大小快照；恢复前再次校验镜像与 metadata，备份完成后原子生成 `metadata.json`，所有破坏性操作都要求 `-AllowDestructive`。
- ARM64 备份元数据记录实际架构、Windows 版本/构建号、已用空间和预留空间；Recovery.exe 的 DISM/BCDBoot 输出同时实时写入日志和 WinRE 控制台，启动修复失败时尝试导入任务创建前的 BCD 快照。
- GUI 在提交前重新读取任务、源、镜像和目标的磁盘 GUID、分区 GUID、偏移、大小、文件系统，并把这些值放入确认对话框。
- Rust Recovery 路径带有清理守卫：在 WinRE 已挂载任务卷后，即使载荷校验、磁盘挂载或 DISM/BCDBoot 提前失败，也会尝试恢复原始注册 WinRE，并保留失败日志。

## 当前验证结果

macOS 本地已完成：

- `cargo test --workspace --all-targets --offline`：核心 crate 7 项测试通过；
- `cargo fmt --all -- --check`：通过；
- PowerShell AST 解析：`windows/BackupRestore.ps1`、`windows/BackupRestore.Gui.ps1` 均通过。

## 尚未宣称完成的实机项

当前先以 Parallels Win11 ARM64 为主线，以下必须在 ARM64 虚拟机和快照上验证后才能发布：

1. ARM64 `Recovery.exe` 的编译、复制进 WinRE 并从 `winpeshl.ini` 实际启动；
2. DISM Capture/Apply、快速格式化、BCDBoot 返回已有系统和添加第二启动项（含启动菜单名称）的完整链路；
3. BitLocker 解锁/拒绝、异常断电后的原始 WinRE/BCD 回滚；
4. GUI 在真实磁盘枚举、二次确认和任务日志展示上的可用性。

在这些实机项完成前，项目属于开发测试版；x64 构建暂缓，不要把 macOS 离线测试结果当作 Windows 恢复成功证明。ARM64 包还会读取
`build-manifest.json`，拒绝在不匹配的 Windows 架构上运行。

## ARM64 收尾顺序

1. 在 Win11 ARM64 虚拟机安装 Rust 与 `aarch64-pc-windows-msvc` target（下载前遵守个人热点确认）。
2. 运行 `windows\\build-windows.ps1 -Architecture arm64`，检查 `build-manifest.json` 中的二进制 SHA-256。
3. 在管理员 Guest Tools 会话执行无破坏性的 `probe -NoReboot`，确认任务身份复核、WinRE 载荷哈希和 `winpeshl.ini -> Recovery.exe` 自动入口。
4. 使用快照分别验证 DISM Capture、单系统 Apply/BCDBoot、双系统 `/addlast`，每次验证后恢复快照并确认原始 WinRE 哈希不变。

## 2026-08-20 ARM64 编译验证

- Parallels Win11 ARM64 已安装 Rust stable、`aarch64-pc-windows-msvc`、Visual Studio C++ Build Tools 和 ARM64 SDK。
- 构建脚本支持 `-CargoTargetDir`，将 Cargo 临时产物放在 VM 本地磁盘，避开共享目录的临时归档限制。
- `windows\build-windows.ps1 -Architecture arm64 -CargoTargetDir C:\BackupRestoreBuild\target` 成功生成版本 0.1.0 包。
- `dumpbin /headers` 报告 `AA64 machine (ARM64)`；`BackupRestore.exe` 与 `Recovery.exe` 的哈希和 `build-manifest.json` 一致。
- 两个 ARM64 程序在 VM 内分别成功执行 `run-command` 和 `hash`。曾发现 1 MiB 栈上哈希缓冲会触发 ARM64 `STATUS_STACK_OVERFLOW`，已改为堆上缓冲。
- 本次只做编译、哈希和无破坏性命令验证，没有执行格式化、DISM Apply、BCDBoot 写入或真实重启恢复。
