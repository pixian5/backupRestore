# BackupRestore 当前进度与决策记录

更新时间：2026-09-13
当前开发版本：`1.5.10`（Cargo.toml×2 与 VERSION 已同步；完整当前基线见 [current-progress-2026-09-13.md](current-progress-2026-09-13.md)）
分支：`main`
本地开发基线：以当前 `HEAD` 为准

本文件是当前状态的唯一摘要。它把用户对下载、功能完整性和交付方式的明确要求记录为工程约束；不逐字保存情绪化表达，只保留会影响后续行为的事实和结论。

> 2026-09-13 起，涉及当前版本、未完成项和最新事实的结论以 [current-progress-2026-09-13.md](current-progress-2026-09-13.md) 为准；本文件后面的时间线保留历史证据，不自动代表 v1.5.8 已通过。

每轮执行均遵守[开发执行协议](development-execution-protocol.md)：先规划、自审，再执行和验证。程序目录内的日志位置、格式和保留边界也以该文档为准。

## 1. 产品目标与 V1 范围

BackupRestore 是面向 Windows 10/11 UEFI/GPT 设备的开发测试版系统备份与还原工具。正常 Windows 负责选择任务、核验环境并准备任务专用 WinRE；WinRE 自动启动 `Recovery.exe`，使用 DISM 捕获或应用 WIM，必要时用 BCDBoot 修复启动项，最后恢复原始 WinRE 并重启。

V1 已设计的任务包括：

| 任务 | 作用 | 是否具有破坏性 |
|---|---|---|
| `probe` | 只验证任务、载荷、卷挂载和 WinRE 自动入口 | 否 |
| `backup` | 从现有 Windows 分区用 DISM 捕获 WIM | 否，写入指定镜像卷 |
| `restore-existing` | 将镜像还原到原 Windows 分区并修复引导 | 是 |
| `create-secondary` | 将镜像还原到独立分区并加入第二启动项 | 是 |

V1 已实现自定义 PE 作为恢复环境（RAM disk 不占分区 / 硬盘启动独立分区两种模式，见 [current-progress-2026-09-11.md](current-progress-2026-09-11.md)）；不包含分区布局重构、网络备份、增量/差异镜像、Legacy BIOS 或自动修改 BitLocker。任何恢复测试只能在可回滚的虚拟机快照中进行。

## 1.1 用户期望流程对照

| 用户期望 | 当前实现 | 结论 |
|---|---|---|
| 选择镜像保存位置、选择要备份的分区，重启进入 WinRE，由程序执行备份 | GUI/Rust prepare 传入用户选择的 `ImagePath` 绝对路径和 `SourceDrive`；准备阶段注入 `Recovery.exe`、`RecoveryTask.env`、`task.json` 到专用 WinRE，默认会调用 `reagentc /boottore` 后重启；WinRE `recover-env` 按 GUID 挂载源/镜像卷并执行 DISM Capture，写入 `.partial`、metadata 和 SHA-256 | 流程已覆盖；自动 WinRE probe 已实机验证，非 C: 的 `S:`/`B:` Capture 已实机验证 |
| 选择镜像位置、选择恢复目标分区，重启进入 WinRE，由程序执行还原 | GUI/Rust prepare 传入用户选择的 `ImagePath` 绝对路径、`WimIndex` 和目标；准备阶段记录绝对路径并同时保存受 GUID 保护的卷内路径，WinRE 按 GUID 挂载镜像/目标，校验 metadata/hash，格式化明确目标、DISM Apply、BCDBoot，再恢复原 WinRE 并重启 | 流程已覆盖；非 C: 的 `S:`/`B:`/`E:` 隔离 Apply/BCDBoot 已实机验证，但从独立 EFI 实际重启回恢复系统仍待验证 |
| 还原任意指定分区 | `restore-existing` 为安全单系统模式，要求目标与选定源分区相同；`create-secondary` 才允许选择不同 NTFS 目标并使用 `/addlast` | 非 C: P/Q 隔离链路已实机验证；独立 EFI 的固件首启动仍是开发测试待验证项 |

准备阶段使用 `-NoReboot` 时会停在正常 Windows，仅用于测试/诊断；不带该开关才会设置一次性 WinRE 启动并重启。当前 GUI 要求镜像使用类似 `B:\BackupRestore\Windows.wim` 的绝对路径，不再让用户填写相对路径；内部为适应 WinRE 盘符变化会额外保存卷内路径。GUI 不会把“准备成功”误报为 WinRE 还原成功。

## 2. 用户对话中确认的约束

| 对话意图 | 固化后的执行规则 | 当前落实位置 |
|---|---|---|
| 不要在未获适当网络条件时下载大文件 | 不在无 Wi-Fi 或用户要求停止下载时尝试替代下载、断点下载或后台下载；先完成不消耗流量的代码、文档和离线检查。 | 工作流规则、本文第 5 节 |
| 热点上的每次大下载都要重新授权 | Rust target、Visual Studio/MSVC、Windows SDK、Docker 镜像、依赖、模型和安装包均是独立下载；每次先运行 `check_network.py`。结果为热点时，必须就这一次下载等待用户明确授权。 | `pixian-dev-workflow`、交接文档 |
| Wi-Fi 或有线网络不重复打断 | 同一次独立下载的网络检测结果为 Wi-Fi/有线且 `is_hotspot=false` 时，可以继续该下载，不需要重复确认。下一个独立大下载仍要重新检测网络。 | `pixian-dev-workflow`、交接文档 |
| 工具链不可用时，先把程序完整设计和编码 | 离线阶段完成任务模型、正常 Windows 脚本、WinRE 载荷、Rust 原生 GUI、构建脚本、日志/状态、恢复边界和文档；工具链装好后不重新设计功能。 | 本仓库全部实现与第 3 节 |
| 工具链装好后应能直接构建使用 | 构建脚本不自动下载，ARM64/x64 分离，明确 VM 本地 target 目录、包结构和 WinRE 启动入口；实际可用性仍以 Windows/WinRE 证据为准。 | `windows/build-windows.ps1`、`windows-build.md` |
| 工具链准备好后直接开发 | 通过共享桌面传输当前工作树源码，使用 VM 本地 Cargo target 直接构建 ARM64 包；不把源码传输误写成工具链下载。 | `windows/build-windows.ps1`、本文件第 3 节 |
| 不触碰 C: 的范围 | 只禁止把 `C:` 作为 `backup` 的源分区或恢复任务的目标分区；允许读取 C: 状态、使用其 WinRE/BCD、写入正常 Windows 的任务和日志文件，以及在隔离测试中临时修改启动顺序。 | 本轮用户澄清、测试边界 |

网络状态不是永久属性，本文不把某次检测结果当作后续下载授权。恢复下载工作前必须重新检测。

## 3. 当前完成度

### 已完成的离线实现

| 范畴 | 已实现内容 | 证据状态 |
|---|---|---|
| 任务与安全模型 | UUID 任务目录、原子 JSON、状态机、卷完整身份、相对路径限制、EFI/MSR/Recovery 排除、容量和 BitLocker 拒绝规则。 | Rust 单元测试已覆盖关键规则 |
| 工作目录所在卷防替换 | 新任务记录 `workspaceVolume`；WinRE 复核任务、源、镜像和目标的 GUID、分区位置、大小、类型、文件系统与序列号。 | 代码与离线测试已覆盖 |
| WinRE 载荷完整性 | 原始/暂存 WinRE、任务专用 payload manifest、每个载荷的 SHA-256、`RecoveryTask.env` 二次 hash 校验。 | 代码已覆盖 |
| 正常 Windows 准备 | 环境、UEFI/GPT、WinRE、Secure Boot、BitLocker 检查；注册 Recovery 分区定位；EFI 选择；WIM 注入；BCD 快照；`last-task.json`。 | Rust 代码、离线测试和运行时边界审计已通过；ARM64 实机证据见下文 |
| WinRE 恢复 | 固定盘符重新挂载、镜像和 metadata 校验、DISM Capture/Apply、BCDBoot、阶段续跑、BCD 失败回滚、原始 WinRE 清理守卫。 | **v1.2.5 ARM64 已实机完成 P/H/Q Capture、Index 1/2 Apply、单系统/第二系统还原和清理重启；独立 EFI 固件首启动仍是明确的开发测试失败项** |
| 成功状态一致性 | WinRE 只有在原始注册 `Winre.wim` 恢复并通过 SHA-256 校验后才写入 `success`；probe 支持 `preflight -> success`；清理失败写入 `failed` 并保留恢复日志。 | probe 实机已验收 |
| probe 同卷情形 | `recover-env` 按卷 GUID 复用已有盘符；同卷源使用工作目录所在卷实际盘符。真实备份/还原仍要求工作目录所在卷与源/目标独立。 | Win11 ARM64 probe 实机已验收 |
| GUI | Rust Win32 原生单窗口是唯一桌面前端，负责操作模式、源/目标卷详细下拉框、镜像读取、WIM 索引、环境和最近任务状态刷新、盘符校验、破坏性确认和管理员准备启动；支持中文/English 切换；镜像使用绝对路径。WIM 索引条目显示序号、名称、描述、版本、架构、Edition、安装类型和大小。产品运行时不调用 PowerShell。 | `v0.8.x` ARM64 包已使用现有工具链重建并完成 GUI 前台点检；`validate-task`、`recover --dry-run` 和边界审计通过。真实 UAC/WinRE 证据按矩阵记录，旧版本证据仅作历史参考 |
| 构建与交付结构 | `build-windows.ps1` 生成架构隔离包，`BackupRestore.exe` 启动 GUI，`Recovery.exe` 作为 WinRE 主机，并随包携带 ARM64 MSVC runtime。 | `v1.0.5` ARM64 包已在 VM 使用既有工具链构建，manifest 与两个二进制 SHA-256 一致；更早版本的管理员 GUI/UAC、Capture、Apply、格式化和独立 EFI BCDBoot 实测证据仍保留 |
| Windows 工具链 | Rust 1.98.0、`aarch64-pc-windows-msvc`、Visual Studio Build Tools ARM64、Windows SDK `10.0.26100.0`。 | 已安装并用于构建 |

### 已执行且通过的本机验证

以下命令只使用本机已有的离线缓存，不能证明 Windows 可运行：

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
pwsh -NoLogo -NoProfile -NonInteractive -Command '$files=@("windows/BackupRestore.exe prepare","windows/build-windows.ps1"); foreach($f in $files){$tokens=$null;$errors=$null; [System.Management.Automation.Language.Parser]::ParseFile((Join-Path (Get-Location) $f),[ref]$tokens,[ref]$errors)|Out-Null; if($errors.Count){$errors|% Message; exit 1}; "AST OK $f"}'
git diff --check
```

历史结果：macOS 本机 CLI 5 项 + core 17 项、Windows ARM64 v1.3.0 CLI 11 项 + core 17 项曾通过。当前 v1.3.3 的最新离线结果为 CLI 8/8、core 17/17、Clippy、运行时边界审计和差异检查全部通过；ARM64 Release 构建曾成功。共享桌面源码目录不能作为 Cargo 测试目标目录，必须使用客体本地 `C:\BackupRestoreBuild\test-target-v<版本>`。详细当前口径见 [current-progress-2026-09-09.md](current-progress-2026-09-09.md)。

2026-08-25 `v0.7.7` 前置重叠校验：`BackupRestore.exe prepare` 现在在解析工作目录所在卷和镜像卷身份之后、开始 WinRE 注入、BCD 导出或设置一次性恢复启动之前，拒绝非 probe 任务的“工作目录所在卷 = 镜像卷”。Rust GUI 也在用户点击创建任务时立即拒绝同一盘符组合。核心任务模型原有的 GUID 分区重叠检查保留为第二层防线。这样错误参数不再可能先重启进入 WinRE，再由 `validate-task` 拒绝。ARM64 实机以 `B:` 工作目录所在卷、`U:` 源卷和 `B:\...wim` 镜像路径验证：子进程退出码为 1，注册 WinRE SHA-256 前后均为 `0CBC86...6FDA1`。

2026-08-25 `v0.7.7` 独立 EFI 启动实测：在新快照 `before-v0.7.7-efi-boot` 中临时把测试 EFI 磁盘 `hdd2` 调为首启动项，固件确实尝试从独立 EFI 启动，但 Windows Recovery 显示 `0xc0430001`，未进入 `U:`。随后恢复原启动顺序 `hdd0 cdrom0 usb hdd1 hdd2 hdd3` 并重新启动回正常 Windows；`C:` 未作为备份源或恢复目标。该结果将“隔离 Apply/BCDBoot 成功”与“独立 EFI 实际引导成功”明确区分，后者仍未通过。

2026-08-25 `v0.7.8` 独立 EFI 二次诊断：在快照 `before-v0.7.8-efi-bcd`（`c9ed1ee9-0a79-4a90-8c63-29afbc5c3033`）中，用管理员令牌读取 `E:\EFI\Microsoft\Boot\BCD`，确认默认 loader 的 `device/osdevice` 均为 `partition=U:`，`U:` 为 GPT 磁盘 3 分区 3、NTFS、约 8 GiB，`U:\Windows\System32\winload.efi` 存在；独立 EFI `E:` 为磁盘 2 分区 2、FAT32。随后用 `bcdboot U:\Windows /s E: /f UEFI /v` 成功重建 BCD，并再次把 `hdd2` 置首启动真实重启，仍稳定进入 Windows Recovery 并显示 `0xc0430001`，没有进入 `U:`。这排除了“仅因旧 BCD 残留”这一解释，但尚未证明根因；当前保留问题为跨磁盘 UEFI/Secure Boot/Windows loader 兼容性或 EFI/OS 分区关联。启动顺序已恢复为 `hdd0 cdrom0 usb hdd1 hdd2 hdd3`，VM 已回到正常 C: Windows，最新源码对应的 v0.7.7 Rust GUI 包已在客体前台运行。`C:` 仍未作为备份源或还原目标。

## 4. 明确未完成的实机验收

以下项目均不能因代码、AST 或 macOS 测试通过而标记完成。已完成的非 C: 实机证据不外推到未覆盖的场景：

1. `create-secondary`、`/addlast` 的 Apply/BCD/文件 payload 回归已完成；独立 EFI 的 Parallels 固件首启动仍未完成；
2. 从独立 EFI 实际启动恢复卷仍未完成；该功能仅保留为开发测试，不属于普通 GUI 或 V1 发布门槛；
3. 独立 EFI 从 Parallels 固件首启动仍失败于 `0xc0430001`，仅保留为开发测试功能；断电窗口的持久状态、Windows 正常启动后的待恢复任务识别/重新请求 WinRE、BCD 失败回滚、BitLocker/身份/容量拒绝均已有代码或实机证据。各恢复阶段的逐阶段断电注入仍不宣称全部完成；
4. 在最新 ARM64 包的真实 Rust Win32 窗口核验长路径、任务目录迁移、确认对话框和 UAC 行为。

以下时间线中出现的 PowerShell 后端、`-EfiDrive` 或 `RecoveryLauncher.cmd` 均是旧版本历史证据；当前产品运行时只使用 Rust，开发 EFI 参数名称是 `--test-efi-drive`，普通 GUI 不暴露该参数。`1.1.4` 起 `winpeshl.ini` 直接运行 GUI 子系统的 `Recovery.exe recover-env %SYSTEMROOT%\System32\RecoveryTask.env`，不再打包或执行任何产品 `.cmd` 启动器。`1.1.5` 起，当程序目录盘符与还原目标盘符相同，Rust 在读取物理卷、请求提升或创建任务前直接拒绝；后续 GUID 分区身份比对仍保留为第二层校验。

2026-08-21 已实测：`Windows 11` ARM64 VM 使用 `v0.3.6` 完成真实自动 probe，任务为 `c12026c0-6a9e-4093-8a8b-2971968a31f7`。流程完成 `Windows -> reagentc /boottore -> WinRE -> winpeshl.ini -> RecoveryLauncher.cmd -> Recovery.exe -> probe preflight -> 原始 WinRE hash 恢复 -> wpeutil reboot -> Windows`；最终 `status.json` 为 `success`，注册 WinRE 与任务原始副本 SHA-256 均为 `0cbc86b44994065c7295f0322df670cf0b6c9e4a7be5099cfd962ddec956fda1`。这证明自动入口、同卷盘符复用和清理状态机；不证明 DISM Capture/Apply、格式化、BCDBoot、双系统或故障回滚。

2026-08-22 备份 fixture 实测：管理员准备任务 `53332658-7dd8-4684-9b19-372d9c680a95` 成功，但旧 `v0.3.6` Recovery 在 DISM Capture 输出为本地代码页时因 UTF-8 日志解析失败，任务为 `failed`，`Windows.wim.partial` 保留用于诊断。该问题已在 `v0.3.7` 修复，新的备份任务必须重新准备，不能复用失败任务。

同日交付产物：历史 ARM64 压缩包已统一移入项目 `.test-artifacts/desktop-archive/2026-08-22`，不再散落桌面。后续桌面只保留用户自己的文件；项目测试截图统一放在 `.test-artifacts/root-captures/`，不进入仓库根目录。

2026-08-22 隔离还原已完成当前包的直接实测：`v0.4.7` 备份任务 `ff6b645b-b9e4-4b4e-945a-1fb406923b0d` 的 Capture 成功，WIM SHA-256 为 `c08c4e7a9628ead708802ca880f46932a4cb6e0547f0cad735bf79ef29711b30`；还原任务 `821eb6af-f13c-46b2-8c1c-af1aa8345e42` 在 `S:` 完成 DiskPart 快速格式化、DISM Apply 和高完整性 BCDBoot，最终 `status.json` 为 `success`。独立 EFI `E:`（GUID `\\?\Volume{6ba9bc91-04dd-4105-9c46-7377ce26b862}\`）存在 `EFI\\Microsoft\\Boot\\bootmgfw.efi`、`EFI\\Boot\\bootaa64.efi` 和 BCD；管理员 `bcdedit /store E:\EFI\Microsoft\Boot\BCD /enum all /v` 返回 0，并列出 Windows Boot Manager 与 Windows 11 loader。该证据只覆盖隔离直接恢复，不宣称从该 EFI 实际重启进入系统。

同日根因记录：早期精简 fixture 使用 WinSxS 下 3224 字节的 `BCD-Template`，BCDBoot 依次暴露缺少 EFI_EX/BOOTRES、Fonts、bootstr 资源以及模板加载错误；使用管理员环境中实际的 `C:\Windows\System32\config\BCD-Template`（20480 字节）后，Capture/Apply/BCDBoot 全链路通过。测试 fixture 脚本已同步优先检查系统实际模板；该脚本位于被 `.gitignore` 忽略的本地测试目录，不作为产品 payload。

2026-08-22 GUI 收口：Rust `native_gui.rs` 接管 `BackupRestore.exe` 唯一桌面窗口，使用 Windows SDK Win32 API，不下载 GUI crate；窗口包含操作模式、任务/源/镜像/目标卷、镜像路径、WIM 索引、第二系统名称、环境刷新、镜像读取、最近任务状态刷新和管理员创建任务。默认值从当前系统及已挂载数据卷建议，所有盘符和镜像绝对路径在本地先校验；镜像卷与源卷在非 probe 模式下不得相同；环境和任务刷新会先显示进行中状态。`v0.4.6` ARM64 包已重建并启动烟测通过，窗口标题为 `BackupRestore - Rust GUI`；按钮交互、UAC、日志刷新仍待真实 VM UI 验收，不能用进程存活代替完整 GUI 验收。

2026-08-22 `v0.4.8` ARM64 构建收口：源码压缩包通过 Parallels 共享桌面传入 `C:\BackupRestoreBuild\source-v0.4.8`，未下载任何新工具链；使用已有 `aarch64-pc-windows-msvc` 工具链和 VM 本地 `C:\BackupRestoreBuild\target-v0.4.8` 构建成功。输出包为 `BackupRestore-windows-arm64-v0.4.8`，`build-manifest.json` 的 `binarySha256` 与 `BackupRestore.exe`、`Recovery.exe` 实际 SHA-256 均为 `526c612d8222920bf76f91e4fb4b04ff413cd555a7f9969f802cb6c0ca798050`；`Recovery.exe hash .\Recovery.exe` 返回 0，`BackupRestore.exe` 进程烟测窗口标题为 `BackupRestore - Rust GUI`。这些证据只覆盖 ARM64 构建、哈希和进程启动，不替代 WinRE、DISM、BCDBoot 或真实重启验收。

2026-08-23 `v0.5.0` 绝对镜像路径收口：GUI 和 PowerShell 参数改为 `ImagePath`，例如 `B:\BackupRestore\Windows.wim`；脚本从路径根解析镜像卷身份，任务 JSON 同时记录 `absolutePath` 和经校验的卷内路径，RecoveryTask.env 记录 `IMAGE_ABSOLUTE_PATH` 并在 WinRE 重新校验。原生 GUI 增加保存/打开文件对话框，备份使用保存对话框，还原使用打开对话框。ARM64 VM 已用既有工具链重建，`build-manifest.json`、`BackupRestore.exe`、`Recovery.exe` SHA-256 均为 `beb50a2f74852285f4508a3b1510ae9f5c2ab7f6dbd02f0b2ca852749ae01073`，`Recovery.exe hash` 返回 0；`BackupRestore.exe prepare -?` 已显示 `ImagePath` 且不再显示 `ImageDrive`/`ImageRelativePath`。这仍不替代真实备份/还原重启验收。

2026-08-23 `v0.5.2` GUI/WIM 收口：修复 WIM 多索引 JSON 数组分支的 Rust 借用错误，并同步两个 Cargo manifest、`Cargo.lock` 与 `VERSION` 到 `0.5.2`。ARM64 VM 使用已有工具链在浅层目录 `C:\BackupRestoreBuild\src`、`C:\BackupRestoreBuild\target`、`C:\BackupRestoreBuild\package` 构建成功，输出为 `BackupRestore-windows-arm64-v0.5.2`；没有重新下载工具链。`BackupRestore.exe` 继续使用 `WINDOWS_GUI` 子系统，WIM 索引下拉项同时展示序号和详细元数据。构建/哈希/启动证据不替代真实 WinRE、DISM、BCDBoot 或重启验收。
2026-08-24 `v0.5.4` probe 准备修复：修正镜像路径为空时的 `Test-Path` 和 `metadataPath` 生成逻辑，避免 `probe -NoReboot` 因空路径绑定错误失败；本机 AST、fmt、clippy、Rust 测试和 diff 检查均已通过，后续 VM 实测证据见下一条记录。
2026-08-24 VM 低负载实测补充：恢复挂起的 ARM64 VM 后，`probe -NoReboot` 任务 `760fcfe1-392d-4142-94af-b830f4286296` 成功，`validate-task`、manifest/payload 哈希和原始/注册 WinRE 哈希均通过；`recover --dry-run` 保持 `prepared`。容量不足的 backup 和未授权的 restore-existing 分支均按预期拒绝，未写入 `.partial`、未替换 WinRE、未格式化目标。测试完成后不执行真实重启，VM 可再次挂起以控制温度。
2026-08-24 `v0.5.6` GUI 改动：移除旧桌面前端，构建包只包含 Rust `BackupRestore.exe`、后端 PowerShell 和 WinRE 载荷；所有 PowerShell 查询隐藏运行。工作目录所在卷、源卷、目标卷改为详细下拉框，`probe` 创建任务固定使用 `-NoReboot`。VM 已重建 `v0.5.6` ARM64 包并验证 `validate-task`/`recover --dry-run`。
2026-08-24 `v0.5.7` 中文 UI 修复：中文模式操作项和语言标签纯中文；右侧说明与卷详情使用可换行多行控件，窗口重新分栏并扩大，避免文字裁剪/覆盖 WIM 区域。
2026-08-24 `v0.5.8` UI 结构修复：操作模式改为四个可点击标签按钮；完整任务/源/目标卷信息移到窗口底部三栏；全部控件显式使用系统默认 GUI 字体。ARM64 包待 Mac 解锁后补最终前台截图验收。

2026-08-23 `v0.5.3` WIM 权限回退：实测发现普通令牌下 `Get-WindowsImage` 可能返回可解析但没有索引的 JSON，GUI 因此不会触发管理员读取；现改为检测“索引为空”同样启动隐藏 `runas` 重试。ARM64 包已在 VM 重建并启动，`BackupRestore.exe`/`Recovery.exe` SHA-256 均为 `1e097ab33b01348db5515f41b25b56313f88f5b4ed9485e2782b7d4905ffc6dd`；点击“读取镜像”后真实下拉框显示索引 1（Windows Backup、162.9 MiB），操作模式提示分别解释当前系统覆盖还原和新增第二系统。该 GUI/UAC 读取证据不替代真实 WinRE、DISM、BCDBoot 或重启验收。

指定分区核实结论：代码和任务协议并不固定 C:。`BackupRestore.exe prepare` 的 `-SourceDrive`、`-TargetDrive` 接受任意单个盘符，镜像卷由用户填写的 `-ImagePath` 绝对路径根盘符解析；创建任务时会读取卷 GUID、磁盘/分区 GUID、偏移、容量、文件系统和序列号，Recovery 阶段按 GUID 复核后才把盘符作为临时挂载路径。`v0.4.7` 隔离实测实际使用非 C: 的 `S:` 源/目标卷、`B:` 镜像卷和独立 `E:` EFI，Capture、快速格式化、Apply-Image、BCDBoot 均成功；因此“不是只能备份 C:”已有实机证据。尚未完成的是从该独立 EFI 实际重启回恢复卷，以及 `create-secondary`、断电续跑和故障回滚，不能把“指定分区支持”外推成所有恢复场景均已验收。

## 5. 恢复工作时的唯一顺序

当前阶段已进入直接开发/构建，但不执行破坏性恢复操作。后续按以下顺序执行：

1. 每个独立大下载前运行：

   ```bash
   python3 ~/.codex/skills/pixian-dev-workflow/scripts/check_network.py
   ```

2. 检测为热点时，报告具体要下载的一个项目并等待授权；检测为 Wi-Fi/有线时，执行该下载。下载下一个大项目之前重新检测。
3. 保持 VM 构建目录浅层：源码使用 Parallels 共享桌面 `C:\Users\x\Desktop\BackupRestore`（与 macOS 工作区同步），Cargo target 使用 `C:\BackupRestoreBuild\target`，输出包使用 `C:\BackupRestoreBuild\package`；不要再复制到旧的 `C:\BackupRestoreBuild\src`，也不要按版本号创建多层 source/target/artifacts 目录。
4. 在 VM 中运行：

   ```powershell
   .\windows\build-windows.ps1 -Architecture arm64 -CargoTargetDir C:\BackupRestoreBuild\target -OutputRoot C:\BackupRestoreBuild\package
   ```

5. 自动 probe、P→H 备份和 P WIM→Q 第二系统还原均已在 ARM64 快照实测成功；后续任何影响 WinRE 路径的修改都要先重复 `probe -NoReboot` 与自动 probe。下一项是独立快照中的 BitLocker、身份不匹配、容量拒绝、BCDBoot 失败回滚和断电续跑故障注入。
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

2026-08-24 `v0.5.9` GUI 布局调整：移除右上角独立说明框。四个模式的提示统一显示在中部白色提示框；工作目录所在卷、源卷、目标卷恢复为纵向三段，每段的左侧标签说明用途，下拉框显示完整一行分区摘要，只读详情框紧随该下拉框。详情首段按当前模式明确说明该卷的用途、会发生的动作和限制，再显示卷标、文件系统、容量、磁盘/分区、分区类型与卷 GUID。字段按模式收紧：探测仅工作目录所在卷/源卷；备份增加镜像绝对路径；单系统还原增加目标卷和 WIM 索引；新增第二系统才增加启动项名称。窗口启动即最大化，语言选择器与操作模式处于同一行；所有多行说明文本统一写入 Windows `CRLF`，确保不依赖自动换行。ARM64 实机截图点检待本轮最终重建后补充。

2026-08-24 `v0.6.0` 最终 UI 点检：ARM64 VM 已用既有工具链构建并在用户 Console 会话前台最大化运行。下拉框保留完整分区摘要；工作目录所在卷、源卷和目标卷的说明框按显式 `CRLF` 逐行显示，且分别紧随自己的下拉框。语言选择器与操作模式位于同一行。实机包、哈希和截图仅证明本轮 GUI 布局/启动，不替代 WinRE、DISM、BCDBoot 或重启验收。

2026-08-24 `v0.6.1` 模式坐标修正和前台验收：实查发现探测页会隐藏 WIM 索引，因此 `v0.6.0` 截图不能证明还原页布局。已把 WIM 索引移动到镜像路径下方、提示框之后，与第二系统名称同一行；避免单系统还原/新增第二系统显示时覆盖中部提示框。ARM64 VM 已用既有工具链构建，`BackupRestore.exe` 在 Console 会话以最大化前台运行（PID `4080`），包位于 `C:\BackupRestoreBuild\package\BackupRestore-windows-arm64-v0.6.1`，二进制 SHA-256 为 `54d9fe3e50fabb4855089f1ec9f479b3164768fe88f07ce870a6a9bfbfcadb2c`。通过最小 Win32 `WM_COMMAND` 辅助程序逐一切换四个真实 GUI 标签：探测仅显示任务/源；备份显示任务/源和镜像；单系统还原显示任务/源/目标、镜像和 WIM 索引；新增第二系统再显示启动项名称。每页无控件覆盖，说明框按 `CRLF` 逐行显示；最终截图保存于 `.test-artifacts/root-captures/v0.6.1-ui-secondary-final.png`。这些证据只覆盖 GUI 布局、字段显隐、最大化启动和标签切换，不替代 WinRE、DISM、BCDBoot 或重启验收。

2026-08-25 `v0.6.2` UI 合理性修复：复核发现详情框高度不足，分区类型与卷 GUID 需要滚动才能看到；最大化后固定 1020×760 布局也留下明显右侧/底部空白。新增 `WM_SIZE`/客户区动态排版，任务、源、目标详情框扩展为完整多行高度，字段宽度随窗口增长，镜像/WIM/按钮区按当前模式重新定位。仍保持默认最大化和模式字段显隐。

2026-08-25 `v0.6.3` 标签切换排版修复：坐标实测发现 `v0.6.2` 切换到备份/还原后只更新显隐，没有重新调用排版，导致镜像路径可能与按钮同一行。现每次标签切换都立即按当前模式重排；此前的 `v0.6.2` 客体截图不作为最终布局证据。

2026-08-25 `v0.6.4` 垂直空间收口：逐项客体坐标检查发现第二系统页详情增高后底部镜像/按钮区接近任务栏；将详情高度调整为 120、行间距调整为 14，在保留完整卷身份行的同时为底部操作区留出空间。`v0.6.4` 需重新构建并截图确认按钮完整可见。

2026-08-25 `v0.6.5` 客户区高度收口：`v0.6.4` 客体截图仍显示第二系统页按钮被任务栏截断。详情高度调整为 112、行间距 8、状态框 64，并缩短模式间垂直间隔；保留完整卷身份行，进一步把镜像/WIM/启动项和按钮区移入客户区。

2026-08-25 `v0.6.5` 最终客体验收：ARM64 包 `C:\BackupRestoreBuild\package\BackupRestore-windows-arm64-v0.6.5` 已用既有工具链重建，`BackupRestore.exe` PID `9868`，SHA-256 `ddbedf79f00bf209453162f94433eab1c2d235ff014c1d8d400bfd0a577d0bd4`。通过 Windows Console 会话内的 Win32 坐标读取逐页检查：探测页按钮 y=577；备份页镜像 y=577、按钮 y=627；单系统还原/新增第二系统镜像 y=683、WIM/启动名 y=717、按钮 y=757；第二系统最终截图 `.test-artifacts/root-captures/v0.6.5-guest-secondary.png` 确认完整卷 GUID/分区 GUID、镜像路径和按钮均在客户区内，未被任务栏截断。所有客体验证均通过 `prlctl`/Windows 会话完成，没有把 Parallels 控制中心截图当作客体证据。

快照复核同日完成：当前仅保留 `before-v0.3.8-fixture-restore`（`c166ae4e-d976-494a-87dd-03b74de283af`）和当前 `before-v0.3.9-isolated-restore`（`973526bc-e905-4f38-9cf1-2d8037ffef5b`）。两者分别是隔离还原前基线和当前子快照，不属于无用快照；本轮未执行删除或回滚。

2026-08-25 `v0.6.6` WIM 读取修复：Windows 客体中 PowerShell `Get-WindowsImage` 对单索引 WIM 可能返回单对象而非数组，GUI 原先只解析数组分支，误报“WIM contains no selectable image indexes”。新增数组、单对象和 `images` 包装对象的统一解析；用 `B:\BackupRestore\Windows.wim` 的原始 JSON（索引 1、SHA-256 `c08c4e7a...`）复核根因，待新包重新点击“读取镜像”确认下拉框显示索引。

2026-08-25 `v0.6.7` WIM 诊断增强：`v0.6.6` 实机重新点击“读取镜像”仍显示无索引，未宣称修复成功。错误状态现在会把最多 4096 字节的原始索引 JSON 写入中部提示框，下一次客体复核可区分普通权限空结果、包装结构或解析错误。

2026-08-25 `v0.6.8` WIM DISM 回退：实机诊断确认普通 GUI 令牌下 `Get-WindowsImage` 返回 `[]`，而同一客体 `dism /English /Get-WimInfo /WimFile:B:\BackupRestore\Windows.wim` 返回索引 1。新增 DISM 文本解析回退（索引、名称、描述、大小），避免依赖隐藏 UAC 的 PowerShell JSON。待新 ARM64 包点击“读取镜像”确认下拉框显示索引 1。

2026-08-25 `v0.6.9` Windows 编译修复：`v0.6.8` ARM64 构建实际失败，原因是 `Option<u64>` 返回函数中对 `Result` 使用了 `?`；macOS 不编译 Windows 模块，因此离线检查未暴露。已改为 `.ok()?`，必须重新进行 ARM64 构建和 WIM 回退验收。

2026-08-25 `v0.7.0` DISM 回退诊断：`v0.6.9` ARM64 构建成功但 GUI 仍未显示索引，新增回退命令的 stdout/stderr 诊断；后续必须在 Windows ARM64 实机确认命令实际执行与解析结果，不能依据 macOS 检查或独立 PowerShell 命令替代。

2026-08-25 `v0.7.1` GUI 自提升：WIM 回退诊断显示 DISM 错误 740，而 VM UAC 策略是管理员自动提升、无安全桌面提示（`ConsentPromptBehaviorAdmin=0`）。`BackupRestore.exe` GUI 启动时现在读取令牌 elevation；非提升令牌使用 `ShellExecuteW("runas")` 自提升并退出，提升后的 GUI 才创建窗口。待 ARM64 实机确认高完整性令牌和 WIM 索引读取。

2026-08-25 `v0.7.2` 英文标签修复：英语模式原先直接使用内部操作全名，固定标签宽度导致文字裁剪。改为完整可见的简短 UI 名称 `Inspect / Backup / Restore / Second system`；详细行为仍显示在中部说明框，避免丢失语义。

2026-08-25 `v0.7.3` 英文纯净性修复：Windows ARM64 英语截图确认四个简短标签已完整显示，但语言标签仍为 `Language / 语言`。改为英语模式纯 `Language`；中文模式保持 `语言`。

2026-08-25 `v0.7.4` PowerShell 中文输出修复：中文“刷新任务状态”实机显示乱码，根因是 Windows PowerShell 5.1 原生输出代码页被 Rust 以 UTF-8 读取。`powershell_output` 现在在所有隐藏查询前显式设定无 BOM UTF-8 `Console.OutputEncoding` 和 `$OutputEncoding`；待 ARM64 实机重新刷新任务状态验证中文可读。

2026-08-25 `v0.7.4` ARM64 实机收口：使用 Windows ARM64 VM 重新构建并启动最新 GUI，管理员 GUI 读取 B 镜像成功，状态框显示镜像路径、WIM SHA-256 `c08c4e7a9628ead708802ca880f46932a4cb6e0547f0cad735bf79ef29711b30`、metadata SHA-256、最小目标容量和“已读取 1 个 WIM 索引”；单系统还原页下拉框显示 `Index 1 | Windows Backup`。英文模式截图确认四标签为 `Inspect / Backup / Restore / Second system`、语言标签纯 `Language`；中文模式刷新任务状态显示 `任务 ID/操作/任务目录/准备日志/恢复日志` 可读。最新前台进程为 `C:\BackupRestoreBuild\package\BackupRestore-windows-arm64-v0.7.4\BackupRestore.exe`（PID `2888`）。

2026-08-25 非 C 指定分区真实备份：复制的 Windows fixture 挂载为 U:（Disk 3 Partition 3，约 8 GiB），工作目录所在卷 B:，独立镜像卷 T:（Disk 3 Partition 2）。任务 `bc1660b8-6863-495d-b333-cd168f9a5c41` 完成 `Windows -> WinRE -> DISM Capture -> 原始 WinRE hash 恢复 -> Windows`，最终 `status.json=success`；WIM 1.63 GB，metadata/hash 一致。该证据不涉及 C:。

2026-08-25 多索引 WIM：从非 C 备份 WIM 导出两个索引到 `T:\BackupRestore\tests\non-c-u-multi\Windows.wim`，DISM `/Get-WimInfo` 显示 Index 1/2，两个名称分别为 `Windows Backup Index 1/2`。首次 `restore-existing` 未完成，根因是任务自动选择系统 EFI（任务 env GUID `d08d...`）而独立测试 EFI E: GUID 为 `6ba9...`，Recovery 在 WinRE 进入前发生 EFI 身份冲突；U: 未被格式化，C: 未触碰。

2026-08-25 EFI 选择缺口修复：`BackupRestore.exe prepare` 新增可选 `-EfiDrive`，默认仍选择系统启动盘 EFI；隔离多磁盘测试可显式指定独立 EFI（例如 E:），避免把生产 EFI 与测试 EFI 混淆。v0.7.6 ARM64 实机已用该参数完成 Index 2 还原。

2026-08-25 Recovery EFI 冲突修复：Index 2 还原的 WinRE 日志显示镜像卷被复用为 E:，而 EFI 也硬编码偏好 E:，导致 EFI 身份冲突、目标 U: 未格式化。Rust Recovery 现以 Z: 作为 EFI 临时挂载偏好，并让 BCD 回滚使用实际 EFI 根路径；生产默认 EFI 选择不变。v0.7.6 ARM64 实机重试成功。

2026-08-25 多索引 Index 2 真实还原成功：任务 `375f4422-7c17-4397-9560-6c83d7ca9ff4` 使用 v0.7.6 Recovery、`-EfiDrive E`、T: 多索引 WIM Index 2、U: 目标分区，完成快速格式化、DISM Apply-Image、`bcdboot F:\Windows /s Z:\ /f UEFI`、原始 WinRE 恢复和自动返回 Windows；最终 `status.json=success`。U: 目标 SYSTEM 与 fixture 源 SYSTEM hive SHA-256 均为 `A70A0D2750D67E0D3B4054C9284F5794B2A699CC3A76203B7297CD3EAF6CC550`；E: BCD、`bootmgfw.efi`、`bootaa64.efi` 均存在。该证据证明指定分区和多索引 Apply/BCDBoot，不证明从 E: 实际引导 U:。

2026-08-25 `v0.7.9` 工作目录架构收口：删除 GUI 中的工作目录卷选择、详情栏和旧任务盘参数。程序目录与 Rust Recovery 均从 `BackupRestore.exe` 所在目录推导 `<程序目录>\tasks\<任务 ID>`、日志、WinRE 载荷、状态和 BCD 快照；任务记录工作目录卷 GUID、磁盘/分区身份及相对路径，WinRE 按这些身份重新挂载后读取任务。还原创建前按卷 GUID阻止“程序目录所在卷 = 还原目标”，阻止路径只显示单按钮弹窗，不创建任务、不修改 WinRE/BCD、不请求重启；备份允许程序目录与源卷相同。镜像路径仍要求用户选择绝对路径，WIM 索引仍通过下拉框展示完整索引信息。

2026-08-26 v0.9.8 安全边界补强：prepare 在任何 BCD/WinRE 写入前完整验证 Task；卷身份包含卷序列号并在 WinRE 挂载后逐字段核对；目标格式化后重新核验分区几何，防止盘符复用或分区替换。最新 ARM64 包来自桌面共享软链接源码，标题显示版本号，控件悬停提示已接入。

2026-08-26 v0.9.9 审计收口：任务验证与 BCD 快照顺序已修正为两阶段，先无副作用验证任务，再导出/哈希 BCD，最后启用并验证一次性启动计划；避免把合法任务误判为缺少 BCD 回滚信息。

2026-08-26 v1.0.0 事务与身份审计修复：已挂载卷不再因卷 GUID 命中而跳过完整身份复核；WinRE 盘符分配改用 X: 临时盘并等待 DiskPart 正常退出；RecoveryTask.env 与载荷 task.json 改为严格绑定；准备阶段在替换注册 WinRE 前先持久化 manifest，失败时恢复原始 WinRE 和 BCD 快照；BCDBoot 后必须确认 loader 的 device/osdevice 指向目标分区；探测/备份不再要求隐藏目标卷。

2026-08-27 v1.0.1 Recovery 分区定位修复：不再忽略 DiskPart 失败后读取固定 `R:`。准备阶段先扫描已挂载卷的实际磁盘/分区身份，再尝试多个空闲盘符并核对卷/GPT GUID、分区类型、文件系统、磁盘号、分区号、偏移和容量；`R:` 被占用或分配异常时安全失败，避免误把其他卷当作 Recovery。

2026-08-27 v1.0.2 隐藏 EFI 定位修复：默认系统 EFI 通常没有盘符，准备阶段先复用已挂载 EFI，否则通过 `mountvol /S` 临时挂载到空闲盘符，读取完整 GPT 身份后立即卸载；挂载失败、类型不符或所有盘符占用时安全失败。ARM64 VM 已从共享桌面源码重新构建 v1.0.2，manifest 与 `BackupRestore.exe`/`Recovery.exe` SHA-256 一致，标题显示 v1.0.2，GUI 已以前台最大化运行。

2026-08-27 v1.0.3 空闲盘符判定修复：`mountvol <letter>: /L` 对未分配盘符返回退出码 1，旧逻辑因此跳过所有临时盘符。现将无挂载点视为空闲，再由 `mountvol /S` 和完整 GPT 身份校验确认；其它命令异常仍安全失败。

2026-08-27 v1.0.4 多语言 tooltip 修复：悬停提示按当前语言生成，中文模式不再显示拼接的英文；切换语言时重建 tooltip，防止旧语言文本残留。ARM64 包已重建，`build-manifest.json`、`BackupRestore.exe` 与 `Recovery.exe` 的 SHA-256 均为 `88399f72a55b7af59ed85173d13a27f60fc4dc18c14de502dcf7c801d322115f`，窗口标题为 `BackupRestore - Rust GUI v1.0.4`。客体画面已确认新窗口前台最大化；真实鼠标悬停弹出框仍未取得可靠截图证据。

2026-08-27 v1.0.5 WIM 多索引元数据：DISM 文本回退解析器按索引保存名称、描述、大小及可选的版本、架构、版本类型和安装类型，支持逗号分隔字节数与常见二进制单位；头部工具版本不会泄漏到镜像条目。Windows ARM64 目标全量测试 8 项 CLI 与 16 项核心测试通过；发行包 `C:\BackupRestoreBuild\package\BackupRestore-windows-arm64-v1.0.5` 的两个二进制及 manifest SHA-256 均为 `5a1fd2b97c358d31ba6dd3d93b088edab1cdd051e5db1cd3e651aade5008af7e`。结束旧实例后最新 GUI 已以前台最大化运行，标题为 `BackupRestore - Rust GUI v1.0.5`，截图 `.test-artifacts/root-captures/v1.0.5-gui.png`；真实 `D:\sources\boot.wim` 的 `wim-info` 已返回索引 1、描述和大小。

2026-08-27 v1.0.6 GUI 文本读取修复：真实多索引 GUI 点检发现跨进程设置的镜像路径可见，但 `WM_GETTEXTLENGTH` 返回 0，读取镜像因此错误提示路径为空。`get_text` 改用 `GetWindowTextLengthW/GetWindowTextW`，避免该消息边界。测试期间在 `U:` 同一源卷两次捕获生成 `V:\multi-index-same-source-v1.0.5.wim`，DISM 返回索引 1/2；C: 未触碰。

2026-08-27 v1.0.7 GUI 文本读取兜底：实测发现部分控件的 `GetWindowTextLengthW` 也可能返回 0，但 `GetWindowTextW` 仍能返回可见文本。`get_text` 在长度为 0 时使用 32 KiB 有界缓冲并按实际写入长度截取，避免合法镜像路径被误判为空；需用 v1.0.7 ARM64 包重新完成多索引读取按钮和下拉截图。

2026-08-27 v1.0.8 GUI 文本读取三层回退：在 `GetWindowTextLengthW/GetWindowTextW` 有界读取后，若控件仍返回 0，再使用受限的 `WM_GETTEXTLENGTH/WM_GETTEXT` 读取，避免 Windows 完整性/线程边界导致合法绝对路径被判为空。测试驱动同时修正了高 DPI 坐标和残留模态框干扰；`U:` 同一源卷生成的 `V:\multi-index-same-source-v1.0.5.wim` 仍保留为双索引 GUI 验收输入，C: 未触碰。ARM64 新包需在 VM 中完成最终“读取镜像”按钮和双索引下拉截图，未取得前不宣称 GUI 多索引验收完成。

2026-08-27 v1.0.8 ARM64 构建回归：从桌面共享源码使用既有 `aarch64-pc-windows-msvc` 工具链成功构建 `C:\BackupRestoreBuild\package\BackupRestore-windows-arm64-v1.0.8`；`BackupRestore.exe`、`Recovery.exe` 与 `build-manifest.json` 的 SHA-256 均为 `8a67df2c833ff0a4b20501e5446273f5ee7af4df015c8ed3a78046acba8744c1`。Windows ARM64 `cargo test --workspace --all-targets --offline` 通过（CLI 8 项、core 16 项），最新 GUI 已结束旧实例并以前台最大化运行，标题 `BackupRestore - Rust GUI v1.0.8`。提升权限的 `wim-info` 实读 `V:\multi-index-same-source-v1.0.5.wim` 返回索引 1/2（Same source capture 1/2，描述和大小均存在），证明同源双索引输入和解析链路；截图保存于 `.test-artifacts/root-captures/v1.0.8-fresh.png`。由于客体真实输入驱动仍无法稳定穿透高 DPI/模态窗口，GUI 双索引下拉最终截图继续标记为待验证，不把 CLI 结果冒充 GUI 证据。

本轮 ARM64 客体验证边界：v1.0.0 已从共享桌面源码重建并通过 ARM64 编译；仍需在新快照中实际停留鼠标确认 tooltip，并重新执行非 C probe/备份/还原链路。不得把旧版本包的截图当作本轮证据。

2026-08-25 系统性架构审计与 Rust 准备迁移：发现并修复“Rust GUI 仍调用 PowerShell 准备/查询”和“Recovery.cmd 保留破坏性回退”两项 P0 根因。新增 Rust `prepare`、`list-volumes`、`inspect-environment`、`wim-info`；GUI 改为调用 Rust 自身 CLI；产品包删除 `BackupRestore.ps1` 和破坏性 `Recovery.cmd`；WinRE 启动器缺少 `Recovery.exe` 时直接失败。Rust 原生卷枚举通过 `DeviceIoControl`/`GetVolumeInformationW`/`GetDiskFreeSpaceExW` 读取 GPT 类型、卷 GUID、磁盘/分区身份和容量，ARM64 实测只返回 C/T/U 普通卷，EFI/Recovery 被排除。Rust `prepare --operation probe --no-reboot`、`validate-task`、`recover --dry-run` 已在 ARM64 通过；Rust 还原 C: 的同卷阻止已验证。自动重启进入 WinRE 的 Rust prepare 链路仍待下一轮非 C 快照复验。

2026-08-25 `v0.8.0` Rust 自动 probe 实测：在快照 `deda1db6-830b-4fdf-9228-e5c8e39c0aa9` 上，从 `T:\BRRustV080` 运行 Rust `BackupRestore.exe prepare --operation probe --source-drive C --target-drive C`，任务 `dcff7126-aa6b-4a5b-910c-d5acbbcbdebe` 完成 Windows → WinRE → `Recovery.exe recover-env` → 原始 WinRE 恢复并校验 → Windows。最终 `status.json=success`；`Recovery.log` 记录 Rust Recovery 启动、probe 验证、`Original registered WinRE restored and verified` 及 `wpeutil.exe reboot`。Windows 返回后注册 WinRE SHA-256 仍为 `0CBC86B44994065C7295F0322DF670CF0B6C9E4A7BE5099CFD962DDEC956FDA1`。全程未备份、还原或格式化 C:。

2026-08-25 `v0.8.0` Rust 真实非 C 备份：任务 `3a6f0d0f-9843-49b8-9841-4a11a38c25d7` 从 U: Capture 到 B: `RustCapture-v081.wim`，完成 Windows → WinRE → DISM Capture → metadata/hash → 原始 WinRE 恢复 → Windows，最终 `status.json=success`；WIM 大小 1,631,193,711 字节，SHA-256 `4ae49a37f1e66f4f749d538e067a51de2a0525b374993b3c4573a42bf14cbb07`。该次暴露 metadata 统计字段为 0，已在 v0.8.1 修复并以 `--no-reboot` env 验证非零统计字段。C: 未触碰。

2026-08-25 `v0.8.2` Rust 多索引还原实测：任务 `fcfdd192-61e0-4b14-b04f-9734dcd26e48` 使用 B: 程序工作目录、T: 多索引 WIM 的 Index 2、U: 唯一格式化目标和独立 EFI E:，完成 Windows → WinRE → DiskPart 格式化 U: → DISM `/Index:2` Apply → `bcdboot F:\Windows /s Z:\ /f UEFI` → 原始 WinRE 恢复 → Windows。最终 `status.json=success`；U:\Windows\System32\config\SYSTEM SHA-256 为 `A70A0D2750D67E0D3B4054C9284F5794B2A699CC3A76203B7297CD3EAF6CC550`，独立 E: BCD 默认 loader 的 `device/osdevice=partition=U:`，`bootmgfw.efi` 存在。C: 未触碰。独立 EFI 从 Parallels 固件实际启动仍保留为单独未解决项。

2026-08-25 独立 EFI 限定为开发测试：普通 GUI 不显示 EFI 选择，也不向 Rust prepare 传入覆盖参数；正常流程只查找真实 GPT EFI 分区。仅测试 CLI 支持 `--test-efi-drive <盘符>`，用于隔离 E: EFI/跨磁盘启动诊断。关闭 Secure Boot 后 hdd2 首启动仍为 `0xc0430001`；把 E: `bootmgfw.efi` 替换为与 U: 完全相同的版本后错误不变。因此已排除“仅 Secure Boot 开关”与“独立 EFI bootmgfw 版本不同”两种单一原因，独立 EFI 固件启动不作为产品功能或 V1 发布门槛。

2026-08-26 工作目录相对路径边界修复：WinRE 解析 `WORKSPACE_ROOT_REL` 时按最后一个 `\\tasks\\` 拆分，允许程序目录本身名为 `tasks`（例如 `D:\\tasks`），同时继续拒绝路径穿越和任务 ID 不匹配。新增 CLI 回归测试覆盖嵌套 `tasks` 目录；任务、日志、状态和载荷仍全部写入当前程序目录，不恢复 `C:\\ProgramData\\BackupRestore`。
2026-08-26 客体旧测试数据清理：确认 `C:\\BackupRestore\\tasks` 的 48 个历史任务目录造成约 61.8 GiB 占用；先卸载两个 DISM 孤儿挂载，再删除该项目明确生成的旧任务目录。C: 已用空间从约 140.4 GiB 降至 78.6 GiB。新增 Rust 终态任务 artifact 清理，保留最近 3 个终态任务的元数据与日志，未完成/异常/仍挂载任务不自动删除。

2026-08-27 v1.2.5 完整 P/H/Q 验收：ARM64 VM 仅使用 P（源/单系统目标）、H（WIM）和 Q（第二系统目标），C: 未作为备份源或还原目标。任务 `89439a72-9fc6-41be-ae49-18be36e9a850` 备份 P 为 Index 1，`b4fec5e1-7d02-4aad-8bc3-ee7959f3e0ef` 将同一源追加为 Index 2；任务 `15ce570a-9aff-4697-b930-ef19e35a2dfc` 以 Index 2 还原 P，任务 `bb73da54-80e3-4b7b-9550-0ac24901aab2`/`8a590f81-7777-4074-915e-e7dd5dec2607` 分别以 Index 1/2 还原 Q。所有任务 `status.json=success`，原始 WinRE 均恢复校验后才写成功；P/Q 与备份前清单排除系统自动生成项后均为 212 个 payload、SHA-256/大小/相对路径差异 0。

2026-08-27/28 v1.3.0 收口核验：Windows ARM64 客体使用最新 Rust 包完成 P→H 备份（任务 `5f2d2262-80c1-43c8-b839-8b2544effedf` 首次生成索引 1，`fcad95ce-9c46-4e74-8302-452fbda7992f` 追加生成索引 2，`31971c01-62b3-47ff-a01f-22dddb2f9541` 再追加生成索引 3），同一 WIM `H:\Images\FullCycle-v1.2.9.wim` 的 3 个索引均由 DISM 实读确认。索引 1/2 第二系统恢复到 Q: 的任务为 `21ecf1b9-72ce-4a65-b279-459880edcb88`、`eac6910f-8387-478b-babf-935135d78d8a`，最终 v1.3.0 包索引 3 恢复到 Q: 的任务为 `8f5ca68c-a7f5-410d-8cfc-2791f33a389d`；单系统索引 2 恢复回 P: 的任务为 `312a7cc0-5c15-4d29-9e7b-3ead67644a16`，最终 v1.3.0 包索引 3 恢复回 P: 的任务为 `8538cc29-79fe-41d3-9de2-8458e3bfaa48`。上述恢复任务均在 WinRE 完成并最终 `success`。P: 还原前后、Q: 各索引还原后的五个 fixture 文件路径与 SHA-256 完全一致，`Compare-Object` 无差异；恢复后 `reagentc /info` 为 Enabled，DISM 无挂载 WIM。最终包 1.3.0 的 ARM64 本地测试为 CLI 11 项 + core 17 项，manifest 与两个 EXE SHA-256 均为 `ec33fd6fd3fc42433a5fdfaea9db02044dace09c13887925adc65e6cbf4bbc20`，前台 Computer Use 截图显示标题 `BackupRestore - Rust GUI v1.3.0`。标题代码位于 `native_gui.rs` 的 `CreateWindowExW`，格式固定为 `BackupRestore - Rust GUI v{PROGRAM_VERSION}`；宿主侧跨完整性进程读取 `MainWindowTitle` 为空不作为标题缺失证据。
同轮最终包故障安全复核：任务 `d557b7b7-7435-432a-a3f9-45b38ffd2588` 通过开发故障 `identity-env-mismatch` 在 WinRE 预检阶段失败，未执行 DISM、格式化或 BCDBoot；恢复后 `reagentc /info` 仍为 Enabled，`dism /Get-MountedWimInfo` 无挂载镜像。
同轮断电续跑复核：最终包任务 `3dd7a348-9190-4e61-90d0-76ecf1fd45cf` 先持久化 `boot-requested` 并故意不请求重启；再次启动 GUI 后 `prepare.log` 记录检测唯一合法待恢复任务、重新请求一次性 WinRE 启动并重启，任务随后在 WinRE 完成 Q: 恢复并最终 `success`。

2026-08-26 v0.9.3 环境摘要编码修复：Windows `ver` 输出受系统代码页影响，旧 GUI 刷新环境时曾把“版本”中文解码成乱码。Rust 现在只提取不受本地化影响的 ASCII 版本号（例如 `10.0.26200.9168`），并新增回归测试；ARM64 v0.9.3 已重新编译、结束旧进程后以前台窗口运行。若系统命令没有可识别版本号，界面明确显示 `Windows version unavailable`，不会显示乱码。
2026-08-26 Windows 系统空间审计：DISM 报告 WinSxS 实际 19.75 GiB、7 个可回收包并建议清理；C: 卷影副本配额已用约 4.08 GiB。未删除 WinSxS 或 `System Volume Information`，避免丢失更新回滚/系统还原能力；清理需用户确认具体范围。
2026-08-26 v0.9.5 清理安全复核：自动清理在删除大型 WinRE 目录前新增完整任务模型与 operation 一致性校验；格式异常或字段缺失的任务永不自动删除。
2026-08-26 v0.9.7 逻辑漏洞修复：WinRE 每次挂载后复核完整 GPT/卷身份和几何信息，格式化后再次复核目标仍是原分区；异常退出守卫使用 Recovery 实际盘符；BCDBoot 仅允许匹配目标分区的 loader；同卷阻止在 UAC 前执行。Rust Win32 GUI 标题显示版本号，并为操作标签、卷下拉框、镜像路径、WIM 索引和操作按钮增加悬停说明。

2026-08-26 系统恢复数据清理（用户已授权）：已删除 C: 上全部 4 个卷影副本，并完成普通 DISM 组件清理；WinSxS 实际占用由约 19.75 GiB 降至约 12.60 GiB，7 个可回收包先清理 5 个。`/StartComponentCleanup /ResetBase` 已退出码 0 完成，之后再次运行普通清理成功。最终 AnalyzeComponentStore 为实际 12.53 GiB，仍报告 2 个可回收项；逐项检查显示它们属于 staged 按需功能/语言包，未手工删除，避免破坏可选功能。`vssadmin list shadows /for=C:` 无卷影副本，C: 可用空间约 181.3 GiB。CheckHealth 与 ScanHealth 仍报告组件存储可修复；未执行 `RestoreHealth`，因为它可能需要下载源文件且当前网络是热点。ResetBase 已永久丢弃旧更新回滚基线，不能卸载已纳入基线的更新。

2026-08-27 v1.0.9 版本一致性防线：根 `VERSION=1.0.9` 与两个 Cargo manifest 停留在 `1.0.8` 会造成包目录和 Rust GUI 标题不一致。`windows/build-windows.ps1` 现会在编译前拒绝该错配。Windows ARM64 用既有工具链重建 `C:\BackupRestoreBuild\package-staging\BackupRestore-windows-arm64-v1.0.9`；真实前台标题为 `BackupRestore - Rust GUI v1.0.9`，已自动读取 `C:\BackupRestoreBuild\multi-index-gui-v1.0.8.wim` 的 2 个索引，截图为 `.test-artifacts/root-captures/v1.0.9-version-consistent.png`。Parallels 辅助功能树只暴露旧 medium-token 桌面，和 `prlctl capture` 的高权限前台桌面不同，不能把它的操作冒充前台下拉点检。

2026-08-27 v1.1.0 GUI 进程收敛：提升后的 Rust GUI 在创建窗口前向同一交互桌面上的旧 `BackupRestoreNativeGui` 正常投递 `WM_CLOSE`，不强杀进程。ARM64 实测由 6 个历史 GUI 收敛为唯一 v1.1.0 进程（窗口标题 `BackupRestore - Rust GUI v1.1.0`），仍保持 WIM 双索引读取结果。随后删除 `C:\BackupRestoreBuild` 下 v0.9.3--v1.0.9 旧发行包、`package-next`、`package-staging`、旧 `src` 副本和 `ui-test`；仅保留 `package\BackupRestore-windows-arm64-v1.1.0` 与离线编译缓存 `target`。`prlctl capture` 对该提升后窗口返回黑帧，但 macOS 前台虚拟机画面和 Windows `MainWindowTitle/Responding` 均确认窗口正常，黑帧不能当作客体黑屏。

2026-08-27 v1.1.1 WIM 索引完整显示：修复单系统还原页 WIM 索引控件仅 440 像素、名称和描述被裁剪的问题。单系统还原页现在占满可用行宽；第二系统页保持菜单名称输入空间，但两种页面的展开列表都设置为可用全宽。Windows ARM64 已重新编译并启动唯一响应中的 `BackupRestore - Rust GUI v1.1.1`；旧 v1.1.0 包已删除，`C:\BackupRestoreBuild\package` 仅保留 v1.1.1。Parallels 当前前台截帧仍可能滞留快速助手的历史帧，不能用它判断 GUI 内容；真实进程标题与响应状态已复核。
