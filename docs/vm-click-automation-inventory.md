# VM 内「辅助点击 / UI 自动化」能力清单（2026-09-26 清查）

> 结论：**有，而且是本项目自研的一套完整通道**，不是第三方软件。VM 里没有安装
> AutoHotkey/按键精灵/向日葵/ToDesk 之类的通用辅助或远控程序——已安装的第三方
> 只有 WinRAR、Chrome/Edge、CC Switch、VS2022 生成工具、ADK/WinPE、Parallels Tools。
> 所有"辅助点击"能力都来自 BackupRestore 自己落的 PowerShell 脚本 + 计划任务。

## 一、核心资产

| 位置 | 作用 |
|---|---|
| `C:\Users\Public\backupRestore-package\clicker.ps1` | **输入注入器**：PowerShell `Add-Type` 编译 C#，调 user32 `SetCursorPos` + `SendInput`/`mouse_event`/`keybd_event`。支持 `-X/-Y -Click`、`-Dbl`、`-Move`、`-Key 0x0D`、`-Text`、`-Chord ctrl,p`。日志写 `clicker-log.txt` |
| `tools/win-clicker/clicker.ps1`（宿主副本） | 与上者同源，附带中文注释说明设计背景 |
| `tools/win-clicker/run-in-session.ps1` | 用 `WTSQueryUserToken` + `CreateProcessAsUser` 在 **Session 1 交互桌面**启动命令（SYSTEM 上下文用） |
| `C:\Users\Public\click.ps1` | 轻量版：`GetDlgItem` + `SendMessage(BM_CLICK)` 直发消息（不需真实鼠标） |
| `click-pe-disk.ps1` / `click-pe-tab.ps1` | UIAutomation + `BM_CLICK` 组合，用于 PE 盘/RAM 单选、tab 切换 |
| 包内 `gui-*.ps1`、`dump-controls.ps1`、`dump-windows.ps1`、`enum-*.ps1`、`send-wm*.ps1`、`launch-in-session*.ps1`、`frames-session1.ps1` 等约 60 个脚本 | GUI 控件枚举/取文本/设置文本/发消息/会话内启动/抓帧 的完整工具箱 |
| 宿主侧 `tools/`（137 个 .ps1） | 对应的宿主驱动与抓帧脚本 |

## 二、为什么必须"VM 内自己点"

macOS 宿主合成的鼠标事件（CGEvent / SmartMouse）**不会被 Parallels 转发进 Guest**；
而 `prlctl exec` 默认跑在 **Session 0（无桌面）**，直接注入无效。
所以可行路径只有两条：

1. `schtasks /create /ru x /rp <pwd> /it` + `schtasks /run`（交互式令牌，注入到可见桌面）；
2. SYSTEM 侧用 `run-in-session.ps1` `CreateProcessAsUser` 到 `winsta0\default`。

## 三、当前状态（2026-09-26 20:1x 实测）

- **没有任何辅助点击程序在运行**：进程表里只有 BackupRestore、cc-switch、
  Parallels Tools 组件和系统进程。
- **自启动项**：HKCU Run 有 `BackupRestoreTest`（BackupRestore.exe）和
  `CC Switch`；启动文件夹为空；无第三方辅助工具。
- **约 70 个 BR* 计划任务存在**（BRClick、BrClick2、BRPE_Click1-5、BRGuiAct、
  BRGuiDismiss、BRGuiSetText、br-gui-send…），多为 **一次性时间触发器且触发时间
  在过去** → 处于 `Ready`、不会再自动跑，属于历史遗留。
- **注入器最后一次真实使用：`clicker-log.txt` 写于 2026-09-16 03:32**（10 天前）。
- **实测调度通道仍可用**：新建不带 `/it` 的任务 → `schtasks /run` 成功，
  clicker 执行并写入 `moved 30,30`（探测后已删除临时任务 BRPROBE_MOVE / BRPROBE_MOVE2）。
- **注意**：带 `/it` 的任务在 `prlctl exec` 上下文里 `schtasks /run` 会报
  `ERROR: Element not found`（exec 不是交互式会话）。要在可见桌面生效，
  走 `run-in-session.ps1`（SYSTEM→交互会话）或让任务的时间触发器自然触发。

## 四、怎么用（2026-09-26 20:3x 实测打通，推荐入口：`tools/win-clicker/br-gui.sh`）

### 4.1 一键入口（宿主 macOS 侧）

```bash
cd tools/win-clicker
./br-gui.sh windows              # 列出窗口：hwnd/标题/矩形/是否前台（点击前先查坐标）
./br-gui.sh click 960 540        # 左键单击（还有 dbl / move / key 0x0D / chord ctrl,p / text）
./br-gui.sh selfcheck            # 自检注入通道（点任务栏空白，用 LastInputTick 判定）
./br-gui.sh shot /tmp/vm.png     # 截图 VM 画面
./br-gui.sh log                  # 查看客体 clicker 执行日志
```

### 4.2 实测确定的三个关键事实（修正本文件早先的说法）

1. **`prlctl exec --current-user` 的进程其实就在 Session 1**（`procSessionId=1 =
   consoleSessionId=1`，能枚举到用户桌面窗口）——不是文档早先写的"Session 0"。
   所以**不需要** schtasks /it，也不需要 SYSTEM 的 run-in-session 就能注入。
2. **但中等完整性会被 UIPI 拦截**：前台窗口是提权的 BackupRestore GUI（High IL）时，
   Medium IL 进程的 `SetCursorPos` 返回 False、光标纹丝不动。
   → 解法：用 `ShellExecute("runas")` 同桌面提权后再注入（`br-gui-exec.ps1` 桥接，
   高完整性可以注入任何窗口）。提权进程看不到 X: 网络盘，所以脚本要先复制到 C:\。
3. **坐标有 DPI 缩放坑**：注入坐标系是 VM 内部 1155x867（物理像素），
   而 `prlctl capture` 截图是 1051x815（逻辑分辨率），**比例 ≈ 1.099（110% 缩放）**。
   从截图量出来的坐标要 ×1.099 才是注入坐标。超界坐标会被静默裁剪
   （实测传 1234 被裁到 1155）。

### 4.3 通道自检判据（别信退出码，信证据）

`br-gui.sh selfcheck` 的硬指标是 `GetLastInputInfo` 的 tick：`SendInput` 注入的
鼠标事件同样会刷新它。tick 不变 = 注入被拦（典型原因：完整性不够 / Session 0），
脚本返回 0 不代表点击生效。当前实测输出 `VERDICT=INPUT_DELIVERED`。

### 4.4 编码坑（PowerShell -File + Add-Type）

`powershell -File` 按**系统 ANSI（GBK）**解码脚本文件。UTF-8 中文注释会变成乱码——
在 PS 段只是难看，但**出现在 `Add-Type` 的 C# 代码块里会直接编译失败**
（报 "Method must have a return type" 之类）。所以新脚本要么全 ASCII，
要么保证 C# 块内零中文（`br-inject-probe.ps1` / `br-gui-selftest.ps1` 即如此）。

### 4.5 其余建议

- 只需点某个已知控件（有 hwnd/控件 ID）→ 优先 `GetDlgItem + SendMessage(BM_CLICK)`，
  不依赖真实光标位置，更稳。
- 中文文本输入别走 `-Text`（bat/命令行编码会坏），用 vmtype.sh 或剪贴板 + Ctrl+V。
- 历史遗留的 70 个 BR* 任务建议清理（见 `docs/cleanup-dev-residue.md`），
  但清理前确认不再依赖其中任何一条。

## 五、新增工具一览（2026-09-26）

| 文件（`tools/win-clicker/`） | 作用 |
|---|---|
| `br-gui.sh` | **宿主一键入口**：windows/move/click/dbl/key/chord/text/selfcheck/shot/log |
| `br-gui-exec.ps1` | 客体桥接：同步脚本到 C:\ → `runas` 提权执行；参数走 base64 绕开 prlctl 吃引号 |
| `br-gui-selftest.ps1` | 注入自检：LastInputTick 判据 + 无害点击任务栏空白 |
| `br-win-list.ps1` | 列窗口（hwnd/pid/矩形/前台标志）+ 屏幕分辨率，stdout 直回宿主 |
| `br-inject-probe.ps1` | 会话/完整性/注入能力探测（med/elev 对照用） |
| `br-gui-probe.ps1` | 早期探测版（保留作对照） |
