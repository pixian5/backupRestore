# WinRE 自动启动 PoC

目标链路：

```text
Windows -> 任务专用 WinRE -> startnet.cmd -> RecoveryPoC.exe
```

第一阶段实际启动程序是无需编译的 `RecoveryPoC.cmd`。它不执行磁盘、DISM、
BCD 或分区操作，只在本次 VM 的持久化 Windows 卷 `C:` 写入
`WinRE-PoC\recovery-started.txt`。`startnet.cmd` 还会写入
`startup-seen.txt`，用于区分“进入 WinRE”与“程序实际启动”。

## 成功条件

重启后，WinRE 自动进入且无需人工点击；随后在工作目录所在卷中同时看到：

- `startup-seen.txt`
- `recovery-started.txt`

Rust 版 `RecoveryPoC.rs` 用于后续替换脚本，不是本次自动入口验证的前置依赖。

这两个文件必须在正常 Windows 返回后仍然可读。只有该链路成立，才进入
DISM Capture/Apply 和 BCD 修复开发。

## 当前边界

本机没有 Windows ARM64/MSVC 链接器；源码先固定最小行为。注入任务专用
`winre.wim` 和注册一次性启动需要 Windows 管理员提升权限，不能由普通
`prlctl --current-user` 会话完成。

第一轮已实测 `reagentc /boottore` 能进入 Windows RE，但标准恢复 UI 不执行
`startnet.cmd`。第二轮改用 `winpeshl.ini` 的 `LaunchApps`：先运行
`RecoveryPoC.cmd` 写标记，再运行原来的 `recenv.exe`。准备脚本为
`prepare-winre-shell.ps1`。
