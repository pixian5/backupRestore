# BackupRestore

Windows 一键系统备份还原工具（**开发测试版**）。

当前系统卷选择「进入 Windows RE」后，程序准备独立恢复镜像副本与任务启动项 → 重启进入 WinRE →
自动拉起 `Recovery.exe` 离线捕获/应用 WIM → 系统还原时修复引导 → 清理临时启动配置 → 重启回 Windows。
非当前系统卷由界面在线执行；预装 PE 是另一条简化路径，能力差异见完整流程文档。

当前仓库版本 **2.3.9**。**在线还原到第二个 Windows 卷全流程实机通过**：STEP 1→4（含新增 STEP 3.5 盘符绑定）+ 重启后第二系统桌面正常（零崩溃）+ WinRE 已注册 + 主系统未受影响。
2.3.9 新增【引导修复】：还原第二阶段起，进度窗出现「引导修复」按钮，用户可勾选要加入启动菜单的系统、排序、改名、设菜单秒数（**无倒计时**，本次还原系统必选，其它默认全选），选择落盘后于映像应用完成时一次性写入 BCD 并回读校验。

> **⚠️ 跑实机测试前必读（磁盘空间硬规则）**：
> [Parallels 快照吃满磁盘根因与空间硬规则](./docs/20261012-1320-Parallels快照吃满磁盘根因与空间硬规则-GPT-5.md)
> —— 快照会把第二系统盘（~87GB）整份复制，拍 3 层就吃掉 265GB。
> **改 BCD 用 `bcdedit /export` 导文件代替拍快照；跑 87GB 镜像前保证 ≥220GB 空闲。**

> **文档导航（精简版）**：
> - 【**首选入口**】：[项目全景与知识浓缩](./docs/20261011-2350-项目全景与知识浓缩-Hy4.md) —— 定位、三条入口差异、架构代码地图、状态机与断电续跑、启动链硬约束与禁令、能力边界、已知缺陷，全部浓缩于此。**新会话先读它。**
> - 【当前开发进度】：详见[当前开发进度](./docs/20261011-2032-当前开发进度-GPT-6.md)（含 2.3.2、2.3.3 更正）；2.3.9 引导修复见[2.3.9 引导修复与 VM 原生编译路径](./docs/20261012-1510-2.3.9引导修复与VM原生编译路径-Fable5.md)。
> - 【下一步待实现】：**引导修复实机验收**（按钮出现时机、弹窗键盘操作、BCD 五项回读）——功能已接完但未跑实机；其余为 PE 作第二系统决策项、ESP 多候选、GUI 鼠标交互、夹具逐文件比对、客体残留清理，以及「还原后是否预置登录凭据」待用户拍板。详见[下一步待实现](./docs/20261011-2032-下一步待实现-GPT-6.md)。
> - 【最新实机】：[在线还原第二系统全流程通过](./docs/20261012-1505-在线还原第二系统全流程通过-GPT-5.md)（**2.3.8 收尾：桌面正常、零崩溃、WinRE 已注册**）、[2.3.9 引导修复与 VM 原生编译路径](./docs/20261012-1510-2.3.9引导修复与VM原生编译路径-Fable5.md)（含**中文 bcdedit 字段名缺陷修复**与 VM 内编译踩坑）、[根因修正-在线路径漏调盘符绑定](./docs/20261012-1435-根因修正-在线路径漏调盘符绑定-GPT-5.md)、[Parallels 快照吃满磁盘根因与空间硬规则](./docs/20261012-1320-Parallels快照吃满磁盘根因与空间硬规则-GPT-5.md)、[灌入WinRE镜像建第二系统两次失败根因与VM并发干扰](./docs/20261012-1755-灌入WinRE镜像建第二系统两次失败根因与VM并发干扰-Step-5-Preview.md)（**「载荷被删」结论已被 1935 文档修正为隐藏属性误判；仍保留并发会话证据**）、[新启动通道盘符解析回归根因与修复方向](./docs/20261012-1935-新启动通道盘符解析回归根因与修复方向-Step-5-Preview.md)（**根因：bootmgr 只解析系统卷 C: 盘符，数据卷暂存必失败；C: 手动条目已实测进 WinRE；10-11 同配置可用=今天回归；修复方向=暂存改放系统卷**）。
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

构建目标是 **Windows ARM64**（`aarch64-pc-windows-msvc`），静态链接 CRT。

> **📌 用户 2026-10-12 决定：后续一律在宿主机（macOS）交叉编译，不走 VM 内编译。**
> 理由：快一倍（8.16s vs 15.24s）、不依赖客体开机与共享目录在线。
> VM 内编译只在需要"编完立刻实机验证"时偶发使用，不是常规路径。

### 常规构建（宿主机）

```bash
./build-win.sh              # 只构建
./build-win.sh --deploy     # 构建 + 部署到 VM + 逐一核对 SHA-256
```

产物：`target/aarch64-pc-windows-msvc/release/BackupRestore.exe`

**产物怎么进 VM**：通过 Parallels 共享文件夹（Host Shared Folders）。
宿主 `/Users/x/code/backupRestore` 被设为共享，客体侧用 **UNC 路径
`\\Mac\backupRestore`** 访问——不用盘符 `X:`，因为 `prlctl exec` 默认走
SYSTEM 账户（Session 0），看不到用户会话的盘符映射，而 UNC 在 SYSTEM 下可读写。
部署由客体侧 `copy` 命令从 UNC 路径拉到两个目标目录（包目录
`C:\Users\Public\backupRestore-package\` 与运行目录 `H:\brwork\`），
每个目录都要同时刷新 `BackupRestore.exe` 与 `Recovery.exe`，
最后逐一比对 4 个 SHA-256 才算成功。

### VM 内原生编译（备选，仅实机验证需要时）

客体内执行，**`+crt-static` 必须显式带**：

```bash
set "CARGO_TARGET_DIR=C:\BackupRestoreBuild\target" && set "RUSTFLAGS=-C target-feature=+crt-static" && cargo build --release --manifest-path X:\Cargo.toml --target aarch64-pc-windows-msvc
```

> ⚠️ **别手打裸 `cargo build`**：漏了 `+crt-static`，产物会依赖
> `VCRUNTIME140.dll` 与 `api-ms-win-crt-*`，进不了精简 WinRE/PE
> （退出码 0xC0000135）。2026-10-12 实测犯过这个错，靠核对 DLL 依赖列表才发现。

两条路径的实测对比（同版本、同 rustc 1.98.1：时间 8.16s vs 15.24s、
产物大小差 16KB、两边均 14 个 DLL 且零 CRT 外部依赖）见
[macOS 交叉编译与 VM 内原生编译实测对比](./docs/20261012-1640-macOS交叉编译与VM内原生编译实测对比-Fable5.md)。

构建规则、静态 CRT、Windows SDK 路径等细节见 `docs/20260819-2245-windows-build.md`；
VM 内编译环境与踩坑见 [2.3.9 引导修复与 VM 原生编译路径](./docs/20261012-1510-2.3.9引导修复与VM原生编译路径-Fable5.md)。

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
