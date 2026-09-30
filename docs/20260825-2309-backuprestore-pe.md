# BackupRestorePE.wim 构建记录

## 产物

已在 Windows 11 ARM64 VM 中使用已安装的 Windows ADK ARM64 WinPE 基础镜像生成：

- VM 路径：`C:\BackupRestorePE\BackupRestorePE.wim`
- 仓库副本：`artifacts/BackupRestorePE.wim`
- ARM64 可启动 ISO：`artifacts/BackupRestorePE.iso`
- 项目源码当前版本：`1.0.8`
- 已验证产物构建版本：`0.8.5`（本轮仅修订文档与测试边界，未重建 WIM/ISO）
- WIM SHA-256：`d5f1515acc2a5bf5d244048b9b9b1975f88433c18b4e13c15f3177393c3d17cc`
- ISO SHA-256：`4bd7b7c567bdbbfe61a24eeeca50d044172141bd02bedb7db47ca48fe9eeadc5`
- WIM 大小：约 350 MiB（LZX）
- 架构：ARM64

## 内容边界

产物以 ADK `arm64\en-us\winpe.wim` 为基础，并写入：

- Windows RE/WinPE 基础系统和基本图形支持
- ADK 基础镜像自带的 NTFS、FAT32、存储、NVMe/SATA/USB 驱动
- `dism.exe`、DISM API/Provider 树（Capture/Apply 所需）
- `bcdboot.exe`
- `BackupRestore.exe`、`Recovery.exe`、MSVC runtime
- `winpeshl.ini`（由 `winpe-winpeshl.ini` 生成，直接启动 `Recovery.exe --pe-desktop`）

构建脚本随后以只读方式重新挂载成品 WIM，核验启动入口、DISM、BCDBoot、磁盘/NTFS/NVMe/USB 驱动文件均存在。

## RAMDISK 启动

`BackupRestorePE.wim` 不能单独证明“U 盘可拔”。构建脚本现在调用 ADK `copype arm64`，保留其 ARM64 UEFI 启动链：

- `EFI\BOOT\bootaa64.efi`
- `EFI\Microsoft\Boot\BCD`
- `boot\boot.sdi`
- `sources\boot.wim`（替换为本项目 WIM）

Windows Boot Manager 通过 BCD/`boot.sdi` 将 `sources\boot.wim` 作为 RAMDISK 加载。PE 完成加载后，运行时系统位于内存中的 `X:`，因此只要任务文件、备份镜像和日志位于其他持久化卷，就可以拔出启动 U 盘。

不能在 `Recovery.exe` 仍读取 U 盘上的 `task.json`、WIM 镜像或日志时拔盘；RAMDISK 只覆盖 PE 系统本身，不会复制外部数据卷。

## 任务文件边界

通用 `BackupRestorePE.wim` 不固化某次操作的 `task.json`。正常 Windows 的
`prepare` 会为每个任务创建独立目录和任务专用 WinRE：任务目录保留
`task.json`、`RecoveryTask.env`、`manifest.json` 与日志，并把经过哈希绑定的
副本注入任务专用 WIM；`Recovery.exe` 启动后再按卷 GUID 挂载任务工作卷并复核。
这样既不会让旧任务残留在通用启动盘里，也不会在恢复目标分区被覆盖后丢失任务。

若要实现“启动盘可拔”，任务工作目录、镜像和日志必须放在另一块持续连接的磁盘上；
它们不能只放在启动 U 盘。

## VM 构建命令

普通 Guest Tools 会话不能直接执行 DISM（错误 740）。提升操作只针对 `C:\BackupRestorePE`，不修改已注册的系统 WinRE：

```powershell
rustc --target aarch64-pc-windows-msvc -O Y:\poc\elevate-build.rs -o C:\BackupRestorePE\elevate-build.exe
Copy-Item Y:\poc\build-backuprestore-pe.ps1 C:\BackupRestorePE\build-backuprestore-pe.ps1 -Force
C:\BackupRestorePE\elevate-build.exe
```

如果提升令牌看不到 Parallels 的 `Y:` 共享盘，脚本会保留 C: 产物；随后用普通 Guest Tools 会话复制到仓库 `artifacts/`。

## 未宣称的范围

本次验证证明 WIM 可由 DISM 挂载、提交、导出并再次只读挂载，且关键文件齐全；同时生成了 ADK ARM64 RAMDISK ISO。2026-08-26 的 Parallels 实测中，VM CPU 明确为 ARM，UEFI 能识别 `UEFI Virtual DVD-ROM` 并显示“Press any key to boot from CD or DVD”。确认后返回的是 Parallels UEFI 固件主菜单（`Boot Manager` 的上级菜单），不是 PE；这表示启动链在 `boot.wim` 加载前失败。随后将固件切换为 Parallels 明确支持 Apple Silicon 的 `efi-arm64`，并关闭“允许选择启动设备”后重置，仍停在同一固件菜单；方向键、回车和空格均未改变菜单状态。由此本轮只能确认对照 ISO 尚未进入 PE，不能把失败归因于自定义 `BackupRestorePE.wim`；当前更像是 Parallels UEFI 菜单/输入状态或其启动链兼容性问题。测试后 VM 已恢复 `efi64`、Secure Boot 开启、硬盘优先。

因此尚未验证 `X:`、`Recovery.exe` 的独立 PE 启动，亦未执行真实 U 盘拔出测试；更不能宣称完整的 Windows → PE → 自动 `Recovery.exe` → 返回 Windows 重启流程成功。

## 2026-09-12 RAM 模式 GUI 自动安装验收（v1.4.7+test hook）

目标：通过真实 GUI 完成一次 RAM 模式 PE 安装（目标卷 Q:、目录 `Q:\BackupRestorePE`、
启动项名 `MyCustomPE`），并以 BCD 实机证据收口「启动项名称以文本框输入为准」。

### 结果（铁证）

- `Q:\BackupRestorePE\sources\boot.wim` 466,320,191 B（复制完成，stock 保留）。
- BCD 新条目 `{bd925061-ade7-11f1-8776-cbc94fdb67d3}`：
  - `description "MyCustomPE"`（启动项名 = 文本框实际输入）
  - `device/osdevice ramdisk=[Q:]\BackupRestorePE\sources\boot.wim,{ramdiskoptions}`
  - `ramdisksdipath \BackupRestorePE\boot\boot.sdi`、`winpe yes`、`detecthal yes`
  - `displayorder ... /addlast`（不修改 Windows 默认启动）
- GUI 日志：`GUI action completed: PE recovery installed (RAM disk wim -> Q:\...、BCD entry {bd925061-...})`；
  GUI 弹出「安装完成」对话框。

### 新增：测试钩子（test hook）

GUI 启动时若存在 `C:\br-test.json`，自动设置 PE 恢复参数并可选自动安装（跳过确认框）。
字段：

```json
{ "mode": "ram", "target_volume": "Q", "pe_dir": "Q:\\BackupRestorePE",
  "pe_name": "MyCustomPE", "pe_image": "Q:\\sources\\boot.wim", "auto_install": true }
```

- 触发点：`window_proc` 的 `WM_CREATE` 末尾（`test_hook_auto_install`），
  自动安装通过 `PostMessage(WM_APP_TEST_INSTALL)` 延迟到窗口显示后执行。
- 确认框跳过：`install_pe_ramdisk` / `install_pe_harddisk` 在
  `TEST_AUTO_CONFIRM` 置位时直接取 `IDYES`，日志记录 `auto-accepted`。
- 用法：写好 json → 启动 GUI（schtasks BRPE_GUI）→ 全自动安装 → 查
  `logs\gui.log` + `bcdedit /enum`。测试完必须删除 `C:\br-test.json`，否则下次启动重复安装。

### 目录盘符校验放宽（用户需求）

`install_pe_ramdisk` 不再强制「目录盘符必须等于目标卷」。现在仅校验路径形式
（`X:\目录`、目录非空、无 `*?<>|"`）与「目录盘符 ≠ 程序卷」。目录盘符可为任意
合法盘符；实际安装位置由目录路径决定（`{drive}:{dir}\sources\boot.wim`）。

### 本轮踩坑（务必留存）

1. **RDP（Windows App）会话中文输入法（微软拼音）拦截键盘输入**：
   `type_text` 打英文进候选、`Ctrl+V` 粘贴内容被吞、`Alt+Shift`/`Shift`/`Ctrl+Space`
   均无法从 macOS 端可靠切换（macOS 会拦截 ctrl/alt 组合）。**结论**：RDP 窗口内
   的文本输入不可靠，改用 test hook / 程序内 SendMessage 写参数。
2. **macOS 剪贴板 → RDP 剪贴板同步失败**：`pbcopy` 的内容在 RDP 里 `Ctrl+V`
   得不到（本机测试是输入法吞字，非同步问题；不要依赖该通道）。
3. **schtasks `/it` + `start powershell` 不启动**：任务结果 0 但 PowerShell
   子进程不出现（服务会话报「不支持请求的会话」，交互会话静默失败）。
   **GUI exe（start BackupRestore.exe）可以**，PowerShell 不行；不要在自动化
   里依赖 schtasks 跑 PS 脚本注入 GUI。
4. **cargo 产物名是 `backuprestore-cli.exe`**，不是 `BackupRestore.exe`：
   复制部署前先确认产物名（`dir target\...\release\*.exe`），否则 copy 了不存在的
   文件而 GUI 仍是旧版。
5. **复制 exe 前必须先 `taskkill /f /im BackupRestore.exe`**：文件被运行中进程
   锁定，`copy /y` 静默失败（`>nul` 吞错误），时间戳不变即失败信号。
6. **GUI 目标卷下拉坐标/RDP 窗口坐标随分辨率变化**：手工点击坐标不可复用；
   自动化一律用控件 ID + 程序内逻辑，不用坐标。

### PE 自动备份/还原验证记录（2026-09-12，配置驱动，零鼠标键盘）

- 流程：Win11 侧写 `S:\pe-task.txt`（ESP，mountvol S: /S）→ `bcdedit /set {bootmgr} bootsequence {pe-guid}` → 重启进 PE → PE 启动读配置自动执行 → 自动重启回 Win11。
- 实测任务：`clean_bootsequence → find-drive marker.txt → backup AUTO H:\pe-backup-test-v2.wim → verify-file → delete-file → verify-file → restore H:\pe-backup-test-v2.wim AUTO → verify-file → verify → reboot`。
- 结果（全部成功）：find-drive 定位测试盘（PE 盘符 G:）✓；backup dism 100% ✓；verify FOUND ✓；delete 后 verify MISSING ✓；restore dism 100% ✓；restore 后 verify FOUND ✓；Win11 侧 `dir P:\backup-test-file.txt` 34 字节确认恢复 ✓；clean_bootsequence 后 BCD 无 bootsequence（无死循环，正常回 Win11）✓。
- 关键：backup/restore 目标 WIM 路径不要写 ESP（S: 仅 278MB，8G 卷必报 Error 112 空间不足）；目标盘用数据盘。

### 本轮踩坑（2026-09-12 追加）

7. **PE 内盘符漂移（重要）**：PE 启动后盘符按发现顺序分配，与 Win11 侧**不固定对应**。
   本次实测：PE 的 `G:` = Win11 的 `P:`（测试盘，靠 find-drive marker 定位），
   PE 的 `H:` = Win11 的 `Q:`（PE 源盘！）——backup 写 `H:\pe-backup-test-v2.wim`
   实际落在 Win11 的 `Q:\`。**结论**：PE 任务里一律用 `find-drive`/`AUTO` 解析目标盘，
   WIM 目标路径也要用 `AUTO` 或确认盘符对应关系，别写死 Win11 盘符。
8. **Windows 系统「自定义缩放」≠ Windows App 缩放**：Win11 显示设置里若设了
   「自定义缩放比例」（如 200%），RDP/远程会话看起来全屏放大。修复：
   设置 → 系统 → 屏幕 → 「关闭自定义缩放并注销」，注销重登即恢复 100%。
   注册表 `HKCU\Control Panel\Desktop\LogPixels` 可能不存在（自定义缩放存别处），
   别只查注册表。
9. **PE 任务 result 文件**：`S:\pe-task-result.txt` 是**覆盖写**；`verify` 动作会把
   当前 result 文件内容回显进 `[RESULT_READBACK]`（执行中读到的是上一轮残留或
   "not yet written"）。**看每步结果以中间文件为准**：`S:\backup-out.txt`、
   `S:\restore-out.txt`、`S:\verify-file-out.txt`、`S:\delete-out.txt`。
10. **prlctl exec 偶发 `PrlJob_GetRetCode: Invalid argument`**：VM 注销/重启后立即
    exec 可能报此错，等 3-5 秒重试即可。

### PE 桌面按钮「自动点击」验证记录（2026-09-12，v1.4.8）

- **机制**：Win11 侧写 `S:\pe-click.txt`（首行 action + 空格参数）→ 设 bootsequence →
  重启进 PE → PE 桌面启动后自动 `PostMessageW(WM_COMMAND, 按钮ID)`（与真实鼠标点击
  走完全相同的分发路径）→ 所有确认框自动接受（等效持续点"是"）→ 执行完成后
  `exit_pe_to_windows`（恢复 BCD default + 清 bootsequence + 重启回 Win11）。
- **配置格式**：`backup <源盘|AUTO> <wim>` / `restore <wim> <目标盘|AUTO>`
  / `secondary <wim> <目标盘|AUTO> [菜单名]`。`AUTO` = find-drive 定位含 marker.txt
  的数据盘；WIM 路径支持 `AUTO:PE\xxx.wim` 前缀（PE 内自动定位 PE 源盘）。
- **实测（备份按钮）**：`backup AUTO AUTO:PE\pe-click-bk.wim` → PE 桌面自动点
  「备份系统」→ find-drive 定位 G:（测试盘）→ find_pe_source_drive 定位 H:（PE 源盘）
  → dism backup G: → H:\pe-click-bk.wim **100% completed** → 自动恢复 BCD 回 Win11 →
  WIM 落在 Win11 `Q:\pe-click-bk.wim`（PE H: ↔ Win11 Q: 映射确认）→ BCD default={default}
  + bootsequence 已清。全程零鼠标键盘。

### 本轮踩坑（2026-09-12 追加）

11. **自动点击的配置文件命名时机**：`S:\pe-click.txt` 在 PostMessage 前改名 `.done`
    （防重复触发），因此**按钮 handler 里读参数要读 `.done`**，不是 `.txt`。
12. **find_pe_source_drive 必须排除数据盘**：测试盘上也可能有 `BackupRestorePE`
    目录（RAM 模式部署过），不排除会把 WIM 写到源盘上（实测曾把 backup 目标解析成
    G: 自身）。排除 find-drive 定位的盘符后再枚举。
13. **PE 里 WIM 路径别写 Win11 盘符**：PE 盘符漂移（Win11 Q: 在 PE 里通常是 H:），
    直接写 `Q:\...` 会 Error 3（路径不存在）。用 `AUTO:PE\` 前缀自动定位。

### 本轮踩坑（2026-09-12 追加，exit 链 + 手动重建 PE 条目）

14. **mount S: 报 145 是预期行为，不是故障**：PE 桌面启动流程已把 ESP 挂到 S:，
    exit 时再次 SetVolumeMountPoint 报已占用(145)，代码 fallback 挂到 T: 继续。
    看到 "mount S: failed, last error: 145" + "mounted at T:" 是正常 fallback。
15. **cmd /c 带 pause 时 GetExitCodeProcess 误报 259**：开发模式 cmd 末尾有 pause，
    WaitForSingleObject(60s) 超时后 GetExitCodeProcess 返回 STILL_ACTIVE=259，
    被误判 bcdedit 失败。修复：先检查 WaitForSingleObject 返回值==0 才取退出码，
    超时则记 "cmd paused, waiting for key"。
16. **PE 内 Q: 不存在，exit 日志别写 Q:**：PE 盘符漂移，Q: 是 Win11 盘符；PE 内
    写 Q:\exit-pe.log 必报 err=3。修复：日志写到卷路径（\\?\Volume{GUID}\，最可靠）
    + S:\exit-pe.log + X:\exit-pe.log（X: RAM 盘重启丢）。
17. **read_pe_click_config 漏 action "exit" 会导致 exit 配置不被消费**：自动投递
    match 漏 ID_PE_EXIT，S:\pe-click.txt 内容 exit 不触发按钮，VM 停在 PE 桌面。
    修复：action match 补 "exit" + 自动投递补 Some(("exit", _)) => Some(ID_PE_EXIT)。
18. **手动重建 PE BCD 条目时 ramdisksdipath 必须指向 boot.sdi，不是 boot.wim**：
    bcdedit /set {ramdiskoptions} ramdisksdipath 的正确值是
    `\BackupRestorePE\boot\boot.sdi`（boot.sdi 是 WIM 启动的 RAM 盘基础）；
    误设成 `sources\boot.wim` 时 PE 启动失败（bootmgr 错误→静默回 Win11 或卡黑屏）。
    **同时要补 `nx OptIn`**（权威参数见 install_pe_ramdisk：ramdisksdidevice +
    ramdisksdipath(boot.sdi) + device/osdevice ramdisk=...,{ramdiskoptions} +
    winpe yes + detecthal yes + systemroot \windows + nx OptIn + description +
    displayorder /addlast）。缺 boot.sdi 或 nx 是本轮多次"重启后 25-50s 回 Win11
    （PE 根本没起来）"的真凶。
19. **恢复快照会同时回滚 BCD 与 boot.wim/exe**：快照回滚后 BCD 里 PE 条目消失
    （bootsequence 指向不存在条目被 bootmgr 静默忽略→直接回 Win11，表现为
    "没进 PE"），Q:\BackupRestorePE\sources\boot.wim 与 BackupRestore.exe 也回退
    到拍快照时的旧版。**恢复快照后必须**：① 重编部署 exe ② dism 更新 boot.wim
    ③ 重建 PE BCD 条目（GUID 每次会变，pe-entry-guid.txt 需同步）。
20. **bcdboot {目标盘} S: 会把目标盘 BCD 条目复制进 ESP**：restore 流程的
    bcdboot G: /s S: 会把测试盘历史测试条目（backuprestore-blank-compare 等）写进
    ESP BCD，污染菜单。测试后需 bcdedit /delete 清理；displayorder 也可能被改写
    （default 不受影响，exit 已恢复）。

### PE 桌面按钮自动点击全链路验证（2026-09-12，v1.4.8+，零鼠标键盘）

- 配置驱动：Win11 侧写 `S:\pe-click.txt`（action + 参数）→ 设 bootsequence →
  重启 → PE 桌面自动 PostMessage 按钮 → 执行 → exit_pe_to_windows（恢复 BCD
  default + 清 bootsequence + 重启）→ 自动回 Win11。全程无人工。
- **backup 实测通过**：`backup AUTO AUTO:PE\pe-click-bk.wim` → find-drive=G: →
  find_pe_source_drive=H: → dism backup G:→H:\pe-click-bk.wim **100%** →
  exit 自动回 Win11 → WIM 落 Win11 Q:\pe-click-bk.wim → bootsequence 清 + default 恢复。
- **restore 实测通过**：`restore AUTO:PE\pe-click-bk.wim AUTO` → format G: 100% →
  restore H:\pe-click-bk.wim→G: 成功 → bcdboot G: /s S: code=0 → exit 回 Win11 →
  Win11 P:\backup-test-file.txt / marker.txt 全部恢复。
- **secondary 实测通过**：`secondary AUTO:PE\pe-click-bk.wim AUTO TestSecond` →
  restore 成功 → add-secondary-entry 创建 BCD 条目 {402a8ed4}（device=partition=P:，
  description=TestSecond）→ displayorder code=0 → exit 回 Win11。（注：测试盘被
  restore 的 bcdboot 标记为 system 卷，secondary 的 format 无 --allow-system 被
  REFUSED——保护机制生效，产品场景第二系统盘无预置引导不受影响。）
- **exit 独立实测通过**：`exit` → .done 消费 → mount S: 145（预期）→ fallback T: →
  BCD 恢复 → bootsequence 清 → default={d2264b2f}(Win11) → 自动重启回 Win11 →
  S:\exit-pe.log 896B 正常落盘（此前 Q:\exit-pe.log err=3 问题已修复）。
- **关键修复确认**：exit 链四根因（145 预期 fallback / 259 误报 / 日志去 Q: /
  exit 识别）全部修复；"卡 PE"真正原因是手动重建 PE 条目 ramdisksdipath 指向
  boot.wim（应为 boot.sdi）+ 缺 nx OptIn，修正后 PE 正常启动。

### 主程序 PE 恢复 tab UI 修复与验证（2026-09-12，v1.5.0）

- 问题（用户截图取证 v1.4.8）：①「PE 目录名」输入框显示完整路径
  `C:\BackupRestorePE`（应为纯目录名，盘符由目标卷决定）；②「PE 启动项名称」
  默认带"内存启动"字样且模式感过强。
- 修复（native_gui.rs）：目录名输入框语义改为**目录名**（默认 BackupRestorePE，
  空则填默认）；兼容旧版完整路径（含 `\` 或 `:` 时取最后一段迁移）；英文标签
  "PE folder path"→"PE folder name"；install_pe_ramdisk 不再从输入框解析盘符
  （dir_name=目录名，drive_char=目标卷决定）；启动项名称空则按语言+当前模式填
  默认，若当前值恰为另一模式默认名（未自定义）则切模式时跟随更新。
- 实机验证（控件文本读取，Session 0 通道，零鼠标键盘）：
  - LBL=[PE 目录名]；DIR=[BackupRestorePE]（纯目录名 ✓）
  - BM_CLICK 切硬盘 → NAME=[Windows PE (BackupRestore) 硬盘启动] ✓
  - BM_CLICK 切回 RAM → NAME=[Windows PE (BackupRestore) 内存启动] ✓
- 版本：1.4.9 → 1.5.0（修好并验证通过后才升，满十进一）。

21. **Session 0 无交互桌面，GUI 验证不能截图**：prlctl exec 启动的 GUI 程序落在
    Session 0（Services，MainWindowHandle=0），CopyFromScreen 卡死/空白，
    PrintWindow 返回纯白图（1040x784，3291B）。**可行通道**：EnumWindows 按标题
    找主窗口 + GetDlgItem 按控件 ID 取子控件 + SendMessageW(WM_GETTEXT) 读
    EDIT 文本（GetWindowText 读不了跨进程 EDIT；P/Invoke 需 CharSet.Unicode，
    EntryPoint="SendMessageW" 避免方法名冲突）。
22. **schtasks /it 不工作（再次确认）**：/create 需显式 /ru 用户名 /rp 密码
    （否则 "No mapping between account names and security IDs"），/run 报
    "Element not found"（once 触发器过期），无法用计划任务在用户会话启动 GUI。
    用户会话验证改走：Session 0 启动 + 控件文本读取；单选/按钮用
    SendMessage(BM_CLICK=0x00F5) 模拟真实点击（WM_COMMAND 直发不切换状态，
    因为程序按 IsDlgButtonChecked 读状态）。
23. **macOS 交叉编译 aarch64-pc-windows-msvc 的完整环境（重要，勿再丢）**：
    - `rustup target add aarch64-pc-windows-msvc`（rust-std，~50-80MB；本轮发现
      该 target 曾丢失导致 `error[E0463]: can't find crate for core`）。
    - linker 用 rust 自带：`rust-lld`（在
      `~/.rustup/toolchains/stable-aarch64-apple-darwin/lib/rustlib/aarch64-apple-darwin/bin/rust-lld`）。
      不要用 `gcc-ld/lld-link`（包装不认 `-flavor link`）。设置
      `CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER=<rust-lld>`。
    - Windows SDK/VC import libs 从 VM 复制到 `~/win-sdk-arm64/`（走 Parallels
      共享，不耗流量）：`um/arm64`（C:\Program Files (x86)\Windows Kits\10\Lib\
      10.0.26100.0\um\arm64）、`ucrt/arm64`（同 SDK 的 ucrt）、`vc/arm64`
      （C:\BuildTools\VC\Tools\MSVC\14.44.35207\lib\arm64——含 msvcrt.lib /
      vcruntime.lib）。全部约 970MB。
    - 链接参数：`RUSTFLAGS="-C link-arg=/LIBPATH:/Users/x/win-sdk-arm64/um/arm64
      -C link-arg=/LIBPATH:/Users/x/win-sdk-arm64/ucrt/arm64
      -C link-arg=/LIBPATH:/Users/x/win-sdk-arm64/vc/arm64"`。缺 msvcrt.lib 报
      `could not open 'msvcrt.lib'`。
    - 一键构建见仓库根 `build-win.sh`。
24. **pe-entry-guid.txt 必须与 BCD 实际 PE 条目同步**：快照恢复或手动重建 PE 条目后
    GUID 会变，但 exe 目录的 pe-entry-guid.txt 不会自动更新；「重启进入 PE」按
    旧 GUID 设 bootsequence 会被 bootmgr 静默忽略（指向不存在条目）→ 重启后不进
    PE 直接回 Win11，表现为"按钮没生效"。排查：`type <exe目录>\pe-entry-guid.txt`
    对比 `bcdedit /enum | findstr "BackupRestore PE"` 的 identifier，不一致就同步。

### 「重启进入 PE」自动重启（2026-09-12，v1.5.1）

- 需求：主程序「重启进入 PE」原本配置完 bootsequence 后只弹提示、需用户手动重启；
  改为**配置成功后立即自动重启**，全程无需用户操作。
- 实现（native_gui.rs `pe_reboot_to_pe`）：bootsequence 设置成功（code=0 且
  pe-task.txt 写入成功）后直接 `ExitWindowsEx(EWX_REBOOT, 0)`；失败（如 Session 0
  缺关机权限）回退 `shutdown.exe /r /t 0 /f`；两条路都失败才弹"自动重启失败，
  请手动重启"错误框。
- 实机验证（零鼠标键盘，控件触发 ID_PE_REBOOT_MAIN=1410）：
  - 日志：mountvol S: code=0 → write pe-task.txt ok=true → bcdedit bootsequence
    {39435381-...} code=0 → "auto reboot now" → ExitWindowsEx failed（Session 0
    特例）→ shutdown.exe fallback code=0。
  - 结果：VM 自动重启 → bootmgr 消费 bootsequence 进 PE → PE 桌面自动执行
    pe-task.txt（S:\pe-task.txt.done 生成）→ clean_bootsequence mountvol=1 clean=1
    （S:\pe-task-result.txt）→ 自动重启回 Win11 → bootsequence 已清空、default 正常。
  - 结论：真实用户交互会话 ExitWindowsEx 直接成功；Session 0 测试环境走 shutdown
    兜底同样完整走通。

### 「PE 恢复」tab 右下角控件遮挡修复（2026-09-12，v1.5.2）

- 现象：v1.5.1「PE 恢复」tab 右下角「创建快捷方式」按钮区域出现残字"dc"；
  点击文本框后「创建快捷方式」按钮上叠出"Windows 备份"文字，按钮被文本框盖住。
- 根因（两处，均与 layout 有关）：
  1. **menu 控件（第二系统名称输入框，ID_MENU=1207，默认文本"Windows 备份"）在
     「PE 恢复」tab 被错误显示**：`set_operation_visibility` 中
     `show_menu = operation == "create-secondary" || operation == "install-pe-entry"`
     把 PE tab 也算进去了。而 `layout_operation` 无条件把 menu reposition 到
     `(field_x+500, secondary_y)`（=680,628），恰好覆盖「创建快捷方式」按钮
     （创建坐标 710,630）。z 序上 menu 创建于 3991、按钮创建于 4123（更后），
     按钮在上、menu 被盖住只露出文字残迹"dc"；点击（EDIT 获得焦点/重绘）后
     menu 文本"Windows 备份"叠到按钮上。
     → 修复：`show_menu = operation == "create-secondary"`（PE tab 隐藏 menu 及
       其标签 2008）。
  2. **「重启进入 PE」（ID_PE_REBOOT_MAIN=1410）与「创建快捷方式」（ID_PE_SHORTCUT
     =1411）创建时用固定坐标（560/710, 630），layout_operation 的按钮循环只排
     4 个通用按钮（1001-1004），PE 两个按钮不随布局 reposition**，一旦窗口/
     布局变化会错位。
     → 修复：`layout_operation` 末尾追加 PE 两个按钮的 reposition 到
     `(560/710, buttons_y, 140, 28)`，与通用按钮行同 y 对齐。
  3. 顺带发现：新增第二系统 tab 中「第二系统名称」标签（2008，x=570 起）与 WIM
     索引下拉框（index，create-secondary 时宽 440 → x 180-620）重叠 50px（标签
     文字压在下拉框右缘）。→ 修复：create-secondary 时 index 宽度 440→380
     （x 180-560），与标签不再重叠。
- 验证（Session 0 控件通道，脚本 tools/check-overlap.ps1、dump-controls.ps1、
  switch-and-check.ps1）：
  - PE tab：menu_edit(1207)/menu_label(2008)/index_list(1206) 全部 WS_VISIBLE=False；
    按钮行 6 按钮（20/150/280/410/560/710，宽 120/120/120/140/140/140）同 y、等距
    10px、无重叠；status/镜像行/PE 三行层次正确。
  - 全 tab（备份/单系统还原/新增第二系统/PE 恢复）SendMessage 切 tab 验证：
    WS_VISIBLE 控件两两重叠面积均 ≤200，无显著遮挡。
  - 踩坑补充：**验证时不要反复 taskkill/重启 GUI 进程**——程序窗口在 VM 可见
    桌面（Parallels 控制台即 Session 0），循环重启会让用户看到程序闪烁、且窗口
    初始化瞬时状态可能截到"旧 tab 提示文本 + 新布局"的混合画面，易被误认为新
    bug；改用 SendMessage WM_COMMAND 切 tab（switch-and-check.ps1）不打扰用户。

## v1.5.3：修复「多切几次 tab 后控件叠加/重绘残留」GUI bug

- **现象**：用户实测（截图 23:11/23:17）PE 恢复 tab 左下角「镜像绝对路径」标签行
  叠出上一 tab（新增第二系统）的 status 提示文本「新增第二系统: 保留当前 Windows…」；
  多切几次 tab 必现。v1.5.2 的三处静态布局修复（show_menu/menu 位置/index 宽度）
  未解决此问题。
- **根因（实锤）**：`set_text` 只调 `SetWindowTextW`——WM_SETTEXT 只把控件区域
  标为异步失效（InvalidateRect 排队），**不保证立即重绘**。快速连续切换 tab 时
  layout_operation 对十余个控件连续 MoveWindow/SetWindowText，WM_PAINT 被合并/
  延迟，STATIC/EDIT 控件区域出现「文本层（GetWindowText 读新文本）与像素层
  （屏幕残留旧文本）不一致」——文本层检测全部干净（2004len=6），但屏幕上旧
  status 文本残留在新 tab 控件区域。
- **修复（双保险）**：
  1. `set_text` 末尾追加 `InvalidateRect(hwnd, NULL, 1) + UpdateWindow(hwnd)`
     强制控件立即重绘（新增 user32 extern 声明 InvalidateRect/UpdateWindow）。
  2. `select_operation` 末尾与 PE 启动方式（RAM/硬盘）切换处理末尾追加
     `InvalidateRect(root, NULL, 1) + UpdateWindow(root)` 整窗同步重绘，
     布局/文案就绪后彻底擦除残留像素。
- **验证（Session 1 真实窗口，v1.5.3）**：WM_COMMAND 快速切换 secondary↔PE
  30 轮（间隔 20ms，比用户操作更极端），结束后 PE tab：
  - 2004（镜像绝对路径）len=6、status len=34，文本层正常；
  - prlctl capture 视觉确认：RAM disk/硬盘启动/PE 目录名/启动项名称/status/
    镜像绝对路径/六按钮全部正常，镜像标签行无任何叠加文本。
- **验证通道踩坑（重要，后续勿再踩）**：
  1. **Session 0（prlctl exec，SYSTEM）看不到 Session 1 GUI 窗口**；用
     `schtasks /create /it /ru x /rp 1 /rl HIGHEST` + `/run` 在 Session 1 交互
     桌面运行验证脚本（find-secondary/stress-session1/fast-switch 等）。
  2. **PowerShell 委托回调里 `$script:xxx` 赋值不可靠**（EnumWindows 回调在
     .NET 线程执行，作用域隔离），必须用 `$global:gxxx` 或 ArrayList 收集；
     否则会假报 WINDOW NOT FOUND。
  3. **Add-Type 里用 System.Drawing.Imaging 必须加 `-ReferencedAssemblies
     System.Drawing.dll`**，否则 Add-Type 编译失败 → EnumWindows 类不存在 →
     假报 WINDOW NOT FOUND（frames-session1.ps1 早期即此原因）。
  4. **WM_COMMAND 发 tab 切换**：wParam 低 16 位是控件 ID（1100-1104），高 16
     位是通知码 0；写成 `(id << 16) | 0` 会把控件 ID 变成 0，切换无效（窗口
     停在原 tab，验证会得出假结论）。
  5. **schtasks 任务创建时机绑定会话/桌面**：任务在用户会话未就绪时创建可能
     绑定错误桌面，找不到窗口；窗口确认在屏后仍失败时，重建任务或换名重试。
  6. Get-Process 的 MainWindowHandle 在跨会话视角可能为 0，不能作为窗口存在
     依据；以 EnumWindows（同桌面）为准。

## 硬盘版 PE（分区启动）实机验证坑（v1.5.8，2026-09-13）

### 1. 标准 PE 可以分区启动，WinRE.wim 不行
- 程序自带 PE（BackupRestorePE.iso 的 sources\boot.wim，367MB）**可以**：DISM
  /Apply-Image 到分区（如 F:）+ bcdboot 分区\Windows /s S: /f UEFI + winpe=Yes，
  Boot Manager 手动选 PE 即可进入 PE 会话。
- **WinRE.wim 不能**用「apply 到分区 + bcdboot partition=」方式启动（报错/回退）；
  ramdisk 方式（WIM+boot.sdi）配置正确也可能直接崩 VM。用户明确：**一律用程序
  自带 PE 镜像，不是 WinRE 镜像**。
- bcdboot 输出 "Setting {default} to {new}" 是 BFSVC 内部别名，实际创建的 loader
  条目是另一个 GUID（以 bcdedit /enum all /v 为准）；/addlast 在 /s 指定卷时被
  忽略，必须手工 bcdedit 恢复 default={current} 与 displayorder（Win11 第一）。

### 2. 标准 PE 无中文字体 → PE 桌面中文乱码
- 标准 WinPE 不含 simsun 等中文字体，Recovery.exe 的 PE 恢复桌面按钮/标题全部
  显示 □□□。
- **修复**：把 Win11 的 C:\Windows\Fonts\simsun.ttc 复制到 PE 分区
  Windows\Fonts\simsun.ttc，PE 桌面中文立即正常（GDI 按字体名找到文件即可用）。

### 3. PE 内的 Recovery.exe 必须是最新版
- ISO 打包时注入 PE WIM 的 Recovery.exe 是构建当时的版本；若之后新增了
  `--pe-desktop` 等分支，旧版收到该参数会输出 usage 并 exit 2 → PE 桌面不出现、
  cmd 一闪 → PE 重启回引导菜单。
- **修复**：部署时同步 `copy 最新 Recovery.exe → PE 分区\Windows\System32\`。

### 4. PE 桌面对话框高度必须预留标题栏
- window_proc_pe_dialog 控件用客户区坐标（按钮 y=236+32），但 CreateWindowExW
  的 height 是**含 WS_CAPTION 标题栏的总高**（约 28-30px）——height=288 时按钮
  底部超出客户区被裁剪，界面显示不完整。
- **修复**：height 288→312（有菜单名 300→324），按钮完整可见（v1.5.8）。

### 5. pe-exit-guid.txt 必须写 Win11 真实 GUID
- ESP 根目录 S:\pe-exit-guid.txt 是「返回 Windows」按钮（exit_pe_to_windows）
  读取的 Win11 条目 GUID（PE 内 {current} 解析成 PE 条目，不能用）。
- 若写入错误/不存在的 GUID，bcdedit /set {bootmgr} default {错误GUID} 会把
  default 设为无效条目（bootmgr 重启回退到 displayorder 第一个，但 BCD 状态脏）。
- **修复**：确保部署时写入 `bcdedit /enum {current} /v` 的真实 GUID（本次修正为
  {a8bafbae-af1a-11f1-a77c-9813bbfbbd66}）。

### 6. bootsequence 在此 Parallels VM 不可靠
- 同一条 bootsequence {PE} 有时被 bootmgr 消费（自动进 PE），有时不消费
  （直接进 Win11，bootsequence 残留）。**验证进 PE 以 Boot Manager 菜单手动选
  PE 为准**；bootsequence 只作"有机会自动进"的尽力而为。

### 7. PE（X: RAM 盘）无法 prlctl exec
- Parallels Tools 不在 PE 里，PE 会话中 prlctl exec 报 "Unable to open new
  session"。PE 内自动化靠：
  - ESP 上写 S:\pe-click.txt（内容 backup / restore / secondary / exit），
    PE 桌面启动时自动投递对应按钮点击（用完改名为 .done）；
  - 需要回 Win11 时 prlctl stop --kill + start（强制）。

## 完整 GUI 进 PE + 智能分流（v1.5.8，2026-09-13）

### 1. 完整多 tab GUI 可以在 WinPE 里直接运行
- `Recovery.exe --tab 1` 在 PE 里能正常启动完整主 GUI（多 tab、语言下拉、磁盘枚举
  全可用），已验证截图。因此「打开完整程序」按钮成立：PE 桌面保留 6 按钮（把
  「重启」合并进「返回 Windows」后，空位改为「打开完整程序」）。
- **坑**：`main.rs should_launch_gui()` 检查 exe 文件名 == "BackupRestore"，
  PE 里文件名是 Recovery.exe，**无参数启动不会进 GUI**（走 CLI 分支）。必须带
  `--tab N` 或 `--open-image` 参数直接进 GUI。

### 2. 智能分流：判断「当前活动系统」用 %SystemDrive%
- 备份/还原 tab 点「创建任务」时：目标卷 == %SystemDrive%（当前正在运行的
  系统）→ 弹窗 3 按钮（进入 PE / 进入 Windows RE / 取消）。
- **不是**用「是否含 Windows 的卷」判断——双系统时另一个 Windows 卷并未运行，
  可以直接在线备份/还原（DISM 捕获离线卷没问题）。
- 弹窗 3 按钮用**自绘模态对话框**（不依赖 comctl32 v6 TaskDialog——项目无
  manifest，TaskDialogIndirect 可能不可用）。

### 3. 在线直接执行（数据盘）
- 非当前活动系统卷 → 后台线程跑 `dism /Capture-Image`（备份）或
  `/Apply-Image`（还原），完成 PostMessage WM_APP_ONLINE_DONE，主窗口弹结果。
- **坑**：std::thread::spawn 闭包里不能直接 move Hwnd（裸指针 *mut c_void 不
  Send）——先 `let root = state.root as usize;`，线程里再 `as Hwnd`。

### 4. pe-task / pe-click 通道
- `S:\pe-click.txt` 新增动作 `main`（自动点「打开完整程序」验收通道）。
- `S:\pe-task.txt` 按空白分词：**WIM 路径含空格会被拆坏**，写任务前必须拦截提示
  （智能分流 schedule_pe_task 已做）。
- ESP 程序正常文件：pe-drive.txt、pe-bootsequence-clean.log、pe-exit-guid.txt
  （部署 GUID）、pe-entry-guid.txt（PE 条目 GUID，Windows 侧部署目录）。
