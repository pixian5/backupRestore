# 压缩选项收敛（2026-09-26）

## 当前产品行为

- GUI 的备份压缩下拉只保留两个选项：`压缩`、`不压缩`。
- `压缩` 固定映射到 DISM `/Compress:fast`，作为默认选择。
- `不压缩` 固定映射到 DISM `/Compress:none`。
- `max` 最大压缩功能已删除：GUI 不再显示 LZX/max，CLI 不接受 `--compress max`，旧任务文件中的 `compress: "max"` 会在任务校验时失败，执行链绝不会把它传给 DISM。
- 中英文切换后仍只有两个压缩选项，索引保持不变。

历史压缩耗时和体积对比记录保留在 `winre-autostart-pitfalls.md` 中，仅作为曾经做过性能对比的证据，不再代表当前可选功能。

## 测试边界

- 已加入跨平台测试，覆盖 `fast`/`none` 接受、`max`/未知值拒绝，以及 GUI 两项索引到 DISM 术语的映射。
- 实机验收必须分别选择 `压缩`、`不压缩`，核对界面仅有两项、语言切换不变，并在测试卷备份日志或命令中确认 `/Compress:fast` 与 `/Compress:none`。
- 测试阶段只允许使用独立测试卷，禁止把 C: 作为备份源或还原目标。

## 实机验收证据（2026-09-27，Windows 11 ARM64 虚拟机）

环境：`BackupRestore - Rust GUI v1.7.4`（窗口标题实测），活动包
`C:\Users\Public\backupRestore-package\BackupRestore.exe`（1688064 字节，SHA-256
`C0DDDFD9A9C71542C28711B996F68CAEA96A51F7455EDCBC55170C8170631ACF`）。
测试卷 `T:`（卷标 `BRTEST174`，NTFS，磁盘 0/分区 5）。本轮全程未把 C: 作为备份源或还原目标。

### 界面：压缩下拉只有两项

用同一交互会话、高完整性进程通过 Win32 消息读取真实 ComboBox（非截图判断）：

| 语言 | LANGUAGE | COMPRESS count | COMPRESS items | 索引映射 |
| --- | --- | --- | --- | --- |
| 中文 | `count=2 cursel=0 [中文 \| English]` | 2 | `[压缩 \| 不压缩]` | 0=压缩、1=不压缩 |
| English | `count=2 cursel=1 [中文 \| English]` | 2 | `[Compression \| No compression]` | 索引不变，0/1 语义不变 |

- 中文、英文切换前后均为两项，无 `max`、无 LZX、无第三项；切换后已选索引保持不变。
- 下拉实际展开截图（中/英）与控件读取结果一致。
- 证据：`.test-artifacts/v174-ui-t-backup-restore/2026-09-27/screens/04-compress-dropdown-zh.png`、
  `05-compress-dropdown-en.png`、`evidence/combo-apply-*.txt`、`evidence/guistate-*.txt`。

### 备份：`/Compress:fast` 与 `/Compress:none` 均已实测

两次备份都新建了独立 WIM（避免 `/Append-Image` 沿用首次压缩设置导致误判）：

| 选项 | 镜像 | 大小（字节） | SHA-256 | 索引 | DISM 权威命令 |
| --- | --- | --- | --- | --- | --- |
| 压缩 | `test-fast.wim` | 367989559 | `538F340FC181CDEF221D90F42C5388A0B128B7012E53E216CC713EB852279BCD` | 1 | `dism.exe /Capture-Image /ImageFile:"...\test-fast.wim" /CaptureDir:T:\ /Name:"2026-09-27 01:17" /Compress:fast` |
| 不压缩 | `test-none.wim` | 422998699 | `124345584F22CE8BF9E16389EA1A3EA3F869B3F385919FB23F7D56077E3E8313` | 1 | `dism.exe /Capture-Image /ImageFile:"...\test-none.wim" /CaptureDir:T:\ /Name:"2026-09-27 01:37" /Compress:none` |

- 命令取自 `C:\Windows\Logs\DISM\dism.log`；两次均为 `success=true` 且 `br-online-op.txt` 到 100%。
- 不压缩产物明显大于压缩产物，与两种压缩语义一致。
- WIM 均落在 `C:\Users\Public\br-test\v174-20260926\`，未写入 T:（不污染被备份源）。

### 还原：本轮未完成，不能视为已验证

- 单系统还原（目标 T:）执行的是覆盖式 `dism /Apply-Image /ApplyDir:T:\`，不删除镜像外文件。
- 实测还原在 72% 处失败：`CreateDestinationFileEx` 对 `T:\Mac disk` 报 `0x80070020`
  （ERROR_SHARING_VIOLATION）。该 0 字节文件被 Parallels Tools 独占持有
  （`prl_cc`/`prl_tools`/`prl_tools_service` 在运行；用任意共享模式、任意读写权限打开均失败，
  且它不是重解析点）。用户要求不主动删除该文件，因此还原闭环暂停，等待处置决定。
- 失败前已写入的部分内容经核对是备份时的状态（标记文件与 data3/data4 均回到基线哈希），
  但这不是一次完整、干净的还原，**不得据此宣称还原已验证**。
- C: 的完整系统备份/还原本轮完全未测试、未验证。
