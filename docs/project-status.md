# BackupRestore 当前进度与决策记录

更新时间：2026-08-22
当前开发版本：`0.4.4`
分支：`main`
本地开发基线：`b32acac`

本文件是当前状态的唯一摘要。它把用户对下载、功能完整性和交付方式的明确要求记录为工程约束；不逐字保存情绪化表达，只保留会影响后续行为的事实和结论。

## 1. 产品目标与 V1 范围

BackupRestore 是面向 Windows 10/11 UEFI/GPT 设备的开发测试版系统备份与还原工具。正常 Windows 负责选择任务、核验环境并准备任务专用 WinRE；WinRE 自动启动 `Recovery.exe`，使用 DISM 捕获或应用 WIM，必要时用 BCDBoot 修复启动项，最后恢复原始 WinRE 并重启。

V1 已设计的任务包括：

| 任务 | 作用 | 是否具有破坏性 |
|---|---|---|
| `probe` | 只验证任务、载荷、卷挂载和 WinRE 自动入口 | 否 |
| `backup` | 从现有 Windows 分区用 DISM 捕获 WIM | 否，写入指定镜像卷 |
| `restore-existing` | 将镜像还原到原 Windows 分区并修复引导 | 是 |
| `create-secondary` | 将镜像还原到独立分区并加入第二启动项 | 是 |

V1 不包含自研 PE、分区布局重构、网络备份、增量/差异镜像、Legacy BIOS 或自动修改 BitLocker。任何恢复测试只能在可回滚的虚拟机快照中进行。

## 2. 用户对话中确认的约束

| 对话意图 | 固化后的执行规则 | 当前落实位置 |
|---|---|---|
| 不要在未获适当网络条件时下载大文件 | 不在无 Wi-Fi 或用户要求停止下载时尝试替代下载、断点下载或后台下载；先完成不消耗流量的代码、文档和离线检查。 | 工作流规则、本文第 5 节 |
| 热点上的每次大下载都要重新授权 | Rust target、Visual Studio/MSVC、Windows SDK、Docker 镜像、依赖、模型和安装包均是独立下载；每次先运行 `check_network.py`。结果为热点时，必须就这一次下载等待用户明确授权。 | `pixian-dev-workflow`、交接文档 |
| Wi-Fi 或有线网络不重复打断 | 同一次独立下载的网络检测结果为 Wi-Fi/有线且 `is_hotspot=false` 时，可以继续该下载，不需要重复确认。下一个独立大下载仍要重新检测网络。 | `pixian-dev-workflow`、交接文档 |
| 工具链不可用时，先把程序完整设计和编码 | 离线阶段完成任务模型、正常 Windows 脚本、WinRE 载荷、GUI 三页、构建脚本、日志/状态、恢复边界和文档；工具链装好后不重新设计功能。 | 本仓库全部实现与第 3 节 |
| 工具链装好后应能直接构建使用 | 构建脚本不自动下载，ARM64/x64 分离，明确 VM 本地 target 目录、包结构和 WinRE 启动入口；实际可用性仍以 Windows/WinRE 证据为准。 | `windows/build-windows.ps1`、`windows-build.md` |
| 工具链准备好后直接开发 | 通过共享桌面传输当前工作树源码，使用 VM 本地 Cargo target 直接构建 ARM64 包；不把源码传输误写成工具链下载。 | `windows/build-windows.ps1`、本文件第 3 节 |

网络状态不是永久属性，本文不把某次检测结果当作后续下载授权。恢复下载工作前必须重新检测。

## 3. 当前完成度

### 已完成的离线实现

| 范畴 | 已实现内容 | 证据状态 |
|---|---|---|
| 任务与安全模型 | UUID 任务目录、原子 JSON、状态机、卷完整身份、相对路径限制、EFI/MSR/Recovery 排除、容量和 BitLocker 拒绝规则。 | Rust 单元测试已覆盖关键规则 |
| 任务卷防替换 | 新任务记录 `taskVolume`；WinRE 复核任务、源、镜像和目标的 GUID、分区位置、大小、类型、文件系统与序列号。 | 代码与离线测试已覆盖 |
| WinRE 载荷完整性 | 原始/暂存 WinRE、任务专用 payload manifest、每个载荷的 SHA-256、`RecoveryTask.env` 二次 hash 校验。 | 代码已覆盖 |
| 正常 Windows 准备 | 环境、UEFI/GPT、WinRE、Secure Boot、BitLocker 检查；注册 Recovery 分区定位；EFI 选择；WIM 注入；BCD 快照；`last-task.json`。 | PowerShell AST 已通过 |
| WinRE 恢复 | 固定盘符重新挂载、镜像和 metadata 校验、DISM Capture/Apply、BCDBoot、阶段续跑、BCD 失败回滚、原始 WinRE 清理守卫。 | probe 已实机完成自动入口和清理；破坏性恢复仍未验收 |
| 成功状态一致性 | WinRE 只有在原始注册 `Winre.wim` 恢复并通过 SHA-256 校验后才写入 `success`；probe 支持 `preflight -> success`；清理失败写入 `failed` 并保留恢复日志。 | probe 实机已验收 |
| probe 同卷情形 | `recover-env` 按卷 GUID 复用已有盘符；同卷源使用任务卷实际盘符。真实备份/还原仍要求任务卷与源/目标独立。 | Win11 ARM64 probe 实机已验收 |
| GUI | Rust Win32 原生窗口负责操作模式、任务/源/镜像/目标卷、WIM 索引、镜像读取、身份状态、破坏性确认和管理员准备启动；旧 PowerShell/WPF 页面保留兼容但不再是默认入口。 | ARM64 交叉编译已通过；原生窗口真实交互、UAC、日志刷新待 VM 解锁后验收 |
| 构建与交付结构 | `build-windows.ps1` 生成架构隔离包，`BackupRestore.exe` 启动 GUI，`Recovery.exe` 作为 WinRE 主机，并随包携带 ARM64 MSVC runtime。 | ARM64 `v0.4.2` 已完成构建，manifest 与二进制 SHA-256 一致；WinRE 自动 probe/破坏性流程仍按证据矩阵区分 |
| Windows 工具链 | Rust 1.98.0、`aarch64-pc-windows-msvc`、Visual Studio Build Tools ARM64、Windows SDK `10.0.26100.0`。 | 已安装并用于构建 |

### 已执行且通过的本机验证

以下命令只使用本机已有的离线缓存，不能证明 Windows 可运行：

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
pwsh -NoLogo -NoProfile -NonInteractive -Command '$files=@("windows/BackupRestore.ps1","windows/BackupRestore.Gui.ps1","windows/build-windows.ps1"); foreach($f in $files){$tokens=$null;$errors=$null; [System.Management.Automation.Language.Parser]::ParseFile((Join-Path (Get-Location) $f),[ref]$tokens,[ref]$errors)|Out-Null; if($errors.Count){$errors|% Message; exit 1}; "AST OK $f"}'
git diff --check
```

当前结果：Rust core 12 项测试通过；Clippy 无 warning；3 个 PowerShell 脚本 AST 通过；差异无空白错误。

## 4. 明确未完成的实机验收

以下项目均不能因代码、AST 或 macOS 测试通过而标记完成：

1. 在独立 VM 快照中验证 DISM Capture；
2. 在独立 VM 快照中验证 `restore-existing`、`create-secondary`、BCDBoot、启动菜单、原 WinRE hash 恢复和正常 Windows 回归；
3. 验证错误边界：BitLocker 拒绝、卷身份不匹配拒绝、目标容量不足拒绝、DISM/BCDBoot 失败后的状态和 BCD 回滚、每个阶段的断电续跑；
4. 在真实 WPF 窗口核验磁盘枚举、确认对话框、长路径、日志刷新和 UAC 行为。

2026-08-21 已实测：`Windows 11` ARM64 VM 使用 `v0.3.6` 完成真实自动 probe，任务为 `c12026c0-6a9e-4093-8a8b-2971968a31f7`。流程完成 `Windows -> reagentc /boottore -> WinRE -> winpeshl.ini -> RecoveryLauncher.cmd -> Recovery.exe -> probe preflight -> 原始 WinRE hash 恢复 -> wpeutil reboot -> Windows`；最终 `status.json` 为 `success`，注册 WinRE 与任务原始副本 SHA-256 均为 `0cbc86b44994065c7295f0322df670cf0b6c9e4a7be5099cfd962ddec956fda1`。这证明自动入口、同卷盘符复用和清理状态机；不证明 DISM Capture/Apply、格式化、BCDBoot、双系统或故障回滚。

2026-08-22 备份 fixture 实测：管理员准备任务 `53332658-7dd8-4684-9b19-372d9c680a95` 成功，但旧 `v0.3.6` Recovery 在 DISM Capture 输出为本地代码页时因 UTF-8 日志解析失败，任务为 `failed`，`Windows.wim.partial` 保留用于诊断。该问题已在 `v0.3.7` 修复，新的备份任务必须重新准备，不能复用失败任务。

同日交付产物：历史 ARM64 压缩包已统一移入项目 `.test-artifacts/desktop-archive/2026-08-22`，不再散落桌面。后续桌面只保留用户自己的文件；项目测试截图统一放在 `.test-artifacts/root-captures/`，不进入仓库根目录。

2026-08-22 隔离还原已推进到引导修复：任务 `985ab31b-e9f1-4c64-a49d-7b044e5f8cde` 在唯一允许的 `S:` 测试卷上完成 DiskPart 格式化和 DISM Apply，且 `S:\Windows\System32\config\SYSTEM` 已恢复。它只指定独立 `E:` EFI 卷（`\\?\Volume{6ba9bc91-04dd-4105-9c46-7377ce26b862}\`），不接触 `C:` 或真实 EFI。旧包 BCDBoot 返回 193；普通令牌 `/v` 诊断返回 5，并明确是隔离 EFI `0x5 Access denied`。`P8B6\\x` 是本地 Administrators 成员，后续必须由其高完整性 RunAs 进程重试 `v0.4.1`，成功前“单系统还原”仍保持实机待验证。

2026-08-22 GUI 收口：Rust `native_gui.rs` 接管 `BackupRestore.exe` 默认窗口，使用 Windows SDK Win32 API，不下载 GUI crate；窗口包含操作模式、任务/源/镜像/目标卷、镜像路径、WIM 索引、第二系统名称、环境刷新、镜像读取和管理员创建任务。旧 `BackupRestore.Gui.ps1` 仅作为兼容前端保留。`v0.4.4` ARM64 包已启动烟测通过，窗口标题为 `BackupRestore - Rust GUI`；按钮交互、UAC、日志刷新仍须在 Windows VM 解锁后验收，不能用进程存活代替完整 GUI 验收。

## 5. 恢复工作时的唯一顺序

当前阶段已进入直接开发/构建，但不执行破坏性恢复操作。后续按以下顺序执行：

1. 每个独立大下载前运行：

   ```bash
   python3 ~/.codex/skills/pixian-dev-workflow/scripts/check_network.py
   ```

2. 检测为热点时，报告具体要下载的一个项目并等待授权；检测为 Wi-Fi/有线时，执行该下载。下载下一个大项目之前重新检测。
3. 保持 Cargo target 在 VM 本地，例如 `C:\BackupRestoreBuild\target`；源码可通过共享桌面传输，避免再次下载仓库压缩包。
4. 在 VM 中运行：

   ```powershell
   .\windows\build-windows.ps1 -Architecture arm64 -CargoTargetDir C:\BackupRestoreBuild\target
   ```

5. 自动 probe 已在 ARM64 快照实测成功；后续任何影响 WinRE 路径的修改都要先重复 `probe -NoReboot` 与自动 probe。当前下一项是独立快照中的备份，其后才是单系统还原、双系统还原和故障注入。
6. 每项实机测试仅保留最小压缩证据：最终 `status.json`、`Recovery.log`、`prepare.log`、前后 WinRE/BCD hash 和必要截图。每项结束后恢复 VM 快照。

## 6. 文档职责与验收口径

| 文档 | 用途 |
|---|---|
| `docs/verification-matrix.md` | 逐项列出代码、离线、实机证据，更新状态只能依据对应证据。 |
| `docs/continuation-handoff.md` | 说明代码结构、任务边界、危险点和后续执行命令。 |
| `docs/implementation-notes.md` | 记录实现细节和已经发生过的技术问题。 |
| `docs/windows-build.md` | 说明 Windows 包的构建条件、架构隔离和输出。 |
| 本文件 | 记录当前进度、用户决策和继续工作的门槛。 |

状态术语固定为“代码已覆盖”“离线已验证”“实机待验证”“实机已验证”“不在 V1”。只有带有 Windows VM 实际日志、持久化文件或可复核快照证据的项目可以从“实机待验证”变更为“实机已验证”。
