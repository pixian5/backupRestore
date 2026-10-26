# 在线还原主链首次实机验证：`RESTORE_SELFCHECK` 从 0 到 1（v2.6.64 时点）

- 时点：2026-10-27 00:10 · HEAD `567651c` · 版本 **保持 2.6.64**（本轮未改源代码）
- 目标：未完成项 **B「真实还原一轮」**——`RESTORE_SELFCHECK` 与空间预检闸门
  自 2.6.54 起**没在实机跑过一次**
- 结论：**用在线数据卷还原完成了 B 的前半部分**，零重启、零引导变更、零快照

---

## 1. 为什么换路径（决策依据，不是偷懒）

B 一直被估成「1–2 小时 + 拍快照 + 87 GB」，因为默认它必须走
**离线恢复环境全系统还原**。跑之前先量了一下宿主：

```
/dev/disk3s5   926Gi   784Gi   102Gi    89%
```

**宿主只剩 102 GiB**。而 87 GB 级还原要求宿主空闲 ≥ 镜像 × 2.5 ≈ **220 GiB**
（CoW 增量模型）。也就是说此刻做全系统还原会重演 10-12「磁盘吃满 → VM 静默
停机 → 连日志都读不了」的事故。

于是先去查这两个闸门到底落在哪条路径上——**如果在线还原就能覆盖，就不必重启**。

## 2. 关键发现：在线数据卷还原就会写 `RESTORE_SELFCHECK`

`restore_selfcheck_line` 有 6 个调用点，其中 `online_operation.rs:521` 正是
`restore-existing` 的**在线数据卷**分支，代码注释写明：

> 数据卷还原时 ntoskrnl 本就不存在，字段如实记 false 即可（判据是序列号）

所以它**不需要进恢复环境**。`online_operation.rs:838` 是 CreateSecondary（第二系统）
分支，那个才装完整 Windows。

### ★ 顺带订正我 10-26 分析里的一处口径错误

我在「整体分析」里写「2.6.54 的**空间预检硬闸**……算大了就会拒绝一次**合法还原**」——
**错的**。`BR-IMG-008 BackupSpaceInsufficient` 是**备份**预检（镜像卷可用空间 <
`backup_space_floor(源已用)` = 已用 ÷ 20 且 ≥ 1 GiB），卡的是**备份**不是还原。
还原侧没有这道闸。本轮验证的是「备份预检不误拦」。

---

## 3. 实测过程（T: `BRTEST` 5 GB 测试卷）

### ★ 顺序陷阱：哨兵必须在**备份之后**才放

哨兵文件若先放上去，会被打进镜像，还原后它又「回来」——
「哨兵消失」这条判据就自相矛盾了。正确顺序：

```
造数据 → 备份（镜像里含数据）→ 布哨兵 → 还原（格式化）→ 验收
```

### 步骤与结果

| 步 | 命令（摘要） | 结果 |
|---|---|---|
| 1 造数据 | 在 `T:\brtest-data` 写 3 个 `BR-DATA-000423-*.txt` + `BR-MARKER.txt`（内容 `MARKER-000423`） | 4 个文件 |
| 2 备份 | `online --operation backup --source-drive T --image-path H:\BRImages\brtest-000423.wim --compress fast --json` | `success:true` **`error_code:null`** → 空间预检**未误拦**；镜像 1988 字节 |
| 3 布哨兵 | `tools/restore/sentinel.sh arm --drive T` | `BR-000441.txt`，`serial_before=0xc0338a4c` |
| 4 还原 | `online --operation restore-existing --target-drive T --image-path …wim --index 1 --format-target --json` | `success:true`，`error_code:null` |
| 5 验收 | `sentinel.sh verify --drive T` | **`VERDICT=PASS`** |

### 三条外部证据（不依赖产品自述）

```
SENTINEL_GONE=1                     哨兵消失（BR-000441.txt 不在了）
SERIAL_BEFORE=0xc0338a4c → AFTER=0x76bb956d    序列号变化（fsutil 外部读）
T:\brtest-data\BR-MARKER.txt → MARKER-000423   镜像内容真的写回去了
```

第三条是**正向证据**：哨兵消失只能证明「卷被清了」，
`MARKER-000423` 原样回来才证明「镜像内容真的被写进去了」——否则一个空卷也能 PASS。

### `RESTORE_SELFCHECK` 首次出现

```
RESTORE_SELFCHECK target=T serial_before=C0338A4C serial_after=76BB956D ntoskrnl=false
```

全机日志历史命中：`TOTAL_HITS=1`（此前为 0，与 10-26 分析一致）。

★ 更值钱的一点：产品自报的序列号对（`C0338A4C → 76BB956D`）与**外部仪器**
`fsutil` 读到的（`0xc0338a4c → 0x76bb956d`）**完全一致**。
这不是「同一台仪器自洽」，而是两条**不同代码路径**的交叉验证——
按判据纪律，只有这样才能算证明。

### 失败路径的错误码也顺手验了

```
online … --image-path H:\BRImages\NOT-EXIST-000423.wim --json
{"success":false,"error":"[BR-PE-039] online operation did not complete successfully",
 "error_code":"BR-PE-039"}
```

码既在 `error_code` 字段、也带在文案里。CLI 侧契约成立。

---

## 4. 未验到的（如实记录）

1. **离线恢复环境路径仍未跑**。本轮只覆盖了在线分支；`main.rs:4371/4386`
   那两处 `append_restore_selfcheck`（PE/WinRE 内）依旧零命中。
   要跑它必须重启 + 87 GB，而**宿主现在只有 102 GiB**，条件不具备。
2. **空间预检只验了「不误拦」方向，没构造「拦下」方向**。
   要触发需要镜像卷可用 < 1 GiB，而本机唯一的小卷是 `S:`(0.3G) 与
   `Y:`(WINRE 0.8G)——这两个都是系统卷，按环境纪律不能碰。
3. **`--format-target` 的反向（不格式化）未跑**：不格式化时序列号不变，
   哨兵法会判 `WARN`（只满足哨兵消失一条），这是设计使然，非缺陷。

---

## 5. 结论与下一步

- B 项**不再是「必须 1–2 小时 + 整机快照」**：在线数据卷还原这条低成本路径
  已经能覆盖 `RESTORE_SELFCHECK` 的在线分支与备份空间预检。
- 剩下真正需要重启的只有「离线分支 + 引导修复」那一半，**等宿主腾到 ≥ 220 GiB 再做**。
- 哨兵法（`tools/restore/sentinel.sh`）此前只在 `T:` 上用 `format` 模拟过判定器
  三态；本轮是**第一次跑真实还原链路**，PASS 分支实机成立。

不升版本：本轮**未改任何源代码**，纯验证与记录。
清理：测试卷 `T:\brtest-data` 已删；镜像 `H:\BRImages\brtest-000423.wim` 留作证据。
