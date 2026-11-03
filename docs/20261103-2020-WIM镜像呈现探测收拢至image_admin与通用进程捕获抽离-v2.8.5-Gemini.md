# WIM镜像呈现探测收拢至image_admin与通用进程捕获抽离 (v2.8.5)

> 时点：2026-11-03 20:20  
> 基线：`v2.8.4` (commit `b5a1d95`) -> `v2.8.5`  
> 模型：Gemini  
> 状态：PASS (405 单测通过 / Clippy 0 警告 / 交叉编译 0 告警 / 8 项 pre-push 门禁全绿)

---

## 一、背景与重构动机

随着 CLI 主入口 `main.rs` 成功瘦身突破 90%，下一步重点推进领域子模块之间职责的高内聚收拢：
1. **WIM 镜像呈现与探测逻辑错位**：原 `wim_info` 与 `sidecar_dism_names`（负责读取 WIM 镜像 sidecar 副档、DISM 元数据解析并输出结构化 JSON）位于 `windows_prepare.rs` 中。然而，`windows_prepare.rs` 的核心职责是 Windows 离线/恢复环境部署与引导准备，查看镜像与元数据解析属于镜像管理体系；
2. **进程捕获逻辑重复**：`windows_prepare.rs` 内部私有实现了一个 `fn capture(program, args)`，未复用通用的 `process_runner` 基础设施；
3. **主入口命令参数解析**：`main.rs` 中调用 `wim_info` 时仍保留着一段 12 行的手工标志提取逻辑。

本期重构目标：**将 WIM 信息探测与呈现逻辑完整迁入自治子模块 `image_admin.rs`，在 `process_runner.rs` 抽离统一的 `capture_output` 进程输出捕获函数，并将 `main.rs` 中的 `wim-info` 命令解析彻底单行收拢。**

---

## 二、重构核心改动

### 1. 通用进程输出捕获器抽离（`process_runner.rs`）
在 `crates/backuprestore-cli/src/process_runner.rs` 新增：
```rust
#[cfg(windows)]
pub(crate) fn capture_output(program: &str, args: &[&str]) -> Result<String, TaskError>
```
封装静默运行命令行程序（`CREATE_NO_WINDOW`）、标准输出与标准错误 UTF-8 解码及失败带状态码报错逻辑。  
`windows_prepare.rs` 内原有的 `fn capture` 直接改为调用 `crate::process_runner::capture_output`，消灭重复代码。

### 2. 镜像探测与信息呈现自治收拢（`image_admin.rs`）
- 将 `wim_info` 与 `sidecar_dism_names` 从 `windows_prepare.rs` 完整迁移至 `image_admin.rs`；
- 在 `image_admin.rs` 封装 CLI 子命令驱动入口：
  ```rust
  #[cfg(windows)]
  pub(crate) fn cli_wim_info(args: Vec<String>) -> Result<(), TaskError>
  ```
  自动解析 `--skip-hash` 标志与镜像路径，并调用 `wim_info`；
- 所有 WIM 镜像相关命令（`list-images`、`delete-old-indexes`、`wim-info`）完全在 `image_admin.rs` 闭环。

### 3. 主入口与文件行数进一步收敛
- **`main.rs`**：`Some("wim-info") => crate::image_admin::cli_wim_info(args.collect())`，彻底消灭参数提取循环，行数从 628 行下调至 **617 行**；
- **`windows_prepare.rs`**：剥离 180 行镜像查看代码，行数从 3,827 行骤降至 **3,647 行**（净减 180 行）。

---

## 三、质量与验证

1. **测试用例**：全仓 405 项单测全部通过（CLI 347 项 + Core 58 项）：
   ```text
   test result: ok. 347 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.14s
   test result: ok. 58 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.08s
   ```
2. **静态扫描与跨平台**：
   - `cargo clippy -p backuprestore-cli --all-targets` 0 警告；
   - `./build-win.sh` 编译通过（产物 3,461,120 字节）；
   - `./tools/test/source-guard.sh` 静态守卫 `RESULT=PASS`；
   - 生产代码保持 **0 unwrap & 0 expect** 双零指标。
3. **Pre-push 门禁与陈旧断言**：
   - `./tools/doc/stale-claims.sh` 扫描 STALE_HITS=0；
   - 8 项 Git 提交门禁全绿。
