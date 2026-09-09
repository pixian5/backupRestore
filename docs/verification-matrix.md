# V1 需求与证据矩阵

这份矩阵防止把静态代码检查误报为 Windows/WinRE 实机成功。状态只允许使用：

当前版本基线：`1.3.3`。版本、失败根因和下一轮执行顺序见 [current-progress-2026-09-09.md](current-progress-2026-09-09.md)；下方历史任务 ID 保留为证据，不等同于最新版已回归。

- **代码已覆盖**：源代码和离线测试已覆盖，仍可能需要实机确认；
- **离线已验证**：本机命令已经通过，但不等同于 Windows 运行；
- **实机待验证**：必须在管理员 Windows 11 ARM64 快照中执行；
- **实机已验证**：有对应 Windows VM 日志、持久化状态和回归证据；适用范围必须写明，不能外推到未执行的破坏性流程；
- **不在 V1**：需求明确排除。

| 需求 | 代码证据 | 当前状态 | 实机验收证据 |
|---|---|---|---|
| Windows 10/11、UEFI、GPT | `windows_prepare.rs`、`VolumeIdentity` | Rust prepare 已在 ARM64 编译；完整架构/UEFI拒绝待补测 | `prepare.log` 中 OS、WinRE、GPT 身份检查通过 |
| x64/ARM64 架构隔离 | `windows/build-windows.ps1`、`build-manifest.json`、`Assert-PackageArchitecture` | `v0.4.8` ARM64 二进制已在 VM 启动并通过 manifest/`Recovery.exe hash` 检查，WinRE 运行待验证 | x64/ARM64 各自产物在对应 Guest 启动并拒绝错架构 |
| 当前/候选 Windows 分区枚举 | Rust `discover_drives`、`Refresh environment` | 代码已覆盖 | GUI 实盘显示下拉项的文件系统、容量、剩余空间和 GUID |
| 盘符不是身份 | `VolumeIdentity`、`verify_task_identity_env`、`mount_env_volume` | 代码已覆盖 | 改盘符或更换卷后任务必须拒绝 |
| Recovery 盘符冲突安全边界 | `identity_from_diskpart`、`ensure_volume_mounted` | **代码已覆盖；v1.0.1 ARM64 重建通过** | 已挂载卷先按实际磁盘/分区身份复用；固定 `R:` 被占用或 DiskPart 失败时不会读取错误卷，实机故障注入待验证 |
| 镜像使用绝对路径 | `ImagePath`、`validate_absolute_path`、task `absolutePath`、`IMAGE_ABSOLUTE_PATH` | `v0.5.0` 代码和 ARM64 参数帮助已验证；Recovery 仍按 GUID 重挂载后使用卷内路径 | Windows GUI 选择 `B:\...\Windows.wim`，换盘符后必须仍解析到同一镜像卷 |
| probe 任务/源卷相同 | `recover-env` 按卷 GUID 扫描已有挂载，任务/源同卷时复用实际盘符 | 实机已验证（Win11 ARM64 probe） | 任务 `c12026c0-6a9e-4093-8a8b-2971968a31f7` 在 WinRE 记录 `TASK volume already mounted at C:; reusing it`，随后为 `success` |
| EFI/MSR/Recovery 保护 | core `is_reserved_partition`、Rust `windows_prepare`、Rust Recovery | Rust 代码与 ARM64 原生 GPT 枚举已验证 | 实盘尝试选中三类分区都被拒绝 |
| 备份 `.partial`、WIM 校验、SHA-256、metadata | `recover_windows` backup、`BackupMetadata`、原生输出按字节日志 | 实机已验证（隔离源卷/镜像卷）；容量拒绝分支已验证 | `v0.4.7` 任务 `ff6b645b-b9e4-4b4e-945a-1fb406923b0d` 为 `success`；WIM `c08c4e7a9628ead708802ca880f46932a4cb6e0547f0cad735bf79ef29711b30` 与 metadata 一致。新一轮 C: -> B: 因 24.0 GB < 250.4 GB 被拒绝，`.partial` 不存在 |
| 镜像容量不是目标容量 | metadata `required_target_size`、Rust prepare/Recovery 双重检查 | 离线已验证 | 小目标卷在格式化前拒绝 |
| BitLocker 不自动修改 | Rust `manage-bde.exe -status` 只读检查 | 代码已覆盖 | 开启保护的源/镜像/目标任务拒绝且状态不变 |
| 临时 WinRE 副本和原始 hash | Rust `prepare`、manifest、`WinreRestoreGuard` | **实机已验证（v1.1.5 ARM64 直启 Rust probe）** | 任务 `8f180630-1d8d-414e-b166-70ed9301d911` 完成后 `status.json=success`，任务载荷不含 `.cmd`，日志记录 `wpeutil.exe reboot`，注册 WinRE hash 已恢复。 |
| WinRE 自动启动 Recovery.exe | `winpeshl.ini` 直接运行 `Recovery.exe recover-env` | **实机已验证（v1.1.5 ARM64）** | WinRE 直接加载 Rust `Recovery.exe`；Recovery 校验正在运行的 EXE SHA-256，完成后恢复原 WinRE 并返回 Windows。没有 `.cmd` 启动器或控制台窗口。 |
| 一次性启动后返回正常 Windows | `reagentc /boottore`、清理/重启路径 | 实机已验证（Win11 ARM64 probe） | `recovery.log` 记录 `wpeutil.exe reboot`；VM 屏幕和 Guest Tools 均确认已回到正常 Windows，未出现 WinRE 循环 |
| Rust prepare 自动进入 WinRE | Rust `prepare`、`winpeshl.ini`、Rust `recover-env` | **实机已验证（v1.1.5 ARM64 probe）** | 任务 `8f180630-1d8d-414e-b166-70ed9301d911` 从 F: 程序目录完成自动启动、Rust Recovery 清理和 Windows 返回；旧启动器链路只作为历史基线。 |
| 单系统还原 | `restore-existing`、DiskPart format、Apply-Image、BCDBoot `/v` | **实机已验证（v1.3.0 ARM64，P/H，Index 2/3）** | 任务 `312a7cc0-5c15-4d29-9e7b-3ead67644a16`（v1.2.9 包 Index 2）和最终包 Index 3 的恢复链均完成；P 目标完成格式化、Apply、BCDBoot、WinRE 清理和重启，P/Q fixture 清单按相对路径与 SHA-256 无差异。 |
| Rust prepare 多索引单系统还原 | Rust `prepare`、Rust `recover-env`、`restore-existing` | **实机已验证（v0.8.2 ARM64）** | 任务 `fcfdd192-61e0-4b14-b04f-9734dcd26e48` 从 B: 工作目录使用 T: Index 2 还原 U:，DiskPart/DISM/BCDBoot/WinRE 清理完成；U: SYSTEM hash 为 `A70A0D…CC550`，E: BCD 默认 loader 指向 U:。 |
| 独立 EFI 实际引导（开发测试） | Parallels `hdd2` 首启动、EFI E:、U: 已 Apply Windows | **实机失败（ARM64，已复测）**；不属于普通 GUI/V1 发布门槛 | E: BCD 默认 loader `device/osdevice=partition=U:`，`bcdboot U:\Windows /s E: /f UEFI /v` 成功；关闭 Secure Boot、并替换 E: bootmgfw 为 U: 同哈希版本后，hdd2 仍进入 Recovery `0xc0430001`。启动顺序和 Secure Boot 已恢复。 |
| 双系统还原 | `create-secondary`、`/addlast`、BCD menu name | **实机已验证（v1.3.0 ARM64，P/H/Q，Index 1/2/3）** | v1.2.9 任务 `21ecf1b9-72ce-4a65-b279-459880edcb88`、`eac6910f-8387-478b-babf-935135d78d8a` 和最终 v1.3.0 任务 `8f5ca68c-a7f5-410d-8cfc-2791f33a389d` 均为 `success`；每次均完成格式化、Apply、BCDBoot、目标绑定、WinRE 清理和重启，P/Q fixture 清单比较无差异。 |
| 断电恢复 | `Stage`、`recover_windows` resume 分支、`resume_pending_boot_task`、`write_json_atomic` | **实机已验证（v1.3.0 ARM64，boot-requested 窗口）**；状态文件采用 `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)`，GUI 能识别唯一合法 `boot-requested` 任务并重新请求 WinRE；逐阶段断电组合仍不宣称全部覆盖 | 最终包任务 `3dd7a348-9190-4e61-90d0-76ecf1fd45cf` 先持久化 `boot-requested` 且无重启，随后 GUI 日志记录检测、重新请求 WinRE 和重启；任务最终 `success`，Q: 恢复、WinRE 清理和重启完成。测试方案 F-03 任务 `eba1f4f2-c1b7-4e86-babe-ed0ac0560c48`（`power-loss-window`）：prepare 在持久化 `boot-requested` 后未请求关机，VM 未重启；GUI 启动后识别唯一待恢复任务并重新请求 WinRE，`Recovery.log` 记录 `Recovery completed`、`WinRE cleanup completed; task marked successful`、`wpeutil.exe reboot`，最终 `status.json=success/progress=100`，WinRE 恢复 Enabled、无挂载镜像。 |
| v1.3.3 阶段断电续跑 | `power-loss-target-erased`、`power-loss-image-applied`、`power-loss-boot-repaired`、一次性 fault marker、目标重格式化序列号容忍 | **实机已验证（v1.3.3 ARM64）** | 三个 fault 在独立快照各自完成真实 WinRE 断电→续跑→success：target-erased `7590b3ac-a869-4518-8ced-65099ae9797f`、image-applied `93abc3a2-5051-467f-8ea0-e2697f8e7014`、boot-repaired `ce53efd6-9261-4148-8df6-f4be524c8adc`，均 success/100、fault marker 恰好 1 个、WinRE Enabled、无挂载 WIM、P↔Q 216 文件 216 大小一致。修复链：v1.3.1 盘符参与 payload 比较 → v1.3.2 清除临时盘符 + 放宽已格式化目标序列号 → v1.3.3 一次性 fault marker 防循环。 |
| BCD 失败回滚 | `bcd-before-raw`、`restore_bcd_snapshot` | **实机已验证（v1.2.5/v1.3.0 ARM64）** | 任务 `635e39d3-034a-4390-873b-b0ae68843863` 在 `BootRepaired` 后注入失败；E: 与原始快照 SHA-256 均为 `FC1E0A...479D`，WinRE 已恢复。测试方案 F-02 任务 `127efbbc-d6ee-44fa-ba73-60b573f35849`（`bcdboot-failure`）：DISM Apply 达 100% 后注入 BCDBoot 失败，`Recovery.log` 记录 `Previous byte-for-byte EFI BCD snapshot restored after boot repair failure`，故障前后 E: BCD SHA-256 均为 `716A4E88...`（53248 字节），WinRE 恢复 Enabled、无挂载镜像 |
| 开发 EFI 故障注入 | `--test-efi-drive`、`--test-fault` | **实机已验证（v1.3.0 ARM64）** | 最终包任务 `d557b7b7-7435-432a-a3f9-45b38ffd2588` 验证身份拒绝；历史任务 `635e39d3...` 验证 BCDBoot 回滚；普通 GUI 不可调用。测试方案 F-01 任务 `315be01f-5f15-4c33-9e7d-bf99428fd97f`（`identity-env-mismatch`）：WinRE 在 DISM/格式化/BCDBoot 前拒绝（`SOURCE volume serial differs between task.json and RecoveryTask.env`），`status.json=failed`，Y 卷未格式化（Windows/fixture 原样），WinRE 恢复 Enabled、无挂载镜像 |
| Rust Win32 GUI 单窗口、二次确认和多语言 | `crates/backuprestore-cli/src/native_gui.rs`、`BackupRestore.exe` | **实机已验证（GUI 范围）**：`v0.6.5` 客户区动态排版在每次标签切换后重排，详情框 112 高度、行距 8、状态框 64；逐页实际矩形确认探测/备份/单系统还原/第二系统的字段显隐、镜像/按钮不重叠，第二系统按钮位于客户区内；v1.1.0 启动会正常关闭旧 GUI 并保留唯一最新实例 | v1.1.0 ARM64 实测 6 个历史 GUI 已收敛为 1 个响应中的 `BackupRestore - Rust GUI v1.1.0` 窗口。发布验收还要求包目录、`build-manifest.json` 与 GUI 标题版本三者一致；不包括 UAC、任务创建、WinRE 或磁盘写入。 |
| 程序目录日志与 GUI 审计 | `append_log`、`native_gui.rs:append_gui_log`、`docs/development-execution-protocol.md` | **ARM64 启动及 WinRE 任务日志实机已验证（v1.2.5）** | v1.2.5 GUI 前台启动后在包目录写入 `logs\gui.log`；备份/还原任务分别写入 `prepare.log`、`recovery.log`，每行带 RFC 3339 时间戳；成功、身份拒绝、BCDBoot 回滚均有持久化证据。 |
| WIM 多索引读取与详细下拉显示 | `parse_wim_images`、`parse_dism_images`、`report_images_value` | **实机已验证（v1.3.0 ARM64）** | 最终包使用 P: 连续备份生成同一 WIM 的索引 1/2/3，DISM `/Get-WimInfo` 实读三项；GUI 仍保留索引序号、名称、描述、版本、架构、Edition、安装类型和大小字段。历史 GUI 双索引截图仍在 `.test-artifacts/root-captures/v1.0.9-open-image-c-drive.png`，本轮恢复任务分别实际使用 Index 1/2/3。 |
| GUI 管理员令牌与 WIM 读取 | `is_elevated`、`relaunch_elevated`、`ShellExecuteW("runas")` | **实机已验证（v0.7.4 ARM64）** | GUI 自提升后读取 WIM 成功，不再出现 DISM 740；最新 GUI 以高完整性用户进程运行 |
| 英文标签可见性 | `ui_text`、`operation_display` | **实机已验证（v0.7.3 ARM64）** | 英文模式四个顶部标签显示 `Inspect / Backup / Restore / Second system`，无裁剪；详细行为由说明框显示 |
| 英文 UI 纯净性 | `ui_text(Language::English, "language")` | **实机已验证（v0.7.3 ARM64）** | 英语模式显示纯 `Language`，不残留中文；中文模式仍显示 `语言` |
| 中文任务状态编码 | Rust Win32 文本控件与 `read_json` BOM 兼容 | **代码已覆盖；旧包实机已验证** | 当前 Rust GUI 不再经由 PowerShell 输出；旧 v0.7.4 证据仅证明历史中文显示结果 |
| 非 C 指定分区真实备份 | `BackupRestore.exe prepare -SourceDrive U -ImagePath T:\...`、Recovery 状态机 | **实机已验证（ARM64）** | 任务 `bc1660b8-6863-495d-b333-cd168f9a5c41` 最终 success；U: fixture 未格式化，WIM/metadata/hash 已记录 |
| 多索引 WIM 导出 | DISM Export-Image | **实机已验证（ARM64）** | T: `non-c-u-multi\Windows.wim` 显示索引 1 和 2；Index 2 已用于任务 `375f4422-7c17-4397-9560-6c83d7ca9ff4` 还原 |
| 隔离测试 EFI 选择 | `BackupRestore.exe prepare --test-efi-drive`、Rust EFI 校验、Recovery EFI=Z: | **实机已验证（ARM64）** | 独立 EFI E: GUID 写入任务 env，WinRE 使用 Z: 临时挂载，避免镜像卷 E: 冲突；参数仅用于开发测试，普通 GUI 不暴露。 |
| 工作目录所在卷与镜像卷重叠拒绝 | `native_gui.rs:create_task`、Rust `prepare`、core `Task::validate` | **实机已验证（v1.2.5 ARM64）** | Rust prepare 在任何 BCD/WinRE 写入前按 GUID 拒绝；GUI 保留同一早期检查，核心校验作为防御纵深 |
| 任务结果不虚报 | `last-task.json`、结果页文案、`status.json` | 实机已验证（Win11 ARM64 probe） | 自动 probe 任务 `c12026c0-6a9e-4093-8a8b-2971968a31f7` 的 `status.json` 为 `success` 仅出现在原始 WinRE hash 恢复校验之后；新 `-NoReboot` 任务 `760fcfe1-392d-4142-94af-b830f4286296` 保持 `prepared`，dry-run 不会伪造成功 |
| 网络/工具链下载规则 | `~/.codex/skills/pixian-dev-workflow/SKILL.md` | 流程已覆盖 | 每个大下载前保留网络检查和授权证据 |

## 当前强制实机顺序

1. ARM64 自动 probe 已实机通过。任何修改 WinRE 载荷、启动器、盘符策略或清理逻辑后，先重复 `probe -NoReboot`，再在新快照中重复自动 probe。
2. 下一项在独立快照中验证备份；先检查工作目录所在卷、镜像卷和剩余容量，不足时停止而不是创建不完整 WIM。
3. 最后按默认单系统还原、可选双系统还原顺序执行，并在每项前确认快照、目标分区身份和破坏性授权范围。
4. 每次实机测试结束后导出最小压缩证据：最终 `status.json`、`Recovery.log`、`prepare.log`、前后 WinRE/BCD hash 和关键截图；恢复快照后再开始下一项。

当前进度和用户明确的下载授权规则见 [project-status.md](project-status.md)。
