# WinRE/PE 载荷更新与 F1/F3 修复：勘察结论与待确认项（2026-09-27）

文档状态：**勘察报告，尚未修改任何产品代码、载荷或启动项。**
本轮全程只读盘点 + 静态代码审计，未重启 VM、未修改 BCD/WinRE 注册、未创建/删除快照、
未执行任何备份还原。

## 0. 用户长期约束（2026-09-27 确立，优先于本文所有方案）

> 开发、构建与测试一律在**测试盘**上进行，不要对 **C 盘**做备份还原操作（体量太大）；
> 只有在全部开发完成后，才在正式环境做一次最终验收。

这条约束直接影响本文的载荷更新路径，见第 4 节待确认项 Q1/Q7。

---

## 1. WinRE/PE 载荷现状（2026-09-27 提权实测）

### 1.1 系统注册 WinRE

来源：`reagentc /info`（提权）、`Get-FileHash`、`dism /Get-ImageInfo`、`dism /Mount-Image /ReadOnly`。

| 项 | 实测值 |
|---|---|
| WinRE 状态 | Enabled |
| 注册位置 | `\\?\GLOBALROOT\device\harddisk0\partition4\Recovery\WindowsRE`（= C:，普通 NTFS Boot 分区，**不是** GPT Recovery 分区） |
| BCD identifier | `{fa68c813-b854-11f1-88b9-da86a19ef236}` |
| Windows RE Version | 10.0.26100.8031 |
| WIM 文件路径 | `C:\Recovery\WindowsRE\Winre.wim` |
| WIM 大小 / SHA-256 | 712,108,371 B / `dbbce2bd54f889fa6af6cace74174d7564823fd5eeb85f310dcea3cda5fe765c` |
| WIM mtime | 2026-09-26 23:02:00 |
| 镜像索引 1 | `Microsoft Windows Recovery Environment (arm64)`，展开大小 3,231,973,223 B |
| **内部 `Windows\System32\Recovery.exe`** | **1,687,040 B / `653abc68ae2234bfd3f68a4941b1c439c935334389bcc8d127a85d5cedcef98e` / mtime 2026-09-26 08:58:08** |
| 内部 `Windows\System32\winpeshl.ini` | 99 B / `743f193a…50a5` / mtime 2026-09-26 23:01:49 |
| 内部残留污染标记 | **无**（`INNER_NO_RECOVERY_DIR`，未见 `\Recovery` 任务目录） |

版本比对表：

| 版本 | BackupRestore.exe 大小 | SHA-256 前缀 |
|---|---|---|
| **v1.7.5（当前活动包）** | 1,689,088 | `f8b77d0e…e184` |
| v1.7.4 | 1,688,064 | `c0dddfd9…1acf` |
| **v1.7.3（= WinRE 载荷内部）** | **1,687,040** | **`653abc68…98e`** |

**结论：WinRE 载荷落后两个版本（v1.7.3 vs 当前 v1.7.5）。一旦走 WinRE 恢复链路，实际执行的不是当前代码。**

### 1.2 BCD 中的历史残留（重要，非本次引入）

`bcdedit /enum all /v` 提取到的 WinRE 相关引用：

| 对象 | 引用目标 | 状态 |
|---|---|---|
| `{fa68c813}`（reagentc 报告的 identifier） | — | 生效中 |
| `{fa68c814}` 设备选项 | `ramdisk=[C:]\Recovery\WindowsRE\Winre.wim`，SDI `partition=C:\Recovery\WindowsRE\boot.sdi` | 生效中 |
| `{265d7bf1}` 设备选项 | `ramdisk=[Y:]\Recovery\WindowsRE\Winre.wim` | **孤儿**：`Y:\Recovery\WindowsRE\` 为空目录 |
| `{e6aed4d5}` | `ramdisk=[Y:]\Recovery\WindowsRE\Winre.wim`，SDI `partition=Y:` | **孤儿**：同上 |
| `{原有}` | `ramdisksdipath \Windows\System32\Recovery\boot.sdi`，device unknown | 孤儿 |

即 **BCD 里存在 3 条指向已失效 Y: 卷 Winre.wim 的遗留引用**（源自 2026-09-25 的 WinRE 重建实验）。
它们目前不影响启动（recoverysequence 指向 `{fa68c813}`），但任何 BCD 清理 / 重新注册动作都必须
先确认所有权，**不得顺手全局删除**。

### 1.3 PE 载荷

| 项 | 实测值 |
|---|---|
| 位置 | `T:\petest\boot.wim`（2026-09-13 遗留实验副本） |
| 大小 / mtime | 367,395,379 B / 2026-09-13 12:19:53 |
| 内部 `Windows\System32\BackupRestore.exe` | 1,026,560 B / `22e6829b0da9d8a2b73432746bf5461eb1ac004d7acb6c09d02e2391a0438734` / 2026-08-25 23:38:13 |
| 内部 `Windows\System32\Recovery.exe` | 同上（同一哈希） |
| 宿主侧正式 PE 产物 | `artifacts/BackupRestorePE.iso`(382M)、`artifacts/BackupRestorePE.wim`(350M)、`artifacts/BaseWinPE.iso`(381M) |

**T:\petest\boot.wim 不是正式产物**，是陈旧实验残留；其内部二进制连 pineline 版本号都对不上
已知列表。是否把它当作需要同步更新的正式 PE 载荷，必须用户裁定（见 Q2）。

### 1.4 磁盘布局（`diskpart list disk / list volume`）

```text
Disk 0   256 GB  GPT         Disk 1  256 GB  GPT         Disk 2  256 GB  GPT
  Volume 1  C:            NTFS    249 GB  Boot
  Volume 2  T:  BRTEST174 NTFS   5120 MB   ← v1.7.5 实机验证用测试卷，4.4 GB 可用
  Volume 3      (无盘符)   NTFS    300 MB  Hidden
  Volume 4      (无盘符)   FAT32   300 MB  System  ← EFI
  Volume 5  Y:  WINRE      FAT32   840 MB  Hidden
  Volume 6  E:  br         NTFS    255 GB  132.9 GB 可用
  Volume 7  P:  BRSource   NTFS     64 GB  6.3 GB 可用
  Volume 8  H:  BRImages   NTFS    191 GB  132.9 GB 可用
  Volume 0  D:  BR         CDFS  1638 KB  CD-ROM
```

**C: 是普通 Boot 分区且承载注册 WinRE（无独立 Recovery 分区）** —— 这正是 F1 的最坏场景，
也是本项目测试 VM 的默认布局。

---

## 2. F1：还原目标可能销毁当前 WinRE 入口

### 2.1 触发条件（精确）

同时满足：

1. 操作为 `restore-existing` 或 `create-secondary`（且带 `--allow-destructive`）；
2. 还原目标卷 = 注册 WinRE 宿主卷（当前布局下即 **C: partition4**）；
3. 执行进入格式化 / Apply 阶段（`main.rs::format_target_partition`）。

当前 VM 布局使这个组合**随时可被选中**：C: 既是承载 WinRE 的卷，也是用户最可能还原的目标。

### 2.2 根因（代码级）

`crates/backuprestore-cli/src/windows_prepare.rs`：

```rust
479:  let recovery = recovery_identity()?;
480:  let efi = efi_identity(options.efi_drive)?;
481:  if workspace.same_partition(&recovery) || workspace.same_partition(&efi) { ...拒绝... }
484:  if image_volume.same_partition(&recovery) || image_volume.same_partition(&efi) { ...拒绝... }
```

只校验了 **workspace（程序目录卷）** 与 **image_volume（镜像卷）**。
**没有 `target.same_partition(&recovery)` 这一条。**

`validate_operation_inputs()`（927 行起）只校验 `image↔source`、`image↔target`、`source↔target`
三对关系，`VolumeIdentity` 的 `recovery` 根本没传进去。

### 2.3 影响范围

- `main.rs::format_target_partition` 格式化目标卷 → `C:\Recovery\WindowsRE\` 下的
  `Winre.wim`(712MB)、`boot.sdi`、`ReAgent.xml`、`ReAgentOld.xml` **全部被删除**；
- `main.rs::restore_original_winre`（1635 行）要从 `original/Winre.wim` 写回注册位置，
  而该原件备份若也在目标卷，会一并被格式化摧毁 → 回滚路径同时失效；
- 即使 Apply 成功，系统也不再有恢复环境；下一次需要 Recovery 时无法进入；
- 这与 `docs/development-roadmap-2026-09-26.md` F1 描述一致，且**在 prepare 层和执行层都缺防御**
  （方案 4.3 要求执行层同样拒绝，当前也没有）。

### 2.4 修复方案

**第一段（建议本轮交付，低风险）**

1. `prepare_task`：新增 `target.same_partition(&recovery)` 拒绝，错误码
   `RestoreTargetHostsRegisteredWinre`，提示语要明确「恢复环境位于还原目标上，尚不能安全执行」，
   **不能**只说「把程序移走」（移走程序不修这个）；
2. `main.rs::recover_*` 执行层：在格式化/Apply 前二次拒绝同一条件（防旧任务绕过准备层）；
3. 纯函数放进 `backuprestore-core`（`validate_volume_roles()`），GUI 与 CLI 共用同一份矩阵；
4. 单元测试覆盖 U01/U02 组合。

**第二段（后续，需 PoC）**：按方案 4.2，仅在「任务专用 WIM 启动 + 原资产保护 + 离线重新注册」
全部通过后，才放开 `target == registeredWinre.volume`。本轮不做。

---

## 3. F3：备份包含被本任务污染的注册 WinRE

### 3.1 触发条件（精确）

同时满足：

1. 操作为 `backup`；
2. 备份源卷 = 注册 WinRE 宿主卷（当前布局下即 **C:**）；
3. **走 WinRE 路径（不带 `--no-reboot`）** —— 系统卷备份必须离线，因此这条几乎必然成立。

关键判断：**在线路径不会触发 F3**。`windows_prepare.rs:747` 在 `no_reboot` 时提前 return，
不执行第 777 行的覆盖。这解释了为什么 v1.7.5 在 T: 上的在线备份（`affected=T`、`system=C` 不等）
是干净且能通过哈希比对的 —— 它压根没走注入/覆盖那条路。

### 3.2 根因（代码级）

`windows_prepare.rs::prepare_payload`：

```rust
677:  r"{}:\Recovery\WindowsRE\Winre.wim",     ← 挂载的是系统注册 WIM 本身
683:  fs::copy(&registered_wim, original.join("Winre.wim"))?;   ← 备份原件
699:  fs::copy(&registered_wim, &staged)?;                       ← 拷到 stage
707:  dism /Mount-Image ... stage/Winre.wim
712:  inject_winre_payload(&mount, &payload)   ← 注入 Recovery.exe + RecoveryTask.env + task.json
729:  dism /Unmount-Image /Commit              ← 固化污染
777:  fs::copy(&staged, &registered_wim)?;     ← ★ 把注入后的 WIM 覆盖回系统注册位置
794:  reagentc /boottore
```

**污染窗口**：从第 777 行覆盖成功，到 Recovery 里 `restore_original_winre()` 恢复为止。
在这段时间内对源卷做 Capture，打进镜像的 `Winre.wim` 就带本任务的
`Recovery.exe` / `RecoveryTask.env` / `task.json`。

`prepare_task` 缺 `source.same_partition(&recovery)` 拒绝 —— 与 F1 同源。

### 3.3 影响范围

- 产出「被污染」镜像：还原后携带旧任务的启动入口与可自动续跑的任务记录；
- 验收难点：污染不看表象。必须挂载生成 WIM 抽检内部 WinRE 哈希，
  **不能**只用「当前系统 WinRE 已恢复」来证明备份干净；
- 当前 VM 布局下，C: 备份（产品主场景）必然触发。

### 3.4 修复方案

**第一段（建议本轮交付）**：`prepare_task` 新增 `source.same_partition(&recovery)` 拒绝
（旧覆盖模式下），错误语义写明「该卷承载当前恢复环境，旧注入方式会污染镜像」；
核心库加纯校验函数 + 单测。

⚠️ **UX 影响必须提前确认**：这条拒绝生效后，**在当前 VM 布局下 C: 备份将完全不可用**，
直到第二段（任务专用 WIM）完成。这是用一个明确拒绝换掉一处静默污染，不是功能倒退，
但必须由用户裁定（见 Q4）。

**第二段（后续）**：改为注入 `tasks/<id>/boot/Winre.wim` 任务副本 + 独立 BCD 对象，
保持系统注册 WIM 干净，并给 Capture 加精确到项目路径的排除规则（方案 7.1）。

### 3.5 F1 与 F3 为何互不冲突

| | F1 | F3 |
|---|---|---|
| 影响的操作 | 还原目标 | 备份源 |
| 新增判断 | `target == recovery.volume` | `source == recovery.volume` |
| 是否共用同一枚举/错误码 | 否，各自独立错误码 | 否 |
| 第二段依赖关系 | 依赖独立 WIM + 离线注册 | 依赖独立任务副本 + 排除规则 |

两者只共享一个**新引入的纯函数** `validate_volume_roles()`（同一次调用里按顺序把两条规则都判了），
不存在状态互斥、不存在顺序依赖，也不需要共享可变结构。第一段交付时二者同时生效，行为叠加不冲突。

---

## 4. 待确认清单（**未确认前不修改任何代码或载荷**）

| # | 需确认项 | 为什么必须确认 | 待选答案 |
|---|---|---|---|
| **Q1** | **WinRE 载荷更新落在哪里** | 用户的约束是「测试盘操作、不动 C:」，但产品 prepare 硬编码读取 `<recovery卷>:\Recovery\WindowsRE\Winre.wim`。在测试盘重建的 WIM **产品不会使用**，除非同时实现第二段的独立任务 WIM 启动 | (A) 直接更新注册 WIM（需快照+授权，触碰 C:）<br>(B) 只在测试盘重建，产品暂不使用<br>**(C) 先在测试盘重建并验证，通过后再经快照+授权写回注册位置 ← 推荐** |
| **Q2** | **PE 载荷是否同步更新，以哪个为准** | `T:\petest\boot.wim` 是 2026-09-13 实验残留（内部 exe 1,026,560B，不在任何已知版本列表），宿主 `artifacts/` 另有正式 PE 产物，二者不是一回事 | (A) 同步更新 `artifacts/BackupRestorePE.wim`<br>(B) 只更新 T:\petest 实验副本<br>(C) PE 本轮不动，只更新 WinRE |
| **Q3** | **F1 修到哪一层** | 完整方案（含独立 BCD、task WIM、离线注册 PoC、断电入口）是 S0–S6 大工程，需实机验证引导；第一段只是加拒绝 | (A) 只做第一段拒绝 ← 推荐<br>(B) 直接冲刺完整方案 |
| **Q4** | **能否接受「C: 备份被拒绝」这一 UX 结果** | F3 第一段修复生效后，当前布局下系统盘备份将**完全不可用**，直到第二段完成 | (A) 接受拒绝，优先保证不产出脏镜像<br>(B) 保持现状直到第二段完成再说 |
| **Q5** | **测试盘工作目录用哪个卷** | 载荷工作需要 ≥ 3 份 WIM 空间（712MB×3 ≈ 2.2GB）加余量 | T:(4.4GB 可用，已是既定测试卷) / E:(132.9GB) / H:(132.9GB) / P:(6.3GB) |
| **Q6** | **是否授权我现在创建 Parallels 快照** | `AGENTS.md` 硬性要求：修改 BCD/WinRE 注册/PE 部署前必须先创建并核验新快照。载荷写回注册位置属于此类。**没有这个授权，任何写回动作都不能开始** | 授权 / 暂不授权（则仅做测试盘内的只读/隔离工作） |
| **Q7** | **约束边界澄清** | 「不要对 C 盘做备份还原」——是否也禁止更新 `C:\Recovery` 下的 WinRE 载荷文件？载荷更新≠备份还原，但确实触碰 C: | 允许更新载荷 / 一律不许动 C: |

---

## 5. 本轮踩到的新坑（已写进项目记忆）

1. **`prlctl exec` 调用 `powershell -File script.ps1 -Name value` 时参数被吞**（`$Name` 为空、
   变量落到默认值）。与已知「prlctl 吃 `$_`、吃引号、吃一层反斜杠」同族。
   **可靠做法：脚本不依赖外部传参，把要用的值写死在脚本里。**
2. **共享目录对本会话新建文件的首次读取可能得 0 字节**（`br-gui-exec.ps1` 的
   `WARN_COPY_BAD src=0 dst=0`），但同路径下一次只读诊断又能读到正常长度。
   **不要把这种 0 字节当成文件不存在**，换桥接实现并打印中间长度。
3. **`reagentc` / `C:\Recovery` 下的文件必须提权才有意义**：低权限下 `reagentc` 报错误 5，
   `Test-Path C:\Recovery\WindowsRE\Winre.wim` 会返回 False（不是文件不存在，是看不到）。
   第一次盘点据此误判过，差点写成「载荷丢失」。
4. DISM 挂载输出带实时进度条，直接 `type` 日志会被刷屏；取证要 `Where-Object` 过滤关键行。

---

## 6. 证据位置

| 内容 | 路径 |
|---|---|
| 提权盘点：reagentc / 注册 WIM / BCD 引用 / PE 内部 / 磁盘布局 | 客体 `C:\Users\Public\pkg\_payload_inv_elev.txt` |
| 提权取证：WIM 镜像信息、内部 Recovery.exe 哈希、完整磁盘布局 | 客体 `C:\Users\Public\pkg\_winre_inner.txt` |
| 桥接与诊断日志 | 客体 `C:\Users\Public\pkg\_elev_bridge.txt`、`_diagsrc.txt` |
| 宿主侧脚本 | `.test-artifacts/payload-inventory.ps1`、`-elev.ps1`、`winre-inner-hash.ps1`、`elev-bridge.ps1` |
