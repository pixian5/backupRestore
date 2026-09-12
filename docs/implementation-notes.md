# V1 实现说明

## 2026-09-13 `v1.4.8` DISM 备份排除配置

- 备份捕获增加 `/ConfigFile` 排除（WinRE 主流程与 PE 直连备份两条路径）：
  - 固定排除：`\$Recycle.Bin`、`\$WINDOWS.~BT`、`\$WINDOWS.~WS`、`\Windows.old`、`\Temp`、`\Windows\Temp`、`\Windows\SoftwareDistribution\Download`、`\Windows\Prefetch`、`\Windows\Logs`、`\Windows\Panther`、`\ProgramData\Microsoft\Windows\WER`。
  - 按真实用户枚举（逐用户字面路径）：`\Users\<p>\AppData\Local\Temp`、Chrome/Edge/Brave/Vivaldi 的 `User Data\<配置>\{Cache,Code Cache,GPUCache,Service Worker\CacheStorage,Service Worker\ScriptCache}`、Firefox `Profiles\<配置>\{cache2,cache2\entries,OfflineCache,startupCache}`、Opera Stable 缓存、INetCache。
  - `hiberfil.sys/pagefile.sys/swapfile.sys/\System Volume Information` 由 DISM 默认排除，未重复列出。
- 关键技术边界（DISM 配置文档约束）：排除表是「根路径锚定」写法；通配符只允许出现在**不以反斜杠开头的路径的最后一段**，因此 `\Users\*\AppData\...` 中间通配符不合法。浏览器缓存目录必须在备份时枚举真实用户目录生成无通配符的字面路径（`crates/backuprestore-core` 的 `build_capture_exclusions`）。
- 配置文件写到 `env::temp_dir()`（WinRE/PE 的 X: RAM 盘），不落在捕获卷内；PE 直连备份生成失败时降级为不带排除继续捕获。WinRE 路径生成失败则返回错误。
- 验证：本机 core 19 项测试通过（含新排除测试，断言无中间通配符）；`cargo check/build --target aarch64-pc-windows-msvc` 通过；VM `Win11-repair` 上 `test-artifacts/run-exclusions-validate.cmd` 真实验证 DISM 接受该 `/ConfigFile` 语法，`\junk` 排除成功、保留目录内容在镜像内。

## 2026-08-27 `v1.2.2` 开发 EFI 故障注入

- 仅开发 CLI 在同时指定 `--test-efi-drive` 时接受 `--test-fault identity-env-mismatch` 或 `bcdboot-failure`；普通 GUI 和正常 EFI 流程不暴露该参数。
- 前者在 manifest 保护的 `RecoveryTask.env` 中制造源卷序列号与 `task.json` 的真实不一致，WinRE 必须在任何 DISM、格式化或 BCDBoot 前由既有身份校验拒绝；后者在持久化 `BootRepaired` 后注入 BCDBoot 失败，以验证既有 BCD 快照回滚与 WinRE 清理。

## 2026-08-27 `v1.2.4` 原始 BCD 快照持久化顺序

- 首次实机复测发现：创建任务目录后才捕获字节级 EFI BCD 快照时，内存中的 `previous_bcd_sha256` 已更新，但工作区 `task.json` 尚未覆盖写入；Recovery 因 payload/task 不一致而提前拒绝。现在在复制 `task.json` 到 WinRE payload 前原子写回完整任务，确保快照哈希、manifest 和 payload 一致。

## 2026-08-27 `v1.2.5` 故障回滚与完整 P/WIM/Q 验收

- `identity-env-mismatch` 实机任务 `00bae5b0-d830-4fac-8bd8-7bbdd3e85188` 在任何 DISM/格式化/BCDBoot 前失败，错误为 `SOURCE volume serial differs between task.json and RecoveryTask.env`；E: BCD 哈希保持 `FC1E0A786691E8F20083DC08A97BC2BD31BD8188BEE3C29E68AC122585CD479D`，Q: 内容计数不变，原注册 WinRE 恢复并校验。
- `bcdboot-failure` 实机任务 `635e39d3-034a-4390-873b-b0ae68843863` 真实完成格式化和 Index 1 Apply 后在 `BootRepaired` 注入失败；`bcd-before-raw` 与 E: BCD SHA-256 均为 `FC1E0A786691E8F20083DC08A97BC2BD31BD8188BEE3C29E68AC122585CD479D`，WinRE 恢复为 Enabled。
- 成功备份任务 `89439a72-9fc6-41be-ae49-18be36e9a850` 从 P 捕获到 `H:\Images\FullCycle-v1.2.5.wim`（索引 1，WIM SHA-256 `89CBDB75E83FCE55F3A8EBE140B696687957C14184EBA3F8D8C633A120C68B47`）；追加任务 `b4fec5e1-7d02-4aad-8bc3-ee7959f3e0ef` 创建索引 2。还原任务 `bb73da54-80e3-4b7b-9550-0ac24901aab2` 使用 Index 1，`8a590f81-7777-4074-915e-e7dd5dec2607` 使用 Index 2，均完成 WinRE、DiskPart、DISM Apply、BCDBoot 和清理重启。
- P/Q 按相对路径、大小和 SHA-256 比较：排除 `System Volume Information`、回收站、Parallels `Mac disk` 和 BCDBoot 更新的 `Windows\System32\config\BCD-Template*` 后，212 个 payload 文件完全一致，差异 0。C: 未作为备份源或还原目标。

## 2026-08-27 `v1.2.1` 状态文件断电窗口修复

- `write_json_atomic` 过去在 Windows 覆盖 `task.json`/`status.json` 时会先删除旧文件，再重命名同目录临时文件。掉电恰好落在两步之间会让恢复任务缺少状态记录，违背原子持久化和断电续跑约束。
- Windows 现在用 `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` 将已 `sync_all` 的同目录临时 JSON 直接替换目标；不再删除原记录。非 Windows 保持同文件系统 `rename`。新增覆盖写入回归测试确认读到完整新 JSON 且不遗留临时文件。
- 本机离线 5 项 CLI + 17 项 core、Clippy、运行时边界审计和差异检查全部通过。Windows 11 ARM64 从共享桌面源码构建 `C:\BackupRestoreBuild\package\BackupRestore-windows-arm64-v1.2.1`，CLI 10 项 + core 17 项全部通过；测试目标目录必须在客体本地，不能置于 Parallels 共享源码目录。manifest、两个 EXE 和前台窗口标题均为 `v1.2.1`，二进制 SHA-256 为 `3c3c27057b1c81cb5ed17023a06f32376928ecc6c6bf213ba1be3d813ffd3415`。

## 2026-08-27 `v1.2.0` P/Q 隔离备份与第二系统还原

- 在新快照 `24860ee1-0682-4b2b-8158-7a68da0f28f8` 上，ARM64 `v1.1.9` 从 P（磁盘 2/分区 2）备份到 H 的 `BlankCompare-v1.1.9.wim`；任务 `a12fa889-c64f-4db3-88be-2adce2dc8518` 为 `success`，WIM SHA-256 与 index-1 metadata 均为 `8f231f3f5042dcf838963ffe880fb705c432700f4b0b690599d11c56882bdffa`。
- 同一 WIM 索引由任务 `e005077c-1431-4987-a98c-8a8074aab5d5` 还原到 Q（磁盘 2/分区 3），使用开发 EFI E。状态为 `success`；Recovery 日志记录 `Apply-Image`、`bcdboot ... /addlast`、目标 loader 绑定验证、原 WinRE 恢复和 `wpeutil reboot`。
- 管理员复核 E: 的 BCD 可打开，新增 loader 的 `device/osdevice=partition=Q:`。普通令牌无法列出受保护 EFI 文件或打开 BCD，不能把“Access denied”误判为 EFI 内容丢失。
- P/Q 的文件树以相对路径、大小和 SHA-256 比较。Parallels 自动产生的锁定 `Mac disk`、`System Volume Information`，以及还原后 BCDBoot 修改的 `BCD-Template`/`.LOG` 和回收站 `desktop.ini` 单列为系统后处理项；排除这些可解释项后，212 个文件、174,008,660 字节完全一致。测试清单工具现在显式报告该规则，避免隐藏差异。

## 2026-08-27 `v1.1.9` 日志与执行纪律

- GUI 的关键交互统一写入 `<程序目录>\logs\gui.log`：刷新环境/最近任务、WIM 元数据读取、镜像选择、任务创建、校验阻止、用户取消和管理员 prepare 启动结果均带 RFC 3339 时间戳。
- 任务目录尚未创建就失败的 CLI 路径继续写入 `<程序目录>\logs\launcher-errors.log`；任务准备和 WinRE 恢复分别保留自己的 `prepare.log` 和 `recovery.log`。这四类日志都跟随程序目录移动。
- GUI 日志采用尽力写入，不让诊断文件写入异常绕过或改变安全校验；准备和恢复阶段的关键日志仍为强制写入。
- 后续每个开发/测试/清理任务先按 `docs/development-execution-protocol.md` 写明计划并完成自审，随后再修改或操作虚拟机。
- Windows 11 ARM64 已用既有工具链真实构建 `C:\BackupRestoreBuild\package\BackupRestore-windows-arm64-v1.1.9`；VM 目标测试 10 项 CLI 和 16 项 core 全部通过，两个 EXE 与 `build-manifest.json` 的 SHA-256 均为 `e0d97e7038e9aa2199907cd5b358890411cb0e664e23715fc6d9b2ba97c22964`。最新 GUI 已以前台最大化运行，实际创建 `logs\gui.log` 首行 `[2026-08-27T04:39:01.106746300+00:00] GUI started; elevated native Rust window initialized`。Computer Use 对客体刷新按钮的坐标输入未稳定转发，未将刷新动作或后续日志事件标记为实机已验证。

当前进度、用户对网络/下载的要求和实机未验证项统一见 [project-status.md](project-status.md)。本文件只记录实现事实与技术边界。

## 2026-08-27 `v1.2.9` 标题核验与故障参数回归

- Rust Win32 窗口标题仍由 `CreateWindowExW` 使用 `BackupRestore - Rust GUI v{PROGRAM_VERSION}` 设置；Windows 客体前台截图实际显示 `BackupRestore - Rust GUI v1.2.8`。宿主通过跨完整性 PowerShell 读取 `MainWindowTitle` 得到空值属于 Windows 访问边界，不能据此判断标题丢失。
- 为 `--test-fault` 增加参数回归测试：`power-loss-window` 不再被错误要求开发 EFI，身份环境篡改和 BCDBoot 失败仍要求显式 `--test-efi-drive`。这样断电窗口测试使用真实系统 WinRE，避免把开发 EFI 选择与正常续跑混在一起。
- v1.2.9 Windows ARM64 客体本地测试通过 CLI 11 项、core 17 项；本机格式化、离线测试、Clippy、运行时边界审计和 `git diff --check` 继续作为构建前门槛。

## 2026-08-27/28 `v1.3.0` P/WIM/Q 完整备份还原验收

- 最新 ARM64 Rust 包从 P: 捕获到 `H:\Images\FullCycle-v1.2.9.wim`。任务 `5f2d2262-80c1-43c8-b839-8b2544effedf`、`fcad95ce-9c46-4e74-8302-452fbda7992f`、`31971c01-62b3-47ff-a01f-22dddb2f9541` 通过同卷候选文件事务生成索引 1/2/3；三个索引均由 DISM `/Get-WimInfo` 实读确认。
- `create-secondary` 使用索引 1/2/3 分别恢复 Q:（任务 `21ecf1b9-72ce-4a65-b279-459880edcb88`、`eac6910f-8387-478b-babf-935135d78d8a`、`8f5ca68c-a7f5-410d-8cfc-2791f33a389d`），`restore-existing` 使用索引 2 和 3 恢复 P:（任务 `312a7cc0-5c15-4d29-9e7b-3ead67644a16`、`8538cc29-79fe-41d3-9de2-8458e3bfaa48`）。五个恢复任务均完成格式化/Apply/BCDBoot、原 WinRE 恢复和自动重启，状态为 `success`。
- P: 还原前后以及 Q: 三次恢复后的完整分区清单各含 215 个文件；排除 Parallels 锁定的 `Mac disk` 与 BCDBoot 自动生成的 `Windows\System32\config\BCD-Template*` 后，均为 212 个 payload 文件，路径、大小和 SHA-256 的 `Compare-Object` 差异为 0。五个 fixture 也逐项一致。恢复后 `reagentc /info` 为 Enabled，`dism /Get-MountedWimInfo` 报告无挂载镜像；最终 v1.3.0 GUI 前台截图标题为 `BackupRestore - Rust GUI v1.3.0`。
- 断电窗口任务 `3dd7a348-9190-4e61-90d0-76ecf1fd45cf` 在 `boot-requested` 持久化后故意不重启；再次启动 GUI 后，`prepare.log` 记录检测待恢复任务、重新请求一次性 WinRE 启动并重启，Recovery 最终将任务标记为 `success`。

> 当前运行边界（2026-08-27，v1.2.5）：产品运行时完全由 Rust 提供。`BackupRestore.exe`、`Recovery.exe`、任务准备、卷枚举、WIM 信息读取和 WinRE 恢复不调用 PowerShell；PowerShell 只保留为 Windows 构建脚本宿主及历史实验记录。较早时间线中的旧脚本、旧参数和旧版本包名均不可作为当前运行入口。

- 2026-08-27 `v1.0.5` WIM 多索引元数据：DISM 文本回退解析器现在按每个 `Index` 分组读取 `Name`、`Description`、`Size`、`Version`、`Architecture`、`Edition/Edition Id` 和 `Installation Type`；支持逗号分隔字节数及常见 KiB/MiB/GiB/TiB 单位。解析严格限定在有效索引之后，避免把 DISM 头部的工具版本误记为镜像版本；新增多索引与无索引回归测试。标准 `/Get-WimInfo` 未提供的字段仍显示为“?”，不虚构元数据。
- 2026-08-27 `v1.0.5` ARM64 回归：Windows 11 ARM64 目标全量测试 8 项 CLI（包含多索引解析、头部版本隔离和参数边界）与 16 项核心测试全部通过；发行构建使用现有 `aarch64-pc-windows-msvc` 工具链成功，`BackupRestore.exe`/`Recovery.exe` 与 manifest SHA-256 均为 `5a1fd2b97c358d31ba6dd3d93b088edab1cdd051e5db1cd3e651aade5008af7e`。最新 GUI 已结束旧实例后以前台最大化运行，标题为 `BackupRestore - Rust GUI v1.0.5`，截图保存于 `.test-artifacts/root-captures/v1.0.5-gui.png`；真实 `D:\sources\boot.wim` 的 `wim-info` 返回索引 1、描述和 2163165471 字节大小。尚未声称多索引 WIM 的 GUI 实盘截图，因为当前挂载卷没有可用多索引镜像；多索引解析由 Windows 目标单元测试覆盖。
- 2026-08-27 `v1.0.6` GUI 文本读取修复：Win32 GUI 的 `get_text` 从 `WM_GETTEXTLENGTH/WM_GETTEXT` 改为 `GetWindowTextLengthW/GetWindowTextW`。真实控件点检确认跨进程自动设置的 `V:\multi-index-same-source-v1.0.5.wim` 能被 Win32 API 读回，修复了消息返回长度为 0 导致的“镜像绝对路径为空”误报；需继续在新 ARM64 包上完成读取按钮、多索引下拉和最终截图验收。
- 2026-08-27 `v1.0.7` GUI 文本读取兜底：部分控件实测仍可能返回 `GetWindowTextLengthW=0`，但 `GetWindowTextW` 能读回可见文本。`get_text` 现在在报告长度为 0 时使用 32 KiB 有界缓冲，按实际写入长度返回字符串，避免有效镜像路径被判为空；下一步必须在 ARM64 新包完成多索引按钮和下拉框验收。
- 2026-08-27 `v1.0.8` GUI 文本读取三层回退：`GetWindowTextLengthW/GetWindowTextW` 返回 0 时，使用 32 KiB 上限缓冲再尝试 `WM_GETTEXTLENGTH/WM_GETTEXT`，覆盖跨完整性/线程边界的原生 EDIT 控件。该改动只影响 GUI 字段读取，不放宽绝对路径校验；ARM64 新包仍需用同源双索引 WIM 完成真实按钮和下拉截图。
- 2026-08-27 `v1.0.8` ARM64 构建与 WIM 实读：既有 `aarch64-pc-windows-msvc` 工具链从共享桌面源码成功构建，包内两个 Rust 二进制和 manifest SHA-256 为 `8a67df2c833ff0a4b20501e5446273f5ee7af4df015c8ed3a78046acba8744c1`。Windows ARM64 CLI 8 项、core 16 项测试通过；提升权限执行 `wim-info V:\multi-index-same-source-v1.0.5.wim` 返回索引 1/2 及各自名称、描述和大小。GUI 最新前台标题为 `BackupRestore - Rust GUI v1.0.8`，但双索引下拉的最终客体截图仍待稳定的真实输入点检，不以 CLI 输出替代。
- 2026-08-27 `v1.0.9` GUI 启动参数与双索引实机验证：新增 `BackupRestore.exe --open-image <绝对 WIM 路径>`，启动后自动进入单系统还原页并调用同一 WIM 读取逻辑。修复 UAC `ShellExecuteW` 重启丢失 argv 的问题；此前它会吞掉 `--open-image` 并回到探测页。Windows ARM64 实机从 `C:\BackupRestoreBuild\multi-index-gui-v1.0.8.wim` 读取同源双索引 WIM，前台 GUI 状态显示“已读取 2 个 WIM 索引”，下拉框显示索引 1 的名称和描述；提升令牌不可见映射共享盘 `Y:`，因此该测试输入必须放在本地卷。
- 2026-08-27 `v1.0.9` 发布版本一致性：根 `VERSION` 已升至 `1.0.9` 时，两个 Cargo manifest 仍是 `1.0.8`。构建目录名取根版本，而窗口标题取 `CARGO_PKG_VERSION`，导致同一包出现 `v1.0.9` 目录与 `v1.0.8` 标题。现已同步根版本、两个 manifest 与锁文件；Windows 包必须同时核对目录、`build-manifest.json` 和窗口标题，三者不一致即视为构建失败。
- 2026-08-27 `v1.1.0` GUI 替换：提升后的新 GUI 在创建窗口前枚举当前交互桌面，只对旧 `BackupRestoreNativeGui` 窗口投递正常 `WM_CLOSE`，短暂等待其释放文件后再显示最新窗口。它不终止进程，也不操作任务、磁盘或 WinRE；目的只是防止多实例锁住旧发行目录并让最新版本无法成为前台实例。
- 2026-08-27 `v1.1.1` WIM 索引可读性：单系统还原将 WIM 索引控件扩展到整行，第二系统页仍为启动菜单名称留出输入空间。两种页面的下拉列表现在设置为窗口可用宽度，避免索引名称、描述和元数据被 440 像素选择框裁剪。
- 2026-08-27 `v1.1.4`--`v1.1.5` 直启 Rust 回归：产品包移除 `RecoveryLauncher.cmd` 和 `BackupRestore.cmd`，`winpeshl.ini` 直接启动 GUI 子系统的 `Recovery.exe recover-env`。在 ARM64 VM 中从 `F:\BackupRestoreMoved-v1.1.5` 执行任务 `8f180630-1d8d-414e-b166-70ed9301d911`，完成 Windows -> WinRE -> Rust Recovery -> 原 WinRE 恢复 -> Windows，最终 `status.json=success`，载荷不含 `.cmd`，日志记录 `wpeutil.exe reboot`。Recovery 同时校验当前 WinRE 中正在运行的 EXE 和工作目录 payload 的 SHA-256。程序目录与还原目标同卷的拒绝已移到物理卷读取、UAC、任务、WinRE 和 BCD 修改之前；C:->C: 与 H:->H: 均保持任务数不变。

## 已实现的安全骨架

- `backuprestore-core` 提供统一的任务 JSON、卷身份、WIM 哈希/大小校验、容量校验、BitLocker 拒绝策略、原子 `task.json` / `status.json` 写入和状态机。
- 备份、已有 Windows 还原、创建第二 Windows 三种任务都要求源/镜像/目标分区身份明确；镜像与目标、源与目标不能相同，EFI/MSR/Recovery 分区不能作为目标。
- 恢复状态在准备、启动、WinRE、预检、捕获、擦除、应用、修复引导、成功/失败之间单向流转；准备阶段失败也会落盘失败状态，避免“无状态卡死”。
- Rust prepare 检查 Windows 分区、GPT 卷身份、WinRE、BitLocker 保护状态，保存原始 WinRE 与 BCD，并用 SHA-256 清单保护 WinRE payload。
- Rust prepare 和 GUI 不会关闭 Secure Boot；未完成 Secure Boot 实机验证时，不把它当作 WinRE 成功证据。
- `winpeshl.ini` 直接进入 GUI 子系统的 `Recovery.exe recover-env`；不再存在产品批处理启动器或任何批处理恢复回退。Recovery 在挂载程序工作目录所在卷后会校验当前正在运行的 WinRE 二进制与 payload manifest 的 SHA-256。
- `BackupRestore.exe prepare -NoReboot` 只生成并校验临时 WinRE 载荷，不设置一次性启动、不替换注册的 WinRE；准备阶段异常会先丢弃残留 DISM 挂载，再尝试用原始副本恢复注册镜像；DISM 提交后会等待 `wimserv.exe` 释放 WIM，再计算哈希。
- payload manifest 还绑定 `RecoveryTask.env` 的 SHA-256；Rust Recovery 读取 env 后会用工作目录所在卷上的 manifest 再校验一次，避免环境变量文件被替换后继续执行。
- WinRE 执行路径按磁盘号/分区号和卷 GUID 重新挂载固定临时盘符，并逐项比对任务 JSON 与 RecoveryTask.env 的磁盘/分区/大小快照；恢复前再次校验镜像与 metadata，备份完成后原子生成 `metadata.json`，所有破坏性操作都要求 `-AllowDestructive`。
- 准备阶段优先从 `reagentc /info` 的 `harddiskN\partitionM` 路径解析注册 Recovery 分区，并优先选择启动盘上的 EFI；解析失败才使用保守候选筛选，不会静默改动其他分区。
- ARM64 备份元数据记录实际架构、Windows 版本/构建号、已用空间和预留空间；Recovery.exe 的 DISM/BCDBoot 输出同时实时写入日志和 WinRE 控制台，启动修复失败时尝试导入任务创建前的 BCD 快照。
- GUI 在提交前重新读取任务、源、镜像和目标的磁盘 GUID、分区 GUID、偏移、大小、文件系统，并把这些值放入确认对话框。
- GUI 只在提升后的 Rust prepare 成功启动时提示任务准备已开始；备份不会附带破坏性开关，还原失败会保留准备日志路径。
- Rust Recovery 路径带有清理守卫：在 WinRE 已挂载工作目录所在卷后，即使载荷校验、磁盘挂载或 DISM/BCDBoot 提前失败，也会尝试恢复原始注册 WinRE，并保留失败日志。
- 2026-08-21 修复：`recover-env` 不再在 DISM/BCDBoot 完成后立即写入 `success`；它会先恢复并校验注册的原始 `Winre.wim`，清理成功后才完成最终状态转换。清理失败会从当前恢复阶段写入 `failed`，避免任务状态在 WinRE 仍被篡改时虚报成功；清理守卫仍会在退出时做一次最后的恢复尝试。
- 2026-08-21 修复：补齐 `preflight -> success` 状态转换，确保非破坏性的 `probe` 在 WinRE 挂载和校验完成后能够正常落盘最终成功状态。
- TaskStore 读取任务时会先验证请求 ID 为 UUID，再确认文件内 `task_id` 与请求一致，避免错误路径或串任务文件被当成当前任务。
- 工作目录所在卷、镜像卷和还原目标会拒绝 EFI/MSR/Recovery 分区；相对路径拒绝 `.`, `..`、绝对路径和盘符前缀，TaskStore 目录按规范化 UUID 定位且不覆盖已有任务目录。
- 新任务 JSON 记录 `workspaceVolume` 身份；Recovery 会把工作目录所在卷本身也与 RecoveryTask.env 的 GUID、盘号、分区号、偏移、大小、类型、文件系统和序列号复核，避免任务目录被换卷后继续执行。
- Recovery.exe 按持久化阶段断点续跑：备份在 `capturing` 阶段重做临时 WIM；还原从 `target-erased`、`image-applied` 或 `boot-repaired` 选择性重做，并在 BCDBoot 失败时保留 BCD 回滚边界。
- 旧版 PowerShell GUI 曾拆为“首页/环境”“备份与还原”“任务结果/日志”三个页面；当前默认入口已收口为 Rust Win32 单窗口，保留同一安全文案和四种操作模式，探测不携带破坏性开关。
- Rust prepare 在 `<程序目录>\last-task.json` 写入最近任务指针；Rust GUI 的“刷新任务状态”直接读取该 JSON，并明确任务准备不等于 WinRE 恢复成功。
- RecoveryTask.env 记录用户选择的 `IMAGE_ABSOLUTE_PATH`，并保留经过校验的卷内路径供 WinRE 换盘符后使用；缺少 Rust Recovery.exe 时不执行任何兼容恢复逻辑。
- `docs/verification-matrix.md` 按每一项需求列出代码证据、离线证据与实机验收证据，后续交接不得用 AST/单元测试替代自动重启、DISM、格式化或 BCD 证据。
- `probe` 允许任务目录与源分区相同：WinRE 先验证并挂载工作目录所在卷为 `T:`，若源与任务是同一分区则复用 `T:`，不再二次分配 `S:`；真实 backup/restore 仍拒绝工作目录所在卷与源或目标重合。
- 2026-08-21 修复 WinRE 盘符复用：恢复主机现在先按卷 GUID 扫描 `C:` 到 `Z:` 的已有挂载，工作目录所在卷、Recovery 卷、源/镜像/目标卷和 EFI 均复用已存在盘符；只有找不到匹配卷时才执行 `mountvol`/DiskPart 分配。实际使用的盘符会回写到任务内存模型，清理原始 WinRE 也使用实际 Recovery 盘符，避免 WinRE 保留 `C:` 时重复分配 `T:` 卡死。
- 2026-08-21 修复 Windows 构建目录依赖：`build-windows.ps1` 现在为每次 Cargo 调用显式传入仓库 `Cargo.toml`；因此 Parallels Guest Tools 从 `C:\` 启动 PowerShell 时，文档中的构建命令仍会使用指定源码目录，而不会错误在 `C:\` 查找 manifest。
- 2026-08-21 修复：工作目录所在卷重叠校验原先只拦截了 source/target，未覆盖 backup 的 destination 和 restore 的 image volume。已补齐同分区拒绝逻辑，并在 core 端新增回归测试，确保工作目录所在卷不能与镜像卷或目标卷重叠。
- 2026-08-21 ARM64 构建：Recovery payload 会随包携带 `VCRUNTIME140.dll` 与 `VCRUNTIME140_1.dll`；WinRE 启动器把完整 payload hash 校验交给 Rust，兼容 `certutil` 输出中的空格。挂载失败时保留 file-backed DiskPart 日志，便于后续 WinRE 实机排查。
- 2026-08-26 卷身份读取修复：`volume_identity` 不再解析本地化 DiskPart 的星号表格来猜磁盘号和分区号；改用 `IOCTL_STORAGE_GET_DEVICE_NUMBER` 获取物理磁盘号，并从 `PARTITION_INFORMATION_EX` 读取 GPT 分区号/GUID/偏移/容量。新增原生查询注释，避免语言、列布局或卷号变化导致身份错配。
- 2026-08-26 GUI 标签布局修复：四种模式的源/目标标签改为短用途文案（例如“源卷（备份来源）”“目标卷（覆盖还原）”），完整破坏性说明保留在下方详情框，避免最大化窗口中标签出现第三行裁剪。
- 2026-08-26 客体空间清理与任务保留：发现旧测试在 `C:\BackupRestore\tasks` 累积 48 个任务，重复保存 `original`、`stage` 和 `mount` 共约 61.8 GiB；先用 DISM `/Unmount-Image /Discard` 卸载两个孤儿挂载，再删除已确认的历史任务目录。Rust `TaskStore::cleanup_terminal_tasks` 现在在新任务准备前只清理已终态任务的大型 WinRE 目录，保留最近 3 个终态任务的诊断元数据；未完成、格式异常或仍挂载的任务永不自动删除。
- 2026-08-26 多语言默认值修复：中文模式的第二系统默认启动项从 `Windows Backup` 改为 `Windows 备份`；只有用户未修改默认值时切换语言才更新它，用户自定义的启动菜单名称不会被覆盖。
- 2026-08-26 v0.9.3 ARM64 GUI 回归：结束旧 `BackupRestore.exe` 后，前台运行 `C:\\BackupRestoreBuild\\package\\BackupRestore-windows-arm64-v0.9.3\\BackupRestore.exe`。本轮最新包点击“刷新环境”后状态框显示 `Windows 10.0.26200.9168`、`arm64`、WinRE 可用，不再出现本地代码页乱码；此前同一布局代码的 v0.9.1 包已逐页切换探测、备份、单系统还原、新增第二系统和 English，确认字段显隐、纵向详情、标签不裁剪、英文纯净。证据截图位于仓库忽略目录 `.test-artifacts/root-captures/v0.9.3-refresh.png`、`v0.9.1-secondary.png` 和 `v0.9.1-english-secondary.png`。

## 当前验证结果

macOS 本地已完成：

- `cargo test --workspace --all-targets --offline`：核心 crate 12 项测试通过；
- `cargo fmt --all -- --check`：通过；
- `scripts/audit-runtime-boundaries.sh`、`cargo fmt`、`cargo test` 和 `cargo clippy` 均通过；PowerShell AST 仅适用于仍保留的 Windows 构建脚本，不再是产品运行时验收项。
- Windows VM 曾生成 `BackupRestore-windows-arm64-v0.2.8`；上一轮已编译 `BackupRestore-windows-arm64-v0.2.9`，包含状态一致性修复。本轮已编译 `v0.3.0` ARM64 包，包含 probe 状态机修复。这类产物只是编译/打包证据，不是 WinRE 运行验收。
- 2026-08-21 `v0.3.0` ARM64 包已在 Windows VM 内实际启动：`Recovery.exe hash` 返回 `dad44f85e4ba78044a56399625934b61e2548c8e471633fb6a03435e04772631`，与 `build-manifest.json` 一致；`validate-task` 对现有任务 fixture 通过。该证据覆盖 ARM64 进程启动和 schema 入口，不覆盖管理员 WinRE、DISM、BCDBoot 或重启。
- 2026-08-21 管理员 `probe -NoReboot` 实测完成了 BCD 快照、原始 WinRE 复制、DISM 挂载/提交、manifest/status 写入，并确认原始 WinRE hash 未改变；首次实测发现 probe 未携带同目录 `Recovery.exe`，已修复脚本默认路径，使所有操作优先复制同架构 Recovery.exe，只有探针包确实缺少 exe 时才保留兼容入口。
- 2026-08-21 `v0.3.1` 自动 WinRE 入口已实际进入 `RecoveryLauncher.cmd` 并启动 `Recovery.exe`；首次入口测试暴露任务分区在 WinRE 已保留 `C:` 而代码强行申请 `T:` 的挂载缺陷。该缺陷已修复为 GUID 扫描复用逻辑，当前 `v0.3.6` 需要重新构建后再次验证终态和原始 WinRE 清理。
- 2026-08-21 Windows ARM64 编译补充：macOS 上的 `cargo test`/Clippy 不会编译 `#[cfg(windows)]` 分支；首次 VM 构建发现 `mountvol` 参数类型和盘符局部变量初始化问题，已在 `v0.3.5` 修复。每次修改 WinRE Rust 路径后，必须在目标 Windows 架构重新构建，不能只依赖 macOS 离线检查。
- 2026-08-21 新增 `poc/restore-task-winre.ps1`：只允许管理员按 UUID 任务恢复其保存的原始 WinRE；脚本复核任务 ID、Recovery 分区 GUID/类型、env/manifest 原始 hash，拒绝错误的 `R:` 盘符后才复制并复核目标 hash。它用于测试失败后的受控清理，不执行 BCD、格式化或还原。
- 2026-08-21 Win11 ARM64 自动 probe 实测通过：`v0.3.6` 的任务 `c12026c0-6a9e-4093-8a8b-2971968a31f7` 从正常 Windows 进入任务 WinRE，`RecoveryLauncher.cmd` 记录启动 `Recovery.exe`；Recovery 日志记录 probe 完成、原始注册 WinRE 恢复并校验、最终写入 `success` 和 `wpeutil reboot`。返回 Windows 后注册镜像 SHA-256 与任务原始副本一致。该证据只覆盖 probe，不覆盖任何磁盘格式化、DISM Capture/Apply、BCDBoot 或双系统写入。
- 2026-08-22 备份实测发现：中文 Windows 的 DISM 输出可能使用系统代码页而不是 UTF-8；Recovery 原先用 UTF-8 `read_line` 读取原生输出，导致 Capture 已启动后日志线程因 `stream did not contain valid UTF-8` 使任务失败。`v0.3.7` 改为按字节读取、UTF-8 lossless replacement 写日志，不能让日志编码问题中断已经开始的 DISM/BCDBoot。
- 2026-08-22 隔离 EFI 诊断：`prlctl exec --current-user` 对应 `P8B6\\x` 本地管理员账户，但普通进程是 UAC medium token。它调用 `bcdboot S:\Windows /s E: /f UEFI /v` 时，源端 ARM64 `bcdboot.exe`、`bootmgfw.efi` 与 `winload.efi` 一致，随后因对隔离 `HarddiskVolume9` 的 `0x5 Access denied` 失败。`v0.4.1` 改用 `target_root.join("Windows")` 构造源路径，并永久传入 `/v`，使高完整性实测能够保留 BFSVC 细节；仍须用 `x` 的 RunAs 令牌验收，不能使用来宾账户替代。
- 2026-08-22 GUI 收口：测试产物不再放在仓库根目录或桌面；历史项目压缩包/构建文件集中到 `.test-artifacts/desktop-archive/2026-08-22`，历史截图集中到 `.test-artifacts/root-captures/2026-08-22`，两者均被 `.gitignore` 忽略。Rust GUI 默认从无破坏 `probe` 开始，自动提出卷建议并新增 WIM 索引字段；提交时将索引传入 `BackupRestore.exe prepare -WimIndex`。
- 2026-08-22 Windows PowerShell 5.1 GUI 烟测首次发现 here-string 解析失败，已改为字符串数组拼接并纳入后续 ARM64 包重建门槛；macOS PowerShell 7 AST 不能替代目标 Windows PowerShell 5.1 解析。
- 2026-08-22 GUI 技术路线纠正（历史记录）：用户要求 Rust 开发，`BackupRestore.exe` 改为直接进入 `native_gui.rs` 的 Win32 原生窗口；当前版本已完成 Rust 收口，GUI、prepare 和 Recovery 均由 Rust 实现，不再调用隐藏 PowerShell 后端。
- 2026-08-22 `v0.4.6` ARM64 构建：Windows VM 使用已有 `aarch64-pc-windows-msvc` 工具链和本地 Cargo target 生成包；`BackupRestore.exe` 的 SHA-256 为 `3180d5b30f34e0c4512c44ad0c8470a0bb7cf1874f6793f97015b884f6061272`，与 `build-manifest.json` 一致，后台进程标题为 `BackupRestore - Rust GUI`。这只证明目标编译和进程烟测，不证明真实按钮、UAC、WinRE 或恢复流程。
- Rust GUI 镜像选择改为绝对 Windows 路径（例如 `B:\BackupRestore\Windows.wim`）；创建任务时从路径根解析镜像卷并记录完整身份，WinRE 仍用同一卷的内部路径重建实际挂载路径。相对路径不再作为用户输入。
- Rust GUI 的正常 Windows 启动使用 Windows 子系统；WIM 读取通过 Rust 调用 DISM `/Get-WimInfo` 并解析索引详情，下拉项展示序号、名称、描述、版本、架构、版本类型和大小。
- 2026-08-23 清理 Parallels VM：删除 `C:\BackupRestoreBuild` 下旧版本构建、日志和测试文件，重建为浅层 `src`、`target`、`package`；仍挂载的旧测试盘 `backup-fixture\source.vhdx` 因 Windows 文件锁保留，未强制卸载或删除。
- 2026-08-23 `v0.5.2` ARM64 构建修复：VM 首次重建发现 WIM 多索引 JSON 数组分支把 `serde_json::Value` 按值传入借用函数，导致 Windows 目标编译失败；已修复为按引用读取并同步两个 Cargo manifest、`Cargo.lock`、`VERSION`。使用已有工具链在 `C:\BackupRestoreBuild\src` 编译，输出包为 `C:\BackupRestoreBuild\package\BackupRestore-windows-arm64-v0.5.2`；未下载新工具链，未执行真实还原或重启。
- 2026-08-23 `v0.5.3` WIM 权限回退修复：普通令牌下 `Get-WindowsImage` 可能返回 JSON 但 `images` 为空，原逻辑误以为读取成功；新增空索引检测，自动走隐藏管理员 PowerShell 重试。ARM64 包已重建并真实启动 GUI，管理员读取 `B:\BackupRestore\Windows.wim` 返回索引 1（`Windows Backup`），下拉框实测显示序号、名称、无描述、未知字段和 162.9 MiB 大小；两个 EXE SHA-256 均为 `1e097ab33b01348db5515f41b25b56313f88f5b4ed9485e2782b7d4905ffc6dd`。测试期间出现的黑色终端是 Parallels 测试命令窗口，已关闭，不属于 BackupRestore GUI。
- 2026-08-24 `v0.5.4` 准备脚本修复：`probe` 不提供镜像路径时，原逻辑仍对空字符串调用 `Test-Path`，并无条件拼接 `metadata.json`，导致 PowerShell 5.1 抛出 “Path 为空”。现在对 `IMAGE_SHA256` 使用非空绝对路径保护，`last-task.json.metadataPath` 仅在存在镜像信息时生成；这样 `probe -NoReboot` 可以继续生成完整任务状态。该修复已通过本机 PowerShell AST 和离线 Rust 检查；Windows VM 的下一次 probe 需在降温后恢复快照再执行。
- 2026-08-24 VM 降温记录：测试期间 Parallels `prl_vm_app` 约 92–104% CPU、约 6.8 GiB 内存，WindowServer 约 60%，而 `pmset -g therm` 未报告 thermal/performance warning。已安全挂起 `Windows 11` VM；挂起后 CPU idle 约 86%、可用内存约 6.4 GiB。恢复 VM 前不要继续高频截图或实机压测。
- 2026-08-24 `v0.5.4` VM 无破坏验证：恢复降温后的 Windows 11 ARM64 VM，使用修复后的 `BackupRestore.exe prepare` 执行 `probe -NoReboot` 成功，任务 `760fcfe1-392d-4142-94af-b830f4286296` 写入 `task.json`、`status.json`、`manifest.json`、`RecoveryTask.env` 和 payload；`BackupRestore.exe validate-task` 退出码为 0，原始 WinRE 与注册 `R:\Recovery\WindowsRE\Winre.wim` 均为 `0CBC86B44994065C7295F0322DF670CF0B6C9E4A7BE5099CFD962DDEC956FDA1`。`recover --dry-run` 退出码为 0，状态仍为 `prepared`，没有执行恢复操作。
- 同一 VM 的容量/破坏性守卫验证：`backup` 从 C: 捕获到 B: 因可用 `24,038,313,984` 字节小于所需 `250,431,545,344` 字节而退出码 1；`Test.wim.partial` 不存在，注册 WinRE 哈希未变。`restore-existing` 未传 `-AllowDestructive` 时退出码 1 并报告 `Restore requires -AllowDestructive.`，没有进入格式化、DISM Apply 或 BCDBoot。
- 2026-08-23 `v0.5.0` ARM64 重建：Windows 参数帮助已显示 `-ImagePath`，旧 `-ImageDrive` / `-ImageRelativePath` 不再是脚本参数；Rust GUI 增加保存/打开文件对话框。ARM64 manifest 与两个二进制哈希均为 `beb50a2f74852285f4508a3b1510ae9f5c2ab7f6dbd02f0b2ca852749ae01073`，`Recovery.exe hash` 返回 0。PowerShell 5.1 后端补充 UTF-8 BOM，目标 Windows AST 解析通过。
- 2026-08-22 GUI 刷新反馈：环境和最近任务状态按钮在执行隐藏 PowerShell 查询前立即显示进行中状态，避免用户误以为按钮没有响应；查询完成后再替换为结果或错误文本。
- 2026-08-22 隔离恢复收尾：使用 `v0.4.7` 在 Win11 ARM64 VM 的非启动测试卷 `S:` 和独立 EFI `E:` 完成真实 Capture/Apply/格式化/BCDBoot。备份任务 `ff6b645b-b9e4-4b4e-945a-1fb406923b0d` 成功，WIM SHA-256 为 `c08c4e7a9628ead708802ca880f46932a4cb6e0547f0cad735bf79ef29711b30`；还原任务 `821eb6af-f13c-46b2-8c1c-af1aa8345e42` 最终为 `success`。高完整性日志包含 `bcdboot.exe S:\Windows /s E:\ /f UEFI /v`、OS loader identifier 和成功返回；管理员 `bcdedit /store E:\EFI\Microsoft\Boot\BCD /enum all /v` 返回 0。该测试没有让 VM 从 E: 实际启动，因此不外推为真实恢复盘重启成功。
- 2026-08-22 `v0.4.8` ARM64 构建：源码通过共享桌面压缩传输到 `C:\BackupRestoreBuild\source-v0.4.8`，使用既有 `aarch64-pc-windows-msvc` 工具链和 VM 本地 target 离线构建成功。`build-manifest.json`、`BackupRestore.exe`、`Recovery.exe` 的 SHA-256 均为 `526c612d8222920bf76f91e4fb4b04ff413cd555a7f9969f802cb6c0ca798050`；`Recovery.exe hash .\Recovery.exe` 返回 0，GUI 进程标题为 `BackupRestore - Rust GUI`。这只证明版本、架构、载荷哈希和启动烟测，不证明真实按钮、UAC、WinRE、DISM、BCDBoot 或重启。
- 指定分区核实：正常 Windows 准备脚本通过 `-SourceDrive`、`-TargetDrive` 接收任意盘符，镜像卷由 `-ImagePath` 绝对路径的根盘符解析，并在任务 JSON 中持久化完整卷身份；Recovery 只把盘符当临时挂载提示，实际通过 volume/disk/partition GUID、偏移、容量、类型、文件系统和序列号复核。`v0.4.7` 的真实隔离 Capture/Apply 测试使用 `S:` 源/目标、`B:` 镜像和 `E:` 独立 EFI，未触碰真实 `C:`，证明当前路径不是 C: 专用。脚本中的 `C:\ProgramData`、`C:\WinRE-PoC` 仅是主机日志/WinRE 临时日志位置，不是数据源或还原目标。
- Rust 默认 GUI 增加 `中文` / `English` 选择器：切换会更新操作项、字段标签、按钮、校验、确认和镜像/任务状态摘要；内部仍传递稳定的 `probe`、`backup`、`restore-existing`、`create-secondary` 操作值。
- 2026-08-24 `v0.5.6` Rust GUI 收口：删除旧桌面前端和构建复制规则，`BackupRestore.exe` 成为唯一桌面 UI；PowerShell 查询统一使用 `CREATE_NO_WINDOW`，管理员调用使用 `-WindowStyle Hidden`。任务、源、目标分区改为详细下拉框，条目显示盘符、文件系统、卷标、总容量、剩余容量、磁盘/分区号，身份区显示卷 GUID；`probe` 创建任务强制追加 `-NoReboot`。
- 2026-08-24 `v0.5.7` 中文 UI 修复：中文模式不再显示内部英文操作名或“语言 / Language”；操作项显示“探测（仅检查）/备份/单系统还原/新增第二系统”。右侧操作说明和卷身份改为可换行只读多行控件，窗口扩大并重新分栏，避免说明覆盖 WIM 下拉框或被裁剪。
- 2026-08-24 `v0.5.8` UI 结构修复：操作模式改为顶部四个可点击标签按钮，不再使用操作下拉框；工作目录所在卷、源卷、目标卷完整信息移动到窗口底部三栏，分别显示卷标、文件系统、容量、剩余空间、磁盘/分区、分区类型和卷 GUID；所有控件统一显式使用 Windows `DEFAULT_GUI_FONT`。
- 2026-08-24 `v0.5.9` 布局收口：去掉右上角说明框，模式提示改由中部白色状态框承载。工作目录所在卷、源卷、目标卷按原始交互逻辑恢复为纵向三段：左侧标签说明用途，下拉框完整展示分区摘要，详情框直接位于对应下方。详情首段不只写名称，而是按 `probe`、备份、单系统还原和新增第二系统解释用途与破坏性边界；无关字段按模式隐藏，窗口创建后最大化，语言选择器与模式标签同一行。`set_text` 把所有逻辑换行规范为 Windows `CRLF`，避免 EDIT 控件把说明挤成一行。
- 2026-08-24 `v0.6.0` GUI 实机点检：Win11 ARM64 Console 会话中由 `BackupRestore.exe` 最大化启动，确认语言选择器同操作模式一行；工作目录所在卷和源卷纵向选择区的标签、完整下拉摘要和对应说明详情框均可见。编译只使用 VM 已有 ARM64 Rust/MSVC 工具链，没有下载。
- 2026-08-24 `v0.6.1` 反思修复：探测模式隐藏 WIM 索引，不能凭 probe 截图推断还原模式没有控件覆盖。通过逐项检查坐标发现旧索引位置仍在中部提示框内，现已移动到镜像路径之后；以后任何 `ShowWindow` 条件显示的控件均须切换到可见模式做实机截图验收。
- 2026-08-24 `v0.6.1` 完整 GUI 点检：为避免 Parallels 前台鼠标映射把光标送到客体却不触发控件，使用同一 Console 会话的临时 Win32 `SendMessage(WM_COMMAND)` 辅助程序，仅向 `BackupRestoreNativeGui` 发送四个标签的命令 ID。逐页截图确认：探测只保留任务/源，备份显示镜像路径，还原显示目标与 WIM 索引，第二系统再显示启动项名称；没有创建任务、执行 UAC、格式化或重启。临时源码传输压缩包已从桌面移入项目忽略的 `.test-artifacts/desktop-archive/2026-08-24/`。
- 2026-08-25 `v0.6.2` UI 合理性修复：前台截图复核发现详情框仅能显示卷标、文件系统和容量，分区 GUID/卷 GUID 被迫滚动；同时固定坐标在最大化窗口右侧和底部浪费空间。新增客户区尺寸读取和 `WM_SIZE` 重排，三个详情框提升到可完整显示身份的高度，字段宽度填充可用区域，模式专属的镜像/WIM/启动名称及按钮随纵向内容移动。后续每个窗口尺寸变化都应再次检查字段是否仍完整可读。
- 2026-08-25 `v0.6.3` 回归修复：真实控件坐标显示切换到备份时镜像编辑框与操作按钮重叠，原因是 `select_operation` 未触发 `layout_operation`。补上切换后的重排调用；验证必须分别读取四个模式下的控件矩形，不能只看探测页。
- 2026-08-25 `v0.6.4` 垂直布局修复：第二系统页的 130 高度详情框把底部控件推至任务栏边缘；将详情高度压到 120、行间距压到 14。由于详情文本在最大化宽度下不换行，6 行卷身份仍可完整显示，同时四个模式底部按钮保持在客户区内。
- 2026-08-25 `v0.6.5` 客户区高度修复：实机截图确认第二系统页按钮仍被任务栏覆盖；继续压缩到详情 112、行距 8、状态框 64，并缩短状态/镜像/按钮间隔。通过客体控件矩形和截图双重检查，不能只凭逻辑坐标判断可见性。
- 2026-08-25 `v0.6.5` 最终验证：探测/备份/单系统还原/新增第二系统均在 Windows Console 会话切换并读取实际控件矩形，确认按钮和模式专属字段不重叠；第二系统截图确认卷详情 6 行身份信息、镜像路径、WIM 索引、启动项名称和按钮完整可见。Parallels 控制中心与 Windows 客体画面严格区分，客体证据只来自 `prlctl capture` 和 Windows 会话。
- 2026-08-25 快照清理复核：`prlctl snapshot-list` 只返回两个有明确用途的快照，未发现可安全删除的无用快照，因此不执行删除，避免破坏隔离还原回滚基线。
- 2026-08-25 GUI 只读按钮复核：`刷新环境` 能更新 Windows/固件/NTFS 信息；`读取镜像` 在现有 WIM 上暴露单索引 JSON 解析缺陷，未把误报当成功。修复后必须重新构建并验证索引下拉框及镜像 SHA-256/metadata 状态；不触发创建任务。
- 2026-08-25 `v0.6.7` 诊断边界：兼容数组/对象/`images` 包装后，实机仍返回无索引。新增受限原始 JSON 诊断到状态栏；这只是定位措施，不是功能成功证明。
- 2026-08-25 `v0.6.8` WIM 回退修复：普通令牌 `Get-WindowsImage` 空结果不是 WIM 损坏；同一 WIM 的 DISM 文本输出包含 Index/Name/Size。GUI 现在在 JSON 无索引时调用 DISM `/English /Get-WimInfo` 并解析索引下拉项，仍只读，不触发恢复。
- 2026-08-25 `v0.6.9` Windows-only 编译坑：`parse_dism_size` 返回 `Option` 时不能直接对 `Result` 使用 `?`；应为 `.parse::<u64>().ok()?`。macOS 的 `cfg(windows)` 排除了该模块，必须把目标 ARM64 构建作为该类修改的必要检查。
- 2026-08-25 `v0.7.0` WIM 回退诊断：v0.6.9 实机仍显示空索引，不能假定 DISM 回退成功；现在错误状态同时记录回退 DISM 输出，区分路径、权限、stderr 和文本解析失败。
- 2026-08-25 `v0.7.1` 提权策略：DISM 740 表明 GUI 不能依赖隐藏子 PowerShell 获得管理员令牌。前端进程本身在创建窗口前检查 `TokenElevation`，非提升时以 `runas` 重启；这覆盖只读 WIM、环境查询和管理员任务准备的一致令牌边界。VM 自动提升策略仍需以实际令牌读取验证。
- 2026-08-25 `v0.7.2` 英文布局：顶部标签宽度按中文设计，不能容纳内部 CLI 名称。英文改为短 UI 名称，说明框承担完整语义；以后不得把 CLI operation key 直接当作固定宽度按钮文字。
- 2026-08-25 `v0.7.3` 多语言细节：英语截图仍暴露 `Language / 语言` 的混合标签，英语 UI 改为纯 `Language`。语言切换的每个静态标签都需同目标语言一致。
- 2026-08-25 `v0.7.4` PowerShell 5.1 编码：隐藏查询的 stdout 默认使用系统代码页，Rust `from_utf8_lossy` 会把中文任务状态显示为乱码。所有 `powershell_output` 调用统一在脚本前设置无 BOM UTF-8 输出；必须用真实中文任务状态复验。
- 2026-08-25 `v0.7.4` 实机确认：Windows ARM64 最新包在高完整性 GUI 中成功读取 WIM 索引 1，英文标签与 `Language` 纯英文；切回中文后任务状态字段显示正常中文。所有证据通过 Windows 客体会话和 `prlctl capture` 获取，未操作 Parallels 控制中心作为客体。
- 2026-08-25 非 C/多索引实测：U: fixture -> T: 镜像的真实备份成功；由此 WIM 导出两个索引。还原首次因自动 EFI 选择系统盘而未进入 Recovery，暴露隔离测试无法指定 EFI 的设计缺口；新增 `-EfiDrive`，生产默认行为不变。
- 2026-08-25 WinRE EFI 临时盘符：Recovery 里镜像卷可能被挂载为 E:，不能再把 E: 作为 EFI 固定盘符。改用 Z: 偏好并保留 GUID 校验；BCD rollback 不再硬编码 E:\EFI。
- 2026-08-25 `v0.7.7` 准备阶段参数边界：核心 `validate-task` 虽能拒绝工作目录所在卷和镜像卷重叠，但此前 PowerShell 已在调用核心验证前导出 BCD、注入 WinRE 并请求重启。该校验已前移到 `Get-VolumeIdentity` 后，GUI 同时在创建任务时拒绝任务盘符等于镜像路径根盘符；仍以 GUID 对比作为脚本的最终判断，盘符只用于即时交互反馈。
- 2026-08-25 `v0.7.9` 程序目录即工作目录：桌面 GUI 不再显示或接收工作目录卷；PowerShell 在任何日志/WinRE/BCD 写入前先解析脚本目录所在卷并与还原目标 GUID 比较。任务路径统一为 `<程序目录>\tasks\<任务 ID>`，`WORKSPACE_*` 环境字段记录该卷 GUID、磁盘/分区身份和程序目录相对卷根的路径，Recovery 在 WinRE 中按身份挂载并回到同一相对目录。备份可与程序目录同卷；单系统还原和新增第二系统若目标同卷，只返回单按钮阻止提示。移动整个程序目录后不需要注册表或固定系统目录，下一次运行会在新目录创建任务、日志和状态。
- 2026-08-25 `v0.7.7` 独立 EFI 启动结果：将测试 EFI `hdd2` 临时置于 Parallels 首启动后，固件进入 Windows Recovery 并返回 `0xc0430001`，没有进入 `U:`。恢复原顺序后正常 Windows 启动。该失败说明需要继续核对独立磁盘的 BCD device/osdevice、EFI 与 Windows 分区的关联以及 Secure Boot/固件路径；在修复前不得宣称独立 EFI 启动验收通过。
- 2026-08-25 `v0.7.8` EFI 二次诊断：管理员 `bcdedit /store E:\EFI\Microsoft\Boot\BCD /enum all /v` 显示默认 U: loader 的 `device` 与 `osdevice` 均为 `partition=U:`；U: 为 GPT 磁盘 3 分区 3，E: 为独立 FAT32 GPT 磁盘 2 分区 2，U: 的 `winload.efi` 和 E: 的 ARM64 boot files 均存在。执行 `bcdboot U:\Windows /s E: /f UEFI /v` 成功并生成新 loader，但从 `hdd2` 首启动仍复现 Recovery `0xc0430001`。因此旧 BCD 混入历史 VHD 项不是唯一根因；未继续修改生产 EFI，启动顺序已恢复，VM 回到 C: Windows。后续需要在隔离快照中验证跨磁盘 UEFI loader、Secure Boot 策略及分区关联，不能把此失败归因于单一 BCD 字段。
- 2026-08-25 开发 EFI 结论：`0xc0430001` 是 `STATUS_SECUREBOOT_ROLLBACK_DETECTED`。关闭 Parallels Secure Boot 后以及将 E: boot manager 替换为 U: 同 SHA-256 版本后，hdd2 首启动仍显示相同错误，故并非“只要关闭 Secure Boot”或“EFI bootmgfw 版本不同”即可修复。独立 EFI 仅保留为开发测试：普通 GUI 没有 EFI 选择；仅 Rust CLI 的 `--test-efi-drive` 可指定隔离 EFI，生产默认流程自动选择真实 GPT EFI 分区。
- 2026-08-24 快照清理：Windows 11 VM 原有 7 层串联快照，已删除 5 个早期冗余点（`before-winre-auto-launch`、`backupRestore-before-winre-validation`、`快照 1`、`before-v0.3.6-winre-probe`、`before-v0.3.6-backup-fixture`），保留 `before-v0.3.8-fixture-restore` 和当前 `before-v0.3.9-isolated-restore`。快照目录从约 22 GiB 降到 6.8 GiB；VM 停止/启动状态均未改动测试磁盘内容。
- 2026-08-22 fixture 根因：WinSxS 下 3224 字节的 `BCD-Template` 在该 ARM64 VM 上无法作为 BCDBoot 模板加载；`C:\Windows\Boot\DVD\EFI\BCD` 又不含可用 OS loader。管理员环境中实际的 `C:\Windows\System32\config\BCD-Template` 为 20480 字节，复制到测试源后 BCDBoot 成功。测试 fixture 脚本已优先检查该系统模板，并对过小文件拒绝继续；fixture 目录被 `.gitignore` 忽略，不进入产品包。

## 2026-08-26 v0.9.8 逻辑审计补强

prepare 在导出 BCD、写入 WinRE 前再次运行完整 Task::validate；VolumeIdentity 同时记录真实卷序列号；格式化目标后再次核验磁盘/分区 GUID、类型、编号、偏移和容量；WinRE 挂载完成后逐字段复核工作区、源、镜像、目标和 EFI 身份。

## 2026-08-26 v0.9.9 审计收口

prepare 在 BCD 导出前先完成完整任务校验，导出并记录 BCD 哈希后再启用一次性启动计划并再次校验，避免校验顺序导致合法任务被误拒绝。

## 2026-08-26 v1.0.0 事务与身份审计修复

- WinRE 已挂载卷即使通过卷 GUID 找到，也必须再次核对磁盘/分区 GUID、类型、文件系统、编号、偏移、容量和卷序列号；不再因“已挂载”路径跳过复核。
- WinRE 盘符分配脚本改写到可写的 `X:\Windows\Temp`，并等待 DiskPart 自然退出（最长 30 秒），不再固定等待 5 秒后强制终止。
- `RecoveryTask.env` 的任务身份字段改为必需并与 `task.json` 逐项比较；载荷内不可变 `task.json` 也必须与工作区任务完全一致，仅允许工作区状态从 `Prepared` 前进到 `BootRequested`。
- 正常 Windows 准备阶段先持久化 manifest 和任务状态，再替换注册 WinRE；替换、`reagentc /boottore`、任务状态写入或重启请求失败时恢复原始 WinRE，并导入 BCD 快照。
- BCDBoot 后读取实际 BCD，确认 `device/osdevice` 至少有一个 Windows loader 指向所选目标分区；不满足时恢复阶段失败并进入既有 BCD 回滚路径。
- `prepare --target-drive` 对探测/备份改为可选；隐藏的目标卷不会再参与备份/探测的 BitLocker 和卷校验。

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
- 2026-08-26 VM 系统目录审计：WinSxS 实际约 19.75 GiB，其中 7 个包被 DISM 标记为可回收，系统报告建议组件清理；`System Volume Information` 的卷影副本配额已用约 4.08 GiB。两者均属于 Windows 系统恢复/更新数据，本轮未直接删除；后续若清理，必须由用户明确确认具体范围。
- 2026-08-26 v0.9.5 清理边界复核：终态任务自动清理现在必须同时通过完整 `Task::validate()`、任务/状态 operation 一致性和终态检查；任何缺字段、身份不完整或记录不一致的目录都计为 malformed 并保留。
- 2026-08-26 系统恢复数据清理（用户明确授权）：在 Windows 11 ARM64 测试虚拟机中执行 `vssadmin delete shadows /for=C: /all /quiet`，删除 C: 上 4 个卷影副本；随后执行普通 `DISM /Online /Cleanup-Image /StartComponentCleanup`，WinSxS 从约 19.75 GiB 降至约 12.60 GiB，7 个可回收包中先移除 5 个。经用户明确允许后继续执行 `/StartComponentCleanup /ResetBase`，永久丢弃旧更新回滚基线；ResetBase 退出码为 0，随后再次执行普通组件清理也成功。最终 AnalyzeComponentStore 报告实际 12.53 GiB、仍有 2 个可回收项（对应 staged 按需功能/语言包，不能手工删除），`vssadmin list shadows /for=C:` 无卷影副本，C: 可用空间约 181.3 GiB。CheckHealth/ScanHealth 仍报告组件存储可修复；未执行 `RestoreHealth`，因为它可能需要下载源文件且当前网络是热点。该结果已明确区分“清理成功”与“组件健康仍需来源修复”。
- 2026-08-26 v0.9.7 逻辑审计修复：WinRE 挂载后现在重新读取并核对卷 GUID、磁盘/分区 GUID、分区类型、文件系统、磁盘号、分区号、偏移和容量；格式化后再次核验目标身份，防止盘符复用或目标被替换。异常清理守卫改用真实 Recovery 卷盘符；BCDBoot 只接受明确绑定到目标分区的 loader，不再回退误改其他启动项；程序目录与还原目标的阻止在管理员提升前执行。GUI 标题包含版本号，并为操作标签、分区、镜像、索引和按钮增加悬停提示。
- 2026-08-27 v1.0.1 Recovery 分区定位修复：准备阶段不再忽略 DiskPart 失败后读取固定 `R:`；先扫描已挂载卷的真实磁盘/分区号，再从多个可用盘符尝试分配，且逐项核对磁盘号、分区号、卷/GPT GUID、分区类型、文件系统、偏移和容量。这样 WinRE 中 `R:` 被占用或 DiskPart 部分失败时会安全拒绝，不会把错误卷当成 Recovery。
- 2026-08-27 v1.0.2 隐藏 EFI 定位修复：默认系统 EFI 没有盘符时，准备阶段先复用已挂载 EFI，否则用 `mountvol /S` 临时挂载到空闲盘符，读取完整 GPT 身份后立即卸载；未找到或类型不符时安全失败。ARM64 v1.0.2 已重新构建并校验版本、manifest 与两个二进制哈希，GUI 已以前台最大化运行。
- 2026-08-27 v1.0.3 空闲盘符判定修复：Windows `mountvol <letter>: /L` 对未分配盘符正常返回退出码 1，原判定把所有空闲盘符误认为不可用，导致 EFI 临时挂载始终跳过。现在将“无挂载点”的退出码 1 视为空闲，并在挂载后继续完整身份核验；其它异常仍由后续分配/核验失败安全拦截。
- 2026-08-27 v1.0.4 多语言 tooltip 修复：悬停提示不再把中英文拼接在同一条文案中；提示内容跟随当前语言，并在语言切换时销毁旧 tooltip、重新注册当前语言文本，避免中文界面残留英文。ARM64 包 `BackupRestore.exe` 与 `Recovery.exe` SHA-256 均为 `88399f72a55b7af59ed85173d13a27f60fc4dc18c14de502dcf7c801d322115f`，窗口标题已显示版本号；客体真实悬停弹框尚未获得可靠截图。

## 2026-08-27 `v1.2.5` BCD 回滚验证副作用

- `bcdedit /store ... /enum` 在字节级恢复后可能重写 BCD hive 的事务元数据，使刚复制并校验通过的文件 SHA-256 改变。故障回滚现在不再调用该枚举命令；以原始快照复制后的立即 SHA-256 相等作为权威证据，避免验证步骤改变被验证对象。

## 2026-08-27 `v1.2.6` 断电后待恢复任务续跑

- 实测在 `reagentc /boottore` 与实际重启之间强制断电，会留下唯一的 `boot-requested` 任务而正常进入 Windows；旧启动入口没有再次请求 WinRE。Rust GUI 启动前现在扫描程序目录下的任务，要求 `task.json`/`status.json` 一致、任务 ID 合法且 Recovery payload、manifest、原始/暂存 WinRE 均存在；确认唯一合法任务后隐藏调用 `reagentc /boottore` 与 `shutdown /r /t 0`，让用户重新启动程序即可继续。多个、损坏或缺少载荷的待恢复任务只写 `launcher-errors.log`/`prepare.log`，不猜测、不自动执行。
# 2026-09-09 v1.3.3 当前基线

完整状态已整理到 [current-progress-2026-09-09.md](current-progress-2026-09-09.md)。本版本包含当前 EFI 自动定位、Windows Boot Manager 状态保留、阶段中断续跑、盘符变化容忍、目标格式化后序列号处理和一次性故障注入 marker。macOS 离线检查为 CLI 8/8、core 17/17、fmt、Clippy、运行时边界审计和 diff 检查全部通过；Windows ARM64 Release 构建曾成功。三种新阶段 fault 的最新版 WinRE 实机最终状态仍待回归，不能以旧 v1.3.0/v1.3.1 任务替代。
