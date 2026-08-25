# BackupRestorePE.wim 构建记录

## 产物

已在 Windows 11 ARM64 VM 中使用已安装的 Windows ADK ARM64 WinPE 基础镜像生成：

- VM 路径：`C:\BackupRestorePE\BackupRestorePE.wim`
- 仓库副本：`artifacts/BackupRestorePE.wim`
- ARM64 可启动 ISO：`artifacts/BackupRestorePE.iso`
- 项目版本：`0.8.7`
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
- `RecoveryLauncher.cmd`、`BackupRestore.cmd`、`winpeshl.ini`

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

本次验证证明 WIM 可由 DISM 挂载、提交、导出并再次只读挂载，且关键文件齐全；同时验证了 ADK ARM64 UEFI RAMDISK 启动链并生成 ISO。2026-08-26 的 Parallels 实测中，UEFI 能识别 `UEFI Virtual DVD-ROM`、显示“Press any key to boot from CD or DVD”，但确认后回到固件菜单，未进入 PE。该 VM 的正常固件为 `efi64`，而 ISO 只携带 ARM64 `bootaa64.efi`；即使临时切至 `efi-arm64` 且关闭 Secure Boot，结果仍相同。测试后 VM 已恢复 `efi64`、Secure Boot 开启、硬盘优先。

因此尚未验证 `X:`、`Recovery.exe` 的独立 PE 启动，亦未执行真实 U 盘拔出测试；更不能宣称完整的 Windows → PE → 自动 `Recovery.exe` → 返回 Windows 重启流程成功。
