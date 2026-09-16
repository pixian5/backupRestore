# BackupRestore 当前执行基线（v1.6.1）

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
- 正常任务 WinRE 入口固定为
  `%SYSTEMROOT%\System32\Recovery.exe,recover-env %SYSTEMROOT%\System32\RecoveryTask.env`。
  该内容内嵌在 Rust 二进制中，创建任务时直接生成 `payload\winpeshl.ini`，不依赖
  程序目录中的外部模板。自定义 PE 桌面仍使用独立的 `windows/winpe-winpeshl.ini`。

## v1.6.1 部署修复

- `winre_payload.rs` 是唯一的 WinRE 静态载荷映射和 shell 契约来源。任务暂存和 WIM 注入使用同一映射，
  每个复制文件均有 SHA-256 核对，提交前后都验证完整载荷。
- 任务 WIM 会删除遗留的 `BackupRestore.exe`、`RecoveryLauncher.cmd` 和
  `winpeshl-boot.cmd`，因而不能再以旧入口启动。
- 已删除自动重定位与 `--relocated` 参数，防止同卷还原偷偷产生副本或继续执行。
- GUI 的“创建桌面快捷方式”由 Rust 通过 Shell 已知文件夹 API 写入 `.url` 启动快捷方式，
  不生成或调用 PowerShell 脚本。
- Windows ARM64 部署脚本使用登录用户会话中的 Parallels `X:` 共享目录复制文件；只有
  Rust 二进制与 PE 模板复制成功，并且客体两个 EXE 的 SHA-256 与宿主构建一致时才输出
  `DEPLOYED`。WinRE shell 不再作为外部文件部署。
- 部署前会删除客体包中旧的 `RecoveryLauncher.cmd`、共享 `winpeshl.ini` 和
  `winpeshl-boot.cmd`，避免旧入口残留。

## 2026-09-16 验证结果

- `cargo fmt --all`、`cargo test --workspace --all-targets --offline`（32 项）和
  `bash scripts/audit-runtime-boundaries.sh` 通过。
- macOS 与 Windows ARM64 的严格 Clippy（`-D warnings`）通过；Windows ARM64 单元测试
  二进制已用 `rust-lld` 与 VM 提取的 SDK import library 完成 `--no-run` 编译。
- `./build-win.sh --deploy` 成功生成并部署 `BackupRestore.exe`（1,518,080 bytes），宿主、
  客体 `BackupRestore.exe` 与 `Recovery.exe` 的 SHA-256 均为
  `b44ba334c03418b826ac2cd7e66c111b8686615714498d04d23a2b522200d578`。
- 静态审计确认 GUI 在提权前、CLI 在 UAC 前阻止工作目录卷覆盖，且产品运行时源码没有
  PowerShell、`TaskDrive` 或 `C:\ProgramData\BackupRestore` 依赖。

## 2026-09-17 真实 WinRE probe

- 任务 `74d805aa-a192-474d-8c86-72b60a26bed3` 已完成
  `Windows -> reagentc /boottore -> WinRE -> Recovery.exe recover-env -> wpeutil reboot -> Windows`。
- `Recovery.log` 记录 Rust Recovery 从任务 env 启动、probe 不执行磁盘操作、原始注册
  WinRE 恢复并校验成功、清理完成；最终 `status.json` 为 `success / 100%`。
- 这证明当前 Rust 自包含入口、任务工作目录重新定位、WinRE 原镜像恢复和返回 Windows
  链路可用；不代表备份 Capture、还原 Apply、格式化或 BCDBoot 已由本次 probe 验证。

## 未验证边界

- 本次已排除“Parallels ARM 无法启动注册 WinRE RAMDISK”的旧结论。仍未由本次 probe
  覆盖的是实际 Capture/Apply、格式化、BCDBoot、第二系统启动和故障注入矩阵。
