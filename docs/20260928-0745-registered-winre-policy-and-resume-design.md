# 注册 WinRE 状态机设计：不重启循环 + 断电续跑 + 幂等

> 日期：2026-09-28
> 目标：给出一套**同时满足三个要求**的完整流程，并明确解释为何必须放弃"入口立即还原"这一步。
> 相关：[20260928-0738-resume-vs-clean-winre-conflict.md](20260928-0738-resume-vs-clean-winre-conflict.md)（冲突定位）
>
> **状态：改动 1–4 已实现并提交**（详见第九节）。校验：Windows 交叉编译通过（仅 LNK4099 缺 PDB 警告），
> `cargo test -p backuprestore-core` 28 全绿。**VM 实机闭环验证尚未做**，按第八节建议执行。

---

## 一、四个必须同时满足的目标

| 编号 | 目标 | 含义 |
|---|---|---|
| **G1** 不循环重启 | 任务完成或中断后，机器回到 **Windows**，不会反复掉回 WinRE |
| **G2** 断电续跑 | WinRE 会话中任意时刻断电 → 回 Windows → **自动重新进 RE 继续任务** |
| **G3 幂等** | 任何步骤（含重复执行、中断重来）都不产生副作用或半状态 |
| **G4 镜像纯净** | 当被备份的源卷**自身承载注册 WinRE** 时，捕获进镜像的必须是**干净原版**，不是我们的注入件 |

---

## 二、结构性约束：为什么不能"两全其美"地并存

注册 WinRE 只有**一个位置**（`R:\Recovery\WindowsRE\Winre.wim`），但它被要求扮演**两个互斥角色**：

| 角色 | 要求注册位是 | 被谁要求 |
|---|---|---|
| **P 载荷宿主** | **注入件**（含 `winpeshl` → 自动拉起 `Recovery.exe`） | G2 断电续跑：重启后必须自动跑起来 |
| **C 捕获纯净** | **干净原版**（微软原始 WinRE） | G4 镜像纯净：DISM 读文件的那一刻必须干净 |

**不能靠"把载荷放别处、留一个干净的注册位"绕开**。此前实机已证伪四条绕法（详见 `docs/20260927-2015-winre-task-wim-phase2.md` 第 7 节）：
- 共用 osloader 改路径、`bcdedit /copy` 克隆对象、`recoverysequence` 自洽、在线 `reagentc /setreimage` → **全部失败**；
- 根因：**WinRE 强校验「启动 ramdisk 路径 == ReAgent 注册位置」，且按注册位置的**文件名**核对**（`ReAgent.log`: `no winre.wim at …Winre-task.wim`）；
- 唯一可行组合就是「**注册位置原样 + `reagentc /boottore`**」。

**结论**：载荷只能住在注册位置 → P 与 C 只能**时间分片**复用同一个文件。设计的核心就是**在什么时刻把它翻成哪一面**。

---

## 三、推荐策略：三条不变量 + 时间分片

> **不变量 A（可启动性）**：只要任务尚未终结、且有可能需要自动重进 WinRE，**注册位必须是注入件**。
> **不变量 B（捕获纯净）**：DISM 捕获**期间及之前瞬间**，注册位必须是干净原版（且这一步**失败必须硬失败**，不允许带着载荷去捕获）。
> **不变量 C（终态非侵入）**：任务到达终结态（Success / Failed）时，注册位必须是干净原版。
>
> 三者按时间分片互斥 A 与 B/C；C 是任务对机器的最终承诺。

### 关键推论：应当**回退**"入口立即还原"

上一版把还原挪到 WinRE 入口（本仓 `fcb48a1`），会让**整个会话**注册位都是干净原版 → 违反不变量 A → 断电后续跑的重启落进原版 WinRE、任务起不来（详见冲突文档）。

而**"入口就干净"换来的额外安全性其实是错觉**：G4 只在 **DISM 读文件的那一瞬间**要求干净。只要在捕获前翻转并保持到捕获结束，纯净性与"入口就干净"完全等价——它不多提供任何保证，却白白牺牲了 G2。

所以推荐：**保留"捕获前翻转"（权威纯净闸门），同时新增"续跑时重建载荷"**，让 G1/G2/G3/G4 同时成立。

---

## 四、详细流程（按阶段）

### 阶段 1 · 准备（Windows 桌面）

```
1-1  校验卷角色（F1–F4，F3 在方案 D 下放行）
1-2  快照当前注册 WIM → task_dir/original/Winre.wim，记 ORIGINAL_WINRE_SHA256
1-3  构造 stage/Winre.wim = 原版 + 注入载荷（Recovery.exe / winpeshl.ini / RecoveryTask.env / task.json）
1-4  写齐产物 + manifest.json（含 staged_winre_sha256、original_winre_sha256）→ 持久化
1-5  ★仅在全部产物持久化之后★  fs::copy(stage → 注册位) + verify
     → 注册位 = 注入件（不变量 A）
1-6  reagentc /boottore（一次性）+ 写 Stage::BootRequested + status.env → 武装完成
1-7  1-5/1-6 任一步失败 → 回滚：registered ← original（+verify）+ 回滚启动请求，机器保持原样
```

> 1-5 必须在最后：现有代码已这么做（`windows_prepare.rs:786-805` 注释 + `restore_registered` 闭包）。

### 阶段 2 · 首次进入 WinRE

```
2-1  bootmgr 消费一次性启动项 → 加载注册位（=注入件）→ winpeshl 自动拉起 Recovery.exe
2-2  ★G1 的根源在这里★：一次性条目在开机时被消费，此后没有任何指向 WinRE 的持久启动项，
     默认启动项始终是 Windows → 无论后面成功、失败、还是被中断，重启都回 Windows
```

### 阶段 3 · WinRE 会话开始

```
3-1  挂载卷（RECOVERY→R:、image、source、workspace）
3-2  建 WinreRestoreGuard（保证退出时必干净，无论成功失败）
3-3  校验运行中的 Recovery.exe 哈希 == manifest.recovery_sha256
3-4  校验 stage/Winre.wim 哈希 == manifest.staged_winre_sha256（防产物被换）
3-5  ★注册位保持注入件★（不变量 A）——这里不做翻转
     理由：任务活着期间随时可能需要重进 WinRE（G2）
3-6  按 Stage 决定从哪继续（resume 语义）
```

### 阶段 4 · 捕获（仅 Backup，且源卷承载注册 WinRE）

```
4-1  if operation==Backup && capture_source_hosts_registered_winre(source, recovery):
         → restore_original_winre()：registered ← original，verify_sha256 == ORIGINAL_WINRE_SHA256
         → 失败即硬失败终止任务（绝不带着载荷去捕获）
4-2  执行 DISM 捕获源卷 → 镜像。整个过程注册位保持干净原版（不变量 B ✔）
4-3  捕获完成后，注册位的取值对镜像纯净已不再有影响（内容已写进 WIM）
```

### 阶段 5 · 收尾并返回 Windows

```
5-1  写终结状态（Success）+ status.env
5-2  WinreRestoreGuard::drop（或显式调用）→ registered ← original + verify
     → 不变量 C ✔（幂等：第 4-1 步已翻过，这里再翻是同一内容）
5-3  重启 → 回到 Windows（G1 ✔）
```

> ⚠️ 注意职责边界：**第 4-1 步是唯一的镜像纯净闸门**；5-2 的 guard 只负责"机器终态干净"，它**不能**救已被污染的镜像（那时候捕获早结束了）。所以 4-1 必须硬失败，不能依赖 guard。

### 阶段 6 · 中断（断电）续跑 —— 本次新增的关键一步

```
6-1  断电 → 开机 → 注册位处于中断瞬间的状态（注入件 / 干净原版 / 半截 都有可能）
     但由于一次性启动项已在 2-1 被消费 → 正常进 Windows（G1 ✔，不会循环）
6-2  Windows 侧 GUI 启动 → resume_pending_boot_task()：
     a. 找到唯一的非终结可续跑任务（stage_resumable_after_interruption）
     b. claim_boot_resume_attempt() 领取重试额度（防无限续跑）
     c. ★新增★ ensure_registered_is_payload()：
          · 挂载 RECOVERY 卷 → 定位注册位
          · 先 verify stage/Winre.wim == manifest.staged_winre_sha256（防部署坏载荷）
          · if sha256(注册位) != manifest.staged_winre_sha256：
                fs::copy(stage/Winre.wim → 注册位) + verify_sha256
          · 已是注入件则跳过（幂等）
        → 这一步同时修复了：干净原版态、半截损坏态、文件缺失态
     d. reagentc /boottore 重新武装（一次性）
     e. shutdown /r
6-3  2-1 重演：bootmgr 消费 → 加载注入件 → Recovery.exe 自动运行 → 回到阶段 3
     → G2 ✔，且重进后 4-1 会再次翻转干净，纯净性依然成立（G4 ✔，G3 幂等 ✔）
```

### 阶段 7 · 同一 Stage 二次中断 / 续跑放弃

```
7-1  6-2b 的 marker 已存在 → 自动续跑被压制（"already had its one retry"），转人工
     这是刻意的防无限循环上限
7-2  ★新增★ 在这一分支里同时把注册位恢复为干净原版：
     既然已放弃自动续跑，就不该把机器留着自己的载荷（不变量 C 的兜底）
7-3  清理/保留任务目录供人工检查，日志明确写清原因与建议动作
```

---

## 五、全场景矩阵

| # | 场景 | 注册位（中断/该时刻状态） | 行为 | 结果 |
|---|---|---|---|---|
| 1 | 正常备份，源卷承载 WinRE | 注入件 → 4-1 翻干净 → 捕获 → 5-2 干净 | 一次成功 | ✅ 镜像干净、机器干净、回 Windows |
| 2 | 正常备份，WinRE 在**别的**卷 | 全程注入件，不翻 | 4-1 闸门不命中（源卷不承载 WinRE） | ✅ 无需翻转，天然干净 |
| 3 | 还原任务（Restore） | 全程注入件 | 无捕获，不翻；5-2 收尾翻干净 | ✅ G4 不适用 |
| 4 | 阶段 3 中途断电 | 注入件 | 6-2c 判定已是载荷→跳过，直接重武装 | ✅ 续跑成功 |
| 5 | 阶段 4-1 **翻转过程中**断电 | **半截文件** | 6-2c 哈希不符→重拷并校验 → 修复 | ✅ 续跑成功（顺带解决了一个此前存在的缺口） |
| 6 | 阶段 4-2 **捕获中**断电 | 干净原版 | 6-2c 重部署载荷 → 重进 → 4-1 再翻干净 | ✅ 续跑成功且重试仍干净 |
| 7 | 阶段 5 收尾断电 | 干净原版 | 6-2c 重部署→重进→跑完→5-2 干净 | ✅ |
| 8 | 6-2d 重武装失败 | 已是注入件 | 现有逻辑 release marker + 返回 Err | ✅ 额度被退还，下次可再试 |
| 9 | 6-2c 部署失败（磁盘错误等） | 未知 | release marker + Err；机器可能载荷在位 | ⚠️ 需人工；靠 7-2 或下次尝试收敛 |
| 10 | 同一 Stage 二次中断 | — | 7-1 压制 + 7-2 恢复干净 | ✅ 不循环，转人工且机器干净 |
| 11 | 用户无待办任务时手动 F3 进 WinRE | 干净原版 | 标准微软 WinRE 菜单 | ✅ 非侵入，符合预期 |
| 12 | 有待办任务时手动进 WinRE | 注入件 | 自动拉起 Recovery.exe 继续任务 | ✅ 可接受/合理 |
| 13 | prepare 阶段 1-5 后、1-6 前失败 | 注入件 | 1-7 回滚 registered ← original | ✅ 机器保持原样 |

---

## 六、幂等性与防循环论证

**幂等（G3）**
- `restore_original_winre`：无条件 copy 同一内容 + 校验同一哈希 → 重跑只覆写相同字节。
- `ensure_registered_is_payload`：先比较哈希，**已是目标态则跳过**；不是则 copy+verify → 重跑收敛。
- 重进 RE 后重跑 4-1：已干净则覆写相同字节；仍幂等。
- 阶段 3 的整体重入：依赖既有 `Stage` 续跑语义（不可续跑的 stage 会明确报错而非盲跑）。

**防循环（G1）**
- 结构性：进入 WinRE 的**唯一途径**是 `reagentc /boottore` 这种**一次性**启动请求，开机即被 bootmgr 消费；不存在指向 WinRE 的持久启动项，默认启动项始终是 Windows。
- 兜底性：续跑有**每 Stage 一次**的额度上限（`claim_boot_resume_attempt`），即使交接反复失败也不会无限重启——这是刻意设计（见 `main.rs:288` 注释）。
- 本设计**不引入任何新的持久启动项**，不破坏上述性质。

---

## 七、需要的代码改动

| 序 | 改动 | 位置 | 说明 |
|---|---|---|---|
| 1 | **回退**入口立即还原 | 撤销 `fcb48a1` 在 `main.rs:759-780` 新增的入口 `restore_original_winre` 调用 | 违反不变量 A；保留 watch-dog 无益且有损 G2 |
| 2 | **新增** `ensure_registered_is_payload()` | 可复用 `main.rs` near `restore_original_winre` | 挂载 RECOVERY → 比对 `manifest.staged_winre_sha256` → 必要时 `stage→注册位` + verify |
| 3 | 在 `resume_pending_boot_task()` 中调用它 | `main.rs:415` 之前（现有 artifacts 校验之后、`reagentc /boottore` 之前） | 素材现成：`stage/Winre.wim` 与 `manifest.json` 已在 line 398/400 校验存在 |
| 4 | 在"续跑被压制"分支补恢复干净 | `main.rs:384-393` 分支内 | 释放前把 `registered ← original`，避免机器带着载荷被搁置 |
| 5 | （可选加固）注册位写入改**临时文件 + 原子 rename** | `restore_original_winre` / `ensure_registered_is_payload` | 消灭"拷贝中途留下半截"的可能，尽管 6-2c 已能自愈 |
| 6 | （可选）hidden commands 增加 `--ensure-payload-registered` 探针 | `main.rs` 命令表 | 便于在真实机器上单独验证该步骤 |

---

## 八、验证计划

**离线（必做）**
- `cargo test -p backuprestore-core`（当前基线 28 passed）
- Windows 交叉编译通过
- 为 `ensure_registered_is_payload` 的三种分支补单测：已是载荷/需部署/产物哈希不符（需把判定逻辑下沉到 `core` 或保持纯函数便于测试）

**VM 实机（建议）**
1. 准备一个备份任务，在 WinRE 会话**早期**（阶段 3）强制断电 → 观察是否自动续跑成功；
2. 在**捕获阶段**（阶段 4-2）强制断电 → 验证续跑成功且最终镜像内 Winre.wim 哈希 == `ORIGINAL_WINRE_SHA256`；
3. 人为把注册位改成错误内容（模拟半截）→ 触发续跑 → 验证被修复；
4. 连续两次在同一 Stage 中断 → 验证进入人工分支且机器 WinRE 被恢复干净；
5. 回归：完整正常备份周期仍能产出干净镜像 + 收尾 `reagentc /info` 显示 Enabled 且哈希为原版。

---

## 九、待办

- [ ] 与用户确认：是否按第三节回退入口翻转（`fcb48a1` 的入口段）
- [ ] 实现 `ensure_registered_is_payload()` 并接入 resume
- [ ] 实现续跑压制分支的"恢复干净"
- [ ] 补单测 + 交叉编译
- [ ] 按第八节跑 VM 实机验证
- [ ] 验证通过后更新 `docs/20260927-2015-winre-task-wim-phase2.md` 与本仓相关文档的状态
