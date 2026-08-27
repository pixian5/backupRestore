# 2026-08-25 全项目系统性审计

## 审计范围

审查了 Rust core/GUI/Recovery、Windows 打包和启动载荷、历史批处理/PowerShell、文档、测试脚本、运行时调用链、Windows-only 编译边界、程序目录工作区和 C: 破坏性边界。

## 发现与处理

| 严重度 | 根因 | 处理 |
|---|---|---|
| P0 | GUI 是 Rust，但创建任务仍通过 `powershell.exe -> BackupRestore.exe prepare`；这使“Rust 前端”与实际运行架构不一致。 | 新增 Rust `prepare`，GUI 使用 `ShellExecuteW` 直接提升并启动自身；删除产品包中的 `BackupRestore.exe prepare`。 |
| P0 | GUI 环境刷新、卷枚举、WIM 读取和任务状态读取仍调用 PowerShell。 | 新增 Rust `list-volumes`、`inspect-environment`、`wim-info`；GUI 改为调用自身 CLI。 |
| P0 | `Recovery.cmd` 曾包含 DISM Capture/Apply、格式化和 BCDBoot 回退，造成 Rust Recovery 缺失时仍可绕过统一状态机。 | 删除破坏性批处理实现；WinRE 只允许 `Recovery.exe recover-env`。 |
| P1 | 卷身份依赖本地化 DiskPart/PowerShell 对象，曾可能产生伪造的磁盘/分区身份。 | Rust 使用 `GetDiskFreeSpaceExW`、`GetVolumeInformationW`、`DeviceIoControl` 读取 GPT 身份和分区类型；无法完整读取时拒绝。 |
| P1 | Windows-only GUI 分支未在 macOS 测试中编译，数组长度、字符串转义等错误只能到 ARM64 才暴露。 | 每轮修改增加 ARM64 构建门槛；本轮已实际暴露并修复多个 Windows-only 编译错误。 |
| P1 | 任务创建顺序曾先建目录再调用原子 TaskStore，导致合法任务被“目录已存在”拒绝。 | 改为先由 TaskStore 创建任务和 task/status，再创建 payload 子目录。 |
| P1 | `RecoveryTask.env` 曾重复写入原始 WinRE hash，Rust 重复键校验会拒绝任务。 | 由 manifest 统一保存 hash，环境文件只保存一次键值。 |
| P1 | 程序目录所在卷与目标卷的阻止只按盘符/不完整身份判断。 | GUI 和 Rust prepare 均按卷/分区身份判断；PowerShell 路径已移除。 |
| P2 | 文档、历史测试脚本仍描述 PowerShell 后端、任务卷和 Recovery.cmd 兼容路径。 | 当前架构文档已更新；历史证据保留但明确为历史，不作为当前运行入口。 |

## 当前运行时边界

产品运行时只包含：

- Rust `BackupRestore.exe`：Win32 GUI、环境/卷/WIM 查询、任务准备；
- Rust `Recovery.exe`：WinRE 载荷校验、Capture/Apply、BCDBoot、清理和状态机；
- `winpeshl.ini`：直接从 WinRE 启动 Rust `Recovery.exe recover-env`；
- Windows inbox `dism.exe`、`bcdedit.exe`、`bcdboot.exe`、`reagentc.exe`、`diskpart.exe`、`shutdown.exe`：由 Rust 直接调用。

PowerShell 仅可作为开发机上的构建脚本宿主，不属于产品运行时；产品包不再携带 `BackupRestore.exe prepare`。

## 已验证证据

- macOS：`cargo fmt --all`、`cargo test --workspace --all-targets --offline`、`git diff --check` 通过；
- Windows ARM64：使用现有 `aarch64-pc-windows-msvc` 工具链构建 v1.1.5 通过；
- Windows ARM64 Rust `list-volumes`：返回 C/T/U 普通 NTFS 卷和真实 GPT 类型，EFI/Recovery 未进入列表；
- Windows ARM64 Rust `inspect-environment`：报告 ARM64、WinRE 可用；
- Windows ARM64 Rust `wim-info`：多索引 WIM 返回 Index 1/2；
- Windows ARM64 Rust `prepare --operation restore-existing --source-drive C --target-drive C`：在任务创建前阻止，未修改 WinRE/BCD、未请求重启；
- Windows ARM64 Rust `prepare --operation probe --no-reboot`：成功生成任务；`validate-task` 和 `recover --dry-run` 通过。
- Windows ARM64 Rust 自动 probe：当前 v1.1.5 任务 `8f180630-1d8d-414e-b166-70ed9301d911` 从 `F:\BackupRestoreMoved-v1.1.5` 完成 Windows → WinRE 直接加载 `Recovery.exe recover-env` → 原始 WinRE hash 恢复 → Windows；`status.json=success`，`Recovery.log` 记录 `wpeutil.exe reboot`，载荷中没有 `.cmd`。旧 `v0.8.0` 经启动器的 probe 仅是历史基线。
- Windows ARM64 Rust 备份：v0.8.1 在 `B:\BRRustV081` 以 U: 为源、B: 为镜像目标完成 `--no-reboot` 准备；任务 `68177954-8208-4661-a4ae-b2531bfcc4d3` 的 env 已记录 `SOURCE_USED_BYTES=3369549824`、`RESERVED_BYTES=2147483648`、`MINIMUM_TARGET_SIZE=8572108800`，`validate-task` 与 `recover --dry-run` 通过。上一版 v0.8.0 的真实 U:→B: Capture 任务 `3a6f0d0f-9843-49b8-9841-4a11a38c25d7` 已成功生成 1.63 GB WIM；C: 未触碰。
- Windows ARM64 Rust 多索引还原：v0.8.2 任务 `fcfdd192-61e0-4b14-b04f-9734dcd26e48` 使用 B: 工作目录、T: Index 2 镜像、U: 测试目标和 E: 独立 EFI，完成 WinRE 真实格式化、Apply、BCDBoot 与原 WinRE 清理；U: SYSTEM hash 恢复为 fixture 的 `A70A0D…CC550`，C: 未触碰。

## 尚未宣称完成

Rust prepare 的真实备份/还原 WinRE 重启链路尚未在本轮重新执行；C: 仍不作为备份源或还原目标。下一步应在非 C: 测试卷上验证 Rust prepare → WinRE → Rust Recovery 的完整链路，再验证移动程序目录后的任务/日志跟随。
