# 错误码清单（`BR-<域>-<序号>`）

> **自动生成，别手改**：`python3 tools/gen-error-codes.py`（真相源是 `crates/backuprestore-cli/src/error_code.rs` 的 `error_codes!` 宏体）。
> **码是契约**——发布后不得重排、不复用退役号；用户报障直接报码即可定位到唯一一行。

## 怎么用

| 场景 | 怎么做 |
|---|---|
| 用户报障 | 让 TA 报 `BR-xxx-nnn`（或截 `error:` 那行的方括号前缀） |
| 翻日志 | `grep -a "\[BR-" Recovery.log` |
| 自动化判据 | 判 `--json` 的 `error_code` 字段，**不要**解析 `error` 文案 |
| 看中文/英文原文 | 查下表，或直接 grep 码 |

## 结构性码（`TaskError` 的结构变体，`Display` 不加前缀）

由 `TaskError::code()` 直接返回，只在 `--json` 的 `error_code` 字段里出现——
`Display` 不加 `[码] ` 前缀（文案是 OS 本地化串或 serde 原文，加前缀只会更难看）。

| 码 | 变体 | 含义 |
|---|---|---|
| `BR-IO-001` | `Io(_)` | 文件/系统调用失败（文案随系统语言，如「系统找不到指定的路径」） |
| `BR-IO-002` | `Json(_)` | JSON 读写/解析失败 |
| `BR-TSK-028` | `InvalidTransition{..}` | 状态机非法迁移 |

自动化用它区分**环境失败**（`BR-IO-*`）与**脚本写错**（`error_code` 为 `null`）。

## core 层（`BR-CORE-NNN`）

core 的码由 `TaskError::coded("BR-CORE-NNN", …)` 直接写在构造点，
文案是英文技术诊断（core 没有语言层）。**77 个语义码**，直接 grep 码定位：

```
grep -rn "BR-CORE-" crates/backuprestore-core/src/
```

## CLI 层（共 216 条）

### `BR-APL` — 应用镜像（DISM / wimlib）（10 条）

| 码 | 变体 | 中文 | English |
|---|---|---|---|
| `BR-APL-001` | `DismApplyFailedOnline` | DISM failed: {0}\n--- 输出尾部（至多 {1} 字节）---\n{2} | DISM failed: {0}\n--- output tail (at most {1} bytes) ---\n{2} |
| `BR-APL-002` | `WimlibBinaryMissing` | 覆盖式应用需要 wimlib 二进制：未找到 {0}（{1} 须与其同目录）。请把 wimlib 官方 Windows ARM64 预编译包的两个文件放进程序目录后重试；不会回退 DISM（该组合下 DISM 必然失败）。 | Overlay apply needs the wimlib binary: {0} was not found ({1} must sit in the same directory). Put both files from the official wimlib Windows ARM64 prebuilt package into the program directory and retry; DISM is not used as a fallback (it always fails in this combination). |
| `BR-APL-003` | `WimlibDllMissing` | 覆盖式应用需要 wimlib 依赖库：{0} 已就位，但同目录缺少 {1}。wimlib 启动时加载不到该库会被系统直接终止（报错不含这两个文件名，也无法归因到部署缺口）；请把 wimlib 官方 Windows ARM64 预编译包的两个文件一起放进程序目录后重试；不会回退 DISM（该组合下 DISM 必然失败）。 | Overlay apply needs the wimlib library: {0} is present but {1} is missing from the same directory. If the loader cannot find it, the OS terminates wimlib immediately (the error names neither file, so a deployment gap cannot be inferred from it). Put both files from the official wimlib Windows ARM64 prebuilt package into the program directory and retry; DISM is not used as a fallback (it always fails in this combination). |
| `BR-APL-004` | `WimlibApplyFailed` | wimlib apply failed: {0}\n--- 输出尾部（至多 8192 字节）---\n{1} | wimlib apply failed: {0}\n--- output tail (at most 8192 bytes) ---\n{1} |
| `BR-APL-005` | `ProgramDirUnresolvableFallback` | 无法定位程序目录（wimlib 降级通道）：{0} | Cannot locate the program directory (wimlib fallback path): {0} |
| `BR-APL-006` | `ProgramDirResolveFailedFallback` | 程序目录解析失败（wimlib 降级通道） | Resolving the program directory failed (wimlib fallback path) |
| `BR-APL-007` | `DismApplyFailedWithReason` | DISM apply 失败：{0}\n{1}\n{2} | DISM apply failed: {0}\n{1}\n{2} |
| `BR-APL-008` | `FallbackAlsoFailed` | DISM apply 失败：{0}\n自动降级 wimlib 重试仍失败：{1}\n{2}\n{3} | DISM apply failed: {0}\nThe automatic wimlib fallback also failed: {1}\n{2}\n{3} |
| `BR-APL-009` | `ProgramFailedNamed` | {0} failed with {1}\n--- 输出尾部（至多 {2} 字节）---\n{3} | {0} failed with {1}\n--- output tail (at most {2} bytes) ---\n{3} |
| `BR-APL-010` | `ProgramExitNonZero` | {0} 失败：{1} | {0} failed: {1} |

### `BR-BOOT` — 引导、BCD、启动菜单（31 条）

| 码 | 变体 | 中文 | English |
|---|---|---|---|
| `BR-BOOT-001` | `BootManagerUnknownContinue` | 引导管理器状态未知，拒绝继续 | Boot manager state is unknown; refusing to continue |
| `BR-BOOT-002` | `PostBootLoaderMissing` | bcdboot 后找不到第二系统的启动加载器 | No boot loader for the secondary system was found after bcdboot |
| `BR-BOOT-003` | `PostBootManagerUnparsable` | 无法解析还原后的引导管理器状态，拒绝报成功 | Cannot parse the boot manager state after restore; refusing to report success |
| `BR-BOOT-004` | `DefaultNotRestoredTarget` | 默认启动项未指向本次还原的系统，拒绝报成功 | The default boot entry does not point to the system restored this run; refusing to report success |
| `BR-BOOT-005` | `MenuEntryLost` | 原有启动菜单项丢失，拒绝报成功 | A pre-existing boot menu entry was lost; refusing to report success |
| `BR-BOOT-006` | `SecondaryMenuEntryMissing` | 第二系统启动项未进入菜单，拒绝报成功 | The secondary-system boot entry did not make it into the menu; refusing to report success |
| `BR-BOOT-007` | `ResumeObjectNotRestored` | bootmgr resume_object 未恢复为主系统，拒绝报成功 | bootmgr resume_object was not restored to the primary system; refusing to report success |
| `BR-BOOT-008` | `RecoveryObjectDestroyed` | 原有系统恢复对象被破坏，拒绝报成功 | The original system recovery object was damaged; refusing to report success |
| `BR-BOOT-009` | `NoBcdEfiPartition` | 未发现具有可读取 BCD 的 GPT EFI 分区 | No GPT EFI partition with a readable BCD was found |
| `BR-BOOT-010` | `MultipleEspAmbiguous` | 检测到多个含 BCD 的 EFI 系统分区（{0}），且无法唯一确定本次引导使用的是哪一个（BCD 指纹比对无唯一匹配）。请避免多 ESP 布局，或仅保留引导磁盘上的 ESP 后重试 | Multiple EFI system partitions with a BCD were found ({0}), and it cannot be determined which one this boot used (no unique BCD fingerprint match). Avoid multi-ESP layouts, or keep only the ESP on the boot disk and retry |
| `BR-BOOT-011` | `MenuTargetMustAppearOnce` | 本次还原系统必须且只能出现一次，不能取消 | The system restored this run must appear exactly once and cannot be deselected |
| `BR-BOOT-012` | `MenuTimeoutOutOfRange` | 启动菜单等待时间超出范围 | The boot menu timeout is out of range |
| `BR-BOOT-013` | `DefaultCannotBeRemoved` | 当前默认启动的系统不能从启动菜单中移除 | The system currently booted by default cannot be removed from the boot menu |
| `BR-BOOT-014` | `MenuDuplicateName` | 启动菜单出现同名项，请改名以便区分 | The boot menu contains duplicate names; rename them so they can be told apart |
| `BR-BOOT-015` | `PlanNameOrVolumeInvalid` | 引导方案名称或卷身份无效 | The boot plan name or volume identity is invalid |
| `BR-BOOT-016` | `PlanDuplicatePartition` | 引导方案重复系统分区 | The boot plan contains a duplicate system partition |
| `BR-BOOT-017` | `PlanNotForTask` | 引导方案不属于当前还原任务 | The boot plan does not belong to the current restore task |
| `BR-BOOT-018` | `PreRepairBootMgrInvalid` | 引导修复前启动管理器无效 | The boot manager is invalid before boot repair |
| `BR-BOOT-019` | `SelectedSystemGone` | 引导修复时所选系统已不存在 | A selected system no longer exists during boot repair |
| `BR-BOOT-020` | `PostRepairLoaderMissing` | 引导修复后找不到系统加载器 | No system loader was found after boot repair |
| `BR-BOOT-021` | `RepairResultUnreadable` | 无法回读引导修复结果 | Cannot read back the boot repair result |
| `BR-BOOT-022` | `RepairOrderMismatch` | 引导修复顺序、默认项、休眠引用或显示时间回读不一致 | The boot repair read-back disagrees on order, default entry, hibernation reference or display timeout |
| `BR-BOOT-023` | `RepairMissingSelectedSystem` | 回读缺少所选系统 | The read-back is missing a selected system |
| `BR-BOOT-024` | `MenuNameMismatch` | 引导菜单名称回读不一致 | The boot menu names do not read back as written |
| `BR-BOOT-025` | `WindowsVolumeMissingPlan` | Windows 系统卷缺少引导修复方案 | A Windows system volume is missing its boot repair plan |
| `BR-BOOT-026` | `BcdbootLoaderMissing` | bcdboot 后找不到本次还原系统的启动加载器 | No boot loader for the system restored this run was found after bcdboot |
| `BR-BOOT-027` | `PostBcdbootBootMgrUnparsable` | BCDBoot 后启动管理器状态无法解析 | Cannot parse the boot manager state after BCDBoot |
| `BR-BOOT-028` | `DefaultSwitchFailed` | 无法把默认启动项指向本次还原的系统，拒绝继续后续破坏性步骤 | Cannot point the default boot entry to the system restored this run; refusing to continue with further destructive steps |
| `BR-BOOT-029` | `LoaderMissingRefuseMenu` | 主系统或新第二系统加载器缺失，拒绝改写启动菜单 | The primary or new secondary system loader is missing; refusing to rewrite the boot menu |
| `BR-BOOT-030` | `BootMenuTargetLoaderMissing` | 缺少必选目标加载器 | Missing required target loader |
| `BR-BOOT-031` | `BootMenuFreezeBlocked` | 引导方案在锁定前已记录错误，拒绝继续：{0} | The boot plan recorded an error before freezing; refusing to continue: {0} |

### `BR-GUI` — 图形界面（2 条）

| 码 | 变体 | 中文 | English |
|---|---|---|---|
| `BR-GUI-001` | `ImageIndexInvalid` | 镜像索引无效 | The image index is invalid |
| `BR-GUI-002` | `ImageIndexNotPositive` | 镜像索引必须是正整数 | The image index must be a positive integer |

### `BR-IMG` — 镜像文件与 DISM 探测（8 条）

| 码 | 变体 | 中文 | English |
|---|---|---|---|
| `BR-IMG-001` | `ImageNotCompleteWindows` | 镜像不是完整 Windows（缺少 reagentc.exe 或 SYSTEM 注册表），还原/新增第二系统需要完整 Windows 镜像。要安装 PE，请改用「安装 PE」功能（内存版或硬盘版），它才是为 PE 设计的路径。本次未格式化任何卷、未写入引导。 | The image is not a complete Windows image (missing reagentc.exe or the SYSTEM registry). Restoring or adding a secondary system needs a complete Windows image. To install PE, use the \"Install PE\" feature (RAM-disk or disk-based); that is the path designed for PE. No volume was formatted and no boot entry was written this run. |
| `BR-IMG-002` | `ImageIsWindowsPe` | 镜像\"{0}\"是 Windows PE，不是完整 Windows；要安装 PE 请改用「安装 PE」功能。本次未格式化任何卷、未写入引导。 | Image \"{0}\" is Windows PE, not a complete Windows image. To install PE, use the \"Install PE\" feature. No volume was formatted and no boot entry was written this run. |
| `BR-IMG-003` | `DismSpawnFailed` | dism 调用失败（无法启动/读取输出）：{0} | Failed to invoke dism (cannot start it or read its output): {0} |
| `BR-IMG-004` | `ImageMountProbeFailed` | 镜像挂载探测失败（dism /Mount-Wim 退出码 {0}{1}）。完整输出已写入准备日志。 | Image mount probe failed (dism /Mount-Wim exit code {0}{1}). The full output was written to the preparation log. |
| `BR-IMG-005` | `ImageUnmountFailed` | 镜像挂载探测的卸载失败（dism /Unmount-Wim 退出码 {0}{1}）。镜像可能仍处于挂载状态并被锁定，请手动用 dism /Unmount-Wim /Discard 清理后再试。 | Unmounting the probe mount failed (dism /Unmount-Wim exit code {0}{1}). The image may still be mounted and locked; clean it up manually with dism /Unmount-Wim /Discard and retry. |
| `BR-IMG-006` | `ImageNameReadFailed` | 读取镜像名失败（dism /Get-WimInfo 退出码 {0}{1}）。请确认镜像文件可访问且未被占用。 | Failed to read the image name (dism /Get-WimInfo exit code {0}{1}). Make sure the image file is accessible and not in use. |
| `BR-IMG-007` | `DeleteIndexReadbackMismatch` | 删除旧索引后回读的索引集合与期望不一致，已中止；镜像可能停在半删状态，请用 wim-info 核实后再试 | The index set read back after deleting old indexes does not match the expectation; aborted. The image may be left half-deleted - verify with wim-info before retrying |
| `BR-IMG-008` | `BackupSpaceInsufficient` | 镜像所在卷可用空间不足以开始备份：可用 {0}，至少需要 {1}（源卷已用 {2}）。请换一个更大的卷或先清理镜像卷；本次未写入任何镜像数据。 | Not enough free space on the volume holding the image to start a backup: {0} available, at least {1} required (source volume uses {2}). Use a larger volume or free space on the image volume first; no image data was written this run. |

### `BR-PE` — PE 通道与写盘契约（58 条）

| 码 | 变体 | 中文 | English |
|---|---|---|---|
| `BR-PE-001` | `ImgPathComponent` | 镜像相对路径包含无效组件 | Image relative path contains an invalid component |
| `BR-PE-002` | `LoaderNoObjectGuid` | 目标加载器缺少对象 GUID | Target loader has no object GUID |
| `BR-PE-003` | `LoaderNotUnique` | 目标引导加载器不唯一或设备字段不完整 | Target boot loader is not unique or its device field is incomplete |
| `BR-PE-004` | `VolumeChanged` | 卷身份、现场编号或几何已变化，拒绝写入 | Volume identity, live number or geometry changed; refusing to write |
| `BR-PE-005` | `PrepareIdentityIncomplete` | 准备期卷身份不完整，拒绝重新绑定 | Prepared volume identity is incomplete; refusing to rebind |
| `BR-PE-006` | `BootVolumeChanged` | 跨启动卷身份或文件系统变化，拒绝重新绑定 | Boot volume identity or file system changed; refusing to rebind |
| `BR-PE-007` | `TargetNotGptNtfs` | 目标必须是身份完整的 GPT/NTFS 基本数据分区 | Target must be an identifiable GPT/NTFS basic data partition |
| `BR-PE-008` | `TargetRoleConflict` | 目标与镜像、工作区、运行程序或恢复载荷冲突，或必需角色未知 | Target conflicts with the image, workspace, running program or recovery payload, or its required role is unknown |
| `BR-PE-009` | `SecondaryNoPrimary` | 第二系统任务缺少已确认的主系统身份 | Secondary-system task lacks a confirmed primary-system identity |
| `BR-PE-010` | `SecondaryCannotOverwrite` | 第二系统不能覆盖主系统，确认不能豁免角色禁区 | A secondary system must not overwrite the primary; confirmation cannot waive the role exclusion |
| `BR-PE-011` | `ProbeRootUnusable` | 探测卷根不可用或存在重解析 | Probe volume root is unusable or carries a reparse point |
| `BR-PE-012` | `ProbeNoReparse` | 安全探测不跟随重解析点 | Safe probing does not follow reparse points |
| `BR-PE-013` | `ProbeNoSymlink` | 安全探测不跟随链接 | Safe probing does not follow links |
| `BR-PE-014` | `NotRegularFile` | 必要文件路径不是普通文件 | A required file path is not a regular file |
| `BR-PE-015` | `AncestorNotDirectory` | 探测祖先路径不是目录 | A probed ancestor path is not a directory |
| `BR-PE-016` | `ParentMissing` | 探测父路径缺失 | Probed parent path is missing |
| `BR-PE-017` | `ProbePathEmpty` | 探测路径为空 | Probed path is empty |
| `BR-PE-018` | `StateWriteFailed` | {0}；失败状态落盘也失败：{1} | {0}; writing the failure state also failed: {1} |
| `BR-PE-019` | `InjectFailed` | 注入失败 | Injection failed |
| `BR-PE-020` | `PendingRequestExists` | 已有待执行 PE 请求，请先处理原任务 | A pending PE request already exists; finish or discard it first |
| `BR-PE-021` | `TaskWriteReadbackMismatch` | 任务写后回读不一致 | Task read-back after writing does not match |
| `BR-PE-022` | `PendingRequestConflict` | 多个待执行请求冲突，拒绝猜测 | Multiple pending requests conflict; refusing to guess |
| `BR-PE-023` | `ImageMustBeLocalAbsolute` | 镜像必须位于本地卷的绝对路径 | The image must be an absolute path on a local volume |
| `BR-PE-024` | `ImageIndexMustBePositive` | 镜像索引必须大于零 | The image index must be greater than zero |
| `BR-PE-025` | `UnknownPeOperation` | 未知 PE 操作 | Unknown PE operation |
| `BR-PE-026` | `TaskIdGenerationFailed` | 无法生成任务编号 | Cannot generate a task id |
| `BR-PE-027` | `DriveLetterInvalid` | 盘符无效 | The drive letter is invalid |
| `BR-PE-028` | `VolumeNoStableGuid` | 卷缺少稳定 GUID 路径 | The volume has no stable GUID path |
| `BR-PE-029` | `PeWriterBusy` | 已有 PE 写任务运行，拒绝并发执行 | A PE write task is already running; refusing concurrent execution |
| `BR-PE-030` | `UnsupportedPeTaskVersion` | 不支持的 PE 任务版本或动作 | Unsupported PE task version or action |
| `BR-PE-031` | `TaskIdHasPath` | 任务编号不能含路径 | The task id must not contain a path |
| `BR-PE-032` | `ImageNoParentDir` | 镜像没有父目录 | The image path has no parent directory |
| `BR-PE-033` | `ImageFileMissing` | 镜像文件不存在，未执行任何格式化 | The image file does not exist; no formatting was performed |
| `BR-PE-034` | `BackupDirReparse` | 备份目录不能经过重解析点 | The backup directory must not traverse a reparse point |
| `BR-PE-035` | `Utf16OutputTruncated` | UTF-16 输出被截断 | UTF-16 output was truncated |
| `BR-PE-036` | `Utf16DecodeFailed` | UTF-16 输出解码失败 | Failed to decode UTF-16 output |
| `BR-PE-037` | `ConsoleDecodeFailed` | 控制台输出解码失败 | Failed to decode console output |
| `BR-PE-038` | `SystemRootUnknown` | SystemRoot 未知 | SystemRoot is unknown |
| `BR-PE-039` | `ProgramFailed` | {0} 失败：{1}\n{2} | {0} failed: {1}\n{2} |
| `BR-PE-040` | `RunningVolumeUnknown` | 运行卷未知 | The running volume is unknown |
| `BR-PE-041` | `CannotWriteRunningVolume` | 不能写入当前运行卷 | Refusing to write to the currently running volume |
| `BR-PE-042` | `PayloadSourceUnknown` | 当前恢复载荷来源未知，拒绝格式化 | The recovery payload source is unknown; refusing to format |
| `BR-PE-043` | `PayloadVolumeUnresolvable` | 恢复载荷卷无法解析 | Cannot resolve the recovery payload volume |
| `BR-PE-044` | `PrimaryVolumeLacksEvidence` | 需保留的主系统卷缺少系统证据 | The primary volume that must be preserved lacks system evidence |
| `BR-PE-045` | `ImageExpandedSizeUnknown` | 镜像展开容量未知，拒绝写入 | The expanded image size is unknown; refusing to write |
| `BR-PE-046` | `ImageSizeOverflow` | 镜像容量溢出 | Image size overflow |
| `BR-PE-047` | `SidecarMismatch` | 副档索引或摘要不匹配 | Sidecar index or digest does not match |
| `BR-PE-048` | `TargetCapacityInsufficient` | 目标容量不足：{0} < {1} | Target capacity is insufficient: {0} < {1} |
| `BR-PE-049` | `DataOverlaySpaceInsufficient` | 数据覆盖目标可用空间不足 | Not enough free space for the data overlay |
| `BR-PE-050` | `ImageMissingSystemFile` | 镜像缺少系统文件 {0}；不能按系统还原成功 | The image lacks the system file {0}; it cannot count as a successful system restore |
| `BR-PE-051` | `BootManagerStateUnknown` | 启动管理器状态未知，拒绝格式化 | Boot manager state is unknown; refusing to format |
| `BR-PE-052` | `ProgramDirUnresolvable` | 无法定位程序目录（wimlib 通道）：{0} | Cannot locate the program directory (wimlib path): {0} |
| `BR-PE-053` | `ProgramDirResolveFailed` | 程序目录解析失败（wimlib 通道） | Resolving the program directory failed (wimlib path) |
| `BR-PE-054` | `SystemFileVerifyFailed` | 系统文件验证失败，不能降级为数据还原成功 | System-file verification failed; this cannot be reported as a successful data restore |
| `BR-PE-055` | `BootPermitMissingEsp` | 引导许可缺少 ESP | The boot permit is missing an ESP |
| `BR-PE-056` | `MissingPreFormatBootState` | 缺少格式化前引导状态 | The pre-format boot state is missing |
| `BR-PE-057` | `BootStateChangedAfterPrecheck` | 预检后引导状态发生变化，停止引导写入 | Boot state changed after preflight; stopping boot writes |
| `BR-PE-058` | `SecondaryMenuOrderInvalid` | 第二系统默认项或菜单顺序不符合约定，拒绝报成功 | The secondary-system default entry or menu order violates the contract; refusing to report success |

### `BR-RE` — 恢复环境注册 / WinRE 关联（47 条）

| 码 | 变体 | 中文 | English |
|---|---|---|---|
| `BR-RE-001` | `WinreStateAmbiguous` | 无法唯一确认 WinRE 启用状态 | Cannot uniquely determine the WinRE enabled state |
| `BR-RE-002` | `WinreLocationNotUnique` | WinRE 注册位置不唯一 | The WinRE registration location is not unique |
| `BR-RE-003` | `WinreIdentifierAmbiguous` | WinRE 注册标识符缺失或不唯一 | The WinRE registration identifier is missing or not unique |
| `BR-RE-004` | `BcdFieldDuplicate` | BCD 字段缺失或重复：{0} | BCD field is missing or duplicated: {0} |
| `BR-RE-005` | `BcdObjectDuplicate` | BCD 对象缺失或重复 | BCD object is missing or duplicated |
| `BR-RE-006` | `WinreNotEnabledOrWrongLocation` | WinRE 未启用或注册位置与目标分区不符 | WinRE is not enabled, or its registration location does not match the target partition |
| `BR-RE-007` | `WinreLoaderChanged` | WinRE 关联的目标系统加载器已变化 | The system loader associated with WinRE has changed |
| `BR-RE-008` | `RecoveryGuidNotUnique` | 系统恢复关联不是唯一 GUID | The system recovery association is not a unique GUID |
| `BR-RE-009` | `RecoveryEntryNotEnabled` | 系统恢复启动未启用 | The system recovery boot entry is not enabled |
| `BR-RE-010` | `RecoveryBcdMismatch` | BCD 恢复关联与 WinRE 注册标识符不一致 | The BCD recovery association disagrees with the WinRE registration identifier |
| `BR-RE-011` | `RecoveryDeviceNotRamdisk` | 恢复加载器设备不是内存盘 | The recovery loader device is not a RAM disk |
| `BR-RE-012` | `RecoveryDeviceNoPartitionId` | 恢复加载器设备缺少分区标识 | The recovery loader device has no partition identifier |
| `BR-RE-013` | `RecoveryDeviceNoOptionObject` | 恢复加载器设备缺少选项对象 | The recovery loader device has no options object |
| `BR-RE-014` | `PartitionTokenUnrecognized` | {0}的分区标识无法识别：{1} | Unrecognized partition identifier for {0}: {1} |
| `BR-RE-015` | `PartitionLetterMismatch` | {0}指向的 {1}: 不是登记位置所在的 disk{2}/partition{3} | {0} points to drive {1}, which is not disk{2}/partition{3} of the registered location |
| `BR-RE-016` | `PartitionVerifyWindowsOnly` | 恢复对象分区核对仅在 Windows 上可用 | Recovery-object partition verification is only available on Windows |
| `BR-RE-017` | `PartitionDeviceMismatch` | {0}指向的 {1} 是 disk{2}/partition{3}，不是登记位置所在的 disk{4}/partition{5} | {0} points to {1}, which is disk{2}/partition{3}, not disk{4}/partition{5} of the registered location |
| `BR-RE-018` | `RecoveryLoaderNotOriginalWim` | 恢复加载器未指向原始 Winre.wim | The recovery loader does not point to the original Winre.wim |
| `BR-RE-019` | `RecoveryLoaderOptionInvalid` | 恢复加载器选项对象标识符无效 | The recovery loader options object identifier is invalid |
| `BR-RE-020` | `RecoveryLoaderIncomplete` | 恢复加载器设备或执行模式不完整 | The recovery loader device or execution mode is incomplete |
| `BR-RE-021` | `RamdiskDeviceFormatInvalid` | 恢复内存盘启动设备格式无效 | The recovery RAM-disk boot device format is invalid |
| `BR-RE-022` | `RamdiskFileNotInTarget` | 恢复内存盘启动文件不在目标注册目录 | The recovery RAM-disk boot file is not in the target registration directory |
| `BR-RE-023` | `SecondaryCloneStillEnabled` | 第二系统克隆配置重置后仍启用，拒绝修改原恢复对象 | The secondary-system clone configuration is still enabled after reset; refusing to modify the original recovery object |
| `BR-RE-024` | `WinreLocationMissing` | 已启用 WinRE 缺少注册位置 | WinRE is enabled but its registration location is missing |
| `BR-RE-025` | `WinreLocationUnparsable` | 已启用 WinRE 的注册位置无法解析：{0} | Cannot parse the registration location of the enabled WinRE: {0} |
| `BR-RE-026` | `WinrePartitionMissingFiles` | 已启用 WinRE 位于独立恢复分区（{0}），但该分区缺少可用的 {1}，拒绝重接 | The enabled WinRE lives on an independent recovery partition ({0}), but that partition lacks a usable {1}; refusing to rewire |
| `BR-RE-027` | `OfflineWinreReOnly` | 离线 WinRE 注册只允许在恢复环境（WinRE/WinPE）中执行 | Offline WinRE registration may only run in the recovery environment (WinRE/WinPE) |
| `BR-RE-028` | `WinreNeedsUefi` | WinRE 注册要求已确认的 UEFI 固件环境 | WinRE registration requires a confirmed UEFI firmware environment |
| `BR-RE-029` | `EspLoaderMismatch` | 指定 ESP 与活动 BCD 的目标加载器不一致 | The specified ESP does not match the target loader of the active BCD |
| `BR-RE-030` | `TargetDiskNumberMissing` | 目标磁盘编号缺失 | The target disk number is missing |
| `BR-RE-031` | `TargetPartitionNumberMissing` | 目标分区编号缺失 | The target partition number is missing |
| `BR-RE-032` | `ImageMissingRecoveryPayload` | 还原镜像缺少恢复原件：{0} | The restore image lacks a recovery payload file: {0} |
| `BR-RE-033` | `RecoveryPayloadCopyVerifyFailed` | 恢复原件复制校验失败 | Verification of the copied recovery payload failed |
| `BR-RE-034` | `RecoveryPayloadEmpty` | 恢复原件为空 | The recovery payload file is empty |
| `BR-RE-035` | `SecondaryBackupExists` | 第二系统原注册备份已存在且内容不同 | A secondary-system registration backup already exists with different content |
| `BR-RE-036` | `SecondaryBackupDigestMismatch` | 第二系统原注册备份摘要不符 | The secondary-system registration backup digest does not match |
| `BR-RE-037` | `WinreIdentifierMissing` | WinRE 注册标识符缺失 | The WinRE registration identifier is missing |
| `BR-RE-038` | `RecoveryAssociationConflict` | 系统已有不同的恢复关联，拒绝覆盖 | The system already has a different recovery association; refusing to overwrite |
| `BR-RE-039` | `RecoveryPayloadMutated` | 注册过程改变了恢复原件 | Registration modified the recovery payload file |
| `BR-RE-040` | `TargetLetterInvalid` | 目标盘符无效 | The target drive letter is invalid |
| `BR-RE-041` | `BootSdiRestoreVerifyFailed` | WinRE 启用失败后的 boot.sdi 恢复校验失败 | Verifying the boot.sdi rollback after a failed WinRE enable failed |
| `BR-RE-042` | `WinreIdentityProbeFailed` | WinRE 身份获取失败：reagentc /info 报错（{0}），退回读 BCD 也失败（{1}） | Failed to obtain the WinRE identity: reagentc /info errored ({0}), and falling back to the BCD also failed ({1}) |
| `BR-RE-043` | `BcdNoDefaultLoader` | BCD 未报告默认启动加载器 | The BCD reports no default boot loader |
| `BR-RE-044` | `BcdNoRecoverySequence` | BCD 未报告系统恢复关联（recoverysequence） | The BCD reports no system recovery association (recoverysequence) |
| `BR-RE-045` | `BcdRecoveryNoDevice` | BCD 恢复对象未报告 device | The BCD recovery object reports no device |
| `BR-RE-046` | `BcdRecoveryRamdiskUnrecognized` | BCD 恢复对象的 ramdisk 卷形态无法识别 | The ramdisk volume form of the BCD recovery object is unrecognized |
| `BR-RE-047` | `WinreComponentsMissing` | 目标卷缺少恢复环境注册组件（{0}），无法注册恢复环境；系统虽可启动，但日后无法从恢复环境检修，按裁定拒绝继续 | The target volume lacks a recovery component ({0}); refusing to continue |

### `BR-TSK` — 任务、状态机、续跑（34 条）

| 码 | 变体 | 中文 | English |
|---|---|---|---|
| `BR-TSK-001` | `LogDirMissing` | 日志目录缺失 | The log directory is missing |
| `BR-TSK-002` | `TaskStateInconsistent` | 任务状态记录不一致，拒绝放弃或清理 | The task state record is inconsistent; refusing to abandon or clean up |
| `BR-TSK-003` | `TaskAlreadyAdvanced` | 任务已进入 {0} 阶段，可能已修改目标卷；不能从新任务对话框清理。请先继续原任务或人工恢复。 | The task has reached stage {0} and may have modified the target volume; it cannot be cleaned up from the new-task dialog. Resume the original task or recover manually first. |
| `BR-TSK-004` | `AbandonPayloadVolumeMismatch` | 放弃任务时载荷卷身份不匹配，拒绝删除 | The payload volume identity does not match when abandoning the task; refusing to delete |
| `BR-TSK-005` | `UnterminatedTaskExists` | 检测到尚未终结的任务，未创建新任务或修改启动配置。请先继续原任务，或明确放弃并清理旧任务：{0} | An unfinished task was detected; no new task was created and no boot configuration was changed. Resume the original task first, or explicitly abandon and clean up the old one: {0} |
| `BR-TSK-006` | `BcdbootPowerCutMissed` | BCDBoot 已结束，运行中断电点未命中 | BCDBoot already finished; the mid-run power-cut point was not hit |
| `BR-TSK-007` | `CannotPauseRunningBcdboot` | 无法暂停运行中的 BCDBoot | Cannot suspend a running BCDBoot |
| `BR-TSK-008` | `SpaceFaultNoTarget` | 空间故障缺少目标 | The space fault has no target |
| `BR-TSK-009` | `SpaceFaultNoLetter` | 空间故障缺少盘符 | The space fault has no drive letter |
| `BR-TSK-010` | `SpaceFaultOnlySmallNonSystem` | 空间故障只允许小型非系统测试卷 | The space fault is only allowed on a small non-system test volume |
| `BR-TSK-011` | `AcceptanceInjectFailed` | 验收注入错误：{0} | Acceptance injection error: {0} |
| `BR-TSK-012` | `ResumeBcdFieldDuplicate` | BCD 字段重复，拒绝继续 | Duplicated BCD fields; refusing to continue |
| `BR-TSK-013` | `ResumeOneShotRequestExists` | 存在其他一次性启动请求，拒绝覆盖 | Another one-shot boot request exists; refusing to overwrite it |
| `BR-TSK-014` | `ResumePayloadOutsideStaging` | 续跑载荷路径不属于任务暂存目录 | The resume payload path is outside the task staging directory |
| `BR-TSK-015` | `ResumeBootChainFieldMismatch` | 续跑启动链字段不符：{0} | Resume boot-chain field mismatch: {0} |
| `BR-TSK-016` | `ResumePayloadUnsafe` | 续跑载荷缺失、为空或路径不可安全访问 | The resume payload is missing, empty, or its path cannot be accessed safely |
| `BR-TSK-017` | `TaskIdInvalid` | 任务标识无效：{0}（{1}） | Invalid task id: {0} ({1}) |
| `BR-TSK-018` | `TaskFileMissing` | 任务文件不存在：{0}；recover 的第一个参数必须是**工作区根目录**（包含 tasks 子目录的那一层，例如 H:\\brwork），不是 tasks 目录本身 | The task file does not exist: {0}; the first argument of recover must be the **workspace root** (the level that contains the tasks subdirectory, e.g. H:\\brwork), not the tasks directory itself |
| `BR-TSK-019` | `SpawnConsoleFailed` | 无法启动后台命令提示符（CreateProcessW 失败） | Cannot start the background command prompt (CreateProcessW failed) |
| `BR-TSK-020` | `ConsoleWindowTimeout` | 后台命令提示符已启动，但 3 秒内没等到它的窗口（保持最小化，未最大化） | The background command prompt started but its window did not appear within 3 seconds (leaving it minimized instead of maximizing) |
| `BR-TSK-021` | `TaskNotRecovered` | 任务尚未完成恢复，拒绝清理 | The task has not finished recovering; refusing to clean it up |
| `BR-TSK-022` | `TaskNotResumable` | 该任务不处于可续跑阶段 | The task is not in a resumable stage |
| `BR-TSK-023` | `TaskMissingLaunchRecord` | 任务缺少启动记录 | The task has no launch record |
| `BR-TSK-024` | `RestoreMissingResumeBoot` | 系统还原缺少续跑启动记录，拒绝擦除 | The restore has no resume boot record; refusing to erase |
| `BR-TSK-025` | `ResumePayloadOnEraseTarget` | 续跑载荷位于擦除目标，拒绝执行 | The resume payload lives on the erase target; refusing to proceed |
| `BR-TSK-026` | `ResumePayloadVolumeMismatch` | 续跑载荷卷与本任务镜像卷不符 | The resume payload volume does not match this task's image volume |
| `BR-TSK-027` | `ProgramTimeout` | {0} 超时（上限 {1} ms），进程已终止 | {0} timed out (limit {1} ms); the process was terminated |
| `BR-TSK-029` | `ReStagingTaskIdInvalid` | RE 暂存任务标识不是 UUID：{0} | RE staging task id is not a UUID: {0} |
| `BR-TSK-030` | `ReStagingVolumeNoGuid` | RE 暂存卷没有有效 GUID | RE staging volume has no valid GUID |
| `BR-TSK-031` | `ReStagingCleanupFailed` | RE 暂存清理失败（{0}）：{1} | RE staging cleanup failed at {0}: {1} |
| `BR-TSK-032` | `ReStagingRootNotDirectory` | RE 暂存卷根不是目录 | RE staging volume root is not a directory |
| `BR-TSK-033` | `ReStagingPathNotPlainDir` | RE 暂存路径不是普通目录：{0} | RE staging path is not a plain directory: {0} |
| `BR-TSK-034` | `ReStagingStillExists` | RE 暂存目录仍然存在：{0} | RE staging directory still exists: {0} |
| `BR-TSK-035` | `PreparationCompensationIncomplete` | 准备阶段补偿未完成：{0} | Preparation compensation incomplete: {0} |

### `BR-VOL` — 卷身份与盘符（26 条）

| 码 | 变体 | 中文 | English |
|---|---|---|---|
| `BR-VOL-001` | `VolumeGuidMissing` | 目标卷缺少稳定 GUID 路径 | The target volume has no stable GUID path |
| `BR-VOL-002` | `VolumeGuidMissingRefuseApply` | 目标卷缺少稳定 GUID 路径，拒绝按盘符应用镜像 | The target volume has no stable GUID path; refusing to apply the image by drive letter |
| `BR-VOL-003` | `VolumeFlushOpenFailed` | 卷刷盘失败：无法打开 {0}:（{1}），GetLastError={2} | Volume flush failed: cannot open {0}: ({1}), GetLastError={2} |
| `BR-VOL-004` | `VolumeFlushFailed` | 卷刷盘失败：FlushFileBuffers({0}:) 返回 0（{1}） | Volume flush failed: FlushFileBuffers({0}:) returned 0 ({1}) |
| `BR-VOL-005` | `StableGuidMissing` | 缺少稳定卷 GUID | The stable volume GUID is missing |
| `BR-VOL-006` | `TargetLetterMissing` | 目标盘符缺失 | The target drive letter is missing |
| `BR-VOL-007` | `CurrentSystemVolumeUnknown` | 当前系统盘未知 | The current system volume is unknown |
| `BR-VOL-008` | `DevicePathUnrecognized` | 设备路径形态无法识别：{0} | Unrecognized device path form: {0} |
| `BR-VOL-009` | `SecondarySystemDirAmbiguous` | 无法唯一读取第二系统原系统目录 | Cannot uniquely read the original system directory of the secondary system |
| `BR-VOL-010` | `SecondarySystemDirUnsupported` | 第二系统的原系统目录不受支持 | The original system directory of the secondary system is not supported |
| `BR-VOL-011` | `SecondaryPartitionGuidInvalid` | 第二系统分区 GUID 无效 | The secondary-system partition GUID is invalid |
| `BR-VOL-012` | `SecondaryLetterMappingAmbiguous` | 第二系统盘符映射格式不明确 | The secondary-system drive-letter mapping format is ambiguous |
| `BR-VOL-013` | `SecondaryHiveUnloadFailed` | 第二系统配置卸载失败：{0} | Failed to unload the secondary-system hive: {0} |
| `BR-VOL-014` | `RefuseModifyCurrentSystemHive` | 拒绝修改当前系统的离线配置 | Refusing to modify the offline configuration of the running system |
| `BR-VOL-015` | `SecondaryMissingHive` | 第二系统缺少注册表配置 | The secondary system lacks its registry hive |
| `BR-VOL-016` | `SecondaryConfigDigestMismatch` | 第二系统配置备份摘要不符 | The secondary-system configuration backup digest does not match |
| `BR-VOL-017` | `SecondaryLetterReadbackMismatch` | 第二系统盘符映射回读不符 | The secondary-system drive-letter mapping does not read back as written |
| `BR-VOL-018` | `DevicePathQueryFailed` | QueryDosDevice({0}:) 失败：卷的 NT 设备路径未知，拒绝写盘符形式规格 | QueryDosDevice({0}:) failed: the volume NT device path is unknown; refusing to write a drive-letter spec |
| `BR-VOL-019` | `DevicePathUnexpected` | 盘符 {0}: 的设备路径形态非预期：{1} | Unexpected device-path form for drive {0}: {1} |
| `BR-VOL-020` | `MountFallbackNoVolumeId` | 挂载后备路径缺少有效卷标识符 | The mount fallback path has no valid volume identifier |
| `BR-VOL-021` | `LiveDiskNumberMissing` | 现场磁盘编号缺失 | The live disk number is missing |
| `BR-VOL-022` | `LivePartitionNumberMissing` | 现场分区编号缺失 | The live partition number is missing |
| `BR-VOL-023` | `VolumeRecordUnresolvable` | 无法定位任务记录的卷（卷 GUID {0}，磁盘 {1} 分区 {2}，大小 {3}，记录盘符 {4}）；已尝试：{5} | Cannot locate the volume recorded by the task (volume GUID {0}, disk {1} partition {2}, size {3}, recorded letter {4}); tried: {5} |
| `BR-VOL-024` | `VolumeGuidMalformed` | 卷 GUID 形态无效，无法分配盘符 | The volume GUID format is invalid; cannot assign a drive letter |
| `BR-VOL-025` | `VolumeExistsNoLetter` | 卷存在但无法分配盘符 | The volume exists but no drive letter could be assigned |
| `BR-VOL-026` | `FormatTargetLetterMissing` | 格式化目标盘符缺失 | The format target drive letter is missing |

## 刻意无码的一类（不会出现在上表）

命令行**用法**错误（`task id is required`、`--target-drive is required`、
`unsupported operation: xxx`，共 300+ 处）**故意不发码**：

1. 本来就是英文技术提示，不需要翻译；
2. 文案本身唯一且稳定，贴文案即可定位；
3. 给 300 多条用法提示各发一个码，只会淹没真正有价值的运行期码。

判读规则：**`err()` = 用法错误（无码）；`errc`/`errc_with` = 带码运行期失败。**
