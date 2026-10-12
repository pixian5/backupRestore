# BackupRestore

Windows 一键系统备份还原工具（**开发测试版**）。

当前系统卷选择「进入 Windows RE」后，程序准备独立恢复镜像副本与任务启动项 → 重启进入 WinRE →
自动拉起 `Recovery.exe` 离线捕获/应用 WIM → 系统还原时修复引导 → 清理临时启动配置 → 重启回 Windows。
非当前系统卷由界面在线执行；预装 PE 是另一条简化路径，能力差异见完整流程文档。

当前仓库版本 **2.3.8**。2.3.8 为实机轮次：在线还原 STEP 4 引导修复**已实机通过**（`resume_object` 复原、主系统默认项保留、第二系统启动项入菜单）；**剩余「实际可启动」启动验收未做**。

> **⚠️ 跑实机测试前必读（磁盘空间硬规则）**：
> [Parallels 快照吃满磁盘根因与空间硬规则](./docs/20261012-1320-Parallels快照吃满磁盘根因与空间硬规则-GPT-5.md)
> —— 快照会把第二系统盘（~87GB）整份复制，拍 3 层就吃掉 265GB。
> **改 BCD 用 `bcdedit /export` 导文件代替拍快照；跑 87GB 镜像前保证 ≥220GB 空闲。**

> **文档导航（精简版）**：
> - 【**首选入口**】：[项目全景与知识浓缩](./docs/20261011-2350-项目全景与知识浓缩-Hy4.md) —— 定位、三条入口差异、架构代码地图、状态机与断电续跑、启动链硬约束与禁令、能力边界、已知缺陷，全部浓缩于此。**新会话先读它。**
> - 【当前开发进度】：详见[当前开发进度](./docs/20261011-2032-当前开发进度-GPT-6.md)（含 2.3.2、2.3.3 更正）。
> - 【下一步待实现】：在线还原 STEP 4 已通过，**剩「重启后从菜单进第二系统」的实际可启动验收**；目标卷 WinRE 注册待其首次启动后 `reagentc`（W09）；PE 作第二系统已有实机结论（剩余为决策项）；后续为 ESP 多候选、GUI 鼠标交互、夹具逐文件比对、客体残留清理。详见[下一步待实现](./docs/20261011-2032-下一步待实现-GPT-6.md)。
> - 【最新实机】：[在线还原 STEP4 引导修复实机通过](./docs/20261012-1345-在线还原STEP4引导修复实机通过-GPT-5.md)（**2.3.8 修复实证：`resume_object` 已复原、主系统默认项保留**）、[Parallels 快照吃满磁盘根因与空间硬规则](./docs/20261012-1320-Parallels快照吃满磁盘根因与空间硬规则-GPT-5.md)、[2.3.0 日志弹窗键盘焦点修复与复核](./docs/20261011-1940-2.3.0日志弹窗键盘焦点修复与复核-Fable5.md)、[PE 作第二系统实验结论](./docs/20261011-2140-PE作第二系统实验结论-Fable5.md)。
> - 【实机·系统卷/数据卷】：[系统卷与引导修复验收](./docs/20261010-2200-系统卷与引导修复验收-Opus5.md)（`BCDBoot` 已通过）、[新载荷路径实机验收](./docs/20261010-1910-新载荷路径实机验收-Opus5.md)（数据卷，跳过 `BCDBoot`）。
> - 【问题与方案】：[可能的问题GPT6.md](./docs/可能的问题GPT6.md)（14 项，已全部处理）、[可能的问题claude5.md](./docs/可能的问题claude5.md)（17 项 + W01～W12，**只设计未实施**）。
> - 【⚠️ 读历史文档前先看】：[代码与文档全量差异核对](./docs/20261012-0038-代码与文档全量差异核对-Hy4.md)——列出「文档说法 vs 当前代码」的差异，避免把旧版本说法当成当前行为。
> - 【完整目录】：根目录 [文档索引.md](./文档索引.md) 逐条列出全部文档；`docs/README.md` 为主题导航。
>
> ⚠️ **判读提醒**：版本号是标签不是文档 ID。2.2.2 同时挂在 `20261010-1910`（数据卷）和 `20261010-2200`（系统卷）上，覆盖范围须按**文档内容**判定，勿按版本号推断。

> 版本号规则（用户 2026-09-29 重申）：每次修改 +0.0.1，**每一位满十才进位**。
> 因此 `1.7.9` 之后是 `1.8.0`（第三位满十，进给第二位），不是 `1.7.10`。
> 历史上 `1.7.10`~`1.7.14` 是旧习惯记法，等价于 `1.8.0`~`1.8.4`；自 v1.8.0 起按规则进位。

---

## 一、产品流程

```text
正常 Windows
  └─ 打开 BackupRestore.exe，点「系统备份」/「系统还原」
       └─ 当前系统卷选择「进入 Windows RE」
            └─ prepare：校验 + 保存 BCD 副本 + 注入独立 WinRE 副本
                 └─ 镜像卷 BackupRestoreRE + 临时 BCD 条目 + 一次性启动
                      └─ 重启 → WinRE 自动启动 Recovery.exe
                           └─ 离线执行 DISM 捕获/应用 WIM
                                └─ 系统还原时 BCDBoot 修复引导
                                     └─ 清理任务启动项和临时副本
                                          └─ 重启回正常 Windows
```

当前准备流程只读系统注册的 WinRE，使用独立任务镜像启动；系统还原在 BCDBoot 后修复并核验目标 WinRE 注册和新加载器关联，终态只清理本任务临时对象。
在线还原仅应用镜像、预装 PE 自动还原固定索引 1，均不能等同 WinRE 完整任务链；具体历史路径差异见 [v1.9.9 流程快照](./docs/20260930-1925-备份还原完整流程-GPT-6.md)，当前行为须对照源码。

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
    ├── online_operation.rs 在线候选镜像事务、回读及副档同步
    ├── windows_command.rs  作业对象约束的命令与进程树管理
    ├── recovery_progress.rs WinRE/在线进度上报
    ├── winre_payload.rs    WinRE 载荷组织
    └── winre_registration.rs 系统还原后的注册与恢复关联核验
```

PowerShell / .NET 只出现在**测试脚手架**（`tools/win-clicker/`）和一次性探针（`.test-artifacts/`），
不进产品、不影响交付物。

---

在线追加先操作候选副本，需额外空间；失败保留候选及日志。正常完成要求镜像和副档回读都通过。
早期在线路径验收只覆盖独立目录部署；该历史轮次未执行格式化或重启。本轮 RE 系统验收见上方最新文档。历史详情见[修复与验证记录](./docs/20260930-2130-在线执行安全与副档一致性修复-GPT-6.md)。

## 三、构建

构建目标是 **Windows ARM64**（`aarch64-pc-windows-msvc`），静态链接 CRT，链接本机 `~/win-sdk-arm64`：

```bash
./build-win.sh
```

产物：`target/aarch64-pc-windows-msvc/release/BackupRestore.exe`
（增加 `--deploy` 才会部署到客体包目录及 `H:\brwork\`，分别更新
`BackupRestore.exe` 和 `Recovery.exe`，并逐一核对 SHA-256）。

构建规则、静态 CRT、Windows SDK 路径等细节见 `docs/20260819-2245-windows-build.md`。

---

## 四、目录说明

| 路径 | 内容 |
|---|---|
| `crates/` | 产品源码（Rust，唯一产品语言） |
| `docs/` | 流程、开发进度、操作记录及历史方案，索引见 `docs/README.md` |
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

- **首选入口**：[项目全景与知识浓缩](./docs/20261011-2350-项目全景与知识浓缩-Hy4.md)（新会话先读）
- **完整目录**（逐份说明）：根目录 [`文档索引.md`](./文档索引.md)
- **主题导航**：[`docs/README.md`](docs/README.md)

高频主题入口：

| 主题 | 文档 |
|---|---|
| 操作 VM 键鼠（自动化点击/按键/截图） | [`vm-input-control-guide`](./docs/20260927-0016-vm-input-control-guide.md) |
| ARM64 构建规则 | [`windows-build`](./docs/20260819-2245-windows-build.md) |
| WinRE 引导 / PD27 固件回归证明 | [`winre-boot-failure-parallels27`](./docs/20260925-0025-winre-boot-failure-parallels27.md) |
| S 盘弹窗 / unmount 竞态与卷路径改造 | [`S盘自动打开问题定位与修复方案`](./docs/20260929-2014-S盘自动打开问题定位与修复方案.md) |
| 历史开发路线图 | [`development-roadmap`](./docs/20260926-2337-development-roadmap.md) |
| v1.9.9 流程快照（**历史，非执行合同**） | [`备份还原完整流程`](./docs/20260930-1925-备份还原完整流程-GPT-6.md) |
| 完整需求原文 | [`Windows 一键系统备份还原 V1——完整开发需求.md`](Windows%20一键系统备份还原%20V1——完整开发需求.md) |

> 轮次快照（40 轮 × 2 份「当前开发进度/下一步待实现」）已归档到 `docs/90-archive/round-snapshots/`，
> 只在追溯某版本当时状态时查看，日常不必读。

---

## 七、历史开发进度（截至 2026-09-30）

> **已归档。** 2026-09-30 及更早的开发流水账（约 300 行：v1.8.x 各轮修复记录、S 盘弹窗排查、已知限制等）已整体移入
> [docs/90-archive/历史开发流水账-20260930.md](./docs/90-archive/历史开发流水账-20260930.md)，内容未作改写。
> 其中「尚未闭环」「待实施」、盘符、VM 版本和清理建议均为当时快照，**不应直接执行**；当前状态见本 README 上方进度入口。

仓库：https://github.com/pixian5/backupRestore
