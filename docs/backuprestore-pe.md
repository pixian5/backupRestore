# BackupRestorePE.wim 构建记录

## 产物

已在 Windows 11 ARM64 VM 中使用已安装的 Windows ADK ARM64 WinPE 基础镜像生成：

- VM 路径：`C:\BackupRestorePE\BackupRestorePE.wim`
- 仓库副本：`artifacts/BackupRestorePE.wim`
- 版本：`0.8.3`
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

## VM 构建命令

普通 Guest Tools 会话不能直接执行 DISM（错误 740）。提升操作只针对 `C:\BackupRestorePE`，不修改已注册的系统 WinRE：

```powershell
rustc --target aarch64-pc-windows-msvc -O Y:\poc\elevate-build.rs -o C:\BackupRestorePE\elevate-build.exe
Copy-Item Y:\poc\build-backuprestore-pe.ps1 C:\BackupRestorePE\build-backuprestore-pe.ps1 -Force
C:\BackupRestorePE\elevate-build.exe
```

如果提升令牌看不到 Parallels 的 `Y:` 共享盘，脚本会保留 C: 产物；随后用普通 Guest Tools 会话复制到仓库 `artifacts/`。

## 未宣称的范围

本次验证证明 WIM 可由 DISM 挂载、提交、导出并再次只读挂载，且关键文件齐全；尚未在该独立 WIM 上执行一次真实的 Windows → WinRE → 自动 `Recovery.exe` → 返回 Windows 重启流程。该流程仍需在独立快照中验收。
