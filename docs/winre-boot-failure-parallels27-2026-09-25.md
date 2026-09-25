# 2026-09-25 定案：WinRE/PE ramdisk 引导失败 = Parallels Desktop 27.x 固件回归

> 结论先行：**不是 BackupRestore 的 bug，不是 BCD/分区/文件系统问题，也不是 WIM 问题。**
> 根因是 Parallels Desktop **27.x**（首次出现在日志中为 27.0.1 build 58670，≤ 09-15 22:40；
> 09-19 15:26 更新到 27.0.2 build 58673）的 EFI 固件在执行 ramdisk 引导（bootmgr 加载
> Winre.wim / boot.wim）时崩溃并触发 VM 复位。降级或等待 Parallels 修复前，
> 本 VM 上"自动重启进 WinRE/PE"链路无法闭环。
>
> **置信度说明（2026-09-25 04:20 更新：已闭环证明）**：最初的"高置信度推断"已于
> 09-25 04:20 通过 **PD 26.4.2 降级 A/B 对照实验闭环证明**——同一 VM、同一 Winre.wim
> （从 ISO install.wim 重新提取）、同一 reagentc 标准注册、同一 bootsequence 触发，
> 在 **26.4.2 上 WinRE 成功引导**（截图 `.test-artifacts/winre-boot-pd26-success-0420.png`，
> 标准的「选择一个选项」菜单；无自发复位），而 27.x 上同链路是 6 秒复位循环。
> **根因坐实：Parallels Desktop 27.x 固件回归，与 BackupRestore 无关。**
>
> **2026-09-25 02:00 独立复现**：全新 tiny11 ARM64 虚拟机（PD 27.0.2 全新安装，
> 与旧 VM 毫无共享状态）上，`prepare --operation backup` 全链路成功（任务创建、
> BCD 快照、WinRE WIM 挂载注入 payload、dism commit、`reagentc /boottore`、
> 状态推进 boot-requested、`shutdown /r`），重启后 bootsequence 被消费但 WinRE
> 仍未引导，13 秒直接回到 Windows 桌面，`status.json` 停在 boot-requested。
> BCD（ramdisk=[HarddiskVolume4]\Recovery\WindowsRE\Winre.wim + ramdisksdipath）
> 与 `reagentc /info`（Enabled, partition4）全部标准。**两台独立 VM 同签名，
> 固件回归实锤。**

## 一、时间线与"某一次之后彻底坏了"

| 日期 | 事件 | WinRE ramdisk 引导 |
|---|---|---|
| 08-21 / 09-09 / 09-13(白天) | Parallels 26.x 或更早 | ✅ 多次成功（有快照与文档为证） |
| 09-13 ~ 09-15 之间 | **PD 升级到 27.0.1 (build 58670，编译于 08-26)**，日志首见 09-15 22:40:43 | — |
| 09-16 05:32 | 首次出现 6 秒 reset 循环（`PSCI: reset requested!`，`OnExitBootServices()` 后 ~4s 复位） | ❌ |
| 09-19 15:26 | app 包 mtime 更新为 **27.0.2 (58673)**；日志首见 09-21 15:03 | ❌ |
| 09-24/25 本轮实测 | WinRE（C: NTFS / Y: FAT32）、PE(367MB, T: NTFS)、全新 tiny VM 全部失败，复位签名一致 | ❌ |

**版本取证命令与结果**（2026-09-25 02:35 复核）：

```
defaults read "/Applications/Parallels Desktop.app/Contents/Info.plist" CFBundleShortVersionString  → 27.0.2
ls -ld "/Applications/Parallels Desktop.app"                                                        → 09-19 15:26
/Library/Logs/parallels.log.2.gz (09-15 22:40 ~ 09-16 07:43)  → 3919× "27.0.1"，0× "26.x"
/Library/Logs/parallels.log.1.gz (09-16 07:43 ~ 09-23)        → 939× 27.0.1，302× 27.0.2
/Library/Logs/parallels.log      (09-23 14:00 ~ 09-25 02:36)  → 926× 27.0.2
```

即：**崩溃在 27.0.1 上就已发生**（09-16 05:32 的复位循环），27.0.2 只是延续该回归。
09-13 18:30 建 T: 分区只是**巧合的时间邻近事件**，不是根因。09-16 文档
`winre-bcd-loop-fix-2026-09-16.md` 中"NTFS RAMDISK 大 WIM 失败、UDF/ISO 成功"的
推断应当修正为：**ramdisk 引导本身在 PD 27.x 上崩溃，与文件系统、WIM 大小无关**
（本轮 367MB 的 PE WIM 同样触发复位）。

## 二、本轮实验证据（2026-09-24 晚 ~ 09-25 凌晨）

1. **FAT32 实验被判无效**：复盘发现上次只拷回了 boot.sdi/ReAgent.xml，
   810MB 的 Winre.wim 根本没上 Y:（"4 文件 813,669,235 字节"是备份目录总和，
   被误读为拷回成功）。本轮已用 robocopy 补拷成功后再测——仍失败。
2. **reagentc 官方注册路径重放**：`reagentc /setreimage /path Y:\...` 后
   `/enable` 会无视 GPT Recovery 分区、把 WinRE 复制到 C:\Recovery\WindowsRE
   （NTFS）并生成标准 BCD 条目（`ramdisk=[C:]\Recovery\WindowsRE\Winre.wim`）。
   用这条"最标准"的配置设 `bootsequence` 重启——同样失败。BCD、boot.sdi、
   分区属性(0x8000000000000001)、GPT 类型 de94bba4 全部核对无误。
3. **PE 小 WIM 对照**：pe-boot.wim(367MB) 拷到 T:\petest，新建 osloader 条目
   （ramdisk=[T:]\petest\boot.wim + ramdisksdipath \petest\boot.sdi + winpe +
   detecthal + nx OptIn），bootsequence 重启——也失败。
4. **抓到 boot 菜单画面**（窗口级截屏）：bootsequence 被消费后菜单高亮的仍是
   default(Windows 11)；WinRE/PETest 条目在列但被静默跳过。
5. **Parallels 日志铁证**（`/Library/Logs/parallels.log`）：
   ```
   00:23:06.791  PET_DSP_EVT_VM_ABOUT_TO_RESET
   00:23:07.252  PET_DSP_EVT_VM_RESET            ← 计划内重启(shutdown /r)
   00:23:13.444  PET_DSP_EVT_VM_ABOUT_TO_RESET   ← 6.4s 后自发复位 = ramdisk 引导崩溃
   00:23:13.683  PET_DSP_EVT_VM_RESET
   ```
   与 09-16 的 6 秒 reset 循环、PD27 论坛已确认的
   "ExitBootServices 后挂起/复位"（Ubuntu 案例，嵌套虚拟化开）同一类签名。
   本 VM 嵌套虚拟化=off、PMU=off，说明 PD27 固件回归不止嵌套虚拟化一个触发面。
6. **排除项**：ESP bootmgfw.efi 曾与 C:\Windows 版本哈希不一致（04281e60… vs
   fae47d78…，当天 17:24 被更新过），已用 C:\Windows 版本覆盖并 bcdboot 重建
   BCD——重测仍失败，排除引导器文件因素。

## 三、判定依据汇总（为什么锁定 Parallels 27）

- 同一 VM、同一 BCD 写法、同一 WIM 在旧版本 Parallels 上成功过多次；
- NTFS/FAT32、810MB/367MB、reagentc 标准条目/手写条目全部失败；
- 失败形态 = 静默复位（Parallels 日志可观测），而非 0xc00000xx 错误画面；
- bootsequence 每次都被消费（引导一次即消失），行为 = 尝试→崩溃→复位→回落。

## 四、后续可选路径

1. **降级 Parallels Desktop 到 26.x**（最后已知可用版本）后复测——**唯一能把"高置信度推断"
   变成"闭环证明"的路径**，同时也是恢复开发验证能力的路径。需要下载 ~600MB 安装包
   （注意网络类型，热点需用户确认）。降级前先给两台 VM 各建快照。
2. **向 Parallels 报 bug**：附上本文的 reset 日志与复现步骤（任一 VM 设
   bootsequence 指向 ramdisk 条目即可复现）。
3. 等 PD 27.0.3+ 修复后升级复测。
4. 在真实 Windows 硬件（x64/ARM）上验证 WinRE 链路（不受本 bug 影响）。
5. 临时替代：ISO/UDF 介质引导 PE 不走 ramdisk BCD 路径，可继续用于手工验证。

## 五、当前 VM 状态（09-25 00:30）

- WinRE：已由 reagentc 标准注册到 C:\Recovery\WindowsRE（NTFS，Enabled），
  BCD 条目 {6dd90d2f}，recoverysequence 指向它——**配置是健康的**，只是 PD27 引不进去。
- Y:(part6, 840MB, FAT32, GPT Recovery)：装有 boot.sdi + ReAgent.xml，
  Winre.wim 已补拷（供将来 FAT32 复测）。
- 遗留测试物：BCD 中 PETest 条目 {6dd90d31}（T:\petest\boot.wim），
  ESP 有一份覆盖后的 bootmgfw.efi（哈希=fae47d78…）与 esp-backup 备份
  （C:\Users\Public\backupRestore-package\esp-backup\）。
- 快照链（可回退）：{d72c40ef} → {02c8b5db} → {7f4ff848}(bcdboot 前) →
  {320b2dcd}(覆盖 ESP 引导文件后)。

## 六、闭环证明（2026-09-25 04:00-04:20，PD 26.4.2 A/B 对照）

### 实验条件
- Parallels Desktop 已降级到 **26.4.2 (57518)**（27.0.2 卸载后装回 26.4.2）。
- 降级后 VM 卡在 UEFI 固件菜单（见 `pd26-downgrade-vm-recovery-2026-09-25.md`），
  先删除 NVRAM.dat/tnvs 由 26.4.2 固件重建后正常进桌面。
- WinRE 资产全部重建：Winre.wim(709,810,082B) + boot.sdi(3,170,304B) 从 Windows 11 ISO
  的 install.wim index 1 用 wimlib-imagex 提取（宿主机），robocopy 进
  C:\Recovery\WindowsRE；删除陈旧 ReAgent.xml 后 reagentc 注册成功（Enabled，
  条目 {fa68c813-b854-11f1-88b9-da86a19ef236}）。
- `reagentc /boottore` 后 **bcdedit /enum {bootmgr} /v 实际确认** bootsequence
  已写入（不信任日志，见 09-24 假成功教训）。

### 结果（04:18:34 重启）
| 观察点 | PD 27.x（昨晚） | PD 26.4.2（本次） |
|---|---|---|
| 计划内复位后的自发 VM_RESET | ~6.4s 必现 | **无**（日志 04:18:50 只有一次计划内 RESET） |
| bootsequence 消费后 | 回落 Windows | **进入 WinRE** |
| WinRE 画面 | 从未出现 | 「选择一个选项」菜单（截图在案） |
| 退出方式 | — | `prlctl send-key-event --scancode 28`（回车选高亮"继续"）→ 25s 后回桌面 Tools OK |

### 结论
**Parallels Desktop 27.x 固件 ramdisk 引导回归 = 唯一根因，闭环证明。**
BackupRestore 的 prepare/WinRE 链路在 26.x 上完全正常。后续路径：
留在 26.4.2 开发验证；或等 27.0.3+ 修复；真机部署不受影响。

## 2026-09-25 PD 26.4.2 上的同卷挂载修复复验

在已降级至 Parallels Desktop 26.4.2 的 Windows 11 ARM64 VM 上，对本轮
`mount_env_volume` 修复执行了真实、非破坏性 `probe` 启动闭环。VM ID 为
`{caee9cb3-bac7-41e2-85f2-32b3a7369114}`；操作前已创建并核验快照
`{60871528-3efb-4ad5-b454-c00fd1e93ae5}`，启动事务前另建并核验
`{c2860f73-0042-4969-a599-2659f51462e3}`。补丁 ARM64 程序部署在
`E:\BR-Recheck-patched-20260925`，部署的 CLI/Recovery SHA-256 均为
`ad647c428c9161585ffcb9f636d90062ba0a9aadf5792a1479bb63258f2cba94`。

真实自动启动的 WinRE 日志显示：`RECOVERY` 角色把分区 4 挂载为 `R:`；`SOURCE`
随后解析到同一卷 GUID，复用 `R:`，没有再把它改挂到 `S:`。Recovery 随后运行
`probe`，没有发出磁盘操作，恢复注册的原始 `Winre.wim` 并按 manifest 中的
`originalWinreSha256` 校验成功，任务状态到达 `success`/100%，再自动重启返回
Windows。返回后的提权审计显示 `reagentc /info` 为 Enabled，注册位置仍为
`harddisk0\partition4\Recovery\WindowsRE`；`bcdedit /enum {bootmgr}` 未显示残留的
`bootsequence`。任务保存的 `bcd-before-raw` SHA-256 与任务 `previousBcdSha256`
完全一致（`2613c32e…648c1f8d0`）。这验证的是保存副本与其 manifest 记录一致，
不是声称运行中 BCD hive 的字节哈希与启动前完全相同。

本次只验证 `probe` 和 WinRE 自动启动/清理路径，**没有验证 WIM Capture、Apply、真实
备份或系统还原**。后续验收必须继续使用隔离目标卷及快照，并按
`docs/verification-matrix.md` 分别记录这些操作；不得将 probe 成功扩大解释为产品级
备份还原成功。证据保存在被忽略的
`.test-artifacts/recheck-20260925/`（尤其 `post-boot-audit.txt`、
`status-patched.txt`、`deploy-patched-e.txt`）。
