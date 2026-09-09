# BackupRestore 当前完整进度基线

更新时间：2026-09-09  
源码版本：`1.3.3`  
分支：`main`  
仓库：`/Users/x/code/backupRestore`  
Windows 共享源码：`C:\Users\x\Desktop\BackupRestore`  
目标平台：Windows 11 ARM64 / UEFI / GPT

本文是本轮继续开发后的事实基线。设计目标、代码实现、离线证据、Windows/WinRE 实机证据、失败证据和剩余工作分开记录。旧版本历史仍保留在其他文档，不能用旧包、旧截图或旧任务 ID代表当前源码。

## 1. 设计目标与固定约束

| 流程 | 设计目标 | 当前结论 |
|---|---|---|
| `probe` | 只检查 Windows、WinRE、卷身份和载荷，不格式化、不 Apply、不重启 | 已实现并实机通过 |
| `backup` | 选择镜像绝对路径和源分区，重启进入 WinRE，由 Rust Recovery 执行 DISM Capture | 已实现；非 C: 的 P/H Capture 已实机通过 |
| `restore-existing` | 选择镜像、WIM 索引和目标分区，WinRE 格式化、Apply、修复当前 EFI、清理并返回 Windows | 已实现；非 C: 隔离 Apply/BCDBoot 已实机通过 |
| `create-secondary` | 保留当前 Windows，在另一分区 Apply，并在当前 EFI 增加第二启动项 | 代码已实现；最新版回归和从菜单实际启动第二系统仍未完成 |

固定规则：

- 产品运行时只有 Rust：Rust Win32 GUI、Rust prepare、Rust `Recovery.exe`。PowerShell 仅用于 Windows 构建和测试控制。
- 程序目录就是工作目录。任务、载荷、状态、日志和 BCD 快照写入 `<程序目录>\tasks`、`<程序目录>\logs`；不依赖 `C:\ProgramData\BackupRestore`，不再让用户选择“任务卷”，不接受 `TaskDrive`。
- 还原前按程序目录所在卷的 GUID 与目标卷比较。相同则立即阻止，不创建任务、不修改 WinRE/BCD、不请求重启；用户必须手动移动整个程序目录。
- 测试不把 `C:` 作为备份源或还原目标；允许读取 C: 的状态及把程序、日志和测试文件放在 C:。
- 普通 GUI 不显示独立 EFI 选择；`--test-efi-drive` 仅用于开发诊断。正常流程用 `mountvol /S` 定位当前实际启动的系统 EFI。
- 镜像路径必须是带盘符的绝对路径；任务同时保存绝对路径和卷内相对路径，应对 WinRE 盘符变化。
- 破坏性测试只在可回滚快照中使用 P/Q/H fixture，不将实验结果外推到 C:。

## 2. 当前代码结构

| 文件 | 职责 |
|---|---|
| `crates/backuprestore-core/src/lib.rs` | 任务模型、卷身份、状态机、原子 JSON、容量和保留分区校验 |
| `crates/backuprestore-cli/src/main.rs` | 两个 Rust EXE 的入口、WinRE 恢复、阶段续跑、BCD 菜单与回滚 |
| `crates/backuprestore-cli/src/native_gui.rs` | Rust Win32 GUI、标签、盘符/WIM 控件、提示、绝对路径、UAC |
| `crates/backuprestore-cli/src/windows_prepare.rs` | 正常 Windows 卷枚举、WinRE/EFI/BCD 准备、WIM 信息、任务写入 |
| `windows/winpeshl.ini` | WinRE 中直接启动 Rust `Recovery.exe` |
| `windows/build-windows.ps1` | ARM64/x64 架构隔离构建和 manifest，不自动下载工具链 |
| `docs/*.md` | 设计、协议、验证矩阵、失败根因和交接记录 |

## 3. 已完成的实现

### 3.1 任务、身份和安全

- 四种操作和完整阶段状态机已存在；TaskStore 使用 UUID 目录并拒绝路径穿越、非法 ID、串任务和不完整目录覆盖。
- 源、工作目录、镜像、目的地、目标和 EFI 保存卷 GUID、磁盘/分区 GUID、磁盘号、分区号、偏移、容量、分区类型、文件系统和卷序列号；盘符只作临时挂载提示。
- EFI、MSR、Recovery 分区不能作为程序目录、镜像卷或还原目标；目标容量按 metadata 和源分区大小检查；BitLocker 状态未知或已加密时拒绝。
- 任务与状态文件采用原子替换；Windows 使用 `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)`。

### 3.2 工作目录、WinRE 和日志

- `WORKSPACE_ROOT_REL` 从程序目录推导；WinRE 先按工作目录卷身份挂载，再读取 `<工作目录>\tasks\<任务 ID>`。
- 原始 WinRE、Recovery 二进制、环境文件、任务 JSON、manifest 和暂存 WinRE 均做 hash 校验；失败时尽力恢复原始 WinRE。
- 只有原始 WinRE 恢复并通过 hash 校验后才写 `success`；失败任务保留日志和中间文件供诊断。
- `winpeshl.ini` 直接启动 Rust Recovery，不再有破坏性批处理回退。
- 日志集中在程序目录：GUI 日志、准备日志、恢复日志、启动错误日志均为 RFC 3339 时间戳逐行记录。

### 3.3 备份、还原和多索引 WIM

- DISM Capture 使用 `.partial`，完成后才转为正式 WIM；每个 WIM 索引有独立 metadata sidecar。
- GUI WIM 下拉项显示索引号、名称、描述、版本、架构、Edition、安装类型和大小；镜像路径为绝对路径。
- 还原阶段先持久化 `target-erased`，Apply 后写 `image-applied`，BCDBoot 前写 `boot-repaired`。
- `restore-existing` 和 `create-secondary` 均用已验证的磁盘号/分区号格式化，不依赖临时盘符。
- 当前 EFI 默认由 `mountvol /S` 定位；第二系统追加后恢复 Windows Boot Manager 的原 default/display order，再追加 secondary loader。
- BCD 解析只处理 Windows Boot Manager 对象，不把固件 `displayorder` 混入 Windows 菜单；活动 BCD 被锁定时有逻辑导出回滚路径。

### 3.4 阶段中断续跑

- GUI 启动时扫描程序目录内唯一合法的非终态任务。`boot-requested`、`recovery-started`、`preflight`、`capturing`、`target-erased`、`image-applied`、`boot-repaired` 可重新请求 WinRE；`prepared` 不自动重启。
- WinRE 盘符变化不会破坏任务比较：payload 比较前清除临时 `drive_letter`，只比较稳定身份。
- 目标格式化会改变卷序列号；已进入目标擦除后的阶段只放宽目标卷序列号，其他身份字段仍严格比较。
- 阶段故障注入增加一次性 marker，防止同一 fault 在续跑时无限重复触发。

### 3.5 GUI

- 唯一桌面前端是 Rust Win32 原生窗口，不使用 WPF/Tauri/PowerShell UI。
- 操作模式为多标签；语言选择器与标签同行；页面按用途显示源卷、目标卷、镜像和 WIM 索引。
- 盘符下拉框显示用途、卷标、文件系统、容量、可用空间、磁盘/分区和 GUID 摘要；说明栏使用系统默认 GUI 字体。
- 窗口启动默认最大化；按钮和输入控件有悬停提示；标题格式为 `BackupRestore - Rust GUI v{PROGRAM_VERSION}`。
- UAC 使用 `ShellExecuteW("runas")`；GUI 不把 UAC 接受或 prepare 成功误报为 WinRE 恢复成功。

## 4. 已验证证据

### 4.1 本机离线检查

源码 v1.3.3 最近一次结果：

- `cargo fmt --all -- --check`：通过。
- `cargo test --workspace --all-targets --offline`：CLI 8/8、core 17/17。
- `cargo clippy --workspace --all-targets --offline -- -D warnings`：通过。
- `bash scripts/audit-runtime-boundaries.sh`：通过。
- `git diff --check`：通过。

这些检查不下载工具链，也不能替代 Windows ARM64/WinRE 行为验证。

### 4.2 Windows ARM64 构建

- 客体已有 `aarch64-pc-windows-msvc`、Visual Studio C++ 和 Windows SDK；v1.3.3 ARM64 Release 构建曾成功。
- 构建命令为：

```powershell
.\windows\build-windows.ps1 -Architecture arm64 `
  -CargoTargetDir C:\BackupRestoreBuild\target-v1.3.3 `
  -OutputRoot C:\BackupRestoreBuild\package
```

- 构建脚本检查根 `VERSION`、两个 Cargo manifest 和 `Cargo.lock` 一致，当前源码均为 `1.3.3`。
- 切换到较早 VM 快照后，v1.3.3 包目录可能不存在；这是快照内容差异，恢复测试快照后必须重新构建。

### 4.3 P/H/Q 正常链路

- P 是非 C: 源/单系统目标，H 是 WIM 镜像卷，Q 是第二系统目标。
- v1.2.5/v1.3.0 已实机完成 P→H Capture、同一 WIM Index 1/2/3、Index 1/2/3 Apply 到 Q、Index 2/3 Apply 回 P、DiskPart、BCDBoot、WinRE 清理和自动回 Windows。
- 代表任务：P→H `5f2d2262-80c1-43c8-b839-8b2544effedf`、`fcad95ce-9c46-4e74-8302-452fbda7992f`、`31971c01-62b3-47ff-a01f-22dddb2f9541`；Index 3→Q `8f5ca68c-a7f5-410d-8cfc-2791f33a389d`；Index 3→P `8538cc29-79fe-41d3-9de2-8458e3bfaa48`。
- 结果均为 `success`；P/Q fixture 按路径、大小和 SHA-256 比较无差异；WinRE 为 Enabled，DISM 无挂载 WIM。

### 4.4 已验证异常

- `identity-env-mismatch`：WinRE 在 DISM/格式化/BCDBoot 前拒绝，目标未格式化，WinRE 恢复。
- `bcdboot-failure`：Apply 后注入，开发 EFI BCD 与快照 hash 一致，任务失败且 WinRE 清理完成。
- `power-loss-window`：`boot-requested` 持久化后 GUI 能识别唯一待恢复任务，重新请求 WinRE，最终成功。
- 1 GiB 目标容量不足被拒绝，未格式化；非 C: 源卷加密到 100% 后备份准备被拒绝。

## 5. 当前未完成与失败证据

### 5.1 阶段断电续跑

三种新 fault 为 `power-loss-target-erased`、`power-loss-image-applied`、`power-loss-boot-repaired`。当前状态是：**代码已覆盖、离线已验证、最新版 ARM64 实机待验证**。

已发现并修复的真实问题：

1. v1.3.1 续跑时 Windows/WinRE 盘符变化参与 payload 全量比较，报 `payload task does not match the workspace task`；v1.3.2 清除临时盘符后比较。
2. 目标格式化改变卷序列号，续跑报 `TARGET volume serial differs after mounting`；v1.3.2 对已格式化目标放宽序列号，其余身份仍严格。
3. v1.3.2 续跑重新进入同一阶段时 fault 无条件再次触发，形成格式化/重启循环；v1.3.3 增加一次性 marker，并只在首次进入阶段时注入。
4. v1.3.3 修改后只完成本机检查和 ARM64 构建；因切换快照导致包目录不在当前快照，尚未用 v1.3.3 完成三种真实 WinRE 故障的最终成功证据。

### 5.2 当前 EFI 第二系统与独立 EFI

- 独立 EFI E: 从 Parallels 固件首启动反复得到 `0xc0430001`。即使 BCD 指向 U:、Secure Boot 切换、`bootmgfw.efi` 版本对齐，仍未进入 U:。
- 独立 EFI 仅保留为开发测试功能，不进入普通 GUI，也不作为 V1 发布门槛；现阶段产品只修改当前系统 EFI。
- v1.3.1 曾完成一次当前 EFI 的第二系统 Apply/BCDBoot，并记录保留原 default、追加 secondary loader；最新 BCD 解析修复尚未在干净快照完成回归。
- 现有证据排除了“仅旧 BCD 残留”“仅 Secure Boot 开关”和“仅 bootmgfw 版本不同”，但不能仅凭 `0xc0430001` 断定唯一根因。

### 5.3 GUI、安装发现和 BitLocker

- GUI 代码已具备标签、绝对路径、盘符、WIM 索引、提示和 UAC；最新版尚未完成一条完整的真实鼠标流程验收。
- 当前卷枚举返回普通 NTFS 卷身份，尚未将包含 `Windows\System32\Config\SYSTEM` 的当前/其他 Windows 安装作为结构化发现字段完整展示。
- prepare 已检查源、镜像和还原目标的 BitLocker；EFI 角色的显式拒绝和所有角色组合的真实矩阵仍需补充。
- 尚未在 C: 上开启或修改 BitLocker；角色测试只能用快照和非 C: 隔离卷。

### 5.4 尚待实机拒绝矩阵

损坏 WIM、镜像文件运行时消失、EFI/Recovery 误选、程序目录移动到另一分区后直接运行、多个合法待恢复任务并存等场景已有代码边界或计划，但最新版尚需客体证据证明：无任务创建、无格式化、无 BCD/WinRE 修改、无重启请求。

## 6. 当前客体事实

- VM 是 Parallels `Windows 11` ARM64、UEFI、Secure Boot 开启；当前状态和星号快照必须每次用 `prlctl status`、`prlctl snapshot-list` 重新确认。
- 阶段测试使用 P/Q/H fixture；WinRE 会把它们重新挂载为其他盘符，例如 G/F/H，所以恢复逻辑必须依赖 GUID。
- 不能把 Parallels 控制中心窗口当作客体 GUI；Computer Use 操作前必须确认截图是 Windows 桌面或 BackupRestore 窗口。
- 测试截图统一放在 `.test-artifacts/root-captures/`，不放项目根目录；历史包和无用快照应在确认引用关系后清理。

## 7. 下一轮执行顺序

1. 恢复含 P/Q/H fixture 的干净快照，重新构建 v1.3.3 ARM64 包并核对 manifest、hash、架构和标题。
2. 在三个独立快照中测试 TargetErased、ImageApplied、BootRepaired fault；每次确认只注入一次，任务最终到达 `success` 或安全 `failed`，WinRE Enabled、无挂载 WIM、Q payload 与源清单一致。
3. 用当前系统 EFI 重跑 `create-secondary`，检查 Windows Boot Manager 只保留原菜单并追加 secondary，不混入 firmware entries。
4. 实测损坏/缺失 WIM、EFI/Recovery 误选、镜像卷消失和程序目录移动；记录无任务、无 BCD/WinRE 变化和无重启。
5. 补充 Windows 安装发现字段和 EFI BitLocker 显式检查，同步 CLI/core 测试。
6. 用最新包保持 GUI 前台不关闭，Computer Use 逐页完成真实鼠标验收；截图只写入忽略目录。
7. 更新所有状态文档，再运行离线检查和 ARM64 构建，最后中文提交并推送。

## 8. 统一判断口径

| 标签 | 证据要求 |
|---|---|
| 代码已覆盖 | 源码和规则存在，未完成行为证明 |
| 离线已验证 | fmt、offline test、clippy、审计通过 |
| ARM64 构建已验证 | 客体工具链构建成功，manifest/hash/架构一致 |
| Windows 准备已验证 | 真实 prepare、任务和载荷身份检查成功 |
| WinRE 实机已验证 | 真实重启进入 WinRE，DISM/格式化/BCDBoot/清理/返回 Windows 有持久证据 |
| GUI 真实鼠标已验证 | 最新包由真实鼠标完成指定交互并有截图/日志 |
| 已知失败 | 可重复错误码、日志和快照回滚证据 |

当前总判断：**Rust 核心产品和非 C: 正常备份/还原链路已完成；v1.3.3 仍是开发测试版，阶段断电续跑最新版实机回归、Windows 安装发现、EFI BitLocker 角色矩阵、最新版 GUI 完整鼠标验收和独立 EFI 首启动仍未收口。**
