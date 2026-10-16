# V2 重跑阻塞根因与 RE 内 apply 失败全记录（2026-10-16）

> 本轮为用户指令「继续推进 V1/V2/V4/V5/V6 按新 default 规则重跑、遇问题自行分析推进」的实测记录。
> 结论：**V2 用例在本机环境下有两道独立的确定性阻塞**，均已定位到根因、现场已收敛、C: 完好无损。
> 两处都不是「新 default 规则」的问题——新规则经 V1 已实机通过（`docs/20261015-0105`/本轮 V1）。

## 0. TL;DR（给赶时间的人）

| # | 现象 | 根因 | 性质 | 现状 |
|---|---|---|---|---|
| **A** | 提权管理员上下文里 `reagentc /info` **恒定 exit 9**（`WinReGetWimVersion failed to initialize DISM error 0xc0040009`），导致 V2 准备链 314ms 内失败 | `C:\Windows\Logs\DISM` 目录的 ACL **不给 Administrators 写权限**，DISM 建/写自己日志失败 → 初始化 0xc0040009。SYSTEM 上下文因 ACE 里带 `SYSTEM:(F)` 一直正常 | **环境/系统层**（非本产品代码 bug，但本产品对 plain `/info` 无容忍会放大它） | 已授权修复现场；已用「撤销授权→复现 / 重授权→恢复」双向实验钉死 |
| **B** | 准备链通过后，RE 内 `dism /Apply-Image /ApplyDir:C:\` **立即 exit 5 Access is denied**（2 秒返回），失败后 RE 反复重武装 → **重启死循环回 RE** | **覆盖式应用要改写 C: 上 `C:\Program Files\WindowsApps\...` 的目录元数据（owner/ACL），该目录受 TrustedInstaller 保护 → `RestoreDirMetadata` 被拒 `0x80070005`**。PE 内对照实验钉死：同环境同镜像，**F: 100% 成功、C: 秒退**；且 `md`（建目录）允许、只有"改写既有受保护目录元数据"被拒 | **产品行为边界**（skip-format + 系统卷 组合此前从未实机验证；`20261014-2345` 那次成功的是**数据卷 T:**） | RE 内清 `bootsequence` 逃出；C: 完好（哨兵存活、序列号未变） |

**关键安全事实**：V2 全程 **C: 一个字节都没被改写**——哨兵 `BR-102220.txt` 仍在、卷序列号仍 `5C93-0047`（覆盖式应用失败在 apply 起步阶段，未格式化）。

---

## 1. 问题 A：提权上下文 `reagentc /info` 恒定 exit 9

### 1.1 现象与影响

V2 走「还原系统到 C 盘」离线入口，准备链在 GUI 日志里 314ms 内报：

```
elevated prepare failed early: exit_code=1 error=REAGENTC.EXE: 操作失败
launcher-errors.log: reagentc.exe failed with exit code: 9
```

而**同一台机器、同一份 `reagentc.exe`**（SHA256 一致）在 SYSTEM 上下文（`prlctl exec`）跑 `reagentc /info` 一直是 **exit 0 成功**。

### 1.2 完整对照矩阵（实测）

| 上下文 | 身份 | Session | 完整性 | `reagentc /info` |
|---|---|---|---|---|
| `prlctl exec` | `nt authority\system` | 0 | System | **exit 0** ✅ |
| br-s1.sh 提权链 | `p8b6\x` | 1 | High | **exit 9** ❌ |

`reagentc` 自身诊断日志（`/logpath` 指定）在失败上下文里给出精确原因：

```
Enter WinReGetWimVersion
WIM file: \\?\GLOBALROOT\device\harddisk0\partition4\Recovery\WindowsRE\winre.wim
Error  [ReAgentc.exe] WinReGetWimVersion failed to initialize DISM error 0xc0040009
Error  [ReAgentc.exe] failed to get winre version: 0x9
```

即：`/info` 内部要用 DISM 读已注册 Winre.wim 的版本，**DISM 初始化失败**。

### 1.3 根因定位（双向对照实验）

排除了 WIM 缺失、staged 状态、cwd、独立 DISM 故障、二进制版本错配之后，注意到失败上下文里 DISM 自己吐的线索：

```
Logging is disabled: Unable to obtain access to the log file C:\Windows\Logs\DISM\dism.log.
```

于是专门验证 ACL：

| 检查项 | SYSTEM 上下文 | 提权 admin 上下文 |
|---|---|---|
| 写 `C:\Windows\Logs\DISM\dism.log` | OK | **DENIED** |
| 在 `C:\Windows\Logs` 下建子目录 | OK | **DENIED** |
| 在 `C:\Windows\Logs\DISM` 下建子目录 | OK | OK（有显式 `Administrators:(OI)(CI)(F)` 时） |

`icacls` 实证：

```
C:\Windows\Logs\DISM       BUILTIN\Administrators:(F)            ← 非继承、不传子项
                           NT AUTHORITY\SYSTEM:(OI)(CI)(F)
                           BUILTIN\Users:(RX)
C:\Windows\Logs\DISM\dism.log   NT AUTHORITY\SYSTEM:(I)(F)        ← 只有 SYSTEM + Users 读
                                BUILTIN\Users:(I)(RX)
```

**关键：`dism.log` 文件级 ACL 只有 `SYSTEM:(I)(F)` + `Users:(I)(RX)`，没有 Administrators 写权限。** 当这个文件/目录由 SYSTEM 身份先建好，提权 admin 之后就再也写不进去。

### 1.4 决定性实验（`br-acl-test.ps1` / `br-acl-revert.ps1`）

在提权上下文（正是失败的那个上下文）里跑：

| 步骤 | 动作 | `reagentc /info` 结果 |
|---|---|---|
| 0 | 基线 | **exit 9** |
| 1 | 给 `C:\Windows\Logs\DISM` 递归授 `Administrators:(OI)(CI)F` | **exit 0** ✅（立即成功） |
| 2 | `icacls /remove:g` 撤销该授权 | **exit 9**（复现） |
| 3 | 再次授权 | **exit 0** |

**授权 → 成功、撤权 → 复现、再授权 → 又成功**：根因钉死为 `C:\Windows\Logs\DISM` 的写权限。

### 1.5 触发条件与「为什么 V1 好、V2 坏」

- `C:\Windows\Logs` **不在镜像里**（已挂载镜像 `Windows\` 下无 `Logs` 目录，实测 `dir /a` 无匹配）——它是系统**运行时**创建的。
- 本机 `C:\Windows\Logs\DISM` 的创建时间是 **2026-10-16 10:38:54**，比 `C:\Windows\Logs`（10:01:24，还原后首启）晚——即**还原到新系统后，由某次 SYSTEM 身份的 DISM 先建了它**，于是后续提权 admin 全部写不进去。
- V1 的准备链跑在**还原前的旧系统**上（那时该目录尚可写）；V2 的准备链跑在**V1 还原出来的新系统**上 → 命中。

### 1.6 处置与产品层建议

- **现场已修复**：对 `C:\Windows\Logs\DISM` 授权 `Administrators:(OI)(CI)F`（实测 `/info` 恢复、V2 准备链随后成功启动，不再 314ms 早退）。
- **产品层脆弱点**：`windows_prepare.rs:250`（`inspect_environment`）、`:696`（`recovery_identity`）、`:2431`（`assert_environment`）三处 `capture("reagentc.exe", &["/info"])` **只要退出码非零就整体失败**，对这个「DISM 写不了自己日志」的上下文级问题零容忍。
  - 建议（待用户拍板）：`recovery_identity()` 在 `/info` 失败时**不硬失败**，退化为「从 BCD/卷信息推 WinRE 位置」或记警告继续——因为该函数的产出仅用于「工作区/镜像卷不得与恢复分区同卷」的安全检查，取不到时后续 WIM 挂载/注入会自行报错。这与 `docs/20261016-0014` 的 P5 降级是同一类思路。

---

## 2. 问题 B：RE 内 `dism /Apply-Image` 到 C: 立即 exit 5

### 2.1 现象

准备链这次**成功**（无 early-exit），VM 重启进 RE。RE 起步后弹出产品原生错误框：

```
恢复暂未完成：invalid task: dism.exe failed with exit code: 5
```

`H:\BRImages\Recovery.log` 主日志铁证：

```
[05:34:04] new boot channel: one-shot bootsequence re-armed for resumption
[05:34:04] 按任务选项跳过格式化（覆盖式应用）：目标卷上镜像没有的文件将保留
[05:34:04] STEP 2/4 还原镜像（时间长）
[05:34:04] running dism.exe /English /Apply-Image /ImageFile:E:\BRImages\c-full-20261014.wim /Index:2 /ApplyDir:C:\
[stdout] Deployment Image Servicing and Management tool
[stdout] Version: 10.0.26100.1
[stdout] Error: 5
[stdout] Access is denied.
[stdout] The DISM log file can be found at X:\windows\Logs\DISM\dism.log
[05:34:06] Recovery retry required at TargetErased: invalid task: dism.exe failed with exit code: 5; boot entry and payload retained
```

- **`/ApplyDir:C:\`** → 立刻（约 2 秒内）`Error 5 Access is denied`。
- 任务状态 `status.json`：`stage=target-erased, progress=35, errorCode=1, error=invalid task: dism.exe failed with exit code: 5`。
- 13:47 第二次点「重试」**完全相同的失败**——**确定性失败**。

### 2.2 根因**已实测钉死**（2026-10-16 16:30-17:04，PE 内对照实验）

> ⚠️ 本节此前写的「覆盖式应用撞上 pagefile/hiberfil/$MFT 等不可改写对象」是**未经证实的推测**。
> 后在 PE 控制台里做了完整对照实验，真因比那条推测**精确得多**，以本节为准。

**实验（全部在同一 PE、同一镜像、同一 DISM `10.0.28000.1` 下）**：

| # | 目标 | 命令 | 结果 |
|---|---|---|---|
| 1 | **C:**（已有完整旧系统） | `dism /Apply-Image ... /ApplyDir:C:\` | **秒退 `Error 5 Access is denied`**（复现 RE 内现象） |
| 2 | **F:**（此前已写过一份系统） | 同一条命令 `/ApplyDir:F:\` | **100% 成功**（`The operation completed successfully.`） |

变量**只剩目标卷**——不是 DISM 版本、不是镜像、不是 PE 环境。

**DISM 自身日志（`X:\windows\Logs\DISM\dism.log`，PE 内可读）给出精确到函数名的失败点**：

```
Warning  Directory metadata: C:\Program Files\WindowsApps\Microsoft.DesktopAppInstaller_1.29.289.0_arm64__8wekyb3d8bbwe (HRESULT=0x80070005)
Error    RestoreDirMetadata:(2418): Access is denied.
Error    WIMCreateDirectory:(3216): Access is denied.
Error    RestoreDirTree:(3468): Access is denied.
Error    WIMApplyImageInternal:(939): Access is denied.
```

**精确根因**：失败发生在 **`RestoreDirMetadata`**——DISM 覆盖式应用时，要把镜像里 `C:\Program Files\WindowsApps\...`（MSIX 应用包目录）的**目录元数据 / Security Descriptor / owner 还原到卷上**。该目录的 owner 是 `NT SERVICE\TrustedInstaller`、带 MSIX 专用 ACL（app-package SID、`TrustedInstaller:(I)(F)`），**操作系统拒绝对这类受保护目录改写 owner/ACL** → `0x80070005` → DISM **立即整体放弃**（不继续）。

**与"数据写入"无关的旁证**（PE 内实测）：
- 在 `C:\Program Files\WindowsApps\` 下 **`md` 建新目录成功**（`dir` 可见）、**可正常 `rd` 删除** → 普通数据写入是允许的；
- 被拒的**只有"改写既有受保护目录的元数据"这一步**。
- F: 上同样路径的 `WindowsApps\...`（DISM 刚写入的）**没有**这层 TrustedInstaller 保护，所以通过。

**结论**：
1. **"跳过格式化 + 还原到系统卷 C:" 这条路在本机确定性走不通**——根因是 C: 上既有 Windows 自带 `Program Files\WindowsApps` 等 TrustedInstaller 保护的目录，DISM 覆盖式应用**必然**在还原它们的目录元数据时被拒。
2. 与 **RE / 在线 / PE 环境无关**：PE 里同样失败（此前误以为"RE 特有问题"）。
3. 与 **DISM 版本无关**：PE 用 `10.0.28000.1` 照样失败。
4. `--skip-format` **对数据卷（无 Windows 的卷）有效**（`20261014-2345` 已验证），但**对已含 Windows 的系统卷无效**。
5. **格式化能通过**是因为：格式化把 C: 清空，`WindowsApps` 等受保护目录一并消失，DISM 只需**新建**目录（`md` 允许）而非**改写**既有受保护目录的元数据 → V1（先格式化）成功。

### 2.3 副作用：RE 失败 → 重启死循环（严重）

`main.rs` 的 `recover_env_interactive()` 逻辑（约 858-873 行）：
- `Err` → 弹 `MessageBoxW`（RETRYCANCEL）→ 选「重试」回到循环重跑同一任务。
- 但失败**发生在 `rearm()` 之后**（`main.rs:2607` 在擦除前重武装一次性启动项），所以每次「取消/Esc」退出载荷后，**`bootsequence` 仍指向 RE**，重启又回 RE、又失败 → **确定性死循环**。
- 实测：`Esc` / 「取消」各试一次，每次都退回同一错误框；三次截图像素级一致。

### 2.4 现场救援（按项目既有流程）

RE 内 `prlctl exec` 不可用（WinRE 无 Parallels Tools，已知限制）。走 `docs/20260916-0759-winre-bcd-loop-fix.md` 的 PE 救援通道：

1. 停 VM → 挂 `artifacts/BaseWinPE.iso` 到 cdrom0 → **UEFI 里手选光驱启动**（见 §3 教训）。
2. PE 内 `diskpart`：确认卷布局 → `select volume 4`（ESP 300MB）→ `assign letter=S`。
3. `copy S:\EFI\Microsoft\Boot\BCD S:\EFI\Microsoft\Boot\BCD.bak-1016`（回滚材料）。
4. `bcdedit /store S:\EFI\Microsoft\Boot\BCD /enum {bootmgr}` → 确认元凶 `bootsequence {7c347ac7-…}`。
5. `bcdedit /store S:\EFI\Microsoft\Boot\BCD /deletevalue {bootmgr} bootsequence` → 回读确认已清。
6. `diskpart` → `select volume 4` → `remove letter=S`（收敛现场）→ `exit`。
7. 停 VM → `--device-bootorder "hdd0"` → 启动 → **30 秒回桌面**（GUI 正常、C: 249.5 GiB/177.8 GiB 可用）。

### 2.5 现场收敛核验（全部通过）

- 哨兵 `C:\BR-102220.txt` **仍在**（覆盖式应用没动 C:）。
- `vol C:` 序列号 **`5C93-0047` 未变**。
- C: 上旧系统完好，BackupRestore GUI v2.6.5 正常自启。

### 2.6 产品层建议

1. **准备阶段就要拦住**：`validate_operation_inputs`（`windows_prepare.rs:2028`）目前对 `restore-existing + skip-format`（即 `format_target=false` 且目标是系统卷）**无任何校验**。建议**准备期硬拒**「系统卷 + 取消格式化」组合——就像 P0 安全闸拦「create-secondary 指向活动系统卷」一样，把不可能成功的组合挡在擦除/重启之前，而不是等 RE 里失败再把用户困在死循环。这与「宁可拒绝、不可白做」的项目一贯取向一致。
2. **死循环必须自动收口**：`recover_env_interactive` 的 RETRYCANCEL 循环在**确定性失败**下会无限困住机器。建议加「同一任务连续失败 N 次则自动清理 `bootsequence` 并重启回正常系统、结束任务」，呼应待办里的「非终态任务自动收口」。

---

## 3. 操作教训（本轮犯错，务必记）

1. **改启动顺序前必须先截图判断当前界面在什么状态。** 本轮我在 **UEFI 固件界面**时误当成“已在『按任意键从 DVD 启动』那一步”，对着固件界面猛发空格，白白浪费时间。**先 capture 看屏、再决定动作**。
2. **`--device-bootorder` 对 UEFI 不必然生效**：`"cdrom0 hdd0"`（双设备）实测无效、`config.pvs` 的 BootingOrder 对 UEFI 无效；本轮最终是**在 UEFI 里手选光驱**才进 PE。不要把「命令行设了顺序」当成「已经进了 CD」。
3. **`type_text.sh` 不支持 `\n`**：把 `"diskpart\nlist volume"` 一次性打字，换行被丢弃 → 屏幕上成了 `diskpartlist volume`。**一条命令一次执行**：先 `type_text.sh "<cmd>"`，再单独 `./vmkey.sh enter`。
4. **RE 内 `prlctl exec` 不可用**（WinRE 无 Parallels Tools）——取证只能读磁盘上的持久日志（`C:\BackupRestore-Recovery-early.log`、镜像卷 `Recovery.log`、任务目录），键盘注入用 `vmkey.sh` / `type_text.sh`。
5. **`Esc` 会取消 RE 的错误框但不会清 `bootsequence`**——取消后载荷重跑、又失败，需要 PE 通道从外部清。

---

## 4. 待用户拍板 / 后续

| 项 | 内容 | 建议 |
|---|---|---|
| B1 | 「系统卷 + 取消格式化（skip-format）」是否在准备期硬拒 | **建议硬拒**（挡在重启前，避免死循环） |
| B2 | `recover_env_interactive` 确定性失败是否自动收口（清 bootsequence + 回系统 + 结任务） | **建议做**（并入「非终态任务自动收口」待办） |
| A1 | `recovery_identity()` 对 `/info` exit≠0 是否降级 | **建议降级为警告**（同 P5 思路；产出仅用于同卷安全检查） |
| — | V2 用例本机还能不能跑通 | 硬拒落地后，「A 入口跳过格式化」在本机**可能本就不可行**；需据 B1 拍板决定 V2 的验收口径（改为「应被明确拒绝」而非「应成功」） |

---

## 5. 复用脚本（本轮新增，均 `tools/win-clicker/`）

| 脚本 | 用途 |
|---|---|
| `br-ctx-matrix.ps1` | 上下文矩阵探针（身份/会话/完整性/版本/ACL/`reagentc /info`/DISM），一次跑清「哪个上下文出问题」 |
| `br-ctx-dismlog.ps1` | 位宽 + 文件版本 + DISM 日志 ACL/可写性 + UNC 直跑 reagentc |
| `br-acl-test.ps1` / `br-acl-revert.ps1` | **根因双向对照实验**（授权→成功 / 撤权→复现） |
| `br-choose-re2.ps1` | 用 EnumWindows 点「进入 Windows RE」（`FindWindowW` 在提权上下文返回 0，不可用） |
| `br-click-dlg-yes.ps1` | 点产品原生确认框的「是(&Y)」（id=6） |
| `br-check-read.ps1` | 只读回读复选框状态（不触发点击） |