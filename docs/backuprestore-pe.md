# BackupRestorePE.wim 构建记录

## 产物

已在 Windows 11 ARM64 VM 中使用已安装的 Windows ADK ARM64 WinPE 基础镜像生成：

- VM 路径：`C:\BackupRestorePE\BackupRestorePE.wim`
- 仓库副本：`artifacts/BackupRestorePE.wim`
- RAMDISK 启动介质目录：`artifacts/BackupRestorePE-media/`
- ARM64 可启动 ISO：`artifacts/BackupRestorePE.iso`
- 版本：`0.8.5`
- SHA-256：`84bca6cd79b1a67887ae8b4b7c4a760607db931e456772e481ba382e2d31c030`
- WIM 大小：约 350 MiB（LZX）
- 架构：ARM64

## 内容边界

产物以 ADK `arm64\en-us\winpe.wim` 为基础，并写入：

- Windows RE/WinPE 基础系统和基本图形支持
- ADK 基础镜像自带的 NTFS、FAT32、存储、NVMe/SATA/USB 驱动
- `dism.exe`、DISM API/Provider 树（Capture/Apply 所需）
- `bcdboot.exe`
- `BackupRestore.exe`、`Recovery.exe`、MSVC runtime
- `RecoveryLauncher.cmd`、`BackupRestore.cmd`、`winpeshl.ini`

构建脚本随后以只读方式重新挂载成品 WIM，核验启动入口、DISM、BCDBoot、磁盘/NTFS/NVMe/USB 驱动文件均存在。

## RAMDISK 启动

`BackupRestorePE.wim` 不能单独证明“U 盘可拔”。构建脚本现在调用 ADK `copype arm64`，保留其 ARM64 UEFI 启动链：

- `EFI\BOOT\bootaa64.efi`
- `EFI\Microsoft\Boot\bootmgfw.efi`
- `boot\BCD`
- `boot\boot.sdi`
- `sources\boot.wim`（替换为本项目 WIM）

Windows Boot Manager 通过 BCD/`boot.sdi` 将 `sources\boot.wim` 作为 RAMDISK 加载。PE 完成加载后，运行时系统位于内存中的 `X:`，因此只要任务文件、备份镜像和日志位于其他持久化卷，就可以拔出启动 U 盘。

不能在 `Recovery.exe` 仍读取 U 盘上的 `task.json`、WIM 镜像或日志时拔盘；RAMDISK 只覆盖 PE 系统本身，不会复制外部数据卷。

## VM 构建命令

普通 Guest Tools 会话不能直接执行 DISM（错误 740）。提升操作只针对 `C:\BackupRestorePE`，不修改已注册的系统 WinRE：

```powershell
rustc --target aarch64-pc-windows-msvc -O Y:\poc\elevate-build.rs -o C:\BackupRestorePE\elevate-build.exe
Copy-Item Y:\poc\build-backuprestore-pe.ps1 C:\BackupRestorePE\build-backuprestore-pe.ps1 -Force
C:\BackupRestorePE\elevate-build.exe
```

如果提升令牌看不到 Parallels 的 `Y:` 共享盘，脚本会保留 C: 产物；随后用普通 Guest Tools 会话复制到仓库 `artifacts/`。

## 未宣称的范围

本次验证证明 WIM 可由 DISM 挂载、提交、导出并再次只读挂载，且关键文件齐全；同时验证了 ADK ARM64 UEFI RAMDISK 启动链并生成 ISO。尚未在真实 U 盘上执行“PE 完成加载后拔盘”测试，也尚未在该独立介质上执行一次完整的 Windows → PE → 自动 `Recovery.exe` → 返回 Windows 重启流程。
