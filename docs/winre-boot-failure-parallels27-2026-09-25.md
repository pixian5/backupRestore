# 2026-09-25 定案：WinRE/PE ramdisk 引导失败 = Parallels Desktop 27 固件回归

> 结论先行：**不是 BackupRestore 的 bug，不是 BCD/分区/文件系统问题，也不是 WIM 问题。**
> 根因是 Parallels Desktop 27.x 的 EFI 固件在执行 ramdisk 引导（bootmgr 加载
> Winre.wim / boot.wim）时崩溃并触发 VM 复位。降级或等待 Parallels 修复前，
> 本 VM 上"自动重启进 WinRE/PE"链路无法闭环。

## 一、时间线与"某一次之后彻底坏了"

| 日期 | 事件 | WinRE ramdisk 引导 |
|---|---|---|
| 08-21 / 09-09 / 09-13(白天) | Parallels 旧版本 | ✅ 多次成功（有快照与文档为证） |
| 09-16 | 出现 6 秒 reset 循环（`PSCI: reset requested!`，`OnExitBootServices()` 后 ~4s 复位） | ❌ |
| 09-19 15:26 | **Parallels Desktop 27.0.2 安装**（app 包 mtime；GuestTools 仍是 27.0.1 也证明 27.x 自动更新链） | ❌ |
| 09-24/25 本轮实测 | WinRE（C: NTFS / Y: FAT32）、PE(367MB, T: NTFS) 全部失败，复位签名一致 | ❌ |

09-13 18:30 建 T: 分区只是**巧合的时间邻近事件**，不是根因。09-16 文档
`winre-bcd-loop-fix-2026-09-16.md` 中"NTFS RAMDISK 大 WIM 失败、UDF/ISO 成功"的
推断应当修正为：**ramdisk 引导本身在 PD27 上崩溃，与文件系统、WIM 大小无关**
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

1. **降级 Parallels Desktop 到 26.x**（最后已知可用版本）后复测——最直接的
   验证/恢复路径。需要下载 ~600MB 安装包（注意网络类型，热点需用户确认）。
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
