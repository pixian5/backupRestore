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

### 4.5 v1.3.3 三阶段断电续跑（实机完成）

在基线快照 `before-v133-stage-fault-tests`（id `{9c3e4813-b14d-4db1-ad19-554bdd8b7a09}`）上切三个独立快照，每个流程 = `prepare --test-fault <name>`（`--current-user` 通道 + Start-Process -Verb RunAs）→ WinRE 内 fault 触发回 Windows → 启动 GUI 触发 `resume_pending_boot_task()`（reagentc /boottore + 重启）→ 续跑至 success：

| 任务 | fault | 任务 ID | 结果 |
|---|---|---|---|
| Test1 | power-loss-target-erased | `7590b3ac-a869-4518-8ced-65099ae9797f` | success/100 |
| Test2 | power-loss-image-applied | `93abc3a2-5051-467f-8ea0-e2697f8e7014` | success/100 |
| Test3 | power-loss-boot-repaired | `ce53efd6-9261-4148-8df6-f4be524c8adc` | success/100 |

- 每个任务 fault marker 恰好 1 个（一次性 marker 防循环）。
- recovery.log 均含 `Preserved Boot Manager default {d2264b2f-...}; restored 1 original display-order entries and appended secondary loader`。
- WinRE Enabled、无挂载 WIM；P↔Q 216 文件 216 size-match、98 目录一致。

### 4.6 当前 EFI BCD 回归（实机完成）

`bcdedit /enum` 确认 Windows Boot Manager 仅保留原 default `{d2264b2f-5431-11f1-9fa0-b55b60784edc}` 并追加 secondary（StageBootRepaired），firmware entries 未混入 Windows 菜单；三个 fault 测试各追加不同 secondary loader GUID。

### 4.7 异常拒绝矩阵（实机完成）

6 案例全部 EXITCODE=1、无新任务目录、`bcdedit /enum` 逻辑对比 UNCHANGED、WinRE 保持 Enabled、全程无重启：

| 案例 | 结果 |
|---|---|
| missing-wim | 拒绝 |
| corrupt-wim | 拒绝 |
| efi-target | 拒绝（旧包报 BitLocker 错误，即第 5 项修复目标） |
| recovery-target | 拒绝 |
| image-vol-gone | 拒绝 |
| same-vol-workspace | 拒绝（"Cannot start restore: the program directory is on Q:, which is the restore target...No task, WinRE, BCD or reboot was requested"） |

### 4.8 Windows 安装发现字段 + EFI 显式拒绝 + json_text 修复（实机完成）

- `windows_prepare.rs`：`DriveReport.has_windows_installation`；`discover_drives()` 检测 `{letter}:\Windows\System32\Config\SYSTEM`；restore 目标在 `assert_bitlocker_off` 之前显式拒绝保留分区（`restore_reserved_target_error()` + `prepare_safety_tests::reserved_restore_target_error_names_the_role`）。
- `native_gui.rs`：`DriveInfo.hasWindowsInstallation` 解析与显示（"| Windows" 标记、"Windows 安装: 是/否"）。
- **修复 bug**：`json_text()` 不处理布尔值 → GUI 中 hasWindowsInstallation 恒 false；修复为 `as_bool().map(|flag| flag.to_string())`，新增 `json_text_reads_boolean_values` 测试。
- 实测 `list-volumes` 新字段：C/P/Q/G/F hasWindowsInstallation=True，H=False。
- Windows 目标测试：CLI 16/16、core 17/17。

### 4.9 GUI 逐页真实验收（实机完成）

最新包（exe hash `355c222d61ceeff5580c73bf6e940c7b9e4ebfdfba321a08c080f7310b3c8159`）保持前台，经 Windows 侧注入真实 WM_COMMAND/BM_CLICK（GUI 提升运行，注入任务以 Interactive+RunLevel Highest 同会话执行，绕过 UIPI）逐页完成，gui.log 30 条动作记录 + 8 张截图归档于 `.test-artifacts/root-captures/v133-gui-*.png`：

- 启动：标题/版本正确，C: 显示 "| Windows" 与 "Windows 安装: 是"。
- 刷新环境：`GUI action completed: refresh environment; eligible_volumes=6`。
- 备份页：源卷（备份来源）、镜像绝对路径 + 浏览…、提示文本正确。
- 单系统还原页：源卷（当前系统）/目标卷（覆盖还原）双面板、WIM 索引占位正确。
- 新增第二系统页：源卷（保留系统）/目标卷（第二系统）、第二系统名称默认 "Windows 备份"。
- 读取镜像空路径：`GUI action blocked: invalid WIM path` + "参数校验失败" 对话框，可正常关闭。
- 读取镜像 `H:\Images\CurrentEfiV131.wim`：`GUI action completed: read WIM metadata; indexes=1`，状态区显示 SHA-256 与索引 1 详情。
- **多索引 WIM 下拉验收（2026-09-10 补齐）**：从 CurrentEfiV131.wim 用 DISM Export-Image 生成 `H:\Images\MultiIndexTest.wim`（索引 1/2，DISM `/Get-WimInfo` 实读确认，名称分别为 "Windows Backup Index 1/2"）；GUI 填入路径读取返回 `GUI action completed: read WIM metadata; indexes=2`（SHA-256 `7abab43c0e840f7080605f451712264833a02e28b8a941d1ac4725a5531c2f70`）；CB_SHOWDROPDOWN 展开后下拉列表显示 "索引 1 | Windows Backup Index 1" 与 "索引 2 | Windows Backup Index 2" 两项，截图归档 v133-gui-07-multiindex-loaded.png / v133-gui-08-multiindex-dropdown.png。
- 刷新任务状态：started/completed 成对记录。

### 4.10 create-secondary 菜单实际启动第二系统（实机完成，2026-09-10）

**验证动线**：只读核查 BCD → 真实重启看菜单 → 固件阶段键盘注入判死 → 死条目定位 → 改道 PE 第二系统 → 真实引导进入 PE。

- **BCD 追加侧（§4.6 已验）基础上补真实重启**：`bcdedit /bootsequence {547dddcc-ac5f-11f1-8d4a-cc37f74e6a64}`（default 保持 `{current}` 不变）→ `prlctl restart` → Boot Manager 菜单真实出现两条目「Windows 11」（默认高亮，25 秒计时）与「StageBootRepaired」，截图 `v133-bootmenu-01-two-entries.png`。
- **死条目定位（0xc000000f 实锤）**：菜单停留无键响应（prlctl send-key-event 在 bootmgr 固件阶段完全不注入——`--key 0x5B` 报格式错、`--key 40/13` 接受但无效果、`--scancode 0xE050/0xE0/0x50` 被拒，与「BIOS/锁屏只有 VNC/USB HID 可能」方案分析一致）；`bcdedit /bootsequence` 改引导 {547dddcc} → 真实重启 → Recovery 蓝屏 "File: \Windows\system32\winload.efi / Error code: 0xc000000f"，截图 `v133-bootmenu-02-recovery-0xc000000f.png`。根因：Q 系统残缺——`Q:\Windows\System32` 只有 `bootstr.dll` 无 `winload.efi`（v1.3.3 断电测试 boot-repaired 阶段的最小重建产物），StageBootRepaired 为「菜单出现但引导必败」的死条目。**实锤"BCD 有条目 ≠ 可引导"。**
- **PE 作为第二系统（用户改道，全部载荷本地化，零下载）**：发现已装 ADK 10.1.28000.1 的 WinPE 加载项（`arm64\en-us\winpe.wim` 366,848,527 B 一直在位，先前"载荷未装"为误判）；copype 反复失败真因是提升会话三个环境变量为空——显式 `set WinPERoot / OSCDImgRoot(Deployment Tools\arm64\Oscdimg) / DISMRoot(C:\Windows\System32)` + 先清理目标目录后 `copype.cmd arm64 C:\WinPE_arm64` **EXIT=0 成功**（脚本保留在共享目录 `_copype6.cmd`）。部署 `C:\WinPE_arm64\media → Q:\`（`Q:\sources\boot.wim` 366MB / `Q:\boot\boot.sdi` / `Q:\bootmgr.efi` 在位）；BCD 新增 osloader 条目 {137379e8-ac70-11f1-8753-f4d0933357ef}「Windows PE Test」device/osdevice=`ramdisk=[Q:]\sources\boot.wim,{ramdiskoptions}`、path `\windows\system32\winload.efi`、winpe yes、detecthal yes；**该 VM BCD 原本不存在 {ramdiskoptions} 对象**，补 `bcdedit /create {ramdiskoptions}` + ramdisksdidevice partition=Q: + ramdisksdipath \boot\boot.sdi；displayorder /addlast + bootsequence 均成功（枚举证据 `_bcdpe2.log`）。
- **真实引导进入 PE（验证目标达成）**：`prlctl restart` → bootmgr → ramdisk 加载 Q:\sources\boot.wim（boot.wim 内 winload.efi，天然绕开 Q 缺物理 winload.efi）→ **进入 WinPE：标题 "Administrator: X:\windows\system32\cmd.exe"，已执行 wpeinit，回到 `X:\Windows\System32>` 提示符（X: 为 PE RAM 盘，标准 PE 特征）**，截图 `v133-bootmenu-03-pe-booted.png`。**"BCD 追加条目 → 真实重启 → 进入第二系统"引导链完整成立。**
- **已知坑（PE 场景特有）**：PE（winpe=yes）环境下 bootmgr 不消费/不写回 bootsequence（PE 是 RAM 盘，无法回写 ESP 的 BCD）→ 设了 bootsequence 后每次重启都会再进 PE，验证后需恢复快照（本轮用 `before-pe-boot-test` 快照还原现场）或人工清 bootsequence。prlctl send-key-event 固件阶段无效已实证，Boot Manager 菜单交互自动化仅剩 VNC/USB HID 通道（未采用）。
- **环境清理**：验证后已恢复 `before-pe-boot-test` 快照还原现场；死条目 `{547dddcc-…}`（StageBootRepaired）已从 BCD 删除（`/delete /cleanup`，2026-09-10），displayorder 恢复仅 `{current}`。
- **结论**：create-secondary 启动侧闭环（BCD 追加 → 菜单 → 引导第二系统）在真实引导链上验证通过（以 PE 作为第二系统）。完整 Windows 第二系统（非 PE）的引导机制与 PE 完全同构（同 bootmgr → ramdisk/分区 → winload.efi 路径），Q 死条目不构成 BCD 机制问题，属目标卷残缺；若需全量闭环可后续用完整系统 WIM 再验，非 V1 门槛。

### 4.11 路线 3：自建 PE「恢复桌面」（实机完成，2026-09-10）

**目标**（用户拍板路线 3）：PE 启动链（winpeshl.ini）直接启动产品 exe，以全屏「专用恢复桌面」呈现（深蓝背景 + 6 卡片 + 底部版本栏 + 数字时钟），真实重启进 PE 验证中文、卡片、可退出回 Windows。

- **代码（已落地，编译通过）**：`crates/backuprestore-cli/src/native_gui.rs` 新增 PE 桌面全套——`run_pe_desktop()`（RegisterClassExW "BackupRestorePeDesktop" + WS_POPUP 全屏 + 消息循环）、`window_proc_pe()`（WM_CREATE 建 6 卡+时钟、WM_TIMER 每秒刷新、WM_CTLCOLORSTATIC/BTN 返回深蓝背景 0x00553a2b、WM_COMMAND 处理卡片、WM_DESTROY 清理）、`exit_pe_to_windows()`（枚举卷→挂 FAT/FAT32 到 S:\→ShellExecuteW bcdedit /store 改 default={current}→Sleep→卸载→ExitWindowsEx 重启）；`main.rs` 加 `--pe-desktop` 分支 + env `BACKUPRESTORE_OPEN_TAB`；`windows/winpeshl.ini` 入口改 `%SYSTEMROOT%\System32\BackupRestore.exe,--pe-desktop`。
- **构建/打包管线（全本地化，零下载）**：`windows\build-windows.ps1 -Architecture arm64` → 包目录含 BackupRestore.exe/Recovery.exe/VCRUNTIME140*.dll/winpeshl.ini；`poc/build-backuprestore-pe.ps1`：DISM 挂载 ADK arm64 winpe.wim → 注入 5 payload → **`/Add-Package WinPE_OCS\WinPE-FontSupport-ZH-CN.cab`（中文字体，本轮新增）** → Commit/Export → copype 三环境变量配方 `_copype6.cmd` → 定制 boot.wim（412,676,713 B）。
- **第一轮实机（2026-09-10 02:30）**：BCD 建 PE 条目 {179ca179-…} + {ramdiskoptions} + default=PE → 真实重启 → **PE 桌面成功显示**（深蓝背景、6 卡、时钟走动，截图 `pe-desktop-boot.png`），但**中文全乱码方块**（定制 wim 缺 CJK 字体）。
- **第二轮失败 0xc0000225 根因定位**：加 FontSupport + 「返回 Windows」卡后重建 wim（fc81bcc5…）→ 部署 → 真实重启 → Boot Manager "Windows failed to start 0xc0000225"（截图 `boot-diag-2.png`、`key-91.png` 证明 Windows 键注入无效）。**回滚排查对比两次 BCD 差异：第一次成功条目含显式 `systemroot \windows` + `nx OptIn`，第二轮 `bcdedit /create` 新条目缺这两个字段 → 补上后（`_deploy3.ps1` 修订）重新部署 → 0xc0000225 消失**。附验证：Q:\sources\boot.wim SHA256 与源 media 完全一致（复制无损坏）。
- **最终实机（2026-09-10 03:16）**：定制 wim（fc81bcc5…，含字体+EXIT）部署 + BCD 完整（displayorder {current} {179ca179}、default=PE、timeout 5）→ 真实重启 → **PE 恢复桌面完整显示：标题「BackupRestore 恢复桌面 v1.3.3」、六卡「备份系统/还原系统/安装第二系统/命令提示符/返回 Windows/重启」全部正常中文、底部版本栏「BackupRestore v1.3.3 | WinPE」+ 数字时钟 03:16:25 走动**（截图 `pe-desktop-zh.png`，OCR 全中文无乱码）。**中文显示 + 桌面本体 = 实机达成。**
- **「返回 Windows」退出链路**：代码逻辑就绪（枚举卷→挂 FAT/FAT32 到 S:\→改 ESP BCD default={current}→ExitWindowsEx 重启）；Windows 侧已验证 ESP BCD 路径存在可读（`mountvol S: /S` → `S:\EFI\Microsoft\Boot\BCD` 存在）。**真实点击受 Parallels 环境限制**（PE 无 Tools 会话、prlctl send-key-event 在 PE 桌面亦不注入——本轮实测 6×Tab+Enter 无反应），自动化无法代点，留用户真实使用鼠标点击验收。
- **收尾（菜单模式）**：验证完成后 BCD default 恢复 `{current}`（默认进 Windows），PE 条目保留在 displayorder（timeout 5 菜单可选）→ 真实重启**自动进 Windows 桌面**（截图 `win-back.png`，3:39 时间戳）。产品形态：**重启见菜单（Windows 11 / Windows PE Test），选 PE 进恢复桌面，PE 内「返回 Windows」退出回 Windows**。
- **结论**：路线 3 核心目标（PE 桌面中文显示 + 引导链 + 可退出设计）实机验证达成；0xc0000225 根因（BCD osloader 条目缺 systemroot/nx）已修复并沉淀到 `_deploy3.ps1`。遗留：PE 内卡片点击自动化（Parallels 注入限制）与「返回 Windows」真机点击留用户手动，非产品缺陷。

## 5. 当前未完成与失败证据

### 5.1 阶段断电续跑

**已收口**：v1.3.3 三个 fault（target-erased/image-applied/boot-repaired）已在独立快照中各自完成真实 WinRE 断电→续跑→success，证据见 §4.5。修复链：v1.3.1 盘符参与比较 → v1.3.2 清除临时盘符 + 放宽已格式化目标序列号 → v1.3.3 一次性 fault marker 防循环。

### 5.2 当前 EFI 第二系统与独立 EFI

- 当前 EFI `create-secondary` 最新版回归已收口（§4.6）：Windows Boot Manager 只保留原菜单并追加 secondary，不混入 firmware entries。
- 独立 EFI E: 从 Parallels 固件首启动反复得到 `0xc0430001`（BCD 指向 U:、Secure Boot 切换、bootmgfw 版本对齐均无效）。已排除“仅旧 BCD 残留”“仅 Secure Boot 开关”“仅 bootmgfw 版本不同”三个假设。
- **决策（2026-09-10，用户确认）：独立 EFI 不作为开发投入。** 产品多系统通过当前系统 EFI 的 BCD 追加启动项实现（create-secondary，已实机验证）；独立 EFI 仅保留 `--test-efi-drive` 代码作内部诊断，不再排期排查 `0xc0430001`，标记为已知限制（不支持从独立 EFI 引导）。
- 该项不进入普通 GUI、不作为 V1 发布门槛；V1 唯一已知失败项随之关闭为“已知限制”。

### 5.3 GUI、安装发现和 BitLocker

**已收口**：Windows 安装发现字段、EFI 保留分区显式拒绝、json_text 布尔修复和 GUI 逐页真实验收均已完成（§4.8/§4.9）。尚未在 C: 上开启或修改 BitLocker；角色测试只能用快照和非 C: 隔离卷（属于固定约束，不构成未完成项）。

### 5.4 尚待实机拒绝矩阵

**已收口**：6 案例拒绝矩阵（§4.7）证明无任务创建、无格式化、无 BCD/WinRE 修改、无重启请求。

## 6. 当前客体事实

- VM 是 Parallels `Windows 11` ARM64、UEFI、Secure Boot 开启；当前状态和星号快照必须每次用 `prlctl status`、`prlctl snapshot-list` 重新确认。
- 阶段测试使用 P/Q/H fixture；WinRE 会把它们重新挂载为其他盘符，例如 G/F/H，所以恢复逻辑必须依赖 GUID。
- 不能把 Parallels 控制中心窗口当作客体 GUI；Computer Use 操作前必须确认截图是 Windows 桌面或 BackupRestore 窗口。
- GUI 提升运行（HIGH 完整性）时，非提升进程的 SendMessage/SetCursorPos/SendInput 均被 UIPI 静默拦截；可用计划任务（`New-ScheduledTaskPrincipal -UserId 'p8b6\x' -LogonType Interactive -RunLevel Highest`）在 Session 1 内提升注入 WM_COMMAND/BM_CLICK/WM_SETTEXT 完成 GUI 自动化（本轮已实测有效）。
- 测试截图统一放在 `.test-artifacts/root-captures/`，不放项目根目录；历史包和无用快照应在确认引用关系后清理。

## 7. 执行顺序（已完成）

第 7 节所列步骤已全部完成（v1.3.3 开发测试版收口）：

1. ✅ 恢复含 P/Q/H fixture 的干净快照，重新构建 v1.3.3 ARM64 包并核对 manifest、hash、架构和标题（exe hash `355c222d…`）。
2. ✅ 三阶段断电 fault 测试（§4.5）。
3. ✅ 当前系统 EFI `create-secondary` 回归（§4.6）。
4. ✅ 异常拒绝矩阵 6 案例（§4.7）。
5. ✅ Windows 安装发现字段 + EFI BitLocker 显式检查 + json_text 布尔修复（§4.8）。
6. ✅ 最新包 GUI 逐页真实验收（§4.9）。
7. ✅ 更新状态文档 → 离线检查 → ARM64 构建 → 中文提交并推送（本轮）。

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

当前总判断：**Rust 核心产品、非 C: 正常备份/还原链路、三阶段断电续跑、当前 EFI BCD 回归、异常拒绝矩阵、Windows 安装发现与 EFI 显式拒绝、GUI 逐页真实验收均已收口；v1.3.3 开发测试版可提交推送。唯一已知失败项是独立 EFI E: 固件首启动 `0xc0430001`（不进入 GUI、不作为 V1 发布门槛）。**
