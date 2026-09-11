# BackupRestore 当前完整进度基线

更新时间：2026-09-11  
源码版本：`1.3.9`  
分支：`main`  
仓库：`/Users/x/code/backupRestore`  
Windows 共享源码：`C:\Users\x\Desktop\BackupRestore`  
目标平台：Windows 11 ARM64 / UEFI / GPT

本文是 v1.3.4 → v1.3.9 的增量基线。v1.3.3 及之前的完整历史与执行顺序见 [current-progress-2026-09-09.md](current-progress-2026-09-09.md) 与 [verification-matrix.md](verification-matrix.md)。

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
- **重启由配置决定**：配置含 reboot 才重启；后续其他功能（备份/恢复等）只需往配置加动作，不强制重启。

### 7.2 实机验证（全自动，用户零操作）

写配置（clean_bootsequence+verify+reboot）→ 设 bootsequence → `prlctl restart` → PE 读配置执行（mountvol=0）→ 自动回 Windows（16 秒）→ `S:\pe-task-result.txt` 落盘、`pe-task.txt.done` 生成、BCD 无 bootsequence 残留。

### 7.3 踩坑记录

1. **bootsequence 会被 bootmgr 消费**：ENUM_BEFORE 显示 PE 启动时 bootsequence 已不在 BCD（RAM 盘场景并非绝对不消费），故「检测 bootsequence 触发自动重启」不可靠 → 改用配置驱动。
2. **先挂载再读配置**：配置在 ESP（S:），PE 启动 S: 未挂载，读配置必先 mountvol。
3. **prlctl restart 对 PE 无 Tools 不响应**：ACPI 重启被取消，需 `prlctl stop --kill` + `start` 兜底。

## 8. PE 下备份/还原小测试盘闭环（2026-09-11 实机验证）

### 8.1 新增配置驱动动作（native_gui.rs pe_task_execute）

| 动作 | 作用 |
|---|---|
| `attach-vhd <vhd路径> <盘符\|AUTO>` | diskpart 挂载 VHD；**必须先 `automount enable`**（PE 精简环境默认不自动分配盘符，手动 assign 也不生效——实测 T:/Z: 均无效，automount 后系统自动分配 J:），`AUTO` 表示枚举 marker.txt 所在盘符写入 `S:\pe-drive.txt` |
| `backup <盘符\|AUTO> <wim路径>` | dism Capture-Image 捕获卷为 WIM |
| `restore <wim路径> <盘符\|AUTO>` | dism Apply-Image 应用 WIM 到卷 |
| `verify-file <路径>` | cmd `if exist` 检查文件（PE 无 PowerShell） |
| `delete-file <路径>` | 删除文件（还原验证：删掉后 restore 应恢复） |
| `dism-diag` | 诊断 dism 各参数变体 |

盘符/路径支持 `AUTO` 前缀（`AUTO:\marker.txt`）→ 运行时解析为实际盘符。

### 8.2 实机闭环结果（全自动，零用户操作）

配置：`attach-vhd Q:\testdisk.vhd AUTO → backup AUTO H:\pe-wim1.wim → verify → delete marker → verify MISSING → restore → verify marker FOUND → reboot`

| 步骤 | 结果 |
|---|---|
| attach VHD（1GB，含 marker + 100MB payload） | 成功，PE 自动分配 **J:** |
| backup（dism Capture） | **The operation completed successfully** |
| verify marker | FOUND |
| delete marker | MISSING |
| restore（dism Apply） | **The operation completed successfully** |
| verify marker | **FOUND（还原恢复）** |
| Windows 侧复核 | marker 内容正确 + payload.bin **104,857,600 字节完整** |

### 8.3 PE 环境踩坑（勿重踩）

1. **PE 的 dism（ADK 28000）对 WIM 文件名转义敏感**：`test-backup.wim`（含 `\t` `\b`）报 Error 123；用 `pe-wim1.wim` 成功。**WIM 文件名避免 `\a \b \f \n \r \t \v` 等字母组合**。
2. **PE dism 的 `/Name` 值不能含连字符**：`/Name:PE-Test` 报 Error 123，`/Name:PE` 成功。
3. **PE diskpart 手动 `assign letter=` 不生效**（报 success 但盘符无效）：需 `automount enable` 让系统自动分配。
4. **PE 无 PowerShell**：验证用 cmd `if exist`。
5. **PE 里盘符与 Windows 不同**：Windows Q:（backupRestore 卷）在 PE 里是 H:；VHD 位置需先搜索实际盘符（`for %d in (C..Z) do if exist %d:\xxx`）。

## 9. 真实磁盘分区备份/还原闭环（2026-09-11 实机验证，v1.3.7 后续）

### 9.1 用户否决 VHD 载体

VHD 链路验证后，用户明确「不要备份还原 VHD，你应该备份还原真实的磁盘」。改用 VM 内真实 NTFS 分区 **P:**（8GB，含 fixture 100MB + Windows 73MB 测试数据，7.8GB 空闲）。

### 9.2 新增 `find-drive` 动作

- 作用：按标记文件枚举真实分区在 PE 中的实际盘符（真实分区 PE 自动挂载，无需 attach；但盘符与 Windows 不同，需按标记定位）→ 写入 `S:\pe-drive.txt`，供 `AUTO` 解析。
- 实测：Windows P: → PE 中为 **G:**，`find-drive backup-test-marker.txt` 正确返回 G。

### 9.3 实机闭环结果（全自动，零用户操作）

Windows 侧 P:\ 放 `backup-test-marker.txt` → 配置：`find-drive → backup AUTO H:\pe-wim1.wim → verify → delete marker → verify MISSING → restore → verify marker FOUND → reboot`

| 步骤 | 结果 |
|---|---|
| find-drive | 实际盘符 G:（= Windows P:） |
| backup（dism Capture 全卷 174MB） | The operation completed successfully |
| delete marker | MISSING |
| restore（dism Apply） | The operation completed successfully |
| verify marker | FOUND（还原恢复） |
| Windows 侧复核 | marker 内容正确 + fixture **100,663,390 字节** + Windows **73,386,230 字节** 与还原前完全一致 |

**结论**：真实分区全卷备份→还原数据 100% 恢复，链路与真实产品（dism WIM）完全一致。

## 10. PE 桌面三按钮接入 + bcdboot default 恢复（2026-09-11 实机验证，v1.3.8 → 1.3.9）

### 10.1 需求背景（P0 缺口①）

完整核对文档时发现：PE 恢复桌面「备份系统/还原系统/安装第二系统」三个核心按钮此前是占位（只 `PE_EXIT_TAB.store` + DestroyWindow，跳主 GUI 后 prepare 依赖正常 Windows，PE 内实际完不成任务）。用户拍板：三个按钮改为 **PE 会话内直接执行**，不依赖重启、不依赖 WinRE。

### 10.2 方案（已确认）

- 把 `pe_task_execute()` 的动作执行逻辑抽成可复用执行器 `execute_pe_task_line(line, &mut result, &mut reboot)`，人工点击与配置驱动共用同一套动作。
- 新增 4 个动作：
  - `find-system-drive`：枚举含 `\Windows\System32\Config\SYSTEM` 的卷写 `S:\pe-drive.txt`
  - `format <盘符> [--allow-system]`：diskpart 快速格式化，默认拒绝 X:(PE) / S:(ESP) / 含 Windows 的卷
  - `bcdboot <系统盘符> [<esp>]`：目标为系统卷时 `bcdboot <d>:\Windows /s <esp> /f UEFI`
  - `add-secondary-entry <盘符> <菜单名...>`：bcdedit osloader 条目（device/osdevice/path/systemroot/nx 全字段避 0xc0000225），displayorder /addlast，default 保持 Windows 第一
- 三个 PE 流程函数 + `pe_dialog` 模态对话框（卷下拉/镜像路径/可选菜单名/红色警告/执行取消）+ `pe_list_volumes` + `pe_default_image_drive` + `pe_result_preview`。
- 安全防线：format 三道拒绝、还原二次确认、WIM 文件名避开 dism 转义字母、日志写 ESP。

### 10.3 bcdboot 关键缺陷与修复（实机发现，两轮修复）

**缺陷**：`bcdboot <测试盘>: /s S:` 会把 Boot Manager 的 default 指向目标卷。第一版恢复逻辑解析 `{bootmgr}` 的 default 字段得到 `{default}` **别名**——但 bcdboot 执行后该别名已重新绑定到新条目，`set default {default}` 等于没恢复 → 真实重启 0xc000000f（winload.efi 缺失，Recovery 蓝屏）。

**修复**：bcdboot 前 `enum {default}` 记录原默认条目引用的 **partition=X:**；bcdboot 后枚举 BCD 全条目，按 `device partition=X:` 找回真实 GUID → `set {bootmgr} default {GUID}` + `displayorder {GUID} /addfirst`。解析兼容中文"标识符"/英文"identifier"。

### 10.4 实机验证（配置驱动全自动，P: 测试盘，两轮）

第一轮（旧版）：`find-drive → backup → format(REFUSED ✓) → bcdboot(code=0, DEFAULT_RESTORE 报成功但无效) → restore → reboot` → 重启 Recovery 蓝屏（default 被改 P:），复现缺陷。

第二轮（修复版，最终收口）：
| 步骤 | 结果 |
|---|---|
| find-drive | G:（= Windows P:） |
| verify marker | FOUND |
| backup G: → H:\pe-bk-test.wim | 成功 100% |
| format G: | REFUSED（含 SYSTEM，无 --allow-system）✓ |
| bcdboot G: /s S: | code=0 + `[DEFAULT_RESTORE {581bfd2c-…}] The operation completed successfully`（真实 GUID 恢复）|
| restore | 成功 100% |
| verify marker | FOUND |
| reboot → 真实重启 | **正常进 Windows 桌面**（非 Recovery）✓ |
| Windows 侧 BCD 复核 | `default {default}` → device partition=C:，description "Windows 11"；displayorder 第一 {default}(C:)；bootsequence 已清；S:\pe-task.txt 已改名 .done |

**结论**：format 保护、bcdboot 执行 + default 恢复、重启回 Windows 全部实机闭环；P: 备份/还原数据完整。测试盘 bcdboot 创建的残留 osloader 条目已用 `displayorder /remove` 移出菜单（不删除条目，default 不受影响）。

### 10.5 版本与产物

- VERSION + 2×Cargo.toml：`1.3.7 → 1.3.8`（三按钮）→ `1.3.9`（bcdboot 修复）
- 正式 GUI `C:\Users\Public\backupRestore-package-v12\BackupRestore.exe`（1,326,080 字节）
- PE WIM `Q:\sources\boot.wim` 已更新（20:37:08，含 v1.3.9）
- **遗留**：PE 桌面三按钮 UI 真实点击验收（对话框→点击→执行→结果弹窗）留用户手动（Parallels 无法注入 PE 输入）；执行器核心链路已由配置驱动全自动实机验证。
