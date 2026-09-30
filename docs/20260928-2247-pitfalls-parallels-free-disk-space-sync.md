# 坑：Parallels「空闲磁盘空间同步」与客体卷可用空间虚高/虚低（2026-09-28）

## 结论先说

这个功能是**保护宿主不被客体写爆**的，代价是客体报告的可用空间被钳制。
2026-09-28 曾关掉验证，随后**已重新打开**（配置回到 `SyncFreeSpaceFromHost=1`）。
宿主剩余只有约 106 GB，保护比客体报告的剩余数字更值得。

## 机制

Parallels 在每个客体卷根目录写一个名为 `Mac disk` 的填充文件，**动态伸缩**，
使客体报告的可用空间永远不超过宿主真实剩余。文件内容自述（读前 512 字节
即可确认，各卷头部字节一致）：

    This is an excessive free disk space holding file by Parallels. It helps
    to make sure that the guest system won't have more free disk space than
    there is available on the host. To get rid of this file turn off the free
    disk space synchronization feature in the virtual machine settings.

配置：`~/Parallels/Windows 11.pvm/config.pvs:365`

```xml
<DiskSpaceOptimization dyn_lists="">
   <SyncFreeSpaceFromHost>1</SyncFreeSpaceFromHost>
</DiskSpaceOptimization>
```

相关项：`ReclaimDiskSpace`（119 行）、`FreeDiskSpaceRatio=50`（381 行）。
prlctl 26.4.2 无对应 CLI 开关（`--optimization` 不被识别），只能改 config.pvs，
且需停机。

## 实测数据

钳制值 = 宿主真实剩余。关同步与开同步对比：

| 卷 | 填充文件 | 报告可用（关） | 报告可用（开） |
|---|---|---|---|
| E: br | 131.24 GB | 237.22 GB | 105.98 GB |
| F: BRIMG | 212.75 GB | 318.73 GB | 105.98 GB |
| H: BRImages | 68.76 GB | 174.74 GB | 105.98 GB |

宿主实测剩余 105.90 GB ≈ 客体报告的 105.98 GB，钳制关系坐实。
`C:` / `Y:` 不出现该文件；`P:` / `T:` 出现但为 0 字节（已满的卷）。

## 关于稀疏性（易判错，已实测澄清）

**该文件不是稀疏文件，真实分配簇。** 用卷算数即可证明：

    350 GB(F:总) - 212.75 GB(填充) - 31.25 GB(真实数据) = 106 GB ≈ 报告剩余

`C:\Recovery\WindowsRE` 之类系统的 Hidden/System 属性问题也曾让我误判过一次，
这次改用卷算术而非 `fsutil volume allocationreport` 的「可用簇数」字段
（该字段与 `Get-Volume` 的 SizeRemaining 不一致，不可靠）。

## 对 BackupRestore 的影响

`prepare` 的容量检查读客体报告的剩余空间，因此会被钳制值误导：
F: 真实可写 318.73 GB，程序只看到 105.98 GB，装不下 C: 的 80 GB 源卷，
备份在空间检查就失败。

**正确的修法在程序侧，不是关掉这个保护。** 容量检查应基于「该卷真实已分配的
数据量」而不是客体报告的剩余。可选项：

1. 统计目标卷上真实数据总量，与源卷已用空间比较（避开 `Mac disk` 干扰）。
2. 识别并排除 `Mac disk` 填充文件后再读剩余空间。

暂未实现。当前 `.test-artifacts/rb2.cmd` 里的
`net stop "Parallels Tools Service"` 绕法会连带断掉 prlctl exec 通道，属治症状。

## 诊断方法（可直接复用）

宿主真实剩余 vs 客体各卷 `SizeRemaining` 对撞：若多个卷的 `SizeRemaining`
数值完全相同且等于宿主剩余，即被钳制。

```bash
df -k /System/Volumes/Data | tail -1 | awk '{printf "host avail = %.2f GB\n", $4/1048576}'
# 客体：Get-Volume | Select DriveLetter,SizeRemaining
```

辅证：客体卷根存在 `Mac disk`，且体积会随宿主剩余变化而伸缩。

## 操作留证

| 动作 | 快照 | 配置备份 |
|---|---|---|
| 关同步验证 | `{0e19cbfe-1daf-48c4-88ce-48f165570a7e}` | `config.pvs.bak-presync-20260928-224502` |
| 重新打开（最终态） | `{6b3ca285-1810-4bda-bd78-65d109a436e4}` | `config.pvs.bak-postsync-off-*` |

最终状态：`SyncFreeSpaceFromHost=1`（保护开启）。

## 踩到的附带问题

往客体复制 `.ps1` 时**中文会被按 GBK 解码而损坏**，PowerShell 报
「字符串缺少终止符」。脚本正文保持 ASCII；需要中文输出就写到宿主侧 UTF-8
文件再回宿主读。
