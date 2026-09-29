# 真实 C: 系统卷还原 PASS —— 系统卷分支（含 BCDBoot）首次跑通（v1.8.4）

- 日期：2026-09-30
- 任务：`b487a545-8c31-489f-881f-a2514dca97cb`
- 用户授权：明确选择「A：还原真实 C:」（消耗 C: 只做最终验收一次的额度）
- 快照：`{c5618191-8ed3-4c72-9484-12a015d7a919}`（还原前建并核验）
- 镜像：`F:\c-real.wim`（09-28 采自 C:，33,448,442,006 B）

## 一、预检（毁 C: 之前必须做的）

| 项 | 结果 |
|---|---|
| 镜像 SHA-256 vs 元数据 `imageSha256` | ✅ **完全一致** `d02034ec4adf40e10d826122fd30951cf5e57bd5c56a7a2770bae23c19718ef4` |
| 镜像大小 vs `imageSize` | ✅ 33,448,442,006 一致 |
| 快照 | ✅ `{c5618191-…}` 已建并核验 |
| C: 基线 | 67,003,346,920 B（`C:\Windows` 38,744,834,737 B） |
| exe 位置 | `H:\brwork`（C: 之外，不会被还原打掉） |

## 二、完整链路（PE 内，17:29:49 → 17:36:34，约 7 分钟）

```text
mount EFI complete via diskpart in 4755ms              ← 卷 GUID 归一化修复后一次成功
Formatted target partition identity re-verified
running dism.exe /Apply-Image /ImageFile:G:\c-real.wim /Index:1 /ApplyDir:C:\
running bcdboot.exe C:\Windows /s Z:\ /f UEFI /v
Verified BCD device/osdevice points to target c:        ← 系统卷分支的关键验证
Recovery completed
new boot channel: BCD objects removed
new boot channel: staging directory F:\BackupRestoreRE removed
Boot entry cleaned; task marked successful
running wpeutil.exe reboot
```

这是与数据卷还原的**真正差别**：数据卷走
`data-volume restore: target has no SYSTEM hive; skipping BCDBoot`；
系统卷会跑 BCDBoot 修启动项并回验 `device/osdevice`。

## 三、验收

| 项 | 结果 |
|---|---|
| 任务状态 | ✅ `stage=success` / `progress=100` |
| C: 能否启动 | ✅ 重启后 `SYSTEMROOT=C:\Windows` |
| C: SYSTEM hive | ✅ 存在（系统卷） |
| C: 字节数 | 66,885,360,671（基线 67,003,346,920，差 117,986,249 B ≈ 112 MB） |
| BCD 本项目残留 | ✅ 0（无 `BackupRestore` 字样、无 `bootsequence`） |
| BCD WinRE 条目 | ✅ 新注册自洽：`reagentc` 标识 `f530b9e0` ↔ `recoverysequence f530b9e0` ↔ 设备选项 `f530b9e1` |
| bootmgr default | `{4fe78777-…}`（BCDBoot 新建，指向还原后的 C:） |

字节数差异是**预期的**：镜像是 09-28 采的，还原后 C: 回到那个时点，
之后累积的部署包、日志等（约 112 MB）消失。已重新部署 v1.8.4
（`a8290b24…`）到 `C:\Users\Public\backupRestore-package` 并核对哈希。

## 四、一个必须处理的副作用：WinRE 被镜像带成 Disabled

还原后 `reagentc /info` 显示：

```text
Windows RE 状态:  Disabled
Windows RE 位置:  (空)
Windows RE 版本:  0.0.0.0
```

原因：**09-28 采那个镜像时，C: 的 WinRE 注册位本就处于异常态**
（文档 `20260929-093132` 记过"连续直测 C: 的代价：WinRE 变 Disabled/注册位空/暂存卷被清"）。
新通道全程只读注册位、不写它，所以这个状态是被镜像带回来的，不是本次还原造成的。

处置：`reagentc /enable` 后恢复
`Enabled` / `\\?\GLOBALROOT\device\harddisk0\partition4\Recovery\WindowsRE` / 10.0.26100.8031。

**这条对产品有真实影响**：新通道要把注册位的 `Winre.wim` 当只读资产来源
（`registered_winre_templates()` 读 `reagentc /info`），WinRE Disabled 时 prepare 直接失败。
属于"镜像时点状态被还原"的固有代价，不是代码缺陷——但值得在产品里给出更明确的报错。

## 五、清理（按用户要求每轮清）

- 任务目录：只留证据目录 `b487a545`，其余全删
- `H:\brwork`：只留两个 exe
- 快照：本轮开始已把 7 个裁到 2 个；本次为 C: 还原新建 `{c5618191}`，
  与 `{f14d2b09}` 共 3 个
