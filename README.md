# BackupRestore

[![CI](https://github.com/pixian5/backupRestore/actions/workflows/ci.yml/badge.svg)](https://github.com/pixian5/backupRestore/actions/workflows/ci.yml)

Windows 一键系统备份还原工具（**开发测试版**）。

当前系统卷选择「进入 Windows 恢复环境」后，程序准备独立恢复镜像副本与任务启动项 → 重启进入恢复环境 →
自动拉起 `Recovery.exe` 离线捕获/应用镜像 → 系统还原时修复引导 → 清理临时启动配置 → 重启回 Windows。
非当前系统卷由界面在线执行；预装预安装环境是另一条简化路径，能力差异见完整流程文档。

**版本号不写在这里**——它必然过期（2026-10-29 审计时本行还停在 2.6.69，实际已是 2.6.77）。
当前版本一律读 [`VERSION`](./VERSION) 文件，或看程序窗口标题；每一版的改动与验收判据见
[`文档索引.md`](./文档索引.md) 顶部「当前与权威」。

> **接手开发先读** [`文档索引.md`](./文档索引.md) 顶部（现状、版本、最新一轮做了什么），
> 再读 [`经验/README.md`](./经验/README.md)（结论库与引用规则）。
> 本机还有一个 `AGENTS.md`（开发验证硬约束）——★ **它已 gitignore、不入库**，clone 后没有这个文件，
> 其内容以本 README §五 的「十条硬约束」为准，**不要链接它**（会造成死链）。
> 需要历史背景时再看 [`docs/20261026-2245-接手AI交接说明-v2.6.63-deepseek-4.1-flash.md`](docs/20261026-2245-接手AI交接说明-v2.6.63-deepseek-4.1-flash.md)——
> ★ **它是 2.6.63 时点的快照，已落后多个版本**，只能当背景，不能当现状。

> 版本号规则（用户 2026-11-05 裁定，统一口径）：**改源代码就 +0.0.1** —— 包括结构搬迁、
> 重构与行为改动；只有纯文档、判据工具、说明类改动不升版。**每一位满十才进位**：
> `1.7.9` 之后是 `1.8.0`，不是 `1.7.10`。三处同步：`VERSION` + 两个 `Cargo.toml`，
> 改完跑 `cargo check` 刷 `Cargo.lock`；版本读 `VERSION` 或看程序窗口标题。
> 历史上 `1.7.10`~`1.7.14` 是旧习惯记法，等价于 `1.8.0`~`1.8.4`；自 v1.8.0 起按规则进位。

---

## 【当前开发进度】

**v2.10.13 已完成工程加固与全面实机回归；v2.10.14 已修复实机暴露的 WinRE DISM 32MB 暂存空间写满（Error 112）与报错弹框超屏问题并完成 VM 部署及首批核验，现按指令暂停其余实机复验（2026-11-27 11:35）。**

1. **v2.10.13 工程加固与缺陷修复（`0085ff6`，VM 二进制 `1ece100e…`）**：
   - `winre_registration.rs` 移除最后两处 `.expect()`，达成 `crates/` 生产代码 `0 unwrap / 0 expect`；
   - `native_gui.rs` 新增 `SingleInstanceGuard` 命名互斥量单实例保护；
   - `abandon-task` 改两步显式反馈（任务清理与 BCD 补偿分开呈现）；`hash_image_logged` 改为两位小数且在线备份同步写出 `backend=` 日志；抽离 `gui_dispatch_plan.rs` 纯分派单测；无副档镜像改优先免挂载 `wimlib dir`；补全 `gui-data-restore.sh online-format`；`reverify-map.py` 全量覆盖 `83` 个 `.rs` 并接入 CI；清理 53 个零散旧文件及 98 个过期构建产物；
   - 修复 [`tools/gui-check/ps/click.ps1`](./tools/gui-check/ps/click.ps1) 未做临时置顶与窗口坐标归属校验导致点击穿透到后台「设置」触发浏览器的问题（新增 `CLICK=COVERED` 守卫）。
2. **v2.10.14 WinRE 热修（`8041efa`，VM 部署件 `9dcffb29…`）**：
   - `process_runner::DismScratchDir` 将 WinRE 内 DISM `/ScratchDir`、`/LogPath` 与 `TEMP`/`TMP` 重定向至镜像所在卷或任务目录（接入 `recovery_backup.rs` 与 `recovery_engine.rs`），绕过基线 `Winre.wim`（`10.0.26100.9968`）仅 `32MB` RAM 盘引起的 `Error: 112`；
   - `failure_policy::summarize_prompt_error` 收敛超长 8 KiB 错误尾部，防止 WinRE 报错弹窗把「重试 / 取消」按钮顶出 `1024×768` 屏幕底边。
3. **验证与现状快照（如实列出）**：
   - **`v2.10.13`（`1ece100e…`）全套实机**：`vm-unit.sh`（`479 passed`）、`ignored-acceptance`（4/4）、`cli-probe`、`data-guard`、`mutation-guard`、`gui-fix-check`、`gui-choice-text`、`gui-negative`（9/9）、`gui-backup cancel/online`、`gui-data-restore`（离线+在线+在线格式化）、`pause-backup/restore`、`orphan-race` 与 **`C:` 系统盘离线还原全链（`1c1164cf…`）** 全 PASS；`gui-matrix.sh` 40 组合在 `12pt/14pt` 全绿（`16pt/18pt` 在 `1024×768` 下存在底溢/裁剪）；`gui-backup.sh system` 于 `26200.9974` 暴露上述两项问题并由 `2.10.14` 修复。
   - **`v2.10.14`（`9dcffb29…`）已完成**：宿主机 `448` 单测、WinARM64 检查与 Core clippy、`source-guard`、`selftest`、`reverify-map --selftest`（83 `.rs`）、`rebuild-deploy.sh`（三方哈希一致、`PREFLIGHT=pass`）、`vm-unit.sh v21014 --allow-known`（`481 passed, 10 ignored, 2 known`）、`mutation-guard`（3/3）、`cli-probe` 全 PASS；其余实机复验（`gui-backup.sh system`、`win-restore`、`power-resume`、按 B 回退及打包验收）按指令暂停并保留干净 VM（`running`、`0` 快照、宿主空闲 `313 GiB`）。

**交付**：见 [`文档索引.md`](./文档索引.md) 顶部状态行与 [`20261127-1135` 完整记录](./docs/20261127-1135-v2.10.13-v2.10.14工程加固实机回归与WinRE暂存修复-Gemini.md)。详细版 → [当前开发进度（详细）](./docs/20261127-1135-当前开发进度-Gemini.md)。

## 【下一步待实现】

1. **恢复后补齐 `v2.10.14` 实机复验与发包**（对照 `reverify-map.py`）：
   - 跑通 `BR_SKIP_IDLE=1 bash tools/vm/gui-backup.sh system` 实证 `DismScratchDir` 消除 WinRE `Error: 112`；
   - 拍 1 个临时 VM 快照补跑完整 `C:` 系统盘离线还原（验证 `/Apply-Image` 暂存与引导修复）后立即删快照；
   - 补跑 `failure_policy` 相关的断电续跑 / 按 B 回退与 `pause-restore` / `orphan-race` / `gui-data-restore` 回归，并完成 `pack-dist.sh` + `release-check.sh --baseline dist/BackupRestore-2.10.12-windows-arm64.zip` 与 `verified/vm-20261127` 基线标签。
2. **修复小屏大字号（`16pt/18pt`）布局溢底/裁剪**：为 `1024×768` 下的高字号主窗口页签（`1101..1106`）与设置窗补齐紧凑排布或滚动支持。
3. **如条件具备补目验 `PE` 桌面重启链**：在独立分区安装 PE 后实机跑通 `restore-data -> PE` 重启环（目前由 `no_write_on_reject` VHD+DISM 与单测覆盖）。

**逐项依据 →** [下一步待实现（详细）](./docs/20261127-1135-下一步待实现-Gemini.md)、[v2.10.13–v2.10.14 实机回归与修复报告](./docs/20261127-1135-v2.10.13-v2.10.14工程加固实机回归与WinRE暂存修复-Gemini.md)。

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

当前准备流程只读系统注册的恢复环境，使用独立任务镜像启动；系统还原在 BCDBoot 后修复并核验目标恢复环境注册和新加载器关联，终态只清理本任务临时对象。
在线还原仅应用镜像、预装预安装环境自动还原固定索引 1，均不能等同恢复环境完整任务链；具体历史路径差异见 [v1.9.9 流程快照](./docs/20260930-1925-备份还原完整流程-GPT-6.md)，当前行为须对照源码。

### V1 支持的四种任务

| 任务 | 作用 | 破坏性 |
|---|---|---|
| `probe` | 只验证任务、载荷、卷挂载和恢复环境自动入口 | 否 |
| `backup` | 从现有 Windows 分区用 DISM 捕获镜像 | 否（写入指定镜像卷） |
| `restore-existing` | 还原到原 Windows 分区并修复引导 | 是 |
| `create-secondary` | 还原到独立分区并加入第二启动项（双系统） | 是 |

### V1 范围边界

**支持**：Windows 10/11、UEFI + GPT、NTFS 系统分区、镜像、Windows 自带恢复环境（也支持自定义预安装环境）、
DISM 作为镜像引擎、BCDBoot/启动配置数据作为启动管理。

**明确不做**（不要擅自扩展成完整 Ghost/预安装环境工具）：分区布局重构、网络备份、增量/差异镜像、
Legacy BIOS、自动修改 BitLocker。

**实现限制**：第二系统盘符绑定的 `secondary_registry::gpt_mapping()` 只支持 GPT 盘
（blob 为 `DMIO:ID:` + PARTUUID 小端 16 字节）；**MBR 盘（12 字节 blob：磁盘签名 + 分区偏移）走不了这条链**，
`repair()` 会因 GUID 解析失败而报错。与「UEFI + GPT」的支持边界一致，但接手方在 MBR 机器上要知道这是硬边界，
不是可降级的路径。机制详见 [盘符与MountedDevices-经验总结](./docs/盘符与MountedDevices-经验总结.md)。

---

## 二、代码结构

产品代码 **全部是 Rust**，两个 crate。依赖只有 `serde` / `chrono` / `serde_json` /
`sha2` / `thiserror` / `uuid` —— **没有任何 .NET/dotnet/C# 运行时依赖**。
**具体行数不写在这里**（它必然过期：本段曾写「约 1.66 万行」，2026-10-29 核对时生产代码已约 3.2 万行）；
要数量就 `wc -l`。

```
crates/
├── backuprestore-core/     库：任务模型、载荷契约、元数据、SHA-256、错误类型
└── backuprestore-cli/      可执行：原生 GUI + 准备 + WinRE 恢复环境
    ├── main.rs             CLI 入口与参数分发（解耦后高内聚调度器，~1,600 行）
    ├── native_gui/         原生 Win32 GUI 自治子系统（从旧巨石解耦为 11 个子模块，~2,500 行）
    │   ├── task_executor.rs     任务调度执行与互斥原子门禁
    │   ├── pe_desktop.rs        WinRE/WinPE 独立恢复桌面与软硬件信息
    │   ├── pe_installer.rs      PE 引导安装、BCD 注册与独立重启
    │   ├── environment.rs       环境探测、提权与常用系统工具拉起
    │   ├── operation_tabs.rs    5 大入口路由与页签布局
    │   └── ...                  （驱动选择、动态算宽、镜像/证据/日志窗口等）
    ├── recovery_engine.rs  恢复宿主状态机、DISM/wimlib 还原流转与连续失败逃生
    ├── recovery_volume.rs  恢复卷挂载、盘符分配、GPT 身份复核与 DiskPart 格式化
    ├── recovery_bcd.rs     BCD 引导维护与双系统菜单编排
    ├── pending_task.rs     开机中断任务接管探测与防反复重启双层闸门
    ├── process_runner.rs   通用外部进程派生、超时控制与日志管道
    ├── prepare_plan.rs     准备阶段纯计算逻辑与跨平台防御单测（macOS 可单测）
    ├── windows_prepare.rs  正常 Windows 侧准备：校验/注入载荷/设置一次性 WinRE 启动
    ├── text_parsing.rs     命令行输出本地化解析（纯逻辑，macOS 可单测）
    ├── online_operation.rs 在线候选镜像事务、回读及副档同步
    ├── windows_command.rs  作业对象约束的命令与进程树管理
    ├── recovery_progress.rs WinRE/在线进度上报与 SafeLock 崩溃免疫
    ├── winre_payload.rs    WinRE 载荷组织
    └── winre_registration.rs 系统还原后的注册与恢复关联核验
```

PowerShell / .NET 只出现在**测试脚手架**（`tools/win-clicker/`）和一次性探针（`.test-artifacts/`），
不进产品、不影响交付物。

---

在线追加先操作候选副本，需额外空间；失败保留候选及日志。正常完成要求镜像和副档回读都通过。
早期在线路径验收只覆盖独立目录部署；该历史轮次未执行格式化或重启。本轮恢复环境系统验收见上方最新文档。历史详情见[修复与验证记录](./docs/20260930-2130-在线执行安全与副档一致性修复-GPT-6.md)。

## 三、构建

构建目标是 **Windows ARM64**（`aarch64-pc-windows-msvc`），静态链接 CRT。

> **📌 用户 2026-10-15 拍板：构建完全走虚拟机内 Windows（唯一路径，macOS 侧不再是发布路径）。**
>
> 沿用 2026-10-12 的实测依据：macOS 侧 clean 全量构建只快 1.8 倍（4.60s vs 8.24s，绝对差 3.6 秒），
> 对开发循环没有意义；而代价是质变的——**macOS 有 59% 的代码根本编译不到**
> （8 个 `#[cfg(windows)]` 模块共 15696 行），那 138 项绿灯对它们证明力为零。
> 2026-10-12 引导修复实机验证时，**每一个阻断缺陷都在 macOS 编译不到的文件里**。
> 2026-10-15 同轮修复：`windows/build-windows.ps1` 的 `Invoke-Cargo` 此前未设
> `+crt-static`（该 flag 只存在于 macOS 侧 `build-win.sh`），虚拟机内路径产物缺静态 CRT——
> 现已幂等补齐，静态链接 CRT 是本路径的必需组成部分。

### 常规构建 + 部署 + 核对（虚拟机内，一条命令）

```powershell
# 在 VM 的 Windows 11 里执行
& X:\windows\build-windows.ps1 -Architecture arm64 -Deploy
```

一条命令完成：构建 → 打包 → 部署到两个运行目录 → 逐个核对 4 个 SHA-256。
不部署就去掉 `-Deploy`。

部署目标与判据：

| 目标 | 定位 | 文件 |
|---|---|---|
| **唯一部署位 `H:\brwork\`** | 所有启动入口都从这里起；任务存储随之落在 `H:\brwork\tasks` | **运行必需四件**：`BackupRestore.exe` + `Recovery.exe` + `wimlib-imagex.exe` + `libwim-15.dll`。另有两件非运行必需：`BackupRestore.settings.ini`（首次保存设置时生成）与 `winpe-winpeshl.ini`（部署脚本会拷，程序只在备份排除表里列它，**缺失不影响运行**） |

> ⚠️ **不再往 C: 部署 exe**（2026-10-24 用户裁定）：历史部署位
> `C:\Users\Public\backupRestore-package` 停止写入，`build-win.sh --deploy` 与
> `windows/build-windows.ps1 -Deploy` 每次部署都会清掉其残留。原因：备份 C: 会把
> 该目录连旧 exe 一起打进镜像，A 入口还原 C: 后自启 GUI 从那里带回旧版二进制
> （2026-10-24 实测自启 v2.6.31 而 H: 里是 v2.6.32）；自启项（用户 `Run` 键
> `BackupRestoreTest`）同轮已改指 `H:\brwork\BackupRestore.exe`，重启实测自启
> 进程 PATH = H:。

> ⚠️ **任务目录跟着"从哪启动"走**（2026-10-24 用户裁定「TaskStore 搬离 C:」）：
> TaskStore 的根 = 可执行文件所在目录，任务目录 = `<运行目录>\tasks`。
> 从 C: 包目录起 → 任务落回 C:，会被 A 入口还原系统盘一起回滚，还会让镜像把旧任务状态带回来。
> 所以启动一律走 `H:\brwork`；包目录的 `\tasks` 已于 2026-10-24 清空（4 个终态任务迁入 `H:\brwork\tasks`）。

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
>
> **在虚拟机里跑 Windows 原生单元测试、以及各条实机用例**：`tools/vm/`（[说明](./tools/vm/README.md)）。
> `tools/vm/vm-unit.sh <标签> <测试名过滤串>` 跑定向测试；`tools/vm/mutation-*.sh` 做对应测试的变异验证；
> 改了产品代码先问 `python3 tools/test/reverify-map.py --since <基线标签> --worktree`，输出里每一项都写着该跑哪个脚本。

### push 门禁（本地 pre-push 钩子，P2-a）

```bash
git config core.hooksPath tools/git-hooks   # 一次性安装（per-clone）
```

安装后每次 `git push` 自动跑 **7 步**（步骤以 `tools/git-hooks/pre-push` 头注释为准）：
`cargo test --workspace`、core clippy `-D warnings`、错误码清单同步、`./build-win.sh` 交叉编译、
Windows **测试**目标编译、文档链接入库检查、**源码静态守卫**（挂盘符必须带租约）。
失败即拒绝 push；`git push --no-verify` 可强行跳过。发布级门禁（构建+部署+冒烟）是
`tools/smoke/deploy-and-smoke.sh`，按需手动跑。

云端另有 GitHub Actions（`.github/workflows/ci.yml`，徽章见文首），覆盖同样的 4 步
（不依赖本机 Windows SDK 的那部分），并在 **windows-latest 原生**跑一遍全部
`cfg(windows)` 单测——这是 mac 上完全看不见的一半。分工：本地钩子负责「推之前 30 秒
就知道」，CI 负责「换机器/交接后判据仍然存在」。

工具链固定在仓库根的 `rust-toolchain.toml`（当前 1.98.1，含 `aarch64-pc-windows-msvc`）。
CI 会断言 runner 的版本与它一致，漂移即红——升级工具链要改这个文件、本地
`rustup update`、处理新出现的 lint，三件事一起提交（2026-10-27 第一次跑 CI 就吃了
「runner 已升到 1.99、本机还是 1.98」的亏）。

### macOS 侧（已废止为发布路径，仅限本地快速类型检查）

用户 2026-10-15 拍板「编译完全走虚拟机内 Windows」。macOS 侧 `./build-win.sh`
**不再用于构建/部署发布产物**；仅保留供本地快速确认 `backuprestore-core` 等跨平台
代码"没把公共库改崩"（`cargo check` 级用途），产物不得部署、不得作为发布版。
历史实测对比（两边产物均 14 DLL、零 CRT 外部依赖、大小差 16KB/89.6KB）见
[macOS 交叉编译与虚拟机内原生编译实测对比](./docs/20261012-1640-macOS交叉编译与VM内原生编译实测对比-Fable5.md)。
**macOS 的绿灯对 `#[cfg(windows)]` 模块（图形用户界面/准备/恢复/在线/预安装环境/boot_entry）零证明力。**

> ⚠️ **`+crt-static` 不能漏**：漏了产物会依赖 `VCRUNTIME140.dll` 与 `api-ms-win-crt-*`，
> 进不了精简恢复环境/预安装环境（退出码 0xC0000135，症状=零输出零副作用）。虚拟机内
> `build-windows.ps1` 已幂等内置该 flag；手打裸 `cargo build` 才会踩到。

构建规则、静态 CRT、Windows 软件开发工具包路径等细节见 `docs/20260819-2245-windows-build.md`；
虚拟机内编译环境与踩坑见 [2.3.9 引导修复与虚拟机原生编译路径](./docs/20261012-1510-2.3.9引导修复与VM原生编译路径-Fable5.md)。

> **环境是否齐全**（2026-10-13 实测，两条路径都实跑过）：见
> [macOS 与虚拟机编译工具链核查](./docs/20261013-1725-macOS与虚拟机编译工具链核查-DeepSeek-V4.1-Flash.md)。
> 结论：工具齐全、无必需下载；macOS 与虚拟机三处 Rust 已统一为 **rustc 1.98.1**；
> **虚拟机两个 profile 的 cargo 源已配 USTC 镜像**（含可直接迁移的 4 行 TOML 与实证）。
> 另记两个入口坑：`X:` 只在虚拟机交互会话存在（SYSTEM 通道要用 UNC `\\Mac\backupRestore`），
> 且 `if exist \\Mac\backupRestore\NUL` 恒为假、不能用来判共享可达。

---

## 四、目录说明

| 路径 | 内容 |
|---|---|
| `crates/` | 产品源码（Rust，唯一产品语言） |
| `docs/` | 流程、开发进度、操作记录及历史方案，索引见 `文档索引.md` |
| `tools/win-clicker/` | 测试脚手架：操控虚拟机键鼠的自动化通道（PowerShell，不进产品） |
| `tools/vm/` | 实机用例脚本（守卫竞态、命令行冒烟、暂停、哨兵法还原、界面专项、界面矩阵、虚拟机内单元测试与变异验证、构建部署、夹具清理）；每个脚本自己给结论。见 [`tools/vm/README.md`](./tools/vm/README.md) |
| `tools/gui-check/` | 界面几何 / 文字 / 提示覆盖的判据（`run.sh`）与界面辅助脚本（`ps/`），以及三道确认的驱动 `restore-confirm.sh` |
| `tools/release/pack-dist.sh` | 打可分发的 Windows ARM64 交付包到 `dist/`（`dist/` 已 gitignore）。**只认虚拟机内原生构建产物** `artifacts/windows/…`（mac 交叉编译产物不得作为发布版），补齐 wimlib 两文件与手册、生成 `README.txt` 与 `SHA256SUMS.txt`；`--local` 仅供本地看包结构。详见 [交付包收口](./docs/20261026-0849-交付包收口dist追上2.6.57-deepseek-4.1-flash.md) |
| `build-win.sh` | macOS 交叉构建（**已废止为发布路径**，仅限本地 `cargo check`；构建唯一入口是虚拟机内 `windows/build-windows.ps1 -Deploy`） |
| `artifacts/` | 预安装环境构建产物（`BackupRestorePE.iso/wim`、`BaseWinPE.iso`） |
| `pe-assets/` | 预安装环境源镜像 / SDI |
| `windows/` | Windows 侧辅助脚本与 `winpeshl.ini` |
| `target/` | Rust 构建产物（8G+，不入库） |
| `.test-artifacts/` | 测试证据与一次性探针（5G+，已 gitignore） |
| `VERSION` | 当前版本号 |
| `AGENTS.md` | **本机指令文件，不入库**（`.gitignore` 标注「含本机凭据」）。开发验证硬约束：改引导走文件级备份、不拍整机快照；空间纪律；判据纪律；发布路径唯一。**clone 后没有这个文件，以本 README §五 为准** |

---

## 五、开发约束（必读）

1. **改引导不拍整机快照，改用文件级备份**。修改启动配置数据、默认项、一次性启动序列、恢复环境注册或
   预安装环境部署之前，先 `bcdedit /export <普通路径>` 导出 BCD 存储，并用 `bcdedit /store <文件> /enum {bootmgr}`
   核验可离线读取；导出失败则手抄 `{bootmgr}` 的 `default`/`displayorder`/`resumeobject` 三项兜底
   （只能手抄、无完整性校验）。**备份失败不得继续。**
   **导出→回导整链已实测**（2026-10-26）：把 `timeout` 从 10 改成 3 → `bcdedit /import` → 回到 10 → **重启仍正常引导**，
   非固件对象与基线逐行一致。★ 回导成败看**值有没有真的回来**，不看退出码。
   （完整版见本机 `AGENTS.md` 第二节——该文件不入库；**clone 出来的仓库以本条为准**。）
2. **启动成功 ≠ 备份还原成功**。验证要跑到 DISM 实际完成、启动项修复、回正常 Windows 为止。
3. **会重写卷的测试必须在可回滚的整机快照里做**（`restore-existing` / `create-secondary` 这类，失败代价是整卷丢失）；**只是改引导项的验证走第 1 条的文件级备份，不占快照**。
   （2026-11-24 注：**拍不拍快照以本机 `AGENTS.md` 第一节「空间纪律」为准**——纯备份不拍、改启动配置走文件级备份、
   全程最多 1 个、空间不够就不拍且无快照也必须能回滚。本条要求的是「必须有可回滚手段」，不是「每次都拍快照」；
   10-12 吃满宿主磁盘的事故正是快照留多了造成的。）
4. **大文件下载前先查网络**。热点环境必须逐次取得用户授权。
5. 每次改动后：本地测试 → Windows ARM64 构建 → 部署 → 核对哈希。
6. **改完行为要扫一遍「陈旧断言」**（2026-10-29 审计后新增）。跑 `tools/doc/stale-claims.sh`：
   它扫 `经验/`、本 README、`AGENTS.md` 里四类词——「至今未修」「待拍板」「没有门禁」「无实机记录」——
   因为这些词一旦写着没改，就代表**有人会去重复劳动，或者以为某道保护并不存在**。
   该工具**只报告、恒 PASS、不进 pre-push 门禁**（一直红的检查最后会被整个删掉，比没有更坏，
   见 `经验/06` §6）。读它先看 `STALE_SCOPE=` 与 `STALE_HITS=` 两个字段：**空集也要能分辨「真没有」和「扫描没跑」**。
   （完整版见本机 `AGENTS.md` §五——该文件不入库；**clone 出来的仓库以本条为准**。）
7. **新写下的结论必须带「适用前提」**（同上新增）。形如「一律」「都」「永远」的断言，
   要么补上前提、要么去实测。反例是 `经验/03` 里「`bcdedit /enum` 会把两种写法都显示成 `[X:]`」——
   漏了「该卷当前有盘符时」这个前提，于是和同一节的实测结果自相矛盾。
   （完整版见本机 `AGENTS.md` §五——该文件不入库；**clone 出来的仓库以本条为准**。）
8. **当前版本号只写一处：`文档索引.md` 顶部那一行**，每次发版同步它。**其他文档一律不写**——
   这个数字必然过期：2026-10-29 审计时，`经验/06` 还写着 2.6.25、本 README 横幅还写着 2.6.69，
   而 `VERSION` 实际已是 2.6.77。别处需要版本就读 `VERSION` 文件，或看程序窗口标题。
   留这一处例外的理由：版本总得有个家，**人人都写才是错**；集中一处，发版只改一行，忘改一眼能看出。
9. **虚拟机设置不许动**（用户 2026-11-24 明令）。对测试虚拟机**不执行任何 `prlctl set`**——空闲自动暂停、
   显示缩放、启动顺序、安全启动、设备增删都算，「临时关掉、用完再恢复」也不做。要用就自己开
   （关机状态 `prlctl start`，已暂停或已休眠 `prlctl resume`），用完 `prlctl suspend`，
   休眠前先跑 `tools/vm/user-idle.sh` 确认没人在用（不要用 `quser` 的空闲时长：它对本地控制台会话恒显示 `none`，分不出
   有人在敲和没人碰过，2026-11-24 实测）。
   任务被「空闲自动暂停」停住时只恢复运行、接着跑，**不去关这个功能**。
   **为什么单独立一条**：这个做法在 `经验/02` 里从 10-09 起就写成了「处置办法」，2026-11-24 有两个会话照着各关了一次，
   第二次被中断后没有恢复，被用户发现并禁止。凡在旧文档里看到「关 pause-idle」「用命令行切缩放」，一律按**已作废**处理。
   经过与订正清单见 [`docs/20261124-1645-虚拟机设置不许改动-新明令与文档订正-GLM-5.4.md`](./docs/20261124-1645-虚拟机设置不许改动-新明令与文档订正-GLM-5.4.md)。
   （完整版见本机 `AGENTS.md` 第七节——该文件不入库；**clone 出来的仓库以本条为准**。）
10. **「清理 / 收口」类逻辑必须能分辨「正在被别的进程使用」**（2026-11-25 实机）。
    界面程序每次启动先跑的「孤儿任务守卫」曾把**正在准备中的任务**当孤儿清掉：它的三条判据（非终态、没写目标卷、
    没有启动项记录）对在办任务全中，而注释里写着「判据极窄、不可能误伤」。**注释里的「不可能误伤」是需要证明的断言。**
    判断依据要来自操作系统持有的东西（独占打开的句柄：共享冲突 = 有人在办，进程一死系统自动放手），
    不要记进程号（会复用），不要看「多久没更新」（阈值定不准）。现行实现：准备流程握住任务目录里的 `preparing.lock`，
    守卫与两条放弃入口遇到在办任务一律不动（放弃命令返回 `BR-TSK-036`）。
    经过见 [`docs/20261125-0010-孤儿任务守卫误清在办任务的复现与修复`](./docs/20261125-0010-孤儿任务守卫误清在办任务的复现与修复-v2.10.9-v2.10.10-GLM-5.4.md)。
11. **判读的硬规矩**（2026-11-25，都是实机踩出来的；完整版见本机 `AGENTS.md` 第四节）：
    ① **「对不上号」不等于「没发生」**——准备失败时命令行输出不带任务编号，按编号去对守卫日志恒为 0；
    从动作方自己的日志行里取编号，再拿被动的那个任务的日志起始时间去对；事后的状态文案会被后一次写入覆盖，
    判据要认当时留下的那一份。② **证据文件名带版本号，每个版本只在它的最终二进制上下结论**（两轮同名会互相覆盖）。
    ③ **变异验证的判读也要自检**：不能锚定行首（输出带缩进），不能拿手工模拟冒充变异（给函数加开关，关掉就是旧行为）。
    ④ **界面控件文字是系统本地编码**（常驻提权助手回传的就是）：脚本比对前逐行严格解码，否则会把「取消」认成别的控件；
    `tools/gui-check/ps/launch.ps1` 会先结束所有名为 `BackupRestore` 的进程，而命令行准备进程同名。
12. **构建与通道的三条**（2026-11-25）：① **先提交、后部署**（部署前置检查的新鲜度判据是「部署件修改时间 ≥ 最新提交时间」）；
    新写的 Windows 专属代码与测试在本机用 `--target aarch64-pc-windows-msvc --all-targets` 检查（连测试目标一起；
    `RUSTFLAGS` 只对那一条命令生效，不要导出）。② **共享盘 `X:` 的映射不跨重启**：依赖它的脚本开头自己映射并读回 `X:\VERSION`；
    命令里不要出现空参数（`-Filter ""` 会被命令行解析吃掉，PowerShell 报「参数缺值」直接退出）；
    长任务启动后**立刻核对它真的在跑**（日志文件出现了没有、进程在不在），启动回显不算数。
    ③ **你自己的自动化输入也算键鼠输入**：连着跑第二个界面脚本会被「有没有人在用」的检查拦下，等空闲满 120 秒，
    不要调低阈值。
13. **修复要在报出缺陷的那条路径上原路复验；「读不到」会伪装**（2026-11-25 晚，完整版见本机 `AGENTS.md` 第四节与 `经验/05`、`经验/02`）：
    ① 在界面上报的缺陷，修复后只验命令行**不算验过**——界面的参数是自己拼的、准备进程是另行拉起的
    （界面全链用例：`tools/vm/gui-data-restore.sh`）。② 「读不到」会伪装成四种样子：**不在**（在 / 不在 / 读不到是三种结果，
    走 `tools/vm/lib/common.sh` 的 `guest_exists`）、**旧的那一个**（认定「新出现的」必须与之前的读数比对）、
    **没执行**（客户端进程还在等 = 命令在跑；客户端已退出且输出为空、再等几秒也没有新任务目录，才是没执行；
    杀掉宿主上的客户端**停不掉虚拟机里的命令**）、**历史**（数整份累积日志；按任务编号划界，或驱动前读一次虚拟机自己的时钟）。
14. **动界面之前先截屏；「点击已执行」不等于点到了目标程序**（2026-11-25 深夜，完整版见本机 `AGENTS.md` 第七节与 `经验/02` §24）：
    「真实鼠标点击」落在屏幕坐标上**最上层的窗口**。用户开着「设置」时，新启动的界面程序停在它后面，两次回报成功的点击
    落进了「设置」（设置项没被改动只是运气）。`tools/gui-check/ps/uiclick.ps1` 现在点击前先把目标窗口提到最上层，
    再核对「这个坐标下是不是目标窗口」，不是就回报 `CLICK=COVERED`、不点——**不要绕过这道守卫**；
    要紧的步骤再看界面日志里有没有对应的行。另：经 `-Extra` 传给常驻助手的参数串**末尾不能是冒号**；
    查看脚本输出只落文件、按字节处理（`LC_ALL=C`），否则外层管道报「非法字节序列」会把正在跑的脚本连带弄断。
15. **读代码发现的缺陷，先在旧二进制上复现再修**（2026-11-25 深夜）：同一个用例在旧二进制上必须红、修好后必须绿
    （`tools/vm/gui-backup.sh cancel`：2.10.10 两项全失败，2.10.11 两项全通过）。先修后验的话，判据恒绿而不自知。
    **往测试卷写大文件前先看宿主空闲**：测试卷所在的虚拟磁盘在宿主上按需增长，客体里写多大、宿主上的磁盘文件就涨多大。

> **C: 不是禁区**（2026-10-12 用户明确删除旧「不触碰 C:」约束）：`C:` 可作测试源与测试目标，
> 也放编译产物（`C:\br-target`）。唯一的硬限制来自产品自身——**代码拒绝把当前活动系统盘当在线还原目标**
> （报 `active Windows volume must be processed offline`），系统卷还原必须走重启进恢复环境的离线路径。

---

## 六、文档索引

全部文档的逐份说明与主题导航**只有一处**：根目录 **[`文档索引.md`](./文档索引.md)**。

- 新会话先读它：顶部是项目全景入口、当前权威文档与本版改动；
- 正文 §一 按时间倒序（找「最新」）、§二 按主题分组（找「某领域」）；
- `docs/` 各子目录、`经验/` 经验库、根目录文件都在它里面。

## 七、历史开发进度（截至 2026-09-30）

> **已归档。** 2026-09-30 及更早的开发流水账（约 300 行：v1.8.x 各轮修复记录、S 盘弹窗排查、已知限制等）已整体移入
> [docs/90-archive/历史开发流水账-20260930.md](./docs/90-archive/历史开发流水账-20260930.md)，内容未作改写。
> 其中「尚未闭环」「待实施」、盘符、虚拟机版本和清理建议均为当时快照，**不应直接执行**；当前状态见 [`文档索引.md`](./文档索引.md)。

仓库：https://github.com/pixian5/backupRestore
