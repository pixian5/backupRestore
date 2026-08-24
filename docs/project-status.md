# BackupRestore 当前进度与决策记录

更新时间：2026-08-24
当前开发版本：`0.5.5`
分支：`main`
本地开发基线：以当前 `HEAD` 为准

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

## 1.1 用户期望流程对照

| 用户期望 | 当前实现 | 结论 |
|---|---|---|
| 选择镜像保存位置、选择要备份的分区，重启进入 WinRE，由程序执行备份 | GUI/PowerShell 传入用户选择的 `ImagePath` 绝对路径和 `SourceDrive`；准备阶段注入 `Recovery.exe`、`RecoveryTask.env`、`task.json` 到专用 WinRE，默认会调用 `reagentc /boottore` 后重启；WinRE `recover-env` 按 GUID 挂载源/镜像卷并执行 DISM Capture，写入 `.partial`、metadata 和 SHA-256 | 流程已覆盖；自动 WinRE probe 已实机验证，非 C: 的 `S:`/`B:` Capture 已实机验证 |
| 选择镜像位置、选择恢复目标分区，重启进入 WinRE，由程序执行还原 | GUI/PowerShell 传入用户选择的 `ImagePath` 绝对路径、`WimIndex` 和目标；准备阶段记录绝对路径并同时保存受 GUID 保护的卷内路径，WinRE 按 GUID 挂载镜像/目标，校验 metadata/hash，格式化明确目标、DISM Apply、BCDBoot，再恢复原 WinRE 并重启 | 流程已覆盖；非 C: 的 `S:`/`B:`/`E:` 隔离 Apply/BCDBoot 已实机验证，但从独立 EFI 实际重启回恢复系统仍待验证 |
| 还原任意指定分区 | `restore-existing` 为安全单系统模式，要求目标与选定源分区相同；`create-secondary` 才允许选择不同 NTFS 目标并使用 `/addlast` | 代码已覆盖，双系统/不同目标的完整重启验收待验证 |

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

网络状态不是永久属性，本文不把某次检测结果当作后续下载授权。恢复下载工作前必须重新检测。

## 3. 当前完成度

### 已完成的离线实现

| 范畴 | 已实现内容 | 证据状态 |
|---|---|---|
| 任务与安全模型 | UUID 任务目录、原子 JSON、状态机、卷完整身份、相对路径限制、EFI/MSR/Recovery 排除、容量和 BitLocker 拒绝规则。 | Rust 单元测试已覆盖关键规则 |
| 任务卷防替换 | 新任务记录 `taskVolume`；WinRE 复核任务、源、镜像和目标的 GUID、分区位置、大小、类型、文件系统与序列号。 | 代码与离线测试已覆盖 |
| WinRE 载荷完整性 | 原始/暂存 WinRE、任务专用 payload manifest、每个载荷的 SHA-256、`RecoveryTask.env` 二次 hash 校验。 | 代码已覆盖 |
| 正常 Windows 准备 | 环境、UEFI/GPT、WinRE、Secure Boot、BitLocker 检查；注册 Recovery 分区定位；EFI 选择；WIM 注入；BCD 快照；`last-task.json`。 | PowerShell AST 已通过 |
| WinRE 恢复 | 固定盘符重新挂载、镜像和 metadata 校验、DISM Capture/Apply、BCDBoot、阶段续跑、BCD 失败回滚、原始 WinRE 清理守卫。 | probe 自动入口已实机完成；隔离测试卷上的 Capture、Apply、格式化和独立 EFI BCDBoot 已实机完成；真实恢复卷启动回归仍待验证 |
| 成功状态一致性 | WinRE 只有在原始注册 `Winre.wim` 恢复并通过 SHA-256 校验后才写入 `success`；probe 支持 `preflight -> success`；清理失败写入 `failed` 并保留恢复日志。 | probe 实机已验收 |
| probe 同卷情形 | `recover-env` 按卷 GUID 复用已有盘符；同卷源使用任务卷实际盘符。真实备份/还原仍要求任务卷与源/目标独立。 | Win11 ARM64 probe 实机已验收 |
| GUI | Rust Win32 原生单窗口负责操作模式、任务/源/镜像/目标卷、WIM 索引、镜像读取、环境和最近任务状态刷新、盘符校验、破坏性确认和管理员准备启动；支持中文/English 切换；镜像使用绝对路径；旧 PowerShell/WPF 页面保留兼容但不再是默认入口。WIM 索引使用下拉框，条目显示序号、名称、描述、版本、架构、Edition、安装类型和大小；非管理员返回空索引时会自动重试隐藏管理员读取。 | `v0.5.3` ARM64 包已在 VM 重建并真实启动 GUI；点击“读取镜像”后下拉框显示 `索引 1 | Windows Backup | ... | 大小 162.9 MiB`，单系统/第二系统模式说明均在窗口显示。UAC/WinRE 仍按证据矩阵验收 |
| 构建与交付结构 | `build-windows.ps1` 生成架构隔离包，`BackupRestore.exe` 启动 GUI，`Recovery.exe` 作为 WinRE 主机，并随包携带 ARM64 MSVC runtime。 | `v0.4.8` ARM64 包已在 VM 使用既有工具链离线构建，manifest 与二进制 SHA-256 一致；`v0.4.7` 的管理员 GUI/UAC、Capture、Apply、格式化和独立 EFI BCDBoot 实测证据仍保留 |
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

当前结果：Rust CLI/core 共 17 项测试通过；Clippy 无 warning；PowerShell 语法检查通过；差异无空白错误。

## 4. 明确未完成的实机验收

以下项目均不能因代码、AST 或 macOS 测试通过而标记完成：

1. 在独立测试卷上验证过的 Capture/Apply 还需要在真实恢复任务进入 WinRE 后再做一次完整回归；
2. `create-secondary`、`/addlast`、真实恢复卷从独立 EFI 启动、原 WinRE hash 恢复和正常 Windows 回归仍未完成；
3. 验证错误边界：BitLocker 拒绝、卷身份不匹配拒绝、目标容量不足拒绝、DISM/BCDBoot 失败后的状态和 BCD 回滚、每个阶段的断电续跑；
4. 在真实 Rust Win32 窗口核验环境刷新、卷默认值、镜像读取、任务状态刷新、确认对话框、长路径和 UAC 行为。

2026-08-21 已实测：`Windows 11` ARM64 VM 使用 `v0.3.6` 完成真实自动 probe，任务为 `c12026c0-6a9e-4093-8a8b-2971968a31f7`。流程完成 `Windows -> reagentc /boottore -> WinRE -> winpeshl.ini -> RecoveryLauncher.cmd -> Recovery.exe -> probe preflight -> 原始 WinRE hash 恢复 -> wpeutil reboot -> Windows`；最终 `status.json` 为 `success`，注册 WinRE 与任务原始副本 SHA-256 均为 `0cbc86b44994065c7295f0322df670cf0b6c9e4a7be5099cfd962ddec956fda1`。这证明自动入口、同卷盘符复用和清理状态机；不证明 DISM Capture/Apply、格式化、BCDBoot、双系统或故障回滚。

2026-08-22 备份 fixture 实测：管理员准备任务 `53332658-7dd8-4684-9b19-372d9c680a95` 成功，但旧 `v0.3.6` Recovery 在 DISM Capture 输出为本地代码页时因 UTF-8 日志解析失败，任务为 `failed`，`Windows.wim.partial` 保留用于诊断。该问题已在 `v0.3.7` 修复，新的备份任务必须重新准备，不能复用失败任务。

同日交付产物：历史 ARM64 压缩包已统一移入项目 `.test-artifacts/desktop-archive/2026-08-22`，不再散落桌面。后续桌面只保留用户自己的文件；项目测试截图统一放在 `.test-artifacts/root-captures/`，不进入仓库根目录。

2026-08-22 隔离还原已完成当前包的直接实测：`v0.4.7` 备份任务 `ff6b645b-b9e4-4b4e-945a-1fb406923b0d` 的 Capture 成功，WIM SHA-256 为 `c08c4e7a9628ead708802ca880f46932a4cb6e0547f0cad735bf79ef29711b30`；还原任务 `821eb6af-f13c-46b2-8c1c-af1aa8345e42` 在 `S:` 完成 DiskPart 快速格式化、DISM Apply 和高完整性 BCDBoot，最终 `status.json` 为 `success`。独立 EFI `E:`（GUID `\\?\Volume{6ba9bc91-04dd-4105-9c46-7377ce26b862}\`）存在 `EFI\\Microsoft\\Boot\\bootmgfw.efi`、`EFI\\Boot\\bootaa64.efi` 和 BCD；管理员 `bcdedit /store E:\EFI\Microsoft\Boot\BCD /enum all /v` 返回 0，并列出 Windows Boot Manager 与 Windows 11 loader。该证据只覆盖隔离直接恢复，不宣称从该 EFI 实际重启进入系统。

同日根因记录：早期精简 fixture 使用 WinSxS 下 3224 字节的 `BCD-Template`，BCDBoot 依次暴露缺少 EFI_EX/BOOTRES、Fonts、bootstr 资源以及模板加载错误；使用管理员环境中实际的 `C:\Windows\System32\config\BCD-Template`（20480 字节）后，Capture/Apply/BCDBoot 全链路通过。测试 fixture 脚本已同步优先检查系统实际模板；该脚本位于被 `.gitignore` 忽略的本地测试目录，不作为产品 payload。

2026-08-22 GUI 收口：Rust `native_gui.rs` 接管 `BackupRestore.exe` 默认窗口，使用 Windows SDK Win32 API，不下载 GUI crate；窗口包含操作模式、任务/源/镜像/目标卷、镜像路径、WIM 索引、第二系统名称、环境刷新、镜像读取、最近任务状态刷新和管理员创建任务。默认值从当前系统及已挂载 NTFS 卷建议，所有盘符和镜像相对路径在本地先校验；镜像卷与源卷在非 probe 模式下不得相同；环境和任务刷新会先显示进行中状态。旧 `BackupRestore.Gui.ps1` 仅作为兼容前端保留。`v0.4.6` ARM64 包已重建并启动烟测通过，窗口标题为 `BackupRestore - Rust GUI`；按钮交互、UAC、日志刷新仍待真实 VM UI 验收，不能用进程存活代替完整 GUI 验收。

2026-08-22 `v0.4.8` ARM64 构建收口：源码压缩包通过 Parallels 共享桌面传入 `C:\BackupRestoreBuild\source-v0.4.8`，未下载任何新工具链；使用已有 `aarch64-pc-windows-msvc` 工具链和 VM 本地 `C:\BackupRestoreBuild\target-v0.4.8` 构建成功。输出包为 `BackupRestore-windows-arm64-v0.4.8`，`build-manifest.json` 的 `binarySha256` 与 `BackupRestore.exe`、`Recovery.exe` 实际 SHA-256 均为 `526c612d8222920bf76f91e4fb4b04ff413cd555a7f9969f802cb6c0ca798050`；`Recovery.exe hash .\Recovery.exe` 返回 0，`BackupRestore.exe` 进程烟测窗口标题为 `BackupRestore - Rust GUI`。这些证据只覆盖 ARM64 构建、哈希和进程启动，不替代 WinRE、DISM、BCDBoot 或真实重启验收。

2026-08-23 `v0.5.0` 绝对镜像路径收口：GUI 和 PowerShell 参数改为 `ImagePath`，例如 `B:\BackupRestore\Windows.wim`；脚本从路径根解析镜像卷身份，任务 JSON 同时记录 `absolutePath` 和经校验的卷内路径，RecoveryTask.env 记录 `IMAGE_ABSOLUTE_PATH` 并在 WinRE 重新校验。原生 GUI 增加保存/打开文件对话框，备份使用保存对话框，还原使用打开对话框。ARM64 VM 已用既有工具链重建，`build-manifest.json`、`BackupRestore.exe`、`Recovery.exe` SHA-256 均为 `beb50a2f74852285f4508a3b1510ae9f5c2ab7f6dbd02f0b2ca852749ae01073`，`Recovery.exe hash` 返回 0；`BackupRestore.ps1 -?` 已显示 `ImagePath` 且不再显示 `ImageDrive`/`ImageRelativePath`。这仍不替代真实备份/还原重启验收。

2026-08-23 `v0.5.2` GUI/WIM 收口：修复 WIM 多索引 JSON 数组分支的 Rust 借用错误，并同步两个 Cargo manifest、`Cargo.lock` 与 `VERSION` 到 `0.5.2`。ARM64 VM 使用已有工具链在浅层目录 `C:\BackupRestoreBuild\src`、`C:\BackupRestoreBuild\target`、`C:\BackupRestoreBuild\package` 构建成功，输出为 `BackupRestore-windows-arm64-v0.5.2`；没有重新下载工具链。`BackupRestore.exe` 继续使用 `WINDOWS_GUI` 子系统，WIM 索引下拉项同时展示序号和详细元数据。构建/哈希/启动证据不替代真实 WinRE、DISM、BCDBoot 或重启验收。
2026-08-24 `v0.5.4` probe 准备修复：修正镜像路径为空时的 `Test-Path` 和 `metadataPath` 生成逻辑，避免 `probe -NoReboot` 因空路径绑定错误失败；本机 AST、fmt、clippy、Rust 测试和 diff 检查均已通过，后续 VM 实测证据见下一条记录。
2026-08-24 VM 低负载实测补充：恢复挂起的 ARM64 VM 后，`probe -NoReboot` 任务 `760fcfe1-392d-4142-94af-b830f4286296` 成功，`validate-task`、manifest/payload 哈希和原始/注册 WinRE 哈希均通过；`recover --dry-run` 保持 `prepared`。容量不足的 backup 和未授权的 restore-existing 分支均按预期拒绝，未写入 `.partial`、未替换 WinRE、未格式化目标。测试完成后不执行真实重启，VM 可再次挂起以控制温度。

2026-08-23 `v0.5.3` WIM 权限回退：实测发现普通令牌下 `Get-WindowsImage` 可能返回可解析但没有索引的 JSON，GUI 因此不会触发管理员读取；现改为检测“索引为空”同样启动隐藏 `runas` 重试。ARM64 包已在 VM 重建并启动，`BackupRestore.exe`/`Recovery.exe` SHA-256 均为 `1e097ab33b01348db5515f41b25b56313f88f5b4ed9485e2782b7d4905ffc6dd`；点击“读取镜像”后真实下拉框显示索引 1（Windows Backup、162.9 MiB），操作模式提示分别解释当前系统覆盖还原和新增第二系统。该 GUI/UAC 读取证据不替代真实 WinRE、DISM、BCDBoot 或重启验收。

指定分区核实结论：代码和任务协议并不固定 C:。`BackupRestore.ps1` 的 `-SourceDrive`、`-TargetDrive` 接受任意单个盘符，镜像卷由用户填写的 `-ImagePath` 绝对路径根盘符解析；创建任务时会读取卷 GUID、磁盘/分区 GUID、偏移、容量、文件系统和序列号，Recovery 阶段按 GUID 复核后才把盘符作为临时挂载路径。`v0.4.7` 隔离实测实际使用非 C: 的 `S:` 源/目标卷、`B:` 镜像卷和独立 `E:` EFI，Capture、快速格式化、Apply-Image、BCDBoot 均成功；因此“不是只能备份 C:”已有实机证据。尚未完成的是从该独立 EFI 实际重启回恢复卷，以及 `create-secondary`、断电续跑和故障回滚，不能把“指定分区支持”外推成所有恢复场景均已验收。

## 5. 恢复工作时的唯一顺序

当前阶段已进入直接开发/构建，但不执行破坏性恢复操作。后续按以下顺序执行：

1. 每个独立大下载前运行：

   ```bash
   python3 ~/.codex/skills/pixian-dev-workflow/scripts/check_network.py
   ```

2. 检测为热点时，报告具体要下载的一个项目并等待授权；检测为 Wi-Fi/有线时，执行该下载。下载下一个大项目之前重新检测。
3. 保持 VM 构建目录浅层：源码 `C:\BackupRestoreBuild\src`、Cargo target `C:\BackupRestoreBuild\target`、输出包 `C:\BackupRestoreBuild\package`；不要按版本号继续创建多层 source/target/artifacts 目录。
4. 在 VM 中运行：

   ```powershell
   .\windows\build-windows.ps1 -Architecture arm64 -CargoTargetDir C:\BackupRestoreBuild\target -OutputRoot C:\BackupRestoreBuild\package
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
