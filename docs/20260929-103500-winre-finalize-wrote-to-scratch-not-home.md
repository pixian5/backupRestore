# v1.7.10 修复：「日志宣称注册位已还原，实机 `C:\Recovery\WindowsRE` 却是空目录」

> 状态：**已修复并实机验证通过（代码 + 单测 + Windows ARM64 构建 + 测试卷闭环）**。
> 证据：`.test-artifacts/winre-finalize-v10710/evidence.md`（本文 §5/§7）。
> 关联交接单：上任 AI 交接文档第二章「未决 bug」。基线：`49606d0 / v1.7.9`。

## 1. 症状

任务日志写着 `Original registered WinRE restored and verified`（main.rs:2020 附近的
`restore_original_winre`），但实机 `reagentc /info` 显示注册在
`harddisk0\partition4\Recovery\WindowsRE`，而那个目录**是空的**，`Winre.wim` 不存在，
`C:\Windows\System32\Recovery` 里也没有备份副本 → 机器**进不了 WinRE**。
复现过两次（`e1691c31` 还原任务也在内），所以不是某张盘的偶发问题，而是逻辑 bug。

干净基线（微软原版 `Winre.wim`）：**712,111,529 字节 / SHA-256 前缀 `1060a552`**。

## 2. 根因：终态写回写到了「RE 暂存卷」，而注册位本身没被写

v2「WinRE 迁出」（[20260928-233000-winre-hosted-restore-solution.md](20260928-233000-winre-hosted-restore-solution.md) §2.1）
在 prepare 阶段做了三件事，其中第二、三件埋下了这个洞：

1. payload 部署到 **RE 暂存卷**（默认=镜像卷，`--re-scratch-drive` 可改选）；
2. `reagentc /disable` → `/setreimage /path <暂存卷>:\Recovery\WindowsRE` → `/enable`
   —— **这一步会把家卷上的 `Winre.wim` 移走**（Windows RE 的镜像文件被搬去新位置）；
3. env 的 `RECOVERY_*` 被改写指向**暂存卷**，并标记 `WINRE_EVACUATED=1`
   （`prepare_payload`：`let env_recovery = scratch.unwrap_or(recovery)`）。

而终态收尾（main.rs:1006）是这么写的：

```rust
let cleanup = restore_original_winre(&values, &task_dir, &log, recovery_letter);
```

`recovery_letter` 来自 `mount_env_volume(..., "RECOVERY", 'R', ...)`——在迁出任务里
**它就是暂存卷**。于是「写回干净原件 + 校验哈希 + 打印成功」全部发生在暂存卷上；
真正承载注册位的家卷（C:）自 `reagentc /disable` 之后**没有任何人再往里写文件**，
只留下一个空目录。日志说成功也没错，只是写的不是你以为的那个位置。

### 2.1 实机证据

任务 `fc314c4e` 的 `payload\RecoveryTask.env`：

```text
WINRE_EVACUATED=1
RECOVERY_DISK_NUMBER=2          # 镜像/暂存卷
RECOVERY_PARTITION_NUMBER=2
RECOVERY_VOLUME_GUID=\\?\Volume{f0753766-30a4-410e-944f-38d139113634}\
TARGET_PARTITION_NUMBER=4       # 还原目标 = C:，真正的注册位
```

即：`RECOVERY_*`（终态写回的落点）指向 disk2/part2，而注册位在 disk0/part4。

`F:\Recovery.log` 全文检索：**只有 7 条 `Original registered WinRE restored and verified`，
没有任何一条含 `evacuat` 或 `finalize`** —— 说明本应把注册搬回家、回收暂存卷的
`finalize_evacuated_winre`（旧 main.rs:2193）**一次都没跑过**。

### 2.2 第二个叠加 bug：`finalize_evacuated_winre` 被 `finalize_success` 挡死

`recover_windows(..., finalize_success)` 的两个调用方：

| 调用方 | `finalize_success` | 说明 |
|---|---|---|
| main.rs:986 `recover_env`（真实 WinRE 入口，`winpeshl` 自动拉起） | **`false`** | 产品实际走的路径 |
| main.rs:679 `recover`（CLI 手工续跑） | `true` | 只有手工才会走 |

而 `finalize_evacuated_winre` 的两个调用点都被 `if finalize_success { ... }` 包着
（旧 main.rs:2769 备份分支 / 2927 还原分支）→ **真实 WinRE 路径永远是死代码**。
即使第一个 bug 修好了，没人调用它也一样回不去。

### 2.3 第三个 bug：PE 内根本做不了 reagentc 重注册

`finalize_evacuated_winre` 内部用「离线目标自带的全路径 reagentc」重注册，但在 PE 里：

- `reagentc /disable` 对离线目标 → **rc=50（不支持）**；
- `/setreimage` 在目标仍为 Enabled 时 → **rc=183**；
- WinRE 内**没有裸 `reagentc.exe`**（rc=9009），必须用 `<目标卷>:\Windows\System32\reagentc.exe`。

→ 结论：**注册重搬回家必须回到正常 Windows 做**，WinRE 内只能做文件级写回。

## 3. 修复（v1.7.10）

核心思路：把「注册位当前在哪」和「注册位原本的家在哪」彻底分开记账，并给它们不同阶段的职责。

| 阶段 | 谁负责 | 做什么 |
|---|---|---|
| prepare（正常 Windows） | 写 `WINRE_HOME_*` | 记下迁出前的家卷身份（= 备份源/还原目标卷），与 `RECOVERY_*` 解耦 |
| WinRE 终态（Success 之前） | `finalize_winre_after_task` + `finalize_evacuated_winre` | 干净原件写回**家卷**并校验哈希；落「待回家」标记；**不动**暂存卷载荷 |
| 桌面（下次启动 GUI） | `finish_pending_winre_rehome` | `reagentc /disable`→`/setreimage /path <家卷>\Recovery\WindowsRE`→`/enable`→`/info` 复核位置→回收暂存卷→删标记 |

### 3.1 具体改动

1. **`windows_prepare.rs`：`write_recovery_env` 新增 `winre_home` 实参**，
   写入 `WINRE_HOME_*`（复用 `insert_identity`，键齐全：`_VOLUME_GUID/_DISK_GUID/_PARTITION_GUID/
   _DISK_NUMBER/_PARTITION_NUMBER/_PARTITION_OFFSET/_PARTITION_SIZE/_PARTITION_TYPE_GUID/
   _FILESYSTEM/_VOLUME_SERIAL`）。调用处传 `recovery`（迁出前的注册卷）——**在 DISM 注入之前
   烘焙进 WIM**，因为 WinRE 读的是 WIM 里那份 env，事后改文件无效（2026-09-29 实机踩坑）。
2. **`main.rs` 新增 `restore_original_winre_at(values, task_dir, log, letter, slot_label)`**：
   父目录不存在则 `create_dir_all`（迁出态备份抓到的 C: 的 `\Recovery\WindowsRE` 本来就可能
   是空目录/不存在，旧代码在这里直接报错退出），哈希已一致则跳过拷贝（幂等），
   日志里**明确写出落点卷与角色**，杜绝「不知道写到了哪」。
3. **`main.rs` 新增 `finalize_winre_after_task(...)`** 取代 main.rs:1006 的裸调用：
   非迁出任务行为与旧版逐字一致；迁出任务→先写标记、再写家卷、写错卷/哈希不符即硬失败
   （任务判负，绝不谎报成功）、暂存卷载荷保持不动（Success 落盘前掉电还得靠它续跑）。
4. **`finalize_evacuated_winre(..., finalize_success)`**：改为写回家卷 + 落标记；
   `finalize_success` 时把暂存卷上的注册 WIM 换成干净原件（`sanitize_scratch_winre`：
   先写 `Winre.wim.clean` 临时档→校验→同卷 rename，任一失败保留原文件，绝不留半截 WIM）。
5. **两处调用点去掉 `if finalize_success` 包裹，并把收尾挪到 `write_transition(Success)` 之前**
   —— 「先写终态再动手」的铁律不变，但**磁盘动作完成后注册位是坏的，任务就不算成功**。
6. **`main.rs` 新增 `finish_pending_winre_rehome` / `finish_pending_winre_rehomes`**，
   挂在 `resume_pending_boot_task()`（GUI 启动入口 native_gui.rs:7842）：
   裸 `reagentc` 序列失败时自动退回「离线 `/target`」序列；用 `reagentc /info` 的
   `harddiskN\partitionM` **复核注册位置确实落在家卷**，复核不过就不动暂存卷；
   任何一步失败都保留标记，下次启动自动重试，绝不制造「两家都空」的死局。
7. **`try_restore_original_winre_from_task`（放弃续跑路径）改走身份解析版**
   `restore_original_winre_for_task`：旧版按 `RECOVERY_*` 找卷，在迁出任务里同样写错位置；
   `WinreRestoreGuard::drop` 一并改用它（守卫不再持有盘符，身份自己会解析）。
8. **`text_parsing.rs` 新增 `reagentc_info_location()`**（纯函数 + 单测）：
   `/info` 的状态/标签是本地化的，但位置**值**永远是 `harddiskN\partitionM` 设备路径，
   因此只解析这个，不依赖任何中文/英文标签。

### 3.2 已知限制（不阻塞，写在这里备忘）

- 桌面收尾只挂在 GUI 启动路径（`resume_pending_boot_task` 的唯一调用方）。**只用 CLI 启动
  不会触发**，需要时在 GUI 启动一次即可；标记会一直留着直到成功。
- 家卷挂载校验对还原任务放宽了 `_VOLUME_SERIAL`（家卷=刚格式化套镜像的目标卷，序列号必变），
  其余 GUID/偏移/大小/文件系统仍逐项强校验。

## 4. 影响面

- **非迁出任务**（旧任务/普通还原/普通备份）：`finalize_winre_after_task` 直接走旧分支，
  `finalize_evacuated_winre` 见 `WINRE_EVACUATED != 1` 立即返回，行为零变化；
  唯一可见差异是日志行多了 `(registered volume X:)` 后缀。
- **迁出任务**：`Winre.wim` 会被写回家卷（以前不会）→ C: 的 `\Recovery\WindowsRE`
  终态恢复为干净原版，与设计文档 §2.3 步骤 1 一致。
- env 新增 `WINRE_HOME_*` 共 10 行；`manifest.json` 的 `recovery_task_env_sha256`
  在 prepare 内计算，前后仍自洽（`WINRE_HOME_*` 由 `write_recovery_env` 一次性写入，
  与 `ORIGINAL_WINRE_SHA256`/`WINRE_EVACUATED` 的 append 顺序不变）。

## 5. 验证（截至 v1.7.10，全部通过）

- `cargo test --workspace`：**73 passed / 0 failed**（core 28 + cli 45，新增
  `reagentc_info_location_ignores_localized_labels` 覆盖中英文 `/info`、Disabled、空输出、
  残缺输出四类输入）。
- `./build-win.sh`（aarch64-pc-windows-msvc）：**通过**，产物 1,763,840 B，
  SHA-256 `9077e5e5030c276012f616c1167d6aff22f83fcba4c3dac6730b430fd864a2bd`
  （仅 LNK4099 缺 PDB 警告）；部署后 VM 内 `H:\brwork\BackupRestore.exe` 哈希一致。
- **测试卷实机闭环 PASS**（详见 §7）：迁出任务的家卷注册位终态哈希 == `ORIGINAL_WINRE_SHA256`；
  桌面重注册把 `/info` 位置搬回家卷、回收暂存卷、删标记；幂等；非迁出任务行为无变化。
- 快照：`{e74b0f10-e929-4366-965c-ef2b59973891}`（改注册位/载荷前由 `tools/vm-snapshot.sh` 创建并核验）。

## 6. 顺带发现（需用户裁定的三点）

1. **备份方向的迁出闸门目前是死代码**：`prepare` 的 `evacuation_required` 判据是
   「`validate_volume_roles` 返回 `BackupSourceOnRegisteredWinre`」，而方案 D
   （`PLAN_D_RESTORE_CLEAN_WINRE_BEFORE_CAPTURE=true`）让备份方向**根本不返回这个冲突**
   → 备份永远走「非迁出」路径。这与
   [20260928-233000-winre-hosted-restore-solution.md](20260928-233000-winre-hosted-restore-solution.md) §6.2
   「备份同样走迁出、C: 注册位全程干净」的描述**不一致**；实际是「备份照旧把载荷注入注册位，
   靠捕获前翻回干净原件保证镜像干净」（= 方案 D 的原始设计）。
   本次实测的任务 `af2b11cd` 证实了这一点：`WINRE_EVACUATED` 未写入，`RECOVERY_*`=源卷。
   **要不要让备份也走迁出**（改判据：备份源 == 注册卷就迁出，与 Plan D 开关解耦）请用户裁定；
   本次不改，只记录。
2. **迁出态备份会把 C: 抓成「`\Recovery\WindowsRE` 空目录 + `ReAgent.xml` 指向暂存卷」的镜像**
   （注册位迁走后源卷上只剩空目录）。这样的镜像还原出来的系统，WinRE 注册指向一个
   已不存在的卷 → 需靠终态重注册兜底。是否在终态一并校验/改写 `ReAgent.xml`，请用户裁定。
3. **镜像卷是 `F:\`（BRIMG 375G），不是历史文档记的 `E:\brimg`**；`P:\Recovery\WindowsRE\t.txt`
   是更早的测试残留（非本机用户数据，但也没动它）。

> 另：本次开始时 VM 内 `C:\Recovery\WindowsRE\Winre.wim` 已是干净原版
> （712,111,529 B / `1060a552…`，实测 WIM 内无 `Recovery.exe`/`winpeshl.ini`/`RecoveryTask.env`，
> 即**未被注入**），因此不必额外修复「机器没有 Winre.wim」；收尾时已把注册搬回 C: 并复核。

## 7. 测试卷实机验证过程与结果（已跑完，全通过）

按顺序执行如下（快照 `{e74b0f10…}` 已在动手前建好并核验）：

0. **修好可启动底座**：`reagentc /disable` → 把干净 `Winre.wim`（712,111,529 B /
   `1060a552…`）放回 `C:\Recovery\WindowsRE` → `/setreimage /path C:\Recovery\WindowsRE`
   → `/enable` → `/info` 复核 Enabled 且位置=`harddisk0\partition4`。
1. **把注册临时搬到测试卷 P:**（复现交接单给的配方）：
   `reagentc /disable` → `/setreimage /path P:\Recovery\WindowsRE` → `/enable`；
   `P:\Recovery\WindowsRE\Winre.wim` 的哈希记下来做基准。
2. 对 **P: 作还原目标**跑 `restore-existing`（应命中 `RestoreTargetOnRegisteredWinre` →
   走迁出）。核对：
   - prepare.log 有 `WinRE evacuated to scratch volume`；env 同时有 `WINRE_EVACUATED=1`
     **和 `WINRE_HOME_PARTITION_NUMBER=<P 的分区号>`**（修复前只有前者）；
   - WinRE 终态日志出现 `... restored and verified (WinRE home volume H:)` 与
     `desktop rehome marker written`；
   - **P:\Recovery\WindowsRE\Winre.wim 的 SHA-256 == 任务 `ORIGINAL_WINRE_SHA256`**
     （修复前这里是空目录，这就是 bug 本身）；
   - 回桌面后 `winre-rehome.log` 出现 `WinRE rehome complete`，
     `reagentc /info` 位置回到 P: 所在分区，`<暂存卷>:\Recovery\WindowsRE` 被清掉，
     `winre-rehome.pending` 标记消失。
3. **备份方向**：同样把注册搬到 P:（P: 既可作源也可作目标），跑 `backup` 命中
   `BackupSourceOnRegisteredWinre`，重复核对 2 的各项。
4. **搬回 C:**（收尾必须做，否则机器没有 WinRE）：
   `reagentc /disable` → `/setreimage /path C:\Recovery\WindowsRE` → `/enable` → `/info` 复核。
5. 幂等性抽验：再跑一次 `winre-rehome`，应返回 `WINRE_REHOME_PENDING=0` 且无副作用。

**实际结果**：还原任务 `5fb37148` 命中 F1 并迁出；终态
`Original registered WinRE restored and verified (WinRE home volume E:)`
（E: 即 WinRE 里的 P:），`P:\Recovery\WindowsRE\Winre.wim` = 712,111,529 B / `1060a552…`；
桌面 `winre-rehome` 后 `/info` 位置回到 `harddisk1\partition2`、`F:\Recovery` 被回收、
标记删除、再跑一次无副作用。备份任务 `af2b11cd` 走非迁出路径，行为与旧版一致
（日志仅多一个落点标注）。全程未把 C: 当程序的备份/还原源或目标。
