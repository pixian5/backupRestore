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
| [winre-payload-and-p0-fix-2026-09-27.md](winre-payload-and-p0-fix-2026-09-27.md) | **v1.7.6**：WinRE 载荷从 v1.7.3 更新到 v1.7.6（测试盘重建 → 快照授权写回 → WinRE 实跑验证），F1/F3 两个 P0 的第一段拒绝（变更清单、测试盘验证、最终验收检查项、回滚方案、VM 提权执行通道经验） |
| [winre-task-wim-phase2-2026-09-27.md](winre-task-wim-phase2-2026-09-27.md) | **阶段 2（路线已定，链路已验证）**：2A 原方案被证伪（WinRE 强校验 ramdisk 路径==注册位置，四路全部拒绝）；servicing 会静默冲掉载荷；**结论 = 注入副本必须坐进注册位置（为启动）+ Capture 排除/换回干净原件（为干净）+ 任务后离线重注册**；第 9 节为 WinRE 内 PoC：**WinRE 里没有 reagentc.exe，必须调用目标 OS 自带的 `%TARGET%\Windows\System32\reagentc.exe` 做 `/setreimage` + `/enable /osguid`**，`/disable` 在 PE 不支持、`bcdboot` 不会建 recoverysequence；含 RE/PE 严格区分、落地配方、bcdboot 事故教训、新候选方案 D |
| [winre-payload-and-p0-recon-2026-09-27.md](winre-payload-and-p0-recon-2026-09-27.md) | 上一条的**勘察报告**：载荷现状盘点、F1/F3 触发条件与根因、Q1~Q7 待确认项（只读，未改任何代码/载荷/启动项） |
| [winre-repair-implementation-plan-2026-09-25.md](winre-repair-implementation-plan-2026-09-25.md) | v1.6.8 审查后的**详细实施方案**（分阶段改动、函数清单、启动事务、故障注入、验收标准）。**仅为待实施方案，不代表已修复** |
| [winre-payload-contract-2026-09-16.md](winre-payload-contract-2026-09-16.md) | WinRE 与 WinPE 模板分离、载荷契约与回归防线（v1.6.0） |
| [winre-autostart-pitfalls.md](winre-autostart-pitfalls.md) | WinRE 恢复路线端到端跑通时踩的坑（2026-09-13 实机验证） |
| [winre-bcd-loop-fix-2026-09-16.md](winre-bcd-loop-fix-2026-09-16.md) | BCD 丢失导致引导循环的修复过程与 WinRE 引导结论 |
| [winre-boot-failure-parallels27-2026-09-25.md](winre-boot-failure-parallels27-2026-09-25.md) | **PD 27.x 固件 ramdisk 引导回归**的排查与 A/B 闭环证明（26.4.2 同链路正常进 WinRE） |
| [recovery-desktop-system-info-2026-09-25.md](recovery-desktop-system-info-2026-09-25.md) | 恢复桌面「软硬件信息」按钮（v1.7.0）的设计与实现说明 |
| [20260929-050206-winre-progress-window-freeze-fix.md](20260929-050206-winre-progress-window-freeze-fix.md) | **进度窗口冻住/不显示百分比**（用户进 WinRE 备份时弹窗无详情）：根因=`stream_to_log` 用 `read_until(b'\n')`，而 DISM 进度用 `\r` 原地刷新不带 `\n`，中间百分比全卡管道缓冲直到结束才落盘；修复=按 `\r` 实时落盘 + 备份加 `STEP n/N` 编号步骤并显示「步骤 n/N：名称」；含回归单测 |
| [20260929-093132-test-on-scratch-volumes-before-c.md](20260929-093132-test-on-scratch-volumes-before-c.md) | ⚠️ **测试纪律（用户 2026-09-29 确立）**：改完功能**先在测试盘跑通**再谈真实 C:，只有用户点名才动 C:；记录连续 7 次直测 C: 的代价（WinRE 变 Disabled/注册位空/暂存卷被清的修复三步）、**把注册临时搬到 P: 即可在测试盘复现 F1/F3 迁出路径**的命令、以及待复现 bug（WinRE 挂载全成功后卡在 boot-requested、无 recovery.log） |

---

## 五、测试 VM 的操作与环境

| 文档 | 作用 |
|---|---|
| **[vm-input-control-guide.md](vm-input-control-guide.md)** | **操作 VM 的操作手册（最新，日常用这个）**：① 鼠标键盘**注入**——四条通道选择矩阵、全部命令、坐标系换算、生效判据、故障排查；② **提权命令通道**——`prlctl exec` 不加 `--current-user` 即 SYSTEM+管理员，DISM/reagentc/改 `C:\Recovery` 直接干，不用 runas 桥接 |
| [vm-click-automation-inventory.md](vm-click-automation-inventory.md) | VM 内 UI 自动化能力的**资产清单与考古记录**：有哪些脚本、怎么来的、踩过哪些坑。顶部已指向操作手册 |
| [operation-channels.md](operation-channels.md) | 早期操作通道全指南（合并豆包/Trae 经验）。**历史全量记录，部分结论已被 `vm-input-control-guide.md` 的 2026-09-26 实测更新**（如注入通道选型、DPI 坐标），冲突时以后者为准 |
| [pd26-downgrade-vm-recovery-2026-09-25.md](pd26-downgrade-vm-recovery-2026-09-25.md) | PD 27→26.4.2 降级后 VM 卡 UEFI 菜单的**修复手册**（NVRAM 重建、快照、ReAgent.xml 等） |
| [vm-boot-repair-newvm.md](vm-boot-repair-newvm.md) | VM 引导修复的「新建 VM 挂旧盘」方案（2026-09-13） |
| [cleanup-dev-residue.md](cleanup-dev-residue.md) | 开发残留清理清单与测试盘重建步骤 |
| [20260928-065800-parallels-shared-folder-and-deploy.md](20260928-065800-parallels-shared-folder-and-deploy.md) | **宿主↔VM 共享盘根因**：`prl_fs` 虚拟通道只在有登录会话时建立；`net view \\Mac` 1702 是假警报；盘符会话级须用 UNC；`build-win.sh --deploy` 选 B（UNC）并端到端验证 |
| [20260928-072014-winre-restore-clean-at-entry.md](20260928-072014-winre-restore-clean-at-entry.md) | ⚠️ **已回退（历史）**：曾把方案 D 还原挪到 WinRE 入口；因与断电续跑冲突而撤销。含"清理一次性启动项"实为 bootmgr 消费 + guard 机制的纠正，仍有参考价值 |
| [20260928-073809-resume-vs-clean-winre-conflict.md](20260928-073809-resume-vs-clean-winre-conflict.md) | ✅ **冲突分析（已修复）**：入口还原干净 RE 与断电续跑真实冲突——**续跑隐式依赖「注册位=注入件」**才能自动跑 Recovery.exe；含冲突矩阵、旧设计对比（该缺口在旧设计中同样存在，只是窗口更小）、修复方案（resume 侧重建 payload） |
| [20260928-074516-registered-winre-policy-and-resume-design.md](20260928-074516-registered-winre-policy-and-resume-design.md) | **注册 WinRE 状态机设计（现行）**：注册位单位置双角色（载荷宿主 vs 捕获纯净）只能时间分片；三条不变量 + 七阶段流程 + 13 种场景矩阵 + 幂等/防循环论证；结论=回退入口翻转、改为「续跑前重建载荷」。**改动 1–4 已实现**（`ensure_registered_is_payload` 接入 resume） |
| [20260928-085000-v178-vm-verify-partial-and-bug.md](20260928-085000-v178-vm-verify-partial-and-bug.md) | **v1.7.8 VM 实机验证（部分通过 + 真实 BUG）**：常规备份周期 2 个完整任务通过；但**续跑修复路径实机失败**——`volume has no disk number`（resume 修复路径复用了 `ensure_volume_mounted` 的 disk_number 强制语义，本机 WinRE 注册卷无独立盘号）；含复现、定位、修复方向、遗留清单 |
| [20260928-093132-v179-fix-resume-mount-and-verify.md](20260928-093132-v179-fix-resume-mount-and-verify.md) | ✅ **v1.7.9：续跑挂载 BUG 修复 + 实机闭环 PASS**——根因=读取端丢弃 env 已有字段 + 挂载端单路依赖 DiskPart；修复=读取端补齐 + 挂载端三级降级（盘符复用→mountvol GUID→DiskPart）；同一任务日志前后对比（00:58 失败 → 01:21 修复→01:22 success）；含可复用验证方法（power-loss-window 注入 + original 覆写构造死局） |
| [20260928-221500-parallels-balloon-host-disk-full.md](20260928-221500-parallels-balloon-host-disk-full.md) | 🚨 **Parallels 气球文件（`Mac disk`）撑爆宿主盘事故**：客体每卷 190-220GB 巨型占位文件、删不掉会自动重建；根因=宿主演化正反馈（宿主越满→Parallels 越想回收→气球越大）；`--online-compact off` **无效**；**停 Tools 的瞬间气球自动消失**是唯一可利用窗口；含 SYSTEM 计划任务编排套路 + 本次处置（删 target 8.7G + 快照 54G → 宿主 393MB→136GB） |
| [20260928-231000-cleanup-test-volumes-and-disks.md](20260928-231000-cleanup-test-volumes-and-disks.md) | 🧹 测试卷/旧备份清理：删 br.hdd(E:)物理盘+3 个残留快照（device-del 被快照引用挡停机也报错的坑）、格式化 P:/T:、删 10 个旧任务目录；保留 F:c-real.wim 真实备份；含 rb3 真实还原被「目标分区承载 WinRE」防护拦截的记录（待设计迁出→还原→重建注册流程） |
| [20260928-233000-winre-hosted-restore-solution.md](20260928-233000-winre-hosted-restore-solution.md) | 💡 **还原目标承载 WinRE 的解决方案（v2 已实现·待 VM 验收）**：根因=续跑启动源(Winre.wim 注册位)落在被格式化的 C: 上，掉电后续跑进不了 WinRE；解法=准备层把 payload WinRE 迁出到 RE 暂存卷(默认镜像卷、`--re-scratch-drive` 可改选)并 `reagentc /enable` 指向它→格式化 C: 安全→finalize 时写回干净原件并回收暂存卷；含备份场景澄清(§6.2)、不变量、代码对接点(已落地：windows_prepare.rs evacuate 触发 / main.rs evacuate_registered_winre / winre_role_conflict_at_execution 放宽 / finalize_evacuated_winre)、验证法 |
| [20260929-103500-winre-finalize-wrote-to-scratch-not-home.md](20260929-103500-winre-finalize-wrote-to-scratch-not-home.md) | 🐛✅ **v1.7.10 修复并实机验证通过「日志宣称注册位已还原、实机 `C:\Recovery\WindowsRE` 却是空目录」**：根因三连——① 迁出任务 env 的 `RECOVERY_*` 指向 RE 暂存卷，终态把干净原件写到了暂存卷、家卷(C:)自 `reagentc /disable` 后没人再写；② `finalize_evacuated_winre` 两个调用点被 `finalize_success` 包住，而真实 WinRE 入口该参数恒为 false → 死代码；③ PE 内做不了 reagentc 重注册(`/disable` rc=50、Enabled 目标 `/setreimage` rc=183、无裸 reagentc)。修复=新增 `WINRE_HOME_*` 家卷身份 + WinRE 内写回家卷并落「待回家」标记 + 桌面 `reagentc` 重注册并用 `/info` 的 `harddiskN\partitionM` 复核 + 复活被 if 挡死的 finalize（且挪到写 Success 之前）；含实机证据与测试卷闭环结果（家卷终态哈希=1060a552…、桌面重注册+暂存卷回收、幂等）；另记录「备份方向迁出闸门是死代码（Plan D 压制 F3 冲突）」等三点待用户裁定 |
| [20260929-130000-pe-channel-poc-winre-wim-boots-from-image-volume.md](20260929-130000-pe-channel-poc-winre-wim-boots-from-image-volume.md) | 🧪 **PoC 通过：「把原 RE 副本注入后放镜像卷 + 自建 BCD 条目启动」可行**——实机进 PE 并拉起载荷（`SYSTEMROOT=X:\windows`、`Recovery.exe` RC=0），C: 注册位与 BCD 零改动。**修正旧结论**：拦住绕法的不是「ramdisk 路径 == 注册位置」，而是「ReAgent 登记的那个对象 + `reagentc /boottore` 的 bootstatus」；因此迁出/回家/Plan D/F1/F3 那整套机制在新通道里可全部去掉。含 bcdedit 七个坑（`ramdisk=[…],{opts}` 无结尾 `]`、`{ramdiskoptions}` 是 Setup 对象、`/enum all` 不列它、PE 盘符重排等）与五项待验证（断电续跑/BCDBoot 影响/Secure Boot/目标系统 WinRE 语义/纯 PE 形态） |

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
