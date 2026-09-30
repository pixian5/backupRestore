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

测试数 29 → 39（CLI），core 19 不变。`registry_query_all()` 另有回归测试，
固定使用 `reg query "<key>"` 的完整值集查询形式，避免 Windows `reg query`
不支持同一命令多个 `/v` 参数导致操作系统/主板分组显示为不可用。

`native_gui.rs` 里只留下必须调用 Win32/子进程的部分，由 Windows ARM64 交叉
clippy 做类型检查——这也是这三个 Windows-only 文件唯一的编译验证手段。

## 实机验收结果（v1.7.2/v1.7.3，2026-09-26）

已在 Windows 11 ARM64 VM 的真实 WinRE 恢复桌面完成验收：

1. 「软硬件信息」按钮打开只读、可滚动窗口，九组信息均能显示内容或明确的
   不可用提示；中文和换行正常，可滚动到底部。
2. 末尾「按钮边界」可见，明确说明「返回 Windows」只处理 BCD/default/
   bootsequence 并重启，不会还原原始 `Winre.wim`。
3. `Alt+F4` 只关闭信息窗口，恢复桌面仍在。
4. v1.7.3 修复「返回 Windows」诊断链：`set default`、`deletevalue bootsequence`
   与 `enum {bootmgr}` 分开验证；真正成功才自动重启，失败才显示诊断框并留在
   PE，不再显示误导性的可见 `bcdedit` 暂停控制台。
5. 点击「返回 Windows」后自动回到 Windows 11，返回后核对 BCD `default` 和
   `bootsequence` 已恢复到预期状态。
6. 原始 WinRE 已恢复到注册路径 `C:\Recovery\WindowsRE\Winre.wim`，SHA-256：
   `0E09F47DC74F90AC65FE8412372A77B322831DA6E4D06BA087FF88DA1E792AC0`；
   `reagentc /info` 显示 `Windows RE 状态: Enabled`。

测试计数更新为 CLI 39、Core 19。**仍未验证完整备份/还原流程**；本节验收范围
只覆盖软硬件信息窗口、返回 Windows、v1.7.3 诊断修复和原始 WinRE 恢复链路。

实机证据保存在已忽略的 `.test-artifacts/v173-20260926/`：返回 Windows 前后截图、
返回后 BCD 枚举、恢复前后 WinRE 哈希和 `reagentc /info` 输出。恢复原始 WinRE
前快照为 `{fd14b936-561a-4cd9-86b8-b4abc58cd145}`，最终快照为
`{f1f69481-33a5-4717-bce2-5089138e7c43}`；清理后仅保留最终快照和禁止删除的
受保护基线 `{d69b3c91-7e71-43eb-b87c-3923138812f5}`。由于 `Winre.wim`
是隐藏/系统文件，直接 `Copy-Item`/`copy` 在本 VM 的服务会话中未完成替换，
最终使用 `xcopy /h /y` 复制并在复制后重新核验哈希和 `reagentc /info`。

## 2026-09-25 复核补记

在 Windows 11 ARM64 VM 的 `cmd.exe` 中实测旧命令：

- `reg query ... /v ProductName /v DisplayVersion ...` 返回 `Invalid syntax`；
- 单值 `reg query ... /v ProductName` 正常。

因此已改为按键查询完整值集，并移除 `winpeshl.ini` 这一过于宽泛的 WinRE 标记；
普通 WinPE 也可能包含该文件，WinRE 判定保留 `ReAgent.xml` / `RecEnv.exe`。
“返回 Windows”仍只处理 BCD/default/bootsequence 和重启，不负责还原原始
`Winre.wim`；本次未修改该恢复边界。
