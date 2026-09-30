# restore-existing 闭环 PASS + 修掉「卷 GUID 形态不同导致身份校验失败」（v1.8.4）

- 日期：2026-09-30
- 版本：v1.8.4（1,801,728 B，SHA-256 `a8290b248049e302932ef9bff438c57e502162f36d9244512b131872645edf9e`）
- 任务：`b6744c45-0d7c-414c-855e-6b121d6af10b`
- 快照：`{f14d2b09-066b-4864-9272-ee04d02f423f}`

## 一、结果

`restore-existing` 闭环 **PASS**：`stage=success` / `progress=100`。

```text
STEP … 还原镜像（DISM /Apply-Image）→ 100%
data-volume restore: target has no SYSTEM hive; skipping BCDBoot
Recovery completed
new boot channel: BCD objects removed
new boot channel: staging directory F:\BackupRestoreRE removed
Boot entry cleaned; task marked successful
running wpeutil.exe reboot
```

EFI 挂载这次**一次成功**：

```text
mount EFI start: expected_guid={d08d796f-…} requested_letter=Z: disk=0 partition=2
mount EFI: assigning Z: with mountvol.exe
mount EFI complete via diskpart in 4546ms
```

（失败那两次是每个候选盘符都试一遍、每次都 mismatch，日志 20+ 行。）

## 二、根因：卷 GUID 的两种写法被当成两个卷

同一个卷在不同地方有不同写法：

| 来源 | 形态 |
|---|---|
| 任务 env / `VolumeIdentity.volume_guid` | `{d08d796f-f082-4402-bdbb-a4a6a09ac53f}`（裸 GUID） |
| `mountvol X: /L` | `\\?\Volume{d08d796f-…}\`（完整路径） |
| `GetVolumeNameForVolumeMountPointW` | 带尾反斜杠的完整路径 |

而 `verify_mounted_volume()` 直接 `actual.eq_ignore_ascii_case(expected)`——
**盘符挂成功了，但校验判为"不是同一个卷"**，于是换下一个盘符重试，
把 Z/J/K/L/M/N/O/P/Q/U/V/Y 全试一遍后整个任务失败。

错误信息 `mount EFI: every candidate drive letter is unavailable` 极具误导性：
听起来像"盘符不够"，实际是"每次都挂上了但比输了"。

## 三、修复

新增 `text_parsing::bare_volume_guid()` + `same_volume()`：
先归一化成裸 GUID 再比较，任一侧解析不出 GUID 就判为**不相等**
（宁可误判为不同而重试，也不能把说不清的两个值当同一个卷——那会写错启动项）。

改了 4 个比较点：
- `mount_env_volume` 里 listing 查找、candidate 命中
- `verify_mounted_volume` 的身份比对
- `verify_live_volume_identity` 的 VOLUME_GUID 字段（从通用循环里拆出来单独归一化）

三个单测在 macOS 上跑（cli 65 + core 23 全绿）：
`same_volume_treats_all_spellings_as_equal`、`same_volume_refuses_to_guess`、
`bare_volume_guid_extracts_from_every_spelling`。

## 四、验收（AGENTS.md：启动成功 ≠ 备份还原成功）

| 项 | 结果 |
|---|---|
| 还原后 T: 总字节 | **31,457,301** —— 与 `dism /Get-WimInfo` 报告的镜像大小 31,457,301 **逐字节一致** |
| 还原前 T: 基准 | 31,457,301（6 个文件）→ 还原后 5 项 + `System Volume Information` |
| BCD 本项目残留 | ✅ 0（`/enum all` 无 `BackupRestore` 字样） |
| 暂存目录 | ✅ `F:\BackupRestoreRE` 已删 |
| 注册位 | ✅ 未动（`harddisk0\partition4` / `b69adf69` / Enabled） |
| 回桌面 | ✅ `SYSTEMROOT=C:\Windows` |
| 任务状态 | ✅ `success` / `100` |

## 五、一个值得记的操作失误

第一次重跑仍失败，原因是**我把新 `Recovery.exe` 复制这件事漏了**：
backup 的 bat 有 `copy`，restore 的 bat 没有，于是 PE 里跑的还是 v1.8.3 的旧构建
（manifest 里 `recoverySha256` 一眼可辨）。教训：**payload 是打进 WIM 的，
改 `BackupRestore.exe` 不等于改了 PE 里跑的那份**，验证前先核对
`H:\brwork\Recovery.exe` 的哈希与 manifest 的 `recoverySha256`。

## 六、仍未闭环

1. **容量/性能**：两次都只用 T:（5 GB 卷、实占 54 MB），DISM 都是 1 秒级。
   大容量场景（v1.7.7 那次 18.9 GB）没验。
2. **`create-secondary` 方向**（第二启动项/双系统）没验。
3. **断电续跑**、BCDBoot 交互（这次 target 无 SYSTEM hive 所以跳过了 BCDBoot）、
   Secure Boot 没验。
4. **系统卷**：T: 是数据卷，走了 `data-volume restore: skipping BCDBoot` 分支；
   真正的系统卷还原（含 BCDBoot 修启动项）没验。
