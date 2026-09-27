# BackupRestore

Windows 一键系统备份还原工具（**开发测试版**）。

在正常 Windows 里点「备份」或「还原」，程序自动准备恢复任务 → 重启进入 WinRE → WinRE 自动拉起
`Recovery.exe` 离线执行 DISM 捕获/应用 WIM → 修复启动项 → 重启回正常 Windows。用户不需要做 U 盘、
进 BIOS、手动选 WinRE，也不需要敲命令。

当前版本 **1.7.7**（`VERSION`、两个 `Cargo.toml` 同步）。

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
- 下一步开发路线图 → [`docs/development-roadmap-2026-09-26.md`](docs/development-roadmap-2026-09-26.md)
- 完整需求原文 → [`Windows 一键系统备份还原 V1——完整开发需求.md`](Windows%20一键系统备份还原%20V1——完整开发需求.md)

---

## 七、当前进度（截至 2026-09-27）

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
| 执行层入口提前还原（方案 D 触发时机前移） | 已落地（2026-09-28） | `main.rs` WinRE 入口（`WinreRestoreGuard` 建立、task 加载后）即调用 `restore_original_winre` 把注册 WIM 覆写回干净原件，F3 污染窗口在入口闭合；捕获前 + 结尾两道保留作幂等安全网 |
| 执行层二次闸 `winre_role_conflict_at_execution` 随开关放开 F3 | 已落地 | `main.rs:1170`，`false` 可一键回退到旧拒绝 |
| 构建（macOS 交叉编译 ARM64） | 产物 `BackupRestore.exe` = 1,697,792B | `build-win.sh` 修复 `rust-lld` 路径后构建通过，字节与方案 D 落地前一致（仅新增 `#[cfg(test)]` 测试） |
| **VM 端到端验证（离线备份承载 RE 的卷 + 抽检镜像 WIM 哈希 + 迁回复原）** | **已通过（2026-09-28 实机闭环）** | 详见 [阶段2 文档 9.5.2.1](docs/winre-task-wim-phase2-2026-09-27.md)：P: 作承载 RE 的卷，prepare 无 F3 拒绝→重启 WinRE 捕获 18.9GB 镜像→挂载抽检 `\Recovery\WindowsRE\Winre.wim` 哈希 = `ORIGINAL_WINRE_SHA256`（1060a552…）→迁回 C: 复原 |

> 方案 D 让「离线备份承载 WinRE 的卷」不再被拒，结合 1.7.6 的 F1/F3 第一段逻辑，**产品主场景（系统盘离线备份/还原）在代码层面已解锁**；该解锁已于 2026-09-28 在 VM 内对「备份源 == 承载注册 WinRE 的卷」这一最坏组合实机闭环验证通过。

### 尚未闭环（按严重度）

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
