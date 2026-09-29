# 新 PE 式通道第一次完整备份闭环 PASS（v1.8.3 实机）

- 日期：2026-09-30
- 版本：v1.8.3（manifest `createdByVersion: 1.8.3`）
- 任务：`f62ff42a-778c-42b9-8944-08f7ae3a2950`
- 快照：`{612bd761-ad3a-46c6-a09d-d4407a0e94a2}`（闭环前建并核验）
- 证据：`.test-artifacts/elev-channel/loop-recovery.txt`（762 行完整日志）

## 一、这是第一次「产品自己走完」

此前 11 个任务全部停在 `prepared` 阶段——那是 v1.7.11~v1.8.3 一直在修的**准备层**。
本轮第一次跑不带 `--no-reboot` 的 prepare，让它自己 arm + 重启，**完整闭环跑通**。

## 二、四步全部完成（PE 内，16:28:03）

```text
STEP 1/4 准备备份环境（挂载卷、校验）
STEP 2/4 捕获系统分区镜像（DISM）→ The operation completed successfully
STEP 3/4 校验镜像并计算哈希、写入元数据
Boot entry cleaned; task marked successful
running wpeutil.exe reboot
```

注意第 4 步是 **`Boot entry cleaned`** 而不是旧通道的 `WinRE cleanup completed`——
这正是新通道的预期行为：注册位全程只读，收尾只清理我们自己建的一次性启动项。

## 三、终态核验（AGENTS.md：启动成功 ≠ 备份还原成功）

| 项 | 结果 | 证据 |
|---|---|---|
| 镜像文件 | ✅ 可用 | `F:\brimg\loop-t.wim` 258,200 B；`dism /Get-WimInfo` 能读，索引 1，名称 `Windows Backup` |
| 镜像元数据 | ✅ | `imageSha256=6a3ee294…`、`capturedUsedBytes=54,374,400`、`programVersion=1.8.3`、源卷 `950bd694…`（disk 0 partition 5） |
| BCD：bootsequence | ✅ 已清 | `/enum {bootmgr} /v` 无 bootsequence |
| BCD：本项目条目 | ✅ 已删 | `/enum all /v` 无 `BackupRestore` 字样 |
| 注册位 | ✅ 未动 | `reagentc /info`：位置 `harddisk0\partition4`、标识 `b69adf69-bbe4-11f1-88fa-ff604011526d`、Enabled |
| 回桌面 | ✅ | 重启后 `SYSTEMROOT=C:\Windows` |
| 任务状态 | ✅ | `status.json`: `stage=success`, `progress=100` |

PE 侧挂载三卷全部走通（`Recovery-early.log`）：
`RECOVERY` → 预挂载的 C:、`SOURCE` → D:、`IMAGE` → G:，均经 `mountvol listing` 定位，
未分配新盘符。

## 四、闭环前清掉的两处遗留

跑之前发现 BCD 里有一条**指向不存在 WIM 的遗留条目**（`F:\BackupRestoreRE\Winre.wim`
已被前面几轮清理删掉，条目还在），另有一个早前 E2 实验留下的 `BRTest`。
两条都已删除并等 60 秒落盘。

教训：**清理暂存文件时要连 BCD 条目一起清理**，否则留下一个指向空气的启动项——
一旦被 arm，机器直接进不了系统。

## 五、这次测得很轻，别当成性能/容量验证

- 备份源 T: 是 5 GB 卷，实际只用了 54 MB，所以镜像只有 258 KB、DISM 一步 1 秒完成。
- **没有验证**：大容量场景（v1.7.7 那次是 18.9 GB）、`restore-existing` 方向、
  `create-secondary` 方向、断电续跑、BCDBoot 交互、Secure Boot。
- 镜像虽能 `Get-WimInfo`，但**尚未 restore 回去验证内容一致**。

## 六、下一步

1. **restore 方向闭环**（把 `loop-t.wim` 还原到某个测试卷）——这是产品另一半主场景，
   也是 v1.7.7 方案 D 解锁的那个组合。
2. 哈希比对（还原后与源逐项一致）。
3. 之后才是断电续跑、BCDBoot、Secure Boot。
