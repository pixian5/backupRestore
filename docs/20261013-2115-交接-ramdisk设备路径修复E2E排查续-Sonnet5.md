# 交接：ramdisk 设备路径修复——单变量实验已定因，产品 E2E 两启两落待排查

> 模型：Claude Sonnet 5
> 日期：2026-10-13 21:15
> 交接对象：接手测试的下一个 AI
> 前序（按时间读）：[1936 对照实验](./20261013-1936-对照实验-镜像卷载荷在当前环境确实进不了WinRE-Sonnet5.md) → [豆包 1900](./20261013-1900-载荷暂存改系统卷修复成功-WinRE备份闭环-豆包.md) → [1935 回归根因](./20261012-1935-新启动通道盘符解析回归根因与修复方向-Step-5-Preview.md)
> 代码状态：**已提交 `8505396`（2.4.6 WIP）**，E2E 未通过，**不要以为修复已生效**

## 一、任务目标（一句话）

让新启动通道的 WinRE 载荷**留在镜像卷（数据卷）**且能正常引导——取代豆包 2.4.5
"载荷放 C:"的权宜方案（那个方案污染 C: 且 restore-existing 断电后无法续跑）。

## 二、已确证的事实（全部有实机证据，不要重测）

### 2.1 真根因：ramdisk 规格的**写入形式**，不是"数据卷不行"

2026-10-13 20:00-20:35 的单变量矩阵（全部手工 bcdedit、同机同载荷）：

| 实验 | 载荷位置 | 目录结构 | 文件属性 | `device`/`osdevice` 写入形式 | 结果 |
|---|---|---|---|---|---|
| Test 1 | F:（数据卷） | 短路径 | 已清 | `ramdisk=[\Device\HarddiskVolume11]\devpathtest\Winre.wim,{…}` | **✅ 进 WinRE** |
| Test A | F: | 短路径 | 隐藏+系统 | `ramdisk=[F:]\devpathtest\Winre.wim,{…}` | ❌ 回落 |
| Test B | F: | 短路径 | 已清 | `ramdisk=[F:]\…` | ❌ 回落 |
| Test C | F: | **GUID 两级目录（复刻产品）** | 隐藏+系统 | `ramdisk=[\Device\HarddiskVolume11]\BackupRestoreRE\<guid>\Winre.wim,{…}` | **✅ 进 WinRE** |
| 对照 | C:（系统卷） | GUID 目录 | 隐藏+系统 | 盘符形式（豆包 2.4.5） | ✅ 进 WinRE |

结论：
1. **`bcdedit` 显示层会把两种写入都规范化成 `ramdisk=[F:]`——`/enum` 看不出存储差异**（这是历史上没人发现的原因）。
2. 盘符形式只对系统卷生效；数据卷必须写 `\Device\HarddiskVolumeN` 设备路径形式。
3. 1935 的"bootmgr 只解析系统卷盘符"表述不完整；`\\?\Volume{GUID}` 形式被 bcdedit 拒绝（1935 已证），但 `\Device\HarddiskVolumeN` 形式被接受且可引导（本轮 Test 1/C 已证）。
4. 目录深度、GUID 目录名、隐藏/系统属性**全部无关**（Test C 全占仍成功）。
5. 历史谜团仍未解：09-29 PoC 与 10-11 任务用**盘符形式**在数据卷上成功过。已排除：Parallels 版本（两次都是 26.4.2）、bootmgr 更新（bootmgfw.efi 停在 09-24）、bootmgr device 形态（已恢复为 `\Device\HarddiskVolume2` 仍失败）。**不要再花时间查这个**，设备路径形式在两种环境都工作，直接用它。

证据截图：`.test-artifacts/boot-menu-verify/devpath-SUCCESS-winre.png`（Test 1）、`sv-C-SUCCESS-product-replica.png`（Test C）、`sv-B-fail-letterform.png`（Test B）。

### 2.2 已排除的假根因（别再碰）

- Parallels 27 固件回归（当前 26.4.2，与 PoC 成功时同版本；且症状是复位循环不是静默回落）
- Windows 更新换 bootmgr（bootmgfw.efi mtime 09-24，无近期更新）
- 载荷文件属性 / 目录结构（Test C 全占仍成功）
- bootmgr 的 `device` 元素形态（`\Device\HarddiskVolume2` vs `S:`，已自行恢复仍失败）
- 我此前文档里"卷缓存未刷净"的推测——**见 2.4，这个理论已被推翻一半**

## 三、当前代码状态（提交 `8505396`，未验证生效）

1. `windows_prepare.rs`：回退豆包 2.4.5（`ReStaging.volume` 回 `image_volume`），注释已写清缘由。
2. `boot_entry.rs`：新增 `dos_device_of()`（QueryDosDeviceW FFI，注意已修过**NUL 截断 bug**——返回值必须截到首个 NUL，多带一个 NUL 会让命令参数直接被拒，报 `nul byte found`）；`finish()` 内 `device/osdevice/ramdisksdidevice` 全部写设备路径形式。**回读校验仍用盘符形式作期望值**（显示层规范化，两者恰好一致）。
3. `windows_prepare.rs`：新增 `flush_volume()` + 武装后、shutdown 前刷载荷卷与 ESP（理论依据见 2.4，**此代码尚未部署**）。
4. 版本 2.4.6；macOS 138 / Windows 167 测试全绿。
5. **已部署到客体的构建是 `6a60ea0b`（含 2、不含 3 的刷盘）**。刷盘代码未部署。

## 四、卡点：产品 E2E 两启两落（核心未解之谜）

时间线（全部本地时间 2026-10-13）：

| 时刻 | 事件 |
|---|---|
| 20:42 | 产品 prepare（任务 `0d8f1ed5-30f5-44de-8297-dfa01339a1fe`，backup T:→F:）武装并重启。载荷暂存 `F:\BackupRestoreRE\0d8f1ed5-…\Winre.wim`（sha256 `26a1d415…`），条目 loader `{889cf421-c703-11f1-896e-a035169616a3}` devopts `{889cf420-…}`，readback 显示 `ramdisk=[F:]\BackupRestoreRE\0d8f1ed5-…\Winre.wim,{889cf420…}`（显示层正常） |
| 20:42:51 | 重启完成 → **回落 #1**：bootsequence 被消费、任务停在 boot-requested、无 WIM |
| 20:46:43 | 我手工 `bcdedit /bootsequence {889cf421}` 重新武装**同一条目**并重启 |
| 20:46:49 | 重启完成 → **回落 #2**（我当时误判为成功，见 4.1 乌龙） |
| 20:47:02 | 桌面 GUI `BackupRestore.exe`（PID 11260，`C:\Users\Public\backupRestore-package`）自启，显示"正在执行备份 STEP 3/4 62%"，**来历不明**（gui.log 无点击记录、temp 无任务目录；可能是用户手动开的，也可能另一会话） |

**未解矛盾**：手工 PS 建的设备路径条目（Test 1/C）100% 可引导；产品 Rust 代码建的条目（读回显示相同）**同一套武装流程两启两落**。注意回落 #2 用的是手工武装产品条目——排除了"武装时序"因素，**问题锁定在条目构造本身（Rust 写入路径）**。

### 4.1 我犯过的乌龙（你必须避开）

把**桌面上 GUI 备份窗口**误认成 WinRE 恢复界面（两者视觉相似：标题"BackupRestore 恢复 v2.4.x"+进度条）。判定环境必须用进程表（`Get-Process explorer` 存在 = Windows；WinRE 无 explorer）或 `Get-CimInstance Win32_OperatingSystem`，**不能只看截图**。

### 4.2 首启回落理论现状

我原推测"722MB 载荷 DISM 覆写后 25 秒内 shutdown /t 0，卷缓存未落盘"。但回落 #2 距武装 6 秒、距覆写 4 分钟——**缓存理论解释不了 #2**，只剩"条目存储层差异"一个候选。刷盘代码留着无害（防御性），但**不要指望它解决本卡点**。

## 五、下一步（按此顺序，别跳步）

### 第 0 步：现场确认

1. VM 里那个 GUI 备份（PID 11260）是否还在跑/跑完——**在跑就等它或问用户，绝不能重启 VM 打断**（在线备份中断会留候选目录）。
2. `git log --oneline -3` 确认 `8505396` 在；`git status` 应基本干净（.office-claw/coding_memory 是本机工具噪声，随其它改动一并提交即可，用户已确认）。
3. BCD 基线核对：`bcdedit /enum {bootmgr} /v` 应为 `default {12e3701c-c361-11f1-90a2-c222b5348ef7}`、`displayorder {152dcf4e-c674-11f1-895e-8d900a1bd081}+{current}`、`timeout 10`、`device partition=\Device\HarddiskVolume2`。不符则先按 AGENTS.md 处理。

### 第 1 步：确认部署的二进制里真的有修复（最廉价的排除项）

`findstr /m /C:"QueryDosDeviceW" H:\brwork\BackupRestore.exe` ——我上次跑报"系统找不到指定的路径"，**命令本身可能失败**（findstr 对 exe 的行为），换 PowerShell 字节搜索：

```powershell
$b=[IO.File]::ReadAllBytes('H:\brwork\BackupRestore.exe');$s=[Text.Encoding]::ASCII.GetString($b);$s.Contains('QueryDosDeviceW')
```

若 False → 6a60ea0b 没编进修复（X: 共享缓存陈旧等），直接重建部署（见第 4 步命令）重跑 E2E 即可，卡点可能就此消失。
若 True → 进第 2 步。

### 第 2 步：读条目**存储层**（显示层看不出差异，这是定因的关键）

WMI BCD API 能拿原始元素值。对产品条目 `{889cf421-c703-11f1-896e-a035169616a3}`（还在 BCD 里，回落不删条目）：

```powershell
$store=[wmi]"root\WMI:BcdStore.OpenStore?FilePath=''" # 或用默认 store 的正确打开方式
# 元素类型：0x02000002=device, 0x02000003=osdevice（BCD_ELEMENT_TYPE 设备类）
```

（WMI BcdStore 的准确打开姿势请现场查证：`BcdStore.OpenStore("")` 打开系统 store；`BcdObject.EnumerateObjects`/`GetElement(0x22000002)` 取值——注意 device 元素类型号要用文档核对，我未及验证 API 细节。）

对比物：手工建一个设备路径条目（照第 3 步 PS 脚本，不武装），读两者 `device` 元素存储值：
- 产品条目存的是 `ramdisk=[F:]…` → Rust 写入路径有 bug（回到代码找，重点看 `dos_device_of` 返回值与 `spec` 拼接、以及 `bcd_write` 重试块是否可能用旧值）
- 产品条目存的也是 `\Device\…` → 存储层相同却两启两落 → 差异在更深处（考虑导出 BCD 二进制 diff：`bcdedit /export C:\br-verify\probe.bcd` 后对比条目二进制）

### 第 3 步：手工条目指向**产品暂存载荷**的判别实验（绕开 WMI 也行）

用 Test C 的 PS 脚本改一个路径：指向 `F:\BackupRestoreRE\0d8f1ed5-30f5-44de-8297-dfa01339a1fe\Winre.wim`（产品暂存的那份，注入版），新建条目（**别复用 {889cf421}**）+ 设备路径 + 武装 + 重启：
- ✅ 进 WinRE → 载荷没问题，纯条目写入差异（回第 2 步代码侧）
- ❌ 回落 → **注入版载荷从数据卷引导不了**（虽然豆包从 C: 引导过它）→ 问题变成"注入流程产出的 WIM 差异"，去比 `F:\BackupRestoreRE\0d8f1ed5…\Winre.wim` 与 `C:\Recovery\WindowsRE\Winre.wim`（注意我手工测试用的是后者副本且成功）

### 第 4 步：E2E 重跑（前几步清障后）

```powershell
# 先部署含刷盘的最新构建（刷盘代码尚未部署！）
& 'X:\windows\build-windows.ps1' -Architecture arm64 -Deploy -CargoTargetDir 'C:\br-verify\target' -DeployPackageDir 'C:\Users\Public\backupRestore-package' -DeployWorkDir 'H:\brwork'
# E2E（T: 38MB 小卷，快）
H:\brwork\BackupRestore.exe prepare --operation backup --source-drive T --image-path F:\BRImages\test-246-e2e.wim --allow-destructive
```

成功判据：重启进 WinRE → 自动捕获 → 回桌面 → status.json `stage=success` → `F:\BRImages\test-246-e2e.wim` 存在。豆包 2.4.5 同型任务全程 19 秒，可作速度参照（超过 5 分钟即异常）。

### 第 5 步：收尾（按仓库惯例，缺一不可）

1. 清理：回落任务的条目 `{889cf421}/{889cf420}`（确认无 displayorder/default 引用后 `bcdedit /delete {…} /f`）、`F:\BackupRestoreRE`、测试 WIM；保留/归档证据日志到 `.test-artifacts/`。
2. E2E 通过后：2.4.6 转正提交（改 WIP 措辞）、写轮次文档、**每轮新写**【当前开发进度】【下一步待实现】一对（不许只追加旧文件）、README 两行指向、文档索引收录——这是用户强调过的硬规矩（有记忆 round-closeout-docs-checklist）。
3. 提醒：`native_gui.rs:3579`（GUI 装 PE ramdisk）也写盘符形式，同类问题，产品主链验证过后再评估是否同修。

## 六、环境与坑（全是本机实测踩过，信我）

| 坑 | 对策 |
|---|---|
| `prlctl exec` 在 VM 重启后丢 X: 共享 | 先单独 `net use X: \\Mac\backupRestore`，成功后**再单独一次** exec 做 copy——链式写在一行里会静默失败 |
| PowerShell 5.1 对无 BOM 的 UTF-8 按 GBK 解析 | 写入客体的 .ps1 必须**纯 ASCII** 或带 UTF-8 BOM |
| 实验性回落后 VM 卡锁屏、exec 报 "Unable to open new session" | `prlctl stop 'Windows 11' --force` + 8 秒 + `prlctl start`，16 秒就绪（自动登录恢复） |
| 长操作前 VM 自动暂停 | `prlctl set 'Windows 11' --pause-idle off` |
| prlctl 输出编码 | 一律 `iconv -f GB18030 -t UTF-8` |
| **本 VM 的全功能 WinRE 里 prlctl exec 可用**（裸 WinPE 不可用） | 判定环境看进程表，别看截图（4.1 乌龙） |
| bcdedit 显示层规范化 | ramdisk 规格的存储形式**无法**从 `/enum` 输出判断 |
| 桌面 GUI 备份窗口与 WinRE 恢复界面视觉相似 | 环境判定用 `Get-Process explorer` |
| 并发会话活跃（豆包/GLM 都动过这个仓库） | 动手前 `git status` + 查文件 mtime；commit 前再查一次 |
| Monitor 去重 | 比较键别含时间戳（我踩过：每条都算新事件刷屏） |
| 破坏性操作纪律 | 任何改引导实验前记 `{bootmgr}` 的 default/displayorder/resumeobject/timeout 四值（AGENTS.md 空间纪律）；实验对象（loader+devopts 对）用完确认无引用后删除 |

## 七、当前 VM 残留清单（接手时核对）

- 任务 `H:\brwork\tasks\0d8f1ed5-30f5-44de-8297-dfa01339a1fe`（boot-requested，两启两落的主体）
- BCD 对象 `{889cf421}`（loader）+ `{889cf420}`（devopts）——产品建，待判别后清理
- 暂存 `F:\BackupRestoreRE\0d8f1ed5-…\Winre.wim`（722MB 注入版）+ boot.sdi
- 豆包成功任务 `ae3dac88`（success，保留作 2.4.5 证据）
- 谜之 GUI 备份（PID 11260，20:47:02 起）——先确认归属再动
- `F:\BRImages\`（test-246-e2e.wim 未生成；testdata.wim 是豆包的）
