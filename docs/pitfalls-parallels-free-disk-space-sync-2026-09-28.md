# 坑：Parallels「空闲磁盘空间同步」让客体卷虚占 394 GB（2026-09-28）

## 症状

客体各卷报告的空闲空间被钳制在宿主真实剩余空间上，导致：

- `E:` / `F:` / `H:` 报告可用**全都是 112.2 GB**（宿主真实剩余 112.18 GB）。
- `BackupRestore.exe prepare --operation backup` 的空间检查失败——客体以为装不下，
  实际真实数据只有几十 GB。
- 宿主盘也一起被撑满。

## 根因

Parallels Desktop 的**空闲磁盘空间同步**功能在各客体卷根目录写入一个名为
`Mac disk` 的填充文件，目的是「客体不得报告比宿主更多的空闲空间」。

配置位置：`~/Parallels/Windows 11.pvm/config.pvs:365`

```xml
<DiskSpaceOptimization dyn_lists="">
   <SyncFreeSpaceFromHost>1</SyncFreeSpaceFromHost>
</DiskSpaceOptimization>
```

文件内容自述（读取前 512 字节即可确认，三个卷头部字节完全一致）：

    This is an excessive free disk space holding file by Parallels. It helps
    to make sure that the guest system won't have more free disk space than
    there is available on the host. To get rid of this file turn off the free
    disk space synchronization feature in the virtual machine settings.

## 实测数据（关同步前后）

| 卷 | 客体认为已用 | 其中 `Mac disk` | 真实数据 | 报告可用（前） | 报告可用（后） |
|---|---|---|---|---|---|
| E: br | 143.7 GB | 124.98 GB | 18.76 GB | 112.2 | 237.22 |
| F: BRIMG | 237.8 GB | 206.5 GB | 31.25 GB | 112.2 | 318.73 |
| H: BRImages | 79.7 GB | 62.51 GB | 17.24 GB | 112.2 | 174.74 |

合计 **394 GB 虚占**。`C:` / `P:` / `T:` 没有该文件，报告正常。

## 修复

停机后把 `config.pvs` 的 `SyncFreeSpaceFromHost` 改为 `0`，再启动。本次操作留证：
快照 `{0e19cbfe-1daf-48c4-88ce-48f165570a7e}`（pre-freedisksync-off-20260928），
配置备份 `config.pvs.bak-presync-20260928-224502`。

## 诊断方法（可直接复用）

用宿主真实剩余空间和各客体卷报告值对撞，一眼就能确认：

```bash
df -k /System/Volumes/Data | tail -1 | awk '{printf "host avail = %.2f GB\n", $4/1048576}'
# 客体：Get-Volume 看每个卷的 SizeRemaining
# 若多个卷的 SizeRemaining 完全相同且等于宿主剩余 → 就是被同步钳制了
```

辅证：
- 客体卷根目录存在 `Mac disk` 文件，且体积与卷容量成比例。
- `fsutil volume allocationreport F:` 的「可用簇」与「已分配簇」之差
  正好等于 `Mac disk` 的大小（该文件不真实分配簇）。

## 注意

- 这个功能是**保护宿主不被客体写爆**的。关掉后客体可报告比宿主更多的空闲空间；
  若客体真的写满，宿主会先满。测试 VM 场景下宿主剩余 112 GB、镜像约 80 GB，
  关掉是安全的；若后续要跑更大的写入，需重新评估。
- 之前的绕法是 `net stop "Parallels Tools Service"`（见 `.test-artifacts/rb2.cmd`），
  那是在治症状：文件会消失但 exec 通道也一起断，且 Tools 重启后可能重新出现。
- `tools/win-clicker/br-macdisk-diag.ps1` 里把这个文件记作「0-byte Parallels
  placeholder」——正常确实是 0 字节占位，开启同步后被放大到 GB 级。
  不要用 `rm` 删它了事，关掉开关它自己消失。
- prlctl 26.4.2 没有对应 CLI 开关（`--optimization` 不被识别），只能改 config.pvs。
