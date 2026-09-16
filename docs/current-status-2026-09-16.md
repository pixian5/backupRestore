# BackupRestore 当前执行基线（v1.6.0）

更新时间：2026-09-16。此文优先于按日期保存的历史进度和 PoC 记录。

## 当前产品边界

- 正常 Windows GUI、任务准备和 WinRE 恢复入口均为 Rust；PowerShell 只用于 ADK
  构建和开发测试，绝不作为产品备份、还原或 WinRE 启动链。
- 程序所在目录就是工作目录。任务、载荷、状态、日志和 BCD 快照都保存在
  `<程序目录>\tasks` 或 `<程序目录>\logs`，不依赖 `C:\ProgramData\BackupRestore`，
  没有用户可见的“任务卷”或 `TaskDrive`。
- 备份允许程序目录与源分区相同。单系统还原和新增第二系统若程序目录所在分区等于
  还原目标，则在任何卷身份查询、UAC、任务创建、WinRE/BCD 修改或重启请求之前停止。
  程序不会自动复制、自动选盘或后台迁移；用户必须手动移动整个程序目录后重试。
- 正常任务 WinRE 模板固定为
  `%SYSTEMROOT%\System32\Recovery.exe,recover-env %SYSTEMROOT%\System32\RecoveryTask.env`。
  自定义 PE 桌面模板固定为 `%SYSTEMROOT%\System32\Recovery.exe,--pe-desktop`。两者
  分别为 `windows/winre-winpeshl.ini` 和 `windows/winpe-winpeshl.ini`，不共享模板。

## v1.6.0 修复

- `winre_payload.rs` 是唯一的 WinRE 静态载荷映射。任务暂存和 WIM 注入使用同一映射，
  每个复制文件均有 SHA-256 核对，提交前后都验证完整载荷。
- 任务 WIM 会删除遗留的 `BackupRestore.exe`、`RecoveryLauncher.cmd` 和
  `winpeshl-boot.cmd`，因而不能再以旧入口启动。
- 已删除自动重定位与 `--relocated` 参数，防止同卷还原偷偷产生副本或继续执行。
- GUI 的“创建桌面快捷方式”由 Rust 通过 Shell 已知文件夹 API 写入 `.url` 启动快捷方式，
  不生成或调用 PowerShell 脚本。
- Windows ARM64 部署脚本只有在全部 Rust 二进制和两个模板复制成功后才输出
  `DEPLOYED`。

## 2026-09-16 验证结果

- `cargo fmt --all`、`cargo test --workspace --all-targets --offline`（32 项）和
  `bash scripts/audit-runtime-boundaries.sh` 通过。
- macOS 与 Windows ARM64 的严格 Clippy（`-D warnings`）通过；Windows ARM64 单元测试
  二进制已用 `rust-lld` 与 VM 提取的 SDK import library 完成 `--no-run` 编译。
- `./build-win.sh` 成功生成 `BackupRestore.exe`（1,517,056 bytes）；未部署到 VM，未请求
  重启。`windows/build-windows.ps1` 与 `poc/build-backuprestore-pe.ps1` 的 PowerShell AST
  解析通过。
- 静态审计确认 GUI 在提权前、CLI 在 UAC 前阻止工作目录卷覆盖，且产品运行时源码没有
  PowerShell、`TaskDrive` 或 `C:\ProgramData\BackupRestore` 依赖。

## 未验证边界

- 当前 Parallels ARM VM 的 NTFS RAMDISK WinRE 引导失败是环境/固件启动栈限制，不能把
  构建或静态检查描述为真实 WinRE 验收。真实 Windows 设备仍需复核完整自动启动链。
