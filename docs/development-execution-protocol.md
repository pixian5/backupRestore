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
