# 恢复环境离线备份引擎独立为 recovery_backup (v2.10.6)

> 时点：2026-11-07 01:30  
> 状态：代码重构落地 · 全量单测 430 项 PASS · Windows 交叉编译通过 (0 warning 0 error) · VM通道离线SKIP  
> 责任：Gemini (自主架构师与质量指挥官，0 Token 模式)  
> 版本：2.10.5 -> 2.10.6 (+0.0.1 源码架构重构与模块解耦)

---

## 一、架构重构背景与解耦目标

在 `crates/backuprestore-cli` 中，核心文件 `recovery_engine.rs` 承担了 WinRE/WinPE 下的所有恢复调度职责。虽然在 `v2.10.5` 中已成功抽离了交互宿主与逃生管理（`recovery_interactive.rs`），但核心调度函数 `recover_windows` 依然膨胀，将**离线备份捕获**与**系统镜像还原**硬编码在同一个大函数内。

特别是 `Operation::Backup` 分支：
- 聚合了近 350 行的备份捕获、心跳观测、DISM 排除列表动态注入、多索引追加、sidecar 元数据持久化与历史索引清理轮转逻辑；
- 辅助函数 `wim_indexes` 直接混在调度引擎中，耦合度高。

本轮 **v2.10.6** 推进 CLI 恢复引擎解耦第二弹：
将离线备份捕获、多版本保留与索引簿记全面剥离至独立自治模块 `crates/backuprestore-cli/src/recovery_backup.rs`！

**重构成果**：
- 新建自治模块 `recovery_backup.rs`（450 行）；
- 核心恢复引擎 `recovery_engine.rs` 再度瘦身 319 行，行数从 1,879 行直降至 **1,560 行**（较初始 2,485 行累计瘦身 **37.2%**）；
- `recover_windows` 中的 `Operation::Backup` 变为纯粹的单行委托调用，结构层次极其干净！

---

## 二、子系统职责边界与实现细节

新建自治模块 `crates/backuprestore-cli/src/recovery_backup.rs` 聚焦以下离线备份的核心领域职责：

1. **源卷与目标镜像路径安全解析 (`execute_recovery_backup`)**：
   - 依赖 `recovery_volume` 稳定解析源卷根目录与目标 WIM 物理路径；
   - 提取或生成标准化镜像索引名称（如 `Windows Backup` 或自定义名称）；
   - 在目标目录下安全创建父级目录结构。
2. **DISM 动态排除配置与心跳观测**：
   - 调用 core 库 `write_capture_exclusion_config` 排除 Parallels 卷根占位符、回收站、临时目录、更新缓存、各用户浏览器缓存及自身部署件；
   - 启动 `capture_heartbeat::Heartbeat` 守护线程，实时监测全卷预扫与写入进度，解决长时间静默引起的界面假死疑虑。
3. **初次捕获与原子追加双轨策略**：
   - **初次捕获**：输出 `.partial` 临时文件，调用 DISM `/Capture-Image`，核验 index 为 1 后原子重命名；
   - **增量追加**：先备份原 WIM 为 `.append-candidate.wim`，调用 DISM `/Append-Image`，回读核验证实恰好生成第 `N+1` 个索引后原子替换。
4. **WIM 索引元数据簿记与性能统计**：
   - 捕获结束后统计总耗时与平均传输速率（bytes/sec）；
   - 持久化 index-specific sidecar JSON 元数据与 SHA-256 指纹；
   - 严格维护旧版 index-1 legacy sidecar 兼容性。
5. **多版本保留策略与历史索引轮转清理 (`keep_indexes`)**：
   - 当任务配置保留数量 `keep` 时，逆序循环调用 DISM `/Delete-Image` 删除最早的历史索引；
   - 每次删除后立即调用 `wim_indexes` 回读核验剩余索引连续性，任何不符立即熔断；
   - 索引删除后调用 `renumber_sidecars` 同步更新元数据副档。
6. **DISM 索引解析下沉与跨平台可测化 (`parse_wim_indexes_output`)**：
   - 将原 Windows 专属的 `wim_indexes` 中的解析逻辑下沉为纯函数 `parse_wim_indexes_output`；
   - 在 macOS / Linux 环境下即可直接运行单测，验证单索引、多索引乱序排序去重、空输出与 0 索引防御。

---

## 三、单元测试覆盖与门面导出

1. **向下兼容与接口收口**：
   - `crates/backuprestore-cli/src/main.rs` 注册 `mod recovery_backup;`；
   - `crates/backuprestore-cli/src/recovery_engine.rs` 顶层统一使用 `pub(crate) use crate::recovery_backup::*;` 重导出，下游零漂移。
2. **新增测试验证**：
   - 新增 `tests::parse_wim_indexes_handles_single_and_multiple`；
   - 新增 `tests::parse_wim_indexes_rejects_empty_or_zero`；
   - 全仓单元测试增至 **430 项**（CLI 352 项 + Core 78 项），100% 通过（0 错误，0 警告）。
3. **门禁检查**：
   - 生产代码保持 **0 unwrap & 0 expect** 双零规范；
   - 源码静态守卫 `tools/test/source-guard.sh`：PASS；
   - 陈旧断言扫描 `tools/doc/stale-claims.sh`：PASS（0 命中）；
   - Windows ARM64 交叉编译 `./build-win.sh`：PASS（0 代码警告）。
