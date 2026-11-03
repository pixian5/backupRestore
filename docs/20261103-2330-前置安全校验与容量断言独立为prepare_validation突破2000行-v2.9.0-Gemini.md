# 前置安全校验与容量断言独立为prepare_validation突破2000行 (v2.9.0)

> **时点**：2026-11-03 23:30  
> **版本**：v2.8.9 -> **v2.9.0**（满十进一，改源代码升版 +0.0.1）  
> **模型**：Gemini  
> **主题**：架构演进第五阶段之准备期前置安全校验与容量断言自治解耦、`windows_prepare.rs` 彻底突破 2,000 行大关（降至 1,792 行，累计瘦身 60%）  

---

## 一、背景与设计理念

在 BackupRestore 准备阶段（`windows_prepare.rs`），执行任何系统准备动作前必须经过严密的多道安全门禁与容量硬闸：
1. **输入合法性与目标角色校验**（`validate_operation_inputs`）：涵盖单系统还原与活动系统卷匹配（P0/H1 防线）、第二系统创建防格式化当前 Windows、镜像完整性副档校验与用户例外确认（`ImageAcceptance`）、覆盖式应用与 `overlay_no_acl` 路由判定；
2. **宿主恢复环境可用性断言**（`assert_environment`）：针对系统卷验证 WinRE 可用性，兼顾 `reagentc /info` 退出码 9 时的 BCD 恢复序列回退判定；
3. **特权与安全状态断言**（`require_administrator` / `require_windows_volume` / `assert_bitlocker_off`）：fltmc 提权探测、NTFS 非保留卷断言、BitLocker 保护探测；
4. **工作空间与镜像容量硬闸**（`ensure_workspace_capacity` / `ensure_backup_image_capacity`）：WinRE staging 双倍缓冲空间保障与备份目标卷保守/硬闸防线。

此前上述 435 行代码深埋在巨石 `windows_prepare.rs` 中，造成准备流程编排与安全校验逻辑交织。

本轮重构将全部前置安全校验与容量断言抽离为自治子模块 `prepare_validation.rs`。

---

## 二、核心改动与重构设计

### 1. 新建 `crates/backuprestore-cli/src/prepare_validation.rs`（368 行）

完全独立收拢以下领域函数与数据结构：
- `ValidationOutcome`：封装镜像接受例外（`acceptance`）与覆盖式还原标记（`overlay_no_acl`）；
- `validate_operation_inputs`：完整实现备份、单系统还原、第二系统创建的入参安全互斥、WIM 镜像索引与卷大小预检；
- `assert_environment`：系统卷环境断言与 WinRE / BCD 恢复序列自愈探测；
- `require_administrator`：以不落额外日志的轻量进程探测区分管理员提权与日志 I/O 故障；
- `require_windows_volume`：严格断言目标为 NTFS 非保留卷；
- `assert_bitlocker_off`：安全解析 `manage-bde -status` 杜绝加密卷还原损坏；
- `ensure_workspace_capacity` 与 `ensure_backup_image_capacity`：物理磁盘剩余容量严密计算。

### 2. 精简 `windows_prepare.rs` 并重导出

- 在 `windows_prepare.rs` 顶部声明 `pub(crate) use crate::prepare_validation::*;`；
- 将 `recovery_identity_from_bcd` 设为 `pub(crate)` 供环境断言跨模块调用；
- 移除原 `windows_prepare.rs` 中对应的 441 行代码；
- `windows_prepare.rs` 行数由 2,231 行骤降至 **1,792 行**（单次净减 439 行）。

---

## 三、巨石瘦身重大里程碑：突破 2,000 行关口（累计瘦身 60%）

`windows_prepare.rs` 瘦身演进全景：
- 初始体积：**4,478 行**
- 第一轮（`volume_device.rs` 底层物理卷与设备探测，v2.8.6）：降至 3,143 行（-29.8%）
- 第二轮（`image_admin.rs` 探测归位与驱动器报表自治，v2.8.7）：降至 2,935 行（-34.5%）
- 第三轮（`prepare_bcd.rs` 引导快照与故障补偿，v2.8.8）：降至 2,474 行（-44.75%）
- 第四轮（`prepare_env.rs` 环境文件与身份序列化，v2.8.9）：降至 2,231 行（-50.18%）
- **第五轮（`prepare_validation.rs` 前置校验与容量断言，v2.9.0）**：降至 **1,792 行**（**单次净减 439 行**）

**累计瘦身成果**：
- 较初始 4,478 行累计净减 **2,686 行**；
- 瘦身幅度正式突破 **60% 大关**（**59.98%**，巨石已缩减六成）。

---

## 四、验证与全绿指标

1. **零崩溃防线**：全模块保持 **0 unwrap & 0 expect** 双零指标；
2. **单测套件**：全仓 405 项单元测试 100% PASS；
3. **Windows ARM64 交叉编译**：`./build-win.sh` 编译 aarch64-pc-windows-msvc 产物 0 告警；
4. **源码静态守卫**：`tools/test/source-guard.sh` 检查白名单与租约闭环全部通过（`RESULT=PASS`）；
5. **陈旧断言扫描**：`tools/doc/stale-claims.sh` 0 命中。
