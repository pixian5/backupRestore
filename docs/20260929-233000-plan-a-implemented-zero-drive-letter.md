# v1.8.2：方案 A 已实施并实机跑通——一次准备任务的临时盘符挂载降到 0

- 日期：2026-09-29
- 版本：v1.8.2（**DEPLOYED SHA256=041008f934c28a1c8c511086a5f2db16089811dd30b2372fdebf4e0bc351d8f9**）
- 快照：`{a9e12d9d-e009-4692-9174-f4045cd0f972}`（改启动项前按 AGENTS.md 建并核验）
- 证据：`.test-artifacts/elev-channel/`（未入库）

## 一、结果

准备 backup（源 T: → 镜像 F:，`--no-reboot`）实机跑通，日志四行：

```text
plan A: verbatim BCD store path = \\?\Volume{d08d796f-...}\EFI\Microsoft\Boot\BCD
Captured byte-for-byte EFI BCD snapshot
new boot channel: readback device=ramdisk=[F:]\BackupRestoreRE\Winre.wim,{7ba02c8b-...}
Task prepared with --no-reboot; registered WinRE unchanged
```

**盘符前后完全一致（`C D F H P T`）**：ESP 临时盘符一次都没分配 →
没有「卷到达」事件 → 没有自动播放弹窗 → 也没有「不可访问」。
清理后重启复核：本项目 BCD 残留 0、无 `bootsequence`、注册位仍是
`harddisk0\partition4` / `b69adf69`、`SYSTEMROOT=C:\Windows`。

## 二、改了什么

| 层 | 函数 | 作用 |
|---|---|---|
| core | `VolumeIdentity::volume_path()` | 卷 verbatim 路径，方案 A 基石 |
| prepare | `efi_bcd_store_path()` | ESP 的 BCD 存储路径，优先零盘符；`snapshot_raw_bcd` / `rollback_boot_request` 改用 |
| prepare | `volume_identity_at_path()` + 三个 `_at` | 不分配盘符读卷身份 |
| prepare | `esp_identity_without_drive_letter()` | 零盘符定位引导 ESP；`efi_identity` 先试它，失败才走 mountvol 循环 |
| text_parsing | `volume_path_to_device_path()` | 卷路径 → 设备路径（**必须转**） |
| text_parsing | `esp_volume_from_listing()` | 从 `mountvol` 输出里挑未挂载卷 |
| GUI | `esp_volume_path_for_task()` | 两处写 `pe-task.txt` 改零盘符，不再 `mountvol S: /S` |

## 三、三个实机才暴露的坑

1. **卷路径 ≠ 设备路径**：拿 `\\?\Volume{GUID}\` 直接 `CreateFileW` + `DeviceIoControl`
   会报 161 / 123。卷 API 吃 `\\?\Volume{GUID}\`，设备 API 吃 `\\.\Volume{GUID}`。
2. **`?` 后少一个反斜杠**：`volume_path()` 曾拼出 `\\?Volume{...}`（Windows 不认）。
   单测当初是**照实现抄**期望值的所以没拦住；现已改为独立构造期望值，
   并把前缀四个字符单独断言。
3. **后缀反斜杠层数**：`storage_device_number` / `physical_volume_identity` 的盘符转发串
   在 raw string 里多写一层，实际路径变成 `\\\\.\H:`。

共同教训：这类错误在日志里只表现为 `CreateFileW 161`，不给线索。
现在 `open_device` 报错带完整路径；三个路径相关函数都有独立期望值的单测
（cli 61 + core 23 全绿）。

## 四、还没做

PE 侧三处 `mountvol S: /S`（`clean_bootsequence`、`verify`、以及 7702 附近）——
PE 启动最早期没有 GUI 那套辅助函数，改动面不同，列为下一阶段。
