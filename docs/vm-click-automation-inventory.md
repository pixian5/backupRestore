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

## 四、使用建议

- 需要真实鼠标/键盘注入 → 用 `run-in-session.ps1` 拉起 `clicker.ps1`，
  或建 `/it` 任务并**用未来时间触发**（不要指望 `schtasks /run`）。
- 只需点某个已知控件（有 hwnd/控件 ID）→ 优先 `GetDlgItem + SendMessage(BM_CLICK)`，
  不必依赖真实光标位置，更稳。
- 坐标是 **VM 内部帧缓冲像素**（与 `prlctl capture` 截图一致），不是宿主屏幕坐标。
- 历史遗留的 70 个 BR* 任务建议清理（见 `docs/cleanup-dev-residue.md`），
  但清理前确认不再依赖其中任何一条。
