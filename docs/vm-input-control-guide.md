# 操作 VM 鼠标键盘 —— 操作手册

> 面向日常使用：怎么在 macOS 宿主上驱动 Parallels 里 Windows 11 虚拟机的键鼠。
> 资产清单与考古记录见 `vm-click-automation-inventory.md`，本文件只讲「怎么用」和「出问题怎么办」。
> 更新：2026-09-27

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
  三条通道都已经走 `runas` 提权，若自己写脚本必须同样提权。
- 确认在 Session 1（用户桌面）。`prlctl exec --current-user` 实测就在 Session 1。

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

## 十、相关文件

| 路径 | 说明 |
|---|---|
| `tools/win-clicker/br-agent-tcp.sh` / `.ps1` | TCP 通道（首选） |
| `tools/win-clicker/br-agent.sh` / `.ps1` | 共享目录通道（备用） |
| `tools/win-clicker/br-gui.sh` | 一次性注入 |
| `tools/win-clicker/br-gui-exec.ps1` | 客体桥接：脚本落 C:\ → `runas` 提权 |
| `tools/win-clicker/vmkey.sh` / `vmtype.sh` | 宿主键盘通道（WinRE/PE 唯一） |
| `tools/win-clicker/_agent-tcp/vmip.txt` | VM IP 缓存 |
| `C:\Users\Public\pkg\agtcp.txt` | 客体 agent 启动日志 |
| `docs/vm-click-automation-inventory.md` | 资产清单与考古记录 |
