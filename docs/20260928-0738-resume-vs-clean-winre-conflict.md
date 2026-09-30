# 断电续跑 × 启动项消费 × 还原原始 RE —— 三者是否冲突

> 日期：2026-09-28
> 结论：**有一处真实冲突**（断电续跑 × 入口还原干净 WinRE），由「方案 D 入口提前」改动引入；已定位根因与修复方案。
>
> ✅ **状态：已修复（2026-09-28 晚）**。按 [20260928-0745-registered-winre-policy-and-resume-design.md](20260928-0745-registered-winre-policy-and-resume-design.md)
> 实现改动 1–4：① 回退入口翻转；② 新增 `ensure_registered_is_payload()`；③ 接入 resume 重武装之前；
> ④ 续跑被压制分支恢复干净注册位。交叉编译通过、core 28 单测全绿（**VM 实机闭环验证尚未做**）。
> 本文作为**冲突根因的历史记录**保留，其中"当前状态"描述为修复前的情况，请以实际代码为准。

---

## 一、三个机制分别是什么，动的是什么资源

先明确边界——三者操作的**不是同一个东西**，这是判断冲突的前提。

| 简称 | 机制 | 位置 | 动的资源 | 目的 |
|---|---|---|---|---|
| **A 断电续跑** | `resume_pending_boot_task()` | `main.rs:331`（Windows 侧 GUI 启动时调用，`main.rs:7842`） | 验证 artifacts → 重新武装一次性启动项（`reagentc /boottore`，`main.rs:423`）→ `shutdown /r` | 中断任务自动回到 WinRE 继续 |
| **B 启动项消费** | bootmgr 开机时自动消费一次性启动项 | 开机瞬间，**不在我们的代码里**（PE 路径才有显式 `pe_self_clean_bootsequence`，`native_gui.rs:7119+`） | BCD 中的一次性启动条目 | 保证下次重启回 Windows、不循环 |
| **C 还原原始 RE** | `restore_original_winre()` | `main.rs:1784`；人口调用 `main.rs:771`（本次新增）、结尾 `main.rs:989`、guard Drop `main.rs:1105` | 注册位文件 `R:\Recovery\WindowsRE\Winre.wim` | 保证捕获进镜像的是干净原版 WinRE |

**关键隐含依赖（全文核心）**：A 续跑能"自动继续"，靠的是**重启后 bootmgr 加载注册位那个 WIM 时，里面还带着我们的 payload**（`winpeshl` 钩子自动拉起 `Recovery.exe`）。这是**隐式的、代码里没写出来的依赖**——`resume_pending_boot_task` 自己只做 artifact 校验和重新武装，**从不检查也不部署注册位 WIM 的内容**。

---

## 二、因果链证据（为什么 A 依赖「注册位 = 注入件」）

`windows_prepare.rs` 的准备流程决定了这个依赖：

```
714  fs::copy(registered_wim → original/Winre.wim)     ← 快照干净原版
730  fs::copy(registered_wim → stage/Winre.wim)
743  inject_winre_payload()                            ← stage 注入 winpeshl + Recovery.exe
800  fs::copy(staged → registered_wim)                 ← 注册位 = 注入件（★关键★）
806  reagentc /boottore                                ← 武装一次性启动项
```

第 800 行决定了：**prepare 结束、重启之后，注册位是「注入件」**，bootmgr 加载它 → `winpeshl` 自动拉起 `Recovery.exe` → 任务自动执行。

而**微软原版（干净件）没有这个钩子**，加载它只会进入标准 WinRE 菜单（疑难解答界面），`Recovery.exe` **不会自动运行**。

---

## 三、冲突判定矩阵

| 组合 | 冲突？ | 分析 |
|---|---|---|
| **A × B** | ❌ 不冲突 | **互补设计**。任务终结（Success/Failed）→ A 不触发（`stage_resumable_after_interruption` 只认非终结 stage），B 保证回 Windows ✓；任务中断（非终结）→ A 重新武装一次再进 RE ✓。这正是「防无限重启循环 + 保留一次续跑」的既定设计（`main.rs:288` 注释）。 |
| **B × C** | ❌ 不冲突 | 动的是**不同资源**（BCD 启动条目 vs WIM 文件内容），且目标一致——都让机器回到「可正常回 Windows、WinRE 是干净原版」的状态。互不干扰。 |
| **A × C** | ✅ **冲突（真实存在）** | **A 要求注册位仍是注入件才能续跑；C 在入口就把注册位替换成干净原版 → 续跑的重启落进「微软原版 WinRE」→ 没有 winpeshl 钩子 → Recovery.exe 不自动拉起 → 任务续不起来。** |

---

## 四、A × C 冲突的具体触发链

```
1. prepare：注册位 = 注入件（stage），reagentc /boottore 武装
2. 重启 → bootmgr 消费一次性启动项 → 加载注入件 → Recovery.exe 自动运行
3. ★本次新增★ 入口立即 restore_original_winre → 注册位 = 干净原版（main.rs:771）
4. …… 会话中任何时刻断电 ……
5. 再次开机：启动项已在步骤 2 被消费 → 正常进 Windows
6. Windows 侧 GUI 检测到非终结任务 → resume_pending_boot_task 重新武装 reagentc /boottore 并重启
7. bootmgr 加载注册位 ← 但此刻注册位是【干净原版】（步骤 3 已替换）
8. → 进入标准 WinRE 菜单，Recovery.exe 不自动运行 → 续跑失败 ❌
```

**后果加重的细节**：步骤 6 一旦成功重新武装并按设计持久了 marker（`claim_boot_resume_attempt`，`main.rs:290`），**这一次的自动续跑额度就被消耗掉了**。即使失败也不会 `release`（release 只在 reagentc/shutdown 自身失败时调用，`main.rs:435/454`）。下次 Windows 启动会命中 `main.rs:388` 的压制：*"already had its one retry; manual inspection is required"* → **退化为只能人工介入**。

---

## 五、诚实更正：上一轮结论答错了

此前回答"我的改动没有影响续跑机制"——**不准确**。当时只看到"入口/结尾调用同一个函数、续跑逻辑本身没被改动"，**没有识别出"续跑隐式依赖注册位 = 注入件"这条隐藏契约**。这正是本次分析要纠偏的点。

---

## 六、旧设计同样有缺口，但窗口更小

对比「捕获前才还原」（旧）与「入口立即还原」（新）：

| 设计 | 注册位在整个会话中的状态 | 续跑可用的时段 | 续跑会失败的时段 |
|---|---|---|---|
| **旧（捕获前还原）** | 注入件 ▁▁▁…▁ 捕获前翻转 → 干净件 | 入口 → 翻转之前（大部分） | **翻转之后 → 任务终结**（含整个 DISM 捕获阶段，其实不短） |
| **新（入口还原）** | 干净件 ▔▔▔▔▔ 全程 | 几乎没有 | **几乎整个会话** |

所以：
- 旧设计**并非零风险**——只要断电落在"还原之后到任务终结"这段（DISM 捕获期间完全包含在内），续跑同样会落进干净原版而失败。这是一个**此前就存在、未被记录的缺口**。
- 本次入口改动把这个**缺口从"捕获阶段"扩大到"整个会话"**——把偶发问题变成大概率问题，属回归。

---

## 七、推荐修复：让续跑自己重建「注册位 = 注入件」

**一次改到位，同时修掉新回归和旧缺口。**

在 `resume_pending_boot_task()` 里，**在重新武装启动项之前**，补一步"确保注册位是注入件"：

1. resume 已经校验 `stage/Winre.wim` 存在（`main.rs:400`）——**素材现成**，无需新增产物；
2. 挂载 RECOVERY 卷（复用 prepare 的做法，`ensure_volume_mounted(recovery, 'R', log)`，`windows_prepare.rs:706`）；
3. 若注册位当前哈希 ≠ `stage` 哈希，则 `fs::copy(stage/Winre.wim → 注册位)` + `verify_sha256`；
4. 然后再执行既有的 `reagentc /boottore` + `shutdown /r`。

**收益**：
- 无论磁盘当时停留在什么状态（干净件 / 注入件 / 半截损坏），续跑的重启**一定落进注入件** → `Recovery.exe` 自动拉起 → 续跑成功；
- 续跑进 WinRE 后，**入口还原再清一遍**（幂等）→ 捕获进镜像的仍是干净原版。**方案 D 的目标与续跑能力两者兼得**；
- 顺带把第六节里旧设计"捕获阶段断电"的既有缺口一并堵上。

**代价**：resume 路径多一次 WIM 拷贝（数百 MB，数秒），且需要在 Windows 侧挂载 RECOVERY 卷——实现上要复用/抽出一个"确保注册 WinRE = 指定哈希"的公共函数，避免 prepare 与 resume 两处逻辑重复。

---

## 八、待办

- [ ] 实现第七节的 resume 侧 payload 重建（本次回归的修复）
- [ ] 考虑为 `restore_original_winre` 的拷贝加"临时文件 + 原子 rename"加固（避免拷贝中途断电留半截 WIM；该风险新旧设计共有，非本次回归）
- [ ] 修复后重跑：`cargo test -p backuprestore-core` + Windows 交叉编译；条件允许时在 VM 做一次"人为中断 → 观察是否自动续跑"的闭环验证
