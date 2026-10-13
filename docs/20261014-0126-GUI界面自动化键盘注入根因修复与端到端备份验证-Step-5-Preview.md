# GUI 界面自动化：键盘注入根因修复 + 下拉框/编辑框驱动配方 + 端到端备份验证

- 撰写时间：2026-10-14 01:26（+08）
- 模型：Step-5-Preview
- 基线 commit：`bcfefee`（2.4.7，客体二进制 SHA256 `1B39A1E2…`）
- 覆盖对象：**`20261014-0054-GUI程序本体备份成功与界面自动化受阻-Opus5.md` 中「真·界面驱动走不通、所有 combo 项数 0」的结论已作废**——根因是探测脚本自身 bug + 键盘注入缺扫描码，界面驱动完全可行。

## 一、TL;DR

1. **下拉框「拿不到项」是假象**：真实原因是探测脚本把 `0x014B`（CB_RESETCONTENT，会清空列表）当成 `CB_GETCOUNT`（正确 `0x0146`），且中等完整性脚本跨完整性读不到提权 GUI 的控件。改对消息码、改到**提权 + 同会话**的 agent 里执行后，`CB_GETCOUNT=5 / CB_GETCURSEL=4 / CB_GETLBTEXT` 全部正常。
2. **键盘注入半瘫的根因**：agent 的 `Vk()` 用 `keybd_event` 发键时**扫描码传 0、不置扩展键标志**。Home/End/Insert/Delete/PageUp/PageDown/方向键都是扩展键，标准 EDIT 控件直接忽略这种按键；Backspace(0x08) 不是扩展键所以一直正常——这一条解释了此前所有"ctrl+a 失灵"的怪象。
3. **ctrl+a 全选无效不是注入问题**：标准 EDIT 控件本身不支持 Ctrl+A（RichEdit 特性）。可靠全选 = **End(0x23) → Shift+Home**。
4. **exec 头号陷阱**：agent 的 `exec` 与 agent **同进程**执行脚本，脚本里写 `exit` 会**把 agent 一起杀掉**（宿主侧表现 = 连接重置、无任何输出）。exec 脚本一律用 `return`。
5. **成果**：全程真实键鼠驱动 GUI 程序本体完成备份：T:（数据卷，走在线路径不重启）→ `H:\BRImages\gui-click-backup.wim`（16,779,577B），日志四步全过，`sha256=c4efbf95…e58f`。

## 二、UI 自动化的最终技术路线（三种方案实测对比）

| 方案 | 手段 | 结果 |
|---|---|---|
| Win32 消息级 | SendMessage / BM_CLICK / CB_ADDSTRING 从外部进程操作控件 | **不可行**。GUI 是提权进程（高完整性），中等完整性进程被 **UIPI** 拦；且 BM_CLICK 对「刷新环境」按钮无效（该按钮走自定义绘制/点击链路） |
| 宿主键盘注入 | `tools/win-clicker/vmkey.sh`（Parallels 通道） | 可用，WinRE/PE 也能用，但**只有键盘** |
| **VM 内 TCP agent** | `br-agent-tcp.sh` → VM 内提权 PowerShell agent（Session 1，高完整性）→ `SetCursorPos`+`mouse_event` 真实点击 / SendInput UNICODE 真实键盘 | **唯一可靠路线**。同会话同完整性，既不被 UIPI 拦，又能用消息回读校验 |

agent 协议（`br-agent-tcp.sh`）：`start / ping / stop / click x y / move / key 0xVV / chord mod,key / text 字符串 / windows / cursor / tick / shot 路径 / batch 文件 / exec 本地.ps1`。空闲 15 分钟自退，崩了重启即可（`start` 内部走 `br-s1.sh br-launch-agent.ps1` 可靠链路）。

## 三、下拉框问题的完整解决

### 3.1 回读（验证选中态/项数）

前提：agent 与 GUI **同会话 + 同完整性**。消息码（全部实测）：

| 用途 | 消息 |
|---|---|
| 项数 | `CB_GETCOUNT = 0x0146` |
| 当前选中索引 | `CB_GETCURSEL = 0x0147` |
| 选中项文本 | `CB_GETLBTEXT = 0x0148`，wParam=索引，lParam=缓冲区 |
| 编辑框/按钮文本 | `WM_GETTEXT = 0x000D`，**wParam 必须传缓冲区字符数**（传 0 会得到空串——本次就踩了） |

控件定位用 `GetDlgItem(主窗口, ID)`，不要枚举（枚举回调里抛异常会穿 native 边界直接杀进程）。主窗口标题含 `BackupRestore`，用 EnumWindows + `Contains` 匹配（按完整标题 `FindWindowW` 没匹配上，疑似短横线字符差异）。

主界面控件 ID（`native_gui.rs`）：

| ID | 控件 | 屏幕中心坐标（本机 1254×954） |
|---|---|---|
| 1001 | 刷新环境 | (80, 489) |
| **1003** | **创建任务** | **(340, 489)** |
| 1202 | 源卷 combo | (705, 133) |
| 1203 | 镜像绝对路径 edit | (660, 383) |
| 1209 | 索引名 | (320, 447) |
| 1210 | 保留最近 N 个 | (650, 447) |
| 1211 | 校验哈希 checkbox | (1080, 419) |

### 3.2 驱动（选源卷）

实测有效序列：**click combo 中心 → `text T`（下拉列表框按首字母跳选）→ `key 0x0D`（回车确认）**。回读 `CB_GETCURSEL=4`、`CB_GETLBTEXT` = `T: | NTFS | 卷标 无卷标 | 总容量 5.0 GiB | 可用 5.0 GiB | 磁盘 0/分区 5`，且界面详情面板同步刷新——选中真实生效。

## 四、键盘注入根因与修复（本次唯一代码改动）

### 4.1 根因

`br-agent-tcp.ps1` 原实现：

```csharp
public static void Vk(byte vk) { keybd_event(vk, 0, 0, UIntPtr.Zero); ... }
```

扫描码 `sc=0`、`dwFlags=0`。对扩展键，标准控件从 lParam 取扫描码/扩展位，`sc=0` 的按键被当作无效键忽略。实测矩阵：

| 键 | 是否扩展键 | 修复前 | 修复后 |
|---|---|---|---|
| Backspace 0x08 | 否 | 有效（120 连发从光标处往前删 83 字符） | 有效 |
| Home 0x24 / End 0x23 | **是** | 无效（光标不动） | **有效** |
| Shift+Home 组合 | Home 是 | 无效 | **有效（可全选）** |
| Ctrl+A | A 不是扩展键 | 无效 | 仍无效——**标准 EDIT 不支持 Ctrl+A**，非注入问题 |

### 4.2 修复内容（`tools/win-clicker/br-agent-tcp.ps1`）

- 新增 `MapVirtualKeyW` 导入；`Vk()` 改为 `MapVirtualKeyW(vk, 0)` 取真扫描码；
- 新增 `IsExtVk()`：0x21/0x22/0x23/0x24/0x25/0x26/0x27/0x28/0x2C/0x2D/0x2E/0x2F/0x5B/0x5C/0x5D/0x6F/0x90/0xA3/0xA5 置 `KEYEVENTF_EXTENDEDKEY(0x0001)`；
- `ModDown/ModUp`（ctrl/alt/shift）同样补扫描码。

验证（修复后重启 agent）：`key 0x24` 后输入落到行首；`key 0x23` + `chord shift,0x24` 后输入**整体替换**原内容（29 字符 → 3 字符）。

### 4.3 编辑框可靠配方（今后一律用这个）

```
click <编辑框中心>      # 焦点+光标落在点击处
key 0x23                # End：光标到行尾
chord shift,0x24        # Shift+Home：选到行首 = 全选
text <内容>             # 整体替换
```

回读校验：`WM_GETTEXT` 且 wParam=缓冲区长度。

## 五、exec 陷阱（本次两次把 agent 搞崩的教训）

agent 的 exec 实现是 `$sb = [ScriptBlock]::Create($code); & $sb`——**在 agent 自己的进程里跑**。脚本里任何 `exit N` 会直接终止 agent 进程：输出行还没写回，宿主侧只看到 TCP 连接重置、零输出。**exec 脚本禁止 exit，用 return**。另外：枚举回调（EnumWindows/EnumChildWindows 的委托）里不要抛异常，会穿 native 边界杀进程；探针优先 `GetDlgItem` 按 ID 直取。

## 六、端到端备份验证（GUI 程序本体，真实键鼠）

操作序列（全部经 agent 真实键鼠）：

1. 点「备份」模式按钮（已选中态）；
2. 点源卷 combo (705,133) → `text T` → 回车 → 回读 cursel=4 ✓；
3. 点镜像路径框 (660,383) → End → Shift+Home → `text H:\BRImages\gui-click-backup.wim` → 回读精确匹配 ✓；
4. 点「创建任务」(340,489) → 状态栏「在线任务已完成并通过校验」→ 「执行完成」对话框（确定按钮中心 (812,555)）→ 点掉。

产物与日志（`C:\Users\x\AppData\Local\Temp\BackupRestore-online-1568-1791912132525301900\Online.log`）：

- STEP 1/4 卷身份：T: = disk 0/partition 5，`volume_guid={950bd694-9a12-43e5-ad52-98d04672091c}`，NTFS；
- STEP 2/4 DISM 捕获（候选副本机制 `.br-online-1568-…\candidate.wim`，`/CheckIntegrity /Compress:fast`，镜像名 `2026-10-14 00:53`）→ 退出码 0；
- STEP 3/4 `sha256=c4efbf95246b7ea3e6d47dd6dfb518003a17e182a366663e465701e9feb1e58f bytes=16779577 保留=1 删除=0`；
- STEP 4/4 镜像及副档发布并回读，候选目录清理；「命令、回读和副档簿记均已通过」。

磁盘核实：`H:\BRImages\gui-click-backup.wim` 16,779,577B + `gui-click-backup.wim.index-1.metadata.json` 1,042B（01:22）。

**再次印证 2.4.7 关键事实**：数据卷备份走**在线路径**（不重启、不动 BCD、不进 RE），与 CLI `prepare` 的离线进 RE 通道是两条路。

## 七、变更文件

| 文件 | 改动 |
|---|---|
| `tools/win-clicker/br-agent-tcp.ps1` | Vk/ModDown/ModUp 补扫描码 + 扩展键标志（根治） |
| `tools/win-clicker/_t3.ps1` / `_t4.ps1` | 控件探针（_t4 = 修正版回读，wParam 传缓冲区长度） |
| `tools/win-clicker/_t5.ps1` | 消息框定位（找「执行完成」确定按钮） |
| `tools/win-clicker/_t6.ps1` | 扩展键根因验证脚本 |
| `tools/win-clicker/_ui-probe6.ps1` / `_ui-probe7.ps1` | 过渡探针（_ui-probe6 因 exec 里写 exit 杀过 agent，勿再用其 exit 写法） |
| `文档索引.md` / `docs/README.md` | 加本条索引；0054 文档加作废横幅 |

## 八、遗留

- agent `text` 逐字符 8ms，长路径输入约 0.3s，可接受；
- 源卷 combo 驱动依赖「首字母跳选 + 回车」，若两个卷首字母相同需再按多次字母或改用鼠标点开下拉后点项（下拉列表弹出后的项定位未做，需要时再补 `br-gui-combo-probe.ps1` 路线）；
- 压缩率/索引名等 combo 未做自动化驱动（本次用默认值即通过）。
