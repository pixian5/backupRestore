# 阶段 2 拆分实施：任务专用 WinRE 副本（2026-09-27）

文档状态：**2A 原方案已被实机 PoC 证伪；路线已定 —— Capture 排除 + 事后补回重建 WinRE（第 8 节）**。
代码改动按 8.2 四条推进；先做「WinRE 内离线 reagentc 重注册」PoC（8.5），通过后才动 WinRE 通道代码。

> 边界提醒：本轮所有改动**只在 WinRE 任务通道内**。PE 通道（`install_pe_ramdisk`）不受
> F1/F3 影响，本阶段不碰，见 8.1。

前置：v1.7.6 已完成阶段 1（F1/F3 第一段拒绝 + 载荷同步）。本文档只讲阶段 2。

---

## 1. 为什么必须做阶段 2（不是可选优化）

阶段 1 把「离线备份 C:」和「还原到 C:」直接**拒绝**了。判定条件是
`源/目标卷 == 承载注册 WinRE 的卷`。

问题在于：**没有独立 Recovery 分区的机器非常常见**（本测试 VM 就是，WinRE 注册在
`harddisk0\partition4` = C:）。这类机器上：

- 产品的**主场景** = 系统盘完整备份/还原；
- 主场景必须走**离线路径**（在线 Capture 系统盘会被文件占用锁住）；
- 离线路径 = `mutates_registered_winre = true` → **被阶段 1 拒绝**。

结论：**不做阶段 2，最终验收在相当一部分真机上会当场失败**。阶段 2 不是能力提升，
是解锁主场景的必经之路。

---

## 2. 实机勘察（VM，2026-09-27）

`bcdedit /enum all /v` 中 WinRE 相关对象：

| 对象 | device / osdevice | 说明 |
|---|---|---|
| `{fa68c813-b854-11f1-88b9-da86a19ef236}` | `ramdisk=[C:]\Recovery\WindowsRE\Winre.wim,{fa68c814-…}` | **当前生效的 WinRE osloader**，`{current}` 的 recoverysequence 指向它 |
| 另两个 osloader | `ramdisk=[Y:]\Recovery\WindowsRE\Winre.wim,…` | 孤儿引用（Y: 是 840M FAT32 hidden 卷，**目录为空**），历史残留 |
| `{fa68c814-…}`（ramdiskoptions） | `ramdisksdidevice partition=C:` / `ramdisksdipath \Recovery\WindowsRE\boot.sdi` | 与 osloader 配对，改路径时要保留 |

`reagentc /info`：Windows RE 位置 `\\?\GLOBALROOT\device\harddisk0\partition4\Recovery\WindowsRE`，状态 Enabled。

**关键结论**：只要把 `{fa68c813-…}` 的 `device`/`osdevice` 从 `Winre.wim` 改成
`Winre-task.wim`（同目录下的任务副本），就能从任务副本启动，
**注册 WIM 全程不被覆盖** → F3 污染链从根上断掉。ramdiskoptions 对象不用动。

---

## 3. 阶段 2A：任务副本 + BCD 重定向 + Capture 排除（解锁 F3）

### 3.1 改动点

| # | 位置 | 改动 |
|---|---|---|
| 1 | `windows_prepare.rs::prepare_payload` | 注入后的 WIM 不再 `fs::copy` 到注册位置（删掉 790 行那次覆盖），改为写 `…\Recovery\WindowsRE\Winre-task.wim` |
| 2 | 新增 `redirect_winre_bcd()` | `reagentc /info` 取当前 recoverysequence GUID → `bcdedit /set {guid} device ramdisk=[R:]\Recovery\WindowsRE\Winre-task.wim,{opts}` 与 `osdevice` 同理 |
| 3 | 新增 `restore_winre_bcd()` | 任务结束/失败时改回注册 WIM 路径，并删除 `Winre-task.wim` |
| 4 | 恢复端（`main.rs::recover_windows` 收尾 / `restore_original_winre`） | 同上：改回 BCD → 删副本 → 校验注册 WIM 哈希 == `ORIGINAL_WINRE_SHA256` |
| 5 | `core::build_capture_exclusions` | 新增可选参数，追加 `\Recovery\WindowsRE\Winre-task.wim` 与任务目录；确保镜像里只有**原始** WIM |
| 6 | `core::validate_volume_roles` | 新策略 `task-wim-redirect` 下**放开** F3 对 `source == recovery` 的拒绝；旧任务（无策略标记）保持拒绝 |

### 3.2 为什么这样能解锁 F3

污染的唯一来源是「把注入后 WIM 覆盖回注册位置」。改成任务副本后：

- 注册 WIM 始终是原始的 → Capture 打进镜像的是原始 WIM，不含任务入口/env/可续跑记录；
- 任务副本本身进 Capture 排除表 → 镜像里连副本都没有；
- 任务结束 BCD 改回 + 副本删除 → 系统回到原状，可用哈希证伪。

### 3.3 验收标准

1. 备份全程（含 WinRE 阶段）注册 WIM 的 SHA-256 与准备前**完全一致**；
2. 产出的 WIM **挂载抽检**：内部 `\Recovery\WindowsRE\` 下无 `Winre-task.wim`，
   `Windows\System32` 下无 `RecoveryTask.env` / `task.json`；
3. 任务结束后 `bcdedit /enum` 显示 ramdisk 路径回到 `Winre.wim`，且 `Winre-task.wim` 已删除；
4. **离线备份 C:（小样本或测试卷）不再被拒** —— 这是解锁的直接判据；
5. `cargo test -p backuprestore-core` 全绿，新增 2A 相关单测。

### 3.4 风险与兜底

- **改共享 osloader**：若任务中途断电，WinRE 会指向一个残留副本。兜底：恢复端每次启动都校验
  BCD 现状，发现残留就改回；另有任务目录里的 `bcd-before-export` 与原始 BCD 字节快照可 import。
- **bcdedit 拒绝改写**：PoC 阶段验证；若失败则退到 2B 的独立对象方案。
- **改启动项前必须建快照**（AGENTS.md 硬约束）。

---

## 4. 阶段 2B：独立一次性 BCD 对象 + 还原后重建（解锁 F1）

2A 解决备份污染；2A **不解决** F1 —— 还原目标 = WinRE 宿主卷时，格式化依然会删掉恢复资产。

2B 范围：

1. 每任务 **新建独立 osloader + 独立 ramdiskoptions 对象**（`bcdedit /create /application osloader`），
   不改共享 `{ramdiskoptions}`、既有 recoverysequence 与默认项；
2. 任务结束删除自建对象；
3. 还原到 WinRE 宿主卷时：先把原始 WIM 与 boot.sdi 搬出到安全位置（任务目录），
   格式化/Apply 之后放回，离线 `reagentc /setreimage` + `/enable` 重新注册；
4. 全部通过后才放开 F1 的 `target == recovery` 拒绝。

前置：2A 完成且实机稳定。

---

## 5. 回滚

| 层级 | 动作 |
|---|---|
| 代码 | 回退到 `73214c2`（v1.7.6）即回到阶段 1 状态 |
| 载荷 | 只需保证版本一致，2A 不改载荷内容 |
| VM | 改 BCD 前的 Parallels 快照（见下节记录） |
| 运行时 | 任务结束时自动改回 BCD + 删副本；失败时走已有的 BCD 字节快照 import |

---

## 6. 待办与决策记录

- [x] 用户选定「阶段 2 拆分做」（2A 先，2B 后）——后被 2A PoC 推翻
- [x] 建快照（改 BCD 前）：`{eb423a36-f670-4d59-9a2c-c5a517db8c6b}`
- [x] **PoC 完成：2A 原方案被证伪**（见第 7 节）
- [x] **路线已定：Capture 排除 + 事后补回重建 WinRE**（见 8.0~8.2，用户 2026-09-27 质疑后收敛）
- [ ] PoC：WinRE 内对已 Apply 的目标卷做离线 `reagentc /setreimage /target` + `/enable`
- [ ] 决定是否启用 sidecar `<镜像>.winre.wim`（多 ~700MB/份）
- [ ] 实施 8.2 四条改动 + 单测
- [ ] 测试盘验证 + 根目录 README 进度更新

---

## 7. PoC 实测记录（2026-09-27，VM 内 4 次重启 + 对照实验）

环境：Win11 26100（ARM64 VM），WinRE 注册在 `C:\Recovery\WindowsRE`，生效 osloader
`{fa68c813-…}`（recoverysequence），配套 ramdiskoptions `{fa68c814-…}`。

| # | 方案 | 结果 |
|---|---|---|
| 对照 | 注册位置原样 + `reagentc /boottore` + `bcdedit /bootsequence` | ✅ **进 WinRE**（蓝屏恢复菜单，实测截图） |
| 1 | 共享 osloader 的 `device/osdevice` 改指 `Winre-task.wim` + bootsequence | ❌ 重启后直接回 Windows |
| 2 | 先在注册位置设 bootstatus，再改路径（bootstatus 独立于路径假设） | ❌ 同上 |
| 3 | `bcdedit /copy` 克隆 osloader（继承全部元素）→ 克隆指任务副本 + bootsequence | ❌ 同上 |
| 4 | `reagentc /setreimage /path <任务目录>` + `/enable`（官方通道） | ❌ `/enable` 无视任务目录，重建 recoverysequence 指回注册位置 |
| 5 | `recoverysequence` 本身指向克隆（自洽链）+ bootsequence | ❌ 同上 |

### 7.1 硬结论

1. **WinRE 强校验「启动 ramdisk 路径 == ReAgent 注册位置」**，与 BCD 对象身份无关
   （克隆自己的对象、甚至把它放进 recoverysequence 都没用）。`ReAgent.log` 佐证：
   `BCD recovery entry points to invalid location (no winre.wim at C:\Recovery\WindowsRE\Winre-task.wim)`
   —— 它按注册位置的**文件名**核对。
2. `reagentc /boottore` 在路径不一致时返回 RC=2（"Windows RE 已禁用"），此时 WinRE
   即使被 bootsequence 拉起也会因 bootstatus 缺失**静默自退**回 Windows。
3. 官方 `/setreimage` 是给离线部署用的；在线场景 `/enable` 会重新扫描注册位置并重建条目。
4. **BCD 层没有绕过方案**。任务环境要不碰注册 WIM，只有两条真路：
   **换 WinPE**（没有 ReAgent 校验），或 **swap-in-place**（见 8.1）。

### 7.2 意外重大发现：Windows servicing 静默冲掉部署的载荷

当天上午部署的 v1.7.6 载荷（`5df96301…`，714,409,997B）在数小时内被 Windows 累积更新
替换为**官方原版 WinRE**（`709,810,082B`，内部无 `Recovery.exe`，`winpeshl.ini` 回到
官方 `recenv.exe`；文件 mtime 保留 servicing 组件时间 2026-09-24）。

已重新注入恢复（官方底版 + v1.7.6 `Recovery.exe`/`winpeshl.ini`，新哈希 `1060a552…`，
写回前后校验一致；恢复前快照 `{efef3f19-f79a-489d-9b2e-cecc5dcac483}`）。

**产品含义**：注册 WinRE 不是可靠的载荷宿主——任何部署都可能被 servicing 冲掉。
任务准备阶段必须校验载荷哈希（已有 `ORIGINAL_WINRE_SHA256` 链）并在不符时重部署；
这也进一步支持「任务环境独立于注册 WinRE」的方向。

---

## 8. 最终方案（2026-09-27 用户裁定）：Capture 排除 + 事后重建 WinRE

### 8.0 先回答两个被质疑的问题

**Q：老流程为什么要「把注入后的 WIM 覆盖回注册位置」？直接排除那个文件不就行了？**

覆盖不是为备份，是**为启动**。今天下午的 PoC（第 7 节）已经用实机证明：
WinRE 启动时按 **ReAgent 注册位置的路径 + 文件名** 校验ramdisk 源，不一致就判
`invalid location` 并自退回 Windows。四条 BCD 重定向路线全被拒，官方 `/setreimage`
也不行。**任务环境唯一能启动的位置就是注册位置**，所以「注入后的 WIM」必须坐进
`C:\Recovery\WindowsRE\Winre.wim`，没有第二个选择。

**排除能否解决污染？能，而且它就是 F3 的正解。** 两者解决的是不同问题，不互相替代：

| 动作 | 解决什么 | 不做会怎样 |
|---|---|---|
| 注入副本写回注册位置 | **能启动**（WinRE 强约束，见 7.1，不可省） | 根本进不了任务环境 |
| Capture 排除 `\Recovery\WindowsRE\Winre.wim` | **F3 污染**（用户提的这条） | 镜像里带一份注入后的脏 WIM + 上次任务的 env/task.json |
| 任务结束后换回原件 + 校验哈希 | 注册 WIM 持久干净 | 系统长期带着副本跑 |

唯一的例外：**如果任务环境换成 PE，第一条约束就不存在**（见 8.1）。但只要任务跑在 WinRE 里，
「注入副本必须坐进注册位置」就不可省，跟备不备份无关。

**Q：F1（格式化删掉 WinRE 宿主目录）不是同理吗？**

同理。**F1 与 F3 收敛成同一个收尾动作：任务结束 / 还原完成后补回 WinRE 并重新注册。**
排除掉 WIM 之后，镜像里本来就没有恢复资产；格式化删掉的东西与「补回」要写的东西完全重叠——
都只是 `\Recovery\WindowsRE\` 那一个目录。所以一处修复解两个 P0：

```
还原到 C:：Format → Apply（镜像里本来就没有 \Recovery\WindowsRE）
            ↓ 收尾：把暂存的原件 WIM 写回 <目标卷>\Recovery\WindowsRE\Winre.wim
            ↓ 离线 reagentc /setreimage /target <目标卷>\Windows + /enable
结果：目标系统有正常可用的 RE（还是任务前的原件，哈希可验）
```

### 8.1 RE 与 PE 必须严格区分（不要混为一谈）

**这两个 P0 只存在于 WinRE 通道，PE 通道天生没有。** 代码事实：

| | WinRE 任务通道 | PE 通道（`install_pe_ramdisk`） |
|---|---|---|
| 入口 | `prepare` → `prepare_payload` → `reagentc /boottore` | GUI「PE 恢复」页手动安装启动项 |
| 存放位置 | **固定** `X:\Recovery\WindowsRE\Winre.wim`（注册位置） | **任意** `<dir>\sources\boot.wim`（用户选的目录） |
| 谁在校验路径 | ReAgent（BootUX/winpeshl 启动前按注册位置核对） | 无，只有 BCD ramdisk 设备/路径本身 |
| 需要覆盖系统文件 | 是（注册 WIM） | 否（只复制到用户指定目录 + 加一条 BCD 项） |
| 会不会被 servicing 冲掉 | **会**（今天实测，7.2） | 不会（不在 servicing 管辖区内） |
| F1 / F3 | **存在** | **不存在** |

结论：**阶段 2 的所有改动必须限定在 WinRE 通道内**，不得顺手改 PE 的逻辑、也不得把 PE
的启动链当成「任务环境已可用」。GUI 里那条 PE 启动链目前只是「安装一个手动 PE 恢复环境」
的功能，**没有任何任务走它**——`recover-env` / `prepare` / `recover` 全都在 WinRE 里跑。

### 8.2 实施方案（四条改动）

| # | 位置 | 改动 |
|---|---|---|
| 1 | `core::build_capture_exclusions` | 追加固定排除项 `\Recovery\WindowsRE\Winre.wim`（WinRE 任务模式下）。此时注册位置放的是任务副本，本来就不该进镜像 |
| 2 | `windows_prepare.rs::prepare_payload` | 保持现在的「注入副本写回注册位置」，但日志与 manifest 明确标注这是**临时顶班**；`original/Winre.wim` 已存在，继续作为换回源与哈希基准 |
| 3 | 恢复端收尾（新增 `redeploy_winre_after_task`） | 备份任务结束：把 `original/Winre.wim` 换回注册位置并校验 == `ORIGINAL_WINRE_SHA256`。还原任务结束：把同一份原件写到**目标卷** `\Recovery\WindowsRE\Winre.wim`，再离线 `reagentc /setreimage /target <OS根>` + `/enable` |
| 4 | `core::validate_volume_roles` | 新策略标记下**放开** F3（`source == recovery`）与 F1（`target == recovery`）的拒绝；旧任务无标记则维持拒绝 |

### 8.3 「补回 WinRE」的资产来源（必须解决，否则排除反而有害）

排除了就没有退路，所以**补回的资产来源**必须保证。三条候选，推荐 A+B 组合：

- **A. 任务目录里的 `original/Winre.wim`**（准备期哈希留底）。
  可行性已被现有规则保证：`windows_prepare.rs:164-172` 已禁止「还原目标 == 程序目录所在分区」，
  任务目录一定活过目标卷格式化。**这是主来源。**
- **B. 镜像旁挂 sidecar `<镜像名>.winre.wim`**（可选开关，默认开）。
  覆盖「拿旧镜像/别的机器的镜像还原」场景——那时任务目录里没有原件。
  代价：每份备份多 ~700MB，做成可开关（默认开，空间紧张时可关）。
- **C. 目标卷还原后仍未恢复 WinRE 时明确告警**（兜底，不允许静默失败）。

三者都做才好：A 快、B 让镜像自包含、C 保证不会「还原完安静地没有恢复环境」。

### 8.4 残留风险（诚实清单）

1. **离线 `reagentc /setreimage /target` + `/enable` 在 WinRE 里能否成功**：未 PoC，必须先验证
   （WinRE 里跑 reagentc 作用于**离线目标卷**是另一套行为）。失败则退到手工 `bcdedit` 建项。
2. **servicing 仍会冲掉注册的 WIM**（7.2）。所以准备期的哈希校验不能去，并且基准必须取
   「本次任务开始时留底的 `original/Winre.wim` 哈希」，不能假设本地存在一份权威原件。
3. **中途断电**：注册位置仍是任务副本 → 系统依旧能正常进 WinRE（不是砖），但下次任务开始前
   应先检测并换回。
4. **与微软惯例相悖**：官方建议系统备份包含 `\Recovery\WindowsRE`。我们排除它是有理由的偏离
   （任务副本绝不能被捕获），条件是自动补回 + 显著日志，**必须写进用户文档而不是悄悄改**。

### 8.5 待确认 / 下一步

- [x] 用户裁定「排除 + 事后重建」（替代 swap-in-place / 换 PE / 维持现状三选一）
- [ ] PoC：WinRE 内对已 Apply 的目标卷做离线 `reagentc /setreimage /target` + `/enable`
- [ ] 是否启用 sidecar（多 ~700MB/份镜像）
- [ ] 实施 8.2 四条改动 + 单测
- [ ] 测试盘验证：离线备份承载 RE 的卷（用不同于生产系统的小卷模拟）+ 抽检镜像无 `\Recovery` +
      任务后注册/目标 WIM 哈希复原
