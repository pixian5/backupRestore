# WinRE 载荷契约修复（v1.6.0）

## 问题

正常 WinRE 任务和自定义 PE 恢复桌面曾共用 `windows/winpeshl.ini`。PE 桌面改动会
意外改变 WinRE 的入口，而任务暂存与 WIM 注入又各自维护文件列表，导致入口与实际
载荷脱节：WIM 可以由 DISM 正常提交，但启动后找不到应该运行的程序。

首次已提交的回归是 `v1.3.3`（`3d38c17`）：入口改为 `BackupRestore.exe --pe-desktop`，
但 WinRE 只注入 `Recovery.exe`。`v1.5.12` 的诊断入口 `winpeshl-boot.cmd` 再次重现了
同类问题。

## 修复

- 正常任务的 WinRE shell 契约内嵌在 Rust `Recovery.exe` 中，准备任务时直接生成
  `payload\winpeshl.ini`，只能启动 `Recovery.exe recover-env`；程序目录不需要外部
  `winre-winpeshl.ini`。
- `windows/winpe-winpeshl.ini` 是 PE 恢复桌面专用模板，只能启动 Rust
  `Recovery.exe --pe-desktop`。
- `winre_payload.rs` 是 WinRE 文件映射和 shell 内容的唯一运行时来源：先生成到任务目录，
  再复制到挂载 WIM 的 `Windows\System32`，每次复制都做 SHA-256 核对。
- 注入后再次检查模板和全部必需文件；旧的 `BackupRestore.exe`、
  `RecoveryLauncher.cmd`、`winpeshl-boot.cmd` 会从任务专用 WIM 删除。
- 任何必需文件缺失、生成入口不符合 Rust 契约或文件哈希不一致都会使准备失败，DISM
  不会提交该 WIM。
- `windows/build-windows.ps1` 和 `poc/build-backuprestore-pe.ps1` 分别校验并打包正确
  模板；PE 构建后重新挂载 WIM 再检查入口。

## 验证边界

Rust 单元测试覆盖外部 shell 文件不影响内嵌契约、静态任务载荷、WIM 目录注入、
注入后哈希比对和不完整任务载荷拒绝。2026-09-17 又在 Windows 11 ARM64 VM 完成真实
probe：WinRE 自动运行 Rust Recovery、恢复原 WinRE 并返回 Windows，任务最终成功。
