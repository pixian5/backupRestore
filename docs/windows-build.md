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

输出位于 `artifacts\windows\BackupRestore-windows-<arch>-v<VERSION>`，每个包
包含同一架构的 `BackupRestore.exe`（启动 GUI）和 `Recovery.exe`（WinRE 恢复），
以及 PowerShell/WinRE 载荷和 `build-manifest.json`。x64 与 ARM64 不可混用。

宿主机当前只安装了 `aarch64-apple-darwin`，没有 Windows MSVC target；Parallels
Win11 ARM64 目前也未安装 Rust/Cargo。因此本轮只能完成离线源码、脚本和配置验证，
实际 ARM64 exe 编译要在 VM 内安装 `aarch64-pc-windows-msvc` 后执行。
