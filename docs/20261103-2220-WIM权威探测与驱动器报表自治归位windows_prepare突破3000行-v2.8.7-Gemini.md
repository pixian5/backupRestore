# WIM权威探测与驱动器报表自治归位 windows_prepare 突破 3,000 行 (v2.8.7)

> 时点：2026-11-03 22:20  
> 模型：Gemini  
> 变更性质：改源代码升版 +0.0.1（2.8.6 -> 2.8.7）  
> 核心任务：将 WIM 权威挂载探测完整迁移至 `image_admin.rs`；将驱动器报表与卷发现完全收拢至 `volume_device.rs`；`windows_prepare.rs` 瘦身突破 3,000 行大关降至 2,935 行。

---

## 一、背景与设计考量

在上一版（v2.8.6）将 Win32 底层硬件 FFI 与物理卷/设备探测独立为 `volume_device.rs` 后，`windows_prepare.rs` 行数降至 3,143 行。继续审计发现，该文件中仍然存在两处领域职责与准备任务编排不相称的残留：
1. **WIM 镜像挂载与完整 Windows 特征探测**：`wim_full_windows_probe`、`wim_mount_markers`、`wim_image_display_name` 混杂在准备引导逻辑中间，与已有的镜像管理模块 `image_admin.rs` 职责重合；
2. **高阶驱动器报表与卷发现**：`DriveReport`、`discover_drives`、`list_volumes` 与底层卷探测紧密关联，理应随底层卷设备探测一同收拢在 `volume_device.rs` 中，并通过统一的 CLI 驱动接口对外部暴露。

---

## 二、重构明细

### 1. WIM 权威挂载与显示名探测自治归位至 `image_admin.rs`
- 在 `crates/backuprestore-cli/src/image_admin.rs` 中：
  - 引入并实现 `pub(crate) fn wim_full_windows_probe(image_path: &str, index: u32, log: &Path) -> Result<ProbeOutcome, TaskError>`；
  - 完整迁移私有实现 `wim_mount_markers`（只读挂载并探测 `reagentc.exe` 与 `SYSTEM` hive）与 `wim_image_display_name`（DISM `/Get-WimInfo` 读取显示名）；
- 在 `windows_prepare.rs` 中：
  - 移除原 141 行探测逻辑；
  - 顶部重导出 `pub(crate) use crate::image_admin::wim_full_windows_probe;`，保持下游调用方（`pe_operation.rs`、`online_operation.rs`）兼容无缝无感知。

### 2. 驱动器报表与卷发现收拢至 `volume_device.rs`
- 在 `crates/backuprestore-cli/src/volume_device.rs` 中：
  - 迁入 `DriveReport` 结构体及 `pub(crate) fn discover_drives() -> Result<Vec<DriveReport>, TaskError>`；
  - 封装统一的 CLI 命令驱动入口 `pub(crate) fn cli_list_volumes() -> Result<(), TaskError>`；
  - 提供跨平台的体系结构探测函数 `pub(crate) fn native_architecture() -> String`；
- 在 `windows_prepare.rs` 中：
  - 彻底移除 `DriveReport`、`list_volumes`、`discover_drives`、`native_architecture` 等重复定义；
- 在 `main.rs` 中：
  - 将 CLI 命令 `"list-volumes"` 改为单行直接分发：`volume_device::cli_list_volumes()`。

### 3. 行数与架构收益
- `windows_prepare.rs` 行数由 3,143 行骤降至 **2,935 行**（单次净减 208 行），**正式突破 3,000 行里程碑关口**；
- 较最初的 4,478 行，累计净减 1,543 行，累计瘦身幅度达 **34.5%**；
- 生产代码保持 0 unwrap & 0 expect 双零防线。

---

## 三、验证记录

### 1. 本地测试与静态审计
- **单元测试**：`cargo test --workspace`，405 项单测全部通过（347 cli + 58 core）。
- **Clippy 检查**：`cargo clippy -p backuprestore-cli --all-targets`，0 警告。
- **Windows ARM64 交叉编译**：`./build-win.sh` 编译通过，0 Rust 代码告警。
- **源码静态守卫**：`./tools/test/source-guard.sh` 校验通过（`RESULT=PASS`）。
- **陈旧断言扫描**：`./tools/doc/stale-claims.sh` 0 命中（`RESULT=PASS`）。
