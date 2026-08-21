# Windows ARM64 主线构建

Windows 二进制不能做成同一个“通用 exe”。本项目采用两个独立产物，当前先开发
ARM64；x64 只保留构建配置，暂不作为默认目标：

| 产物 | Rust target | 运行环境 |
|---|---|---|
| x64 | `x86_64-pc-windows-msvc` | Windows 10/11 x64 |
| ARM64 | `aarch64-pc-windows-msvc` | Windows 10/11 ARM64 |

在 Win11 ARM64 开发机上默认运行（只构建 ARM64）：

```powershell
rustup target add aarch64-pc-windows-msvc
.\windows\build-windows.ps1
```

脚本不会自动下载工具链。缺少目标时会直接停止，避免在用户网络环境中
偷偷产生大流量。需要显式构建 x64 时再运行：

```powershell
.\windows\build-windows.ps1 -Architecture arm64
.\windows\build-windows.ps1 -Architecture x64
```

Parallels 共享目录不适合 Rust 的临时归档操作时，可把 Cargo 输出放到
Windows 虚拟磁盘：

```powershell
.\windows\build-windows.ps1 -Architecture arm64 -CargoTargetDir C:\BackupRestoreBuild\target
```

输出位于 `artifacts\windows\BackupRestore-windows-<arch>-v<VERSION>`，每个包
包含同一架构的 `BackupRestore.exe`（启动 GUI）和 `Recovery.exe`（WinRE 恢复），
以及 PowerShell/WinRE 载荷、所需 MSVC runtime 和 `build-manifest.json`。x64 与 ARM64 不可混用。

ARM64 包中的 `BackupRestore.ps1` 会读取同目录的 `build-manifest.json`，
将当前 Windows 原生架构与包架构比对；在 ARM64 Windows 上运行 x64 包或反之会在
任何磁盘操作前停止。RecoveryTask.env 和 metadata.json 也会记录实际 ARM64 架构、
Windows 版本/构建号和容量安全字段。

宿主机仍只安装了 `aarch64-apple-darwin`，不承担 Windows 交叉链接。当前 VM 已确认
Rust、`aarch64-pc-windows-msvc` 和 Visual Studio C++ 工具可用。源码可以通过
Parallels 共享桌面传入 VM；构建时仍应把 Cargo target 目录放在 VM 本地磁盘，避免
共享目录的临时文件语义差异。
