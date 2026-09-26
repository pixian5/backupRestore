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

- 单系统还原（目标 T:）执行的是覆盖式 `dism /Apply-Image /ApplyDir:T:\`，不删除镜像外文件
  （该语义已由用户裁定接受，见下「还原语义：已决策」）。
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

### 还原语义：已决策（2026-09-27，用户裁定）

用户裁定**接受覆盖式语义**，即 `dism /Apply-Image /ApplyDir:<target>` 只写入镜像内文件、
不删除镜像外文件。验收标准据此调整：

| 破坏动作 | 期望结果（覆盖式） |
| --- | --- |
| 修改 `BRTEST174-20260926-before.txt` | 恢复为基线 SHA-256 `0C356FD7…A44E` |
| 删除/改名 `data4.bin` | 被重新写回，SHA-256 `8565A714…2DA2` |
| 新增 `BRTEST174-20260926-after.txt` | **残留、不被清除**（预期行为，不计为失败） |

原定的「`after.txt` 被清除」一条**作废**，不再作为失败判据。此决策不改动产品代码，
`VERSION` 保持 1.7.4。

### 解除阻塞所需的授权（仍待用户决定）

因该文件被完全独占，唯一可行路径是**先解除 `Parallels Tools Service` 对它的持有**（例如临时
stop 该服务并在还原完成后 start），或改用不含该占位文件的卷作为还原目标。两种做法都会改变
VM 的当前状态，故本轮未擅自执行。

> 本节结论已被 2026-09-27 的 v1.7.5 修复取代：**不再需要停服务**，直接从产品层排除该占位
> 文件即可。详见下方「v1.7.5 解阻塞与闭环完成」。

## v1.7.5 解阻塞与闭环完成（2026-09-27）

用户裁定「把排除环境噪声做成产品功能」后，v1.7.5 完成修复与实机闭环，上一节的解除阻塞授权
需求与 72% 失败一并作废。

- **产品改动**：`\Mac disk` 加入固定排除表首项；新增共享 helper
  `write_capture_exclusion_config`，把 CLI 备份/追加、GUI 在线备份（`execute_online`，原缺陷点）、
  PE 备份三条路径全部收敛到同一份 `/ConfigFile`。`VERSION` 与两个 `Cargo.toml` 升为 `1.7.5`，
  产物 1,689,088 字节 SHA-256 `f8b77d0e…e184`，窗口标题实测 `BackupRestore - Rust GUI v1.7.5`。
- **关键实测事实**：`[ExclusionList]` **对被独占文件同样生效**（DISM 在打开文件之前就跳过），
  且匹配**大小写不敏感**。所以排除即等价于解除了该锁对还原的阻塞——不必 stop 服务。
- **本轮只跑 `/Compress:none`**（按用户指令，不再同时跑 fast）。
  - 备份：`op=backup source=T target=C`，`exit=0`，`test-none.wim` 527,855,975 字节，
    只读挂载后根目录 `ABSENT Mac disk`，`keep-marker.txt`/`petest`/`data1..4.bin` 齐备。
  - 还原：损伤 T: 后走 `restore-existing`→target T:→Index 1，**服务保持 `Running`、
    `T:\Mac disk` 仍被独占**（`locked=True`）的情况下 `exit=0`；`data3.bin`、`data4.bin` 恢复，
    marker 回到基线 `632DBB2E…72DE`。**这是本节最关键的断言：不停服也能还原成功。**
  - 覆盖式语义（见「还原语义：已决策」）继续成立：`after.txt`、`data4.bin.renamed` 残留属预期。
- 证据：`.test-artifacts/v175-ui-t-backup-restore/2026-09-27/evidence/`。

> 上文中“`VERSION` 保持 1.7.4”仅描述当时「还原语义决策本身不改代码」这一事实；v1.7.5 的版本
> 提升来自随后的排除功能修复，两者不矛盾。

## 启动入口核验（2026-09-27，只读）

只读审计（`evidence/entry-audit.txt`）。活动入口全部为 v1.7.4 且哈希一致：

| 入口 | 路径 | 大小 | SHA-256 | 版本/标题 |
| --- | --- | --- | --- | --- |
| 运行进程 pid=6996 | `C:\Users\Public\backupRestore-package\BackupRestore.exe` | 1688064 | `C0DDDFD9…1ACF` | 窗口标题 `BackupRestore - Rust GUI v1.7.4` |
| 同目录 | `C:\Users\Public\backupRestore-package\Recovery.exe` | 1688064 | `C0DDDFD9…1ACF` | mtime 2026-09-26 23:50:43 |
| 桌面快捷方式 | `C:\Users\x\Desktop\BackupRestore.lnk` | — | — | 指向上述 exe |
| HKCU Run | `BackupRestoreTest` | — | — | 指向上述 exe |
| 计划任务 `BRLaunch`（Disabled） | 同上 | — | — | 参数 `--tab 1` |

**发现：仍存在旧版本载荷（本轮未被使用）**

- **WinRE 载荷不是 v1.7.4**：`C:\Recovery\WindowsRE\Winre.wim`
  （712108371B，SHA `DBBCE2BD…`，mtime 2026-09-26 23:02:00）内
  `\Windows\System32\Recovery.exe` = 1687040B，SHA `653ABC68…`，mtime 2026-09-26 08:58:08。
  该哈希与 `.test-artifacts/v173-20260926/prepare-gui-re.ps1` 中的 `$expectedExe` 完全相同，
  即实际为 **v1.7.3** 构建。
- **PE 载荷（T: 上的遗留副本）更旧**：`T:\petest\boot.wim`（367395379B，SHA `D5F1515A…`，
  2026-09-13 遗留）内 `\Windows\System32\BackupRestore.exe` 与 `Recovery.exe` 均为 1026560B，
  SHA `22E6829B…`，mtime 2026-08-25 23:38:13；未在历史构建产物中匹配到版本号。
- 非入口旧副本（不在启动路径）：`backupRestore-package\tasks_hold2\payload\Recovery.exe`
  （1511424，`A06144…`）、`C:\Users\Public\BR-Recheck-20260925\Recovery.exe`（1624576，`648590…`）、
  `C:\BackupRestoreBuild\package\BackupRestore-windows-arm64-v1.5.12\Recovery.exe`（1511424，`A06144…`）。
- `BR-GUI` / `BR-GUI2`（Disabled）指向不存在的 `C:\Users\Public\backupRestore-package-v12`。

**需注意的遗留计划任务（本轮均未触发）**

- `BRPREP` / `BRPREP2`（Ready）：`BackupRestore.exe prepare --operation backup --source-drive C
  --target-drive C --image-path E:\br-cdrive-v1.wim --compress fast` —— 参数与 C: 相关。
- `BRGuiSetText`（Ready）：会向 GUI 控件 1203 写入 `H:\Images\CurrentEfiV131.wim`。

证据：`evidence/pe-payload-winre.txt`、`evidence/pe-payload-t-petest.txt`。

本轮 T: 走**在线**路径（`affected=T`、`system=C` 不相等），不重启、不进 WinRE/PE，因此上述旧载荷
本轮未被使用。但若后续改走 WinRE/PE 恢复链，实际执行的仍是 v1.7.3（或更早）的二进制。
更新 WinRE/PE 载荷属于「修改 WinRE/PE 部署」，按项目约束需先创建并核验新快照并获授权，本轮未执行。

WinRE 只读状态：`reagentc /info` 为 Enabled，location
`\\?\GLOBALROOT\device\harddisk0\partition4\Recovery\WindowsRE`；
`C:\Windows\System32\Recovery\Winre.wim` 不存在。

## 本轮范围、未做项与自行披露

- C: 从未作为备份源或还原目标。**C: 的完整系统备份/还原本轮完全未测试、未验证。**
- 未修改 BCD、WinRE、默认启动项、bootsequence；未创建/删除快照；未重启 VM；未停止任何 Parallels 服务。
- 自行披露：在压缩机制早期排查中，曾手工执行两条**非产品**的 DISM 命令：
  `dism /Capture-Image /ImageFile:…\test-fast.wima … /Compress:fast`（文件名拼写错误的一次尝试）
  与 `dism /Capture-Image /ImageFile:…\manual-test.wim /CaptureDir:C:\Users\Public\pkg … /Compress:none`
  （捕获的是一个 C: 上的目录，不是 C: 卷，也不是产品备份路径）。正式结论只采用产品 GUI 创建任务后
  `dism.log` 记录的 `/Compress:fast`、`/Compress:none` 两次命令。
- 因 GUI 还原受阻，还原闭环未完成。该项验收标准已按用户裁定改为覆盖式语义（见「还原语义：已决策」）：
  `after.txt` 残留属预期行为，不作为失败判据；仍需在闭环中验证的是镜像内文件全部恢复且哈希匹配。
- 还原闭环仍被 `T:\Mac disk` 独占锁阻塞，等待解除该锁的授权。
