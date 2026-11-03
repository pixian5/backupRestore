# WinRE载荷注入与引导武装独立为prepare_payload (v2.9.2)

> **时点**：2026-11-03 23:55  
> **版本**：v2.9.1 -> **v2.9.2**（改源代码升版 +0.0.1）  
> **模型**：Gemini  
> **主题**：架构演进第七阶段之 WinRE 载荷暂存、DISM 注入与引导武装独立为 `prepare_payload.rs`，`windows_prepare.rs` 缩减至 1,280 行（累计瘦身 71.4%）  

---

## 一、背景与动机

在 `BackupRestore` 的准备（Prepare）体系中，在完成安全校验与环境探测之后，最核心也是最繁杂的系统级操作是：
1. **工作区目录拓扑初始化与载荷清理**：清空并结构化 `workspace_dir`；
2. **底层 BCD 原始字节快照与备份**：调用 `prepare_bcd` 摄取 EFI 分区裸 BCD 快照，建立故障防御回滚防线；
3. **注册位 WinRE 复制与载荷注入**：从 `ReAgent.xml` 指向的路径复制 `winre.wim` 到工作区，并通过 DISM 挂载目录或 wimlib 注入工具链（`BackupRestore.exe`、`Recovery.exe`、`libwim-15.dll` 等）以及 `RecoveryTask.env` / `task.json`；
4. **提交 DISM 挂载并暂存至镜像卷**：卸载并提交修改，将定制载荷安全同步至系统保护卷或工作区；
5. **BCD 临时引导项创建与一次性启动武装**：创建自定义 WinRE 条目，调用 `arm_one_shot` 配准下一次开机直入恢复环境；
6. **文件系统缓存刷盘与系统重启**：调用 Win32 API `flush_volume` 刷盘所有受影响卷，并安全调用 `shutdown.exe /r`。

此前这整套深层、繁杂的物理暂存逻辑全部硬编码在 `windows_prepare.rs` 的 `prepare_task` 下半段或内嵌函数中。

本轮重构将整个载荷装配、载荷元数据与一次性引导武装流程完整收拢至独立的子模块 `prepare_payload.rs`。

---

## 二、核心改动与重构设计

### 1. 新建 `crates/backuprestore-cli/src/prepare_payload.rs`（287 行）

- **数据结构 `ReStaging`**：
  封装暂存工作区上下文：
  - `workspace_dir: PathBuf`
  - `staged_wim_path: PathBuf`
  - `mount_dir: PathBuf`
  - `tools_dir: PathBuf`
- **核心函数 `prepare_payload`**：
  封装 8 步核心载荷注入与引导流程：
  1. 结构化创建工作目录与清理陈旧挂载；
  2. 获取 EFI 活动 BCD 字节级快照（`snapshot_raw_bcd`）；
  3. 定位注册位 `winre.wim` 并带租约盘符拷贝到暂存目录；
  4. 注入恢复工具链（Recovery 五件套）与 Task 身份环境变量；
  5. 挂载 WIM、注入载荷、卸载并提交改动；
  6. 暂存到备用镜像卷（若配置）并配置 BCD 启动项；
  7. 写入 `payload_manifest.json`；
  8. 执行一次性启动武装（`arm_one_shot`），刷盘目标卷（`flush_volume`），按需触发重启（`shutdown /r`）。

### 2. 精简 `windows_prepare.rs`

- 在 `main.rs` 中登记并门控 `#[cfg(windows)] pub(crate) mod prepare_payload;`；
- 在 `windows_prepare.rs` 中引入并重导出 `pub(crate) use crate::prepare_payload::*;`；
- 从 `windows_prepare.rs` 中完整剥离 310 行内嵌实现与未使用的 imports；
- `windows_prepare.rs` 行数由 1,590 行暴跌至 **1,280 行**（单次净减 310 行）。

---

## 三、瘦身里程碑

`windows_prepare.rs` 七轮拆分演进历程：
- 初始体积：**4,478 行**
- 第一轮（`volume_device.rs`，v2.8.6）：降至 3,143 行（-29.8%）
- 第二轮（`image_admin.rs` 探测与驱动器报表，v2.8.7）：降至 2,935 行（-34.5%）
- 第三轮（`prepare_bcd.rs` 引导快照与补偿，v2.8.8）：降至 2,474 行（-44.75%）
- 第四轮（`prepare_env.rs` 环境文件与身份序列化，v2.8.9）：降至 2,231 行（-50.18%）
- 第五轮（`prepare_validation.rs` 前置校验与容量断言，v2.9.0）：降至 1,792 行（-59.98%）
- 第六轮（`environment_admin.rs` 环境诊断与任务收口，v2.9.1）：降至 1,590 行（-64.49%）
- **第七轮（`prepare_payload.rs` 载荷注入与引导武装，v2.9.2）**：降至 **1,280 行**（单次净减 310 行，**累计瘦身幅度正式突破 71.4%**）

---

## 四、质量门禁与验证

1. **单元测试与跨平台编译**：
   - 宿主机 `cargo test --workspace`：405 项单测全部 PASS；
   - 宿主机 `./build-win.sh`：交叉编译 0 warning 0 error；
   - 代码风格 `cargo clippy --all-targets`：0 warnings。
2. **陈旧断言扫描**：
   - `./tools/doc/stale-claims.sh` 结果全绿。
3. **源码静态守卫**：
   - `./tools/test/source-guard.sh`：RESULT=PASS。
4. **生产代码健壮性**：
   - 严守生产代码 **0 unwrap & 0 expect** 纪律。
