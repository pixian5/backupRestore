# 准备期 BCD 快照与引导回滚补偿独立为 prepare_bcd (v2.8.8)

> 时点：2026-11-03 22:50  
> 模型：Gemini  
> 变更性质：改源代码升版 +0.0.1（2.8.7 -> 2.8.8）  
> 核心任务：将准备期 ESP 分区探测、活动引导指纹鉴别、BCD 字节级快照捕获与准备故障统一回滚补偿彻底解耦为独立模块 `prepare_bcd.rs`；`windows_prepare.rs` 瘦身幅度突破 40% 降至 2,474 行。

---

## 一、背景与设计考量

在上一版（v2.8.7）完成了 WIM 权威探测与驱动器报表发现的解耦之后，`windows_prepare.rs` 降至 2,935 行。
继续审计代码结构，发现准备阶段的启动链安全保障机制存在一个高度内聚的领域：
1. **ESP 分区免盘符定位**：Win32 原生卷路径枚举、通过 BCD 指纹挑选真实活动 ESP 卷；
2. **BCD 快照捕获与持久化**：在触碰引导配置前捕获 byte-for-byte 的 raw 快照（或系统占用时的逻辑导出备份）；
3. **准备期统一补偿机制**：当准备任务在条目创建、DISM 注入、一次性启动项武装、卷缓存强刷或关机请求任一步失败时，统一反向执行补偿（清一次性启动项、删本任务 GUID 引导对象、删镜像卷载荷、恢复 BCD 快照）。

该机制此前混杂在 `windows_prepare.rs` 中间（1011-1460 行，共 450 行）。在恢复环境侧已有 `recovery_bcd.rs` 负责离线 BCD 编排的前提下，将在线准备态对应的引导保障与快照回滚子系统收拢至 `prepare_bcd.rs`，形成对称清晰的架构形态。

---

## 二、重构明细

### 1. 新建 `crates/backuprestore-cli/src/prepare_bcd.rs`（402 行）
完整封装在线准备态 BCD / ESP 子系统：
- **ESP 分区与 BCD store 路径解析**：
  - `efi_bcd_store_path`：优先使用 `\\?\Volume{GUID}\EFI\Microsoft\Boot\BCD` 免盘符 verbatim 路径，GUID 缺失时保底使用带租约临时盘符；
  - `esp_identity_without_drive_letter`：原生枚举隐藏卷并校验 EFI 类型与 Boot/BCD 存在性；
  - `active_esp_by_store_fingerprint` / `boot_manager_fingerprint`：比对活动系统 `{bootmgr}` 指纹（default, display_order, resume_object）在多 ESP 场景下自动挑出引导 ESP；
  - `native_volume_paths`：基于 Win32 API（`FindFirstVolumeW` / `FindNextVolumeW`）直接枚举底层卷 GUID。
- **BCD 快照与统一故障补偿**：
  - `snapshot_raw_bcd`：抓取原始 BCD 存储快照并校验 SHA-256，锁定时回退到逻辑导出；
  - `compensate_prepare_failure`：统一失败补偿总闸门，撤销本任务已产生的全部引导与载荷副作用；
  - `rollback_bcd_snapshot` / `rollback_boot_request`：快照回退实现，保护当前引导服务免遭会话级毒化。

### 2. 重构 `windows_prepare.rs`
- 引入 `pub(crate) use crate::prepare_bcd::*;` 并重导出公共函数；
- 移除 1011-1460 行的全部 BCD 快照与回滚补偿函数；
- 清理不再使用的 `capture_logged` 导入；
- 代码行数由 2,935 行骤降至 **2,474 行**（单次净减 461 行）；
- 相比初始 4,478 行，累计净减 2,004 行，**累计瘦身幅度达 44.75%**。

### 3. 主入口注册
- 在 `main.rs` 注册 `#[cfg(windows)] pub(crate) mod prepare_bcd;`。

---

## 三、验证记录

### 1. 本地测试与静态审计
- **单元测试**：`cargo test --workspace`，405 项单测全部通过（347 cli + 58 core）。
- **Clippy 检查**：`cargo clippy -p backuprestore-cli --all-targets`，0 警告。
- **Windows ARM64 交叉编译**：`./build-win.sh` 编译通过，0 Rust 代码告警。
- **源码静态守卫**：`./tools/test/source-guard.sh` 校验通过（`RESULT=PASS`）。
- **陈旧断言扫描**：`./tools/doc/stale-claims.sh` 0 命中（`RESULT=PASS`）。
- **双零防线**：生产代码持续坚守 0 unwrap & 0 expect。
