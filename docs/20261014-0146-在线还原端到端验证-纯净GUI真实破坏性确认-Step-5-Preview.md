# 在线还原端到端验证：纯净 GUI + 真实破坏性确认（2026-10-14 01:46，v2.4.7）

- 撰写时间：2026-10-14 01:46（+08）
- 模型：Step-5-Preview
- 基线 commit：`ce364f3`（2.4.7）
- 前篇：[GUI 界面自动化键盘注入根因修复与端到端备份验证](20261014-0126-GUI界面自动化键盘注入根因修复与端到端备份验证-Step-5-Preview.md)

## 一、TL;DR

**在线还原全程通过**，且这次是**不带 `--test-hook` 的纯净 GUI**：破坏性确认框真实弹出、真实点击「是」、DISM 真实 Apply。目标 T:（数据卷）→ 在线路径（不格式化、不修复引导、不重启）。

两个重要过程发现：
1. **第一次还原的确认框是被测试钩子自动接受的**——当时 GUI 进程命令行是 `BackupRestore.exe --test-hook`，`TEST_AUTO_CONFIRM` 使 `MB_YESNO` 破坏性确认被自动点掉。还原本身真实，但少了人工确认这一环，故重启纯净 GUI 重跑。
2. **激活窗口不能用 `ShowWindow(SW_RESTORE=9)`**——它会把最大化窗口恢复成默认 1020×760，布局坐标全变，点击全部落空（本次因此点空 combo 一次）。激活用 `SetForegroundWindow`；要最大化用 `SW_MAXIMIZE=3`。

## 二、还原流程的关键机制（源码核实）

- **切到「单系统还原」时目标卷自动设为当前源卷选择**（`native_gui.rs` `select_operation`）：先在有 T: 选中的状态切模式，目标即 T:，无需手动选目标。
- **智能分流**：还原的 `affected` = 目标卷；`affected == %SystemDrive%` 才弹 PE/RE 选择走离线；数据卷目标直接走**在线覆盖**（不格式化、不修引导）。
- **索引**：还原必须选 WIM 索引；点「读取镜像」后自动选中唯一索引（`count=1 cursel=0`）。
- **确认链**（无 test-hook 时全部真实弹出）：
  1. 破坏性确认 `MB_YESNO`「目标分区将被覆盖，确认继续？」——汇总模式/源卷/镜像路径/目标卷/WIM 索引；
  2. 档案检查：副档 `.index-N.metadata.json` 存在且大小一致、未勾「严格对比哈希」→ **静默通过无弹窗**（本次即此路径）。
- **前置硬校验**（都会弹错误框并中止）：镜像路径非法；程序目录所在卷 == 目标卷；镜像卷 == 源卷（备份时）。

## 三、纯净 GUI 重跑的完整操作序列（真实键鼠）

| # | 操作 | 坐标/按键 | 回读验证 |
|---|---|---|---|
| 1 | 重启 GUI 不带 `--test-hook`（`restart-gui-plain.ps1`：计划任务 Interactive+Highest，无参数） | — | 进程命令行 `"…\BackupRestore.exe"`（无钩子） |
| 2 | 激活 + **SW_MAXIMIZE(3)**（勿用 9！） | — | rect=(-8,-8)-(1262,914) |
| 3 | 源卷 combo 选 T: | click (705,133) → `text T` → Enter | `cursel=4` = `T: \| NTFS \| … 磁盘 0/分区 5` |
| 4 | 切「单系统还原」 | click (326,99) | 目标 combo 回读 = T:（自动） |
| 5 | 填镜像路径 | click (660,539) → End(0x23) → Shift+Home → `text H:\BRImages\gui-click-backup.wim` | 框内容精确匹配 |
| 6 | 读取镜像 | click (210,615) | 索引 combo = `索引 1 \| Windows Backup (index 1) \| … arm64 \| Client`，cursel=0 |
| 7 | 创建任务 | click (340,615) | 「破坏性确认」框弹出 |
| 8 | **点「是」** | click (660,579)（否 = (756,579)） | 弹「执行完成」 |
| 9 | 点「确定」 | click (812,555) | 状态栏「在线任务已完成并通过校验」 |

## 四、验证证据

在线日志（`C:\Users\x\AppData\Local\Temp\BackupRestore-online-964-1791913542299025000\Online.log`，pid 964 = 纯净 GUI）：

- STEP 1/4 卷身份：T: = disk 0/partition 5，`volume_guid={950bd694-9a12-43e5-ad52-98d04672091c}`，serial BC9662F5；
- STEP 2/4 「还原镜像（数据卷在线覆盖，不格式化、不修复引导）」：DISM `/Apply-Image /ImageFile:H:\BRImages\gui-click-backup.wim /Index:1 /ApplyDir:T:\ /CheckIntegrity /Verify` → **退出码 0**（约 130ms，镜像 16MB）；
- STEP 3/4 回读目标卷身份；DISM 已执行完整性和应用校验；
- STEP 4/4 完成在线还原；未执行格式化或系统引导修复。

落盘抽查（`prlctl exec`）：`T:\BRRE213testdata\` 含 `data1.bin`(1,048,576B)、`readme.txt`(142B)、`sub1\`、`sub2\`；`T:\` 根含 `BackupRestoreRE\`；卷序列号 BC96-62F5 与日志一致。

界面状态：模式=单系统还原，源卷 T: / 目标卷 T:，状态栏「在线任务已完成并通过校验」。

## 五、过程记录：两次还原

| 次序 | GUI | 确认框 | 结论 |
|---|---|---|---|
| 第一次 | `--test-hook`（pid 1568） | 被 `TEST_AUTO_CONFIRM` 自动接受 | 还原真实成功，但缺人工确认环节 → 决定重跑 |
| 第二次 | 纯净（pid 964） | **真实弹出、真实点是** | **完整通过，作为验收依据** |

## 六、变更文件

| 文件 | 改动 |
|---|---|
| `tools/win-clicker/restart-gui-plain.ps1` | 新增：不带 `--test-hook` 的 GUI 重启（计划任务提权） |
| `tools/win-clicker/_t8.ps1` | 新增：「破坏性确认」对话框定位探针 |
| `docs/20261014-0146-*.md` | 本档案 |
| `文档索引.md` | 加本条 |
| `.workbuddy/memory/2026-10-14.md`、`MEMORY.md` | 在线还原结论 + 激活窗口 SW_RESTORE 教训 + 确认框坐标 |

## 七、遗留与后续

- 「新增第二系统」「PE 恢复」两条入口尚未做端到端实机（本次只覆盖备份/单系统还原）；
- 还原到**另一个含 Windows 的卷**（双系统在线还原）未测——该路径会格式化并写引导，需单独规划测试卷；
- 纯净 GUI 现仍在运行（pid 964），后续验收可继续复用；若要还原测 `--test-hook` 场景，用 `restart-gui-testhook.ps1` 切回。
