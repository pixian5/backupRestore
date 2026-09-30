# 开发执行协议

本项目每次接到开发、测试、清理或运行任务时，必须先完成以下顺序，避免根据临时想法直接修改系统或代码。

1. **规划**：明确目标、影响的模块、是否涉及下载/管理员权限/分区/WinRE/BCD、预期验证证据和回滚边界。
2. **自审**：检查方案是否违反“程序目录即工作目录”、不触碰未授权目标分区、ARM64 实机验证边界、网络下载规则或已有任务/快照保护规则；发现冲突时先改方案。
3. **执行**：只执行自审后的最小必要改动。涉及破坏性 VM 操作前重新确认目标盘符、GUID、快照和用户授权范围。
4. **验证**：运行与改动匹配的静态检查、离线测试、Windows ARM64 构建或 WinRE 实机测试。不得用 macOS 编译或代码阅读替代 Windows/WinRE 行为验证。
5. **记录**：更新状态/验证文档，区分“代码已覆盖”“离线已验证”“实机已验证”和失败证据；完成一轮变更后递增版本。

## Windows ARM64 构建边界

Windows 客体中的 `C:\Users\x\Desktop\BackupRestore` 是与 macOS 共享的实时源码目录，只用于读取源码。Parallels 共享文件系统不能满足 Cargo 在测试过程中删除临时 archive 目录的语义，不能把它用作 `CARGO_TARGET_DIR`；否则可能出现 `os error 87`，这不是 Rust 编译或测试失败。

ARM64 包和目标测试必须分别写到客体本地浅层路径，例如 `C:\BackupRestoreBuild\target` 与 `C:\BackupRestoreBuild\test-target-v<版本>`，同时显式指定共享源码的 `Cargo.toml`。发布前核对根 `VERSION`、两个 Cargo manifest、包目录、manifest 和窗口标题。

## 产品日志

所有日志都在程序目录内，移动整个 `BackupRestore` 文件夹后不会依赖旧路径、注册表或 `C:\ProgramData`：
| 文件 | 内容 |
|---|---|
| `logs\gui.log` | GUI 启动后的关键动作、校验阻止、文件选择、WIM 读取和管理员准备启动结果 |
| `logs\launcher-errors.log` | 任务目录创建前的 CLI/准备失败，例如卷身份预检失败 |
| `logs\prepare-bootstrap.log` | 任务准备期间的共享引导记录和终态任务清理记录 |
| `tasks\<任务 ID>\prepare.log` | 该任务的 WinRE 准备、DISM、BCD 和回滚细节 |
| `tasks\<任务 ID>\recovery.log` | WinRE 中的挂载、备份/还原、BCDBoot、清理和结果 |

日志行统一由 Rust `append_log` 写入，格式固定为：

```text
[2026-08-27T12:34:56.789+00:00] 具体事件和结果
```

GUI 日志是诊断记录，采用尽力写入，不能因日志卷暂时不可写而妨碍安全校验或恢复流程；任务准备和恢复的关键日志写入失败仍会使相应操作失败。

## VM 会话与 GUI 自动化（Parallels Windows 11 ARM64，v1.4.4 踩坑记录）

以下坑全部在本机 Parallels 开发验证中实机踩过并已绕过，后续自动化必须遵守，避免重复踩坑。

### 1. prlctl exec 是服务会话（Session 0），GUI 不可见

- `prlctl exec "Windows 11" cmd /c ...` 在**服务会话**执行，看不到/无法枚举用户会话窗口：`EnumWindows` 找不到用户 GUI、`Get-Process.MainWindowHandle=0`。
- **窗口句柄跨会话无效**：把服务会话读到的 hwnd 拿去 `SendMessageW/SendMessageTimeoutW` 会报 `ERROR_INVALID_WINDOW_HANDLE (1400)`——**绝不能跨会话注入点击/消息**。
- 服务会话直接 `start` GUI：进程能起来但**窗口不显示**（无窗口站）；`prlctl exec` 还会**挂住等 GUI 消息循环退出**（start 后 exec 不返回）——启动 GUI 不要用 prlctl exec 前台跑。

### 2. 启动用户会话 GUI 的正确姿势（schtasks /it /rl highest + 包装批处理）

```text
# C:\brgui.cmd（包装脚本，避免 schtasks /tr 引号解析问题）
@echo off
start "" "C:\Users\Public\backupRestore-package\BackupRestore.exe" --tab 4

# 创建并运行（SYSTEM 权限创建，/it=交互令牌 /rl highest=提升令牌绕过 UAC）
schtasks /create /tn BRGUITest /tr C:\brgui.cmd /sc once /st 23:35 /ru x /it /rl highest
schtasks /run /tn BRGUITest
```

- **`/tr` 带空格路径 + 参数会解析失败**（任务 `LastTaskResult=2` ERROR_FILE_NOT_FOUND）——必须用**无空格路径的包装批处理**中转。
- **必须 `/rl highest`**：GUI 有自动提升逻辑（`native_gui.rs` `if !is_elevated() { return relaunch_elevated(); }`），非提升启动会 `ShellExecute(runas)` 弹 UAC 等待用户点击——没人点就空等然后退出（进程短暂出现、无窗口、无日志）。
- 任务完成后删除：`schtasks /delete /tn BRGUITest /f`。

### 3. 环境变量跨 UAC 提升丢失，命令行参数才可靠

- `BACKUPRESTORE_OPEN_TAB=4` 经批处理 `set` 后 `start` GUI：GUI 内部提升（relaunch_elevated）后**环境变量读不到**（提升进程不继承）——旧方案失效。
- **命令行参数跨提升保留**：`BackupRestore.exe --tab 4` 在提升后仍能读到。main.rs 已加 `--tab N` 特判（照 `--open-image` 模式：`env::set_var("BACKUPRESTORE_OPEN_TAB", ...)` + `launch_gui()`），WM_CREATE 里同时支持 args 与 env 双通道。

### 4. main.rs 参数解析：未知第一参数直接 usage() 退出

- `main()` 的 `match args.next()` 对**任何未登记的第一参数**走 `_ => usage()`（打印帮助 + 退出）——新增 CLI 参数**必须**在 main.rs 加特判分支（`--tab`/`--open-image`/`--pe-desktop`/`--pe-reboot` 模式），否则 GUI 根本不会启动（进程一闪而过、无日志）。

### 5. 正式包 exe 被运行中的 GUI 锁定

- `copy /y` 覆盖正式包时若 GUI 正在运行 → **“已复制 0 个文件”**（exe 被占用，时间戳不变）→ 必须**先 `taskkill /f /im BackupRestore.exe` 再 copy**；部署后核对文件时间戳与字节数。
- `taskkill /f /im` 会杀掉**所有会话**的实例（含用户会话）——部署后需重新启动用户会话 GUI 时用第 2 节方法。

### 6. 其他小坑

- **PowerShell 5.1 无 `IntPtr.Parse`**：用 `[IntPtr][System.Convert]::ToInt64($hex, 16)`。
- **PowerShell 经 cmd 传 `$_` 会被环境变量展开破坏**：复杂 PowerShell 一律写成 .ps1 文件再执行，别用 `powershell -Command` 内联传 `$_`。
- **截图**：`prlctl capture "Windows 11" --file <path>`（prlctl **没有** screenshot 子命令）；capture 截 VM 帧缓冲，**能截到用户会话画面**——是验证用户 GUI 的唯一直接证据。
- **EDIT 控件无边框看不出是输入框**：`create_control` 创建 EDIT 必须带 `WS_BORDER`（或 `WS_EX_CLIENTEDGE`），否则白底文字像普通文本。
- 服务会话与用户会话的 GUI 实例并存时按 PID 精确杀：`taskkill /f /pid <pid>`，别用 /im 误杀用户正在用的实例。
