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
| `br-agent-tcp.sh` | **宿主首选入口**（TCP 版）：start/ping/click/move/key/chord/text/windows/shot/batch/stop |
| `br-agent-tcp.ps1` | **客体常驻 agent**：提权监听 0.0.0.0:9124，收命令→注入→回结果，空闲 15 分钟自退 |
| `br-agent.sh` / `br-agent.ps1` | 共享目录 UNC 版（免端口，备用；单步 0.2s） |
| `br-gui.sh` | 一次性 exec 注入（不起常驻，1.4s/步） |
| `br-gui-exec.ps1` | 客体桥接：同步脚本到 C:\ → `runas` 提权执行；参数走 base64 绕开 prlctl 吃引号 |
| `br-gui-selftest.ps1` | 注入自检：LastInputTick 判据 + 无害点击任务栏空白 |
| `br-win-list.ps1` | 列窗口（hwnd/pid/矩形/前台标志）+ 屏幕分辨率，stdout 直回宿主 |
| `br-inject-probe.ps1` | 会话/完整性/注入能力探测（med/elev 对照用） |
| `br-gui-probe.ps1` | 早期探测版（保留作对照） |

### TCP 通道（当前首选）

- VM IP（Parallels 共享网段）`10.211.55.13`，端口 `9124`（环境变量 `VM_IP`/`PORT` 可覆盖）。
- 实测：单步 0.20s（其中 ~0.15s 是宿主 python 启动开销，网络往返 <5ms）；
  12 步 batch 0.45s；中文窗口标题完整；截图 base64 回传 110KB。
- 前提：客体防火墙放行（测试 VM 已关闭防火墙）。VM 无公网 IP，不暴露到外网。
- agent 启动走 `br-gui-exec.ps1` 提权（High IL，可注入提权前台窗口），日志在
  客体 `C:\Users\Public\pkg\agt cp.txt`。

## 六、常驻 agent 模式（2026-09-26 21:3x 打通，密集自动化用）

> 为什么又做了常驻：用户确认①大量时间在 Win11 桌面测试（WinRE 只做实机最后验证），
> ②版本迭代多、操作步数累积可观，③VM 是内网临时机无公网 IP，无安全顾虑。
> 于是「每步 1.4s」的 exec 通道成为实际瓶颈，常驻方案收益成立。

### 6.1 用法

```bash
cd tools/win-clicker
./br-agent.sh start            # 启动常驻 agent（提权、Session 1，空闲 15 分钟自动退出）
./br-agent.sh click 960 540    # 单条，往返 ~0.2s
./br-agent.sh batch cmd.txt    # 一次投递多行命令按序执行（50 步 ~0.3s）
./br-agent.sh windows          # 列窗口（完整中文标题）
./br-agent.sh shot vm.png      # agent 侧截图 → _agent/vm.png（1155x867，即注入坐标系）
./br-agent.sh text '中文也行'   # SendInput UNICODE，中文可用
./br-agent.sh stop
```

实测性能：单步 0.2s（原 1.4s，快 7 倍）；10 步批量 0.21s（原 14s，快 67 倍）。

### 6.2 架构与关键发现

- 传输走 **Parallels 共享目录的 UNC 路径** `\\Mac\backupRestore\tools\win-clicker\_agent\`，
  宿主直接写 macOS 本地文件，agent 轮询执行后写回结果——**不开网络端口、不需要 IP**。
- **盘符 X: 在提权进程里不可见，但 UNC `\\Mac\backupRestore` 可见**（实测）——
  这是 agent 能工作的前提。
- **不能复用同一个文件名反复覆盖**：共享目录客户端缓存会让旧内容存活十几秒
  （实测同名覆盖往返 >12s）。必须每条命令用唯一文件名 `cmd.<id>.txt` / `res.<id>.txt`。
- 脚本落地必须 **ReadAllBytes 读源 → UTF8 解码 → GBK(936) 编码写目标**：
  ① UNC 上 `ReadAllText` 会拿到空串；② `Copy-Item` 从共享源复制会产出 0 字节文件；
  ③ 转 GBK 后中文注释显示正常、报错行号不偏移。
- **PowerShell 陷阱：`$out += "字面量" + 表达式` 会丢内容**（实测 `cursor`/`shot`
  两个分支因此静默无输出）。必须先存变量再插值：`$c = ...; $out += "cursor=$c"`。
- P/Invoke 取窗口标题必须 `EntryPoint="GetWindowTextW"` + `CharSet.Unicode`，
  否则按 ANSI marshalling，标题被截成首字符。
- agent 心跳/诊断在 `_agent/heartbeat.txt`（share 路径、elevated、session）。

### 6.3 通道选择结论（更新）

- 键盘优先 `vmkey.sh`（0.6s，全场景覆盖含 WinRE/PE）。
- Win11 桌面内的密集 GUI 自动化用 **`br-agent.sh`**（0.2s/步，批量更快）。
- 偶发一次性操作用 `br-gui.sh`（1.4s，无需 agent 常驻）。
- 网络端口方案仍然不需要：共享目录已给出同量级延迟且零端口。
