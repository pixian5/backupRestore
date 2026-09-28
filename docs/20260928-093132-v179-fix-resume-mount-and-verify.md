# v1.7.9：修复续跑挂载 BUG（volume has no disk number）并实机闭环验证

> 日期：2026-09-28
> 性质：BUG 修复 + 实机闭环验证 **PASS**
> 相关：[20260928-085000-v178-vm-verify-partial-and-bug.md](20260928-085000-v178-vm-verify-partial-and-bug.md)（BUG 发现）、
> [20260928-074516-registered-winre-policy-and-resume-design.md](20260928-074516-registered-winre-policy-and-resume-design.md)（状态机设计）

## 1. 根因（比上一份文档的猜测更精确）

上一份文档猜测「resume 路径复用了 disk_number 强制语义」。实机取证后根因更简单：

`RecoveryTask.env` 里 RECOVERY 身份的实际内容（实机读取）：

```
RECOVERY_DISK_GUID       = {8d3d3cd3-...}        ← 有
RECOVERY_PARTITION_GUID  = {876b8d3d-...}        ← 有
RECOVERY_VOLUME_GUID     = \\?\Volume{876b9...}\ ← 有
RECOVERY_DISK_NUMBER     = （空）                 ← 空！
RECOVERY_PARTITION_NUMBER= （空）                 ← 空！
```

两个环节叠加成 BUG：

1. **读取端丢信息**：`recovery_volume_from_env`（main.rs）只读了 DISK_GUID/PARTITION_GUID
   两个键，把 env 里已有的 `RECOVERY_VOLUME_GUID`、`RECOVERY_DISK_NUMBER`、
   `RECOVERY_PARTITION_NUMBER` 全部丢弃，构造出的 identity 只有 GUID。
2. **挂载端只有一条路**：`ensure_volume_mounted`（windows_prepare.rs）在没有
   drive_letter 时**强制要求 disk_number + partition_number** 走 DiskPart；
   identity 里 disk_number=None → 直接报 `volume has no disk number`。

（disk_number 为空的原因：RECOVERY 身份是按 GUID 构造的注册位身份，本机 WinRE
注册在 disk0/partition4 = C: 的 OS 分区上，prepare 侧从未为它查过数字坐标。）

## 2. 修复（v1.7.9，两处）

### 2.1 读取端补齐（main.rs `recovery_volume_from_env`）

把 env 里已有的字段全部带进 identity：volume_guid、disk_number、partition_number、
partition_offset、partition_size。空值/坏值 parse 失败 → 保持 None，不影响
`same_partition`（只比两个 GUID）的既有语义。

### 2.2 挂载端三级降级（windows_prepare.rs `ensure_volume_mounted`）

```
① identity.drive_letter 已有            → 直接用
② 枚举已挂载盘符 C..Z，按 disk/partition GUID（或 volume GUID）匹配 → 复用该盘符
   （最常见：注册 WinRE 就在 C:\Recovery\WindowsRE，C: 本来就挂着）
③ 有 volume_guid → mountvol <letter>: \\?\Volume{...}\ 挂载 + 挂后校验
④ 有 disk_number+partition_number → 原 DiskPart 路径（最后手段）
全失败 → 明确报错（不再是误导性的 "no disk number"）
```

新增辅助函数 `mounted_letter_for_identity`（GUID 匹配已挂载盘符）与
`mount_volume_guid`（mountvol 按 GUID 挂载，赋盘后回读 `volume_identity`
确认盘符真的指向目标卷）。

## 3. 实机闭环验证（PASS，铁证）

测试构造：任务 `6571753e`（backup T: → E:\brimg\v178-res3.wim）用
`--test-fault power-loss-window` 停在 durable `boot-requested` 态 →
手动把注册位覆写为 original（`1060A552`，模拟翻转后死局）→ 部署 v1.7.9 →
SYSTEM 直跑 exe 触发 resume。

`prepare.log` 时间线（同一任务、同一文件，BUG 前后对比）：

```
00:58:16  Pending boot recovery abandoned: ... volume has no disk number   ← v1.7.8 ❌
01:02:32  Pending boot recovery abandoned: ... volume has no disk number   ← v1.7.8 ❌
01:21:11  Detected durable interrupted task ... resuming (v1.7.9)          ← 触发
01:21:16  Task payload re-registered as WinRE for resumption               ← 修复 ✅
01:21:16  Re-requested one-time Windows RE boot                            ← 重武装 ✅
01:21:16  Restart requested to resume pending task in Windows RE           ← 重启
01:22:13  status.json: stage=success progress=100                          ← 自动跑完 ✅
```

终态核验：
- 注册位哈希 = `1060A552…`（= original，不变量 C「终态干净」成立）✅
- 镜像 `v178-res3.wim`（4,058,984,853B）产出、可挂载、内容即源卷 T: ✅
- 顺带确认：v1.7.8 两次失败均正常 release 了重试额度（main.rs:450 本来就有
  release，上一份文档「resume 失败烧掉重试额度」的说法**不成立**，订正）。

## 4. 验证方法沉淀（可复用）

1. `--test-fault power-loss-window` 造 durable boot-requested 态（不重启，留操作窗口）。
2. `copy <task>\original\Winre.wim → C:\Recovery\WindowsRE\Winre.wim` 无损构造
   「翻转后死局」（original 是合法 WinRE，即使 resume 不触发也只是回原版菜单）。
3. SYSTEM 直跑 `H:\brwork\BackupRestore.exe`（无参）即可触发 resume——它会修复、
   重武装并重启，然后自动完成整个周期。
4. VM 磁盘太快（3.5GB 捕获 ~1-2 分钟），轮询抢断电窗口不可行；上述注入法零竞态。

## 5. 版本与清理

- 版本 1.7.8 → **1.7.9**；产物 `BackupRestore.exe` = 1,715,712B（SHA `C9E343FF…`）。
- `H:\brwork` 与 `C:\Users\Public\backupRestore-package`（GUI 自启点）均已更新到 v1.7.9。
- VM 测试残留已清：T:\fill.bin、E:\brimg 四个 v178*.wim + metadata、本次 3 个任务目录、C:\brmount。
- 交叉编译通过；`cargo test -p backuprestore-core` 28 全绿。

## 6. 遗留

- 无阻塞项。可选优化：`insert_identity` 在 disk_number 为 None 时写空串而非
  `unwrap_or_default()` 的 "0"（当前读取端已兼容两种写法，改写入端纯属一致性）。
