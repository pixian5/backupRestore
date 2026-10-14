# BackupRestore

Windows 一键系统备份还原工具（**开发测试版**）。

当前系统卷选择「进入 Windows RE」后，程序准备独立恢复镜像副本与任务启动项 → 重启进入 WinRE →
自动拉起 `Recovery.exe` 离线捕获/应用 WIM → 系统还原时修复引导 → 清理临时启动配置 → 重启回 Windows。
非当前系统卷由界面在线执行；预装 PE 是另一条简化路径，能力差异见完整流程文档。

当前仓库版本 **2.5.3**：完成「还原入口三分」+「格式化目标分区」选项覆盖 A/B/C 三入口×在线离线全通道（默认勾选并校验格式化成功；取消=覆盖式应用）——「单系统还原」拆为 **还原系统到 C 盘 / 还原数据到非 C 盘 / 还原系统到非 C 盘** 三个入口（tab 总数 6，**入口名称为用户拍板原文案不得改动**），语言栏靠右显示，底层收尾路由复用 2.4.9 机制零改动，详见[还原入口三分轮文档](./docs/20261014-1253-还原入口三分UI拆分A通道B数据C他盘-GLM-5.md)与[文案修正轮](./docs/20261014-1315-入口文案改回全称与语言栏右对齐-GLM-5.md)。

2.4.3 复核文档：[复核另一会话改动](./docs/20261013-1036-复核另一会话改动-预检被放行致P卷被毁与恢复-Haiku4.5.md)；2.4.2 待办五项全做记录见[2.4.2 轮文档](./docs/20261013-0340-2.4.2-待办五项全做-门禁措辞PE预检ESP多候选鼠标验收与清理-Opus5.md)。

上一版本 2.4.1。**在线还原到第二个 Windows 卷全流程实机通过**：STEP 1→4（含新增 STEP 3.5 盘符绑定）+ 重启后第二系统桌面正常（零崩溃）+ WinRE 已注册 + 主系统未受影响。
**2.4.1 引导修复 GUI 前台实机验收通过**：测试钩子全自动链路（无人工点击）跑完 STEP 1→4，界面弹出「执行完成」，BCD 五项独立回读全部通过（主系统默认项、resumeobject、displayorder、timeout、菜单名）。此前连报四轮的“原有启动菜单项丢失”根因是被删旧 loader 以**退出码 0 + 空文案**返回、被误判为丢失外来项，详见[连续还原三轮同错根因](./docs/20261013-0152-连续还原三轮同错根因-被删旧loader误判为菜单丢失-Opus5.md)与[验收通过记录](./docs/20261013-0219-引导修复GUI前台实机验收通过-Opus5.md)。
2.3.9 新增【引导修复】：还原第二阶段起，进度窗出现「引导修复」按钮，用户可勾选要加入启动菜单的系统、排序、改名、设菜单秒数（**无倒计时**，本次还原系统必选，其它默认全选），选择落盘后于映像应用完成时一次性写入 BCD 并回读校验。

> **⚠️ 跑实机测试前必读（磁盘空间硬规则）**：
> [Parallels 快照吃满磁盘根因与空间硬规则](./docs/20261012-1320-Parallels快照吃满磁盘根因与空间硬规则-GPT-5.md)
> —— 快照会把第二系统盘（~87GB）整份复制，拍 3 层就吃掉 265GB。
> **改 BCD 用 `bcdedit /export` 导文件代替拍快照；跑 87GB 镜像前保证 ≥220GB 空闲。**

> **文档导航（精简版）**：
> - 【**首选入口**】：[项目全景与知识浓缩](./docs/20261011-2350-项目全景与知识浓缩-Hy4.md) —— 定位、三条入口差异、架构代码地图、状态机与断电续跑、启动链硬约束与禁令、能力边界、已知缺陷，全部浓缩于此。**新会话先读它。**
> - 【当前开发进度】：**阶段 1-4 全部实机通过**（1a/1b/1c/2/3/4，v2.5.7 `e04fa92`）——阶段 4 为 C 入口**在线通道首次实机**（全项 + 一次性 bootsequence 启动验收，`docs/20261015-0010`）；**唯一剩余入口验证 = 阶段 5（A 入口·还原系统到 C 盘）**。本轮另产出 10-14 事故**文档可信度地图**（`docs/20261015-0021` §2）：1900/1904 的「三个实锤缺陷」全部误判、2315 的「唯一失败点」叙事是 post hoc 重建（主因是 v2.5.4 的 resumeobject bug，2.5.5 已修）、1535/1552 的「快照嫌疑」不能成立为结论——**引用 10-14 文档前先查该表**。详见[当前开发进度](./docs/20261015-0021-当前开发进度-Sonnet5.md)。
> - 【下一步待实现】：①**阶段 5（A 入口全链）**——清 T: 哨兵 → 拍快照 + 抄 BCD 三行 → 纯净 GUI 走 A 入口 → 两道确认真实弹出 → 三件套验收 → 删快照；②P3 文案；③P4 字面 'C'；④P5 WinRE 注册降级；⑤「安装 PE」端到端补账；⑥清理决策（镜像索引 1 去留等，问用户）。详见[下一步待实现](./docs/20261015-0021-下一步待实现-Sonnet5.md)。
> - 【最新实机】：[在线还原第二系统全流程通过](./docs/20261012-1505-在线还原第二系统全流程通过-GPT-5.md)（**2.3.8 收尾：桌面正常、零崩溃、WinRE 已注册**）、[2.3.9 引导修复与 VM 原生编译路径](./docs/20261012-1510-2.3.9引导修复与VM原生编译路径-Fable5.md)（含**中文 bcdedit 字段名缺陷修复**与 VM 内编译踩坑）、[根因修正-在线路径漏调盘符绑定](./docs/20261012-1435-根因修正-在线路径漏调盘符绑定-GPT-5.md)、[Parallels 快照吃满磁盘根因与空间硬规则](./docs/20261012-1320-Parallels快照吃满磁盘根因与空间硬规则-GPT-5.md)、[灌入WinRE镜像建第二系统两次失败根因与VM并发干扰](./docs/20261012-1755-灌入WinRE镜像建第二系统两次失败根因与VM并发干扰-Step-5-Preview.md)（**「载荷被删」结论已被 1935 文档修正为隐藏属性误判；仍保留并发会话证据**）、[新启动通道盘符解析回归根因与修复方向](./docs/20261012-1935-新启动通道盘符解析回归根因与修复方向-Step-5-Preview.md)（**根因：bootmgr 只解析系统卷 C: 盘符，数据卷暂存必失败；C: 手动条目已实测进 WinRE；10-11 同配置可用=今天回归；修复方向=暂存改放系统卷**）。
> - 【实机·系统卷/数据卷】：[系统卷与引导修复验收](./docs/20261010-2200-系统卷与引导修复验收-Opus5.md)（`BCDBoot` 已通过）、[新载荷路径实机验收](./docs/20261010-1910-新载荷路径实机验收-Opus5.md)（数据卷，跳过 `BCDBoot`）。
> - 【问题与方案】：[可能的问题GPT6.md](./docs/可能的问题GPT6.md)（14 项，已全部处理）、[可能的问题claude5.md](./docs/可能的问题claude5.md)（17 项 + W01～W12，**只设计未实施**）。
> - 【⚠️ 读历史文档前先看】：[代码与文档全量差异核对](./docs/20261012-0038-代码与文档全量差异核对-Hy4.md)（基线 2.3.3）与 [docs 文档与源代码可能的差异](./docs/20261013-1230-可能的差异-GLM-5.2.md)（**基线 2.4.4，最新**）——列出「文档说法 vs 当前代码」的差异，避免把旧版本说法当成当前行为。
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

**实现限制**：第二系统盘符绑定的 `secondary_registry::gpt_mapping()` 只支持 GPT 盘
（blob 为 `DMIO:ID:` + PARTUUID 小端 16 字节）；**MBR 盘（12 字节 blob：磁盘签名 + 分区偏移）走不了这条链**，
`repair()` 会因 GUID 解析失败而报错。与「UEFI + GPT」的支持边界一致，但接手方在 MBR 机器上要知道这是硬边界，
不是可降级的路径。机制详见 [盘符与MountedDevices-经验总结](./docs/盘符与MountedDevices-经验总结.md)。

---

## 二、代码结构

产品代码 **全部是 Rust**，两个 crate，约 1.66 万行。依赖只有 `serde` / `chrono` / `serde_json` /
`sha2` / `thiserror` / `uuid` —— **没有任何 .NET/dotnet/C# 运行时依赖**。

```
crates/
├── backuprestore-core/     库：任务模型、载荷契约、元数据、SHA-256、错误类型
└── backuprestore-cli/      可执行：原生 GUI + 准备 + WinRE 恢复环境
    ├── native_gui.rs       原生 Win32 GUI（8325 行）
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

构建目标是 **Windows ARM64**（`aarch64-pc-windows-msvc`），静态链接 CRT。

> **📌 用户 2026-10-12 决定：构建与测试的主路径都在 VM 内（Windows 侧）。**
>
> **推翻同日早先"一律宿主机交叉编译"的决定。** 原因是实测发现那个决定的核心理由
> 不成立：clean 后全量构建 macOS 只快 1.8 倍（4.60s vs 8.24s，绝对差 3.6 秒），
> 对开发循环没有意义；而代价是质变的——**macOS 有 59% 的代码根本编译不到**
> （8 个 `#[cfg(windows)]` 模块共 15696 行），那 138 项绿灯对它们证明力为零。
> 2026-10-12 引导修复实机验证时，**每一个阻断缺陷都在 macOS 编译不到的文件里**。

### 常规构建 + 部署 + 核对（VM 内，一条命令）

```powershell
# 在 VM 的 Windows 11 里执行
& X:\windows\build-windows.ps1 -Architecture arm64 -Deploy
```

一条命令完成：构建 → 打包 → 部署到两个运行目录 → 逐个核对 4 个 SHA-256。
不部署就去掉 `-Deploy`。

部署目标与判据：

| 目标 | 文件 |
|---|---|
| 包目录 `C:\Users\Public\backupRestore-package\` | `BackupRestore.exe` + `Recovery.exe` + `winpe-winpeshl.ini` |
| 运行目录 `H:\brwork\` | `BackupRestore.exe` + `Recovery.exe` |

**复制成功不算数**，判据只有"四个文件的 SHA-256 与刚构建的成品逐个相等"。
2026-09-30 踩过「只更新包目录、运行目录还是旧件」的坑，唯一症状是镜像元数据里的
`programVersion` 偏低，静态检查看不出来。

可用 `-CargoTargetDir` / `-OutputRoot` / `-DeployPackageDir` / `-DeployWorkDir` 覆盖默认路径。

### 跑测试

```powershell
cargo test --manifest-path X:\Cargo.toml            # 全量
cargo test --manifest-path X:\Cargo.toml -p backuprestore-cli
```

> ⚠️ **macOS 侧 `cargo test` 的绿灯不能作为这些代码的验证依据。**
> `online_operation`、`native_gui`、`windows_prepare`、`windows_command`、
> `recovery_progress`、`pe_operation`、`boot_entry`、`recovery_fault`
> 八个模块都是 `#[cfg(windows)]`，macOS 上静默跳过。
> 改到它们必须在 Windows 侧跑一遍测试——macOS 抓不到的错包括类型不匹配、
> borrow 检查、以及本会话全部 7 个阻断缺陷。

### macOS 交叉编译（保留，仅作快速 sanity check）

```bash
./build-win.sh              # 只构建
./build-win.sh --deploy     # 构建 + 部署 + 核对（经 UNC 路径 \\Mac\backupRestore）
```

只在改 `backuprestore-core` 这类**跨平台**代码、需要快速确认"没把公共库改崩"时用。
产物质量与 Windows 侧等同（两边均 14 个 DLL、零 CRT 外部依赖、大小差 16KB），
实测对比见
[macOS 交叉编译与 VM 内原生编译实测对比](./docs/20261012-1640-macOS交叉编译与VM内原生编译实测对比-Fable5.md)。
**它不能替代 Windows 侧测试。**

> ⚠️ **无论哪条路径，`+crt-static` 都不能漏**：漏了产物会依赖
> `VCRUNTIME140.dll` 与 `api-ms-win-crt-*`，进不了精简 WinRE/PE
> （退出码 0xC0000135）。`build-windows.ps1` 与 `build-win.sh` 都已内置，
> 手打裸 `cargo build` 才会踩到。

构建规则、静态 CRT、Windows SDK 路径等细节见 `docs/20260819-2245-windows-build.md`；
VM 内编译环境与踩坑见 [2.3.9 引导修复与 VM 原生编译路径](./docs/20261012-1510-2.3.9引导修复与VM原生编译路径-Fable5.md)。

> **环境是否齐全**（2026-10-13 实测，两条路径都实跑过）：见
> [macOS 与虚拟机编译工具链核查](./docs/20261013-1725-macOS与虚拟机编译工具链核查-DeepSeek-V4.1-Flash.md)。
> 结论：工具齐全、无必需下载；macOS 与 VM 三处 Rust 已统一为 **rustc 1.98.1**；
> **VM 两个 profile 的 cargo 源已配 USTC 镜像**（含可直接迁移的 4 行 TOML 与实证）。
> 另记两个入口坑：`X:` 只在 VM 交互会话存在（SYSTEM 通道要用 UNC `\\Mac\backupRestore`），
> 且 `if exist \\Mac\backupRestore\NUL` 恒为假、不能用来判共享可达。

---

## 四、目录说明

| 路径 | 内容 |
|---|---|
| `crates/` | 产品源码（Rust，唯一产品语言） |
| `docs/` | 流程、开发进度、操作记录及历史方案，索引见 `docs/README.md` |
| `tools/win-clicker/` | 测试脚手架：操控 VM 键鼠的自动化通道（PowerShell，不进产品） |
| `build-win.sh` | macOS 交叉构建 + 部署 + 哈希核对（**已降级为快速 sanity check**，主路径是 `windows/build-windows.ps1 -Deploy`） |
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
