# WinRE 载荷更新 + F1/F3 第一段修复：变更、验证与回滚（2026-09-27）

本文是 [勘察报告](20260927-1005-winre-payload-and-p0-recon.md) 的执行结果。
版本：**v1.7.5 → v1.7.6**。PE 载荷本轮**不处理**（用户裁定）。

用户本轮裁定（已执行）：

| 项 | 裁定 |
|---|---|
| 载荷落点 | 先在测试盘重建验证，通过后再经**快照 + 授权**写回注册位置 |
| PE | 本轮不动，只测 WinRE（RE） |
| F1 | 只做**第一段拒绝**（准备层 + 执行层二次闸，不做独立 WIM 方案） |
| 测试卷 | 由我按空间需求选（结论：workspace=H:、image=E:、source=T:、target=P:） |
| 快照 | 已授权创建 |
| 「不动 C:」的边界 | 只是**不用程序对 C 盘做备份/还原**；更新 WinRE 载荷等其它操作允许 |

---

## 1. 变更清单

### 1.1 产品代码（Rust，唯一语言）

| 文件 | 变更 | 说明 |
|---|---|---|
| `crates/backuprestore-core/src/lib.rs` | 新增 `VolumeRoleConflict` 枚举、`VolumeRoles` 结构、`validate_volume_roles()` 纯函数 | F1/F3 共用的规则矩阵，GUI/CLI/Recovery 三处同一份判定；含 4 组新单测 |
| `crates/backuprestore-cli/src/windows_prepare.rs` | 准备层校验改用 `validate_volume_roles()` | 新增 F1（target vs recovery）与 F3（source vs recovery，仅旧注入模式）两条拒绝；EFI 校验保留且文案拆开 |
| `crates/backuprestore-cli/src/main.rs` | 新增 `recovery_volume_from_env()`、`winre_role_conflict_at_execution()`，在备份捕获前 / 还原格式化前各加一道闸 | 执行层二次拒绝，防止旧任务或跨版本任务绕过准备层 |
| `VERSION`、`crates/*/Cargo.toml` | 1.7.5 → **1.7.6** | 产物 1,694,208 B / `f694a846ad44c65737afa0a6785cf07b9a43d9f7ec0a78cabb34c4ab7b41a85f` |

单测：`cargo test -p backuprestore-core` **25 passed / 0 failed**（新增 4 组：工作区/镜像卷冲突、F1、F3 随 `mutates_registered_winre` 变化、F1 与 F3 互不干扰）。
交叉编译 `cargo check --target aarch64-pc-windows-msvc` 与 `cargo clippy` 均干净（仅既有的 libcmt PDB 链接告警）。

### 1.2 WinRE 载荷（已写回注册位置）

| 项 | 变更前 | 变更后 |
|---|---|---|
| 注册位置 | `C:\Recovery\WindowsRE\Winre.wim`（harddisk0 partition4） | 不变 |
| WIM 大小 | 712,108,371 B | **714,409,997 B** |
| WIM SHA-256 | `dbbce2bd…765c` | **`5df963018c763933d08e4c3d79cac7b0d6b3dc9c838d84b564ac8782d93d6efe`** |
| 内部 `Windows\System32\Recovery.exe` | 1,687,040 B / `653abc68…98e`（**v1.7.3**） | **1,694,208 B / `f694a846…85f`（v1.7.6）** |
| 内部 `winpeshl.ini` | 99 B，`[LaunchApps] %SYSTEMROOT%\System32\Recovery.exe,recover-env …RecoveryTask.env` | 不变（仍指向 Recovery.exe，无需改） |
| 内部残留污染 | 无 | 无（`\Recovery` 任务目录不存在） |
| `boot.sdi` | 存在 | 不变 |

**PE 载荷：本轮不动。** `T:\petest\boot.wim` 是 2026-09-13 的实验残留（内部 exe 1,026,560 B，不在任何版本列表），
宿主 `artifacts/BackupRestorePE.wim` 是另一条产物线。二者都不是当前 WinRE 链路的一部分，
按用户裁定本轮只测 RE；PE 同步留到需要 PE 实机验证时再做（见第 8 节待办）。

---

## 2. F1：还原目标可能销毁当前 WinRE 入口

### 触发条件（精确）

同时满足：① 操作为 `restore-existing` 或 `create-secondary`（带 `--allow-destructive`）；② 还原目标卷 = 注册 WinRE 宿主卷（本 VM 即 **C: partition4**）；③ 走到格式化 / Apply 阶段。

### 根因

`prepare_task` 只校验了 workspace 与 image_volume 对 recovery 的关系，**没有 `target`**；
`validate_operation_inputs()` 里 `recovery` 根本没参与。`main.rs` 执行层同样没有这道判断，
`format_target_partition` 会直接 `diskpart format`，把 `C:\Recovery\WindowsRE\` 下的
`Winre.wim`、`boot.sdi`、`ReAgent.xml` 连同回滚原件一起删除。

### 方案（第一段，本轮交付）

1. 核心库纯函数 `validate_volume_roles()` 增加 `target == recovery` 拒绝，错误码 `restore-target-on-registered-winre`；
2. 准备层调用该纯函数（GUI 与 CLI 共用同一份矩阵）；
3. `recover_windows` 在 `format_target_partition` 之前二次判同一条件（用 `RecoveryTask.env` 里的 `RECOVERY_DISK_GUID` / `RECOVERY_PARTITION_GUID`），挡住旧任务 / 手工续跑绕过准备层；
4. 4 组单测覆盖（含负向对照）。

提示语明确说明「格式化会删除 Winre.wim 及用于回滚的原件副本」+「先把 WinRE 迁出该分区，或选择其它还原目标」，**不是**「把程序移走」。

### 影响范围

当前这台的布局（WinRE 注册在 C:，无独立 Recovery 分区）下，**以 C: 为还原目标的所有操作都被拒绝**。
标准布局（有独立 WinRE 分区）下 C: ≠ recovery 卷，不受影响。

---

## 3. F3：备份包含被本任务污染的注册 WinRE

### 触发条件（精确）

① 操作为 `backup`；② 备份源卷 = 注册 WinRE 宿主卷（本 VM 即 C:）；③ **走 WinRE 路径（不带 `--no-reboot`）**。

### 根因

`prepare_payload` 把注入后的 WIM `fs::copy` 回注册位置（v1.7.6 代码 `windows_prepare.rs` 覆盖注册 WIM 那一行），
从那一刻到 Recovery 里 `restore_original_winre()` 为止是「污染窗口」；窗口内对源卷做 Capture，
镜像里的 `Winre.wim` 就带着本任务的 `Recovery.exe` / `RecoveryTask.env` / `task.json`。

### 方案（第一段，本轮交付）

1. `validate_volume_roles()` 增加 `source == recovery` 拒绝，错误码 `backup-source-on-registered-winre`；
2. **条件收紧为 `mutates_registered_winre == true`（即 `!options.no_reboot`）**——污染只发生在「把注入后的 WIM 覆盖回注册位置」这条路径上，`--no-reboot` 在线准备在覆盖之前就返回，不存在污染窗口，因此在线备份**不拒绝**；
3. `recover_windows` 在捕获之前二次判同一条件（拿不到 recovery 身份时跳过这道闸，不臆造身份）；
4. 单测覆盖「同一布局下 `--no-reboot` 放行」这一行为差异。

### 「C: 备份完全不可用」到底是什么意思（用户追问的解答）

这句话的完整含义，逐条拆开：

1. **它不是产品永久失去系统盘备份能力**，而是「**在这台 VM 的当前布局下**，系统盘备份不再走 WinRE 路径」。
   判定条件是 `备份源卷 == 承载注册 WinRE 的卷`。这台机器没有独立 Recovery 分区，WinRE 注册在 C:，
   所以「备份 C:」和「源卷就是恢复环境宿主卷」是同一件事 → 命中拒绝。
   换成一台有独立 WinRE 分区的正常机器（绝大多数出厂 Windows），C: ≠ recovery 卷，备份 C: **完全不受影响**。
2. **走在线路径（带 `--no-reboot`）的 C: 备份不被拒绝**。我把条件收紧到 `mutates_registered_winre` 就是为这个：
   在线准备不改写注册 WIM，没有污染窗口，没有理由拒绝。真正被挡掉的只有「离线（WinRE）系统盘备份」这一种组合。
3. **对本轮开发的实际代价是零**：用户已定下长期约束——开发/测试一律在测试盘，不对 C: 做备份还原。
   被禁掉的这条路径本来就不走。
4. **换来的是什么**：把一处**静默产脏镜像**（还原后自带上次任务的启动入口与任务记录，且验收时看表象看不出来，
   必须挂载 WIM 抽检内部哈希才能发现）换成一个**明确拒绝**。

所以这不是功能倒退，而是「最坏布局下，先把会产出脏镜像的那条路关掉」，等第二段（任务专用 WIM + 独立 BCD 对象，
保持系统注册 WIM 干净）完成后自动放开。

---

## 4. F1 与 F3 互不冲突

| | F1 | F3 |
|---|---|---|
| 约束对象 | 还原目标卷 | 备份源卷 |
| 判定 | `target == recovery` | `source == recovery` **且** 会改写注册 WIM |
| 错误码 | `restore-target-on-registered-winre` | `backup-source-on-registered-winre` |
| 生效位置 | 准备层 + 格式化前 | 准备层 + 捕获前 |
| 第二段依赖 | 任务专用 WIM + 离线重新注册 | 任务副本注入 + Capture 排除规则 |

实现上两者只共享一个新引入的**纯函数**（一次调用里按 operation 分支各判一条），无状态、无顺序依赖、无共享可变结构。
单测 `f1_and_f3_do_not_interfere` 专门验证：备份时传了 target 不触发 F1；还原时源 == recovery 不触发 F3。

---

## 5. 测试盘验证步骤与结果

卷角色分配（全部避开 C:）：**workspace = H:\brwork**（程序目录）、**image = E:\brimg**、**source = T:**（5 GB 小卷）、**target = P:**。
前置快照：`{e9dbdf22-53f6-4a87-9bde-3b3411bbb4da}`（2026-09-27 建，目的：v1.7.6 载荷写回与 F1/F3 修复前的回退基线）。

### 5.1 载荷重建（只在测试盘，未触碰 C:）

| 步骤 | 命令要点 | 结果 |
|---|---|---|
| 复制注册 WIM 到测试盘 | `C:\Recovery\WindowsRE\Winre.wim` → `H:\payload\stage\Winre.wim` | sha 与源一致 `dbbce2bd…` |
| 挂载 | `dism /Mount-Image /Index:1 /MountDir:H:\payload\mount` | rc=0 |
| 检查旧载荷 | 内部 `Recovery.exe` | 1,687,040 B / `653abc68…`（v1.7.3，落后两版，与勘察一致） |
| 注入 | 用 `H:\brwork\Recovery.exe`（v1.7.6）覆盖 | 内部变 1,694,208 B / `f694a846…` |
| 提交 | `dism /Unmount-Image /Commit` | rc=0，新 WIM `5df96301…`（714,409,997 B） |
| 复核 | 只读重挂 + 哈希 | 内部 = `f694a846…`，`winpeshl.ini` 不变，无 `\Recovery` 残留；**注册 WIM 此时仍为 `dbbce2bd…`（未被改）** |

### 5.2 F1 / F3 拒绝用例（CLI，prepare 层）

| # | 命令（工作目录 H:\brwork） | 期望 | 实测 |
|---|---|---|---|
| T1b | `create-secondary --source-drive T: --target-drive C: --image-path E:\brimg\src.wim --allow-destructive --no-reboot` | F1 拒绝 | ✅ exit=1，`还原目标分区承载当前 Windows 恢复环境（WinRE）…` |
| T1c | 同上但 `--target-drive P:` | 通过 | ✅ exit=0，TASK_ID=4ef99ed2… |
| T2 | `backup --source-drive C: --image-path E:\brimg\c.wim`（离线） | F3 拒绝 | ✅ exit=1，`备份源分区承载当前 Windows 恢复环境（WinRE）…` |
| T3 | `backup --source-drive C: … --no-reboot`（在线） | **通过**（收紧条件生效） | ✅ exit=0，TASK_ID=a7cdab56… |
| T4 | `backup --source-drive T: … --no-reboot` | 通过 | ✅ exit=0，TASK_ID=1715f196… |
| T5 | `restore-existing --source-drive T: --target-drive T: --image-path E:\brimg\src.wim --allow-destructive --no-reboot` | 通过 | ✅ exit=0，TASK_ID=b61400bc… |
| T6 | `create-secondary --source-drive T: --target-drive P: … --no-reboot` | 通过 | ✅ exit=0，TASK_ID=56d4665d… |

附带证据：`RecoveryTask.env` 里确有 10 行 `RECOVERY_*`（含 `RECOVERY_DISK_GUID` / `RECOVERY_PARTITION_GUID`），
执行层二次闸能拿到身份；全部 `--no-reboot` 用例跑完后注册 WIM 仍是 `dbbce2bd…`，**准备层未误改系统 WinRE**。

### 5.3 载荷写回 + WinRE 端到端实跑

| 步骤 | 结果 |
|---|---|
| 写回前留底 | `C:\Recovery\WindowsRE\Winre.wim` → `H:\payload\out\Winre-before-v176.wim`（`dbbce2bd…`） |
| 写回 | 注册 WIM = `5df96301…`（与测试盘重建产物一致），`reagentc /info` 仍指向 `harddisk0 partition4`，`boot.sdi` 在位 |
| 端到端 | `prepare --operation backup --source-drive T: --image-path E:\brimg\t.wim`（**不带 `--no-reboot`**，真进 WinRE） |
| WinRE 内 | 捕获成功：367,990,173 B，1s；日志 `WinRE cleanup completed; task marked successful` |
| **版本证据** | 镜像 sidecar `t.wim.index-1.metadata.json` → **`"programVersion": "1.7.6"`** ← WinRE 里跑的确实是新载荷（旧载荷是 1.7.3） |
| 任务后 | 注册 WIM 被 cleanup 还原为 `5df96301…`（= 新载荷，正确），VM 正常回桌面 |

**结论：载荷更新 + F1/F3 第一段均已实测闭环。**

---

## 6. 最终验收（正式环境）需要确认的检查项

在正式环境做那一次最终验收时，逐项打勾：

1. **版本一致**：`BackupRestore.exe` 与 WinRE 内 `Recovery.exe` 同为同一版本（挂载注册 WIM 抽检内部哈希，与宿主产物哈希比对）。
2. **WinRE 可引导**：`reagentc /info` = Enabled；`reagentc /boottore` 后能真的进 WinRE，且 winpeshl 能拉起 Recovery.exe（不是黑屏命令行）。
3. **任务闭环**：一次完整备份 → WinRE 捕获 → 自动回桌面；`Recovery.log` 出现 `WinRE cleanup completed`，任务状态 = Success。
4. **注册 WIM 复原**：任务结束后注册 `Winre.wim` 的 SHA-256 与任务 `manifest.original_winre_sha256` 一致（当前实现用 `original/Winre.wim` 还原并校验）。
5. **镜像干净**：挂载产出的 WIM，检查内部 `Windows\System32\` 下**没有**本任务的 `Recovery.exe` 之外的 `RecoveryTask.env` / `task.json` 残留（F3 第二段的验收点，第一段靠拒绝规避）。
6. **还原到非系统卷**：还原到目标卷后能正常启动（或至少能 Apply + 修引导，视 create-secondary 场景而定）。
7. **F1/F3 拒绝可复现**：在正式环境用「目标/源 = 承载 WinRE 的卷」各跑一次 prepare，应得到本文的中文拒绝提示，且**无任何副作用**（未创建任务、未改 BCD、未改 WinRE）。
8. **回滚可用**：故意让一次 WinRE 任务失败（例如 `--test-fault power-loss-window`），确认注册 WIM 与 BCD 能自动回滚，机器能正常回桌面。

---

## 7. 失败时的回滚方案

| 场景 | 回滚动作 | 依据 |
|---|---|---|
| 载荷写回后 WinRE 起不来 / 引导异常 | `prlctl snapshot-switch "Windows 11" -i e9dbdf22-53f6-4a87-9bde-3b3411bbb4da` | 本轮前置快照（写回之前的状态，注册 WIM = `dbbce2bd…`） |
| 只想退回旧载荷、不动其它 | 用留底副本覆盖：`copy /y H:\payload\out\Winre-before-v176.wim C:\Recovery\WindowsRE\Winre.wim`，再校验 sha = `dbbce2bd…765c` | 测试盘留底（本次未删除） |
| 更早就有的干净基线 | 快照 `{10afd315-362e-43ae-a4b0-192cdb4760db}`（本会话开始时的当前快照） | 用户恢复的正常 Windows 基线 |
| 代码回退 | `git revert` / `git checkout` 到提交前的版本，重新 `./build-win.sh` 并重做载荷写回 | 代码変更只涉及三个 Rust 文件，无数据迁移 |
| BCD 被误改 | 任务目录内有 `bcd-before-raw` / `bcd-before-export`；`reagentc /info` 确认注册路径；**不要全局清理 BCD**——库里还有 3 条指向已失效 Y: 卷的孤儿引用，删之前先确认所有权 | 勘察报告第 1.2 节 |

注意：快照切换会丢弃快照之后的全部磁盘改动，切换前确认没有需要保留的测试数据。

---

## 8. 待办（第二段，本轮不做）

1. **F1 第二段**：任务专用 WIM 启动 + 原资产保护 + 离线重新注册，通过后才放开 `target == registeredWinre.volume`。
2. **F3 第二段**：改为注入 `tasks/<id>/boot/Winre.wim` 任务副本 + 独立 BCD 对象，系统注册 WIM 保持干净；给 Capture 加精确到项目路径的排除规则。
3. **PE 载荷同步**：先确定正式 PE 产物以哪个为准（`artifacts/BackupRestorePE.wim` 还是 VM 内实验副本），再一次性同步并做 PE 实机验证。
4. **执行层二次闸的实机触发验证**：触发它意味着要真的格式化承载 WinRE 的卷，只能在一次性 VM 上做（代价是重建 WinRE）。目前只有静态证据（env key 齐全 + 单测覆盖纯逻辑），**尚未实机触发过**——验收时若要补，请用一次性 VM。

---

## 9. 本轮实测经验（VM 提权执行通道）

比之前用的 `runas` 提权桥接简单得多，后续做 VM 内操作优先用这条：

1. **`prlctl exec "Windows 11" cmd /d /c "..."`（不加 `--current-user`）就是 SYSTEM + 管理员**（实测 `whoami` = `nt authority\system`，`net session` 成功）。
   DISM 挂载/提交、`reagentc`、写 `C:\Recovery` 全部直接可用，**不需要 ShellExecute runas 桥接**。
2. **SYSTEM 看不到 X: 盘符**，但 UNC `\\Mac\backupRestore\...` 读写都正常（读脚本、写结果回宿主都走 UNC）。
3. **`powershell -File` 不能直接吃 UNC 路径**（prlctl 传参把路径搞坏，PowerShell 报找不到 `.ps1`）。
   先 `copy /y \\Mac\...\x.ps1 C:\Users\Public\pkg\x.ps1`，再执行本地路径。
4. **脚本正文保持 ASCII**：`-File` 按 GBK 解码，中文注释可能把脚本解坏（既有踩坑）。要输出中文就写到 UNC 上的 UTF-8 文件，回宿主再读。
5. **`Get-Item` 对这些几百 MB 的 WIM 会报「找不到路径」，但 `Get-FileHash` 正常**。取长度用 `[System.IO.FileInfo]::new($p).Length`。
6. `prlctl exec` 里 cmd 的反斜杠要写成 `\\`（UNC 路径 `\\\\Mac\\...`），这是既有踩坑（prlctl 吃一层反斜杠）。

---

## 10. 证据位置

| 内容 | 路径 |
|---|---|
| 卷盘点 + 部署 | 宿主 `.test-artifacts/v176-step1-deploy.ps1` |
| 测试盘重建载荷 | `.test-artifacts/v176-step2-rebuild.ps1` |
| 只读复核 | `.test-artifacts/v176-step3-verify.ps1` |
| F1/F3 + 正常路径回归 | `.test-artifacts/v176-step4-tests.ps1`，输出 `v176-t*.txt` |
| F1 拒绝 + 执行层证据 | `.test-artifacts/v176-step5-f1.ps1`，`v176-env-recovery-keys.txt` |
| 载荷写回 | `.test-artifacts/v176-step6-writeback.ps1` |
| WinRE 端到端 | `.test-artifacts/v176-step7-e2e.ps1`，输出 `v176-t7-prepare.txt`；客体 `E:\brimg\Recovery.log`、`E:\brimg\t.wim.index-1.metadata.json` |
