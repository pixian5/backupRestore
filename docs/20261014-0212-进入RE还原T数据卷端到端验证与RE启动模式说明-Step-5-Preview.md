# 进入 WinRE 还原 T:（5G 数据卷）端到端验证 + RE 启动模式说明（2026-10-14 02:12，v2.4.7）

> 模型：Step-5-Preview｜触发：用户要求"验证进入 RE 还原刚备份的 T:（5G 数据卷）"，并问"修改版的 RE 走的是内存还是独立分区模式"
> 任务：`559d28bd-684c-47d4-b05a-0fe6aefcdc8f`｜镜像：`H:\BRImages\re-channel-t-backup.wim`（02:00 刚由同一通道备份）

## 0. 结论先行

**RE 离线还原 T: 成功**，与 02:00 的 RE 备份构成完整 round-trip。RE 内执行仅 **11 秒**（18:10:37 → 18:10:48 UTC），全程无人工干预。

| 验收项 | 结果 |
| --- | --- |
| 任务终态 | `status.json`：`restore-existing / success / 100` |
| 格式化 | diskpart 格式化 **disk=0 partition=5**（T:），格式化后身份复核通过 |
| 应用镜像 | `dism /Apply-Image /ImageFile:E:\BRImages\re-channel-t-backup.wim /Index:1 /ApplyDir:D:\` → success（D:\ = RE 内 T:） |
| 引导 | **零改动**：`目标卷不是 Windows 系统卷，跳过引导修复候选探测` + `data-volume restore: target has no SYSTEM hive; skipping BCDBoot` |
| T: 落盘 | `BackupRestoreRE` + `BRRE213testdata`（data1.bin / readme.txt / sub1 / sub2）与镜像一致 |
| BCD 现场 | 与基线逐字一致（default `{12e3701c…}`、displayorder 首项 `{152dcf4e…}`、无 bootsequence、自建条目已删） |
| 载荷暂存 | `H:\BackupRestoreRE\559d28bd…` removed and verified；镜像文件未被改动（仍 16,779,593B / 01:57） |

## 1. 命令与通道约束

```bat
H:\brwork\BackupRestore.exe prepare --operation restore-existing ^
  --source-drive T --target-drive T ^
  --image-path H:\BRImages\re-channel-t-backup.wim --wim-index 1 --allow-destructive
```

prepare 侧校验（`windows_prepare.rs:1977-2008`）实见：

- `--allow-destructive` **必填**（还原即破坏性操作，CLI 面以此充当确认）；
- `RestoreExisting` 要求 **source == target**（"single-system restore target must equal source"）——所以源、目标都传 T；
- 镜像卷 ≠ 目标卷（H: ≠ T:）；镜像存在；副档大小一致（16,779,593 == 16,779,593，静默通过）；
- ⚠️ **`wim_full_windows_probe` 本会硬拒数据卷镜像**（缺 reagentc/SYSTEM 标记即拒），但本机 DISM `/Mount-Wim` 不可用（报 87，与 2026-10-13 实测的附加 VHD 环境一致）→ 降级为镜像名启发式，`T-Drive-RE-Channel` 不含 PE 特征 → 放行。**这与 01:46 在线还原走通是同一环境条件**，也正对应待办修复项⑤「wim_full_windows_probe 处置」——数据卷镜像在挂载可用的机器上会被 prepare 拒，而恢复期本有 data-volume 分支，判据不一致。

## 2. 完整时间线（UTC / GMT+8）

| 时间 | 事件 |
| --- | --- |
| 18:10:14 / 02:10:14 | 载荷暂存 `H:\BackupRestoreRE\559d28bd…\Winre.wim`（sha256 `f916831a…`，与备份任务的注入载荷同源不同任务） |
| 18:10:21 | bootsequence 武装于 `{561d0ba3-c731-11f1-897b-9f904d57e8d0}` → `shutdown /r /t 0` |
| 18:10:37 | WinRE 起来；挂载：RECOVERY→C:、SOURCE→D:、IMAGE→E:、**TARGET→D:（与 SOURCE 共用，同一卷）**、EFI→Z:（diskpart 指派） |
| 18:10:42 | `Recovery.exe started operation=RestoreExisting`；`/Get-WimInfo`；目标身份重绑定 (0,5)→(0,5)；跳过引导修复候选探测 |
| 18:10:44 | **一次性 bootsequence 重新武装（续跑保护）**；diskpart 格式化 disk=0 partition=5；格式化后身份复核 |
| 18:10:46 | DISM Apply-Image 成功；`data-volume restore: skipping BCDBoot` |
| 18:10:46-48 | STEP 4/4：bootsequence 清除 → BCD 对象删除并核验 → 暂存目录删除并核验 → 任务标记成功 |
| 18:10:48 | `wpeutil reboot` → 约 18:11 回桌面 |

**断电续跑设计细节**：擦除目标前会把一次性启动**重新武装**（日志 `one-shot bootsequence re-armed for resumption`），保证还原中途断电后下次开机仍进同一任务；成功清理时再清除。备份通道无此步（源卷只读）。

## 3. 与另两条还原通道的对比

| | 在线还原（GUI，01:46 已验证） | **RE 离线还原（本次）** |
| --- | --- | --- |
| 触发 | GUI「单系统还原」，目标 ≠ 活动系统盘 | `prepare --operation restore-existing` |
| 格式化目标 | 否（数据卷在线直写） | **是**（diskpart 格式化后应用） |
| DISM 参数 | `/ApplyDir:T:\ /CheckIntegrity /Verify` | `/ApplyDir:D:\`（**不带** /CheckIntegrity /Verify） |
| BCDBoot | 跳过（无 SYSTEM hive） | 跳过（日志明示 data-volume 分支） |
| 重启 | 不重启 | 重启进 RE，完成后回桌面 |
| 破坏性确认 | GUI 弹窗手动点是 | CLI `--allow-destructive` 显式开关 |

两侧判据一致：**数据卷镜像 → 只应用、不碰引导**；完整 Windows 镜像 → 走格式化 + BCDBoot 全流程。

## 4. 附答：修改版的 RE 走内存还是独立分区？

**内存（ramdisk）模式**，证据链：

1. BCD 条目形态：`device/osdevice = ramdisk=[H:]\BackupRestoreRE\<任务id>\Winre.wim,{设备选项GUID}`（`boot-entry.json` 与 prepare.log 回读一致，v1.7.11 起一律设备路径形态）。
2. bootmgr 把该 WIM **整个载入内存**成 RAM 盘，RE 内表现为 **X:** 盘——DISM 的排除配置文件在 `X:\windows\SystemTemp\BackupRestore-exclusions.ini`、diskpart 脚本在 `X:\Windows\Temp\…`，均为 RAM 盘路径。
3. WIM 文件本体只是**暂存源**，放在镜像卷（H:\BackupRestoreRE\<id>\），任务结束即删；它不是启动分区。
4. 机器上真正的恢复分区（`C:\Recovery\WindowsRE` = disk0/partition4，注册 WinRE）在本通道里**全程只读**（仅 `reagentc /info` 读取核对），不写入、不注册、不改 boottore。

对照：v1.7.11 之前的旧通道才是"独立分区"模式——把注入后的 Winre.wim 写回注册恢复分区。现在的 PE 部署功能另有一个"硬盘版"选项（自带分区），但**本条备份/还原 RE 通道是纯 ramdisk**，不占任何分区。

## 5. 备注

- 离线 Apply 不带 `/CheckIntegrity /Verify`，在线带——两侧完整性判据不同（离线侧靠副档预检 + DISM 自身 WIM 结构校验），记录备查。
- 本次 round-trip（RE 备份 → RE 还原）与 01:46 的在线还原互为交叉验证：同一镜像两种通道落到 T: 的内容一致。
