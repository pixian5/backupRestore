# V1 实现说明

当前进度、用户对网络/下载的要求和实机未验证项统一见 [project-status.md](project-status.md)。本文件只记录实现事实与技术边界。

## 已实现的安全骨架

- `backuprestore-core` 提供统一的任务 JSON、卷身份、WIM 哈希/大小校验、容量校验、BitLocker 拒绝策略、原子 `task.json` / `status.json` 写入和状态机。
- 备份、已有 Windows 还原、创建第二 Windows 三种任务都要求源/镜像/目标分区身份明确；镜像与目标、源与目标不能相同，EFI/MSR/Recovery 分区不能作为目标。
- 恢复状态在准备、启动、WinRE、预检、捕获、擦除、应用、修复引导、成功/失败之间单向流转；准备阶段失败也会落盘失败状态，避免“无状态卡死”。
- Windows 准备脚本检查 Windows 10/11、ARM64（同时保留 x64 配置）、UEFI/GPT、WinRE、BitLocker 状态，保存原始 WinRE 与 BCD，并用 SHA-256 清单保护 WinRE payload。
- 准备脚本和 GUI 会记录 Secure Boot 状态（on/off/unknown）；不会关闭 Secure Boot，也不会把未验证的 Secure Boot 结果当作 WinRE 实机成功。
- `winpeshl.ini` 现在自动进入哈希校验的 `RecoveryLauncher.cmd`；真实任务优先启动 `Recovery.exe recover-env`，没有 exe 时只允许 `probe` 兼容脚本运行。
- `BackupRestore.ps1 -NoReboot` 只生成并校验临时 WinRE 载荷，不设置一次性启动、不替换注册的 WinRE；准备阶段异常会先丢弃残留 DISM 挂载，再尝试用原始副本恢复注册镜像；DISM 提交后会等待 `wimserv.exe` 释放 WIM，再计算哈希。
- payload manifest 还绑定 `RecoveryTask.env` 的 SHA-256；Rust Recovery 读取 env 后会用任务卷上的 manifest 再校验一次，避免环境变量文件被替换后继续执行。
- WinRE 执行路径按磁盘号/分区号和卷 GUID 重新挂载固定临时盘符，并逐项比对任务 JSON 与 RecoveryTask.env 的磁盘/分区/大小快照；恢复前再次校验镜像与 metadata，备份完成后原子生成 `metadata.json`，所有破坏性操作都要求 `-AllowDestructive`。
- 准备阶段优先从 `reagentc /info` 的 `harddiskN\partitionM` 路径解析注册 Recovery 分区，并优先选择启动盘上的 EFI；解析失败才使用保守候选筛选，不会静默改动其他分区。
- ARM64 备份元数据记录实际架构、Windows 版本/构建号、已用空间和预留空间；Recovery.exe 的 DISM/BCDBoot 输出同时实时写入日志和 WinRE 控制台，启动修复失败时尝试导入任务创建前的 BCD 快照。
- GUI 在提交前重新读取任务、源、镜像和目标的磁盘 GUID、分区 GUID、偏移、大小、文件系统，并把这些值放入确认对话框。
- GUI 现在只在管理员准备脚本返回 0 时提示任务准备成功；备份不会附带破坏性开关，还原失败会显示退出码和准备日志路径。
- Rust Recovery 路径带有清理守卫：在 WinRE 已挂载任务卷后，即使载荷校验、磁盘挂载或 DISM/BCDBoot 提前失败，也会尝试恢复原始注册 WinRE，并保留失败日志。
- 2026-08-21 修复：`recover-env` 不再在 DISM/BCDBoot 完成后立即写入 `success`；它会先恢复并校验注册的原始 `Winre.wim`，清理成功后才完成最终状态转换。清理失败会从当前恢复阶段写入 `failed`，避免任务状态在 WinRE 仍被篡改时虚报成功；清理守卫仍会在退出时做一次最后的恢复尝试。
- 2026-08-21 修复：补齐 `preflight -> success` 状态转换，确保非破坏性的 `probe` 在 WinRE 挂载和校验完成后能够正常落盘最终成功状态。
- TaskStore 读取任务时会先验证请求 ID 为 UUID，再确认文件内 `task_id` 与请求一致，避免错误路径或串任务文件被当成当前任务。
- 任务卷、镜像卷和还原目标会拒绝 EFI/MSR/Recovery 分区；相对路径拒绝 `.`, `..`、绝对路径和盘符前缀，TaskStore 目录按规范化 UUID 定位且不覆盖已有任务目录。
- 新任务 JSON 记录 `taskVolume` 身份；Recovery 会把任务卷本身也与 RecoveryTask.env 的 GUID、盘号、分区号、偏移、大小、类型、文件系统和序列号复核，避免任务目录被换卷后继续执行。
- Recovery.exe 按持久化阶段断点续跑：备份在 `capturing` 阶段重做临时 WIM；还原从 `target-erased`、`image-applied` 或 `boot-repaired` 选择性重做，并在 BCDBoot 失败时保留 BCD 回滚边界。
- GUI 已拆为“首页/环境”“备份与还原”“任务结果/日志”三个页面；结果页明确区分“任务已准备”和 WinRE 重启后的真实成功，探测不携带破坏性开关。
- 准备脚本在 `C:\ProgramData\BackupRestore\last-task.json` 写入最近任务指针；GUI 结果页可回读 task ID、任务目录、status.json、Recovery.log、prepare.log、镜像路径和 metadata 摘要，并手动刷新状态。
- 无 Rust 可执行文件的 `Recovery.cmd` 兼容入口增加任务 ID、任务路径、镜像相对路径和保留分区类型检查；RecoveryTask.env 同步记录镜像/目标分区类型、文件系统和卷序列号。
- `docs/verification-matrix.md` 按每一项需求列出代码证据、离线证据与实机验收证据，后续交接不得用 AST/单元测试替代自动重启、DISM、格式化或 BCD 证据。
- `probe` 允许任务目录与源分区相同：WinRE 先验证并挂载任务卷为 `T:`，若源与任务是同一分区则复用 `T:`，不再二次分配 `S:`；真实 backup/restore 仍拒绝任务卷与源或目标重合。
- 2026-08-21 修复 WinRE 盘符复用：恢复主机现在先按卷 GUID 扫描 `C:` 到 `Z:` 的已有挂载，任务卷、Recovery 卷、源/镜像/目标卷和 EFI 均复用已存在盘符；只有找不到匹配卷时才执行 `mountvol`/DiskPart 分配。实际使用的盘符会回写到任务内存模型，清理原始 WinRE 也使用实际 Recovery 盘符，避免 WinRE 保留 `C:` 时重复分配 `T:` 卡死。
- 2026-08-21 修复 Windows 构建目录依赖：`build-windows.ps1` 现在为每次 Cargo 调用显式传入仓库 `Cargo.toml`；因此 Parallels Guest Tools 从 `C:\` 启动 PowerShell 时，文档中的构建命令仍会使用指定源码目录，而不会错误在 `C:\` 查找 manifest。
- 2026-08-21 修复：任务卷重叠校验原先只拦截了 source/target，未覆盖 backup 的 destination 和 restore 的 image volume。已补齐同分区拒绝逻辑，并在 core 端新增回归测试，确保任务卷不能与镜像卷或目标卷重叠。
- 2026-08-21 ARM64 构建：Recovery payload 会随包携带 `VCRUNTIME140.dll` 与 `VCRUNTIME140_1.dll`；WinRE 启动器把完整 payload hash 校验交给 Rust，兼容 `certutil` 输出中的空格。挂载失败时保留 file-backed DiskPart 日志，便于后续 WinRE 实机排查。

## 当前验证结果

macOS 本地已完成：

- `cargo test --workspace --all-targets --offline`：核心 crate 12 项测试通过；
- `cargo fmt --all -- --check`：通过；
- PowerShell AST 解析：`windows/BackupRestore.ps1`、`windows/BackupRestore.Gui.ps1`、`windows/build-windows.ps1` 均通过；Win11 ARM64 的 Windows PowerShell 5.1 输出也已覆盖 UTF-8 BOM JSON 读取。
- Windows VM 曾生成 `BackupRestore-windows-arm64-v0.2.8`；上一轮已编译 `BackupRestore-windows-arm64-v0.2.9`，包含状态一致性修复。本轮已编译 `v0.3.0` ARM64 包，包含 probe 状态机修复。这类产物只是编译/打包证据，不是 WinRE 运行验收。
- 2026-08-21 `v0.3.0` ARM64 包已在 Windows VM 内实际启动：`Recovery.exe hash` 返回 `dad44f85e4ba78044a56399625934b61e2548c8e471633fb6a03435e04772631`，与 `build-manifest.json` 一致；`validate-task` 对现有任务 fixture 通过。该证据覆盖 ARM64 进程启动和 schema 入口，不覆盖管理员 WinRE、DISM、BCDBoot 或重启。
- 2026-08-21 管理员 `probe -NoReboot` 实测完成了 BCD 快照、原始 WinRE 复制、DISM 挂载/提交、manifest/status 写入，并确认原始 WinRE hash 未改变；首次实测发现 probe 未携带同目录 `Recovery.exe`，已修复脚本默认路径，使所有操作优先复制同架构 Recovery.exe，只有探针包确实缺少 exe 时才保留兼容入口。
- 2026-08-21 `v0.3.1` 自动 WinRE 入口已实际进入 `RecoveryLauncher.cmd` 并启动 `Recovery.exe`；首次入口测试暴露任务分区在 WinRE 已保留 `C:` 而代码强行申请 `T:` 的挂载缺陷。该缺陷已修复为 GUID 扫描复用逻辑，当前 `v0.3.6` 需要重新构建后再次验证终态和原始 WinRE 清理。
- 2026-08-21 Windows ARM64 编译补充：macOS 上的 `cargo test`/Clippy 不会编译 `#[cfg(windows)]` 分支；首次 VM 构建发现 `mountvol` 参数类型和盘符局部变量初始化问题，已在 `v0.3.5` 修复。每次修改 WinRE Rust 路径后，必须在目标 Windows 架构重新构建，不能只依赖 macOS 离线检查。
- 2026-08-21 新增 `poc/restore-task-winre.ps1`：只允许管理员按 UUID 任务恢复其保存的原始 WinRE；脚本复核任务 ID、Recovery 分区 GUID/类型、env/manifest 原始 hash，拒绝错误的 `R:` 盘符后才复制并复核目标 hash。它用于测试失败后的受控清理，不执行 BCD、格式化或还原。
- 2026-08-21 Win11 ARM64 自动 probe 实测通过：`v0.3.6` 的任务 `c12026c0-6a9e-4093-8a8b-2971968a31f7` 从正常 Windows 进入任务 WinRE，`RecoveryLauncher.cmd` 记录启动 `Recovery.exe`；Recovery 日志记录 probe 完成、原始注册 WinRE 恢复并校验、最终写入 `success` 和 `wpeutil reboot`。返回 Windows 后注册镜像 SHA-256 与任务原始副本一致。该证据只覆盖 probe，不覆盖任何磁盘格式化、DISM Capture/Apply、BCDBoot 或双系统写入。
- 2026-08-22 备份实测发现：中文 Windows 的 DISM 输出可能使用系统代码页而不是 UTF-8；Recovery 原先用 UTF-8 `read_line` 读取原生输出，导致 Capture 已启动后日志线程因 `stream did not contain valid UTF-8` 使任务失败。`v0.3.7` 改为按字节读取、UTF-8 lossless replacement 写日志，不能让日志编码问题中断已经开始的 DISM/BCDBoot。
- 2026-08-22 隔离 EFI 诊断：`prlctl exec --current-user` 对应 `P8B6\\x` 本地管理员账户，但普通进程是 UAC medium token。它调用 `bcdboot S:\Windows /s E: /f UEFI /v` 时，源端 ARM64 `bcdboot.exe`、`bootmgfw.efi` 与 `winload.efi` 一致，随后因对隔离 `HarddiskVolume9` 的 `0x5 Access denied` 失败。`v0.4.1` 改用 `target_root.join("Windows")` 构造源路径，并永久传入 `/v`，使高完整性实测能够保留 BFSVC 细节；仍须用 `x` 的 RunAs 令牌验收，不能使用来宾账户替代。
- 2026-08-22 GUI 收口：测试产物不再放在仓库根目录或桌面；历史项目压缩包/构建文件集中到 `.test-artifacts/desktop-archive/2026-08-22`，历史截图集中到 `.test-artifacts/root-captures/2026-08-22`，两者均被 `.gitignore` 忽略。WPF GUI 默认从无破坏 `probe` 开始，自动提出卷建议并新增 WIM 索引字段；提交时将索引传入 `BackupRestore.ps1 -WimIndex`。
- 2026-08-22 Windows PowerShell 5.1 GUI 烟测首次发现 here-string 解析失败，已改为字符串数组拼接并纳入后续 ARM64 包重建门槛；macOS PowerShell 7 AST 不能替代目标 Windows PowerShell 5.1 解析。
- 2026-08-22 GUI 技术路线纠正：用户要求 Rust 开发，`BackupRestore.exe` 已改为直接进入 `native_gui.rs` 的 Win32 原生窗口；它用 `ShellExecuteW(runas)` 调起已有管理员准备脚本，Recovery/CLI 仍共用同一 Rust 二进制。PowerShell/WPF 文件不再是默认 GUI 入口，保留仅为兼容和后端脚本调用。

## 尚未宣称完成的实机项

当前先以 Parallels Win11 ARM64 为主线，以下必须在 ARM64 虚拟机和快照上验证后才能发布：

1. DISM Capture/Apply、快速格式化、BCDBoot 返回已有系统和添加第二启动项（含启动菜单名称）的完整链路；
2. BitLocker 解锁/拒绝、异常断电后的原始 WinRE/BCD 回滚；
3. GUI 在真实磁盘枚举、二次确认和任务日志展示上的可用性。

在这些实机项完成前，项目属于开发测试版；x64 构建暂缓，不要把 macOS 离线测试结果当作 Windows 恢复成功证明。ARM64 包还会读取
`build-manifest.json`，拒绝在不匹配的 Windows 架构上运行。

## ARM64 收尾顺序

1. 在 Win11 ARM64 虚拟机安装 Rust 与 `aarch64-pc-windows-msvc` target（下载前遵守个人热点确认）。
2. 运行 `windows\\build-windows.ps1 -Architecture arm64`，检查 `build-manifest.json` 中的二进制 SHA-256。
3. 已完成：在管理员 `x` 会话执行 `probe -NoReboot` 和独立快照上的自动 probe，确认任务身份复核、WinRE 载荷哈希、`winpeshl.ini -> Recovery.exe`、清理与返回 Windows。
4. 后续使用独立快照验证 DISM Capture、单系统 Apply/BCDBoot、双系统 `/addlast`；每次验证后恢复快照并确认原始 WinRE 哈希不变。

## 2026-08-20 ARM64 编译验证边界

- 构建脚本支持 `-CargoTargetDir`，将 Cargo 临时产物放在 VM 本地磁盘，避开共享目录的临时归档限制；脚本不会自动安装或下载工具链。
- 早期恢复会话曾通过 `prlctl` 只读检查发现当前 Win11 ARM64 VM 缺少 `cargo`/`rustup`；随后已安装 Rust/ARM64 MSVC/Windows SDK，并生成 `BackupRestore-windows-arm64-v0.2.8`。本轮已用同一工具链生成 `v0.3.0` ARM64 包；其 WinRE 自动入口、DISM、BCDBoot 和重启链路仍未实机验收。
- Windows PowerShell 5.1 的原生命令改用 .NET `ProcessStartInfo` 等待并把 DISM 专用日志写入任务日志，避免 `Tee-Object` 管道或同步 `.Result` 在 ARM64 VM 上出现命令已完成但父进程不返回。
- 历史探测曾发现 1 MiB 栈上哈希缓冲会触发 ARM64 `STATUS_STACK_OVERFLOW`，已改为堆上缓冲；另修复了 PowerShell 5.1 UTF-8 BOM 导致的 JSON 解析失败。历史探测没有执行格式化、DISM Apply、BCDBoot 写入或真实重启恢复。
- 0.2.1 ARM64 探测曾出现 DISM 提交后立即读取 WIM 的短暂文件锁；0.2.3 修复为独立 DISM 日志、固定等待释放窗口并记录独立失败日志，避免 PowerShell 5.1 枚举 `wimserv.exe` 进程时卡住。
