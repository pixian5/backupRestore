# BackupRestore

Windows 一键系统备份还原工具（**开发测试版**）。

在正常 Windows 里点「备份」或「还原」，程序自动准备恢复任务 → 重启进入 WinRE → WinRE 自动拉起
`Recovery.exe` 离线执行 DISM 捕获/应用 WIM → 修复启动项 → 重启回正常 Windows。用户不需要做 U 盘、
进 BIOS、手动选 WinRE，也不需要敲命令。

当前版本 **1.9.5**（`VERSION`、两个 `Cargo.toml`、`Cargo.lock` 同步）。

> **文档导航**：
> - 完整文档目录与各文档说明请参阅根目录 [文档索引.md](文档索引.md)。
> - 【当前开发进度】：wim-info 实现 sidecar 元数据优先读取（毫秒级呈现）与 --skip-hash 快速预览开关，详见 [docs/20260930-1830-当前开发进度-Gemini.md](docs/20260930-1830-当前开发进度-Gemini.md)。
> - 【下一步待实现】：孤儿 BCD 与孤儿暂存目录开机自检机制、新增第二系统与自定义 PE 恢复桌面闭环实测，详见 [docs/20260930-1830-下一步待实现-Gemini.md](docs/20260930-1830-下一步待实现-Gemini.md)。

> 版本号规则（用户 2026-09-29 重申）：每次修改 +0.0.1，**每一位满十才进位**。
> 因此 `1.7.9` 之后是 `1.8.0`（第三位满十，进给第二位），不是 `1.7.10`。
> 历史上 `1.7.10`~`1.7.14` 是旧习惯记法，等价于 `1.8.0`~`1.8.4`；自 v1.8.0 起按规则进位。

---

## 一、产品流程

```text
正常 Windows
  └─ 打开 BackupRestore.exe，点「系统备份」/「系统还原」
       └─ prepare：校验环境 + 注入载荷到任务专用 WinRE + reagentc /boottore
            └─ 重启
                 └─ WinRE 自动启动 Recovery.exe
                      └─ 离线执行 DISM 捕获/应用 WIM
                           └─ BCDBoot 修复启动项 → 恢复原 WinRE → 重启
                                └─ 正常 Windows
```

### V1 支持的四种任务

| 任务 | 作用 | 破坏性 |
|---|---|---|
| `probe` | 只验证任务、载荷、卷挂载和 WinRE 自动入口 | 否 |
| `backup` | 从现有 Windows 分区用 DISM 捕获 WIM | 否（写入指定镜像卷） |
| `restore-existing` | 还原到原 Windows 分区并修复引导 | 是 |
| `create-secondary` | 还原到独立分区并加入第二启动项（双系统） | 是 |

### V1 范围边界

**支持**：Windows 10/11、UEFI + GPT、NTFS 系统分区、WIM 镜像、Windows 自带 WinRE（也支持自定义 PE）、
DISM 作为镜像引擎、BCDBoot/BCD 作为启动管理。

**明确不做**（不要擅自扩展成完整 Ghost/PE 工具）：分区布局重构、网络备份、增量/差异镜像、
Legacy BIOS、自动修改 BitLocker。

---

## 二、代码结构

产品代码 **全部是 Rust**，两个 crate，约 1.66 万行。依赖只有 `serde` / `chrono` / `serde_json` /
`sha2` / `thiserror` / `uuid` —— **没有任何 .NET/dotnet/C# 运行时依赖**。

```
crates/
├── backuprestore-core/     库：任务模型、载荷契约、元数据、SHA-256、错误类型
└── backuprestore-cli/      可执行：原生 GUI + 准备 + WinRE 恢复环境
    ├── native_gui.rs       原生 Win32 GUI（7907 行）
    ├── main.rs             入口与任务分发（3080 行）
    ├── windows_prepare.rs  正常 Windows 侧准备：校验/注入载荷/设置一次性 WinRE 启动（2136 行）
    ├── text_parsing.rs     命令行输出（DISM/bcdedit/mountvol）本地化解析（1015 行）
    ├── recovery_progress.rs WinRE 侧进度上报
    └── winre_payload.rs    WinRE 载荷组织
```

PowerShell / .NET 只出现在**测试脚手架**（`tools/win-clicker/`）和一次性探针（`.test-artifacts/`），
不进产品、不影响交付物。

---

## 三、构建

构建目标是 **Windows ARM64**（`aarch64-pc-windows-msvc`），静态链接 CRT，链接本机 `~/win-sdk-arm64`：

```bash
./build-win.sh
```

产物：`target/aarch64-pc-windows-msvc/release/BackupRestore.exe`
（脚本会把它同时部署为客体 `C:\Users\Public\backupRestore-package\` 下的
`BackupRestore.exe` 和 `Recovery.exe`，并核对 SHA-256）。

构建规则、静态 CRT、Windows SDK 路径等细节见 `docs/windows-build.md`。

---

## 四、目录说明

| 路径 | 内容 |
|---|---|
| `crates/` | 产品源码（Rust，唯一产品语言） |
| `docs/` | **全部文档：顶层 32 份 + `开发方案/` 12 份，索引见 `docs/README.md`** |
| `tools/win-clicker/` | 测试脚手架：操控 VM 键鼠的自动化通道（PowerShell，不进产品） |
| `build-win.sh` | ARM64 交叉构建 + 部署 + 哈希核对 |
| `artifacts/` | PE 构建产物（`BackupRestorePE.iso/wim`、`BaseWinPE.iso`） |
| `pe-assets/` | PE 源 WIM / SDI |
| `windows/` | Windows 侧辅助脚本与 `winpeshl.ini` |
| `target/` | Rust 构建产物（8G+，不入库） |
| `.test-artifacts/` | 测试证据与一次性探针（5G+，已 gitignore） |
| `VERSION` | 当前版本号 |
| `AGENTS.md` | 开发验证硬约束（**改启动项前必须建快照**） |

---

## 五、开发约束（必读）

1. **动启动项前必须建快照**。修改 BCD、默认项、一次性启动序列、WinRE 注册或 PE 部署之前，
   先创建并核验 Parallels 快照，记录快照 ID、目的和变更范围。快照失败不得继续。详见 `AGENTS.md`。
2. **启动成功 ≠ 备份还原成功**。验证要跑到 DISM 实际完成、启动项修复、回正常 Windows 为止。
3. **测试只能在可回滚的 VM 快照里做**，尤其是 `restore-existing` / `create-secondary`。
4. **不触碰 C:** 的范围是：不把 `C:` 作为 backup 源分区或还原目标分区；读取 C: 状态、使用其
   WinRE/BCD、写任务和日志是允许的。
5. **大文件下载前先查网络**。热点环境必须逐次取得用户授权。
6. 每次改动后：本地测试 → Windows ARM64 构建 → 部署 → 核对哈希。

---

## 六、文档索引

**完整文档索引（每份说明）见 [`文档索引.md`](文档索引.md)**。以下为高频入口：

**所有文档的逐份说明在 [`docs/README.md`](docs/README.md)**，顶层 32 份分九类 + `开发方案/` 子目录。新接手先读这三份：

| 文档 | 作用 |
|---|---|
| [`docs/project-status.md`](docs/project-status.md) | 当前状态、V1 范围、用户确认的约束 |
| [`docs/current-status-2026-09-16.md`](docs/current-status-2026-09-16.md) | 执行基线（当前版本/未完成项以它为准） |
| [`docs/verification-matrix.md`](docs/verification-matrix.md) | 哪些功能真实验证过、证据在哪 |

高频入口：

- 操作 VM 键鼠（自动化点击/按键/截图）→ [`docs/vm-input-control-guide.md`](docs/vm-input-control-guide.md)
- ARM64 构建规则 → [`docs/windows-build.md`](docs/windows-build.md)
- WinRE 引导 / PD27 固件回归证明 → [`docs/winre-boot-failure-parallels27-2026-09-25.md`](docs/winre-boot-failure-parallels27-2026-09-25.md)
- S 盘反复弹窗/unmount 竞态根因与卷路径改造方案 → [`docs/202609292014-S盘自动打开问题定位与修复方案.md`](docs/202609292014-S盘自动打开问题定位与修复方案.md)
- 下一步开发路线图 → [`docs/development-roadmap-2026-09-26.md`](docs/development-roadmap-2026-09-26.md)
- 完整需求原文 → [`Windows 一键系统备份还原 V1——完整开发需求.md`](Windows%20一键系统备份还原%20V1——完整开发需求.md)

---

## 七、当前进度（截至 2026-09-30）

截至 2026-09-30 17:00 的进度已更新至 [当前开发进度](docs/20260930-1700-当前开发进度-Gemini.md)，下一步安排见
[下一步待实现](docs/20260930-1700-下一步待实现-Gemini.md)。

### v1.8.11→v1.8.14（2026-09-30）：GUI 离线环境选择按钮修复 + 实机双分支复验 PASS

用户反馈「进入 Windows RE 点了没反应」。三个连锁缺陷：

1. `ask_system_drive_handler` 在 `DestroyWindow` **之后**还用 `GetWindowLongPtrW(dialog, …)` 读结果盒 —— 窗口已销毁，未定义行为，实机稳定返回 `0`，用户点的 `choice=2` 被吞，程序按取消处理。
2. `window_proc_system_choice` 的 `WM_DESTROY` 调 `PostQuitMessage` —— **线程级**，会连带杀死共享消息队列的主窗口循环，实机抓到「点完对话框整个程序消失」。
3. v1.8.13 改 `IsWindow` 循环后暴露：`WM_COMMAND` 里**同步** `DestroyWindow` 会销毁正在执行窗口过程的按钮 → 跨进程 `SendMessage(BM_CLICK)` 永久阻塞。

修复：① 结果盒裸指针留局部变量直接读；② 删掉 `WM_DESTROY` 的 `PostQuitMessage`、模态循环改按 `IsWindow(dialog)` 退出；③ 两处改 `PostMessageW(WM_CLOSE)` 异步销毁 + 新增 `WM_CLOSE` 分支执行真正的 `DestroyWindow`。另 prepare 改 `SW_SHOWNORMAL`、确认框文案改走 `operation_display`（此前中文界面显示成「将创建 backup 任务」）。

| 项 | 结果 |
|---|---|
| 本机验证 | `cargo test --workspace` **95 passed / 0 failed**（cli 72 + core 23） |
| 构建/部署 | v1.8.14 SHA-256 `71a157d0…abab06`，本地与 `H:\brwork` 一致 |
| **实机复验** | ✅ **用真实 `WM_COMMAND` 注入（非 `--test-hook`）**：RE 分支 `choice=2` 正确捕获、PE 分支 `choice=1` 正确捕获，进程全程存活，确认框可取消回主界面 |

> ✅ 该项已闭环，详见 [docs/20260930-085630-gui-system-drive-choice-button-verified.md](docs/20260930-085630-gui-system-drive-choice-button-verified.md)。**已补验**：「点『是』→ 真实重启进 RE → 备份 → 回 Windows」整段已于 2026-09-30 实机 PASS（真实系统卷 C:，87GB），见 [docs/20260930-100300-re-full-backup-passed.md](docs/20260930-100300-re-full-backup-passed.md)。

### ✅ RE 分支完整备份闭环 PASS（2026-09-30 约 09:44，v1.8.14 实机）

真实系统卷 C:（87GB 未压缩）走 GUI `choice=2` → 真实重启进 WinRE → DISM 捕获 → 回 Windows；终态干净。
这是此前「按钮级复验」之后唯一未补的整段链路。

| 终态核验 | 结果 |
|---|---|
| 任务状态 | ✅ `stage=success` / `progress=100`（`status.json`） |
| 镜像可用 | ✅ 索引 1，`dism /Get-WimInfo` 可读，大小 87,254,275,408 字节（磁盘 33.4GB fast 压缩） |
| BCD 一次性启动 | ✅ 已清（`{bootmgr}` 无 `bootsequence`） |
| 注册 WinRE | ✅ 未动 / `Enabled`（`reagentc /info` 位置 `harddisk0\partition4`，标识符 `f530b9e0`） |
| 本轮自建 BCD 条目 `{29662687}` | ✅ 已删（`disarm()` 生效） |

> ⚠️ **发现历史孤儿 BCD 条目**：`bcdedit /enum all` 仍有一对 `{76ead7a9}/{76ead7a8}`（description "BackupRestore task RE"，ramdisk=`F:\BackupRestoreRE\Winre.wim`），来自**更早**测试轮次（本轮 GUID 为 `{29662687}` 已正确清理，二者不同）。不在 bootsequence/displayorder，不影响启动；建议建快照后用 `bcdedit /delete` 清理，并删已空的 `F:\BackupRestoreRE`。详见 [docs/20260930-100300-re-full-backup-passed.md](docs/20260930-100300-re-full-backup-passed.md)。

### 已实机验证（测试卷 T:，非系统卷，在线路径）

| 项 | 状态 |
|---|---|
| 压缩选项收敛：`压缩`/`不压缩` → `/Compress:fast` / `/Compress:none`，`max` 已删除 | 中英文界面实测，两项索引不变 |
| `backup` 捕获 WIM + 只读挂载哈希比对 | 两种压缩各一次，哈希与源逐项一致 |
| `restore-existing` 还原闭环 | v1.7.5 起**不停 Parallels 服务也能成功**（见下） |
| v1.7.5 修复：`\Mac disk` 纳入默认排除 + GUI 在线备份补接 `/ConfigFile` | 产物 `f8b77d0e…e184`，已部署客体并核对哈希 |
| 离线测试 | `cargo test -p backuprestore-core` 28 passed / 0 failed |

### v1.7.6（2026-09-27）：载荷更新 + F1/F3 第一段已闭环

| 项 | 状态 | 证据 |
|---|---|---|
| WinRE 载荷 v1.7.3 → **v1.7.6** | 测试盘重建 → 快照授权写回 → **WinRE 内实跑** | 注册 WIM `5df96301…`；WinRE 产物 sidecar `"programVersion": "1.7.6"` |
| F1（还原目标 == WinRE 宿主卷） | 准备层拒绝 + 格式化前二次闸 | T1b 拒绝 / T1c·T5·T6 正常通过 |
| F3（离线备份源 == WinRE 宿主卷） | 准备层拒绝（收紧到 `--no-reboot` 之外的离线路径）+ 捕获前二次闸 | T2 拒绝 / T3 在线放行 / T4 正常通过 |
| 完整离线链路（桌面 prepare → WinRE 捕获 → 回桌面） | 测试卷 T: → E: 实跑成功 | `E:\brimg\t.wim` 367,990,173 B，日志 `WinRE cleanup completed` |

详见 [`docs/winre-payload-and-p0-fix-2026-09-27.md`](docs/winre-payload-and-p0-fix-2026-09-27.md)。

### v1.7.7（2026-09-27 晚）：方案 D「捕获前换回干净原件」落地，F1/F3 两个根因彻底解锁

阶段 2 路线在 1.7.6 之后收敛为用户裁定的**方案 D**：不把 `\Recovery\WindowsRE\Winre.wim`
排除出镜像，而是**在 DISM 捕获之前把它覆写回任务暂存的干净原件**，于是镜像自带干净 WinRE
（合乎微软惯例、异地/旧镜像还原自包含），代价仅一次 ~700MB 写入。一处修复同时解 F1/F3 两个 P0。

| 项 | 状态 | 证据 |
|---|---|---|
| 核心改动：`validate_volume_roles` 扩展 5 参数 + `PLAN_D_RESTORE_CLEAN_WINRE_BEFORE_CAPTURE` 开关 | 已落地 | `core` 28 单测全绿（新增 `plan_d_opens_f3_when_source_hosts_registered_winre` 锁定「开关开→F3 放开、关→恢复拒绝」） |
| 准备层 F3 放开（离线备份源 == WinRE 宿主不再被拒） | 已落地 | `windows_prepare.rs:509` 传 `PLAN_D_RESTORE_CLEAN_WINRE_BEFORE_CAPTURE` |
| 执行层 `restore_clean_winre_before_capture` | 已落地 | `main.rs:1213`，捕获前覆写源卷注册 WIM 为干净原件并校验哈希；**失败即硬失败终止任务**，绝不静默产脏镜像 |
| 执行层入口提前还原 | **已回退（2026-09-28，不再采用）** | 曾把还原挪到 WinRE 入口，但因**与断电续跑冲突**而撤销：续跑隐式依赖「注册位=注入件」自动拉起 Recovery.exe，入口即换干净件会让续跑落进微软原版 WinRE。详见 [docs/20260928-073809-resume-vs-clean-winre-conflict.md](docs/20260928-073809-resume-vs-clean-winre-conflict.md) |
| 注册 WinRE 状态机（不循环 + 断电续跑 + 幂等） | 已落地（2026-09-28） | 会话期间注册位**保持注入件**；仅 DISM 捕获前翻干净（唯一权威纯净闸门）；新增 `ensure_registered_is_payload()` 在 resume 重武装之前确保注册位=注入件（已是则跳过，幂等）；续跑被压制分支恢复干净。设计见 [docs/20260928-074516-registered-winre-policy-and-resume-design.md](docs/20260928-074516-registered-winre-policy-and-resume-design.md) |
| 交叉编译 + 单测 | 通过 | Windows `aarch64-pc-windows-msvc` 构建通过（仅 LNK4099 缺 PDB 警告）；`cargo test -p backuprestore-core` 28 全绿。**VM 实机闭环（含断电续跑场景）尚未验证** |
| 执行层二次闸 `winre_role_conflict_at_execution` 随开关放开 F3 | 已落地 | `main.rs:1170`，`false` 可一键回退到旧拒绝 |
| 构建（macOS 交叉编译 ARM64） | v1.7.10 产物 `BackupRestore.exe` = 1,763,840B | `build-win.sh` 构建通过（仅 LNK4099 缺 PDB 警告，无害） |
| **VM 端到端验证（离线备份承载 RE 的卷 + 抽检镜像 WIM 哈希 + 迁回复原）** | **已通过（2026-09-28 实机闭环）** | 详见 [阶段2 文档 9.5.2.1](docs/winre-task-wim-phase2-2026-09-27.md)：P: 作承载 RE 的卷，prepare 无 F3 拒绝→重启 WinRE 捕获 18.9GB 镜像→挂载抽检 `\Recovery\WindowsRE\Winre.wim` 哈希 = `ORIGINAL_WINRE_SHA256`（1060a552…）→迁回 C: 复原 |

> 方案 D 让「离线备份承载 WinRE 的卷」不再被拒，结合 1.7.6 的 F1/F3 第一段逻辑，**产品主场景（系统盘离线备份/还原）在代码层面已解锁**；该解锁已于 2026-09-28 在 VM 内对「备份源 == 承载注册 WinRE 的卷」这一最坏组合实机闭环验证通过。

> **版本说明**：上表中「执行层入口提前还原」= v1.7.7 发布当晚追加的尝试（`fcb48a1`），**已于 v1.7.8 回退**；
> 「注册 WinRE 状态机」= v1.7.8 的正式做法。两者是一对互斥方案，请勿把前者当作现行逻辑。
> v1.7.8 另修复了一个**旧设计里同样存在、此前未被记录的缺口**：DISM 捕获期间（或干净件翻转过程中）断电时，
> 续跑重启会落进干净原版 WinRE 而没有 `winpeshl` 钩子 → `Recovery.exe` 不会自动拉起 → 任务续不起来。
> 现由 `ensure_registered_is_payload()` 在重武装之前保证注册位是注入件，把该漏洞与新引入的回归一并堵死。
> **v1.7.9（2026-09-28）**：修复 v1.7.8 实机验证抓到的续跑挂载 BUG（`volume has no disk number`，
> 根因=读取端丢弃 env 字段 + 挂载端单路依赖 DiskPart），并以「power-loss-window 注入 + original 覆写
> 构造死局」完成**断电续跑场景实机闭环验证 PASS**（修复→重武装→WinRE 自动跑完→success→终态还原干净）。
> 详见 [docs/20260928-093132-v179-fix-resume-mount-and-verify.md](docs/20260928-093132-v179-fix-resume-mount-and-verify.md)。

### v1.7.10（2026-09-29）：修复「日志宣称注册位已还原、实机注册位目录却是空的」

| 项 | 状态 | 证据 |
|---|---|---|
| 根因定位（迁出任务终态把干净原件写到了 RE 暂存卷，家卷自 `reagentc /disable` 后无人再写；`finalize_evacuated_winre` 被 `finalize_success` 挡成死代码；PE 内做不了 reagentc 重注册） | **已定位（代码 + 实机日志双证据）** | 见 [docs/20260929-103500-winre-finalize-wrote-to-scratch-not-home.md](docs/20260929-103500-winre-finalize-wrote-to-scratch-not-home.md) |
| 修复：`WINRE_HOME_*` 家卷身份 + WinRE 内写回家卷并落「待回家」标记 + 桌面 `reagentc` 重注册并用 `/info` 的 `harddiskN\partitionM` 复核 + 回收暂存卷 | 已落地 | `windows_prepare.rs` `write_recovery_env`；`main.rs` `restore_original_winre_at` / `finalize_winre_after_task` / `finalize_evacuated_winre` / `finish_pending_winre_rehome`；`text_parsing.rs` `reagentc_info_location` |
| 新增 CLI `winre-rehome`（桌面手动收尾，GUI 启动时自动跑同一逻辑） | 已落地 | `main.rs` `winre_rehome()` |
| 交叉编译 + 单测 | 通过 | `./build-win.sh` 产物 1,763,840B，SHA-256 `9077e5e5…a2bd`；`cargo test --workspace` 73 passed / 0 failed |
| 测试卷实机闭环（P: 承载注册 WinRE + 还原触发迁出 + 桌面重注册） | **PASS** | 家卷终态 `Winre.wim` 712,111,529B / `1060a552…`；`/info` 位置搬回 P:；暂存卷 `F:\Recovery` 已回收；幂等；非迁出任务行为无变化。证据 `.test-artifacts/winre-finalize-v10710/evidence.md`（未入库） |

### v1.7.10 + PoC（2026-09-29 下午）：「PE 式自建 BCD 条目」通道 PoC 通过

| 项 | 状态 | 证据 |
|---|---|---|
| PoC 命题：把原 `Winre.wim` 副本注入载荷放镜像卷、用**自建 BCD 条目**（独占设备选项对象 + 独占 osloader + 一次性 `bootsequence`）启动，全程不碰系统注册 WinRE | **通过** | 实机进 PE（`SYSTEMROOT=X:\windows`）、载荷钩子拉起、`Recovery.exe` 在 PE 内 RC=0；`C:\Recovery` 与 BCD 零改动。见 [docs/20260929-130000](docs/20260929-130000-pe-channel-poc-winre-wim-boots-from-image-volume.md)，证据 `.test-artifacts/pe-channel-poc/` |
| **修正旧结论**：拦住绕法的不是「启动 ramdisk 路径 == ReAgent 注册位置」，而是「**ReAgent 登记的那个 BCD 对象 + `reagentc /boottore` 的 bootstatus**」 | 已用 A/C1 对照实验证明 | 实验 A：直接 `bootsequence` 系统自带 WinRE 条目 → 8 秒被丢弃；实验 C1：复制该条目改指镜像卷副本 → 成功进 PE |
| 若改走新通道可整体删除的机制 | 已列清单 | 迁出 / `WINRE_HOME_*` / 待回家标记与桌面收尾 / Plan D / F1·F3 两道闸 / servicing 竞态（§3.1） |
| 改走新通道仍需自建的部分 | 已列清单 | BCD 条目生命周期、按卷 GUID 定位（PE 盘符会重排）、镜像卷 700MB、验收自动化（PE 内 `prlctl exec` 不可用）（§3.2） |
| 尚未验证 | 5 项 | 断电续跑 / BCDBoot 影响 / Secure Boot / 目标系统 WinRE 语义 / 纯 PE 形态（§3.3）——**待用户裁定后再开工** |
### ✅ 断电续跑闭环也 PASS（2026-09-30 03:16，v1.8.7 实机）

备份、还原、断电续跑三条都在新 PE 式通道里跑通了。

| 阶段 | 结果 |
|---|---|
| 故障点 | `power-loss-image-applied`（镜像灌完、启动项未修的最深断电点） |
| 中断落盘 | `stage=image-applied` / `progress=75`（不清理，模拟真实断电） |
| GUI 检测 | `Detected durable interrupted task … resuming` |
| 重武装 | `one-shot bootsequence re-armed for resumption` |
| 续跑完成 | `Boot entry cleaned; task marked successful` |
| 内容一致 | T: 还原后 **31,457,301 字节与镜像逐字节一致**，marker 全消失 |
| 终态 | BCD 无本项目对象、无 `bootsequence`、暂存目录已清空、注册位未动 |

这条路上连抓两个真缺陷：

1. **v1.8.5**：`create_entry` 按设计在 DISM 注入前跑，`boot-entry.json` 记的是注入前
   哈希，`rearm()` 永远拒绝重武装——**续跑此前对所有任务都是死的**。
2. **v1.8.7**：`restore-existing` 的 source 与 target 是**同一个分区**，被格式化后
   source 序列号也变，而 TARGET 早为此放行、SOURCE 却一直传 `false`。同一个分区
   两种标准，自相矛盾。

> ⚠️ **仍未闭环**：容量/性能（T: 5 GB 实占 54 MB，DISM 都是秒级）、
> `create-secondary`（双系统）、Secure Boot、新进度窗口（v1.8.5）还没在 PE 里实看、
> `power-loss-target-erased` / `power-loss-boot-repaired` 另两个断电点。


备份、还原两个方向都在新 PE 式通道里跑通了。

| 项 | 结果 |
|---|---|
| 还原后 T: 总字节 | **31,457,301** —— 与 `dism /Get-WimInfo` 报告的镜像大小**逐字节一致** |
| BCD 本项目残留 | ✅ 0 |
| 暂存目录 | ✅ `F:\BackupRestoreRE` 已删 |
| 注册位 | ✅ 未动（`harddisk0\partition4` / `b69adf69`） |
| 任务状态 | ✅ `success` / `100` |

本轮修掉的根因很值得记：**卷 GUID 有两种写法**（env 存裸 `{GUID}`，
`mountvol` 给 `\\?\Volume{GUID}\`），`verify_mounted_volume` 直接字符串比较，
于是**盘符挂成功了却判为"不是同一个卷"**，把所有候选盘符试一遍后任务失败。
错误信息 `every candidate drive letter is unavailable` 极其误导——听起来像盘符不够，
实际是每次都挂上了但比输了。修复用 `same_volume()` 先归一化再比较。

> ⚠️ **仍未闭环**：容量/性能（两次都只用 T: 这个 5 GB 卷、实占 54 MB，DISM 都是 1 秒级）、
> `create-secondary`（双系统）方向、断电续跑、BCDBoot（这次 target 无 SYSTEM hive 所以跳过）、
> Secure Boot、以及**真正的系统卷还原**（T: 是数据卷，走的是 data-volume 分支）。
### ✅ 新 PE 式通道第一次完整备份闭环 PASS（2026-09-30 00:28，v1.8.3 实机）

此前 11 个任务全停在 `prepared` 阶段——v1.7.11~v1.8.3 修的一直是**准备层**。
本轮第一次跑不带 `--no-reboot` 的 prepare，PE 内四步全完成：

```text
STEP 1/4 准备备份环境（挂载卷、校验）
STEP 2/4 捕获系统分区镜像（DISM）→ The operation completed successfully
STEP 3/4 校验镜像并计算哈希、写入元数据
Boot entry cleaned; task marked successful        ← 新通道特征（旧通道是 WinRE cleanup）
running wpeutil.exe reboot
```

| 终态核验 | 结果 |
|---|---|
| 镜像可用 | ✅ `F:\brimg\loop-t.wim` 258,200 B，`dism /Get-WimInfo` 可读，索引 1 |
| 元数据 | ✅ `imageSha256=6a3ee294…`、`capturedUsedBytes=54,374,400`、`programVersion=1.8.3` |
| BCD：bootsequence | ✅ 已清 |
| BCD：本项目条目 | ✅ 已删（`/enum all` 无 `BackupRestore` 字样） |
| 注册位 | ✅ **未动** —— `harddisk0\partition4` / `b69adf69` / Enabled |
| 回桌面 | ✅ `SYSTEMROOT=C:\Windows` |
| 任务状态 | ✅ `stage=success` / `progress=100` |

> ⚠️ **这次测得很轻，别当成容量/性能验证**：备份源 T: 是 5 GB 卷但实占仅 54 MB，
> 镜像 258 KB、DISM 一步 1 秒完成。**未验证**：大容量（v1.7.7 那次是 18.9 GB）、
> `restore-existing` / `create-secondary` 方向、断电续跑、BCDBoot、Secure Boot，
> 以及把镜像还原回去做内容比对。

### v1.8.3（2026-09-30 凌晨）：PE 侧也改零盘符 + 详细日志，方案 A 收尾

| 项 | 状态 |
|---|---|
| PE 侧三处 `mountvol S: /S` | ✅ 改零盘符优先 | `clean_bootsequence` / `add-secondary-entry` / `pe_task_execute` 都走 `esp_store_path_for_pe()`；失败自动回退挂盘符，行为不会更差 |
| 函数内 19 处 `> S:\xxx.txt` 重定向 | ✅ 统一改写 | `rewrite_s_root(cmd, esp_root)`；已挂盘符时原样返回 |
| `.done` 的 rename 目标 | ✅ 同卷 | 零盘符时 `task_file` 是卷路径，target 也必须是同一个卷路径 |
| 收尾卸载 | ✅ 只在真挂了才卸 | 零盘符路径下执行 `mountvol S: /D` 是无意义操作，还可能卸掉别人挂的卷 |
| 读不到配置时静默 return | ✅ 修 | 原来直接 `return false`，现场被抹掉；现在写下 `no config at <路径>` + detail |
| PE 侧实跑 | ⏳ 未验证 | 代码与单测已过；需在 PE 启动后验证 `pe-task.txt` 读写与三条动作 |
| 产物 | v1.8.3 = 1,800,704 B | SHA-256 `c454bcc7…5246b7` |

> `rewrite_s_root` 的单测**当场抓到一个真 bug**：第一版只把 `S` 换成卷根、没吃掉
> `S:` 后那个反斜杠，路径变成 `卷根\:` / `卷根\\`，Windows 直接不认。
> 这是"单测必须独立构造期望值、不能照实现抄"的又一次验证。
### v1.8.2（2026-09-29 深夜）：**方案 A 落地，临时盘符挂载降到 0**

| 项 | 状态 | 证据 |
|---|---|---|
| **一次准备任务的盘符挂载次数** | **3+ → 0** | 准备 backup T:→F: 前后盘符完全一致（`C D F H P T`），ESP 盘符一次都没分配 → 没有卷到达事件 → **不弹自动播放窗口、也不会有「不可访问」** |
| ESP BCD 读写 | ✅ 零盘符 | 日志 `plan A: verbatim BCD store path = \\?\Volume{GUID}\EFI\Microsoft\Boot\BCD` → `Captured byte-for-byte EFI BCD snapshot` |
| ramdisk_spec 修正 | ✅ 持续有效 | `readback device=ramdisk=[F:]\BackupRestoreRE\Winre.wim,{devopts}` |
| 零盘符读卷身份 / 定位 ESP | ✅ | `volume_identity_at_path` + 三个 `_at`；`efi_identity` 先试零盘符，失败才走 mountvol 循环 |
| 卷路径 → 设备路径 | ✅ | `volume_path_to_device_path()` —— 两者**不通用**，混用报 CreateFileW 161/123 |
| GUI 两处写 pe-task.txt | ✅ 零盘符 | 不再 `mountvol S: /S` |
| 清理与启动项终态 | ✅ | 自建条目已删、重启后残留 0、注册位仍是 `harddisk0\partition4` / `b69adf69`、`SYSTEMROOT=C:\Windows` |
| 产物 | v1.8.2 = 1,790,464 B | SHA-256 `041008f9…351d8f9` |

> 三个实机才暴露的坑（都加了独立期望值的单测）：**卷路径 ≠ 设备路径**（`\?\Volume{GUID}\` vs
> `\\.\Volume{GUID}`）、`?` 后少一个反斜杠、raw string 多一层反斜杠。它们在线码里只表现为
> `CreateFileW 161`，不给线索；`open_device` 报错现已带完整路径。
> **PE 侧三处 `mountvol S: /S` 仍未改**（PE 启动最早期没有 GUI 那套辅助函数），是下一阶段。

### v1.8.1（2026-09-29 深夜）：方案 A 门槛通过，ESP 日志迁出根目录

| 项 | 状态 | 证据 |
|---|---|---|
| **方案 A（卷 GUID 路径、零临时盘符）能否实施** | ✅ **能，阻塞已排除** | E3：两条路径 BCD 副本 SHA-256 完全相同 `976951da…`；E1：`set {bootmgr} default` 后 `/enum` 输出 `fc /b` 逐字节无差异（**写操作不做归一化**）；E2：`/set` `/create` `/delete` 全通过；E4：verbatim 卷路径可直接枚举 ESP 文件 |
| ESP 根 38 个日志的来源 | ✅ 查清 | 产品 `run_cmd_to_file(...) > S:\xxx.txt` 取证输出 + 我的探针；必须留根的只有 `pe-task.txt`/`.done`/`pe-task-result.txt` 三个控制通道 |
| 日志迁移 | ✅ | `text_parsing::esp_log_path()`：控制通道留 `S:\` 根，其余 47 处迁到 `S:\BackupRestore\logs\`；单测锁约定 |
| ESP 既有残留清理 | ✅ | 38 个 `.txt`/`.log` 全删（用 verbatim 卷路径，`del` 不吃这种路径）；复核 ESP 根只剩 `pe-exit-guid.txt` + `EFI\` + `System Volume Information`，`BCD_OK=True` |
| 快照合理清理 | ✅ | 7 → 2（用户授权），宿主可用 152 → 180 GiB |
| 产物 | v1.8.1 = 1,779,200 B | SHA-256 `605c32a0…760f1f16` |

> 一度像风险信号的"写后文件哈希不同"已定性：差异全在偏移 `0x30` 起的固件描述/路径缓存区，
> 属显示层缓存而非语义字段。`device` 显示 `partition=\Device\HarddiskVolume2` 只是因为
> 当时没挂盘符，同一次运行里挂着 S: 时两种路径都显示 `partition=S:`。

### v1.8.0（2026-09-29 晚）：主路径首次在产品内完整跑通，`mountvol` 退出码坑修掉

| 项 | 状态 | 证据 |
|---|---|---|
| **完整主路径 prepare（不带 `--test-efi-drive`）** | **PASS** | 任务 `ddce302b-…` 日志首行 `capturing bcdedit.exe /store S:\EFI\Microsoft\Boot\BCD`——产品自己找到 ESP 并挂载/卸载；`readback device=ramdisk=[F:]\BackupRestoreRE\Winre.wim,{devopts}`；`Task prepared with --no-reboot; registered WinRE unchanged`；终态盘符只剩 `C D F H P T`，无残留 |
| `mountvol X: /S` 退出码不可信 | ✅ 修复 | 实测 `Z:` 返回 1 但已挂上；`Y/X/W` 返回 0 但只是重复挂同一卷。**旧代码信这个码 → 每次换下一个盘符重挂 → 这就是"S 盘老是自动打开"的直接机制**。改用 `/L` 是否回显卷路径判定 |
| PE 任务收尾统一卸载 S: | ✅ 修复 | `pe_task_execute()` 跑完 result 已落盘后 `mountvol S: /D`，消掉"不可访问"另一半症状 |
| `letter as u8 as char` 静默截断 | ✅ 修复 | 改 `char::from_u32(...).filter(is_ascii_alphabetic)`，非法盘符记诊断而不是写错位置 |
| 提权会话通道 | ✅ 打通 | 计划任务 `/rl highest /it`，用户 `x`（SID 1000，High Mandatory Level）——这是唯一能跑 `mountvol /S` 的通道 |
| 清理与终态 | ✅ | BCD 残留 0、无 bootsequence、注册位仍是 `harddisk0\partition4` / `b69adf69`、重启回 `SYSTEMROOT=C:\Windows` |
| 产物 | v1.8.0 = 1,771,520 B | SHA-256 `d61a3995…6b0bc6` |

详见 [docs/202609292014-S盘自动打开问题定位与修复方案.md](docs/202609292014-S盘自动打开问题定位与修复方案.md)（含 21:0x 补测的退出码实证）。

> ⚠️ 仍未闭环：S 盘弹窗本身（方案 A：卷 GUID 路径零盘符）因 5.2 写路径归一化风险**尚未实施**；
> restore 方向、断电续跑、BCDBoot 交互、Secure Boot、还原目标 WinRE 注册语义也仍未验证。
> **不要把 v1.8.0 用于实机备份/还原**，VM 内可用版本仍是 v1.7.10。

### v1.7.12 ~ v1.7.14（2026-09-29 晚）：ProcMon 抓出真因，prepare 首次在产品内跑通

| 项 | 状态 | 证据 |
|---|---|---|
| v1.7.11 阻塞真因 | ✅ **定位并修复：不是进程上下文，是 `ramdisk=` 值畸形** | 正确形态 `ramdisk=[F:]\BackupRestoreRE\Winre.wim,{devopts}`（`[` 只包「卷」、`]` 紧跟卷闭合）。ProcMon `Process Create` 的 `Command line` 给出真实 argv；案例 A 一份 trace 同时含成功与失败，比原计划的 A/B 对照更严格 |
| v1.7.13 `{bootmgr}` 别名 | ✅ 修复 | 知名别名不是 GUID 形态，过不了 `require_guid`；改用白名单式 `require_identifier`（只有 `{bootmgr}`，仍拒 `{default}`/`{current}`/`{ntldr}`） |
| v1.7.14 `--no-reboot` 不再武装 | ✅ 修复 | 原来 `create_entry` 无条件 `/bootsequence`，`--no-reboot` 也会改 bootmgr；抽成显式 `arm_one_shot()` 由 `windows_prepare` 在载荷就绪后调用 |
| 单测位置修正 | ✅ | 原先断言错形态的单测在 **`#[cfg(windows)]` 模块里，macOS 上一次都不编译**；现已全部搬到 `text_parsing.rs`，`cargo test --workspace` 74 passed |
| **实机 prepare（`--operation backup --source-drive T --no-reboot --test-efi-drive Y`）** | **PASS** | `PREPARE_EXIT=0`、`boot-entry.json` 落盘、BCD 回读 `device`/`osdevice` 正好等于 `ramdisk_spec` 期望值；`--no-reboot` 时 `/enum {bootmgr}` 无 `bootsequence` |
| 清理与启动项终态 | ✅ 干净 | 本项目自建 BCD 对象全删（重启后复核残留 0）、注册位未动（`harddisk0\partition4` / `b69adf69`）、临时盘符 Y: 已移除、重启回 `SYSTEMROOT=C:\Windows` |
| 产物 | v1.7.14 = 1,769,472 B | SHA-256 `e34cde4c…a1d55d` |

完整过程、三次快照 ID、宿主磁盘/通道差异见
[docs/20260929-204000-ramdisk-spec-root-cause-and-no-reboot-armed.md](docs/20260929-204000-ramdisk-spec-root-cause-and-no-reboot-armed.md)。

> ⚠️ 本次 prepare 走的是 `--test-efi-drive Y`（绕开 SYSTEM 通道会挂起的 `mountvol /S`），
> **尚未验证产品自己跑完整 `mountvol /S` → ESP 定位的主路径**；restore 方向、断电续跑、
> BCDBoot 交互、Secure Boot、还原目标 WinRE 注册语义仍未闭环。**不要把 v1.7.14 用于实机
> 备份/还原**——VM 内可用版本仍是 v1.7.10（`H:\brwork\BackupRestore.exe`）。
> PoC 机制本身已验证可用（[docs/20260929-130000](docs/20260929-130000-pe-channel-poc-winre-wim-boots-from-image-volume.md)）。

### 尚未闭环（按严重度）

0. **待用户裁定的两点**（详见 [docs/20260929-103500](docs/20260929-103500-winre-finalize-wrote-to-scratch-not-home.md) §6）：
   ① 备份方向的「WinRE 迁出」闸门因方案 D 压制 F3 冲突而成为死代码——是否让备份也走迁出；
   ② 迁出态备份抓到的镜像里 `ReAgent.xml` 指向暂存卷、`\Recovery\WindowsRE` 是空目录——是否在终态
   一并校验/改写。
1. **C: 完整系统备份/还原仍未实机测试** —— 它是产品主场景，但按用户长期约束，开发/测试一律在
   测试盘进行，C: 只在最终验收时做一次。v1.7.7 方案 D 已对「备份源 == 承载注册 WinRE 的卷」这一
   最坏组合在 VM 内实机闭环验证通过（用的是 P: 测试卷作承载 RE 的卷，逻辑与备份 C: 完全相同），
   因此 **F3 机制本身已不再是障碍**；剩的只是「最终验收时对真实 C: 跑一次完整备份/还原」这一约定步骤。
2. **阶段 2 方案 D 已完成并实机闭环验证通过**（见上表 + 阶段2 文档 9.5.2.1）。PE 通道（`install_pe_ramdisk`，
   任意路径 ramdisk、无 ReAgent 校验）天生没有 F1/F3，本阶段不需要动 PE。
3. **Windows servicing 会静默冲掉部署的载荷** —— v1.7.6 载荷部署数小时后
   被累积更新替换为官方原版 WinRE，已重新注入恢复。任务准备阶段的载荷哈希校验因此不可省略，
   长期需要把任务环境与注册 WinRE 解耦。
4. **PE 载荷未同步** —— VM 内 `T:\petest\boot.wim` 是 2026-09-13 实验残留，
   宿主 `artifacts/BackupRestorePE.wim` 是另一条产物线。按已定路线（保持 WinRE 任务通道）
   **本阶段不需要动 PE**；先用户裁定「以哪个为准」再一次性同步。

### S 盘反复弹窗且「不可访问」（2026-09-29 已定位，修复方案待实施）

| 项 | 状态 |
|---|---|
| 现象 | 每次准备任务都弹出 `S:\` 文件夹窗口，随后提示无法访问/不可用 |
| 根因 | `mountvol S: /S` 给 ESP 分配盘符 → Windows 自动播放 `UnknownContentOnArrival → MSOpenFolder`（`InvokeVerb=open`）弹窗 → 程序用完 `mountvol S: /D` 撤销盘符 → 已打开的窗口失效 |
| 修复方向 | 改用卷 GUID 路径 `\\?\Volume{GUID}\EFI\Microsoft\Boot\BCD`，一次准备任务的临时盘符挂载降至 0 次（`bcdedit /store` 卷路径已在实机验证可读） |
| 阻塞门槛 | `/store` 写操作下 `device` 字段是否被归一化改写，需实机实验确认 |
| 详细证据与分阶段方案 | [docs/202609292014-S盘自动打开问题定位与修复方案.md](docs/202609292014-S盘自动打开问题定位与修复方案.md) |

### 其他已知限制

- **独立 EFI 首启动回恢复系统**：未验证（`0xc0430001`，已标记为 V1 已知限制，非交付门槛）。
- **Parallels Desktop 27.x 固件存在 ramdisk 引导回归**（6 秒复位循环，已 A/B 闭环证明，与本项目代码无关）。
  开发验证目前留在 **PD 26.4.2** 上做，注意关闭自动更新。

> 提醒：README 里的流程图描述的是**目标链路**，不是当前已验证链路。判断「做到了哪一步」
> 请以 `docs/verification-matrix.md` 的实机证据为准。

### 文档基线脱节（待修）

`docs/project-status.md` 停留在 **2026-09-24 / v1.6.6**，落后当前 1.7.6 六个版本；
`docs/current-status-2026-09-16.md` 更早。用它们判断当前进度会得出错误结论，
当前版本以 `VERSION` 和 `git log` 为准。

仓库：https://github.com/pixian5/backupRestore
