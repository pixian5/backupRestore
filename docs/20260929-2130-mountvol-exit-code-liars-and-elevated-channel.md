# v1.8.0：`mountvol /S` 退出码不可信（S 盘"老是打开"的直接机制）+ 打通用户提权会话通道

- 日期：2026-09-29
- 版本：v1.8.0（1,771,520 B，SHA-256 `d61a39953bce1049dc1e92ff80161031c7f299b79e5e64cc20cbac71d56b0bc6`）
- 证据：`.test-artifacts/elev-channel/`（未入库）

## 一、为什么"S 盘老是自动打开，而且提示不可访问"

一句话：**`efi_identity()` 信了 `mountvol X: /S` 的退出码，而那个码在实机上会骗人。**

在用户提权会话里逐盘符实测（`mvwhy-result.txt`）：

| 盘符 | `/S` 退出码 | 紧随的 `/L` 回显 |
|---|---|---|
| `Z:` | 1（看起来失败） | `\\?\Volume{d08d796f-…}\` ← **其实挂上了** |
| `Y:` | 0（看起来成功） | 同一个卷 ← 已在挂载态，重复 `/S` 无事可做 |
| `X:` | 0 | 同一个卷 |
| `W:` | 0 | 同一个卷 |

旧代码的逻辑是"退出码非 0 → 这个盘符不行，换下一个"。于是 Z: 明明已经挂上 ESP，却被判
失败；循环继续拿 Y: 再挂一次同一个卷……**自动播放只负责"打开"，反复重挂负责"老是"**。
而每轮用完 `mountvol /D` 撤销盘符时，已经弹出的资源管理器窗口就指向一个不存在的盘符，
于是显示"不可访问"且不自动关闭。

这与另一份文档 [202609292014](20260929-2014-S盘自动打开问题定位与修复方案.md) 是同一现象
的两个互补解释：那一份讲清了"为什么打开"（自动播放
`UnknownContentOnArrival → MSOpenFolder`）与"为什么不可访问"（用完即卸），本份讲清"为什么
**老是**"（退出码谎言导致每轮重复挂载）。两者都已补测确认。

## 二、修复

1. **`efi_identity()` 不再看 `/S` 退出码**，发起挂载后用 `mountvol X: /L` 是否回显卷路径
   判定成功。判据逻辑抽成纯函数 `text_parsing::mountvol_listing_has_volume()`，配套单测
   `mountvol_listing_reports_volume_presence_from_l_not_s_exit_code` —— 放在 macOS 也编译的
   模块（延续 v1.7.12 的教训：`#[cfg(windows)]` 里的单测在开发机上一次都不跑）。
2. **PE 侧统一卸载**：`pe_task_execute()` 跑完任务、写完 `pe-task-result.txt`、改好 `.done`
   之后执行 `mountvol S: /D`。原来 PE 挂上 S: 后从不卸载，Windows 侧再看到的就是一个
   内容会突然失效的窗口。
3. **`letter as u8 as char` → `char::from_u32(...).filter(is_ascii_alphabetic)`**：
   前者在 u16 > 255 时静默截断成错盘符（`native_gui.rs` `exit_pe_to_windows`）。

## 三、完整主路径首次在产品内跑通

之前两次 prepare 都靠 `--test-efi-drive Y` 绕开 `efi_identity()`，因为 `prlctl exec` 的
SYSTEM 会话跑 `mountvol /S` 会**挂起不返回**，把通道堵死。

本轮打通了用户提权会话通道（`schtasks /rl highest /it`，用户 `x`，SID 1000，
High Mandatory Level），于是能跑真主路径。任务 `ddce302b-d1f1-4191-ad8b-1fae9cfd14d3`：

```text
[13:05:16] capturing bcdedit.exe /store S:\EFI\Microsoft\Boot\BCD /enum all /v
[13:05:16] Captured byte-for-byte EFI BCD snapshot
[13:05:23] new boot channel: payload staged at F:\BackupRestoreRE\Winre.wim sha256=1060a552…
[13:05:23] new boot channel: device options {6e1d0cf7-bc06-11f1-8900-b4d663a47373}
[13:05:23] new boot channel: loader {6e1d0cf8-bc06-11f1-8900-b4d663a47373}
[13:05:24] new boot channel: readback device=ramdisk=[F:]\BackupRestoreRE\Winre.wim,{6e1d0cf7-…}
                            osdevice=ramdisk=[F:]\BackupRestoreRE\Winre.wim,{6e1d0cf7-…}
[13:05:48] Task prepared with --no-reboot; registered WinRE unchanged
```

首行的 `/store S:\EFI\Microsoft\Boot\BCD` 说明产品自己完成「挂 ESP → 读身份 → 卸盘符」全链；
终态盘符只剩 `C D F H P T`，临时盘符已正常卸掉。

## 四、清理

- 删掉本次自建 BCD 对象 `{6e1d0cf8}` / `{6e1d0cf7}`；**等满 90 秒再重启**（v1.7.12 那轮踩过：
  删完立刻软重启会因 ESP 写入未 flush 而"复活"）。
- 删除 2 个旧诊断快照 `{61adc34b-…}` / `{8c011708-…}`（旧通道诊断轮，记录文件已被先前会话
  清理，无保留价值）。链根 `{e9b6419a-…}` **保留**——它无父无记录，删它风险最大。
  现存 6 个：根 + `538467a0`（v1.7.12）+ `148b4c1a`（v1.7.13）+ `cc4df418`（v1.7.14）+
  `192cb4b7`（E1）+ `55eb434e`（v1.8.0 主路径，当前回退点）。
- 终态复核：本项目 BCD 残留 0、无 `bootsequence`、注册位仍是
  `harddisk0\partition4` / `b69adf69`、重启后 `SYSTEMROOT=C:\Windows`。

## 五、仍未闭环

1. **方案 A（卷 GUID 路径、零临时盘符）未实施** —— 卡在 §5.2 的写路径归一化风险：
   卷路径打开时 `device` 显示 `partition=\Device\HarddiskVolume2`，盘符方式显示
   `partition=S:`，若写操作也归一化就可能改写 ESP 设备引用。E1 实验脚本已备好
   （`.test-artifacts/elev-channel/e1full.ps1`），但 PowerShell 通道在计划任务会话里
   多次静默失败，未取到完整结果，**E1 尚未定论**。
2. ESP 根目录仍堆着 38 个开发日志（约 93 KB，09-11 至 09-26）——引导分区不该这么用。
3. restore 方向、断电续跑、BCDBoot 交互、Secure Boot、还原目标 WinRE 注册语义。
