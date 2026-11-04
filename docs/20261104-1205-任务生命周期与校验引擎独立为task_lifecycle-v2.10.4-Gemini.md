# 任务生命周期与校验引擎独立为 task_lifecycle (v2.10.4)

> 时点：2026-11-04 12:05  
> 状态：代码重构落地 · 全量单测 427 项 PASS · 虚拟机原生编译部署 PASS  
> 责任：Gemini (自主架构师与质量指挥官，0 Token 模式)  
> 版本：2.10.3 -> 2.10.4 (+0.0.1 改动源码/架构行为)

---

## 一、架构重构背景与里程碑意义

在 `crates/backuprestore-core` 中，`lib.rs` 最初承载了 3,463 行的巨石逻辑，聚合了 I/O 驱动、持久化、路径处理、排除列表、状态机、容量预检、副档元数据与核心校验。
经过多轮战略级抽离：
1. `v2.9.4`：抽离 `capture_exclusion.rs`，`lib.rs` 降至 2,842 行；
2. `v2.9.5`：抽离 `task_store.rs`，`lib.rs` 降至 1,673 行；
3. `v2.10.0`：抽离 `volume_role.rs`，`lib.rs` 降至 1,350 行；
4. `v2.10.1`：抽离 `capacity_plan.rs`，`lib.rs` 降至 1,185 行；
5. `v2.10.2`：抽离 `stage.rs`，`lib.rs` 降至 1,051 行；
6. `v2.10.3`：抽离 `task_spec.rs`，`lib.rs` 跌破千行降至 847 行。

本轮 **v2.10.4** 迎来了 Core 库解耦的终极里程碑：将任务合法性校验引擎（`Task::validate`）、状态机跃迁调度（`Task::transition`）以及全套校验单元测试抽离至独立自治模块 `crates/backuprestore-core/src/task_lifecycle.rs`！
**`lib.rs` 瘦身 642 行骤降至 205 行，累计瘦身幅度达 94.08%！彻底完成门面化（Facade）蜕变！**

---

## 二、子系统职责边界与实现细节

新建自治模块 `crates/backuprestore-core/src/task_lifecycle.rs`（608 行），收拢以下核心逻辑：

1. **多业务线分区互斥与防自毁校验 (`Task::validate`)**：
   - **版本与引导模式**：版本锁定 `TASK_VERSION = 1`，严格校验 `boot_once`（一次性启动）以及 BCD 回滚哈希快照；
   - **压缩算法白名单**：调用 `canonical_compression` 守卫 `fast` 与 `none`，阻断历史废弃的 `max`；
   - **载荷与例外绑定**：强校验 `payload.task_id == self.task_id`，校验四大载荷 SHA-256；强校验 `ImageAcceptance` 绑定的索引、尺寸与哈希，确保例外记录无法被篡改或泛化放行；
   - **业务线分区边界互斥**：
     - `Probe`：阻断 image/destination/target；
     - `Backup`：强校验源分区与目标镜像卷不同（防止卷内死循环写入），阻断 EFI/MSR/Recovery 保留分区；
     - `RestoreExisting`：强校验还原目标与现有 Windows 系统在同一分区（自替换），目标角色必须为 `ExistingWindows`，引导模式必须为 `ReturnExisting`；
     - `CreateSecondary`：强校验第二系统目标与现有源分区不同，目标角色必须为 `NewWindows`，引导模式必须为 `AddSecondary`，且严格防范控制字符与超长菜单名；
   - **工作区独立性与自毁防御**：强校验工作卷必须存在且非保留分区，且**工作卷绝不能与还原目标卷为同一物理分区**（防止解压覆盖自身执行文件）。
2. **状态机安全推进 (`Task::transition`)**：
   - 依赖 `Stage::can_transition_to` 状态机流转图，非法跃迁直接熔断返回 `TaskError::InvalidTransition`（携带契约错误码 `BR-TSK-028`）；
   - 跃迁成功后原子产出 `StatusRecord` 并折算基准进度。
3. **辅助判定**：
   - `missing(name)`：提供标准的 `BR-CORE-033` 字段缺失错误；
   - `valid_menu_name(value)`：限制最大 256 字符且严禁包含任何控制字符。

---

## 三、门面化与单元测试覆盖

1. **`lib.rs` 门面化成果**：
   - 重构前：847 行；
   - 重构后：**205 行**；
   - 本次净减：**642 行**；
   - 较初始 3,463 行累计净减：**3,258 行（累计瘦身 94.08%）**！
   - `lib.rs` 仅保留统一公共导出、版本常量、`TaskError` 枚举与 `Task` / `StatusRecord` 核心数据模型声明。
2. **单测扩充与验证**：
   - 将原有 14 组校验测试完整迁入 `task_lifecycle.rs`；
   - 新增 `task_transition_lifecycle` 专门覆盖合法状态机推进与非法跳跃拦截断言；
   - 在 `lib.rs` 保留 `task_new_initial_state` 门面冒烟测试；
   - core 库单测增至 **78 项**，全仓单测增至 **427 项**（349 cli + 78 core），100% PASS。

---

## 四、质量门禁与验证矩阵

1. `cargo check --workspace`：编译 0 错误 0 警告；
2. `cargo test --workspace`：**427 项单测全部通过**（0 失败，0 忽略）；
3. `tools/test/source-guard.sh`：源码静态安全守卫 `RESULT=PASS`；
4. `tools/doc/stale-claims.sh`：陈旧断言扫描 `RESULT=PASS`（0 命中）；
5. Windows ARM64 交叉编译：`./build-win.sh` 编译通过；
6. 虚拟机原生构建部署：VM 内 Windows ARM64 原生编译部署至 `H:\brwork`，GUI 快速巡检 `RESULT=PASS`。
