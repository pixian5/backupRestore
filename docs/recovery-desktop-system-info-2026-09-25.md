# 恢复桌面「软硬件信息」按钮（v1.7.0，2026-09-25）

## 为什么需要

恢复桌面原有六个入口全是**动作**（备份/还原/第二系统/命令提示符/返回 Windows/
打开完整程序）。用户在 WinRE 里最需要的第一步却是**看清现状**：这台机器是什么
硬件、装的哪个 Windows、磁盘分区什么样、WinRE 和 BitLocker 状态如何。没有这个
入口时只能开命令提示符逐条敲，在无键盘的 VM 里尤其难用。

## 采集边界（只读，且不依赖 PowerShell/WMI）

WinRE 是精简镜像：**没有 PowerShell，没有 WMI，可能没有显卡驱动，可能不支持
BitLocker**。因此全部改用 PE/RE 一定自带的控制台工具：

| 分组 | 命令 |
|---|---|
| 概览 | `hostname`、Win32 指标、`CARGO_PKG_VERSION` |
| 操作系统 | `reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion"` |
| 处理器与内存 | `systeminfo`（只保留 CPU/内存/系统类型行） |
| 主板与固件 | `reg query "HKLM\HARDWARE\DESCRIPTION\System\BIOS"` |
| 显示设备 | `pnputil /enum-devices /class Display /connected` + `GetSystemMetrics` |
| 存储 | `echo list disk \| diskpart`、`echo list volume \| diskpart` |
| 网络 | `ipconfig /all` |
| 恢复与安全 | `reagentc /info`、`manage-bde -status`、SecureBoot 注册表键 |

关键设计：**任何一项失败都不是致命错误**，只显示「未检测到/不可用」。
`sysinfo_probe` 因此不把非零退出码当失败——WinRE 里 `manage-bde` 没有 BitLocker
就会非零退出，这属正常。只有输出确实为空才降级为不可用提示。

编码上采用 `decode_bcdedit_bytes` 统一解码：`reg`/`diskpart` 输出是 OEM/ANSI，
`bcdedit` 常是 UTF-16LE，混在一起必须按字节嗅探，不能假定 UTF-8。

## 底栏环境标签修正

底栏此前**硬编码为 `WinPE`**，导致经 `reagentc /boottore` 进入 WinRE 的运行被
错标。现由 `sysinfo_environment_label()` 实际判定：

1. `SystemRoot` 不在 `X:` → `Windows`（已安装系统）。
2. 在 `X:` 且存在 `ReAgent.xml` / `winpeshl.ini` / `RecEnv.exe` 等恢复脚手架
   → `WinRE`。
3. 在 `X:` 但没有这些标记 → `WinPE`。

判定只看环境变量和文件是否存在，不执行任何命令，因此不会引入新的阻塞风险。

## 「返回 Windows」的边界已在界面内写明

信息窗口末尾固定显示【按钮边界】段，明确：

> 「返回 Windows」只恢复/设置 BCD 默认项、清理一次性启动状态并重启；
> 它不会还原原始 `Winre.wim`。

这是实测确认的行为（`exit_pe_to_windows` 只动 BCD 与重启）。把它写进界面是因为
「返回 Windows」这个名字容易让人以为它会把系统恢复原状，包括 WinRE 镜像。

## 测试盲区处理

`native_gui.rs` 由 `#[cfg(windows)]` 门控，macOS 上 `cargo test` 触及不到。
沿用既有约定，把纯文本逻辑放进 `text_parsing.rs`：

- `format_system_info_report`：分组稳定性 + 缺失兜底（2 个测试）
- `systeminfo_cpu_memory`：中英文输出过滤、修补程序列表不泄漏、空输入（2 个测试）

测试数 29 → 33（CLI），core 19 不变。

`native_gui.rs` 里只留下必须调用 Win32/子进程的部分，由 Windows ARM64 交叉
clippy 做类型检查——这也是这三个 Windows-only 文件唯一的编译验证手段。

## 未验证项（重要）

**本轮只完成离线验证与构建，尚未在 VM 内实机点击。** 原因：VM 当前停在临时替换的
GUI WinRE 里供用户观察，按项目约束不得在用户确认前重启、部署或改动 VM 状态。

实机验收仍需：

1. 用户确认观察结束。
2. 按 `AGENTS.md` 先创建并核验新快照。
3. 部署 v1.7.0 载荷并核对哈希。
4. 进入恢复桌面点击「软硬件信息」，确认七宫格布局、窗口可滚动、九个分组都有内容
   或明确的不可用提示、底栏显示 `WinRE`。
5. 恢复原始 WinRE 并核验 SHA-256 `0E09F47D…E792AC0`。
