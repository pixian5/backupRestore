# BackupRestore 当前完整进度基线

更新时间：2026-09-11  
源码版本：`1.3.5`  
分支：`main`  
仓库：`/Users/x/code/backupRestore`  
Windows 共享源码：`C:\Users\x\Desktop\BackupRestore`  
目标平台：Windows 11 ARM64 / UEFI / GPT

本文是 v1.3.4 → v1.3.5 的增量基线。v1.3.3 及之前的完整历史与执行顺序见 [current-progress-2026-09-09.md](current-progress-2026-09-09.md) 与 [verification-matrix.md](verification-matrix.md)。

## 0. 本轮结论（v1.3.5 开发测试版收口）

- **新增第 5 操作模式「PE 恢复」**（`install-pe-secondary`）：程序内一键把 PE WIM 设为第二操作系统，替代此前半手工脚本链（copype → DISM 注入 → 部署 → BCD）。
- **实机全流程验收通过（2026-09-11 00:59，Parallels Windows 11 ARM64）**：GUI 选 WIM + 目标卷 Q: → 复制 WIM、确保 boot.sdi、复用系统 {ramdiskoptions} 新建「Windows PE (BackupRestore)」osloader 启动项并加入 displayorder → 弹窗「安装完成」；`default {current}` 未改动。
- 遗留已知限制：新条目与既有 PE Test 条目同机制（ramdisk osloader），真实菜单引导链路已有 §4.10 实机证据；本轮未重复真实重启（可下次快照回归）。

## 1. 功能实现（v1.3.5）

| 文件 | 改动 |
|---|---|
| `crates/backuprestore-cli/src/native_gui.rs` | `selected_operation` 增加 `install-pe-secondary`（index=4）；`Controls.operation_tabs` 扩为 5 个 tab（ID_OPERATION_PE=1104，宽 95、x 起点 180/279/378/477/576）；新增中文 tab「PE 恢复」、hint、卷标签「源卷（不使用）/目标卷（PE 安装位置）」；`BACKUPRESTORE_OPEN_TAB` 支持 1..=4 |
| `install_pe_secondary` | 校验 WIM 存在 → 校验目标卷已选且 ≠ 程序所在卷 → MB_YESNO 确认 → 复制 WIM 到 `目标卷\sources\boot.wim`（已有文件首装保留 `.stock`）→ 确保 `\boot\boot.sdi` → BCD 步骤 |
| `run_cmd_to_file` | CreateProcessW + CREATE_NO_WINDOW + 输出重定向到文件，返回 exit code（开发期 GUI 提升运行下 cmd 可见由用户手动执行验证） |
| `extract_bcd_guid` | 自适应 GBK（`from_utf8_lossy`）/ UTF-16LE（BOM 或 NUL 密集检测）解码，返回**裸 GUID**（不含花括号） |
| 版本 | VERSION + 2×Cargo.toml `1.3.4 → 1.3.5` |

### BCD 写法（实机修正后最终形态）

1. **复用系统 `{ramdiskoptions}`**（bcdedit 在本平台不支持 `/create /application ramdisk`，返回「指定的应用程序类型无效」）：
   - `bcdedit /set {ramdiskoptions} ramdisksdidevice partition=Q:`
   - `bcdedit /set {ramdiskoptions} ramdisksdipath \boot\boot.sdi`
2. **创建 osloader 条目**：`bcdedit /create /d "Windows PE (BackupRestore)" /application osloader` → 解析输出文件 GUID（中文系统 bcdedit 输出为 GBK/UTF-16，需自适应解码）
3. **设置条目属性**：device/osdevice=`ramdisk=[Q:]\sources\boot.wim,{ramdiskoptions}`、winpe yes、detecthal yes、systemroot \windows、nx OptIn、description
4. **加入菜单**：`bcdedit /displayorder {guid} /addlast`

不修改 `default`，重启后从 Boot Manager 菜单手动选择进入 PE。

## 2. 实机验证证据（v1.3.5，2026-09-11）

| 项 | 结果 |
|---|---|
| 5 个 tab 显示 | GUI 截图确认「探测/备份/单系统还原/新增第二系统/PE 恢复」全部显示（pe-install-tab-zoom4.png） |
| PE 恢复页面布局 | 源卷（不使用）C: / 目标卷 Q: / 镜像路径 / 第二系统名称 / hint 正常（pe-install-tab5-page.png） |
| 复制 WIM | `Q:\sources\boot.wim` SHA-256 与 `C:\BackupRestorePE\BackupRestorePE.wim` 一致（412,033,605 字节）；`.stock` 保留上一版 WIM |
| boot.sdi | `Q:\boot\boot.sdi` 存在（3,170,304 字节） |
| BCD 条目 | `bcdedit /enum` 出现「Windows PE (BackupRestore)」，osdevice=ramdisk=[Q:]\sources\boot.wim,{ramdiskoptions}；`default {current}` 未改动 |
| GUI 弹窗 | 「安装完成：PE 恢复环境已安装到 Q: 卷。启动菜单已新增「Windows PE (BackupRestore)」」（用户截图 00:59） |
| 日志 | `logs\gui.log` 记录 `PE install started / bcdedit ... -> 0` 全链路 |

## 3. 实机排查中修复的 3 个错误（勿重踩）

1. **bcdedit 不支持 `/create /application ramdisk`**（「指定的应用程序类型无效」）→ 复用系统 `{ramdiskoptions}`，只需 `/set` 重指向目标卷。
2. **bcdedit 中文输出编码**：GUI 提升运行时 cmd 重定向 bcdedit 输出为 GBK（无 BOM，`CF EE ...` 中文双字节）；早期 UTF-8 `read_to_string` 解析失败 → 误判为 UTF-16 → 后改为 NUL 密集检测 + 自适应解码。实际为 **GBK**（ASCII GUID 部分可直接扫描）。
3. **GUID 双花括号**：`extract_bcd_guid` 原返回 `{guid}`（含花括号），`format!("{{{}}}", os_guid)` 再包一层 → `{{guid}}` → bcdedit 拒绝。改为返回裸 GUID 后命令正常。

## 4. 已知边界

- PE 是 RAM 盘引导：目标卷不参与 PE 运行期，`install-pe-secondary` 只把 WIM/SDI/BCD 写好；PE 内「返回 Windows」退出链沿用 v1.3.3 已闭环方案。
- 复用系统 `{ramdiskoptions}` 会影响所有引用它的 ramdisk 条目（当前只有 PE 条目，且目标卷一致 Q:，无实际冲突）。
- 目标卷必须 ≠ 程序所在卷（代码校验）；未校验目标卷文件系统类型（要求 NTFS，界面 hint 已注明）。
- 重启进 PE 需用户从 Boot Manager 菜单手动选择（`displayorder` 新增，非 `bootsequence`）——安全设计，避免 PE 循环。

## 5. 未完成 / 下一轮建议

- [x] 真实重启 → Boot Manager 菜单选择「Windows PE (BackupRestore)」→ 进入 PE 恢复桌面的完整闭环回归 —— **v1.3.6 已实机收口**（见 §6，PE 内「重启」回 Win11 且 Boot 菜单 default 仍 Win11）。
- [ ] `set_operation_tabs` 中 `PE dev:` 诊断日志前缀在产品化时可收敛为正式日志。
- [ ] 目标卷文件系统校验（NTFS）与 boot.sdi 缺失时从 ADK 自动复制的提示完善。

## 6. v1.3.6 / v1.3.7（2026-09-11 追加）

### 6.1 v1.3.6：bootsequence + PE 内启动后自清（零操作闭环，已实机验收）

| 项 | 结果 |
|---|---|
| 自清实现 | `pe_self_clean_bootsequence()`：PE 桌面弹窗前静默执行 `mountvol.exe S: /S` + `bcdedit.exe /store S:\EFI\Microsoft\Boot\BCD /deletevalue {bootmgr} bootsequence`，结果写 `S:\pe-bootsequence-clean.log`（回 Windows 后可读回） |
| 「重启进入 PE」按钮 | 主窗口 ID_PE_REBOOT_MAIN=1410，读 `pe-entry-guid.txt`（install-pe-secondary 写入的裸 GUID）→ `bcdedit /set {bootmgr} bootsequence {<guid>}` |
| 实机闭环 | 用户点「重启进入 PE」（exit code=0）→ 手动重启自动进 PE 桌面 → PE 点「重启」→ **回 Win11、Boot 菜单 default 仍 Win11、`bcdedit /enum {bootmgr}` 无 bootsequence** |
| 遗留取证 | ESP 上 `pe-bootsequence-clean.log` 未找到（CLEAN_LOG_MISSING）——闭环成立（bootsequence 被消费/清除）但自清代码是否落盘未取证，待下次进 PE 跑 3 条命令确认 |

### 6.2 v1.3.7：GUI 创建桌面快捷方式

- 「创建快捷方式」按钮（ID_PE_SHORTCUT=1411，PE 恢复 tab）：生成 **BackupRestore.lnk**（指向程序本身、无参数，双击打开 GUI）。
- **Parallels 桌面重定向坑**：提升进程 `[Environment]::GetFolderPath('Desktop')` 返回 `C:\Mac\Home\Desktop` 或 systemprofile，**不是 Windows 物理桌面** → ps1 改为遍历 `GetFolderPath('Desktop')` + `$env:USERPROFILE\Desktop` 候选路径全部创建、去重。
- `--pe-reboot` 独立入口：无窗口设置 bootsequence（供快捷方式复用；非提升先提权重启）。
- 实机状态：手动脚本创建 `C:\Users\x\Desktop\BackupRestore.lnk` 成功（SHORTCUT_OK）；GUI 按钮已部署，**按钮点击待用户切 PE tab 实点确认**（注入 SendMessage 验证未成功——见 §6.3）。
- 已提交 git `2623037`（v1.3.6→1.3.7）。

### 6.3 开发调试经验（勿重踩）

1. **WM_COMMAND 注入编码**：`wParam = MAKEWPARAM(控件ID, 通知码)`——**ID 在低 16 位**（`1104` 直接传即可），先前写成 `1104 << 16`（ID 在高位）导致消息无响应。
2. **注入通道不可靠**：`--current-user` PowerShell 的 `SendMessage` 到 GUI 主窗口有时卡住/无响应（FindWindow 按标题也返回 0），GUI 自动化建议以用户实点为准；调试脚本 `_pe-inject-*.ps1` 已 gitignore。
3. **PE 自清取证命令**（下次进 PE 执行）：`type X:\Windows\System32\logs\gui.log | findstr self-clean`、`mountvol S: /S & dir S:\pe-bootsequence-clean.log`、`bcdedit /store S:\EFI\Microsoft\Boot\BCD /enum {bootmgr} | findstr bootsequence`。

## 7. 配置驱动 PE 任务机制（v1.3.7+，2026-09-11 实机闭环）

### 7.1 机制（用户文档明确要求：写配置 → PE 读配置执行 → 按配置决定是否重启）

- **配置**：Windows 侧挂载 ESP 后写 `S:\pe-task.txt`（每行一个动作；`reboot` 行 = 执行完自动重启回 Windows）。
- **PE 侧**：`pe_task_execute()`——**先 `mountvol S: /S` 挂载 ESP**（关键：PE 启动时 S: 未挂载，先挂载才能读到配置）→ 逐行执行动作 → 结果落 `S:\pe-task-result.txt` → 配置改名 `pe-task.txt.done` 防重复 → 按配置含 `reboot` 则 `wpeutil reboot` 自动回 Windows。
- 支持动作：`clean_bootsequence`（清 {bootmgr} bootsequence）、`verify`（bcdedit enum + dir + 读回取证）。
- **重启由配置决定**：配置含 reboot 才重启；后续其他功能（备份/恢复等）只需往配置加动作，不强制重启。

### 7.2 实机验证（全自动，用户零操作）

写配置（clean_bootsequence+verify+reboot）→ 设 bootsequence → `prlctl restart` → PE 读配置执行（mountvol=0）→ 自动回 Windows（16 秒）→ `S:\pe-task-result.txt` 落盘、`pe-task.txt.done` 生成、BCD 无 bootsequence 残留。

### 7.3 踩坑记录

1. **bootsequence 会被 bootmgr 消费**：ENUM_BEFORE 显示 PE 启动时 bootsequence 已不在 BCD（RAM 盘场景并非绝对不消费），故「检测 bootsequence 触发自动重启」不可靠 → 改用配置驱动。
2. **先挂载再读配置**：配置在 ESP（S:），PE 启动 S: 未挂载，读配置必先 mountvol。
3. **prlctl restart 对 PE 无 Tools 不响应**：ACPI 重启被取消，需 `prlctl stop --kill` + `start` 兜底。
