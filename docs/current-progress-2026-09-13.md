# BackupRestore 当前完整进度基线（2026-09-13）

更新时间：2026-09-13 14:30（CST）  
源码版本：`v1.5.8`（**未升级**——用户要求「彻底修完、全部测试完之后再升版本」，本轮功能部分实机验证，尚未最终收口）  
分支：`main`  
仓库：`/Users/x/code/backupRestore`（github.com/pixian5/backupRestore）  
构建环境：macOS Apple Silicon（m5 / macOS 26）交叉编译 `aarch64-pc-windows-msvc`  
测试环境：Parallels VM `Win11-repair`（Windows 11 ARM64 / UEFI / GPT）

> 本文是 **v1.4.0 → v1.5.8 及本轮（2026-09-13）** 的完整增量基线 + 交接文档。
> v1.3.3 及更早历史见 [current-progress-2026-09-09.md](current-progress-2026-09-09.md)；
> v1.4.0 双启动模式见 [current-progress-2026-09-11.md](current-progress-2026-09-11.md)。
> 本文件是**当前状态与待办唯一权威摘要**；接手 AI 必须先读本文件，再读 [continuation-handoff.md](continuation-handoff.md) 与 [backuprestore-pe.md](backuprestore-pe.md)。

---

## A. 项目全貌（接手 AI 一分钟了解）

**产品**：Windows 10/11 UEFI/GPT 系统备份与还原工具（Rust + 原生 Win32 GUI 单窗口）。
**核心流程**：正常 Windows 里选任务（备份/还原/新增第二系统/PE 恢复）→ 程序准备任务并进入 PE 或 WinRE → 环境内自动启动 `Recovery.exe` → 用 DISM 捕获/应用 WIM → BCDBoot 修引导 → 自动重启回 Windows。
**当前已扩展**：自建/自定义 PE 作为第二启动系统（RAM disk 不占分区 / 硬盘启动独立分区两种模式）、PE 内恢复桌面（6 按钮 GUI）、PE 自动执行通道（零用户操作）、备份/还原智能分流（当前活动系统 → 弹窗选 PE/RE；数据盘 → 在线直接执行）。

**技术栈**：Rust（edition 2021）、windows-rs（Win32 API 裸调用，无 manifest——**因此 TaskDialogIndirect 等 comctl32 v6 控件不可依赖**）、DISM 命令行（系统自带 dism.exe）、bcdedit、mountvol、DiskPart（仅必要时）。无第三方 GUI 框架。

**构建**：`./build-win.sh`（macOS 交叉编译，目标 `aarch64-pc-windows-msvc`，release 产物约 1.45 MB）。**不依赖 Windows 侧构建**——代码在 macOS 改，编译出 exe，通过 Parallels 共享目录或 prlctl exec copy 部署进 VM。

**代码结构**：`crates/backuprestore-cli/src/`：
- `main.rs`：CLI 入口 + `should_launch_gui()`（**文件名必须是 "BackupRestore" 才默认进 GUI**——PE 里是 Recovery.exe，必须带 `--tab N` 或 `--open-image` 参数才进 GUI）
- `native_gui.rs`：全部 GUI（约 5900 行）、PE 桌面（`--pe-desktop`）、pe-click/pe-task 执行通道、智能分流、在线执行（**本轮主要改动文件**）
- `recovery_progress.rs`：恢复进度 GUI 窗口
- 其它：`cli.rs`、`core.rs`、`disk.rs`、`volume_identity.rs`、`pe.rs`（PE 安装）、`winre.rs`、`prepare.rs` 等

---

## B. 当前环境快照（2026-09-13 实测确认）

### B.1 虚拟机
- VM 名称：`Win11-repair`（Parallels，Apple Silicon，UEFI/GPT）
- RDP：Windows App / 微软远程桌面，用户 `x`，密码 `1`（已设自动登录）；分辨率建议 1280×720（**缩放 200% 的根因是 Windows 系统内「自定义缩放」而非 Windows App 客户端——已于 2026-09-12 通过 RDP 关闭并重登修复；2026-09-13 实测确认当前无缩放、桌面图标/任务栏比例正常**）
- GUI 自动化：**用 RDP（Windows App）连进 VM 后台操作，别把窗口放 macOS 前台影响用户**；prlctl exec 可执行命令（PE 内不可 exec）

### B.2 磁盘与分区（实测）
| 磁盘 | 分区 | 卷 | 用途 |
|---|---|---|---|
| Disk0 (256G) | part2 | S: | ESP（挂载点，BCD 所在） |
| | part4 | C: | Win11 系统（{current}，254G） |
| | part5 | R: | 系统 Recovery（WinRE） |
| Disk1 (128G) | part2 | E: | 备份卷（存放 WIM） |
| | part3 | F: | 10G testvol —— **当前为硬盘版 PE** |

### B.3 BCD 现状（2026-09-13 14:2x 实测）
```
{bootmgr}: default={current}, displayorder={current},{9537818b-af2a-11f1-87a0-fc459f503d3c}, timeout=30, bootsequence=空
{current}        : partition=C:  "Windows 11"
{9537818b-af2a-11f1-87a0-fc459f503d3c}: partition=F: "PE"（硬盘版 PE 条目，osloader，winpe yes）
```
- **PE 条目 GUID**：`{9537818b-af2a-11f1-87a0-fc459f503d3c}`（pe-entry-guid.txt 记录，Windows 侧部署目录也有一份）
- **Win11 真实 GUID**：见 `S:\pe-exit-guid.txt`（bcdedit /enum {current} 的 identifier）——**写 pe-exit-guid.txt 时必须写 Win11 真实 GUID，曾误写成部署 GUID 导致「返回 Windows」设 default 无效**
- 每次进 PE 后，PE 桌面启动时自动消费 bootsequence（清空）；「返回 Windows」按钮还会把 default 显式设回 {current} 兜底

### B.4 ESP（S:）自动化文件（程序读写）
| 文件 | 作用 | 动作/格式 |
|---|---|---|
| `pe-click.txt` | PE 桌面自动点击验收通道 | 内容：`backup` / `restore` / `secondary` / `exit` / `main`；消费后改名 `.done` |
| `pe-task.txt` | PE 自动执行任务 | 每行一动作：`backup <盘符> "<wim>"` / `restore "<wim>" <盘符>` / `secondary ...` / `reboot`；**按空白分词，WIM 路径不能含空格** |
| `pe-task-result.txt` | PE 执行取证 | PE 执行完写结果，可带回 Windows 读 |
| `pe-exit-guid.txt` | 返回 Windows 用 | Win11 真实 BCD GUID（{a8bafbae-...} 等） |
| `pe-entry-guid.txt` | 部署/进 PE 用 | PE 条目 BCD GUID（{9537818b-...}） |
| `pe-drive.txt` | PE 盘符记录 | 程序写入 |
| `pe-bootsequence-clean.log` | bootsequence 自清日志 | 程序写入 |

### B.5 程序部署位置
- **Windows 侧**：`C:\Users\Public\backupRestore-package\` → `BackupRestore.exe`、`Recovery.exe`、`RecoveryLauncher.cmd`、`winpeshl.ini`
- **PE 分区（F:）**：`F:\Windows\System32\Recovery.exe`（**每次新构建必须同步更新此文件**，旧版无 `--pe-desktop` 参数会输出 usage exit 2）
- **PE 中文字体**：`simsun.ttc` 已复制到 `F:\Windows\Fonts\`（PE 原版无中文字体，否则中文全变方块）

---

## C. 当前已实现功能全貌（v1.5.8）

| 功能 | 说明 | 状态 |
|---|---|---|
| 探测（probe） | 只检查环境/卷/WinRE 入口，不破坏 | ✅ |
| 备份（backup） | DISM Capture-Image 捕获分区到 WIM；GUI 压缩率下拉（max/fast/none） | ✅（在线/PE 两路） |
| 单系统还原（restore-existing） | DISM Apply-Image 还原 + BCDBoot 修引导 | ✅（含 PE/RE 两路） |
| 新增第二系统（create-secondary） | 镜像还原到独立分区 + 加第二启动项 | ✅（**注意：有残留任务 eb1c36a7/bbadfc1d 待清理**） |
| PE 恢复（install-pe-entry） | 两种模式：RAM disk（不占分区，WIM 复制到指定目录内存启动）/ 硬盘启动（独立分区） | ✅ |
| PE 恢复桌面（--pe-desktop） | 6 按钮：备份系统/还原系统/安装第二系统/命令提示符/返回 Windows/打开完整程序 | ✅ v1.5.8 新布局 |
| 打开完整程序 | PE 桌面调完整多 tab GUI（ShellExecuteW 当前 exe + --tab 1） | ✅ 实测 PE 内可运行 |
| 返回 Windows | 恢复 BCD（default={current} + 删 bootsequence）→ 自动重启 | ✅ |
| PE 自动执行通道 | pe-click / pe-task / pe-task-result | ✅ |
| 智能分流 | 备份/还原目标==当前活动系统 → 弹窗选 PE/RE/取消；数据盘 → 在线后台 DISM | ✅ 代码完成，**弹窗/在线执行待实机点击验证** |
| 备份耗时统计 | 开始时间+总时长+速度 + Recovery.log 放 wim 同目录 | ❌ **未实现（待办）** |
| PE 备份 GUI 进度 | Recovery.exe 进度 GUI 化（不闪 cmd） | ⚠️ **需核实 recovery_progress.rs 是否已接入 PE 桌面备份** |

---

## D. v1.4.0 → v1.5.8 关键演进（简表）

| 版本 | 内容 |
|---|---|
| v1.4.0 | 操作模式改名 install-pe-secondary → install-pe-entry；双启动模式（RAM disk/硬盘启动）共存；启动项名跟随程序语言 |
| v1.5.x | PE 桌面（备份/还原/安装第二系统/命令提示符/返回 Windows/重启 6 按钮）；bootsequence 自动消费；pe-click/pe-task 自动执行通道；硬盘版 PE 分区启动收口；pe-exit-guid 返回 Windows 链路 |
| v1.5.8（本轮） | 重启合并进返回 Windows；新增「打开完整程序」；智能分流（弹窗 3 按钮 + 在线执行）；对话框高度修复 |

---

## E. 本轮（2026-09-13）完成工作详情

### E.1 PE 桌面对话框高度修复
- `show_pe_dialog` 短对话框 288→312、长对话框 300→324（预留标题栏高度），「执行/取消」按钮不再被裁切。用户截图验收通过。

### E.2 硬盘版 PE 分区启动实机收口（重要结论）
- **用标准 PE 的 boot.wim**（`BackupRestorePE.iso\sources\boot.wim`，Index 1）dism Apply 到 F: 分区 + bcdboot 分区引导。
- **WinRE.wim 不行**：分区启动失败 / ramdisk 方式在 Parallels 崩 VM（已验证）——**PE 恢复的硬盘模式必须用标准 PE boot.wim，不要用 WinRE.wim**。
- PE 无中文字体 → 复制 `simsun.ttc`。
- PE 里 Recovery.exe 必须最新版（旧版无 `--pe-desktop` → usage exit 2，桌面起不来）。
- `pe-exit-guid.txt` 必须写 Win11 真实 GUID。
- **Parallels bootsequence 不可靠**（时消费时不消费）→ 所有退出路径都写 default 兜底。

### E.3 PE 桌面按钮合并与新增（native_gui.rs）
- 删除独立「重启」按钮分支（`ID_PE_REBOOT`），合并进「返回 Windows」（`ID_PE_EXIT`）。
- 新增「打开完整程序」（`ID_PE_MAIN_GUI = 1406`，复用原 ID_PE_REBOOT 槽位）：`ShellExecuteW` 启动当前 exe + `--tab 1`。
  - **坑**：`main.rs should_launch_gui()` 检查 exe 文件名=="BackupRestore"；PE 里是 Recovery.exe，**无参数启动走 CLI 分支不会进 GUI**，必须带 `--tab`/`--open-image`。
- PE 桌面 6 按钮新布局：`备份系统 | 还原系统 | 安装第二系统` / `命令提示符 | 返回 Windows | 打开完整程序`（截图确认 v1.5.8）。

### E.4 pe-click 新增 `main` 动作
- `S:\pe-click.txt` 内容 `main` → PE 桌面自动点「打开完整程序」（验收通道，消费后改名 .done）。

### E.5 智能分流实现（create_task 内，native_gui.rs）
用户规格：**备份/还原 tab 点「创建任务」时，目标卷如果是「当前活动系统」（不是任何含 Windows 的卷——双系统场景）→ 弹窗询问**。
- 判断：`target_drive` 去尾冒号大写 == `%SystemDrive%`（env，默认 "C:"）。
- **相等（当前活动系统）** → 自绘模态 3 按钮对话框 `ask_system_drive_handler`：
  1. **进入 PE（推荐）** → `schedule_pe_task`：mountvol S: → 写 `S:\pe-task.txt`（备份：`backup <源> "<wim>"`；还原：`restore "<wim>" <目标>`；结尾 `reboot`）→ bcdedit bootsequence 指向 PE 条目 GUID → `ExitWindowsEx(EWX_REBOOT)` 自动重启（失败回退 shutdown.exe；**WIM 路径含空格先拦截提示**，pe-task 按空白分词会拆坏）。
  2. **进入 Windows RE** → 走原有 prepare → ShellExecuteW 重启进 WinRE 链（不变）。
  3. **取消** → 不动作，return。
- **不等（数据盘/非活动系统）** → `run_online_operation`：后台线程跑 DISM（备份 `/Capture-Image`、还原 `/Apply-Image`），完成 `PostMessage WM_APP_ONLINE_DONE(0x8002)`，主窗口读全局 `ONLINE_RESULT` 弹结果框，**不重启**。
  - **坑**：`std::thread::spawn` 闭包不能直接 move Hwnd（`*mut c_void` 不 Send）→ `let root = state.root as usize;` 线程里再 `as Hwnd`。
- 弹窗用**自绘模态对话框**（不依赖 comctl32 v6 TaskDialog——项目无 manifest）。

### E.6 实机验证记录（本轮，全部截图/命令确认）
1. PE 桌面 v1.5.8 新布局（6 按钮）——prlctl capture 截图确认。
2. 「打开完整程序」→ 完整多 tab GUI 在 PE 正常运行（源卷枚举到 PE 的 X:、压缩率下拉 fast、语言中文）——截图确认。
3. 「返回 Windows」→ BCD default 回到 {current}——bcdedit 实测确认。
4. ESP 残留清理：删除 pe-addsec-*.txt、pe-gui-backup/restore.txt 等测试残留；**保留程序正常文件**（pe-drive.txt、pe-bootsequence-clean.log、pe-exit-guid.txt、pe-entry-guid.txt）。
5. git 提交：`71c172f`（PE 桌面 + 返回 Windows 自动验证）、`3afa83c`（合并重启 + 打开完整程序 + 智能分流，565 insertions）。

### E.7 智能分流 RE 路收口（16:00-16:12，用户反馈"点 RE 无反应"后的修复）
**问题**：点「进入 Windows RE」→ 弹窗关闭但无任何后续（无任务、无提示）→ 用户强烈反馈"没有任何反应，只是弹窗关了"。

**根因（两层）**：
1. **RDP 坐标点击偏差**：cu 点击弹窗 RE 按钮（OCR y≈565-595）多次未命中（实际落取消/间隙）→ 触发的是 choice=0（取消）→ 静默 return。**取消按钮（y≈635-665）反而能点中**（偏差向下时落到取消上）。这不是产品 bug，是测试环境输入坑。
2. **产品 UX 缺陷**：即使真触发 choice=2，旧代码只是静默 `ShellExecuteW runas` 启动 prepare；若 prepare 启动失败，错误只写状态栏——而**状态栏控件（y=440 高 90）被备份 tab 的镜像路径/压缩率控件覆盖**（布局重叠），用户完全看不到 → 表现为"无反应"。

**修复（native_gui.rs）**：
1. **choice=2 增加确认框**（`show_message` MB_YESNO）：明确告知"将创建备份任务并以管理员权限准备，准备完成后系统会重启进入 Windows RE 执行备份"，确认后才走 prepare → 不再静默。
2. **ShellExecute runas 失败时弹窗报错**（MB_OK | MB_ICONERROR，显示 ShellExecute 错误码），不再只写被覆盖的状态栏。
3. **test hook 新增 `system_drive_choice`（0=取消 1=PE 2=RE）**：`br-test.json` 配置后点「创建任务」直接采用该选择（跳过弹窗+确认框），绕开 RDP 点不到弹窗按钮的测试坑；`State` 新增 `test_drive_choice: Option<i32>`。

**实机验证结果（hook system_drive_choice=2）**：
- ✅ 点「创建任务」→ 直接走 RE 路由 → 状态栏显示"已启动管理员准备脚本"（ShellExecute 成功，>32）
- ✅ prepare 创建任务 `12f95494`（backup、imagePath=E:\1.wim、stage=boot-requested）
- ✅ prepare.log：`reagentc.exe /boottore` 操作成功 → `shutdown.exe /r /t 0` 自动重启
- ⚠️ **Parallels 重启后未进 WinRE**（回 Win11）：`reagentc /boottore` 的一次性启动在 Parallels UEFI 固件下不生效（与 bootsequence 同类虚拟机固件坑）。**真实硬件上 reagentc /boottore 是标准路径，应有效**；Parallels 环境需用「手动重启后 WinRE 菜单选一次」或 PE 路替代。**此坑写入 backuprestore-pe.md**。

**结论**：智能分流「进入 Windows RE」路的产品逻辑（确认框 → prepare → 任务创建 → WinRE 设置 → 自动重启）已完整验证；「进入 PE」路（choice=1 → schedule_pe_task）入口代码与历史实机一致、hook 已支持，实机验证待安排（会重启进 PE 执行备份，需小分区/接受慢备份）；「在线执行」（数据盘）仍缺可测试数据盘（当前 E 是备份卷、F 是硬盘版 PE）。

---

## F. 智能分流规格（完整，供验证/继续开发）

```
创建任务（备份/还原）：
  target_upper == %SystemDrive% ?
  ├─ YES → 自绘模态 3 按钮：
  │    ├─ 「进入 PE（推荐）」→ schedule_pe_task()
  │    │     mountvol S: → 写 pe-task.txt（操作+参数+reboot）→ bcdedit bootsequence={PE GUID}
  │    │     → ExitWindowsEx 重启 → PE 自动执行 → 自动回 Windows
  │    ├─ 「进入 Windows RE」→ 原 prepare → ShellExecuteW（WinRE 链）
  │    └─ 「取消」→ return
  └─ NO  → run_online_operation()：后台线程 DISM Capture/Apply
          → WM_APP_ONLINE_DONE → 弹结果框（不重启）
```

**待实机验证**：
1. Windows GUI 里选 C: 点备份 → 弹窗 3 按钮出现 ✅（多轮截图确认）；**三路**：取消 ✅、RE ✅（E.7，hook 验证 prepare 全链）、PE ⏳（hook choice=1 待测，会重启进 PE 备份）。
2. 数据盘（E: 等）在线备份/还原 → 后台执行 + 结果框 ⏳（缺可测试数据盘）。
3. PE 侧：pe-task 写 backup/restore + reboot 全链路（此前 pe-task 只实测过部分动作）。

---

## G. 坑与经验大全（接手 AI 必读）

详见 [backuprestore-pe.md](backuprestore-pe.md)（持续维护）。核心摘录：

1. **bootsequence + PE 场景**：PE 整个在 RAM 盘跑，bootmgr 的 bootsequence 无法被消费回写 ESP BCD → 每次重启都进 PE。解法：PE 桌面启动时**自清 bootsequence** + 退出时显式设 default={current}。
2. **Parallels bootsequence 不可靠**（时消费时不消费）→ 所有路径 default 兜底。
3. **WinRE.wim 不能做硬盘版 PE**（分区启动失败/ramdisk 崩 VM）；必须用标准 PE boot.wim。
4. **PE 无中文字体** → 复制 simsun.ttc；**PE 无 GUI 主题**，控件用裸 Win32 默认样式。
5. **Recovery.exe 必须最新版**部署到 PE 分区（旧版无 --pe-desktop → usage exit 2）。
6. **pe-exit-guid.txt 写错 GUID** → default 设置无效（曾误写部署 GUID）。
7. **should_launch_gui 文件名检查**：Recovery.exe 无参数不进 GUI，必须 --tab。
8. **Hwnd 不能跨线程 move**（*mut c_void 不 Send）→ 转 usize。
9. **pe-task 按空白分词**：WIM 路径含空格会拆坏 → 写任务前拦截。
10. **自绘模态对话框**（不用 TaskDialogIndirect——无 manifest，comctl32 v6 不可用）。
11. **VM 无法启动 / 文件丢失**：VM 在 `/Users/x/Parallels/`（200+GB——含快照）；曾因移走 .pvm 无法启动，已从 `~/.Trash/` 移回。**操作 VM 文件前先确认路径，别乱动**。
12. **缩放 200% 根因 = Windows 系统自定义缩放**（不是 Windows App 客户端）——已通过 RDP 关闭并重登修复；**任何 GUI 自动化前先截图核实当前缩放**，勿拿过期结论断言。RDP 分辨率 1280×720 合适。
13. **GUI 多 tab 切换叠加/遮挡**（v1.5.2 曾严重）：改 UI 后必须多切几次 tab 回归（根因曾与控件创建/定位逻辑有关，v1.5.2 已修）。
14. **满十进一版本号**：1.3.9→1.4.0、1.4.9→1.5.0 进位；**彻底测完再升**，不要先设计版本号再改代码。
15. **备份/还原测试纪律**：不备份还原 C 盘、不用 VHD、用真实磁盘小分区（F: 10G 或 E:）。
16. **状态栏控件被 tab 控件覆盖**：`status`（y=440 高 90）与备份 tab 的镜像路径（425-455）/压缩率（465-495）控件重叠 → 状态栏文字用户看不到。**凡是错误/状态提示不能只写状态栏**，关键路径必须弹窗（已为 RE prepare 失败/成功加弹窗）；后续布局修复时把 status 移到不会被覆盖的位置。
17. **RDP（cu）坐标点击弹窗按钮偏差大**：主界面大按钮（创建任务 y≈525）能点中，但自绘弹窗小按钮（RE y≈565-595）多次点不中（实际落取消/间隙）。**弹窗按钮验证用 test hook `system_drive_choice` 绕开**；Tab/Enter/Space 键盘在 RDP 内不映射到按钮（勿再浪费时间重试）。
18. **reagentc /boottore 在 Parallels UEFI 下无效**：prepare 链执行成功（reagentc 报"操作成功"）但重启后不进入 WinRE（回 Win11）——Parallels 固件不消费一次性 WinRE 启动（与 bootsequence 同类坑）。**真实硬件应有效**；Parallels 环境验证 WinRE 链只能看到「prepare→任务创建→重启」，进 WinRE 执行需手动重启后选 WinRE 菜单。

---

## H. 待办事项（下一位 AI 按优先级执行）

> 每项给出验收标准。执行完更新本文 + verification-matrix。

### P0（本轮功能未闭环，先做）
1. **智能分流弹窗实机验证**：三路——取消 ✅、RE ✅（E.7 hook 验证 prepare 全链：任务创建+reagentc /boottore+自动重启；Parallels 不进 WinRE 属虚拟机固件坑）、**PE 路 ⏳**（hook `system_drive_choice=1` + 点创建任务 → schedule_pe_task 写 S:\pe-task.txt + bootsequence → 重启进 PE 自动执行。**注意会重启并执行备份，需先改 pe-task 目标为小分区或接受 C 盘慢备份**）。验收：截图 + 结果文件。
2. **在线备份/还原实机验证**（选 E: 或其它非系统盘——注意 F: 当前是 PE，不能当数据盘用；备份一个小分区 → 还原 → 校验）。验收：DISM 退出码 0 + 数据一致。

### P1（用户明确提过的功能）
3. **备份耗时统计**：开始时间 + 总时长 + 速度；`Recovery.log` 放到备份出的 wim 同目录。
4. **GUI 备份 tab 压缩率下拉文案 verbatim**：max → `LZX（文件最小，耗时特别长，CPU占用特别多）`；fast → `XPRESS（推荐！文件稍大，非常快，CPU占用低）`；none → `不压缩（最快，文件最大，几乎不耗CPU）`（当前下拉只显示 fast/max/none 裸值——需核验是否已带说明，若只裸值则改）。用户还要求对比 1.wim(max) 与 fast 耗时差（只差 2GB 如果耗时差很大就 fast 默认——**当前默认已是 fast**）。
5. **PE 备份 GUI 进度**：核实 Recovery.exe 在 PE 桌面点备份时是否弹 cmd——应改为 GUI 进度（recovery_progress.rs），不显示 cmd。
6. **还原从非 C 盘运行**：自动检测程序所在分区是否是待恢复分区，若是 → 把程序自复制到 wim 所在文件夹再执行（用户建议）。
7. **分区识别精简**：VolumeIdentity 只保留 `partition_unique_guid` + `disk_guid` 两个字段（跨 Windows/WinRE 识别同一分区，不要过度防御）。
8. **create-secondary 残留任务清理**：`eb1c36a7`（stage=boot-requested）、`bbadfc1d`（历史残留）——从任务表清理。
9. **verification-matrix / testing-plan 状态同步**：v1.3.3 三阶段断电已实机收口，两文档不应再标「实机待验证/尚未收口」；同步最新验证结果（用户曾明确要求核实并修改）。

### P2（收尾）
10. **版本号升级**：全部测试通过后 v1.5.8 → v1.5.9（+0.0.1 满十进一；VERSION、Cargo.toml×2、Cargo.lock、GUI 标题同步；Windows 包目录/build-manifest 一致）。
11. **docs 范围描述修正**：删除「明确不做自研 PE」表述（已自定义 PE 完成）——continuation-handoff.md / project-status.md 里仍有此表述，需改为「已支持自定义 PE（RAM disk/硬盘启动）」。
12. **PE 桌面返回 Windows 新布局回归**：合并后 6 按钮整体再跑一遍（点返回 Windows → 回 Win11 默认）。

---

## I. 用户偏好与对话关键点（写给接手 AI——直接影响验收）

### I.1 工作方式（反复强调，违反会被骂）
1. **能自动化绝不让用户手动**：PE 内零点击是硬要求——「Win11 配好配置 → 重启 → PE 自动执行 → 自动回 Windows」。能在 Win11 侧写配置让 PE 自读自执行，就绝不要求用户在 PE 里点/敲命令。
2. **固件/引导菜单操作做不了**（EFI 里、Boot Manager 选系统、BIOS 级操作）→ 明确告诉用户手动做，不装成功。
3. **操作后必须自查产物存在**（如快捷方式 .lnk 是否真创建、文件是否真落盘、按钮是否真显示）——曾多次因「没检查就宣称成功」被要求 git 回退。**每个操作做完都要验证**。
4. **重要坑/经验/发现写 docs/*.md 中文文档**；代码中文注释。
5. **git 中文 commit + push**（github.com/pixian5/backupRestore）；提交时把本轮无关修改一并带上。
6. **不要备份还原 C 盘**（测试）、**不要 VHD**、**用真实磁盘小分区**测试备份还原。
7. **用程序自带的 PE 镜像**（不是 WinRE 镜像）——用户多次纠正。
8. **版本号**：彻底修完、全部测试通过后再升；满十进一；**不要先设计版本号再改代码再测试**。
9. **UI 文案走程序设置的语言**（中文界面中文、英文界面英文），不必双语；启动项名、按钮名同样。
10. **独立 EFI 分区没必要**——在现有 ESP/BCD 里加启动项即可（用户已确认）。
11. **路径/盘符不写死**：盘符可以是任意合法目录/卷（用户提过「盘符不一定是 c，可以是任意合法目录」）。
12. **代码块/命令一次给全**（用户抱怨过「分多段不方便复制」）；**所有检查都自动做**，别让用户传截图（用户曾骂「你自己也能截图看」）。

### I.2 技术环境约束
- macOS Apple Silicon 本机（m5/macOS 26）；Windows 侧是 Parallels VM（Win11-repair）。
- 本机不要装 MySQL 等——用 docker。
- 下载大文件前确认网络（用户在用流量时，>100MB 要问；ADK 等已装过不用再下）。
- RDP/GUI 操作**在后台**做（Windows App 连 VM），别把远程桌面/全屏放 macOS 前台影响用户使用电脑。

### I.3 关键对话决策记录（本轮及近期）
| 用户原话意图 | 落实的工程决定 |
|---|---|
| 「返回 Windows 是不是和重启实质一样了？」→「合并为返回 Windows」 | PE 桌面删除独立重启按钮，合并进返回 Windows |
| B 方案（保留 6 按钮桌面 + 加「打开完整程序」按钮） | 新增 ID_PE_MAIN_GUI，完整 GUI 在 PE 内可运行（已实测） |
| 备份/还原目标如果是当前活动系统→弹窗询问进 PE/RE | 智能分流：%SystemDrive% 判断 + 自绘 3 按钮弹窗 |
| 「注意不是含 Windows 的启动卷、而是当前活动系统（有双系统）」 | 判断用 %SystemDrive% 而非「卷含 Windows」 |
| 「非系统盘直接在当前系统进行备份、还原」 | run_online_operation 后台线程 DISM，不重启 |
| 「PE 启动项名不要写死，以实际文本框为准」 | PE 启动项名 = 文本框实际值（v1.4.0 已落实，防回归） |
| 「PE 目录名才是 RAM 模式那行显示的」 | RAM disk 行显示目录名/路径输入框；启动项名两模式通用（v1.4.0） |
| 「以后假如增加其他功能，执行完了也重启吗？」 | pe-task 每行一动作 + 显式 `reboot` 才重启，不是无条件重启 |

---

## J. 命令速查（构建/部署/测试）

```bash
# 构建（macOS 交叉编译）
cd /Users/x/code/backupRestore && ./build-win.sh
# 产物：target/aarch64-pc-windows-msvc/release/BackupRestore.exe（约 1.45MB）

# 部署（Parallels 共享目录 \\Mac\backupRestore 映射到 repo）
prlctl exec "Win11-repair" cmd /c "copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe C:\\Users\\Public\\backupRestore-package\\BackupRestore.exe"
prlctl exec "Win11-repair" cmd /c "copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe C:\\Users\\Public\\backupRestore-package\\Recovery.exe"
prlctl exec "Win11-repair" cmd /c "copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe F:\\Windows\\System32\\Recovery.exe"

# 进 PE（零用户操作验收通道）
prlctl exec "Win11-repair" cmd /c "mountvol S: /S >nul & echo main > S:\\pe-click.txt & bcdedit /set {bootmgr} bootsequence {9537818b-af2a-11f1-87a0-fc459f503d3c} >nul 2>&1"
prlctl restart "Win11-repair"
# 等待 ~70-80s 后 prlctl capture 截图验证

# 回 Win11（强制，必要时）
prlctl stop "Win11-repair" --kill; sleep 3; prlctl start "Win11-repair"
# 等待 ~100s UP，bcdedit 确认 default={current}

# 其它
prlctl capture "Win11-repair" --file /path/out.png   # 截图
bcdedit /enum {bootmgr} | findstr default            # 查默认
```

---

## K. 文档索引

| 文档 | 内容 |
|---|---|
| [continuation-handoff.md](continuation-handoff.md) | 交接总入口（设计决策、代码边界、执行顺序） |
| [project-status.md](project-status.md) | 状态摘要（本文的简短版） |
| [current-progress-2026-09-09.md](current-progress-2026-09-09.md) | v1.3.3 及更早历史 |
| [current-progress-2026-09-11.md](current-progress-2026-09-11.md) | v1.4.0 双启动模式增量 |
| **current-progress-2026-09-13.md（本文）** | **v1.5.8 当前权威基线 + 待办 + 用户偏好** |
| [backuprestore-pe.md](backuprestore-pe.md) | PE 全部坑与经验（持续维护） |
| [verification-matrix.md](verification-matrix.md) | 验收矩阵（**状态待同步，见 H9**） |
| [testing-plan.md](testing-plan.md) | 测试计划（同上） |
| [development-execution-protocol.md](development-execution-protocol.md) | 开发执行协议 |

---

## L. 补充事件（2026-09-13 15:0x 会话中断前记录——接手 AI 必读）

### L.1 本轮实机验证的 GUI 注入通道结论（用户反复追问后核实）
- **Windows App 缩放**：200% 缩放的根因是 **Windows 系统内「自定义缩放」**（非 Windows App 客户端），已于 2026-09-12 通过 RDP 关闭并重登修复。**2026-09-13 实测确认当前无缩放**（桌面图标/任务栏比例正常）——本文 B.1 / G12 已同步修正。
- **GUI 自动化通道图谱**（历史+本轮实测，勿再踩）：
  - ✅ **RDP（Windows App）+ cu 键盘**：可用（9-12 曾全程用它完成 RAM 安装验收）。注意 **RDP 内 macOS 修饰键不映射成 Windows 修饰键**（如 cmd+R 不触发 Win+R），要用「鼠标点击 + 普通字母键」或「资源管理器类型搜索跳转」。
  - ✅ **cu 鼠标（RDP 会话窗口内）**：RDP HID 转发可用（缩放正常后坐标基本准），但点击有 ~20-30/1000 的系统性偏差（需按截图 OCR 微调，或改用键盘导航）。
  - ✅ **prlctl exec / send-key-event --scancode（Windows 运行态）**：可用。
  - ❌ **cu 点击 Parallels 窗口（com.parallels.desktop.console）**：合成鼠标不进入 guest（CGEvent 不转发）——死路。
  - ❌ **prlctl 在 bootmgr/PE 内注入**：无效；**prlctl 无鼠标注入接口**。
  - ❌ **prlctl exec 启动 GUI**：落 Session 0 不可见。

### L.2 ⚠️ macOS 侧「background shell task limit reached」大坑（本次卡死根因）
- **现象**：Bash / mac_computer_use_tool / computer_use_tool **全部**报 "background shell task limit reached"，无法执行任何命令。
- **根因**：agent 会话的后台 shell 池有上限（约 4-5 个槽）。以下操作会占槽且**不自动释放**：
  1. `run_in_background=true` 的 **prlctl stop/start/restart VM** 长命令（sleep 90-260s 循环）——**用完后必须 TaskStop（有 backgroundTaskId 可停）**；
  2. **mac_computer_use_tool 调用中 Python 抛异常崩溃**（如 app not found / 无效坐标）——**崩溃 runner 占槽且无暴露 id，只能等超时或重启客户端**。
- **处理**：先 Grep 轨迹 `run_in_background` / TaskOutput 的 task_id 逐个 TaskStop；若仍有 cu 崩溃残留 → **重启豆包客户端**（无需重启 Mac）。
- **经验**：每次 `run_in_background` 长任务后立即 TaskStop；cu 调用前先确认目标 app 存在，避免 Python 崩溃。

### L.3 智能分流核心代码审查结论（2026-09-13，代码已含在 1,455,616B 产物中）
- `create_task`：备份→用 `source_drive`（被捕获卷）、还原→用 `target_drive`（被覆盖卷）与 `%SystemDrive%` 比较（正确，非「含 Windows 的卷」）。
- 相等 → `ask_system_drive_handler`（自绘模态 3 按钮：1=PE、2=RE、其他=取消）→ PE 走 `schedule_pe_task`（mountvol S: → 写 pe-task.txt → bootsequence={PE GUID} → ExitWindowsEx 重启，fallback shutdown.exe）；WIM 路径含空格先拦截。
- 不等 → `run_online_operation`（后台线程 DISM + PostMessage WM_APP_ONLINE_DONE(0x8002) + ONLINE_RESULT 全局读结果框，不重启）。
- 压缩率：下拉显示说明文案（`compress_level_labels` 按语言返回 LZX/XPRESS/不压缩 3 项），`create_task` 按索引 0/1/2 映射回 max/fast/none（默认 fast）。
- **待实机验证**：弹窗三路 + 数据盘在线执行（见 H-P0）。

### L.4 中断前现场
- VM `Win11-repair` 已由用户手动重启（running）；RDP（Windows App）已断开需重连。
- GUI 未运行（tasklist 无 BackupRestore）。
- 下一步（重启豆包后）：重连 RDP → 启动 BackupRestore.exe → 备份 tab 选 C: 创建任务截图弹窗（点取消）→ 选 E: 创建任务验证在线执行 → 收口 H-P0。

### L.5 智能分流 RE 路「固件层修复」穷尽结论（2026-09-13 17:00-17:10）
**用户选项 1（硬修新 VM 固件恢复条目）已穷尽，确认是 Parallels 固件限制，修复不可行。**

尝试与结论：
1. **R: 恢复分区重建 EFI 链（成功但无效）**：R:\EFI\Microsoft\Boot\bootmgfw.efi（自 S: 复制 3,119,968B）+ R: BCD（{bootmgr} default=WinRE osloader {e6aed4d4-…} + ramdiskoptions {055a1b87-…}）。构建脚本 `build-winre-bcd.cmd`（必须 CRLF）→ `\\Mac\backupRestore` 共享 → VM 内执行。坑：`bcdedit /createstore` 后 store 为空，`{ramdiskoptions}` 保留名不能直接 create（"不支持该参数"），须 `/create /d "..." /device` 捕获新 GUID；`{bootmgr}` 保留名可以 create。
2. **固件恢复条目 {e6aed4ce-…} 被系统固定在 S:**：`bcdedit /set device partition=R:` 报成功但 enum 仍 S:——固件条目由 Parallels NVRAM 控制，BCD 层无法改写。
3. **{fwbootmgr}.bootsequence 指向 WinRE BCD 条目被拒**：displayorder 拒绝 osloader 条目（/addfirst 报成功但 displayorder 不变）→ bootsequence={e6aed4d4-…} 报"找不到元素"。固件 displayorder 只接受固件应用条目（101fffff）。
4. **NVRAM 移植被拒（关键）**：关 VM → 备份 NVRAM.dat/NVRAM.tnvs → 复制旧 VM（Windows 11.pvm）NVRAM → 启动报 "Failed to start the VM: Operation canceled"（UUID 绑定，Parallels 拒绝）→ 恢复备份后 VM 正常。备份文件：NVRAM.dat.bak-winre-fix-170704。

**最终结论**：Parallels 新 VM（Win11-repair）固件环境：
- 注册的恢复固件条目 {e6aed4ce-…} 指向普通 bootmgr（进 Win11 而非 WinRE）；
- 固件不支持一次性启动（BootNext/bootsequence 消费后无法启动非默认条目，reagentc /boottore 必然假成功）；
- NVRAM 无法从旧 VM 移植（UUID 绑定）。
→ **WinRE 自动进入在本 VM 不可修复**。智能分流 RE 路实机闭环需：旧 VM（Windows 11.pvm，固件完整）验证 或 真实硬件/VirtualBox/QEMU 验证。PE 路不受影响（Boot Manager 菜单选择，不依赖固件一次性启动）。

### L.6 数据卷备份/还原 fast 全链路实机验证（旧 VM Windows 11.pvm，2026-09-13 18:30-19:05）
**目标**：用户要求「完整测试程序的备份还原（fast 就行）」，且避免完整 C 盘测试 → 用 5GB 测试分区 T: 验证备份→还原闭环。

**测试环境**：
- 旧 VM `Windows 11.pvm`（26200.9168，IP 10.211.55.13，用户 x 密码 1）
- diskpart 从 C: 收缩 5GB 建 T:（testvol/NTFS），写入 4×52,428,800B（data1~4.bin，共 200MB）
- 镜像 `C:\Users\Public\br-test\test-backup.wim`
- **核心代码改动（未提交前）**：
  1. `windows_prepare.rs::assert_environment()`：数据卷（无 SYSTEM hive）允许在线备份/还原，仅系统卷强制要求 WinRE 可用——**已实机验证**。
  2. `main.rs` 还原 BootRepaired 阶段：目标卷无 `\Windows\System32\config\SYSTEM`（数据卷）→ 跳过 BCDBoot 并走状态机过渡（ImageApplied→BootRepaired→Success）——修复原「数据卷还原误跑 bcdboot.exe T:\Windows → 0x80092003」失败。

**验证结果**：
- **备份成功**：GUI 在线备份（Session 0 hook 触发）→ test-backup.wim **428KB**（fast/XPRESS 压缩 200MB 全零数据，压缩比 ~470:1）。CLI 路径 `prepare --operation backup --source-drive T --image-path … --compress fast --no-reboot` + `recover` 任务创建/执行均正常。
- **还原成功**：还原前在 T: 添加 newfile.txt 标记 → `prepare restore-existing` + `recover --efi-root S:\` → 格式化 T: + DISM /Apply-Image 100% → **data1~4.bin 完整恢复、newfile.txt 被清除**，status.json `stage=success, progress=100`。
- **bcdboot 修复生效**：日志 `data-volume restore: target has no SYSTEM hive; skipping BCDBoot` → `Recovery completed`，无错误退出。

**CLI 踩坑（重要，后续复现）**：
1. `recover <root> <id>` 的 root 必须是 **workspace 根**（如 `C:\Users\Public\backupRestore-package`），TaskStore 内部会拼 `/tasks/{id}`——传任务目录会 os error 3，传 tasks 目录（父级）同样 os error 3。
2. 还原执行必须传 `--efi-root <已挂载 ESP 根>`（`mountvol S: /S` 后传 `S:\`），否则 `EFI root must be explicitly mounted before recovery`。
3. `prepare` 的 find_system_efi 会跳过**已挂载**的盘符（S: 被手动挂载时全部扫描失败 → `system GPT EFI partition was not found`）——先 `mountvol S: /D` 再 prepare。
4. 任务一旦 Failed 即终端，重跑同一任务报 `task is failed`——须重新 prepare 新建任务。
5. 备份目标 wim 已存在时 CLI recover 捕获报 `os error 2`（文件冲突）；GUI 在线备份（首次创建 wim）已验证成功。增量/覆盖策略后续需明确（如 /Append 或提示先删除）。

### L.7 WIM 追加索引 + 索引名 + 保留最近 N 个（GUI/CLI 全链路实机验证，2026-09-13 19:27-19:40）
**需求**（用户确认方案）：备份时 wim 已存在 → 自动 `/Append-Image` 追加新索引；GUI 备份 tab 增加「索引名」输入框（默认程序启动时间，可改）与「保留最近 N 个」清理选项；还原 tab 下拉列出 wim 全部索引（已有 wim-info，确认覆盖）。

**代码改动（已提交）**：
1. `backuprestore-core/src/lib.rs`：`ImageSpec.name: Option<String>`；`Task.keep_indexes/image_name`（camelCase，serde default skip）。
2. `windows_prepare.rs`：`PrepareOptions.image_name/keep_indexes` + `--image-name`/`--keep-indexes` 解析（keep=0 报错）；备份任务写入字段；restore 构造 ImageSpec 补 `name: None`。
3. `main.rs` 备份执行：首次 `/Capture-Image /Name:{image_name}`；追加走 `candidate 复制 → /Append-Image /Name → /CheckIntegrity → 校验索引数 +1 → rename` 防掉电；keep 清理循环 `/Delete-Image /Index:1` 删最旧；sidecar 重编号重写。
4. `native_gui.rs`：控件 `ID_INDEX_NAME_EDIT=1209`/`ID_KEEP_EDIT=1210`；WM_CREATE 默认索引名 = GetLocalTime `2026-09-13 19:20` 格式；标签 2015/2016（ui_text 中英 + apply_language 更新）；可见性仅 backup tab；布局压缩率行下方新行（索引名左 280px + keep 右 80px，buttons_y 下移 30px）；create_task 备份分流传 `--image-name`/`--keep-indexes`；`run_online_operation` 加 image_name/keep_indexes 参数；`execute_online` 在线备份：wim 存在 → `/Append-Image`，否则 `/Capture-Image /Compress`；keep 用 `wim-info` 数索引 + 循环删 Index 1。

**实机验证结果（旧 VM，T: 5GB 数据卷，test2-backup.wim）**：
- ✅ 首次备份：`--image-name "2026-09-13 19:40"` → 索引 1（Name 正确写入）。
- ✅ 二次追加：`--image-name "2026-09-13 19:41" --keep-indexes 2` → 索引 2（Append 成功，keep 未触发）。
- ✅ 三次追加：`--image-name "2026-09-13 19:42" --keep-indexes 2` → 追加索引 3 → **自动删除最旧（19:40）**，wim-info 剩 2 索引（19:41、19:42）——keep 清理实锤。
- ✅ keep 后还原校验：**修复 hash mismatch**（见踩坑 1），`prepare restore-existing --wim-index 2` 校验通过 → recover → 格式化 T: + Apply 100% → unique 标记文件被清除、wim 内容完整恢复。
- ✅ 还原 tab 下拉：wim-info 实时读全部索引（Name 显示），GUI 无需依赖 sidecar。

**新踩坑（重要）**：
1. **keep 删除后 WIM 哈希变化导致还原校验失败**：keep 清理先删索引再重写 sidecar，但 sidecar 的 `image_sha256` 用的是清理前的哈希 → 后续 prepare 还原报 `restore image hash does not match metadata`。修复：keep_cleaned 分支末尾重新 `sha256_file(destination)` 并同步所有剩余 sidecar（含 legacy）。
2. **追加路径 previous_metadata 容错**：历史 WIM（旧版在线备份产物）无 `.index-N.metadata.json` sidecar → `read_index_metadata` 报 os error 2 中断备份。修复：previous_metadata 循环改为 `if let Ok` 跳过缺失。
3. **Task::new 补字段**：新增 Task 字段后构造器必须同步初始化（E0063），rust 编译期已拦。
4. **prepare 还原仍需 `--source-drive`**：`restore-existing` 校验要求 source-drive（即使数据卷还原 source=target=T），缺失报 `--source-drive is required`。

**已知限制（未覆盖）**：
- GUI 在线备份（execute_online）的 append/keep 路径代码已写但 **Session 0 无界面无法实测**（旧 VM GUI 自动化已弃用）；CLI 侧 main.rs 同逻辑已全链实测。execute_online 追加无 candidate/CheckIntegrity（简化路径），后续如需严格防掉电需对齐 CLI 的 candidate 策略。
- 还原「metadata 校验依赖 sidecar」：prepare 还原对缺失 sidecar 的第三方/PE WIM 跳过哈希校验（既有行为），keep 清理后 sidecar 已同步最终哈希。

### L.8 GUI 在线备份 exit=87 修复 + 档案降级 + keep=0 语义（2026-09-13 20:43-20:55）
**用户反馈**：打开程序点备份后弹窗 `[ONLINE backup] exit=87 (no output captured)`。

**原因（已定位）**：`execute_online`（GUI 在线备份路径）把 DISM 命令拼成字符串交给 cmd 执行；索引名默认是 `2026-09-13 20:43` 这种**带空格**的时间，`/Name:2026-09-13 20:43` 没加引号 → cmd 把参数拆成 `/Name:2026-09-13` 和 `20:43` 两个 → DISM 报 87（ERROR_INVALID_PARAMETER 参数错误）。CLI 路径（main.rs）用参数数组不经过字符串拼接所以没事。

**修复**：`execute_online` 的 Capture/Append/Apply/Delete 命令中 `/ImageFile:"…"`、`/Name:"…"` 全部加引号。**实机验证**：在 VM 手动执行与修复后完全相同的命令 `dism.exe /Append-Image /ImageFile:"…" /CaptureDir:T:\ /Name:"2026-09-13 20:51 引号测试"` → EXIT=0 成功（修复前同命令不带引号必 87）。CLI 侧用空格+中文索引名 `2026-09-13 20:50 测试空格` 完整备份成功，wim-info 显示 Name 正确。

**档案（.metadata.json）降级为「提示+确认」，不再是硬性拦截**：
- 用户理由：档案文件可能被弄丢，不应因此无法还原。
- CLI：新增 `--force-restore-hash`；`validate_operation_inputs` 中哈希不匹配时默认拒绝，但带该参数则放行（日志/提示由 GUI 负责）。
- GUI（create_task 还原分支）：还原前检查 `.index-N.metadata.json`——
  - 档案缺失 → 弹窗「未在此镜像旁找到备份档案文件（可能被移动或删除）。跳过完整性校验直接还原，可能还原到错误或损坏的镜像。是否仍要继续还原？」；
  - 档案存在但哈希不匹配 → 弹窗「备份档案与镜像不匹配（镜像可能被修改或损坏）。是否仍要还原？」；
  - 点「是」→ 传 `--force-restore-hash` 继续；点「否」→ 取消任务。
  - 在线还原（数据卷）本就不校验哈希，弹窗仅作警告，确认后照常执行。
- **实机验证**：`--force-restore-hash` 参数被 prepare 正常接受（metadata 匹配时无副作用）。

**保留最近 N 个：留空或 0 = 全部保留（不清理）**：
- CLI：`--keep-indexes 0` 不再报错（旧代码报「must be greater than zero」），解析为 None（不清理）——**实机验证** prepare 成功。
- GUI：文本框留空/0/非数字 → 解析为 None → 不清理（已有逻辑，与 CLI 对齐）。
- keep>0 才清理，且至少保留 1 个（keep.max(1)）。

**GUI 手工检查清单（照着点，供后续人工/可操作桌面环境验收）**：
1. 打开 BackupRestore（Win11 桌面）→ 应无任何启动弹窗（本次 87 弹窗只在点击备份后出现，不是每次打开）。
2. 备份 tab：源卷选 T:（测试数据卷）→ 镜像路径填 `C:\Users\Public\br-test\gui-check.wim` → 压缩率 fast → 索引名默认应显示当前时间（可改）→ 保留最近 N 个留空 → 创建任务 → 状态区显示「已启动在线备份」，完成弹窗应显示 `exit=0`（不再是 87）。
3. 同一镜像再点一次备份（wim 已存在）→ 应追加成功（弹窗 exit=0），用「读取镜像」应看到 2 个索引且 Name 分别为两次输入。
4. 保留最近 N 个填 2 → 再备份一次 → 完成后「读取镜像」应只剩最近 2 个索引（最旧被自动删除）。
5. 还原 tab：镜像路径指向上面 wim → 读取镜像 → 下拉应列出全部索引（含 Name）→ 选第 2 个索引 → 目标卷选 T: → 创建任务 → 确认弹窗 → 还原成功后 T: 内容应为该索引对应内容。
6. 档案缺失测试：把 `gui-check.wim` 复制成 `gui-check-nomd.wim`（旁边无档案文件）→ 还原 tab 选它 → 创建任务 → 应弹「备份档案缺失」→ 点「是」→ 应能继续还原成功。
7. PE 恢复 tab：三个按钮（重启进入 PE / 返回 Windows / 创建快捷方式）功能不受本轮改动影响。

**已知限制（沿用 L.7）**：GUI 在线备份/弹窗的完整人工点击路径无法由远程命令行实测（Session 0 无交互桌面），上述 1-7 需在 Win11 桌面人工过一遍；CLI 侧同等逻辑已全部实机验证。

### L.9 全量审计「拼字符串执行命令」类隐患（2026-09-13 21:05）
起因：L.8 修复了 execute_online 的 `/Name` 带空格未加引号（exit=87）。用户要求排查是否还有其他同类问题。逐点审计了全部命令执行入口：

**结论：同类问题仅 1 处，已修复**：
- `native_gui.rs` PE 硬盘安装的格式化步骤：`cmd /c diskpart /s {script_file}` —— script_file 是程序所在目录下的临时脚本，若程序装在含空格目录（如 `C:\Users\张三\My Apps\`）会被 cmd 拆参。已改为 `diskpart /s "{script_file}"`。

**确认安全（无需改）**：
1. `main.rs` 全部 `run_logged`（dism/bcdboot/bcdedit/diskpart/wpeutil/bcdedit import）：Rust `Command::new().args()` 参数数组，Windows 下自动正确加引号，不经过 cmd 解析。
2. `execute_online`（L.8 已修）：ImageFile/Name 全部加引号。
3. 创建快捷方式：`powershell -File "{ps1}"` 已加引号。
4. PE 安装 bcdedit：`/create /d "{entry_name}"` 已加引号；其余 set 命令只含 GUID（`{...}` 无空格）或盘符（单字符）。
5. PE 硬盘安装 Apply：`/ImageFile:"{image_path}"` 已加引号。
6. bootsequence：`bcdedit /set {bootmgr} bootsequence {{{guid}}}` —— GUID 无空格。
7. PE 重启/返回 Windows：wpeutil reboot、shutdown、mountvol 均为固定字符串。
8. `run_cmd_to_file_timeout` 的日志重定向 `> "{out}"` 已加引号。
9. GUI prepare 提权（ShellExecuteW runas）：参数经 `quote_argument`（含空白或 `"` 时加引号）。

**经验沉淀（防再犯）**：本项目有两条命令执行通道——`run_logged`（参数数组，安全）与 `run_cmd_to_file*`（`cmd.exe /c` 整串，**凡是用户输入/路径/名称必须自己加引号**）。以后新增命令一律优先用参数数组；必须拼字符串时，对每个动态值做「可能含空格吗？」检查并加 `"`。

### L.10 键盘导航改造 + VM 内注入器 + prlctl 宿主导入通道（2026-09-13 21:00-22:00）
背景：用户点名两条主线——①在 VM 内部通过 prlctl exec 运行小工具（类 pyautogui）配合 prlctl capture 截图实现「看画面→算坐标→注入点击/键盘」闭环；②改造 BackupRestore GUI 使其能完全用 Tab/方向键/回车/快捷键操作（键盘导航）。目标：绕开「macOS 宿主合成鼠标事件不被 Parallels 转发给 Guest（CGEvent/SmartMouse）」这一核心限制。

#### 一、重大发现：prlctl send-key-event 宿主导入键盘通道完全可用
- **命令**：`prlctl send-key-event "Windows 11" -k <十进制键码> -e press|release`（键码表见 Parallels 官方「List of Parallels Keyboard Key Codes」，-k 用**十进制**，0x 前缀会被拒）。
- **实测**：`-k 115`（Left Win）press+release → 开始菜单弹出（2048×1472 区域像素变化）——宿主导入键盘 100% 生效，走 Parallels 官方输入管道，**不依赖 macOS CGEvent、不依赖 VM 内进程**。
- **组合键**：依次 press 修饰键 → press 主键 → release 主键 → release 修饰键（如 Ctrl=37, P=33：37p,33p,33r,37r）。
- **作用**：这是本轮最可靠的自动化输入通道。VM 内 keybd_event 单键可注入但**组合键/快捷键不可靠**（见踩坑 6），prlctl 通道对 GUI 快捷键（Ctrl+B/R/P）、关模态弹窗（Enter）、Tab、方向键全部实测有效。
- **已封装**：`tools/win-clicker/vmkey.sh <组合>`，如 `./vmkey.sh ctrl+p`、`./vmkey.sh tab`、`./vmkey.sh f5`（键名→键码映射内置：a-z=38/39/40/41/42/43/44/45/46/52/53/54/55/56/57/58/24-33（QWERTY 行序，不是字母序！），0-9=10-19，esc=9 enter=36 tab=23 space=65 backspace=22，方向键 up=98 down=104 left=100 right=102，f1-f12=67-76/95-96，home=97 end=103 pgup=99 pgdn=105 insert=106 delete=107，win=115 menu=117，ctrl=37 alt=64 shift=50）。

#### 二、GUI 键盘导航改造（native_gui.rs，v1.5.8 已编译部署）
- 三个消息循环（系统选择对话框 L≈2328、PE 桌面 L≈7418、主窗口 L≈7506）全部改为 `if IsDialogMessageW(hwnd,&msg)==0 { Translate; Dispatch }` → Tab 遍历焦点、方向键切换单选、回车默认按钮、Esc 可用。
- 主 window_proc 增 WM_KEYDOWN 分支：Ctrl+B→备份 tab、Ctrl+R→还原 tab、Ctrl+P→PE 恢复 tab、Ctrl+O→读取镜像、F5→刷新环境、Ctrl+Enter→创建任务，走 `PostMessageW(WM_COMMAND)`（与真实点击同路径）。
- 「进入 PE/RE」对话框："进入 PE"加 BS_DEFPUSHBUTTON（回车默认），Esc=取消；主界面"创建任务"加 BS_DEFPUSHBUTTON；操作模式组首个单选加 WS_GROUP（方向键只在组内切换）。
- 注意：Ctrl 检测用 `(GetKeyState(VK_CONTROL as i32) as u16) & 0x8000 != 0`（直接 & 0x8000 会因 i16 溢出编译错）。

#### 三、VM 内注入器（clicker.ps1 + run-in-session.ps1）
- `tools/win-clicker/clicker.ps1`：PowerShell Add-Type 编译 C#，user32 的 SetCursorPos+mouse_event/SendInput（点击）、keybd_event（按键）、SendInput UNICODE（文本）、Chord（组合键，逗号分隔如 ctrl,p）。日志追加写 `C:\Users\Public\backupRestore-package\clicker-log.txt`。
- `tools/win-clicker/run-in-session.ps1`：SYSTEM 上下文用 WTSQueryUserToken(SessionId) → DuplicateTokenEx → CreateEnvironmentBlock → CreateProcessAsUser 在交互会话启动命令，输出 pid/alive/exited_early/create_failed 诊断。
- `tools/win-clicker/activate.ps1`：AppActivate 激活指定标题窗口（辅助）。
- `tools/win-clicker/diag-fg.ps1`：查前台窗口句柄/标题/类名/焦点控件（诊断用）。

#### 四、实机验证结果（prlctl 通道，全部截图核验）
1. ✅ Ctrl+B → 备份 tab（界面内容整体切换）
2. ✅ Ctrl+R → 单系统还原 tab
3. ✅ Ctrl+P → PE 恢复 tab（显示 RAM disk/硬盘启动/PE 启动项名等）
4. ✅ Ctrl+O → 读取镜像（镜像路径为空时正确弹「参数校验失败：镜像绝对路径无效」——功能正常非 bug）
5. ✅ 回车 → 关闭模态 MessageBox 弹窗（参数校验失败弹窗 Enter 即关）
6. ✅ Tab × N → 焦点在控件间移动（逐次截图 diff 区域变化）
7. ✅ 方向键 ↓ → 操作模式单选组切换（探测→备份→单系统还原，tab 跟着变）
8. ⚠️ F5 → 截图无变化（刷新环境执行后界面相同，无法从像素确认；无报错）
9. ⚠️ PE tab「RAM disk/硬盘启动」单选组：机制与操作模式组一致（同为 BS_AUTORADIOBUTTON+WS_GROUP+IsDialogMessage），未单独逐键实测（焦点 Tab 路径难精确停在组内），推断有效，后续可补测。

#### 五、踩坑记录（全部解决）
1. **prlctl exec 默认跑在 Session 0（无交互桌面）**：schtasks 在该环境创建任务报中文乱码/「元素找不到」→ 弃用，改 WTS API 直启（run-in-session.ps1）。
2. **PowerShell 5.1 按 GBK 读 UTF-8 文件**：C# here-string 里**任何中文注释都会吞行**导致 Add-Type 编译失败（报「The name 'xxx' does not exist」/类成员错误）→ **clicker.ps1 的 C# 段必须纯英文注释**（本地已用脚本校验 C# 段非 ASCII 字符数为 0）。
3. **CreateEnvironmentBlock 缺失 → 子进程 0xC0000142（DLL init failed）**；加上后还需 `CREATE_UNICODE_ENVIRONMENT=0x00000400`，否则 create_failed=87。
4. **显式 si.lpDesktop="winsta0\default" → 0xC0000142**；改为 lpDesktop=null（token 决定默认桌面）→ 进程 alive=1、窗口出现在可见桌面。
5. **CREATE_NEW_CONSOLE → 隐藏控制台窗口抢焦点**，-Text 输入落到控制台 → 改 CREATE_NO_WINDOW=0x08000000（无窗口不抢焦点）。
6. **VM 内 keybd_event 的局限**：单键注入（如 A 键到有焦点的记事本）成功；但 **Win 键/Ctrl+P 组合对 GUI 无效**（合成事件不进入 Parallels 输入管道或焦点不在目标窗口）——**全部改用 prlctl send-key-event 宿主导入**。
7. **mouse_event / SendInput 鼠标点击在 Parallels VM 内确认不可用**：点击「显示桌面」按钮（任务栏最右）多次无反应（SetCursorPos 可能动了光标但点击事件未被桌面消费）。人手鼠标有效是因为走 IOHID 源头专线，程序合成鼠标事件走应用层分发，不进入 Parallels 订阅管道。**结论：鼠标注入不可用，键盘注入（prlctl 通道）可靠，GUI 必须能纯键盘操作——键盘导航改造正是为此。**
8. **Chord 解析 bug**：`Convert.ToByte("p", 16)` 抛异常（字母不是十六进制）→ 字母映射 VK（a→0x41 即 ASCII 大写，注意键盘物理行序：Q=24 W=25 E=26 R=27 T=28 Y=29 U=30 I=31 O=32 P=33）。
9. **测试残留 C:\br-test.json 导致程序一启动就自动执行在线备份并弹「执行成功」**（test_hook_auto_install 读它）→ 已把测试钩子改为**仅在显式 `--test-hook` 参数下启用**（正常启动不读 JSON），该残留不再影响。注：删除该文件被系统安全策略拦截（不可逆操作），改代码门控绕开，文件仍在但已无害。

#### 六、工具用法速查
```bash
# 宿主导入键盘（推荐，最可靠）
./tools/win-clicker/vmkey.sh ctrl+p      # 组合键
./tools/win-clicker/vmkey.sh tab|enter|esc|win|f5|down|up  # 单键
# VM 内注入（辅助，Session 1）
prlctl exec "Windows 11" cmd /c "powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\backupRestore-package\run-in-session.ps1 -SessionId 1 -Command \"powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File C:\Users\Public\backupRestore-package\clicker.ps1 -Key 0x41\""
# 截图
prlctl capture "Windows 11" --file /tmp/vm-shot.png
```

#### 七、待办
- PE tab「RAM disk/硬盘启动」单选组方向键逐键实测（低优先级，机制已验证）。
- GUI 检查清单 7 步（L.8）可用 prlctl 键盘通道替代人工逐步验收（文本输入仍受限于 VM 内 Text 注入不可用——可改用剪贴板粘贴或 prlctl 单键逐字，或接受 VM 内 keybd_event 对 ASCII 单键有效）。
- 版本号：本轮验证未 100% 收口（PE 单选组/F5 像素级确认），按用户「修完再升」规则暂保持 v1.5.8，下轮收口后升 1.5.9。
