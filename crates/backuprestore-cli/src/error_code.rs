//! 错误码与错误文案的**单一来源**。
//!
//! 为什么要这张表（2026-10-25 用户拍板）：
//! 1. **码是契约**——用户报障说 `BR-VOL-003` 就够，不用贴一整段中文；维护者按码
//!    定位到唯一一行，也不会因为「文案改了一个词」而对不上。
//! 2. **文案要能换语言**——`TaskError` 里塞死中文字面量，英文界面下错误信息仍是
//!    中文。v2.6.43 只把 `append_log` 的日志文案多语言化了，`err()` 的 245 处没动。
//! 3. **单测不回归**——默认语言仍是中文、文案逐字保持原样，历史 `contains("中文片段")`
//!    断言照旧命中；只是 `Display` 多了一个 `[BR-XXX-NNN] ` 前缀（单测用 `contains`，
//!    不是全等，所以不受影响）。
//!
//! 码的格式：`BR-<域>-<三位序号>`。**序号一旦发布永不重排**——重排等于改契约。
//! 新增时在对应域末尾追加，**不复用**已退役的号。
//!
//! 域划分：
//! - `CFG` 命令行参数 / 配置项 / 选项组合
//! - `PRE` 输入预检、参数校验、路径组件
//! - `VOL` 卷身份、盘符、几何、稳定编号
//! - `IMG` 镜像文件、副档、索引、哈希
//! - `APL` 应用镜像（DISM / wimlib 通道）
//! - `BOOT` 引导、BCD、启动项、启动菜单
//! - `RE` 恢复环境注册、WinRE 关联
//! - `PE` PE 通道与写盘契约（含安全探测）
//! - `TSK` 任务状态机、续跑、任务目录
//! - `GUI` 图形界面
//!
//! 用宏而不是 `const` 数组 + 字符串 key：调用点写 `ErrCode::VolumeChanged`
//! 拼错**编译期就报**；字符串 key 只能靠运行期单测兜。
//!
//! # 什么带码、什么不带码（2026-10-25 划定的边界）
//!
//! **带码**：运行期失败——卷身份不符、镜像损坏、引导回读不一致、WinRE 注册
//! 关联错误、任务状态机拒绝…… 这些是用户真正会撞上的，且文案要双语。
//!
//! **不带码**：命令行**用法**错误（`task id is required`、`--target-drive is
//! required`、`unsupported operation: xxx`）。理由：① 它们本来就是英文技术
//! 提示、不需要翻译；② 文案本身唯一且稳定，用户贴文案即可定位；③ 给 300 多条
//! 用法提示各发一个码，只会淹没真正有价值的运行期码。
//!
//! 所以 `crates/` 里仍然存在 `err(...)` 调用；**看到 `err` 就当"用法错误"，
//! 看到 `errc`/`errc_with` 才是带码的运行期失败**。
//!
//! # 三个结构性码（不在本表里）
//!
//! `TaskError` 的结构变体由 `TaskError::code()` 直接返回固定码，`Display`
//! **不加** `[码] ` 前缀（它们的文案是 OS 本地化串或 serde 原文，加前缀只会更难看）：
//!
//! | 变体 | 码 | 含义 |
//! |---|---|---|
//! | `Io(_)` | `BR-IO-001` | 文件/系统调用失败（文案随系统语言） |
//! | `Json(_)` | `BR-IO-002` | JSON 读写/解析失败 |
//! | `InvalidTransition{..}` | `BR-TSK-028` | 状态机非法迁移 |
//!
//! 它们只在 `--json` 的 `error_code` 字段与本文档里出现，供自动化区分
//! 「环境失败」与「脚本写错」。

use backuprestore_core::TaskError;

macro_rules! error_codes {
    (
        $(
            $(#[$meta:meta])*
            $variant:ident => ($code:literal, $zh:literal, $en:literal)
        ),* $(,)?
    ) => {
        /// 稳定的错误码枚举。变体名是给人看的，`code()` 是给契约看的。
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum ErrCode {
            $($(#[$meta])* $variant),*
        }

        impl ErrCode {
            /// 稳定机器码（`BR-<域>-<序号>`）。**发布后不得更改**。
            pub(crate) fn code(self) -> &'static str {
                match self {
                    $(Self::$variant => $code),*
                }
            }

            /// 中文模板（`{0}`、`{1}` 为占位）。默认语言，也是历史文案的逐字原文。
            pub(crate) fn zh(self) -> &'static str {
                match self {
                    $(Self::$variant => $zh),*
                }
            }

            /// 英文模板，占位与中文一一对应。
            pub(crate) fn en(self) -> &'static str {
                match self {
                    $(Self::$variant => $en),*
                }
            }
        }

        /// 全量清单，供单测做「码唯一 / 中英占位一致 / 英文无 CJK」校验。
        #[cfg(test)]
        pub(crate) const ALL_CODES: &[ErrCode] = &[$(ErrCode::$variant),*];
    };
}

error_codes! {
    // ── PE 通道与写盘契约（`pe_safety`）──────────────────────────────────
    // 这批全部是**拒写**判据：宁可拒绝，也不要在身份不明时动别人的盘。
    ImgPathComponent => ("BR-PE-001", "镜像相对路径包含无效组件", "Image relative path contains an invalid component"),
    LoaderNoObjectGuid => ("BR-PE-002", "目标加载器缺少对象 GUID", "Target loader has no object GUID"),
    LoaderNotUnique => ("BR-PE-003", "目标引导加载器不唯一或设备字段不完整", "Target boot loader is not unique or its device field is incomplete"),
    VolumeChanged => ("BR-PE-004", "卷身份、现场编号或几何已变化，拒绝写入", "Volume identity, live number or geometry changed; refusing to write"),
    PrepareIdentityIncomplete => ("BR-PE-005", "准备期卷身份不完整，拒绝重新绑定", "Prepared volume identity is incomplete; refusing to rebind"),
    BootVolumeChanged => ("BR-PE-006", "跨启动卷身份或文件系统变化，拒绝重新绑定", "Boot volume identity or file system changed; refusing to rebind"),
    TargetNotGptNtfs => ("BR-PE-007", "目标必须是身份完整的 GPT/NTFS 基本数据分区", "Target must be an identifiable GPT/NTFS basic data partition"),
    TargetRoleConflict => ("BR-PE-008", "目标与镜像、工作区、运行程序或恢复载荷冲突，或必需角色未知", "Target conflicts with the image, workspace, running program or recovery payload, or its required role is unknown"),
    SecondaryNoPrimary => ("BR-PE-009", "第二系统任务缺少已确认的主系统身份", "Secondary-system task lacks a confirmed primary-system identity"),
    SecondaryCannotOverwrite => ("BR-PE-010", "第二系统不能覆盖主系统，确认不能豁免角色禁区", "A secondary system must not overwrite the primary; confirmation cannot waive the role exclusion"),
    ProbeRootUnusable => ("BR-PE-011", "探测卷根不可用或存在重解析", "Probe volume root is unusable or carries a reparse point"),
    ProbeNoReparse => ("BR-PE-012", "安全探测不跟随重解析点", "Safe probing does not follow reparse points"),
    ProbeNoSymlink => ("BR-PE-013", "安全探测不跟随链接", "Safe probing does not follow links"),
    NotRegularFile => ("BR-PE-014", "必要文件路径不是普通文件", "A required file path is not a regular file"),
    AncestorNotDirectory => ("BR-PE-015", "探测祖先路径不是目录", "A probed ancestor path is not a directory"),
    ParentMissing => ("BR-PE-016", "探测父路径缺失", "Probed parent path is missing"),
    ProbePathEmpty => ("BR-PE-017", "探测路径为空", "Probed path is empty"),
    StateWriteFailed => ("BR-PE-018", "{0}；失败状态落盘也失败：{1}", "{0}; writing the failure state also failed: {1}"),
    // 只在测试注入路径构造（故障矩阵用它模拟后端失败）。
    #[allow(dead_code)]
    InjectFailed => ("BR-PE-019", "注入失败", "Injection failed"),

    // ── 恢复环境注册：`reagentc` 状态、BCD 恢复关联、恢复原件（`winre_registration`）──
    // 这批全是「读不明白就拒写」：恢复关联搞错会让开机进不了系统或进错系统。
    WinreStateAmbiguous => ("BR-RE-001", "无法唯一确认 WinRE 启用状态", "Cannot uniquely determine the WinRE enabled state"),
    WinreLocationNotUnique => ("BR-RE-002", "WinRE 注册位置不唯一", "The WinRE registration location is not unique"),
    WinreIdentifierAmbiguous => ("BR-RE-003", "WinRE 注册标识符缺失或不唯一", "The WinRE registration identifier is missing or not unique"),
    BcdFieldDuplicate => ("BR-RE-004", "BCD 字段缺失或重复：{0}", "BCD field is missing or duplicated: {0}"),
    BcdObjectDuplicate => ("BR-RE-005", "BCD 对象缺失或重复", "BCD object is missing or duplicated"),
    WinreNotEnabledOrWrongLocation => ("BR-RE-006", "WinRE 未启用或注册位置与目标分区不符", "WinRE is not enabled, or its registration location does not match the target partition"),
    WinreLoaderChanged => ("BR-RE-007", "WinRE 关联的目标系统加载器已变化", "The system loader associated with WinRE has changed"),
    RecoveryGuidNotUnique => ("BR-RE-008", "系统恢复关联不是唯一 GUID", "The system recovery association is not a unique GUID"),
    RecoveryEntryNotEnabled => ("BR-RE-009", "系统恢复启动未启用", "The system recovery boot entry is not enabled"),
    RecoveryBcdMismatch => ("BR-RE-010", "BCD 恢复关联与 WinRE 注册标识符不一致", "The BCD recovery association disagrees with the WinRE registration identifier"),
    RecoveryDeviceNotRamdisk => ("BR-RE-011", "恢复加载器设备不是内存盘", "The recovery loader device is not a RAM disk"),
    RecoveryDeviceNoPartitionId => ("BR-RE-012", "恢复加载器设备缺少分区标识", "The recovery loader device has no partition identifier"),
    RecoveryDeviceNoOptionObject => ("BR-RE-013", "恢复加载器设备缺少选项对象", "The recovery loader device has no options object"),
    PartitionTokenUnrecognized => ("BR-RE-014", "{0}的分区标识无法识别：{1}", "Unrecognized partition identifier for {0}: {1}"),
    PartitionLetterMismatch => ("BR-RE-015", "{0}指向的 {1}: 不是登记位置所在的 disk{2}/partition{3}", "{0} points to drive {1}, which is not disk{2}/partition{3} of the registered location"),
    // 只在 `cfg(not(windows))` 分支构造，Windows 构建里天然用不到。
    #[allow(dead_code)]
    PartitionVerifyWindowsOnly => ("BR-RE-016", "恢复对象分区核对仅在 Windows 上可用", "Recovery-object partition verification is only available on Windows"),
    PartitionDeviceMismatch => ("BR-RE-017", "{0}指向的 {1} 是 disk{2}/partition{3}，不是登记位置所在的 disk{4}/partition{5}", "{0} points to {1}, which is disk{2}/partition{3}, not disk{4}/partition{5} of the registered location"),
    RecoveryLoaderNotOriginalWim => ("BR-RE-018", "恢复加载器未指向原始 Winre.wim", "The recovery loader does not point to the original Winre.wim"),
    RecoveryLoaderOptionInvalid => ("BR-RE-019", "恢复加载器选项对象标识符无效", "The recovery loader options object identifier is invalid"),
    RecoveryLoaderIncomplete => ("BR-RE-020", "恢复加载器设备或执行模式不完整", "The recovery loader device or execution mode is incomplete"),
    RamdiskDeviceFormatInvalid => ("BR-RE-021", "恢复内存盘启动设备格式无效", "The recovery RAM-disk boot device format is invalid"),
    RamdiskFileNotInTarget => ("BR-RE-022", "恢复内存盘启动文件不在目标注册目录", "The recovery RAM-disk boot file is not in the target registration directory"),
    SecondaryCloneStillEnabled => ("BR-RE-023", "第二系统克隆配置重置后仍启用，拒绝修改原恢复对象", "The secondary-system clone configuration is still enabled after reset; refusing to modify the original recovery object"),
    WinreLocationMissing => ("BR-RE-024", "已启用 WinRE 缺少注册位置", "WinRE is enabled but its registration location is missing"),
    WinreLocationUnparsable => ("BR-RE-025", "已启用 WinRE 的注册位置无法解析：{0}", "Cannot parse the registration location of the enabled WinRE: {0}"),
    WinrePartitionMissingFiles => ("BR-RE-026", "已启用 WinRE 位于独立恢复分区（{0}），但该分区缺少可用的 {1}，拒绝重接", "The enabled WinRE lives on an independent recovery partition ({0}), but that partition lacks a usable {1}; refusing to rewire"),
    OfflineWinreReOnly => ("BR-RE-027", "离线 WinRE 注册只允许在恢复环境（WinRE/WinPE）中执行", "Offline WinRE registration may only run in the recovery environment (WinRE/WinPE)"),
    WinreNeedsUefi => ("BR-RE-028", "WinRE 注册要求已确认的 UEFI 固件环境", "WinRE registration requires a confirmed UEFI firmware environment"),
    EspLoaderMismatch => ("BR-RE-029", "指定 ESP 与活动 BCD 的目标加载器不一致", "The specified ESP does not match the target loader of the active BCD"),
    TargetDiskNumberMissing => ("BR-RE-030", "目标磁盘编号缺失", "The target disk number is missing"),
    TargetPartitionNumberMissing => ("BR-RE-031", "目标分区编号缺失", "The target partition number is missing"),
    ImageMissingRecoveryPayload => ("BR-RE-032", "还原镜像缺少恢复原件：{0}", "The restore image lacks a recovery payload file: {0}"),
    RecoveryPayloadCopyVerifyFailed => ("BR-RE-033", "恢复原件复制校验失败", "Verification of the copied recovery payload failed"),
    RecoveryPayloadEmpty => ("BR-RE-034", "恢复原件为空", "The recovery payload file is empty"),
    SecondaryBackupExists => ("BR-RE-035", "第二系统原注册备份已存在且内容不同", "A secondary-system registration backup already exists with different content"),
    SecondaryBackupDigestMismatch => ("BR-RE-036", "第二系统原注册备份摘要不符", "The secondary-system registration backup digest does not match"),
    WinreIdentifierMissing => ("BR-RE-037", "WinRE 注册标识符缺失", "The WinRE registration identifier is missing"),
    RecoveryAssociationConflict => ("BR-RE-038", "系统已有不同的恢复关联，拒绝覆盖", "The system already has a different recovery association; refusing to overwrite"),
    RecoveryPayloadMutated => ("BR-RE-039", "注册过程改变了恢复原件", "Registration modified the recovery payload file"),
    TargetLetterInvalid => ("BR-RE-040", "目标盘符无效", "The target drive letter is invalid"),
    BootSdiRestoreVerifyFailed => ("BR-RE-041", "WinRE 启用失败后的 boot.sdi 恢复校验失败", "Verifying the boot.sdi rollback after a failed WinRE enable failed"),

    // ── PE 通道编排：请求排队、任务文件、预检门禁（`pe_operation`）────────
    PendingRequestExists => ("BR-PE-020", "已有待执行 PE 请求，请先处理原任务", "A pending PE request already exists; finish or discard it first"),
    TaskWriteReadbackMismatch => ("BR-PE-021", "任务写后回读不一致", "Task read-back after writing does not match"),
    PendingRequestConflict => ("BR-PE-022", "多个待执行请求冲突，拒绝猜测", "Multiple pending requests conflict; refusing to guess"),
    ImageMustBeLocalAbsolute => ("BR-PE-023", "镜像必须位于本地卷的绝对路径", "The image must be an absolute path on a local volume"),
    ImageIndexMustBePositive => ("BR-PE-024", "镜像索引必须大于零", "The image index must be greater than zero"),
    UnknownPeOperation => ("BR-PE-025", "未知 PE 操作", "Unknown PE operation"),
    TaskIdGenerationFailed => ("BR-PE-026", "无法生成任务编号", "Cannot generate a task id"),
    DriveLetterInvalid => ("BR-PE-027", "盘符无效", "The drive letter is invalid"),
    VolumeNoStableGuid => ("BR-PE-028", "卷缺少稳定 GUID 路径", "The volume has no stable GUID path"),
    PeWriterBusy => ("BR-PE-029", "已有 PE 写任务运行，拒绝并发执行", "A PE write task is already running; refusing concurrent execution"),
    UnsupportedPeTaskVersion => ("BR-PE-030", "不支持的 PE 任务版本或动作", "Unsupported PE task version or action"),
    TaskIdHasPath => ("BR-PE-031", "任务编号不能含路径", "The task id must not contain a path"),
    ImageNoParentDir => ("BR-PE-032", "镜像没有父目录", "The image path has no parent directory"),
    ImageFileMissing => ("BR-PE-033", "镜像文件不存在，未执行任何格式化", "The image file does not exist; no formatting was performed"),
    BackupDirReparse => ("BR-PE-034", "备份目录不能经过重解析点", "The backup directory must not traverse a reparse point"),
    Utf16OutputTruncated => ("BR-PE-035", "UTF-16 输出被截断", "UTF-16 output was truncated"),
    Utf16DecodeFailed => ("BR-PE-036", "UTF-16 输出解码失败", "Failed to decode UTF-16 output"),
    ConsoleDecodeFailed => ("BR-PE-037", "控制台输出解码失败", "Failed to decode console output"),
    SystemRootUnknown => ("BR-PE-038", "SystemRoot 未知", "SystemRoot is unknown"),
    ProgramFailed => ("BR-PE-039", "{0} 失败：{1}\n{2}", "{0} failed: {1}\n{2}"),
    RunningVolumeUnknown => ("BR-PE-040", "运行卷未知", "The running volume is unknown"),
    CannotWriteRunningVolume => ("BR-PE-041", "不能写入当前运行卷", "Refusing to write to the currently running volume"),
    PayloadSourceUnknown => ("BR-PE-042", "当前恢复载荷来源未知，拒绝格式化", "The recovery payload source is unknown; refusing to format"),
    PayloadVolumeUnresolvable => ("BR-PE-043", "恢复载荷卷无法解析", "Cannot resolve the recovery payload volume"),
    PrimaryVolumeLacksEvidence => ("BR-PE-044", "需保留的主系统卷缺少系统证据", "The primary volume that must be preserved lacks system evidence"),
    ImageExpandedSizeUnknown => ("BR-PE-045", "镜像展开容量未知，拒绝写入", "The expanded image size is unknown; refusing to write"),
    ImageSizeOverflow => ("BR-PE-046", "镜像容量溢出", "Image size overflow"),
    SidecarMismatch => ("BR-PE-047", "副档索引或摘要不匹配", "Sidecar index or digest does not match"),
    TargetCapacityInsufficient => ("BR-PE-048", "目标容量不足：{0} < {1}", "Target capacity is insufficient: {0} < {1}"),
    DataOverlaySpaceInsufficient => ("BR-PE-049", "数据覆盖目标可用空间不足", "Not enough free space for the data overlay"),
    ImageMissingSystemFile => ("BR-PE-050", "镜像缺少系统文件 {0}；不能按系统还原成功", "The image lacks the system file {0}; it cannot count as a successful system restore"),
    BootManagerStateUnknown => ("BR-PE-051", "启动管理器状态未知，拒绝格式化", "Boot manager state is unknown; refusing to format"),
    ProgramDirUnresolvable => ("BR-PE-052", "无法定位程序目录（wimlib 通道）：{0}", "Cannot locate the program directory (wimlib path): {0}"),
    ProgramDirResolveFailed => ("BR-PE-053", "程序目录解析失败（wimlib 通道）", "Resolving the program directory failed (wimlib path)"),
    SystemFileVerifyFailed => ("BR-PE-054", "系统文件验证失败，不能降级为数据还原成功", "System-file verification failed; this cannot be reported as a successful data restore"),
    BootPermitMissingEsp => ("BR-PE-055", "引导许可缺少 ESP", "The boot permit is missing an ESP"),
    MissingPreFormatBootState => ("BR-PE-056", "缺少格式化前引导状态", "The pre-format boot state is missing"),
    BootStateChangedAfterPrecheck => ("BR-PE-057", "预检后引导状态发生变化，停止引导写入", "Boot state changed after preflight; stopping boot writes"),
    SecondaryMenuOrderInvalid => ("BR-PE-058", "第二系统默认项或菜单顺序不符合约定，拒绝报成功", "The secondary-system default entry or menu order violates the contract; refusing to report success"),

    // ── 卷身份与盘符（`windows_prepare` / `online_operation`）─────────────
    VolumeGuidMissing => ("BR-VOL-001", "目标卷缺少稳定 GUID 路径", "The target volume has no stable GUID path"),
    VolumeGuidMissingRefuseApply => ("BR-VOL-002", "目标卷缺少稳定 GUID 路径，拒绝按盘符应用镜像", "The target volume has no stable GUID path; refusing to apply the image by drive letter"),
    VolumeFlushOpenFailed => ("BR-VOL-003", "卷刷盘失败：无法打开 {0}:（{1}），GetLastError={2}", "Volume flush failed: cannot open {0}: ({1}), GetLastError={2}"),
    VolumeFlushFailed => ("BR-VOL-004", "卷刷盘失败：FlushFileBuffers({0}:) 返回 0（{1}）", "Volume flush failed: FlushFileBuffers({0}:) returned 0 ({1})"),
    StableGuidMissing => ("BR-VOL-005", "缺少稳定卷 GUID", "The stable volume GUID is missing"),
    TargetLetterMissing => ("BR-VOL-006", "目标盘符缺失", "The target drive letter is missing"),
    CurrentSystemVolumeUnknown => ("BR-VOL-007", "当前系统盘未知", "The current system volume is unknown"),
    DevicePathUnrecognized => ("BR-VOL-008", "设备路径形态无法识别：{0}", "Unrecognized device path form: {0}"),

    // ── 任务与状态机（`windows_prepare`）──────────────────────────────────
    LogDirMissing => ("BR-TSK-001", "日志目录缺失", "The log directory is missing"),
    TaskStateInconsistent => ("BR-TSK-002", "任务状态记录不一致，拒绝放弃或清理", "The task state record is inconsistent; refusing to abandon or clean up"),
    TaskAlreadyAdvanced => ("BR-TSK-003", "任务已进入 {0} 阶段，可能已修改目标卷；不能从新任务对话框清理。请先继续原任务或人工恢复。", "The task has reached stage {0} and may have modified the target volume; it cannot be cleaned up from the new-task dialog. Resume the original task or recover manually first."),
    AbandonPayloadVolumeMismatch => ("BR-TSK-004", "放弃任务时载荷卷身份不匹配，拒绝删除", "The payload volume identity does not match when abandoning the task; refusing to delete"),
    UnterminatedTaskExists => ("BR-TSK-005", "检测到尚未终结的任务，未创建新任务或修改启动配置。请先继续原任务，或明确放弃并清理旧任务：{0}", "An unfinished task was detected; no new task was created and no boot configuration was changed. Resume the original task first, or explicitly abandon and clean up the old one: {0}"),

    // ── 镜像文件与 DISM 探测（`windows_prepare` / `online_operation`）─────
    ImageNotCompleteWindows => ("BR-IMG-001", "镜像不是完整 Windows（缺少 reagentc.exe 或 SYSTEM 注册表），还原/新增第二系统需要完整 Windows 镜像。要安装 PE，请改用「安装 PE」功能（内存版或硬盘版），它才是为 PE 设计的路径。本次未格式化任何卷、未写入引导。", "The image is not a complete Windows image (missing reagentc.exe or the SYSTEM registry). Restoring or adding a secondary system needs a complete Windows image. To install PE, use the \"Install PE\" feature (RAM-disk or disk-based); that is the path designed for PE. No volume was formatted and no boot entry was written this run."),
    ImageIsWindowsPe => ("BR-IMG-002", "镜像\"{0}\"是 Windows PE，不是完整 Windows；要安装 PE 请改用「安装 PE」功能。本次未格式化任何卷、未写入引导。", "Image \"{0}\" is Windows PE, not a complete Windows image. To install PE, use the \"Install PE\" feature. No volume was formatted and no boot entry was written this run."),
    DismSpawnFailed => ("BR-IMG-003", "dism 调用失败（无法启动/读取输出）：{0}", "Failed to invoke dism (cannot start it or read its output): {0}"),
    ImageMountProbeFailed => ("BR-IMG-004", "镜像挂载探测失败（dism /Mount-Wim 退出码 {0}{1}）。完整输出已写入准备日志。", "Image mount probe failed (dism /Mount-Wim exit code {0}{1}). The full output was written to the preparation log."),
    ImageUnmountFailed => ("BR-IMG-005", "镜像挂载探测的卸载失败（dism /Unmount-Wim 退出码 {0}{1}）。镜像可能仍处于挂载状态并被锁定，请手动用 dism /Unmount-Wim /Discard 清理后再试。", "Unmounting the probe mount failed (dism /Unmount-Wim exit code {0}{1}). The image may still be mounted and locked; clean it up manually with dism /Unmount-Wim /Discard and retry."),
    ImageNameReadFailed => ("BR-IMG-006", "读取镜像名失败（dism /Get-WimInfo 退出码 {0}{1}）。请确认镜像文件可访问且未被占用。", "Failed to read the image name (dism /Get-WimInfo exit code {0}{1}). Make sure the image file is accessible and not in use."),
    DeleteIndexReadbackMismatch => ("BR-IMG-007", "删除旧索引后回读的索引集合与期望不一致，已中止；镜像可能停在半删状态，请用 wim-info 核实后再试", "The index set read back after deleting old indexes does not match the expectation; aborted. The image may be left half-deleted - verify with wim-info before retrying"),
    BackupSpaceInsufficient => ("BR-IMG-008", "镜像所在卷可用空间不足以开始备份：可用 {0}，至少需要 {1}（源卷已用 {2}）。请换一个更大的卷或先清理镜像卷；本次未写入任何镜像数据。", "Not enough free space on the volume holding the image to start a backup: {0} available, at least {1} required (source volume uses {2}). Use a larger volume or free space on the image volume first; no image data was written this run."),

    // ── 引导与启动菜单（`online_operation` / `windows_prepare`）───────────
    BootManagerUnknownContinue => ("BR-BOOT-001", "引导管理器状态未知，拒绝继续", "Boot manager state is unknown; refusing to continue"),
    PostBootLoaderMissing => ("BR-BOOT-002", "bcdboot 后找不到第二系统的启动加载器", "No boot loader for the secondary system was found after bcdboot"),
    PostBootManagerUnparsable => ("BR-BOOT-003", "无法解析还原后的引导管理器状态，拒绝报成功", "Cannot parse the boot manager state after restore; refusing to report success"),
    DefaultNotRestoredTarget => ("BR-BOOT-004", "默认启动项未指向本次还原的系统，拒绝报成功", "The default boot entry does not point to the system restored this run; refusing to report success"),
    MenuEntryLost => ("BR-BOOT-005", "原有启动菜单项丢失，拒绝报成功", "A pre-existing boot menu entry was lost; refusing to report success"),
    SecondaryMenuEntryMissing => ("BR-BOOT-006", "第二系统启动项未进入菜单，拒绝报成功", "The secondary-system boot entry did not make it into the menu; refusing to report success"),
    ResumeObjectNotRestored => ("BR-BOOT-007", "bootmgr resume_object 未恢复为主系统，拒绝报成功", "bootmgr resume_object was not restored to the primary system; refusing to report success"),
    RecoveryObjectDestroyed => ("BR-BOOT-008", "原有系统恢复对象被破坏，拒绝报成功", "The original system recovery object was damaged; refusing to report success"),
    NoBcdEfiPartition => ("BR-BOOT-009", "未发现具有可读取 BCD 的 GPT EFI 分区", "No GPT EFI partition with a readable BCD was found"),
    MultipleEspAmbiguous => ("BR-BOOT-010", "检测到多个含 BCD 的 EFI 系统分区（{0}），且无法唯一确定本次引导使用的是哪一个（BCD 指纹比对无唯一匹配）。请避免多 ESP 布局，或仅保留引导磁盘上的 ESP 后重试", "Multiple EFI system partitions with a BCD were found ({0}), and it cannot be determined which one this boot used (no unique BCD fingerprint match). Avoid multi-ESP layouts, or keep only the ESP on the boot disk and retry"),

    // ── 恢复环境身份探测（`windows_prepare` 的 BCD 读取）──────────────────
    WinreIdentityProbeFailed => ("BR-RE-042", "WinRE 身份获取失败：reagentc /info 报错（{0}），退回读 BCD 也失败（{1}）", "Failed to obtain the WinRE identity: reagentc /info errored ({0}), and falling back to the BCD also failed ({1})"),
    BcdNoDefaultLoader => ("BR-RE-043", "BCD 未报告默认启动加载器", "The BCD reports no default boot loader"),
    BcdNoRecoverySequence => ("BR-RE-044", "BCD 未报告系统恢复关联（recoverysequence）", "The BCD reports no system recovery association (recoverysequence)"),
    BcdRecoveryNoDevice => ("BR-RE-045", "BCD 恢复对象未报告 device", "The BCD recovery object reports no device"),
    BcdRecoveryRamdiskUnrecognized => ("BR-RE-046", "BCD 恢复对象的 ramdisk 卷形态无法识别", "The ramdisk volume form of the BCD recovery object is unrecognized"),
    /// 目标卷缺少恢复环境注册组件。
    ///
    /// 【2026-10-28 用户裁定】此前这种情况会「降级跳过」——任务报成功，但新系统
    /// 根本没有恢复环境。用户明确要求改为失败：看到「还原成功」却进不去恢复环境，
    /// 比当场失败危险得多。
    WinreComponentsMissing => ("BR-RE-047", "目标卷缺少恢复环境注册组件（{0}），无法注册恢复环境；系统虽可启动，但日后无法从恢复环境检修，按裁定拒绝继续", "The target volume lacks a recovery component ({0}); refusing to continue"),

    // ── 应用镜像（`online_operation`）────────────────────────────────────
    DismApplyFailedOnline => ("BR-APL-001", "DISM failed: {0}\n--- 输出尾部（至多 {1} 字节）---\n{2}", "DISM failed: {0}\n--- output tail (at most {1} bytes) ---\n{2}"),
    WimlibBinaryMissing => ("BR-APL-002", "覆盖式应用需要 wimlib 二进制：未找到 {0}（{1} 须与其同目录）。请把 wimlib 官方 Windows ARM64 预编译包的两个文件放进程序目录后重试；不会回退 DISM（该组合下 DISM 必然失败）。", "Overlay apply needs the wimlib binary: {0} was not found ({1} must sit in the same directory). Put both files from the official wimlib Windows ARM64 prebuilt package into the program directory and retry; DISM is not used as a fallback (it always fails in this combination)."),
    WimlibDllMissing => ("BR-APL-003", "覆盖式应用需要 wimlib 依赖库：{0} 已就位，但同目录缺少 {1}。wimlib 启动时加载不到该库会被系统直接终止（报错不含这两个文件名，也无法归因到部署缺口）；请把 wimlib 官方 Windows ARM64 预编译包的两个文件一起放进程序目录后重试；不会回退 DISM（该组合下 DISM 必然失败）。", "Overlay apply needs the wimlib library: {0} is present but {1} is missing from the same directory. If the loader cannot find it, the OS terminates wimlib immediately (the error names neither file, so a deployment gap cannot be inferred from it). Put both files from the official wimlib Windows ARM64 prebuilt package into the program directory and retry; DISM is not used as a fallback (it always fails in this combination)."),
    WimlibApplyFailed => ("BR-APL-004", "wimlib apply failed: {0}\n--- 输出尾部（至多 8192 字节）---\n{1}", "wimlib apply failed: {0}\n--- output tail (at most 8192 bytes) ---\n{1}"),
    ProgramDirUnresolvableFallback => ("BR-APL-005", "无法定位程序目录（wimlib 降级通道）：{0}", "Cannot locate the program directory (wimlib fallback path): {0}"),
    ProgramDirResolveFailedFallback => ("BR-APL-006", "程序目录解析失败（wimlib 降级通道）", "Resolving the program directory failed (wimlib fallback path)"),
    DismApplyFailedWithReason => ("BR-APL-007", "DISM apply 失败：{0}\n{1}\n{2}", "DISM apply failed: {0}\n{1}\n{2}"),
    FallbackAlsoFailed => ("BR-APL-008", "DISM apply 失败：{0}\n自动降级 wimlib 重试仍失败：{1}\n{2}\n{3}", "DISM apply failed: {0}\nThe automatic wimlib fallback also failed: {1}\n{2}\n{3}"),

    // ── 第二系统离线注册表（`secondary_registry`）───────────────────────
    SecondarySystemDirAmbiguous => ("BR-VOL-009", "无法唯一读取第二系统原系统目录", "Cannot uniquely read the original system directory of the secondary system"),
    SecondarySystemDirUnsupported => ("BR-VOL-010", "第二系统的原系统目录不受支持", "The original system directory of the secondary system is not supported"),
    SecondaryPartitionGuidInvalid => ("BR-VOL-011", "第二系统分区 GUID 无效", "The secondary-system partition GUID is invalid"),
    SecondaryLetterMappingAmbiguous => ("BR-VOL-012", "第二系统盘符映射格式不明确", "The secondary-system drive-letter mapping format is ambiguous"),
    SecondaryHiveUnloadFailed => ("BR-VOL-013", "第二系统配置卸载失败：{0}", "Failed to unload the secondary-system hive: {0}"),
    RefuseModifyCurrentSystemHive => ("BR-VOL-014", "拒绝修改当前系统的离线配置", "Refusing to modify the offline configuration of the running system"),
    SecondaryMissingHive => ("BR-VOL-015", "第二系统缺少注册表配置", "The secondary system lacks its registry hive"),
    SecondaryConfigDigestMismatch => ("BR-VOL-016", "第二系统配置备份摘要不符", "The secondary-system configuration backup digest does not match"),
    SecondaryLetterReadbackMismatch => ("BR-VOL-017", "第二系统盘符映射回读不符", "The secondary-system drive-letter mapping does not read back as written"),
    DevicePathQueryFailed => ("BR-VOL-018", "QueryDosDevice({0}:) 失败：卷的 NT 设备路径未知，拒绝写盘符形式规格", "QueryDosDevice({0}:) failed: the volume NT device path is unknown; refusing to write a drive-letter spec"),
    DevicePathUnexpected => ("BR-VOL-019", "盘符 {0}: 的设备路径形态非预期：{1}", "Unexpected device-path form for drive {0}: {1}"),

    // ── 引导修复方案与回读（`boot_menu`）────────────────────────────────
    MenuTargetMustAppearOnce => ("BR-BOOT-011", "本次还原系统必须且只能出现一次，不能取消", "The system restored this run must appear exactly once and cannot be deselected"),
    MenuTimeoutOutOfRange => ("BR-BOOT-012", "启动菜单等待时间超出范围", "The boot menu timeout is out of range"),
    DefaultCannotBeRemoved => ("BR-BOOT-013", "当前默认启动的系统不能从启动菜单中移除", "The system currently booted by default cannot be removed from the boot menu"),
    MenuDuplicateName => ("BR-BOOT-014", "启动菜单出现同名项，请改名以便区分", "The boot menu contains duplicate names; rename them so they can be told apart"),
    PlanNameOrVolumeInvalid => ("BR-BOOT-015", "引导方案名称或卷身份无效", "The boot plan name or volume identity is invalid"),
    PlanDuplicatePartition => ("BR-BOOT-016", "引导方案重复系统分区", "The boot plan contains a duplicate system partition"),
    PlanNotForTask => ("BR-BOOT-017", "引导方案不属于当前还原任务", "The boot plan does not belong to the current restore task"),
    PreRepairBootMgrInvalid => ("BR-BOOT-018", "引导修复前启动管理器无效", "The boot manager is invalid before boot repair"),
    SelectedSystemGone => ("BR-BOOT-019", "引导修复时所选系统已不存在", "A selected system no longer exists during boot repair"),
    PostRepairLoaderMissing => ("BR-BOOT-020", "引导修复后找不到系统加载器", "No system loader was found after boot repair"),
    RepairResultUnreadable => ("BR-BOOT-021", "无法回读引导修复结果", "Cannot read back the boot repair result"),
    RepairOrderMismatch => ("BR-BOOT-022", "引导修复顺序、默认项、休眠引用或显示时间回读不一致", "The boot repair read-back disagrees on order, default entry, hibernation reference or display timeout"),
    RepairMissingSelectedSystem => ("BR-BOOT-023", "回读缺少所选系统", "The read-back is missing a selected system"),
    MenuNameMismatch => ("BR-BOOT-024", "引导菜单名称回读不一致", "The boot menu names do not read back as written"),

    // ── 续跑、故障注入与测试夹具（`recovery_fault` / `resume_safety`）────
    BcdbootPowerCutMissed => ("BR-TSK-006", "BCDBoot 已结束，运行中断电点未命中", "BCDBoot already finished; the mid-run power-cut point was not hit"),
    CannotPauseRunningBcdboot => ("BR-TSK-007", "无法暂停运行中的 BCDBoot", "Cannot suspend a running BCDBoot"),
    SpaceFaultNoTarget => ("BR-TSK-008", "空间故障缺少目标", "The space fault has no target"),
    SpaceFaultNoLetter => ("BR-TSK-009", "空间故障缺少盘符", "The space fault has no drive letter"),
    SpaceFaultOnlySmallNonSystem => ("BR-TSK-010", "空间故障只允许小型非系统测试卷", "The space fault is only allowed on a small non-system test volume"),
    AcceptanceInjectFailed => ("BR-TSK-011", "验收注入错误：{0}", "Acceptance injection error: {0}"),
    ResumeBcdFieldDuplicate => ("BR-TSK-012", "BCD 字段重复，拒绝继续", "Duplicated BCD fields; refusing to continue"),
    ResumeOneShotRequestExists => ("BR-TSK-013", "存在其他一次性启动请求，拒绝覆盖", "Another one-shot boot request exists; refusing to overwrite it"),
    ResumePayloadOutsideStaging => ("BR-TSK-014", "续跑载荷路径不属于任务暂存目录", "The resume payload path is outside the task staging directory"),
    ResumeBootChainFieldMismatch => ("BR-TSK-015", "续跑启动链字段不符：{0}", "Resume boot-chain field mismatch: {0}"),
    ResumePayloadUnsafe => ("BR-TSK-016", "续跑载荷缺失、为空或路径不可安全访问", "The resume payload is missing, empty, or its path cannot be accessed safely"),

    // ── 图形界面（`native_gui`）─────────────────────────────────────────
    ImageIndexInvalid => ("BR-GUI-001", "镜像索引无效", "The image index is invalid"),
    ImageIndexNotPositive => ("BR-GUI-002", "镜像索引必须是正整数", "The image index must be a positive integer"),

    // ── 任务标识、任务文件与启动记录（`main`）───────────────────────────
    TaskIdInvalid => ("BR-TSK-017", "任务标识无效：{0}（{1}）", "Invalid task id: {0} ({1})"),
    TaskFileMissing => ("BR-TSK-018", "任务文件不存在：{0}；recover 的第一个参数必须是**工作区根目录**（包含 tasks 子目录的那一层，例如 H:\\brwork），不是 tasks 目录本身", "The task file does not exist: {0}; the first argument of recover must be the **workspace root** (the level that contains the tasks subdirectory, e.g. H:\\brwork), not the tasks directory itself"),
    SpawnConsoleFailed => ("BR-TSK-019", "无法启动后台命令提示符（CreateProcessW 失败）", "Cannot start the background command prompt (CreateProcessW failed)"),
    ConsoleWindowTimeout => ("BR-TSK-020", "后台命令提示符已启动，但 3 秒内没等到它的窗口（保持最小化，未最大化）", "The background command prompt started but its window did not appear within 3 seconds (leaving it minimized instead of maximizing)"),
    TaskNotRecovered => ("BR-TSK-021", "任务尚未完成恢复，拒绝清理", "The task has not finished recovering; refusing to clean it up"),
    TaskNotResumable => ("BR-TSK-022", "该任务不处于可续跑阶段", "The task is not in a resumable stage"),
    TaskMissingLaunchRecord => ("BR-TSK-023", "任务缺少启动记录", "The task has no launch record"),
    RestoreMissingResumeBoot => ("BR-TSK-024", "系统还原缺少续跑启动记录，拒绝擦除", "The restore has no resume boot record; refusing to erase"),
    ResumePayloadOnEraseTarget => ("BR-TSK-025", "续跑载荷位于擦除目标，拒绝执行", "The resume payload lives on the erase target; refusing to proceed"),
    ResumePayloadVolumeMismatch => ("BR-TSK-026", "续跑载荷卷与本任务镜像卷不符", "The resume payload volume does not match this task's image volume"),
    ProgramTimeout => ("BR-TSK-027", "{0} 超时（上限 {1} ms），进程已终止", "{0} timed out (limit {1} ms); the process was terminated"),

    // ── 卷定位与盘符分配（`main`）───────────────────────────────────────
    MountFallbackNoVolumeId => ("BR-VOL-020", "挂载后备路径缺少有效卷标识符", "The mount fallback path has no valid volume identifier"),
    LiveDiskNumberMissing => ("BR-VOL-021", "现场磁盘编号缺失", "The live disk number is missing"),
    LivePartitionNumberMissing => ("BR-VOL-022", "现场分区编号缺失", "The live partition number is missing"),
    VolumeRecordUnresolvable => ("BR-VOL-023", "无法定位任务记录的卷（卷 GUID {0}，磁盘 {1} 分区 {2}，大小 {3}，记录盘符 {4}）；已尝试：{5}", "Cannot locate the volume recorded by the task (volume GUID {0}, disk {1} partition {2}, size {3}, recorded letter {4}); tried: {5}"),
    VolumeGuidMalformed => ("BR-VOL-024", "卷 GUID 形态无效，无法分配盘符", "The volume GUID format is invalid; cannot assign a drive letter"),
    VolumeExistsNoLetter => ("BR-VOL-025", "卷存在但无法分配盘符", "The volume exists but no drive letter could be assigned"),
    FormatTargetLetterMissing => ("BR-VOL-026", "格式化目标盘符缺失", "The format target drive letter is missing"),

    // ── 主链引导收尾（`main`）───────────────────────────────────────────
    WindowsVolumeMissingPlan => ("BR-BOOT-025", "Windows 系统卷缺少引导修复方案", "A Windows system volume is missing its boot repair plan"),
    BcdbootLoaderMissing => ("BR-BOOT-026", "bcdboot 后找不到本次还原系统的启动加载器", "No boot loader for the system restored this run was found after bcdboot"),
    PostBcdbootBootMgrUnparsable => ("BR-BOOT-027", "BCDBoot 后启动管理器状态无法解析", "Cannot parse the boot manager state after BCDBoot"),
    DefaultSwitchFailed => ("BR-BOOT-028", "无法把默认启动项指向本次还原的系统，拒绝继续后续破坏性步骤", "Cannot point the default boot entry to the system restored this run; refusing to continue with further destructive steps"),
    LoaderMissingRefuseMenu => ("BR-BOOT-029", "主系统或新第二系统加载器缺失，拒绝改写启动菜单", "The primary or new secondary system loader is missing; refusing to rewrite the boot menu"),

    // ── RE 暂存清理与准备阶段补偿（`boot_cleanup` / `windows_prepare`）────
    ReStagingTaskIdInvalid => ("BR-TSK-029", "RE 暂存任务标识不是 UUID：{0}", "RE staging task id is not a UUID: {0}"),
    ReStagingVolumeNoGuid => ("BR-TSK-030", "RE 暂存卷没有有效 GUID", "RE staging volume has no valid GUID"),
    ReStagingCleanupFailed => ("BR-TSK-031", "RE 暂存清理失败（{0}）：{1}", "RE staging cleanup failed at {0}: {1}"),
    ReStagingRootNotDirectory => ("BR-TSK-032", "RE 暂存卷根不是目录", "RE staging volume root is not a directory"),
    ReStagingPathNotPlainDir => ("BR-TSK-033", "RE 暂存路径不是普通目录：{0}", "RE staging path is not a plain directory: {0}"),
    ReStagingStillExists => ("BR-TSK-034", "RE 暂存目录仍然存在：{0}", "RE staging directory still exists: {0}"),
    PreparationCompensationIncomplete => ("BR-TSK-035", "准备阶段补偿未完成：{0}", "Preparation compensation incomplete: {0}"),

    // ── 引导方案（`boot_menu`）─────────────────────────────────────────
    BootMenuTargetLoaderMissing => ("BR-BOOT-030", "缺少必选目标加载器", "Missing required target loader"),
    BootMenuFreezeBlocked => ("BR-BOOT-031", "引导方案在锁定前已记录错误，拒绝继续：{0}", "The boot plan recorded an error before freezing; refusing to continue: {0}"),

    // ── 外部命令失败回执（`main` 的 `run_logged_timeout`）────────────────
    ProgramFailedNamed => ("BR-APL-009", "{0} failed with {1}\n--- 输出尾部（至多 {2} 字节）---\n{3}", "{0} failed with {1}\n--- output tail (at most {2} bytes) ---\n{3}"),
    // **直通**通道的程序失败：子进程输出直接给用户，没有输出尾部可附。
    // 与 `ProgramFailedNamed` 分开是因为模板不同：后者末尾有 `{3}` 这一槽，
    // 直通场景没有文本可传，硬塞空串会在文案与 `--json` 里留下一个裸换行
    // （2026-10-27 实机：`"[BR-PE-039] cmd.exe exit code: 3\n"`）。
    ProgramExitNonZero => ("BR-APL-010", "{0} 失败：{1}", "{0} failed: {1}"),
}

/// 不带占位的错误。
pub(crate) fn errc(code: ErrCode) -> TaskError {
    errc_with(code, &[])
}

/// 带占位的错误：`{0}`、`{1}`… 按序替换。
///
/// 参数在**调用点**先转成 `&str`（`&x.to_string()`、`&format!("{x:?}")`），
/// 因为这里要的是一张纯文本表——把类型信息带进来会让本模块依赖每个调用点的
/// 具体类型，表就不再是「纯数据」了。
pub(crate) fn errc_with(code: ErrCode, args: &[&str]) -> TaskError {
    let template = match crate::i18n::get() {
        crate::i18n::MessageLanguage::Chinese => code.zh(),
        crate::i18n::MessageLanguage::English => code.en(),
    };
    // 单遍填空：参数值里即使含 `{1}` 也不会被二次替换（见 `fill_placeholders`）。
    let rendered = crate::i18n::fill_placeholders(template, args);
    TaskError::Coded {
        code: code.code(),
        rendered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_well_formed() {
        // 码是契约：重复了用户报障就对不上，格式错了脚本 / 文档里 grep 不到。
        let mut seen = std::collections::BTreeSet::new();
        for code in ALL_CODES {
            let id = code.code();
            assert!(seen.insert(id), "错误码重复：{id}");
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(parts.len(), 3, "错误码格式应为 BR-<域>-<序号>：{id}");
            assert_eq!(parts[0], "BR", "错误码前缀必须是 BR：{id}");
            assert_eq!(parts[2].len(), 3, "序号必须是三位：{id}");
            assert!(
                parts[2].chars().all(|c| c.is_ascii_digit()),
                "序号必须全是数字：{id}"
            );
            assert_eq!(code.zh().trim(), code.zh(), "{id} 中文首尾有空白");
            assert_eq!(code.en().trim(), code.en(), "{id} 英文首尾有空白");
        }
    }

    #[test]
    fn placeholders_match_between_languages() {
        // 中英占位数不一致 = 英文文案会漏替换，直接显示字面 `{1}`。
        for code in ALL_CODES {
            let count = |text: &str| {
                let mut n = 0;
                while text.contains(&format!("{{{n}}}")) {
                    n += 1;
                }
                n
            };
            assert_eq!(
                count(code.zh()),
                count(code.en()),
                "{} 中英占位数量不一致：zh={:?} en={:?}",
                code.code(),
                code.zh(),
                code.en()
            );
        }
    }

    #[test]
    fn english_has_no_cjk_and_differs_from_chinese() {
        for code in ALL_CODES {
            assert!(
                !code.en().chars().any(|c| c as u32 > 0x2FFF),
                "{} 的英文里混了 CJK：{}",
                code.code(),
                code.en()
            );
            assert_ne!(
                code.zh(),
                code.en(),
                "{} 中英一样，多半是漏翻",
                code.code()
            );
        }
    }

    #[test]
    fn display_carries_the_code_prefix() {
        // 前缀是给人 grep 的：日志 / status.json 里按 `[BR-` 就能捞出所有带码错误。
        let error = errc(ErrCode::VolumeChanged);
        assert_eq!(
            error.to_string(),
            "[BR-PE-004] 卷身份、现场编号或几何已变化，拒绝写入"
        );
    }

    #[test]
    fn placeholders_are_filled_in_order() {
        let error = errc_with(ErrCode::StateWriteFailed, &["外层失败", "落盘也失败"]);
        assert_eq!(
            error.to_string(),
            "[BR-PE-018] 外层失败；失败状态落盘也失败：落盘也失败"
        );
    }
}
