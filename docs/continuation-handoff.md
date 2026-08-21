# BackupRestore V1 交接与继续开发说明

> 开始工作前先阅读 [文档索引](README.md) 和 [当前进度与决策记录](project-status.md)。后者是当前状态和用户最新工程约束的唯一摘要；本文件负责具体实现交接，不重复记录会变化的进度。

本文是给下一位开发者或 AI 的工作交接文件。它记录“为什么这样设计”、已经完成的代码边界、不能误报的验证结论，以及工具链可用后应该直接执行的顺序。若本文与源代码不一致，以源代码和最新测试结果为准，并在修复后同步更新本文。

## 1. 用户目标与已经确认的沟通决策

用户要求开发一个 Windows 10/11 的系统备份与还原工具：在正常 Windows 中选择备份或还原，程序准备任务并进入系统自带 WinRE，WinRE 自动启动 Recovery.exe，使用 DISM 捕获/应用 WIM，使用 BCDBoot 修复启动项，恢复原始 WinRE 后自动重启。V1 支持 UEFI/GPT、NTFS、WIM、普通单系统还原，以及用户主动选择的第二系统模式；不做自研 PE、分区重构、网络备份、增量镜像或 Legacy BIOS。

用户特别要求：

1. 先在无 Wi-Fi/个人热点环境下把代码、页面、脚本、文档和离线测试全部写好，不要为了“先试一下”偷偷下载工具链。
2. 每一次单独的大文件下载（Rust target、Visual Studio/MSVC、Windows SDK、Docker 镜像、依赖、模型或安装包）前都运行 `check_network.py`。检测到个人热点时，必须针对这一次下载重新请求用户授权；旧下载的授权、旧任务的授权或单独一句“继续”都不能继承。Wi-Fi 或有线网络无需再问一次。用户明确说没有 Wi-Fi 或不下载时，立即停止下载。
3. 工具链以后准备好后，目标是“直接编译并运行”，不再重新设计功能；但真实 Windows/WinRE、DISM、格式化、BCDBoot 和重启仍必须进行实机验收，不能由 macOS 静态检查代替。
4. 重要操作和踩坑写入 `docs/`，中文说明和中文提交信息优先。

上述网络规则已经写入用户级技能 `~/.codex/skills/pixian-dev-workflow/SKILL.md`。本仓库不能保存私密凭据，也不能把热点授权写成永久开关。

### 1.1 关键对话摘要

以下是影响工程边界的必要对话结论，按意图记录，不把情绪化措辞复制进代码或提交：

| 对话意图 | 已落实的工程决定 |
|---|---|
| “没有热点/没有 Wi-Fi 时先把不需要流量的代码写完” | 本轮只使用已有本地工具和离线 Cargo 缓存；不安装 Rust target、Visual Studio、SDK 或其他大文件。 |
| “以后工具链好了要直接编译运行” | `windows/build-windows.ps1` 不自动下载，目标架构、Cargo target 目录、产物文件名和运行入口已固定；交接文档给出唯一构建顺序。 |
| “热点每次下载前重新授权，Wi-Fi 不需要反复问” | 工作流技能按每个独立下载执行 `check_network.py`；热点授权不跨下载继承，Wi-Fi/有线不重复询问。 |
| “所有页面、功能、恢复流程都设计完整” | GUI 有环境、备份/还原、结果/日志三个页签；core、PowerShell、WinRE launcher、Recovery.exe 和 fallback cmd 的职责均已写入文档。 |
| “让其它 AI 接手后直接继续” | `docs/README.md`、`project-status.md`、本文、`implementation-notes.md`、`windows-build.md` 和 `verification-matrix.md` 分别记录入口、进度、文件结构、决策、构建和证据。 |
| “工具链完成后直接继续开发” | 已通过 Parallels 共享桌面传入当前工作树，ARM64 构建脚本可以在 VM 本地 target 目录直接产出包；本轮不执行破坏性恢复。 |

## 2. 当前仓库与版本

- 仓库：`https://github.com/pixian5/backupRestore`
- 本地路径：`/Users/x/code/backupRestore`
- 默认分支：`main`
- 当前开发版本：以根目录 `VERSION` 为准；每完成一轮修改必须执行 `python3 ~/.codex/skills/pixian-dev-workflow/scripts/bump_version.py --root .`，同步两个 Cargo manifest 和 `Cargo.lock`。当前轮目标版本为 `0.3.0`。
- 重要历史提交：
  - `4561f7b`：加强任务标识校验并同步版本；
  - 更早提交包含 ARM64 构建脚本、WinRE JSON 兼容、DISM 日志和清理守卫。
- 提交信息使用简短中文，例如：`完善恢复断点续跑与交接文档`。

## 3. 代码结构与运行边界

```text
crates/backuprestore-core/src/lib.rs       纯 Rust 任务模型、身份、安全校验、状态机、TaskStore
crates/backuprestore-cli/src/main.rs       BackupRestore.exe GUI 启动器、Recovery.exe CLI/WinRE 主机
windows/BackupRestore.ps1                  正常 Windows 管理员准备任务、复制/挂载 WinRE
windows/BackupRestore.Gui.ps1              WPF 页面：环境、备份还原、结果日志
windows/RecoveryLauncher.cmd               WinRE 自动入口和载荷哈希检查
windows/Recovery.cmd                      无 Recovery.exe 时仅限 probe 的兼容入口
windows/winpeshl.ini                       WinRE [LaunchApps] 自动启动入口
windows/build-windows.ps1                  x64/ARM64 分离打包，不自动下载工具链
docs/implementation-notes.md               当前实现、验证证据和未验证边界
docs/windows-build.md                      Windows ARM64/x64 构建说明
docs/continuation-handoff.md                本交接文件
docs/project-status.md                      当前进度、对话决策和继续条件
docs/README.md                              文档阅读入口
```

`BackupRestore.exe` 的文件名会被 Rust 程序识别为 GUI 启动器；它旁边必须有 `BackupRestore.Gui.ps1`。同一个二进制复制为 `Recovery.exe` 后，使用 `recover-env <RecoveryTask.env>` 进入 WinRE 恢复路径。构建脚本生成架构专用目录，x64 和 ARM64 不能混用。

## 4. 已实现的安全与功能约束

### 4.1 任务模型

- `Operation`：`probe`、`backup`、`restore-existing`、`create-secondary`。
- `Stage`：`prepared`、`boot-requested`、`recovery-started`、`preflight`、`capturing`、`target-erased`、`image-applied`、`boot-repaired`、`success`、`failed`。
- 所有任务都保存源、任务、镜像/目的地和目标卷的 disk GUID、partition GUID、volume GUID、分区类型、盘号、分区号、偏移、容量、文件系统和卷序列号；盘符只作为 WinRE 临时挂载提示。
- `TaskStore` 用规范化 UUID 定位任务目录，拒绝路径穿越、非法 UUID、已存在目录覆盖和 `task.json` ID 串任务。
- 正常 Windows 准备脚本在写入 task JSON 后，会优先调用同一 Rust 二进制的 `validate-task` 命令；这样 PowerShell 生成的字段必须通过 Recovery.exe 使用的同一套 schema 校验。
- `status.json` 与 `task.json` 的任务 ID、操作类型必须匹配；`recover-env` 不允许重复执行已处于终态的任务。

### 4.2 备份

- 备份镜像写入 `<relative-path>.partial`；DISM Capture、Get-WimInfo、SHA-256 均成功后才移动为正式 WIM，再写 `metadata.json`。
- metadata 记录捕获电脑、Windows 版本、架构、build、WIM index、镜像 hash/大小、源分区身份、已用空间、预留空间、最小目标容量和程序版本。
- 任务卷、镜像卷、目标卷都拒绝 EFI、MSR、Recovery 分区；备份目的地不能与源分区相同。
- 断电后若状态是 `capturing`，恢复会删除本任务自己的 `.partial` 并重新捕获，避免把不完整 WIM 当成成品。

### 4.3 还原

- `restore-existing` 目标必须是当前 Windows 源分区；`create-secondary` 必须使用 `add-secondary` 启动计划和非空启动菜单名称。
- 还原前读取 WIM 信息、metadata 和 SHA-256，并检查目标容量不小于 metadata 要求；BitLocker 保护开启时拒绝继续。
- 目标卷先持久化 `target-erased` 再格式化；`image-applied` 在 Apply-Image 后写入；`boot-repaired` 在 BCDBoot 前写入，以便 BCDBoot 失败时导入任务创建前 BCD 快照。
- 断电恢复策略：`target-erased` 重做格式化和 Apply-Image；`image-applied` 重做 Apply-Image 和引导修复；`boot-repaired` 只重做 BCDBoot/启动菜单并验证 `bootmgfw.efi`。任何重做都只能针对已经通过身份复核的明确目标。
- EFI 必须由 WinRE 先挂载到显式盘符；Rust 不再猜测 `S:\` 或其他默认盘符。

### 4.4 WinRE 与清理

- 准备阶段保存原始 `Winre.wim` 和 BCD 快照，生成任务专用副本，注入 `RecoveryLauncher.cmd`、`Recovery.cmd`、`RecoveryTask.env`、`task.json`、`winpeshl.ini` 和可选 `Recovery.exe`。
- 每次注入有 payload manifest 和 SHA-256；`-NoReboot` 不替换注册 WinRE，不设置一次性启动。
- manifest 同时绑定 `RecoveryTask.env` 的 SHA-256；Rust Recovery 在读取 env 后再次校验它，环境变量文件被替换时拒绝执行。
- DISM 卸载后固定等待短窗口，防止 Windows PowerShell 5.1 的 WIM 文件锁尚未释放。
- Recovery.exe 在载荷校验、挂载、DISM、BCDBoot 失败时尽力恢复原始 WinRE；WinRE 路径会延迟写入 `success`，直到原始注册镜像恢复并通过 hash 校验。清理失败写入 `failed`，不删除任务目录，保留日志供人工处理。
- `Recovery.cmd` 只作为无 Rust 二进制时的 probe 兼容路径，真实 backup/restore 没有 `Recovery.exe` 会拒绝；兼容入口额外检查任务 ID、`TASK_ROOT_REL`、相对镜像路径和保留分区类型。

## 5. GUI 页面现状

`windows/BackupRestore.Gui.ps1` 目前拆成三个页签：

1. **首页 / 环境**：显示 Windows 版本/build、固件、启动盘 GPT 状态、Secure Boot、C: BitLocker、WinRE 注册状态，并枚举可识别的 NTFS Windows 安装和候选目标分区。页面明确说明静态信息不等于 WinRE 重启成功。
2. **备份与还原**：选择 `probe`、`backup`、`restore-existing` 或 `create-secondary`；填写任务卷、源卷、镜像卷、目标卷、镜像相对路径和第二系统名称。普通模式默认 `backup`；单系统还原自动使用源卷作为目标并禁用目标编辑；只有双系统启用启动菜单名称。提交前再次读取 GUID、分区 GUID、偏移、容量、剩余空间和文件系统，破坏性模式必须二次确认。
3. **任务结果 / 日志**：显示准备脚本退出码、任务 ID、任务目录、`status.json`、`Recovery.log` 和 `prepare.log`，支持刷新状态，并明确“任务已准备”不是“恢复成功”；准备脚本通过 `C:\ProgramData\BackupRestore\last-task.json` 传递最近任务指针，真实终态要看任务卷上的 `status.json` 和 `Recovery.log`。

GUI 仍是 PowerShell/WPF 开发版，macOS 上只能做 AST 解析，不能声称 WPF 真实交互已经验收。若以后改成原生 Rust GUI，保留上述页面和安全文案，不要删掉“准备成功 ≠ 恢复成功”的边界。

## 6. 已踩坑与不要重复犯的错误

- **热点下载误操作**：大文件下载前没有逐次检查网络会违反用户明确要求。必须先执行 `python3 ~/.codex/skills/pixian-dev-workflow/scripts/check_network.py`，热点情况下停下来问本次授权；当前不要下载。
- **把 macOS 测试当 Windows 成功**：`cargo test`、`clippy`、PowerShell AST、静态脚本检查都不能证明 ARM64 Windows、WinRE 自动入口、DISM、format、BCDBoot 或真实重启成功。
- **共享目录构建**：Parallels 共享目录可能不适合 Rust 临时归档；工具链可用后优先把 Cargo target 放到 VM 本地盘，例如 `C:\BackupRestoreBuild\target`。
- **PowerShell 5.1 UTF-8 BOM**：读取 JSON 必须使用项目已有的兼容读取逻辑；不要改回假设无 BOM 的简单读取。
- **WIM 文件锁**：DISM 提交后不能立即计算/复制 WIM；保留独立 DISM 日志和等待窗口。
- **盘符不是身份**：任务和 Recovery 阶段必须用 volume/disk/partition GUID 加大小/偏移复核，不能仅信任 C:、D:、S:。
- **路径穿越**：镜像相对路径不能有 `.`, `..`, `/` 根、盘符前缀或其他逃逸形式；不要把用户输入直接拼成任务根路径。
- **保留分区**：EFI、MSR、Recovery 既不能存任务/镜像，也不能作为还原目标；改 PowerShell 时要同步 Rust 和 `Recovery.cmd` 的检查。
- **状态重复写入**：断电恢复不能无条件重复写同一个状态；状态机的合法边界和“哪个阶段允许重做”必须同时维护。
- **BCD 回滚时机**：不能在 `write_failure` 后才判断原状态，因为写失败会覆盖原阶段；先保存 `stage_before_failure`，并把 `BootRepaired` 前后的失败都纳入回滚判断。
- **GUI 进程参数空格**：PowerShell `Start-Process -ArgumentList` 需要安全引用完整参数，路径包含空格时不能直接依赖数组隐式转换。
- **不要虚构实机证据**：本轮已在 Parallels ARM64 VM 生成 ARM64 包，但这不能替代 WinRE 自动入口、DISM、BCDBoot 或真实重启证据。

## 7. 工具链可用后的唯一推荐顺序

在用户明确允许并且网络规则允许时：

1. 工具链已安装在 Parallels Windows 11 ARM64 VM；源码通过共享桌面传输到 `C:\BackupRestoreBuild\source`，Cargo target 使用 VM 本地目录。
2. 在仓库目录执行：

   ```powershell
   .\windows\build-windows.ps1 -Architecture arm64 -CargoTargetDir C:\BackupRestoreBuild\target
   ```

   首次出现缺少 target 时脚本应停止，而不是自动下载。

3. 在管理员 Guest 会话先运行 `probe -NoReboot`，确认任务身份、载荷 hash、`winpeshl.ini -> RecoveryLauncher.cmd -> Recovery.exe`；保存 `status.json`、`Recovery.log`、`prepare.log` 和 WinRE 原始 hash。
4. 使用 VM 快照分别验证：备份、单系统还原、双系统 `/addlast`、磁盘身份不匹配拒绝、BitLocker 拒绝、断电后阶段恢复、BCDBoot 失败 BCD 回滚、原始 WinRE 恢复和自动重启。
5. 只有拿到重启后的真实证据，才能在文档中把对应项从“未验证”改成“已验证”。每个验证后恢复快照，避免把测试卷当成用户数据。
6. 完成修复后再次运行离线测试、AST、`git diff --check`，递增版本，中文提交并推送 `origin/main`。

## 8. 当前应执行的离线验证

```bash
cargo fmt --all
cargo test --workspace --all-targets --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
pwsh -NoLogo -NoProfile -NonInteractive -Command '$files=@("windows/BackupRestore.ps1","windows/BackupRestore.Gui.ps1","windows/build-windows.ps1"); foreach($f in $files){$tokens=$null;$errors=$null; [System.Management.Automation.Language.Parser]::ParseFile((Join-Path (Get-Location) $f),[ref]$tokens,[ref]$errors)|Out-Null; if($errors.Count){$errors|% Message; exit 1}; "AST OK $f"}'
git diff --check
```

这些命令不下载工具链，适合当前个人热点/无 Wi-Fi 状态。若其中任何一项失败，先修复并重新验证，再版本递增和提交；不要仅凭“代码看起来完整”结束任务。
