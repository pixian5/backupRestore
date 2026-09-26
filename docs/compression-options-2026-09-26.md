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

### 备份保真度：WIM 只读挂载核对（替代性证据）

GUI 还原受环境阻塞（见下节），因此另用 `dism /Mount-Image /ReadOnly` 只读挂载两个 WIM 的索引 1，
直接读取镜像内文件并比对哈希。这一步不写 T:，也不触发任何还原事务：

| 镜像 | 索引 1 名称 | 镜像内 `before.txt` | 镜像内 `data1..4.bin` | 镜像内 `keep-marker.txt` | `after.txt` / `data4.bin.renamed` |
| --- | --- | --- | --- | --- | --- |
| `test-fast.wim` | `2026-09-27 01:17` | 99B `0C356FD7…A44E` | 各 52428800B `8565A714…2DA2` | 13B `7A023DFB…B905` | 均不存在 |
| `test-none.wim` | `2026-09-27 01:37` | 99B `0C356FD7…A44E` | 各 52428800B `8565A714…2DA2` | 13B `7A023DFB…B905` | 均不存在 |

两镜像的索引数与 `Size`（580,281,306）一致，内容哈希与 T: 基线清单逐一相同，
说明**备份链路在数据层面是忠实、可校验的**；挂载后均已 `/Unmount-Image /Discard`，
`dism /Get-MountedImageInfo` 复查为 `No mounted images found`。
证据：`evidence/wim-mount-fast.txt`、`evidence/wim-mount-none.txt`。

### 还原：本轮未完成，被环境阻塞，不能视为已验证

- 单系统还原（目标 T:）执行的是覆盖式 `dism /Apply-Image /ApplyDir:T:\`，不删除镜像外文件。
- 实测还原在 72% 处失败：`CreateDestinationFileEx:(3804) -> CreateFile failed T:\Mac disk`
  `HRESULT=0x80070020`（ERROR_SHARING_VIOLATION）。
- 根因已定位（Restart Manager + 服务/进程查询，均为只读）：
  - `T:\Mac disk` 是 Parallels SmartMount 的占位产物，由 `prl_tools_service.exe`
    （pid 4116，session 0）在服务启动时创建并独占持有；Restart Manager `RmGetList` 报告的唯一
    持有者即 `Parallels Tools Service`（`restartable=True`）。
  - 文件创建时间 `2026-09-26 23:44:10` 与该服务进程启动时间完全相同。
  - 同一时刻 `E:`(105,277,091,840B)、`H:`(53,160,550,400B)、`P:`(0B)、`T:`(0B) 四个数据卷根目录
    同时出现同名占位文件，`C:` 与 `Y:` 没有 —— 说明它是 Parallels 生成的多卷占位，不是用户数据。
  - 6 种 (ReadWrite/Write/Read × None/ReadWrite) 打开组合全部失败，故在锁被持有期间
    连改名和删除都不可行。
  - 该占位文件同时存在于两个 WIM 内（0B，SHA-256 `E3B0C442…`），因此 `Apply-Image` 必然尝试创建它。
- 同一失败并非本轮引入：`dism.log` 中 `2026-09-13 23:27:25`/`23:27:45` 已有两条完全相同的
  `T:\Mac disk (HRESULT=0x80070020)` 记录（PID=1228、PID=4720），属长期存在的环境问题，
  与本轮产品代码无关。
- 失败前已写入的部分内容经核对是备份时的状态（标记文件与 data3/data4 均回到基线哈希），
  但这不是一次完整、干净的还原，**不得据此宣称还原已验证**。
- 本轮未重启 VM、未修改 BCD/WinRE/启动项、未删除任何快照、未停止任何 Parallels 服务，
  也未删除 `Mac disk`（用户要求不主动删除）。还原闭环因此暂停，等待授权处置。
- C: 的完整系统备份/还原本轮完全未测试、未验证。

### 解除阻塞所需的授权（待用户决定）

因该文件被完全独占，唯一可行路径是**先解除 `Parallels Tools Service` 对它的持有**（例如临时
stop 该服务并在还原完成后 start），或改用不含该占位文件的卷作为还原目标。两种做法都会改变
VM 的当前状态，故本轮未擅自执行。
