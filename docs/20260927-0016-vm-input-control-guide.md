# 操作 VM —— 鼠标键盘 + 提权命令通道（操作手册）

> 面向日常使用：怎么在 macOS 宿主上驱动 Parallels 里 Windows 11 虚拟机的键鼠，
> 以及怎么在 VM 里跑管理员命令（DISM / reagentc / 改 `C:\Recovery`）。
> 资产清单与考古记录见 `20260926-2020-vm-click-automation-inventory.md`，本文件只讲「怎么用」和「出问题怎么办」。
> 更新：2026-09-27

> **先分清两件事**：
> - **要模拟键鼠**（点按钮、打字）→ 下面第一~七节，必须在 Session 1 桌面里注入，需要提权是因为 UIPI。
> - **只要跑命令**（DISM 挂载、`reagentc`、读写 `C:\Recovery`、批量脚本）→ 直接跳到
>   **[第十节：提权命令通道](#十提权命令通道跑命令改系统不需要-runas-桥接)**，
>   `prlctl exec` 不加 `--current-user` 就是 SYSTEM+管理员，**不需要 runas 桥接**，比键鼠通道快得多也稳得多。

---

## 一、先选通道

| 场景 | 用哪个 | 延迟 | 鼠标 | 键盘 | 截图 |
|---|---|---|---|---|---|
| **Win11 桌面，常规操作** | `br-agent-tcp.sh` | 0.2s/步（网络往返 <5ms） | ✅ | ✅ | ✅ |
| Win11 桌面，不想开端口 | `br-agent.sh` | 0.2s/步 | ✅ | ✅ | ✅ |
| 偶发一次，不想起常驻 | `br-gui.sh` | 1.4s/步 | ✅ | ✅ | ✅ |
| **WinRE / WinPE / BIOS 菜单** | `vmkey.sh` | 0.6s/次 | ❌ | ✅ | ❌ |

**选择规则**：

1. 能用键盘解决的优先用键盘（`vmkey.sh`）—— 覆盖面最广，WinRE/PE 里唯一可用。
2. 需要鼠标/坐标 → Win11 桌面内一律 `br-agent-tcp.sh`。
3. 进入 WinRE/PE 后，鼠标通道**全部失效**（Parallels Tools 不运行、没有网络栈），
   只能用 `vmkey.sh` 发按键。退出 WinRE 菜单实测：`./vmkey.sh enter`（选高亮「继续」）。

---

## 二、TCP agent（首选）

客体里常驻一个提权 PowerShell agent，监听 `0.0.0.0:9124`，收命令 → 注入 → 回结果。

### 启动与状态

```bash
cd tools/win-clicker
./br-agent-tcp.sh start      # 启动，自动探测 VM IP 并等端口就绪
./br-agent-tcp.sh ping       # UP / DOWN
./br-agent-tcp.sh ip         # 当前 VM IP（Parallels 共享网段 10.211.55.x）
./br-agent-tcp.sh stop       # 让 agent 退出
```

- VM IP 自动探测并缓存到 `_agent-tcp/vmip.txt`；环境变量 `VM_IP`、`PORT` 可覆盖。
- **agent 空闲 15 分钟自动退出**，超时后重跑 `start` 即可。
- 启动日志在客体 `C:\Users\Public\pkg\agtcp.txt`，正常会看到
  `BRAGENT_LISTENING port=9124 elevated=True session=1`。
- 前提：客体防火墙放行 9124（测试 VM 已关闭防火墙）；VM 在内网无公网 IP。

### 操作命令

```bash
./br-agent-tcp.sh windows            # 列窗口：hwnd/pid/矩形/是否前台/完整中文标题 —— 点之前先查这个拿坐标
./br-agent-tcp.sh click 960 540      # 左键单击
./br-agent-tcp.sh dbl 960 540        # 双击
./br-agent-tcp.sh move 960 540       # 只移动光标
./br-agent-tcp.sh key 0x0D           # 虚拟键（0x0D=回车）
./br-agent-tcp.sh chord ctrl,p       # 组合键
./br-agent-tcp.sh text '中文也行'    # UNICODE 输入，中文正常
./br-agent-tcp.sh cursor             # 当前光标位置
./br-agent-tcp.sh tick               # GetLastInputInfo tick（判断输入是否被接收）
./br-agent-tcp.sh screen             # 屏幕分辨率
./br-agent-tcp.sh shot /tmp/vm.png   # 截图，base64 回传存盘
./br-agent-tcp.sh batch ops.txt      # 一次发多行，每行一条命令
```

`ops.txt` 示例：

```
move 500 300
click 578 855
text hello
windows
```

### 实测性能

- 单步 0.20s（其中 ~0.15s 是宿主 python 启动开销，**网络往返本身 <5ms**）
- 12 步批量 0.45s（含 10 字符中文输入）
- 截图 1155x867 → PNG 约 110KB，回传无压力

---

## 三、br-gui.sh（一次性，不起常驻）

```bash
./br-gui.sh windows            # 列窗口
./br-gui.sh click 960 540      # 单击（另有 dbl / move / key / chord / text）
./br-gui.sh selfcheck          # 通道自检（点任务栏空白，无害）
./br-gui.sh shot /tmp/vm.png   # 截图
./br-gui.sh log                # 客体注入日志
```

每次调用都会重新 `Add-Type` 编译 C#，所以慢（1.4s）。**适合偶发操作，不适合循环。**

---

## 四、br-agent.sh（共享目录版，免端口备用）

与 TCP 版命令相同，传输走 Parallels 共享目录 UNC
`\\Mac\backupRestore\tools\win-clicker\_agent\`，**不开端口**。

```bash
./br-agent.sh start            # 启动（提权，空闲 15 分钟自退）
./br-agent.sh click 960 540
./br-agent.sh batch ops.txt
./br-agent.sh status           # 看 heartbeat.txt（含 idle-exit 时间）
./br-agent.sh stop
```

TCP 通道不可用时（比如防火墙不让开端口）用它。

---

## 五、vmkey.sh / vmtype.sh（宿主键盘，全场景）

```bash
./vmkey.sh enter                 # 单键
./vmkey.sh ctrl+p                # 组合键
./vmkey.sh alt+f4
./vmkey.sh up / down / left / right / tab / esc / win
./vmkey.sh f5                    # F 键走 scancode 通道，实测有效
./vmtype.sh 'E:\backup.wim'      # 逐字符输入（ASCII 路径/命令用）
```

**这是 WinRE / WinPE / BIOS 菜单里唯一的自动化手段。**

已知特性：

- F 键必须用 scancode（`-s`）通道：Parallels 的 `-k` 键码对 F 键映射错误，
  实测 `-k 71/76/116` 都不触发 F5，`-s 63` 有效（脚本已自动处理）。
- `vmtype.sh` 用 UUID 而非 VM 名（含空格的 VM 名在部分通道静默失败）。
- 只支持 ASCII，中文别用它。

---

## 六、坐标系（必读）

| 来源 | 分辨率 | 说明 |
|---|---|---|
| **注入坐标 / TCP agent 截图 / `windows` 矩形** | **1155x867** | 物理像素，直接用 |
| `prlctl capture "Windows 11" --file x.png` | 1051x815 | 逻辑分辨率，**110% DPI** |

- 从 `prlctl capture` 的截图量出的坐标要 **×1.099** 才是注入坐标。
- 用 `br-agent-tcp.sh shot` 拿到的截图**就是注入坐标系**，不用换算——优先用这个。
- 超界坐标会被**静默裁剪**（传 1234 会被裁到 1155），不会报错。

---

## 七、判定注入是否真的生效

**别信退出码，也别信脚本日志。** 硬指标是 `GetLastInputInfo` 的 tick：

```bash
./br-agent-tcp.sh tick      # 记下 tick
./br-agent-tcp.sh click 960 540
./br-agent-tcp.sh tick      # tick 变了 = 系统确实收到了输入
```

`SendInput` 注入的事件同样会刷新这个 tick。tick 不变就是被拦了。

---

## 八、故障排查

### 8.1 端口 DOWN

```bash
./br-agent-tcp.sh ping      # DOWN
```

原因通常是 agent 空闲 15 分钟自退了。重跑 `./br-agent-tcp.sh start`。
若 `start` 后仍 DOWN，查客体日志 `C:\Users\Public\pkg\agtcp.txt`。

### 8.2 注入没反应（tick 不变）

- 前台是**提权**窗口（BackupRestore GUI 就是）时，中等完整性进程的注入会被 UIPI 静默拦截。
  三条键鼠通道都已经走 `runas` 提权，若自己写脚本必须同样提权。
- 确认在 Session 1（用户桌面）。`prlctl exec --current-user` 实测就在 Session 1。
- 注意：**只是跑命令不要走这条路**。`prlctl exec` 不加 `--current-user` 直接是 SYSTEM+管理员，
  见第十节。SYSTEM 在 Session 0，反过来**不能**用来注入键鼠。

### 8.3 VM 切到前台后，连 macOS 光标都动不了（左右键正常）

这是 **Parallels SmartMouse 捕获没释放**，不是配置坏、不是 CPU 打满、不是 Tools 崩。

处理顺序：

1. 按 **Ctrl+Alt**（Parallels 释放鼠标捕获的快捷键）
   —— 注意：不能用 `vmkey.sh` 代替，那是发给客体的，宿主侧捕获不认。
2. `Cmd+Tab` 切到别的 app 再切回来。
3. 退出全屏模式（全屏下 SmartMouse 最容易卡）。
4. 根治：关闭 SmartMouse。当前 VM 已设为 `Enabled=0`
   （`~/Parallels/Windows 11.pvm/config.pvs` 的 `<SmartMouse>` 节）。

   ⚠ **`MouseSync` / `MouseVtdSync` 必须保持 1**，它们负责鼠标同步，关掉会导致光标漂移。

   另注：`prlctl set mouse` 在 26.4.2 上**不可用**（报 `Unrecognized option`），
   只能通过 Parallels GUI 改，或停机后手改 `config.pvs`。
   VM 运行中改的配置，关机时可能被内存中的配置回写覆盖——改完下次关机后复查一次。

### 8.4 客体 CPU 看起来 100%

`LoadPercentage`（WMI）是瞬时值，容易误读。要看真实占用，得采样两次
`TotalProcessorTime` 求差值（脚本：`.test-artifacts/cpu-delta.ps1`）。

---

## 九、写新的注入脚本必须遵守的约束

踩过的坑，再写一遍脚本前先读：

1. **`Add-Type` 的 C# 代码块必须全 ASCII**。
   `powershell -File` 按 GBK 解码脚本，C# 段里的 UTF-8 中文会直接编译失败。
2. **UNC 共享目录上 `ReadAllText` 拿到空串、`Copy-Item` 复制产出 0 字节文件**。
   必须 `ReadAllBytes` → UTF8 解码 → GBK(936) 写目标。
3. **共享目录同名文件覆盖有缓存**（实测往返 >12s）。
   命令/结果必须用唯一文件名（`cmd.<id>.txt` / `res.<id>.txt`）。
4. **PowerShell 陷阱：`$out += "字面量" + 表达式` 会丢内容**（`cursor`/`shot` 分支因此静默无输出）。
   必须先存变量再插值：`$c = [BrAgt]::Cur(); $out += "cursor=$c"`。
5. **P/Invoke 取窗口标题要 `EntryPoint="GetWindowTextW"` + `CharSet.Unicode`**，
   否则按 ANSI marshalling，标题被截成首字符。
6. **提权进程看不到盘符 `X:`，但看得到 UNC `\\Mac\backupRestore`**。
   要提权执行的脚本必须先落到客体 `C:\`。
7. `prlctl exec` 命令行会吃掉 `$_`、`\` 等字符，复杂参数走 `-File` 脚本或 base64 传递。

---

## 十、提权命令通道（跑命令 / 改系统，不需要 runas 桥接）

2026-09-27 实测发现，比旧的 `runas` 桥接（`br-gui-exec.ps1` / `elev-bridge.ps1`）简单得多。
**只要在 VM 里跑命令、不需要模拟键鼠，一律用这条。**

```bash
# SYSTEM + 管理员，DISM / reagentc / 写 C:\Recovery 全部直接可用
prlctl exec "Windows 11" cmd /d /c "whoami & net session >nul 2>nul && echo ADMIN_YES || echo ADMIN_NO"
# → nt authority\system / ADMIN_YES

# 标准套路：脚本从共享目录拷进 VM，再本地执行（见下方坑 2、3）
prlctl exec "Windows 11" cmd /d /c "copy /y \\\\Mac\\backupRestore\\.test-artifacts\\x.ps1 C:\Users\Public\pkg\\x.ps1 & powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\pkg\\x.ps1"
```

**什么时候不能用它**：需要模拟键鼠、操作 GUI 窗口时——SYSTEM 在 Session 0，没有桌面交互，
键鼠注入必须回到 `--current-user` + runas 提权的通道（第一~七节）。

### 五个必须遵守的点（2026-09-27 23:30 实测修正）

0. **【最关键】SYSTEM 模式必须用「多参数」姿态**：
   `prlctl exec "Windows 11" cmd.exe /c "whoami"` —— `cmd.exe` 是程序，`/c` 和脚本是后续参数。
   **不能**把 `"cmd.exe /c whoami"` 整体当一个引号串传给 `prlctl exec`：SYSTEM 模式（不加
   `--current-user`）会把它当成「程序名 = cmd.exe /c whoami」（含空格），找不到 → exec 静默失败、
   stdout 为空。2026-09-27 当晚一度误判「exec 通道全断」，根因就是这个姿势错误，不是基础设施坏。

1. **不加 `--current-user`** 才是 SYSTEM；加了就是普通用户的 Session 1（中等完整性，会被 UIPI 拦）。

2. **`--current-user` 在 VM 冷启、尚无登录会话时返回固定占位 `p8b6\x^M`**（每次都一样、8 字节），
   **不是通道坏了，是无会话**。判别法：用 SYSTEM 模式跑 `whoami`，返回干净 `nt authority\system`
   即证明通道活；返回 `p8b6\x` 则是 `--current-user` 缺会话。要彻底用 `--current-user`，先让 VM 登录。

3. SYSTEM 看不到 `X:` 盘符；UNC `\\Mac\backupRestore\...` **读**取决于共享盘状态，但
   **【2026-09-27 23:30 实测】VM→宿主共享盘写入失败**（冷启 + 新 dispatcher 后仍写不进，
   `echo > \\Mac\...` 不落地）。因此**结果回传改走 exec stdout**（见下方清单），不再依赖共享盘写。

4. **脚本内联优先**：共享盘写坏后，把逻辑直接塞进 `powershell.exe -Command "<内联>"`（多参数），
   复杂脚本用 Here-String 先写 VM 本地 `C:\Users\Public\pkg\`（exec `cmd /c echo` 或 powershell 写），
   再 `powershell -File` 本地路径执行。

5. **stdout 是 GBK 编码**：中文会乱码但内容正确；宿主侧读回后用 `iconv -f GBK -t UTF-8`
   或 python `bytes.decode('gbk')` 解码。多行 / 长输出（如 `reagentc /info`）正常回传。

6. 命令行反斜杠：多参数姿势下引号内写单 `\`（zsh 不转义）；VM 内 UNC 是 `\\Mac\...`。
   取长度用 `[System.IO.FileInfo]::new($p).Length`（`Get-Item` 对几百 MB 的 WIM 会报「找不到路径」，
   但 `Get-FileHash` 正常）。

### 已验证可用清单（2026-09-27 23:30 复核）

| 操作 | 结果 |
|---|---|
| `dism /Mount-Image`、`/Unmount-Image /Commit`、`/Get-ImageInfo` | ✅（SYSTEM 多参数） |
| `reagentc /info`（多行）、`/boottore` | ✅ |
| 读写 `C:\Recovery\WindowsRE\`（含覆盖 `Winre.wim`） | ✅（VM 本地） |
| **启动 VM 程序并取回 stdout**（GBK，多行/长输出正常） | ✅ **主结果回传通道** |
| 复制文件**进** UNC `\\Mac\backupRestore`（VM→宿主写） | ❌ 当前坏，改用 stdout 回传 |
| 触发 `shutdown /r`（会真的重启 VM，注意后果） | ✅ |

实例脚本（可直接抄）：`.test-artifacts/v176-step{1..7}-*.ps1`；
说明见 [`20260927-1047-winre-payload-and-p0-fix.md`](20260927-1047-winre-payload-and-p0-fix.md) 第 9 节。

---

## 十一、相关文件

| 路径 | 说明 |
|---|---|
| `tools/win-clicker/br-agent-tcp.sh` / `.ps1` | TCP 通道（首选） |
| `tools/win-clicker/br-agent.sh` / `.ps1` | 共享目录通道（备用） |
| `tools/win-clicker/br-gui.sh` | 一次性注入 |
| `tools/win-clicker/br-gui-exec.ps1` | 客体桥接：脚本落 C:\ → `runas` 提权 |
| `tools/win-clicker/vmkey.sh` / `vmtype.sh` | 宿主键盘通道（WinRE/PE 唯一） |
| `tools/win-clicker/_agent-tcp/vmip.txt` | VM IP 缓存 |
| `C:\Users\Public\pkg\agtcp.txt` | 客体 agent 启动日志 |
| `docs/20260926-2020-vm-click-automation-inventory.md` | 资产清单与考古记录 |
