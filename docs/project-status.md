# BackupRestore 当前进度与决策记录

更新时间：2026-08-25
当前开发版本：`0.7.7`
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
| 不触碰 C: 的范围 | 只禁止把 `C:` 作为 `backup` 的源分区或恢复任务的目标分区；允许读取 C: 状态、使用其 WinRE/BCD、写入正常 Windows 的任务和日志文件，以及在隔离测试中临时修改启动顺序。 | 本轮用户澄清、测试边界 |

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
| GUI | Rust Win32 原生单窗口是唯一桌面前端，负责操作模式、任务/源/目标卷详细下拉框、镜像读取、WIM 索引、环境和最近任务状态刷新、盘符校验、破坏性确认和管理员准备启动；支持中文/English 切换；镜像使用绝对路径。WIM 索引条目显示序号、名称、描述、版本、架构、Edition、安装类型和大小；非管理员返回空索引时会自动重试隐藏管理员读取。PowerShell 仅作为隐藏后端脚本。 | `v0.5.6` ARM64 包已使用现有工具链重建，包内无旧前端；`validate-task` 和 `recover --dry-run` 通过。盘符下拉框需下一次低负载 VM 窗口验收。已有 `v0.5.3` ARM64 包真实启动 GUI 并读取 WIM 索引。UAC/WinRE 仍按证据矩阵验收 |
| 构建与交付结构 | `build-windows.ps1` 生成架构隔离包，`BackupRestore.exe` 启动 GUI，`Recovery.exe` 作为 WinRE 主机，并随包携带 ARM64 MSVC runtime。 | `v0.4.8` ARM64 包已在 VM 使用既有工具链离线构建，manifest 与二进制 SHA-256 一致；`v0.4.7` 的管理员 GUI/UAC、Capture、Apply、格式化和独立 EFI BCDBoot 实测证据仍保留 |
| Windows 工具链 | Rust 1.98.0、`aarch64-pc-windows-msvc`、Visual Studio Build Tools ARM64、Windows SDK `10.0.26100.0`。 | 已安装并用于构建 |

### 已执行且通过的本机验证

以下命令只使用本机已有的离线缓存，不能证明 Windows 可运行：

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
pwsh -NoLogo -NoProfile -NonInteractive -Command '$files=@("windows/BackupRestore.ps1","windows/build-windows.ps1"); foreach($f in $files){$tokens=$null;$errors=$null; [System.Management.Automation.Language.Parser]::ParseFile((Join-Path (Get-Location) $f),[ref]$tokens,[ref]$errors)|Out-Null; if($errors.Count){$errors|% Message; exit 1}; "AST OK $f"}'
git diff --check
```

当前结果：Rust CLI/core 共 17 项测试通过；Clippy 无 warning；PowerShell 语法检查通过；差异无空白错误。

2026-08-25 `v0.7.7` 前置重叠校验：`BackupRestore.ps1` 现在在解析任务卷和镜像卷身份之后、开始 WinRE 注入、BCD 导出或设置一次性恢复启动之前，拒绝非 probe 任务的“任务卷 = 镜像卷”。Rust GUI 也在用户点击创建任务时立即拒绝同一盘符组合。核心任务模型原有的 GUID 分区重叠检查保留为第二层防线。这样错误参数不再可能先重启进入 WinRE，再由 `validate-task` 拒绝。ARM64 实机以 `B:` 任务卷、`U:` 源卷和 `B:\...wim` 镜像路径验证：子进程退出码为 1，注册 WinRE SHA-256 前后均为 `0CBC86...6FDA1`。

2026-08-25 `v0.7.7` 独立 EFI 启动实测：在新快照 `before-v0.7.7-efi-boot` 中临时把测试 EFI 磁盘 `hdd2` 调为首启动项，固件确实尝试从独立 EFI 启动，但 Windows Recovery 显示 `0xc0430001`，未进入 `U:`。随后恢复原启动顺序 `hdd0 cdrom0 usb hdd1 hdd2 hdd3` 并重新启动回正常 Windows；`C:` 未作为备份源或恢复目标。该结果将“隔离 Apply/BCDBoot 成功”与“独立 EFI 实际引导成功”明确区分，后者仍未通过。

2026-08-25 `v0.7.8` 独立 EFI 二次诊断：在快照 `before-v0.7.8-efi-bcd`（`c9ed1ee9-0a79-4a90-8c63-29afbc5c3033`）中，用管理员令牌读取 `E:\EFI\Microsoft\Boot\BCD`，确认默认 loader 的 `device/osdevice` 均为 `partition=U:`，`U:` 为 GPT 磁盘 3 分区 3、NTFS、约 8 GiB，`U:\Windows\System32\winload.efi` 存在；独立 EFI `E:` 为磁盘 2 分区 2、FAT32。随后用 `bcdboot U:\Windows /s E: /f UEFI /v` 成功重建 BCD，并再次把 `hdd2` 置首启动真实重启，仍稳定进入 Windows Recovery 并显示 `0xc0430001`，没有进入 `U:`。这排除了“仅因旧 BCD 残留”这一解释，但尚未证明根因；当前保留问题为跨磁盘 UEFI/Secure Boot/Windows loader 兼容性或 EFI/OS 分区关联。启动顺序已恢复为 `hdd0 cdrom0 usb hdd1 hdd2 hdd3`，VM 已回到正常 C: Windows，最新源码对应的 v0.7.7 Rust GUI 包已在客体前台运行。`C:` 仍未作为备份源或还原目标。

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

2026-08-22 GUI 收口：Rust `native_gui.rs` 接管 `BackupRestore.exe` 唯一桌面窗口，使用 Windows SDK Win32 API，不下载 GUI crate；窗口包含操作模式、任务/源/镜像/目标卷、镜像路径、WIM 索引、第二系统名称、环境刷新、镜像读取、最近任务状态刷新和管理员创建任务。默认值从当前系统及已挂载数据卷建议，所有盘符和镜像绝对路径在本地先校验；镜像卷与源卷在非 probe 模式下不得相同；环境和任务刷新会先显示进行中状态。`v0.4.6` ARM64 包已重建并启动烟测通过，窗口标题为 `BackupRestore - Rust GUI`；按钮交互、UAC、日志刷新仍待真实 VM UI 验收，不能用进程存活代替完整 GUI 验收。

2026-08-22 `v0.4.8` ARM64 构建收口：源码压缩包通过 Parallels 共享桌面传入 `C:\BackupRestoreBuild\source-v0.4.8`，未下载任何新工具链；使用已有 `aarch64-pc-windows-msvc` 工具链和 VM 本地 `C:\BackupRestoreBuild\target-v0.4.8` 构建成功。输出包为 `BackupRestore-windows-arm64-v0.4.8`，`build-manifest.json` 的 `binarySha256` 与 `BackupRestore.exe`、`Recovery.exe` 实际 SHA-256 均为 `526c612d8222920bf76f91e4fb4b04ff413cd555a7f9969f802cb6c0ca798050`；`Recovery.exe hash .\Recovery.exe` 返回 0，`BackupRestore.exe` 进程烟测窗口标题为 `BackupRestore - Rust GUI`。这些证据只覆盖 ARM64 构建、哈希和进程启动，不替代 WinRE、DISM、BCDBoot 或真实重启验收。

2026-08-23 `v0.5.0` 绝对镜像路径收口：GUI 和 PowerShell 参数改为 `ImagePath`，例如 `B:\BackupRestore\Windows.wim`；脚本从路径根解析镜像卷身份，任务 JSON 同时记录 `absolutePath` 和经校验的卷内路径，RecoveryTask.env 记录 `IMAGE_ABSOLUTE_PATH` 并在 WinRE 重新校验。原生 GUI 增加保存/打开文件对话框，备份使用保存对话框，还原使用打开对话框。ARM64 VM 已用既有工具链重建，`build-manifest.json`、`BackupRestore.exe`、`Recovery.exe` SHA-256 均为 `beb50a2f74852285f4508a3b1510ae9f5c2ab7f6dbd02f0b2ca852749ae01073`，`Recovery.exe hash` 返回 0；`BackupRestore.ps1 -?` 已显示 `ImagePath` 且不再显示 `ImageDrive`/`ImageRelativePath`。这仍不替代真实备份/还原重启验收。

2026-08-23 `v0.5.2` GUI/WIM 收口：修复 WIM 多索引 JSON 数组分支的 Rust 借用错误，并同步两个 Cargo manifest、`Cargo.lock` 与 `VERSION` 到 `0.5.2`。ARM64 VM 使用已有工具链在浅层目录 `C:\BackupRestoreBuild\src`、`C:\BackupRestoreBuild\target`、`C:\BackupRestoreBuild\package` 构建成功，输出为 `BackupRestore-windows-arm64-v0.5.2`；没有重新下载工具链。`BackupRestore.exe` 继续使用 `WINDOWS_GUI` 子系统，WIM 索引下拉项同时展示序号和详细元数据。构建/哈希/启动证据不替代真实 WinRE、DISM、BCDBoot 或重启验收。
2026-08-24 `v0.5.4` probe 准备修复：修正镜像路径为空时的 `Test-Path` 和 `metadataPath` 生成逻辑，避免 `probe -NoReboot` 因空路径绑定错误失败；本机 AST、fmt、clippy、Rust 测试和 diff 检查均已通过，后续 VM 实测证据见下一条记录。
2026-08-24 VM 低负载实测补充：恢复挂起的 ARM64 VM 后，`probe -NoReboot` 任务 `760fcfe1-392d-4142-94af-b830f4286296` 成功，`validate-task`、manifest/payload 哈希和原始/注册 WinRE 哈希均通过；`recover --dry-run` 保持 `prepared`。容量不足的 backup 和未授权的 restore-existing 分支均按预期拒绝，未写入 `.partial`、未替换 WinRE、未格式化目标。测试完成后不执行真实重启，VM 可再次挂起以控制温度。
2026-08-24 `v0.5.6` GUI 改动：移除旧桌面前端，构建包只包含 Rust `BackupRestore.exe`、后端 PowerShell 和 WinRE 载荷；所有 PowerShell 查询隐藏运行。任务卷、源卷、目标卷改为详细下拉框，`probe` 创建任务固定使用 `-NoReboot`。VM 已重建 `v0.5.6` ARM64 包并验证 `validate-task`/`recover --dry-run`。
2026-08-24 `v0.5.7` 中文 UI 修复：中文模式操作项和语言标签纯中文；右侧说明与卷详情使用可换行多行控件，窗口重新分栏并扩大，避免文字裁剪/覆盖 WIM 区域。
2026-08-24 `v0.5.8` UI 结构修复：操作模式改为四个可点击标签按钮；完整任务/源/目标卷信息移到窗口底部三栏；全部控件显式使用系统默认 GUI 字体。ARM64 包待 Mac 解锁后补最终前台截图验收。

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

2026-08-24 `v0.5.9` GUI 布局调整：移除右上角独立说明框。四个模式的提示统一显示在中部白色提示框；任务卷、源卷、目标卷恢复为纵向三段，每段的左侧标签说明用途，下拉框显示完整一行分区摘要，只读详情框紧随该下拉框。详情首段按当前模式明确说明该卷的用途、会发生的动作和限制，再显示卷标、文件系统、容量、磁盘/分区、分区类型与卷 GUID。字段按模式收紧：探测仅任务卷/源卷；备份增加镜像绝对路径；单系统还原增加目标卷和 WIM 索引；新增第二系统才增加启动项名称。窗口启动即最大化，语言选择器与操作模式处于同一行；所有多行说明文本统一写入 Windows `CRLF`，确保不依赖自动换行。ARM64 实机截图点检待本轮最终重建后补充。

2026-08-24 `v0.6.0` 最终 UI 点检：ARM64 VM 已用既有工具链构建并在用户 Console 会话前台最大化运行。下拉框保留完整分区摘要；任务卷、源卷和目标卷的说明框按显式 `CRLF` 逐行显示，且分别紧随自己的下拉框。语言选择器与操作模式位于同一行。实机包、哈希和截图仅证明本轮 GUI 布局/启动，不替代 WinRE、DISM、BCDBoot 或重启验收。

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

2026-08-25 非 C 指定分区真实备份：复制的 Windows fixture 挂载为 U:（Disk 3 Partition 3，约 8 GiB），任务卷 B:，独立镜像卷 T:（Disk 3 Partition 2）。任务 `bc1660b8-6863-495d-b333-cd168f9a5c41` 完成 `Windows -> WinRE -> DISM Capture -> 原始 WinRE hash 恢复 -> Windows`，最终 `status.json=success`；WIM 1.63 GB，metadata/hash 一致。该证据不涉及 C:。

2026-08-25 多索引 WIM：从非 C 备份 WIM 导出两个索引到 `T:\BackupRestore\tests\non-c-u-multi\Windows.wim`，DISM `/Get-WimInfo` 显示 Index 1/2，两个名称分别为 `Windows Backup Index 1/2`。首次 `restore-existing` 未完成，根因是任务自动选择系统 EFI（任务 env GUID `d08d...`）而独立测试 EFI E: GUID 为 `6ba9...`，Recovery 在 WinRE 进入前发生 EFI 身份冲突；U: 未被格式化，C: 未触碰。

2026-08-25 EFI 选择缺口修复：`BackupRestore.ps1` 新增可选 `-EfiDrive`，默认仍选择系统启动盘 EFI；隔离多磁盘测试可显式指定独立 EFI（例如 E:），避免把生产 EFI 与测试 EFI 混淆。v0.7.6 ARM64 实机已用该参数完成 Index 2 还原。

2026-08-25 Recovery EFI 冲突修复：Index 2 还原的 WinRE 日志显示镜像卷被复用为 E:，而 EFI 也硬编码偏好 E:，导致 EFI 身份冲突、目标 U: 未格式化。Rust Recovery 现以 Z: 作为 EFI 临时挂载偏好，并让 BCD 回滚使用实际 EFI 根路径；生产默认 EFI 选择不变。v0.7.6 ARM64 实机重试成功。

2026-08-25 多索引 Index 2 真实还原成功：任务 `375f4422-7c17-4397-9560-6c83d7ca9ff4` 使用 v0.7.6 Recovery、`-EfiDrive E`、T: 多索引 WIM Index 2、U: 目标分区，完成快速格式化、DISM Apply-Image、`bcdboot F:\Windows /s Z:\ /f UEFI`、原始 WinRE 恢复和自动返回 Windows；最终 `status.json=success`。U: 目标 SYSTEM 与 fixture 源 SYSTEM hive SHA-256 均为 `A70A0D2750D67E0D3B4054C9284F5794B2A699CC3A76203B7297CD3EAF6CC550`；E: BCD、`bootmgfw.efi`、`bootaa64.efi` 均存在。该证据证明指定分区和多索引 Apply/BCDBoot，不证明从 E: 实际引导 U:。
