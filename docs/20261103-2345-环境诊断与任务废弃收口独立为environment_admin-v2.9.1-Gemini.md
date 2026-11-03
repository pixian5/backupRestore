# 环境诊断与任务废弃收口独立为environment_admin (v2.9.1)

> **时点**：2026-11-03 23:45  
> **版本**：v2.9.0 -> **v2.9.1**（改源代码升版 +0.0.1）  
> **模型**：Gemini  
> **主题**：架构演进第六阶段之环境诊断、启动项呈现与任务废弃收口独立为 `environment_admin.rs`，`windows_prepare.rs` 缩减至 1,590 行（累计瘦身 64.5%）  

---

## 一、背景与动机

在 CLI 与 GUI 交互中，存在三类重要但独立于准备流程主干的系统级管理动作：
1. `list_boot_entries`（`list-boot-entries` 命令）：枚举 Windows BCD 中的所有启动项描述（供 GUI 撞名预检与引导菜单自适应布局解析）；
2. `inspect_environment`（`inspect-environment` 命令）：探测宿主操作系统版本、硬件架构、WinRE 注册可用性及系统各物理驱动器列表（供 GUI 初始化诊断与面板状态刷新）；
3. `abandon_task`（`abandon-task` 命令及挂起孤儿任务清理）：针对未进入破坏性擦除阶段或已处于终态（Failed）的任务进行事务补偿、解除一次性 UEFI 启动项、清除暂存 WinRE 载荷并安全关闭任务记录。

此前这三个函数深植于 `windows_prepare.rs` 中，导致准备引擎与外部诊断/收口逻辑交织。

本轮重构将上述职责抽离为高内聚子模块 `environment_admin.rs`，并实现顶层路由的直接分发。

---

## 二、核心改动与重构设计

### 1. 新建 `crates/backuprestore-cli/src/environment_admin.rs`（198 行）

- `list_boot_entries`：静默调用 `bcdedit.exe /enum all /v`，解码输出并组装 JSON 结构返回启动项描述列表或错误原因；
- `inspect_environment`：聚合调用 `cmd.exe /c ver`、`reagentc /info`（容错至 BCD 探测）与 `discover_drives()`，组装环境元数据 JSON；
- `abandon_task`：完整接管任务放弃与终态收口逻辑，校验任务前后一致性，执行 BCD 补偿（`compensate_prepare_failure`）、撤销引导（`disarm`）并清理工作空间载荷。

### 2. 精简 `windows_prepare.rs` 与路由直通

- 在 `main.rs` 中直接将 `list-boot-entries`、`inspect-environment` 与 `abandon-task` 命令路由至 `environment_admin`；
- 在 `pending_task.rs` 中将孤儿任务自动收口直接调用 `environment_admin::abandon_task`；
- 将 `windows_prepare.rs` 中的 `efi_identity` 设为 `pub(crate)` 供跨模块复用，并重导出 `environment_admin`；
- `windows_prepare.rs` 行数由 1,792 行进一步降至 **1,590 行**（单次净减 202 行）。

---

## 三、瘦身里程碑

`windows_prepare.rs` 六轮拆分演进历程：
- 初始体积：**4,478 行**
- 第一轮（`volume_device.rs`，v2.8.6）：降至 3,143 行（-29.8%）
- 第二轮（`image_admin.rs` 探测与驱动器报表，v2.8.7）：降至 2,935 行（-34.5%）
- 第三轮（`prepare_bcd.rs` 引导快照与补偿，v2.8.8）：降至 2,474 行（-44.75%）
- 第四轮（`prepare_env.rs` 环境文件与身份序列化，v2.8.9）：降至 2,231 行（-50.18%）
- 第五轮（`prepare_validation.rs` 前置校验与容量断言，v2.9.0）：降至 1,792 行（-59.98%）
- **第六轮（`environment_admin.rs` 环境诊断与任务收口，v2.9.1）**：降至 **1,590 行**（单次净减 202 行，**累计瘦身幅度达 64.49%**）

---

## 四、验证与全绿指标

1. **双零防线**：生产代码持续坚守 **0 unwrap & 0 expect** 指标；
2. **本地单测**：全仓 405 项单元测试 100% PASS；
3. **Windows ARM64 交叉编译**：`./build-win.sh` 编译 aarch64-pc-windows-msvc 产物 0 告警顺利完成；
4. **源码静态守卫**：`tools/test/source-guard.sh` 验证租约生命周期（`HOLD_FOR_PROCESS_COUNT=3`，`RESULT=PASS`）；
5. **陈旧断言扫描**：`tools/doc/stale-claims.sh` 0 命中。
