# 断电续跑闭环 PASS —— 顺带修掉两个真缺陷（v1.8.5 / v1.8.7）

- 日期：2026-09-30
- 任务：`ed7afb6d-f644-4e5f-bdf9-4dcf8d25e7a1`
- 故障点：`power-loss-image-applied`（镜像灌完、启动项未修的最深断电点）
- 快照：`{4934b6f1-4f6f-44fa-b6a1-e01efd11f862}`

## 一、结果

完整链路四步全对：

```text
19:14:27  payload hash a0327e92… → refreshing the boot entry        ← v1.8.5 修复
19:14:47  diskpart format target
19:14:51  dism /Apply-Image → 落盘 stage=image-applied/75
          ── 此处模拟断电（wpeutil reboot，故意不清理）──
19:15:51  GUI: Detected durable interrupted task … resuming
19:15:56  new boot channel: one-shot bootsequence re-armed
19:16:14  Recovery.exe started … Apply-Image … Boot entry cleaned
          stage=success / progress=100
```

验收：T: 还原后 **31,457,301 字节，与镜像逐字节一致**；三个 marker 文件全部消失；
BCD 无本项目对象、无 `bootsequence`；暂存目录 `F:\BackupRestoreRE` 已清空；
注册位未动（`harddisk0\partition4` / `f530b9e0` / Enabled）。

## 二、这条路上连抓两个真缺陷

### 缺陷 1（v1.8.5 修）：续跑对所有任务都是死的

```text
Pending boot recovery abandoned: our boot entry is unusable:
  payload WIM hash differs from the prepared one; refusing to re-arm
```

`create_entry` 按设计在 DISM 注入**之前**跑（v1.7.11 顺序要点），
`boot-entry.json` 记的是注入前干净原件的哈希 `1060a552`；注入后载荷 WIM 变成
`a0327e92`，记录没刷新 → `rearm()` 必然拒绝 → 机器永久停在 75%。

修复 `boot_entry::refresh_payload_hash()`：注入完成并覆盖到镜像卷之后刷新簿记（幂等）。
修复后日志明确出现 `payload hash is now …; refreshing the boot entry`。

### 缺陷 2（v1.8.7 修）：SOURCE 序列号校验把续跑挡死

```text
mount SOURCE complete via pre-mounted letter D:          ← 卷 GUID 对得上、挂载成功
SOURCE volume serial differs after mounting              ← 序列号校验拒了
```

`restore-existing` 的硬约束是 **target == source（同一分区）**，而 restore 会
DiskPart 格式化 target → source 的卷序列号跟着变。

PE 侧 TARGET 早就为此放行（`TargetErased/ImageApplied/BootRepaired` 三个 Stage），
**SOURCE 却一直传 `false`**。同一个分区，TARGET 允许变而 SOURCE 不允许，本身就矛盾。

修复：SOURCE 用同一条 `reformatted_ok` 条件。

## 三、一个测试设施缺陷（顺带发现）

`interrupt_after_stage_if_requested` 的防重入标记是
`log.with_file_name(".fault-<name>.triggered")`——**按日志路径定位**，
而 `F:\brimg\Recovery.log` 是跨任务共享的。于是同一故障**一生只能触发一次**：
我第二次重测时故障静默不触发，任务一次跑完，差点被误判成"续跑修好了"。

下次要重测必须手工删 `F:\brimg\.fault-power-loss-image-applied.triggered`。
**这个标记应该按 task_id 定位**，否则每个新任务都继承上一个任务的"已触发"状态。
本次未修（属于测试设施，不影响产品路径），已记为待办。

## 四、仍未闭环

- 容量/性能（T: 5 GB 实占 54 MB，DISM 都是秒级）；
- `create-secondary`（双系统）方向；
- Secure Boot；
- 新进度窗口（v1.8.5）还没在 PE 里实看；
- `--test-fault power-loss-target-erased` / `power-loss-boot-repaired` 另两个断电点。
