# 操作通道全指南与对比（如何驱动测试 VM「Windows 11」）

> 本文是操作 Parallels 测试 VM 的**唯一权威手册**，合并了豆包与 Trae 两边全部实测经验。
> 结论先行：**Parallels 会丢弃一切"程序合成"的鼠标事件**（无论该程序跑在 macOS 宿主
> 还是 Win11 虚拟机内部），所以**鼠标通道彻底不可用**；可靠通道是**键盘注入 + 命令行 +
> GUI 全键盘导航**（v1.5.9 起 BackupRestore GUI 已支持纯键盘操作）。

---

## 一、全通道对比总表（核心结论）

| # | 通道 | 机制 | 实测结果 | 可靠性 | 适用场景 | 限制 |
|---|---|---|---|---|---|---|
| 1 | **prlctl send-key-event（宿主导入键盘）** | 走 Parallels 官方输入管道（`-k` 虚拟键码 / `-s` 扫描码），不经过 macOS CGEvent | ✅ 组合键（Ctrl+P）、单键（回车/Tab/F5）、Win 键全部有效 | ★★★★★ 首选 | 驱动 VM 内任何 GUI：快捷键、Tab 遍历、回车确认、方向键 | 只能键盘；F 键必须用 `-s` 扫描码（见 §六） |
| 2 | **prlctl exec（命令执行）** | 在 VM 内以指定用户跑命令 | ✅ 普通命令成功；需系统级权限的操作失败 | ★★★★ | 复制文件、启动脚本、查状态 | 跑在 Session 0（无桌面）；令牌 UAC 过滤（Medium，无法提权）；引号会被剥 |
| 3 | **run-in-session.ps1（交互会话启动）** | WTS QueryUserToken + CreateProcessAsUser 在 Session 1 启动进程 | ✅ GUI 能出现在用户可见桌面 | ★★★★ | 启动 BackupRestore.exe、diag2、activate 等需要可见桌面的程序 | 需要 SYSTEM/SeTcbPrivilege（exec 默认可用） |
| 4 | **prlctl capture（截图）** | 抓 VM 帧缓冲 | ✅ 2048×1472 清晰 | ★★★★★ | "看"VM 画面、验证界面变化 | 无 |
| 5 | **VM 内程序注入键盘（clicker.ps1 keybd_event/SendInput）** | 在 Win11 里用 user32 API 模拟按键 | ✅ 单键（ASCII 字母）能打字 | ★★★ | 向有焦点的文本框输入 | **组合键/修饰键不可靠**（Win 键、Ctrl+P 对 GUI 无效）——合成事件不进入 Parallels 输入管道 |
| 6 | **VM 内程序注入鼠标（clicker.ps1 SetCursorPos+mouse_event/SendInput）** | 在 Win11 里模拟鼠标点击 | ❌ 点击"显示桌面"无反应 | ✗ 不可用 | —— | **Parallels Tools 鼠标通道不订阅合成鼠标事件**；人手物理鼠标走 IOHID 硬件专线，程序合成事件走应用层分发，不进入 Parallels 订阅管道 |
| 7 | **macOS 宿主 AX 点击（computer-use / 辅助功能）** | 通过 macOS AX API 对 Parallels 窗口发点击 | ❌ 对 VM 窗口点按钮无效果 | ✗ 不可用 | —— | macOS 合成的 CGEvent 鼠标事件不被 Parallels 转发给 Guest（SmartMouse 只接受物理鼠标捕获） |
| 8 | **prlctl 鼠标注入** | Parallels 官方 CLI | ❌ **无此能力**（无 mouseputxy 类命令） | ✗ 不存在 | —— | Parallels 明确缺失 Guest 鼠标注入接口（对比 VirtualBox VBoxManage、QEMU QMP 都有） |
| 9 | **RDP / Windows App** | RDP 协议独立 HID 通道 | ⚠️ 可手动用，但需登录态/前台窗口 | ★★ | 人工介入、看画面 | 会开新会话抢焦点；自动化坐标属于 RDP 会话空间；Windows 11 Home 默认无 RDP Server |
| 10 | **USB HID 虚拟设备直通** | 模拟 USB 鼠标直通给 Guest | 未实测（配置重、无现成封装） | 理论可行 | 需要硬件级鼠标事件 | 实现成本高，不优先 |

**一句话总结**：鼠标（6/7/8）三条路全死，键盘两条路（1=宿主官方通道 / 5=VM 内单键）活着，
其中 **prlctl send-key-event（通道 1）是最稳、最快的**，一切自动化优先走它。

---

## 二、直接操作 VM 的工具链（tools/win-clicker/）

| 工具 | 作用 | 示例 |
|---|---|---|
| `vmkey.sh` | 一行注入任意键/组合键（封装键码+扫描码） | `./vmkey.sh ctrl+p` / `./vmkey.sh enter` / `./vmkey.sh f5` / `./vmkey.sh shift+tab` |
| `clicker.ps1` | VM 内注入器（鼠标/键盘/文本/组合键） | `powershell -File clicker.ps1 -Key 0x41`（A 键） |
| `run-in-session.ps1` | 在 VM 交互桌面（Session 1）启动程序 | `powershell -File run-in-session.ps1 -SessionId 1 -Command "C:\...\BackupRestore.exe"` |
| `diag2.ps1` | 跨线程查前台窗口的焦点控件 ID/类名/文本（键盘导航定位用） | 结果写 `C:\Users\Public\backupRestore-package\diag2.txt` |
| `activate.ps1` | 把后台窗口拉回前台（AppActivate） | `powershell -File activate.ps1 -Title BackupRestore` |
| `diag-fg.ps1` | 查前台窗口句柄/标题/类名/焦点（早期诊断） | 输出到控制台/文件 |
| `listwin.ps1` | 列出窗口 | 诊断用 |

**文本输入工具（Trae 后续新增，避免逐字注入）**：

| 工具 | 作用 |
|---|---|
| `tools/win-clicker/type_text.sh` | 向 VM 输入一整段文本，**小写→大写自动加 shift**，支持 `\` `/` `:` 等符号路径 |
| `tools/win-clicker/vmtype.sh` | 文本输入（与 type_text 类似） |
| `tools/win-clicker/set-image-path.ps1` | 直接设置 GUI 镜像路径输入框（配 `set-image-path.bat`） |
| `tools/win-clicker/switch-tab.ps1` | 切 GUI tab（配 `switch-tab.bat`） |
| `tools/win-clicker/clickbtn.ps1` | 按控件 ID 点击按钮（VM 内） |
| `tools/win-clicker/dump-controls.ps1` | 枚举窗口控件树（配 `dump-controls.bat`） |
| `tools/win-clicker/send-wm.ps1` / `sendwm-elev.ps1` | 发送 WM_COMMAND 等消息到 GUI（提权/诊断变体） |

> 注：目录里大量 `_` 开头的 `.ps1/.b64` 是历次调试的一次性脚本（BCD 探测、WinRE 注入、
> DISM 测试等），仅存档，日常操作不需要；`captures/` 是历史截图存档。

### vmkey.sh 支持的全部键名

```
修饰键：ctrl / alt / shift / win
单键：esc  enter  tab  space  backspace  up  down  left  right  home  end
      pgup  pgdn  insert  delete  f1..f12
字母：a-z（注意是键盘物理行序，见 §六）
数字：0-9
组合：ctrl+p / alt+f4 / ctrl+shift+p / shift+tab / ctrl+enter
```

---

## 三、prlctl exec 关键坑（勿再踩，Trae 实测）

1. **语法**：必须**不加 `--`**，且 VM 名字含空格时用 **UUID**（不是名字）。
   正确：`prlctl exec <UUID> --user x --password 1 cmd /c "echo HI"`。
   加 `--` 或用含空格的名字 → 静默 exit 2（此前所有"exec 损坏"判断均因此误判）。
2. **引号剥除**：`prlctl exec` 会剥掉命令里的双引号 → 不要在 `-Command "..."` 里依赖双引号。
   可靠做法：把 PS 脚本写成文件再 base64（`-EncodedCommand`，UTF-16LE → base64）传参，或直接传脚本文件路径。
3. **exec 身份**：令牌是 **UAC 过滤**（Medium，Admins deny-only，仅 SeShutdown）。
   `bcdedit /enum`、`reagentc`、挂 `S:`、`schtasks /create /rl HIGHEST`、`runas` 全部失败，无法非交互提权。
   → 需要系统级 prep（BCD/RE/ESP）时：**让程序自己提权**（GUI 的 ShellExecuteW runas 弹 UAC），
   或 run-in-session 在交互会话里跑，或走 WinRE/BCD 修复通道（§七）。
4. **exec 默认 Session 0（无桌面）**：GUI 起不来也看不见 → 用 `run-in-session.ps1` 进 Session 1。

---

## 四、键盘注入 vs 鼠标注入：为什么一个通一个死

### 鼠标（死）
- **机制**：Windows 把鼠标事件分两路——物理鼠标走 IOHID 硬件专线 → Parallels Tools 驱动 → Guest 输入管线（可信源头）；程序用 `SendInput`/`mouse_event` 合成的点击走应用层分发，**Parallels Tools 鼠标通道不订阅**。
- **证据**：VM 内 clicker.ps1 点"显示桌面"无反应；宿主 AX 点 VM 窗口无效果；prlctl 无鼠标注入命令。
- **结论**：无论模拟鼠标的程序跑在 Mac 还是 Win11 里，Parallels 一律不认。**不要再尝试任何鼠标自动化**。

### 键盘（活）
- **宿主通道**（prlctl send-key-event）：Parallels 官方输入管道，与物理键盘同源，100% 有效。
- **VM 内通道**（keybd_event）：单键 ASCII 有效；**组合键不可靠**（合成 Ctrl/Win 不进入管道）。
- **结论**：GUI 必须支持纯键盘操作（本项目 v1.5.9 起已满足：Ctrl+B/R/P 切页、Tab 遍历、方向键切单选、回车默认按钮、Esc 取消、F5 刷新、Ctrl+Enter 创建任务）。

---

## 五、GUI 键盘导航快捷键（v1.5.9+）

| 快捷键 | 作用 |
|---|---|
| Ctrl+B | 切到备份 tab |
| Ctrl+R | 切到单系统还原 tab |
| Ctrl+P | 切到 PE 恢复 tab |
| Ctrl+O | 读取镜像 |
| F5 | 刷新环境 |
| Ctrl+Enter | 创建任务 |
| Tab / Shift+Tab | 焦点前进/后退 |
| ↑/↓/←/→ | 单选组内切换（操作模式、RAM disk/硬盘启动） |
| Enter | 触发默认按钮（创建任务 / 进入 PE 弹窗确认） |
| Esc | 关闭模态弹窗 / 取消 |

**已验证**（实机截图核验）：Ctrl+P/R/B 切页、Ctrl+O 空路径弹"参数校验失败"、回车关弹窗、
Tab 焦点序列（可用 diag2.ps1 读出控件 ID）、方向键切操作模式（探测→备份→还原→第二系统→PE）、
方向键切 RAM disk/硬盘启动（1412↔1413，目录行显隐联动）、F5 刷新（日志 `GUI action completed:
refresh environment`）。

---

## 六、键码速查（Parallels，实测校准）

### `-k` 虚拟键码（Parallels 官方表，十进制）
```
esc=9  1..0=10..19  backspace=22  tab=23  enter=36  space=65
ctrl=37  alt=64  shift=50  win=115  menu=117
home=97  up=98  pgup=99  left=100  right=102  end=103  down=104  pgdn=105  insert=106  delete=107
```
**字母键按键盘物理行序（QWERTY），不是字母序**：
```
q=24 w=25 e=26 r=27 t=28 y=29 u=30 i=31 o=32 p=33
a=38 s=39 d=40 f=41 g=42 h=43 j=44 k=45 l=46
z=52 x=53 c=54 v=55 b=56 n=57 m=58
```
> 实测：方向键 98/104/100/102、修饰键、字母键均正确。

### `-s` 扫描码（PS/2 set1，F 键必须用这个！）
```
F1=59 F2=60 F3=61 F4=62 F5=63 F6=64 F7=65 F8=66 F9=67 F10=68 F11=87 F12=88
```
> **坑**：Parallels 的 `-k` 对 F 键映射错误（-k 71/76/116 都不能触发 F5），
> 只有 `-s 63`（PS/2 set1 F5 扫描码）实测能触发。vmkey.sh 已内置此映射，直接用 `./vmkey.sh f5`。

---

## 七、WinRE / BCD 修复通道（VM 进不了 Windows 时）

当 VM 引导循环 / BCD 丢条目标时，用 **Windows 安装 ISO 引导 WinPE + bcdboot 重建**
（详见 [winre-bcd-loop-fix-2026-09-16.md](winre-bcd-loop-fix-2026-09-16.md)）：

1. ISO：`~/Downloads/Win11_25H2_Chinese_Simplified_Arm64_v2.iso`。
2. 停 VM → `--device-set cdrom0 --image <iso> --connect` → `--device-bootorder "cdrom0 hdd0"`。
3. 启动出现 "Press any key..." 时 `./vmkey.sh space` 进安装程序。
4. `Shift+F10` 开 cmd（`./vmkey.sh shift+f10`）→ diskpart 把 ESP 挂为 S:。
5. `bcdboot C:\Windows /s S: /f UEFI`（**必须带 /s 和 /f UEFI**）重建引导条目。
6. 验证 `bcdedit /store S:\EFI\Microsoft\Boot\BCD /enum {default}`。
7. 卸 ISO、恢复 `hdd0 cdrom0`，重启。

### 关键结论：WinRE RAMDISK 引导在 Parallels ARM 上不可行
- `ramdisk=[C:]\...\Winre.wim`(803MB) 引导恒失败（黑屏~2min 回落）；8GB 内存、注入版/原始
  WIM、C:/Y: 位置均无效；但**从 ISO(UDF) 引导同体积 WinPE 成功**。
- 锁定为 **bootmgr 对该 VM 的 ntfs RAMDISK 大 WIM 读取失败**（环境限制，非软件 bug）。
- VM 内存当前 **6GB**（8GB 曾诱发引导异常；改 `--memsize` 须先停 VM）。
- 2026-09-17 起已确认**注册 WinRE 恢复链路可用**（`reagentc /boottore` → WinRE →
  `Recovery.exe recover-env` → `wpeutil reboot` → Windows，见 current-status-2026-09-16.md）。

---

## 八、推荐工作流（日常开发操作模板）

### A. 看 VM 当前画面
```bash
prlctl capture "Windows 11" --file /tmp/vm-shot.png && open /tmp/vm-shot.png   # 或直接 Read
```

### B. 驱动 GUI 做一件事（示例：切到 PE 恢复 tab 并按 F5）
```bash
cd /Users/x/code/backupRestore
./tools/win-clicker/vmkey.sh ctrl+p   # 切 PE tab
./tools/win-clicker/vmkey.sh f5       # 刷新环境（scancode 通道）
prlctl capture "Windows 11" --file /tmp/vm-shot.png   # 截图验证
```

### C. 确认焦点在哪个控件（键盘导航定位）
```bash
prlctl exec <UUID> cmd /c "powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\backupRestore-package\run-in-session.ps1 -SessionId 1 -Command \"powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\backupRestore-package\diag2.ps1\""
prlctl exec <UUID> cmd /c "type C:\Users\Public\backupRestore-package\diag2.txt"
# 输出示例：focusid=1412 class='Button' text='RAM disk（不占分区）'
```

### D. 启动/重启 GUI 到可见桌面
```bash
prlctl exec <UUID> cmd /c "taskkill /f /im BackupRestore.exe >nul 2>&1 & copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\backuprestore-cli.exe C:\\Users\\Public\\backupRestore-package\\BackupRestore.exe >nul & copy /y ... Recovery.exe >nul"
prlctl exec <UUID> cmd /c "powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\backupRestore-package\run-in-session.ps1 -SessionId 1 -Command \"C:\Users\Public\backupRestore-package\BackupRestore.exe\""
```

### E. GUI 被抢前台（注入无效时）
```bash
prlctl exec <UUID> cmd /c "powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\backupRestore-package\run-in-session.ps1 -SessionId 1 -Command \"powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\backupRestore-package\activate.ps1 -Title BackupRestore\""
# 再 diag2 确认 fg=BackupRestoreNativeGui
```

### F. 查看 GUI 日志（验证操作是否真正执行）
```bash
prlctl exec <UUID> cmd /c "powershell -NoProfile -Command \"Get-Content C:\\Users\\Public\\backupRestore-package\\logs\\gui.log -Tail 5\""
```

---

## 九、踩坑全集（按层分类）

### 宿主导入层（prlctl）
1. **F 键必须用 `-s` 扫描码**，`-k` 映射错（§六）。
2. **注入前确认 GUI 在前台**：否则按键落在开始菜单/Parallels 代理窗口
   （`prl_cc_fgproxy`）。用 activate.ps1 拉回 + diag2 确认。
3. **Ctrl 检测**：程序里用 `GetAsyncKeyState`（即时物理状态）而非 `GetKeyState`
   （消息队列有延迟，prlctl 注入的 Ctrl 偶发读不到）——v1.5.9 已修。
4. **F5 被 IsDialogMessage 消费**：消息循环里要在 IsDialogMessage **之前**拦截 F5——
   v1.5.9 已修。

### VM 内注入层（clicker.ps1 / run-in-session.ps1）
5. **PowerShell 5.1 按 GBK 读 UTF-8 文件**：C# here-string 里**任何中文注释都会吞行**
   导致 Add-Type 编译失败 → clicker.ps1 的 C# 段必须纯英文注释。
6. **CreateEnvironmentBlock 缺失 → 子进程 0xC0000142**；还需
   `CREATE_UNICODE_ENVIRONMENT=0x00000400`，否则 create_failed=87。
7. **显式 lpDesktop="winsta0\default" → 0xC0000142**；改 lpDesktop=null。
8. **CREATE_NEW_CONSOLE → 隐藏控制台抢焦点**，Text 输入落错窗口 → 改
   `CREATE_NO_WINDOW=0x08000000`。
9. **keybd_event 组合键不可靠**（§四）——组合键一律走 prlctl。
10. **Chord 解析**：`Convert.ToByte("p",16)` 抛异常 → 字母映射 VK（a→0x41），
    且注意键盘物理行序。

### exec / 会话层（Trae 实测）
11. exec 语法：不加 `--`、空格名用 UUID（§三-1）。
12. 引号剥除：用脚本文件 + EncodedCommand（§三-2）。
13. exec 令牌 UAC 过滤无法提权（§三-3）——系统级操作让程序自己提权。

### 行为层（踩过的坑）
14. **Parallels 丢弃合成鼠标事件**：鼠标自动化全部不可行（§四）。
15. **GUI 必须前台才收键**（§九-2）。
16. **切 tab 后控件重叠**：v1.5.2 修复过（IsDialogMessage + 布局重排），若再现优先查
    set_child_visible 与窗口尺寸。

---

## 十、当前测试链路状态（2026-09-16 基线，详见 current-status-2026-09-16.md）

- 测试 VM：`Windows 11`（UUID `{caee9cb3-bac7-41e2-85f2-32b3a7369114}`）。
- 程序版本：v1.6.4（GUI 标题显示）；程序目录 `C:\Users\Public\backupRestore-package\`。
- 防假标记：`C:\br-test\marker.txt`（`CDRIVE-VERIFY-MARKER`）+ `marker-nonce.txt` +
  `keepfile.txt`；备份目标 `E:\br-cdrive-v1.wim`（E: = br.hdd 独立盘）。
- 已确认真实 WinRE 恢复链路（probe 任务 success/100%）；未覆盖：真实 Capture/Apply、
  格式化、BCDBoot、第二系统启动、故障注入矩阵。
- WinRE 卡死根因（v1.6.2-v1.6.4 已修）：对 C:→Z: 逐个 `mountvol /L` 阻塞 → 改
  `GetVolumePathNamesForVolumeNameW` 按卷 GUID 直查 + 30s 超时。
