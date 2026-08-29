# BackupRestore 备份与还原测试方案

更新时间：2026-08-30
适用范围：Windows 10/11 UEFI/GPT 开发测试版系统备份还原工具（当前 `VERSION`：1.3.0）
配套文档：[project-status.md](project-status.md)、[verification-matrix.md](verification-matrix.md)、[implementation-notes.md](implementation-notes.md)、[development-execution-protocol.md](development-execution-protocol.md)

本方案是把“代码/离线证据”与“Windows/WinRE 实机证据”分开验收的操作手册。任何破坏性还原测试只允许在可回滚的虚拟机快照中执行；`C:` 不作为备份源或还原目标。判定口径沿用：**代码已覆盖 / 离线已验证 / 实机待验证 / 实机已验证**。

***

## 1. 目标与范围

### 1.1 目标

1. 验证四类任务模型：`probe`、`backup`、`restore-existing`、`create-secondary` 的正常链路与异常链路。
2. 证明破坏性边界（格式化、Apply、BCDBoot）只在受控条件下发生，且失败可回滚。
3. 证明断电、身份篡改、BCD 失败等故障注入下系统到达安全终态（`success` 或 `failed`），不污染生产启动链。
4. 区分“隔离 Apply/BCDBoot 成功”与“独立 EFI 实际引导成功”（后者为开发测试项）。

### 1.2 范围与明确排除

| 覆盖                                                   | 不覆盖（不在 V1）                 |
| ---------------------------------------------------- | -------------------------- |
| probe / backup / restore-existing / create-secondary | 自研 PE、分区布局重构               |
| 磁盘/分区身份、盘符变更、容量、BitLocker 拒绝                         | 网络/增量/差异镜像                 |
| 断电续跑、BCD 失败回滚、身份篡改拒绝                                 | Legacy BIOS、自动修改 BitLocker |
| 隔离多索引 WIM（Index 1/2/3）                               | 独立 EFI 固件首启动（仅开发测试）        |

***

## 2. 测试环境与工具

| 项目        | 说明                                                                                                                                             |
| --------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| 宿主        | MacBook（本机只跑离线/静态检查，不跑 WinRE）                                                                                                                  |
| 客体        | 可回滚的 Windows 11 ARM64 虚拟机（Parallels），V 系列快照基线                                                                                                  |
| fixture 卷 | P（源/单系统目标）、Q（第二系统目标）、H（WIM 镜像卷）、独立 E（开发 EFI）。C: 全程不触碰                                                                                          |
| 构建        | `windows/build-windows.ps1 -Architecture arm64 -CargoTargetDir C:\BackupRestoreBuild\target -OutputRoot C:\BackupRestoreBuild\package`         |
| 离线门槛      | `cargo fmt --all -- --check`、`cargo test --workspace --all-targets --offline`、`cargo clippy ... -D warnings`、PowerShell AST、`git diff --check` |
| 故障注入      | `--test-fault identity-env-mismatch \| bcdboot-failure \| power-loss-window`；`identity-env-mismatch`/`bcdboot-failure` 需同时给 `--test-efi-drive` |
| 无副作用      | `--no-reboot`（正常 Windows 内准备即停）、`validate-task`、`recover --dry-run`                                                                            |

> 测试目标目录必须在客体本地（如 `C:\BackupRestoreBuild\test-target-v<版本>`），不能放在 Parallels 共享源码目录，否则 Cargo 测试不可信。

***

## 3. 测试分层

三层递进，上一层通过不代表下一层：

1. **离线/静态层**（本机）：单元/规则测试、fmt、clippy、AST、diff。
2. **正常 Windows 准备层**（客体，`--no-reboot`）：prepare 校验、卷枚举、身份写入、BCD 快照、WinRE 注入、manifest/hash。
3. **WinRE 实机执行层**（客体，真实重启）：自动入口、挂载、DISM Capture/Apply、格式化、BCDBoot、清理、重启返回。

故障注入单独作为第 4 层，覆盖破坏性边界和异常终态。

***

## 4. 测试用例矩阵

每个用例给出：前置、步骤、通过判据、证据、状态。状态与 [verification-matrix.md](verification-matrix.md) 对齐；`实机已验证` 的用例回归只需在代码改动影响对应路径时重跑。

### 4.1 离线 / 规则层（始终作为构建前门槛）

| ID   | 用例                        | 判据                                                                 |
| ---- | ------------------------- | ------------------------------------------------------------------ |
| O-01 | 状态机合法/非法迁移                | 合法路径可转，非法迁移返回 `InvalidTransition`                                  |
| O-02 | backup 拒绝源=镜像卷            | `validate` 返回错                                                     |
| O-03 | 盘符只是临时提示，GUID 才是身份        | 改盘符后 `same_partition` 仍判同                                          |
| O-04 | workspace 卷拒绝保留分区与目标重叠    | 拒绝保留分区、拒绝与还原目标同分区                                                  |
| O-05 | EFI/MSR/Recovery 三类分区保护   | 三类分区不能做镜像卷/目标                                                      |
| O-06 | 绝对/相对路径边界                 | 阻止 `..`、控制字符、非盘符根、卷根结尾                                             |
| O-07 | 容量与 BitLocker 拒绝          | `ensure_capacity`/`ensure_bitlocker_accessible`                    |
| O-08 | 原子 JSON 覆盖（断电安全）          | `MoveFileExW(REPLACE_EXISTING\|WRITE_THROUGH)`，读回完整新 JSON、无 tmp 残留 |
| O-09 | 任务 ID 匹配/大小写归一            | 文件 ID ≠ 请求 ID 拒绝                                                   |
| O-10 | Windows PowerShell/BOM 兼容 | `read_json` 接受 BOM                                                 |
| O-11 | clean 保留最近终态、跳过未完成/挂载任务   | 报告字段正确                                                             |

### 4.2 正常 Windows 准备层（`--no-reboot`）

| ID   | 用例                     | 步骤 / 判据                                                                                                |
| ---- | ---------------------- | ------------------------------------------------------------------------------------------------------ |
| P-01 | 环境检查                   | `prepare.log` 记录 OS、GPT、WinRE、Secure Boot、BitLocker 状态                                                 |
| P-02 | 卷枚举与身份写入               | `list-volumes` 只返回普通卷；任务记录 disk/partition/volume GUID、大小、FS、序列号                                        |
| P-03 | BCD 快照                 | 写 BCD 前持久化 `previous_bcd_sha256`，且 task.json 与 manifest 一致                                             |
| P-04 | WinRE 注入与 payload hash | manifest + 各 payload SHA-256，RecoveryTask.env 二次 hash                                                  |
| P-05 | probe 准备               | `BackupRestore.exe prepare --operation probe [...] --no-reboot` 落 `prepared`，`recover --dry-run` 不伪造成功 |
| P-06 | 工作目录=镜像卷拒绝             | prepare/GUI 在任何 WinRE/BCD 写入前按 GUID 拒绝                                                                 |

### 4.3 WinRE 实机执行层（真实重启，VM 快照内）

| ID   | 用例             | 判据                                                                                         |
| ---- | -------------- | ------------------------------------------------------------------------------------------ |
| E-01 | 自动 probe       | 任务 `c12026c0-...` 类：Windows→WinRE→原始 WinRE 恢复校验→Windows，`status.json=success`，无 `.cmd` 启动器 |
| E-02 | 真实备份（非 C）      | P→H，DISM Capture，metadata+SHA-256 一致，原始 WinRE hash 恢复，`success`                            |
| E-03 | 多索引 WIM        | P 连续备份生成同一 WIM 的 Index 1/2/3，DISM 实读确认                                                     |
| E-04 | 单系统还原          | Index→P：DiskPart 格式化→Apply→BCDBoot `/v`→WinRE 清理→重启；P fixture 相对路径+SHA-256 前后无差异           |
| E-05 | 双系统还原（第二系统）    | Index→Q：格式化→Apply→`/addlast`→目标绑定→WinRE 清理→重启；Q fixture 无差异                                |
| E-06 | 盘符变更后仍解析到同一卷   | WinRE 换盘符挂载后仍按 GUID 命中同源/镜像/目标                                                             |
| E-07 | 还原后环境          | `reagentc /info` 为 Enabled，`dism /Get-MountedWimInfo` 无挂载镜像                                |
| E-08 | WIM 多索引 GUI 读取 | GUI 下拉显示 Index 序号/名称/描述/大小，实际用于 Index 1/2/3 还原                                             |

### 4.4 故障注入 / 异常路径层（开发 CLI）

| ID   | 用例              | 故障                                   | 判据                                                                                 |
| ---- | --------------- | ------------------------------------ | ---------------------------------------------------------------------------------- |
| F-01 | 身份篡改拒绝          | `--test-fault identity-env-mismatch` | WinRE 在任何 DISM/格式化/BCDBoot 前拒绝；未格式化、`status.json=failed`；恢复后 WinRE 仍 Enabled、无挂载镜像 |
| F-02 | BCD 失败回滚        | `--test-fault bcdboot-failure`       | 在 `BootRepaired` 后注入失败；EFI BCD 与原始快照字节级一致，WinRE 已恢复                                |
| F-03 | 断电窗口续跑          | `--test-fault power-loss-window`     | 在 `boot-requested` 持久化后不重启；再次启动 GUI 识别唯一待恢复任务、重新请求 WinRE、任务最终 `success`            |
| F-04 | 独立 EFI 引导（开发测试） | `--test-efi-drive E`                 | 已复测为 `0xc0430001`；不作为产品功能；逐阶段断电组合不宣称全部覆盖                                           |

***

## 5. 快照 / 数据管理规范

1. 任何破坏性执行前建立独立快照（如 `before-<version>-<scenario>`），每项结束恢复快照再开始下一项。
2. 使用 P/Q 固定 fixture 清单比较工具：以**相对路径 + 大小 + SHA-256** 比较；将 Parallels 自动项（`Mac disk`、`System Volume Information`、离解后的 `BCD-Template`/`.LOG`、回收站 `desktop.ini`）单列为系统后处理项并显式排除，避免隐藏差异。
3. 每次实机测试保留最小压缩证据：最终 `status.json`、`Recovery.log`、`prepare.log`、前后 WinRE/BCD hash、关键截图（放 `.test-artifacts/root-captures/`）。
4. 完成任务后清理：保留最近 3 个终态任务元数据与日志；未完成/异常/仍挂载任务不自动删除；卸载 DISM 孤儿挂载。

***

## 6. 修改后的强制回归顺序

任何改动影响 WinRE 载荷、启动器、盘符策略或清理逻辑时，按序执行：

1. 本机离线门槛（O-01..O-11）全绿；
2. 客体 `probe --no-reboot`（P-05）→ 真实自动 probe（E-01）；
3. 新快照中真实备份（E-02/E-03）；
4. 默认单系统还原（E-04）、可选双系统还原（E-05）；每项先确认快照、目标分区身份与破坏性授权范围；
5. 故障注入（F-01..F-03）在新快照中依次执行并恢复。

***

## 7. 通过/完成判据

* 所有离线用例（O-*）与对应层 P-*/E-*/F-* 用例有明确证据（persist 文件或可复核快照）。

* 破坏性用例都必须落 `success`（正常链路）或 `failed`（异常注入）并恢复原始 WinRE/BCD。

* 不把 AST/单元测试/进程存活作为 WinRE、DISM、格式化、BCDBoot 或真实重启成功的替代证据。

* 启动顺序依赖固件首启动等证据缺失的场景，在 verification-matrix 中保持 `实机待验证` 或 `不属 V1`，不外推。

***

## 8. 执行记录（2026-08-30，Windows 11 ARM64 快照内）

以下为按本方案在可回滚快照中执行并落盘证据的真实结果；F 层从 `F-baseline-afterE5` 快照依次执行并恢复。

### 8.1 正常链路（E 层）

| ID   | 用例            | 结果 | 证据 |
| ---- | ------------- | -- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| E-01 | 自动 probe       | 通过 | 任务 `c12026c0-6a9e-4093-8a8b-2971968a31f7`：Windows→WinRE→原始 WinRE hash 恢复校验→Windows，`status.json=success`，无 `.cmd` 启动器 |
| E-02 | 真实备份（非 C）     | 通过 | P→H DISM Capture，metadata+SHA-256 一致；多轮备份任务 `ff6b645b-...` 等为 `success`，原始 WinRE hash 恢复 |
| E-03 | 多索引 WIM       | 通过 | P 连续备份生成同一 WIM 的 Index 1/2/3，DISM `/Get-WimInfo` 实读确认，GUI 下拉显示序号/名称/描述/大小 |
| E-04 | 单系统还原         | 通过 | 任务 `312a7cc0-...`（Index 2）与最终包 Index 3 恢复链完成：格式化→Apply→BCDBoot `/v`→WinRE 清理→重启；P/Q fixture 相对路径+SHA-256 无差异 |
| E-05 | 双系统还原（第二系统）   | 通过 | 任务 `8f5ca68c-...`（v1.3.0）：`create-secondary` Y 卷格式化→Apply→`/addlast`→`WindowsBackupSecondary` 菜单名→WinRE 清理→重启，`success` |
| E-07 | 还原后环境         | 通过 | 各破坏性任务后 `reagentc /info` 均为 Enabled、`dism /Get-MountedWimInfo` 无挂载镜像 |

### 8.2 故障注入（F 层）

| ID   | 用例        | 结果 | 证据（任务 ID / status / 关键日志）                                                                                                     |
| ---- | --------- | -- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| F-01 | 身份篡改拒绝    | 通过 | 任务 `315be01f-5f15-4c33-9e7d-bf99428fd97f`：WinRE 在任何 DISM/格式化/BCDBoot 前拒绝，错误 `SOURCE volume serial differs between task.json and RecoveryTask.env`；`status.json=failed`、未格式化（Y 卷 Windows/fixture 原样）、WinRE 恢复 Enabled、无挂载镜像 |
| F-02 | BCD 失败回滚  | 通过 | 任务 `127efbbc-d6ee-44fa-ba73-60b573f35849`：DISM Apply 100% 后注入 BCDBoot 失败；`Recovery.log` 记录 `Previous byte-for-byte EFI BCD snapshot restored after boot repair failure`；故障前后 E: BCD SHA-256 均为 `716A4E88...`（53248 字节）、WinRE 恢复 Enabled、无挂载镜像 |
| F-03 | 断电窗口续跑    | 通过 | 任务 `eba1f4f2-c1b7-4e86-babe-ed0ac0560c48`：prepare 持久化 `boot-requested` 后未请求关机（VM 未重启）；GUI 启动识别唯一待恢复任务并重新请求 WinRE；`Recovery.log` 记录 `Recovery completed`、`WinRE cleanup completed; task marked successful`、`wpeutil.exe reboot`；最终 `success/progress=100`、WinRE 恢复 Enabled、无挂载镜像 |
| F-04 | 独立 EFI 引导（开发测试） | 已知失败 | 已复测为 `0xc0430001`；不作为产品功能，逐阶段断电组合不宣称全部覆盖 |

### 8.3 快照基线

`F-baseline-afterE5`（`{fa61a33e-e2ab-4113-954e-a76ad83b351a}`）：E 层全部完成后、F 层执行前的独立回滚基线。F-01/F-02/F-03 全部在破坏性执行前确认目标分区身份与授权范围，结束后可随时恢复该基线。

