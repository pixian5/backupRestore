# BackupRestore 文档索引

本文件是 `docs/` 下所有文档的作用说明与阅读顺序。
产品需求原文在仓库根目录：`Windows 一键系统备份还原 V1——完整开发需求.md`。

**优先级**：用户最新指令 > 源代码与已保存的实机证据 > 本文档。
静态检查或历史记录不能替代实机证据——离线条目通过不等于 Windows/WinRE 实机通过。

---

## 一、先看这三份（决定你现在该做什么）

| 文档 | 作用 |
|---|---|
| [project-status.md](project-status.md) | **当前进度与决策的总入口**：版本、用户决策、已完成/未完成项、继续条件。判断"现在该干嘛"先看它 |
| [current-status-2026-09-16.md](current-status-2026-09-16.md) | **当前执行基线**（v1.6.6 起）：产品入口、工作目录、验证边界。优先于所有按日期的进度文档 |
| [verification-matrix.md](verification-matrix.md) | **需求与证据矩阵**：每项需求的代码/离线/实机证据分别到哪一步。防止把静态检查误报成实机成功 |

---

## 二、开发流程与规范

| 文档 | 作用 |
|---|---|
| [development-execution-protocol.md](development-execution-protocol.md) | 接到开发/测试/清理任务时必须遵守的执行顺序（先建快照、先验证再部署等硬约束） |
| [continuation-handoff.md](continuation-handoff.md) | 交接说明：后续开发者/AI 的实现边界与执行顺序。不重复记录会变的进度 |
| [development-roadmap-2026-09-26.md](development-roadmap-2026-09-26.md) | 下一步开发路线图与风险排序。**只是规划，不代表已完成或已验收** |
| [testing-plan.md](testing-plan.md) | 备份/还原的分层测试方案与用例矩阵：离线、准备、WinRE 实机、故障注入 |
| [implementation-notes.md](implementation-notes.md) | 实现细节、历史问题与验证边界的补充记录 |
| [systematic-audit-2026-08-25.md](systematic-audit-2026-08-25.md) | 全项目系统性审计：架构、已修复根因、当前验证边界 |

---

## 三、构建与产物

| 文档 | 作用 |
|---|---|
| [windows-build.md](windows-build.md) | **Windows ARM64/x64 构建与产物规则**。验收固定在 Win11 ARM64 VM：改代码必须重新编译并启动 GUI 前台检查，macOS 仅离线用途 |
| [backuprestore-pe.md](backuprestore-pe.md) | `BackupRestorePE.wim` 自定义 PE 的构建记录与产物说明 |
| [compression-options-2026-09-26.md](compression-options-2026-09-26.md) | 压缩选项的收敛结论与当前产品行为边界 |

---

## 四、WinRE / 引导 / 恢复

| 文档 | 作用 |
|---|---|
| [winre-repair-implementation-plan-2026-09-25.md](winre-repair-implementation-plan-2026-09-25.md) | v1.6.8 审查后的**详细实施方案**（分阶段改动、函数清单、启动事务、故障注入、验收标准）。**仅为待实施方案，不代表已修复** |
| [winre-payload-contract-2026-09-16.md](winre-payload-contract-2026-09-16.md) | WinRE 与 WinPE 模板分离、载荷契约与回归防线（v1.6.0） |
| [winre-autostart-pitfalls.md](winre-autostart-pitfalls.md) | WinRE 恢复路线端到端跑通时踩的坑（2026-09-13 实机验证） |
| [winre-bcd-loop-fix-2026-09-16.md](winre-bcd-loop-fix-2026-09-16.md) | BCD 丢失导致引导循环的修复过程与 WinRE 引导结论 |
| [winre-boot-failure-parallels27-2026-09-25.md](winre-boot-failure-parallels27-2026-09-25.md) | **PD 27.x 固件 ramdisk 引导回归**的排查与 A/B 闭环证明（26.4.2 同链路正常进 WinRE） |
| [recovery-desktop-system-info-2026-09-25.md](recovery-desktop-system-info-2026-09-25.md) | 恢复桌面「软硬件信息」按钮（v1.7.0）的设计与实现说明 |

---

## 五、测试 VM 的操作与环境

| 文档 | 作用 |
|---|---|
| **[vm-input-control-guide.md](vm-input-control-guide.md)** | **操作 VM 鼠标键盘的操作手册（最新，日常用这个）**：四条通道选择矩阵、全部命令、坐标系换算、生效判据、故障排查 |
| [vm-click-automation-inventory.md](vm-click-automation-inventory.md) | VM 内 UI 自动化能力的**资产清单与考古记录**：有哪些脚本、怎么来的、踩过哪些坑。顶部已指向操作手册 |
| [operation-channels.md](operation-channels.md) | 早期操作通道全指南（合并豆包/Trae 经验）。**历史全量记录，部分结论已被 `vm-input-control-guide.md` 的 2026-09-26 实测更新**（如注入通道选型、DPI 坐标），冲突时以后者为准 |
| [pd26-downgrade-vm-recovery-2026-09-25.md](pd26-downgrade-vm-recovery-2026-09-25.md) | PD 27→26.4.2 降级后 VM 卡 UEFI 菜单的**修复手册**（NVRAM 重建、快照、ReAgent.xml 等） |
| [vm-boot-repair-newvm.md](vm-boot-repair-newvm.md) | VM 引导修复的「新建 VM 挂旧盘」方案（2026-09-13） |
| [cleanup-dev-residue.md](cleanup-dev-residue.md) | 开发残留清理清单与测试盘重建步骤 |

---

## 六、踩坑记录（按日期）

| 文档 | 作用 |
|---|---|
| [pitfalls-build-env-2026-09-25.md](pitfalls-build-env-2026-09-25.md) | 构建环境两个坑：镜像源与 `target` 所有权——都表现为「代码没动突然构建不了」，都不是代码问题 |
| [pitfalls-static-crt-2026-09-25.md](pitfalls-static-crt-2026-09-25.md) | 动态 CRT 导致程序在 WinRE / 精简 Windows 上无法加载（v1.6.7） |

---

## 七、历史快照（不可作为当前执行合同）

| 文档 | 记录的版本/时间 | 说明 |
|---|---|---|
| [current-progress-2026-09-09.md](current-progress-2026-09-09.md) | v1.3.3 / 09-09 | 最早基线快照 |
| [current-progress-2026-09-11.md](current-progress-2026-09-11.md) | v1.3.9 / 09-11 | 进度快照 |
| [current-progress-2026-09-13.md](current-progress-2026-09-13.md) | v1.5.10 / 09-13 | 自 v1.6.0 起被 `current-status-2026-09-16.md` 取代 |
| [current-progress-2026-09-15.md](current-progress-2026-09-15.md) | 09-15 | C: 系统卷真实备份+还原的阶段受阻记录（当时的 `RecoveryLauncher.cmd` 入口已移除） |

---

## 八、子目录 `docs/开发方案/`

按阶段拆分的开发方案包（2026-09-26 生成）：

| 文档 | 作用 |
|---|---|
| `00-开发方案总览与依赖` | 总览与阶段间依赖关系 |
| `阶段0-设计与决策冻结` | 设计冻结与决策记录 |
| `阶段1-F1-阻止还原目标覆盖注册WinRE` | F1 需求的实现方案 |
| `阶段1-F3-阻止WinRE宿主卷污染备份` | F3 需求的实现方案 |
| `阶段1-F4-统一Windows与Mac静态CRT构建` | F4：静态 CRT 构建统一（对应 `pitfalls-static-crt-2026-09-25.md`） |
| `阶段2-任务专用WinRE副本与独立BCD启动` | 阶段 2：任务专用 WinRE 副本 + 独立 BCD 启动 |
| `阶段3-断电与回滚实机验收矩阵` | 阶段 3：断电与回滚的实机验收矩阵 |
| `阶段4-独立EFI启动闭环` | 阶段 4：独立 EFI 启动闭环 |
| `阶段5-能力扩展` | 阶段 5：能力扩展规划 |
| `工程债T1-GUI单文件拆分` | 工程债：GUI 单文件拆分 |
| `工程债T2-测试通道加固` | 工程债：测试通道加固 |
| `工程债T3-历史脚本与文档归档` | 工程债：历史脚本与文档归档 |

> 文件名带时间戳后缀（`-20260926-234228`），引用时按前缀匹配即可。

---

## 九、建议阅读顺序（新接手时）

1. `project-status.md` —— 现在什么状态
2. `verification-matrix.md` —— 哪些真的验证过
3. `development-execution-protocol.md` —— 动手前要遵守什么
4. `windows-build.md` —— 怎么构建和验收
5. `vm-input-control-guide.md` —— 怎么驱动测试 VM
6. `winre-repair-implementation-plan-2026-09-25.md` —— 当前待实施的改动

需要排查具体故障时，按目录翻第六节（踩坑）和第四、五节（WinRE / VM 环境）的对应文档。
