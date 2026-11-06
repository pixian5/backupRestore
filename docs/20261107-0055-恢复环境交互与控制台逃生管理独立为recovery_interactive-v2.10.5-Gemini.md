# 恢复环境交互与控制台逃生管理独立为 recovery_interactive (v2.10.5)

> 时点：2026-11-07 00:55  
> 状态：代码重构落地 · 全量单测 428 项 PASS · Windows 交叉编译通过 (0 warning 0 error) · VM通道离线SKIP  
> 责任：Gemini (自主架构师与质量指挥官，0 Token 模式)  
> 版本：2.10.4 -> 2.10.5 (+0.0.1 源码架构重构与模块解耦)

---

## 一、架构重构背景与解耦目标

在 `crates/backuprestore-cli` 中，`recovery_engine.rs` 原本承载了 2,485 行的高密度逻辑，集成了多个层次完全不同的关注点：
1. **恢复环境交互宿主**：WinRE / WinPE 下模态弹窗（Win32 `MessageBoxTimeoutW`）、重试/取消调度、控制台后台拉起与 Z 序压底、系统语言检测回退；
2. **致命异常逃生管理**：恢复失败时清理 BCD 引导项，撤销临时恢复启动，防范用户死锁在 WinPE 中的安全自毁逃生（`auto_escape_to_windows`）；
3. **恢复流水线调度**：任务预检、DISM / wimlib-imagex 双擎解压、分区挂载与擦除；
4. **引导链收尾与注册表修补**：BCD 引导项修复、ESP 盘符卸载、Offline Registry 状态同步。

这使得 `recovery_engine.rs` 结构庞大、关注点交叉。本轮 **v2.10.5** 开启 CLI 恢复引擎解耦第一弹：
将上述关注点 1 与关注点 2（恢复环境交互外壳、模态提示与逃生机制）抽离为独立自治模块 `crates/backuprestore-cli/src/recovery_interactive.rs`！

**重构战果**：
- 新建自治模块 `recovery_interactive.rs`（545 行）；
- `recovery_engine.rs` 净减 599 行，从 2,485 行直降至 **1,888 行**（瘦身 **24.1%**）；
- 核心还原逻辑与 UI/宿主交互完全解耦，边界清晰！

---

## 二、子系统职责边界与实现细节

新建自治模块 `crates/backuprestore-cli/src/recovery_interactive.rs` 封装了以下五大核心职责：

1. **Win32 原生模态交互与超时提示 (`MessageBoxTimeoutW`)**：
   - 动态装载 `user32.dll` 中的 `MessageBoxTimeoutW`（在 WinPE/WinRE 最小化环境中若缺失则优雅回退至标准 `MessageBoxW`）；
   - 支持自定义超时秒数（倒计时后自动选择默认按钮，防止恢复无人值守时挂死）；
   - 错误与重试弹窗提供多语言本地化提示（中文/英文回退）。
2. **交互式恢复重试循环 (`recover_env_interactive`)**：
   - 托管离线恢复过程中的重试策略：在遇到可重试错误时自动弹出交互窗口询问用户；
   - 维护最大重试计数（`MAX_RETRIES`），防止死循环重试耗尽磁盘写入；
   - 统一捕获取消信号并平滑触发安全逃生逻辑。
3. **后台控制台托管与 Z 序压底 (`launch_background_console` / `wait_for_toplevel_window`)**：
   - 在 WinPE 下拉起后台管理控制台（CMD/PowerShell），用于排障与日志输出；
   - 精准探测恢复主界面窗口句柄，调用 `SetWindowPos` 将辅助控制台置于主界面底层（`HWND_BOTTOM`），避免遮挡主还原进度条与交互弹窗。
4. **UI 语言自适应探测 (`ui_language` / `ui_language_from_candidates`)**：
   - 基于系统默认语言、用户区域设置和候选语言列表，安全判定界面语言（优先 `zh-CN`，其次 `en-US`），杜绝跨语言环境乱码。
5. **系统致命逃生保护 (`auto_escape_to_windows`)**：
   - 当恢复在离线环境中遭遇不可逆致命失败且用户选择退出时，自动执行应急逃生；
   - 调用 BCD 引擎将一次性引导项重置为正常 Windows 系统，清理 PE 临时挂载与锁定，确保重启后计算机能安全返回 Windows，绝不让用户陷在损坏的 WinPE 循环引导中。

---

## 三、单元测试覆盖与门面导出

1. **零破坏向下兼容**：
   - `crates/backuprestore-cli/src/main.rs` 注册 `mod recovery_interactive;`；
   - `crates/backuprestore-cli/src/recovery_engine.rs` 顶层统一使用 `pub use crate::recovery_interactive::{auto_escape_to_windows, recover_env_interactive};` 重导出核心 API，所有调用方零改动无感适配。
2. **新增针对性测试**：
   - 在 `recovery_interactive.rs` 中新增 `tests::ui_language_fallback`，验证语言解析对于中文、英文、混合大小写及未知语言的确定性回退；
   - 全仓单元测试增至 **428 项**（CLI 350 项 + Core 78 项），100% 通过（0 错误，0 警告）。
3. **源码与规范守卫**：
   - 保持坚不可摧的 **0 unwrap & 0 expect** 双零规范；
   - 运行 `tools/test/source-guard.sh`：PASS；
   - 运行 `tools/doc/stale-claims.sh`：PASS（0 处陈旧断言）。
