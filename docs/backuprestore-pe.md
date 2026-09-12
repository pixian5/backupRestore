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
- `winpeshl.ini`（直接启动 `Recovery.exe recover-env`）

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
