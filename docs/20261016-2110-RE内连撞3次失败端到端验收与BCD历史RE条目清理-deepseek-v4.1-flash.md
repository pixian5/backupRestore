# RE 内连撞 3 次失败端到端验收 + BCD 历史 RE 条目清理（2026-10-16）

> **本轮用户指令**：①跑「RE 内连撞 3 次失败的端到端」；②清 BCD 里历史 RE 条目；③回答「9 月的旧任务是什么」。
>
> **本轮无代码改动**，版本仍 **2.6.8**。补上 `20261016-2027` §4 明确标注为"未做"的那条硬证据，
> 并顺手清掉 `20261016-2016` 复核报告里点名的现场卫生项。

---

## 0. TL;DR

| # | 事项 | 结论 |
|---|---|---|
| 1 | **RE 内连撞 3 次失败端到端** | ✅ **通过**。构造确定性失败任务进 RE，`Recovery.exe started` ×3、`ACCEPTANCE_CHECKPOINT apply error` ×3 → 第 3 次**不再弹框**、直接 disarm + 标终态 + 重启回 Windows |
| 2 | **自动逃出后的现场** | `bootsequence` 已清、任务两个 BCD 对象已删、`H:\BackupRestoreRE` 已清；任务 `stage=failed` + 专属终态文案；**C: 哨兵在、序列号 `5C93-0047` 未变** |
| 3 | **终态任务不再被续跑** | ✅ 从 `H:\brwork` 启动 GUI（pid 10084）后 `prepare.log` **仍是 4366 行、尾部停在 `running shutdown.exe /r /t 0`，零新增** |
| 4 | **`abandon-task` 终态收口（真实任务）** | ✅ **exit 0**；`payload`/`stage`/`original`/`mount` 已删；`task.json`/`status.json`/`manifest.json`/日志/`fault-apply.json` 全留 |
| 5 | **BCD 历史 RE 条目清理** | ✅ 删 **8 条历史 RE 条目 + 8 个配对 device-options**（共 16 个对象）；`osloader` 由 10 个降到 **2 个**（`{0cec537a-…}` + `{current}`）；`reagentc /info` 仍 **Enabled** 且 BCD 标识符指向保留项 |
| 6 | **「9 月旧任务」** | ❌ **说法有误**：包目录那 2 个任务实为 **2026-10-14** 的任务（`1aa188b1`=backup/success、`1bb72a5e`=backup/capturing 非终态），详见 §6 |
| 7 | 构建/测试 | 本轮**未改代码**，未重跑 `cargo test`（沿用 v2.6.8 的 154+43 全绿） |

---

## 1. 端到端怎么构造（选型与理由）

### 1.1 为什么不能复用「历史那次失败」

`20261016-1413` 记录的 RE 内 apply 失败是「`restore-existing` + `--skip-format` 打到 C: 系统卷」，
但 **v2.6.7 已在准备期硬拒这个组合**（`pe_safety::overlay_restore_on_windows_volume_error`）——
它再也进不了 RE，所以不能拿它做端到端。

### 1.2 选型：`--test-fault acceptance:apply:error`

`recovery_fault` 是既有的显式验收钩子（配置来自任务载荷 env 的 `TEST_FAULT`），
`valid()` 白名单里有 `apply` 这个点，模式 `error`。检查点位置在 `main.rs:2802`：

```
STEP 2/4 还原镜像（时间长）
    p.wait_if_paused(log, "STEP 2/4 还原镜像");
    recovery_fault::checkpoint("apply", log)?;   // ← 注入点
    run_logged("dism.exe", ["/Apply-Image", ...])
```

**关键性质**：`error` 模式下 marker 不抑制重复触发（只有 `fault-released` 才抑制，
`mode != "error"` 的短路条件对 `error` 不成立）→ **三次尝试全部失败**，正是"连撞 3 次"需要的语义。

`--test-fault` 的取值校验（`windows_prepare.rs:484-501`）除了 6 个历史字面量，还接受
`crate::recovery_fault::valid(&fault)` 的任意合法值 → `acceptance:apply:error` 合法。

### 1.3 目标卷选 **T:**（理由与代价）

| 候选 | 判定 |
|---|---|
| **C:** | ❌ 会擦掉当前系统与哨兵；且 `c-full-20261014.wim` index 2 的 `minimumTargetSize` = 249.5 GiB，C: 恰好够，但破坏性最大 |
| **F:** | ⚠️ 350 GiB，可用，但会擦掉 PE 对照实验留在那里的那份系统 |
| **T:** | ✅ **选用**。5 GiB 数据卷，`Windows\System32\config\SYSTEM` **不存在** → `target_is_windows_volume()` = false → 走**数据卷分支**（不碰引导）；`T-drive-backup.wim`（16.7 MB，源分区 5 GiB）尺寸匹配 |

**代价（如实记录）**：T: 上原有一份 2026-10-14「数据卷 + 取消格式化」测试现场（含 `SENTINEL-skipformat.txt`），
本轮被快格清空（序列号 `30D1-A4CA` → `021A-553B`，`T:\` 现为空目录）。
原内容仍可从 `H:\BRImages\T-drive-backup.wim` 复原。

> 附带确认一条判据口径：`Get-Volume` 报 T: 的 `Size` = 5,368,705,024 B，而镜像 `minimumTargetSize` =
> 5,368,709,120 B（差 4 KiB，NTFS 备份引导扇区）。prepare **通过**了尺寸校验，证明判据用的是
> `VolumeIdentity.partition_size`（**物理分区长度**）而非 `Get-Volume` 的 Size——用后者目测会误判"差 4 KiB"。

### 1.4 实际命令

```cmd
H:\brwork\BackupRestore.exe prepare --operation restore-existing --source-drive T --target-drive T ^
  --image-path H:\BRImages\T-drive-backup.wim --wim-index 1 --allow-destructive ^
  --test-fault acceptance:apply:error
```

（`restore-existing` 要求 source == target，故两处都写 `T`；不传 `--skip-format` → 默认格式化。）

---

## 2. 执行时间线（全部实测）

| 时刻（本地 / UTC） | 事件 | 证据 |
|---|---|---|
| 20:49:30 / 12:49:30 | prepare 建 BCD 对象：device options `{06d95f19-c960-11f1-898e-e0eb03ca5aee}`、loader `{06d95f1a-c960-11f1-898e-e0eb03ca5aee}` | 任务 `prepare.log` |
| 20:49:36 / 12:49:36 | `one-shot bootsequence armed at {06d95f1a-…}` → `running shutdown.exe /r /t 0` | 同上；`bcdedit /enum {bootmgr}` 读回 `bootsequence {06d95f1a-…}` |
| 20:49:37 起 | **第 1 次**：RE 内 STEP 1/4 → rearm → `diskpart format fs=ntfs quick` → `ACCEPTANCE_CHECKPOINT apply error` → 弹框① | 截图 `e2e-01`/`e2e-04`；`Recovery.log` 中第 1 条 checkpoint 记录（在 60 行 tail 之前） |
| — | 按 `Enter`（MessageBox 默认按钮 = 「重试」）→ **第 2 次** | 截图 `e2e-05` 显示 1/4 重新开始 |
| 20:50:57 / 12:50:57 | 第 2 次 STEP 1/4 | `Recovery.log` |
| 20:51:00.629 / 12:51:00.629 | 第 2 次 `ACCEPTANCE_CHECKPOINT apply error` → 弹框②「这是第 2 次失败」 | 截图 `e2e-07`；`Recovery.log` |
| — | 再按 `Enter` → **第 3 次** | — |
| 20:51:43.5 / 12:51:43.5 | 第 3 次 `Recovery.exe started from env task=58980052…` | `Recovery.log` |
| 20:51:46.926 / 12:51:46.926 | 第 3 次 `ACCEPTANCE_CHECKPOINT apply error` | `Recovery.log` + `fault-apply.json` 时间戳 |
| 20:51:48.314 / 12:51:48.314 | `status.json` 被写成 `stage=failed` + 终态文案 | `status.json` |
| ~20:51:5x | **无第 3 次弹框**，画面转黑 → 重启回 Windows | 截图 `e2e-08`（第 3 次运行中）→ `e2e-09`（黑屏） |
| 20:53 | 桌面恢复，GUI v2.6.8 自启 | 截图 `e2e-11` |

**失败次数硬计数**（对 `H:\BRImages\Recovery.log` 与任务目录 `Recovery-early.log` 做字符串统计）：

| 计数项 | 值 |
|---|---|
| `ACCEPTANCE_CHECKPOINT apply error` | **3** |
| `Recovery.exe started from env task=58980052` | **3** |
| `fault-apply.json` 内容 | `{"at":"2026-10-16T12:51:46.923464Z","mode":"error","point":"apply"}` |

→ **"连撞 3 次"是硬计数，不是目测。**

---

## 3. 自动逃出效果核验（回到 Windows 后）

| 检查项 | 结果 |
|---|---|
| `bcdedit /enum {bootmgr}` | **无 `bootsequence`**（已 disarm）；`default {current}`、`displayorder {current}` |
| `bcdedit /enum osloader` | 只剩 `{0cec537a-…}`（reagentc 注册项）+ `{current}`；本任务的 `{06d95f1a-…}`/`{06d95f19-…}` **已删** |
| 任务 `status.json` | `"stage": "failed"`、`"error": "恢复在恢复环境中连续失败，已自动逃出并标记为终态（任务与现场保留，可人工诊断或清理）"`、`updated 2026-10-16T12:51:48.314303400Z` |
| 任务 `task.json` | `"status": "failed"` |
| `validate-task task.json` | `valid task 58980052-… operation=RestoreExisting **status=Failed**`，exit 0 |
| 载荷目录 `H:\BackupRestoreRE` | **找不到文件**（已清） |
| `vol C:` | **`5C93-0047` 未变** |
| 哨兵 `C:\BR-102220.txt` | **仍在**（13 字节，10-22 10:22 那枚） |
| `vol T:` | `30D1-A4CA` → **`021A-553B`**，`T:\` 空 → 目标确已被擦除（apply 失败发生在格式化之后，符合设计） |
| `X:\BackupRestore-escape.log` | 找不到（内存盘，重启即失）——**已知缺口**，见 §8 |
| `H:\brwork\tasks` 全部任务 | `2f8f8efb`=failed、`58980052`=failed、`6f6ebf7a`/`a531b02b`/`c6cb1fa2`=success → **无非终态任务** |

### 3.1 ★ 终态任务不再被续跑（复核报告新增验收点，本轮用**真实逃出任务**复验）

用 `tools/win-clicker/restart-gui-h.ps1` 从 `H:\brwork` 启动 GUI（此前正是这份副本会触发续跑）：

```
GUI_UP pid=10084     （Get-Process 确认 Path = H:\brwork\BackupRestore.exe）
```

任务 `prepare.log`：

| 时点 | 行数 | 尾部 |
|---|---|---|
| 启动 GUI 前 | 4366 | `[12:49:36.962] running shutdown.exe /r /t 0` |
| 启动 GUI 后 | **4366（未变）** | **同上，零新增** |

→ 任务已 `Failed`，`stage_resumable_after_interruption` 不选它 → **不再武装、不再重启**。
这条在 `20261016-2027` §3.5 用的是**手工改过 JSON 的任务**，本轮用的是**真正由自动逃出写成 Failed 的任务**，证据等级更高。

---

## 4. `abandon-task` 终态收口（真实任务实测）

```
H:\brwork\BackupRestore.exe abandon-task H:\brwork 58980052-e197-4d85-af4e-a1327821f7be
ABANDON_EXIT=0
```

| 类别 | 变化 |
|---|---|
| **大目录已删** | `payload` / `stage` / `original` / `mount` 全部消失 |
| **证据保留（16 项）** | `task.json`、`status.json`、`status.env`、`manifest.json`、`prepare.log`、`Recovery-early.log`、`Recovery-early.diskpart.log`、`WinPE-init.log`、`wpeinit.log`、`boot-entry.json`、`bcd-before-bootmgr.txt`、`bcd-before-export`、`bcd-before-raw`、`diskpart-format.txt`、**`fault-apply.json`**、`recovery.log`（本次新写） |
| `status.json` | 仍为 `stage=failed` + 终态文案（未被收口改写） |
| `validate-task` | 仍 `status=Failed`，exit 0 |
| BCD / `H:\BackupRestoreRE` / C: 哨兵 | 均无变化，保持干净 |

---

## 5. BCD 历史 RE 条目清理

### 5.1 清理前的完整判读（先证明"删哪些、为什么安全"）

用 `bcdedit /enum all /v` 全量导出（3942 行）后按对象解析：

- BCD 共 **609 个对象**：1 个 `{bootmgr}`、1 个 Windows loader（`{current}`）、**9 个 `winpe` 的 RE 对象**、
  78 个「休眠状态恢复」对象、520 个其它（UEFI 固件项/`memdiag`/`filedevice` 等）。
- **9 个 RE 对象里只有 1 个是 `reagentc` 注册项** `{0cec537a-c913-11f1-8987-86bb9001dde7}`：
  它被 `{current}` 的 `recoverysequence` 与 `{bootmgr}` 的 `resumeobject` 链引用 → **必须保留**。
- 其余 **8 个是历史条目**，逐个核对引用关系：
  - 它们的 device-options 对象**只被自己引用**（`device`/`osdevice` 各一次）→ 可随条目一起删；
  - 每个条目还被一个「休眠状态恢复」对象以 `recoverysequence` 引用——**但那些 resume 对象本身无人引用**（全量文本扫描各只出现 1 次，即自己的标识行）→ 是纯孤儿；
  - **无任何条目出现在 `displayorder`**，`{bootmgr}.bootsequence` 为空 → 删除**不影响启动**。

### 5.2 执行

先导出完整备份（可整份 `/import` 回滚）：

```
bcdedit /export C:\BackupRestoreBCD\BCD-before-clean-20261016     → 638,976 字节，EXIT=0
copy 到 \\Mac\backupRestore\.test-artifacts\BCD-before-clean-20261016
```

再逐条 `bcdedit /delete {GUID} /f`，**16 条全部 `=0`**：

| # | 历史 RE 条目（osloader） | ramdisk 指向 | 配对 device-options |
|---|---|---|---|
| 1 | `{265d7bf0-b12c-11f1-8822-f36649e3d2a6}` | `[Y:]` | `{265d7bf1-…}` |
| 2 | `{a6c31a81-c8fb-11f1-8989-ebb1e817f7db}` | `[C:]` | `{a6c31a82-…}` |
| 3 | `{af1ecd39-c518-11f1-8ac2-9a808d18a8a6}` | `[unknown]` | `{af1ecd3a-…}` |
| 4 | `{b69adf69-bbe4-11f1-88fa-ff604011526d}` | `[C:]` | `{b69adf6a-…}` |
| 5 | `{bf4adf49-c4af-11f1-a8ea-ba0bec634878}` | `[unknown]` | `{bf4adf4a-…}` |
| 6 | `{d543a50a-c7d8-11f1-b50c-f7181a302b50}` | `[F:]` | `{d543a50b-…}` |
| 7 | `{eeca24db-bb87-11f1-88f3-a820f5e16ef7}` | `[F:]` | `{eeca24dc-…}` |
| 8 | `{f53cce28-c359-11f1-a10a-a37d684423c7}` | `[unknown]` | `{f53cce29-…}` |

### 5.3 清理后核验

| 检查项 | 结果 |
|---|---|
| `bcdedit /enum osloader` | **2 个**：`{0cec537a-c913-11f1-8987-86bb9001dde7}` + `{current}` |
| `{current}` 的 `recoverysequence` | 仍指向 `{0cec537a-…}` ✅ |
| `{bootmgr}` | `default {current}`、`displayorder {current}`、**无 `bootsequence`** |
| `reagentc /info` | `Windows RE 状态: Enabled`；`引导配置数据(BCD)标识符: 0cec537a-c913-11f1-8987-86bb9001dde7`（**与保留项一致**）；WinRE 版本 `10.0.26100.8031`；`REAGENTC.EXE: 操作成功` |
| 本轮端到端产生的 `{06d95f1a-…}`/`{06d95f19-…}` | 已被 `disarm` 清掉，未污染 |

### 5.4 未清的（**如实标注，超出"RE 条目"范围**）

- **7 个孤儿「休眠状态恢复」对象**（`{41326e71-…}`/`{a8bafbad-…}`/`{ab6d9baa-…}`/`{bce8e6b2-…}`/`{d233223e-…}`/`{d4ed7e19-…}`/`{f3e612e2-…}`）本轮**未删**——
  它们不是 RE 条目，且删掉 RE 条目后它们的 `recoverysequence` 变成悬挂引用（**无害**：无人引用它们，也不在 `displayorder`）。
  我按"只做被要求的事"处理；要清可一并删（另 71 个无关联的 resume 孤儿同理）。
- BCD 整体仍有 **609 个对象**（含 520 个 `OTHER`），是长期实验累积的通用膨胀，**未动**。

---

## 6. 「9 月的旧任务」到底是什么

**结论：说法有误——`C:\Users\Public\backupRestore-package\tasks` 里那 2 个任务是 2026-10-14 的，不是 9 月的。**
（上一轮我沿用交接文档的"9 月"表述，本轮实测纠正。）

| 任务 | operation | stage | progress | `updated`（UTC → 本地） | 目录 mtime | `validate-task` |
|---|---|---|---|---|---|---|
| `1aa188b1-69d8-4b72-8fba-0efe635b9b3b` | backup | **success** | 100 | `2026-10-14T06:32:52Z` → 10-14 14:32 | 10-14 14:32 | `valid … status=Success` |
| `1bb72a5e-b837-419b-a4e5-247f5fa9bc38` | backup | **capturing**（非终态） | 20 | `2026-10-14T13:45:28Z` → 10-14 21:45 | 10-16 20:52 | `valid … status=Capturing` |

- `1aa188b1` 的 `prepare.log` 尾部：`06:23:19 one-shot bootsequence armed at {bb265245-c797-11f1-…}` → `running shutdown.exe /r /t 0`。
  一次**成功的备份**，跑的是包目录那份副本。
- `1bb72a5e` 的 `boot-entry.json`：`created 2026-10-14T13:45:06Z`，
  `wimPath = H:\BackupRestoreRE\1bb72a5e-…\Winre.wim`（**该文件已不存在**）→ 续跑时
  `rearm` 校验载荷失败，打 `Pending boot recovery abandoned: our boot entry is unusable: invalid task: 续跑载荷缺失、为空或路径不可安全访问`。

**它为什么反复刷日志**（本轮新观察到第 4 次）：`rearm` 失败走 `release_boot_resume_attempt` **释放 marker**，
所以 `boot-resume-Capturing.attempted` 不残留 → **每次包目录 GUI 启动都会再试一次**。现有 4 条：

```
[11:56:52] Detected durable interrupted task … resuming task 1bb72a5e-… stage=Capturing
[12:10:34] （同上）
[12:12:23] （同上）
[12:52:03] （同上）   ← 本轮自动逃出重启回 Windows、包目录 GUI 自启时又试了一次
```

> 注意与 `20261016-2027` §1 的说法对齐：**"每 stage 只续跑一次"指的是 claim 成功后的正常路径**；
> 这里因为 `rearm` 失败**主动释放**了 marker，所以属于"每次都试但每次都不重启"，不会造成死循环，
> 只是日志噪音 + 每次都真的去读一遍 BCD 载荷。

**真正的 9 月残留确实存在，但不在 `tasks` 里**（`C:\Users\Public` 下）：
`BR-Recheck-20260925`、`brv15`（09-14）、`winre-src`（09-25）、`BackupRestore-actual-20261005-0724`（10-05）、`BRPE-safety-212`（10-05）。

---

## 7. 本轮新踩的环境坑（都值得写进工具速查）

1. **`prlctl exec` 加不加 `--current-user` 是两种权限，别混**：
   - **不加** → `nt authority\system`：**能跑 `bcdedit`**、能用 UNC 写 `\\Mac\backupRestore\…`，但**看不到 `X:`**；
   - **加 `--current-user`** → 交互用户 `p8b6\x`，**未提权** → `bcdedit` 直接报 `无法打开引导配置数据存储。拒绝访问。`。
   → 结论：**BCD 操作用 SYSTEM 会话 + UNC 共享盘写日志**；**读写仓库文件用 `--current-user`**。
2. **`.cmd` 里不能写中文**：UTF-8 的中文字节被 cmd 按 GBK 解析，会破坏引号配对，
   **整个脚本静默不执行**（连第一行 `echo … > file` 都没跑，现象是"输出文件根本没生成"）。
   → 一切投进 VM 的 `.cmd` **一律纯 ASCII**；要匹配中文日志改用英文子串（本轮用 `Recovery retry required at TargetErased` 替代中文错误串）。
3. **RE 内唯一的交互通道**：`prlctl exec` 不可用（无 Parallels Tools），
   靠 `prlctl capture` 截图判状态 + `tools/win-clicker/vmkey.sh enter` 注入键盘。
   **实测 `Enter` 命中 MessageBox 默认按钮「重试」有效**（`MB_RETRYCANCEL` 的默认按钮就是 Retry）。
4. **`Get-Volume` 的 Size 比物理分区长度小 4 KiB**（NTFS 备份引导扇区），
   目测比对尺寸会误判"差 4 KiB 装不下"；程序判据用的是 `VolumeIdentity.partition_size`，不受影响（§1.3 已实证）。

---

## 8. 本轮未做 / 待办

| 项 | 说明 |
|---|---|
| **`X:\BackupRestore-escape.log` 在内存盘** | 自动逃出的 disarm/标终态明细只写进 WinRE 的 `X:\`（重启即失）。**持久证据只剩"效果"**（BCD 已清 + `status.json` 终态文案）。v2.6.6 起就列为待办（"日志目录改程序卷 GUID 定位"），本轮仍未做 |
| **7 个孤儿 resume 对象 + 71 个无关联 resume 孤儿 + 520 个其它 BCD 对象** | 未清，见 §5.4；需用户裁定范围 |
| **包目录 2 个旧任务** | 未清。`1bb72a5e` 是**非终态**，`abandon-task` 只对终态开放；要清得先定"这种载荷已失效的旧任务"算什么终态口径 |
| **T: 已被格式化** | 原 10-14「数据卷 + 取消格式化」测试现场没了；内容仍可从 `H:\BRImages\T-drive-backup.wim` 复原 |
| **测试任务 `58980052` 目录保留** | 已收口（大目录清空、证据留存），留作本轮硬证据；不需要可直接删目录 |
| GUI 显示"当前工作区 + 任务数" | 复核报告 §5.3 建议，仍未做 |

---

## 9. 相关文档

- 本轮要补的证据缺口：[`20261016-2027` v2.6.8 终态收口](20261016-2027-v2.6.8-终态收口与R3文案分叉修复-deepseek-v4.1-flash.md) §4
- 现场卫生项来源：[`20261016-2016` 接手复核报告](20261016-2016-接手复核报告-五条清单验证与两处结论修正-deepseek-v4.1-flash.md)
- 首次记录该失败形态：[`20261016-1413` V2 重跑阻塞根因与 RE 内 apply 失败全记录](20261016-1413-V2重跑阻塞根因与RE内apply失败全记录-deepseek-v4.1-flash.md)
- 自救能力首次落地：[`20261016-1836` v2.6.6 恢复环境自救能力](20261016-1836-v2.6.6恢复环境自救能力与exit5精确根因-deepseek-v4.1-flash.md)
- 待办权威：[`20261015-2150` 已知未修与长期守则清单](20261015-2150-已知未修与长期守则清单落库-Step-5-Preview.md)

---

## 附：本轮现场产物（`.test-artifacts/`，gitignore）

| 文件 | 内容 |
|---|---|
| `e2e-00-before.png` … `e2e-12-gui-h.png` | 端到端全程截图（含弹框①②、黑屏逃出、桌面恢复） |
| `e2e-prepare.txt` / `e2e-tasklog.txt` | prepare 输出与任务环境 |
| `e2e-verify.txt` / `e2e-count.txt` / `e2e-abandon.txt` / `e2e-resume-check.txt` | 各项核验原始输出 |
| `e2e-task-58980052/` | 收口前的任务目录整份备份 |
| `BCD-before-clean-20261016` | BCD 全量备份（638,976 B，可 `/import` 回滚） |
| `bcd-all.txt` / `bcd-inventory.txt` | BCD 全量对象导出与解析清单 |
| `bcd-clean.cmd` / `e2e-*.cmd` | 本轮 VM 侧脚本（纯 ASCII） |
