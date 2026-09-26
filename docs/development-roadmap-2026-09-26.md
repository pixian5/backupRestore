# BackupRestore 下一步开发路线图（2026-09-26）

文档状态：**规划与风险排序文档，不包含代码改动，不代表任何未完成项已修复或已验收。**

规划基线：源码 `v1.7.3`（commit `935becd`）+ 现有文档证据。
主要依据：
- [project-status.md](project-status.md)：当前进度、已完成与未完成项；
- [winre-repair-implementation-plan-2026-09-25.md](winre-repair-implementation-plan-2026-09-25.md)：F1～F4 缺口与两段式修复方案（待实施）；
- [peer-comparison.md](peer-comparison.md)：与同类成熟工具的能力差距；
- [verification-matrix.md](verification-matrix.md)：逐项代码/离线/实机证据状态。

发生冲突时，优先级为：用户最新指令 > 源代码与已保存实机证据 > 本文。

---

## 1. 现状一句话

BackupRestore 的安全模型（GUID 多重身份、状态机、`.partial`、WinRE/BCD 双快照、载荷 SHA-256 哈希链）和非 C 分区真实 Capture/Apply 已在 ARM64 实机验证；但**恢复入口机制存在已确认的代码缺口**，且独立 EFI 首启动链路未闭环，因此尚不能作为通用灾备产品发布。

---

## 2. 风险排序（按严重度与阻塞关系）

| ID | 风险 / 缺口 | 严重度 | 证据 | 潜在影响 | 当前状态 |
|---|---|---|---|---|---|
| F1 | WinRE 位于普通 OS 卷且该卷被选作还原目标，格式化可能删除当前恢复入口 | **P0** | `windows_prepare.rs::recovery_identity/prepare_task`、`main.rs::format_target_partition/restore_original_winre` | 还原后系统无恢复环境，可能无法再进 WinRE | 代码未拒绝，仅设计 |
| F3 | 备份含 WinRE 的源卷时，Capture 可能把当前任务注入的启动入口/env/可续跑任务打进镜像 | **P0**（与 F1 同批） | `prepare_payload`、`recover_windows`、`build_capture_exclusions` | 生成「被污染」镜像，还原后携带旧任务入口 | 代码未处理，仅设计 |
| F2 | 程序与 WinRE 同在 C: 时旧同卷判断误拦合法 probe/backup | **P1** | `windows_prepare.rs::prepare_task` | 合法同卷任务被拒，功能受限 | 代码未放宽（须与 F3 同批） |
| E1 | 独立 EFI 固件首启动持续失败 `0xc0430001` | **P1**（开发测试功能，非 V1 门槛） | `project-status.md:86-96`、`continuation-handoff.md:43` | 无法证明「独立 EFI 实际引导进恢复系统」 | 多次诊断，根因未定 |
| V1 | 断电注入 / BCDBoot 失败回滚 / BitLocker / 身份篡改的完整实机矩阵未跑全 | **P1** | `project-status.md:90-97`、`peer-comparison.md:15` | 失败路径的可靠性未被实证 | 部分代码有，实机不全 |
| F4 | Windows 构建入口与 Mac 脚本的静态 CRT 策略未统一 | **P2** | `build-win.sh:24-31`、`windows/build-windows.ps1` | 精简 Windows/WinRE 下产物可能因缺 DLL 无法启动 | Mac 脚本已修，Windows 入口待统一 |
| T1 | `native_gui.rs` 7915 行单文件、大量手写 `unsafe` P/Invoke | **P2**（工程债） | `crates/backuprestore-cli/src/native_gui.rs` | 维护/回归成本高，Windows-only 错误只在 ARM64 暴露 | 现状 |
| T2 | 测试环境依赖 Parallels 通道，`prlctl exec` 抖动需重试 | **P2**（工程债） | `build-win.sh:37-66` | 构建/部署偶发半成品状态 | 已有重试与 SHA 校验兜底 |
| T3 | `tools/`、多份按日期进度文档、历史脚本残留 | **P3**（工程债） | `tools/`、`docs/` | 新人/AI 接手时易误用历史入口 | 已有「当前基线」消歧规则 |

> 说明：F1 与 F3 是**同一批必须一起交付**的改动——单独「去掉程序同卷校验」会继续产生 F3 污染镜像（见修复方案 3.1）。

---

## 3. 分阶段开发路线图

### 阶段 0：设计与决策冻结（无代码）
- **目标**：确认 F1～F3 修复方案的两段式边界、任务 schema 版本隔离策略、以及「任务专用 WinRE 副本 + 独立一次性 BCD」技术选型。
- **产出**：评审通过 [winre-repair-implementation-plan-2026-09-25.md](winre-repair-implementation-plan-2026-09-25.md) 第 3～6 节。
- **验收**：用户确认方案；明确第一段/第二段可独立交付。
- **依赖**：无。

### 阶段 1：阻止危险组合 + 统一构建（第一段）
- **目标**：在不改变启动机制的前提下，先消除「可能删除恢复入口」和「备份污染」两类危险。
- **改动范围**：
  - 新增拒绝规则：还原目标 = 注册 WinRE 宿主卷 → 报 `RestoreTargetHostsRegisteredWinre`；
  - 备份源 = 注册 WinRE 宿主卷时，旧覆盖模式暂时拒绝（防 F3）；
  - 统一 Windows/Mac 构建的静态 CRT 策略（F4），并以最终 EXE 导入表验证无 `VCRUNTIME`/`api-ms-win-crt-*`；
  - 完善诊断与回归测试（core 纯校验函数 + 两端一致）。
- **前置条件**：阶段 0 完成；按 AGENTS.md 先建并核验新快照再触碰启动项。
- **验收标准**：
  - 危险组合在**任何 BCD/WinRE 写入前**被拒，注册 WinRE SHA-256 前后不变；
  - 两个 Cargo manifest / `VERSION` / `Cargo.lock` / CI 版本检查通过；
  - Windows ARM64 交叉 clippy 与 Release 构建通过，导入表静态。
- **风险**：F2 行为放宽**不得**在本阶段单独启用，必须与阶段 2 绑定。

### 阶段 2：任务专用 WinRE 副本启动（第二段）
- **目标**：用任务独占的 WinRE WIM+SDI 和独立一次性 BCD 对象启动，系统注册 WIM 不再被临时覆盖。
- **改动范围**（拟新增，见方案第 4～6 节）：
  - core 新增 `WinreLocation` 与 `validate_volume_roles()`；CLI 新增 `winre_location.rs`、`task_boot.rs`；
  - 任务新增 `recoveryBoot`/`registeredWinre`/`captureExclusions` 与新 schema 版本；
  - 新策略 `task-wim-one-time`，历史任务识别为 `legacy-registered-wim` 且禁止静默续跑；
  - 每任务独占 osloader/设备选项对象，不改共享 `{ramdiskoptions}`、既有 recoverysequence 与默认项。
- **前置条件**：阶段 1 完成；独立 BCD 引导方式先在隔离快照 PoC 通过。
- **验收标准**：
  - 真实 VM 完成 probe/backup/restore，且系统 WinRE 与 BCD 默认项未被改写；
  - 备份 WIM 内部**抽检确认干净**（不含当前任务入口/env/可续跑记录）；
  - 还原目标 = 原 WinRE 宿主卷的场景在独立启动 + 原恢复资产保护 + 离线重新注册均通过后才放开。
- **风险**：这是本路线图中改动最大、实机依赖最深的一段；失败需可回滚到阶段 1 状态。

### 阶段 3：断电/回滚实机验收矩阵
- **目标**：把已有的状态机与回滚代码变成可复现的实机证据。
- **改动范围**：按 `testing-plan.md` 与 `verification-matrix.md` 补全逐阶段断电注入、`BootRepaired` 前后 BCDBoot 失败回滚、BitLocker 拒绝、身份篡改拒绝的用例与脚本。
- **前置条件**：阶段 1 或阶段 2 的启动机制稳定。
- **验收标准**：每个用例在可回滚快照中完成，记录任务 ID、阶段、恢复日志与最终状态；失败用例必须给出明确停留位置与人工恢复路径。

### 阶段 4：独立 EFI 启动闭环
- **目标**：解决 `0xc0430001`，证明从独立 EFI 实际启动进入恢复系统并回归正常 Windows。
- **改动范围**：跨磁盘 UEFI / Secure Boot / 分区关联的隔离诊断；必要时调整 BCDBoot 目标与 SDI/WIM 引用。
- **前置条件**：阶段 2 的独立 BCD 对象机制可用。
- **验收标准**：隔离快照中从独立 EFI 首启动成功进入恢复系统，完成后恢复 `hdd0` 首启动顺序；`C:` 全程未作为备份源或还原目标。
- **边界**：该功能仅保留为开发测试，不作为普通 GUI/V1 发布门槛。

### 阶段 5：能力扩展（非 V1，按需评估）
- 增量/差异备份、VSS 在线一致性、镜像加密/分卷/去重、整盘分区布局备份、文件级恢复、调度/保留策略、异机驱动注入、网络/云目标。
- **前置条件**：阶段 1～3 稳定且用户明确立项。

---

## 4. 优先级矩阵与依赖关系

```
阶段0 (设计冻结)
  └─> 阶段1 (阻止危险组合 + 统一构建)  ── P0，可独立交付
        └─> 阶段2 (任务专用 WinRE + 独立BCD)  ── P0/P1，F2 放宽必须在此
              ├─> 阶段3 (断电/回滚实机矩阵)  ── P1
              └─> 阶段4 (独立EFI启动闭环)    ── P1，仅开发测试
                    └─> 阶段5 (能力扩展)     ── 非V1
```

- **P0（必须先做）**：阶段 0 → 阶段 1（F1、F3）。
- **P1（核心闭环）**：阶段 2（F2 随其放开）→ 阶段 3、阶段 4。
- **P2（工程债，可并行）**：F4 构建统一、T1 拆分 GUI、T2 测试通道加固。
- **P3（清理）**：T3 归档历史脚本与旧进度文档。

---

## 5. 建议里程碑

| 里程碑 | 内容 | 判定标准 |
|---|---|---|
| M1 | 危险组合全部被拒 | 阶段 1 验收通过，注册 WinRE 哈希不变 |
| M2 | 任务专用启动落地 | 阶段 2 验收通过，系统 WinRE/BCD 未被改写 |
| M3 | 失败路径可信 | 阶段 3 矩阵跑全，逐用例有实机证据 |
| M4 | 独立 EFI 闭环 | 阶段 4 验收通过（开发测试范围） |

---

## 6. 待用户决策项

1. 是否先只做**阶段 1**（低风险、可快速交付），还是直接推进阶段 2？
2. 独立 EFI（阶段 4）是否仍列为**开发测试功能**，不进入 V1 发布门槛？
3. 阶段 5 的能力扩展中，哪一项优先级最高（增量 / VSS / 加密 / 整盘布局）？
4. 是否允许在验证阶段按 AGENTS.md 约定创建新的 Parallels 快照并触碰启动项。

---

## 7. 不做什么（明确边界）

- 本文件不修改代码、`VERSION`、Cargo manifest、构建脚本或程序行为；
- 不把设计目标写成已实现功能，不把离线检查写成实机验收；
- 不触碰 `C:` 作为备份源或还原目标（允许读取其启动配置与 WinRE）；
- 不在无新快照保护的情况下触碰测试机启动项。