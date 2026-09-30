# VM 引导修复：新建 VM 挂旧盘方案（2026-09-13）

## 现象

Parallels Windows 11 VM（`Windows 11.pvm`）在空间治理（merge+compact 12 层快照 → 单层 62G）后正常启动过一次，随后用户操作挂起（suspended），再启动失败：

- 固件主菜单正常，但 Boot Manager 里**只有「UEFI Shell」**，没有「Windows Boot Manager」。
- Parallels 弹「在该虚拟机中没有安装操作系统」，点确定后再次出现。
- `prlctl reset` 硬重启后依旧。

## 根因

- 磁盘数据**完好**：`prl_disk_tool check` 100% 通过，`C:\Users\x`、C 盘 192GB 可用等均与 compact 后一致。
- 损坏的是 **EFI 引导变量（Boot0000 / BootOrder）**，存储在 VM 的 NVRAM（固件存储）里，挂起/恢复过程中丢失。
- 现象佐证：NVRAM 重置（删 NVRAM.dat/tnvs 由 Parallels 重建）无效、Secure Boot 关闭（config.pvs `EfiSecureBoot` 1→0）无效，Boot Manager 仍只有 UEFI Shell。

## 关键死路

固件菜单里需要键盘导航（进 Boot Maintenance Manager → Boot From File），但：

- `mac_computer_use_tool` 的按键**无法传入 Parallels VM 窗口**（VM 窗口层不转发宿主按键到固件）。
- `prlctl send-key-event --scancode`（十进制：Enter=28 / Down=80 / Esc=1 / Tab=15）**在弹窗出现前有效**（曾成功进 Boot Manager），但 Parallels 判定「无操作系统」后弹模态窗，**弹窗存在期间 send-key-event 被 Parallels 拦截，键完全无效**。
- CLI 光驱：`--device-set cdrom0` 报「设备不存在」（Parallels 27 光驱是动态设备，config.pvs 里无显式 CdRom 块）。

## 解决方案：新建 VM + 挂旧盘（推荐，已成功）

思路：旧盘引导链（ESP + bootmgfw.efi）其实是好的，只是旧 VM 的 NVRAM 引导变量坏了。新建一个 VM 获得**全新干净的 NVRAM**，把旧盘作为硬盘挂进去，干净固件直接扫描到 ESP 引导文件 → Windows 直接启动，**无需 PE 修复**。

命令：

```bash
# 1. 创建新 VM（无硬盘，win-11 模板）
prlctl create "Win11-repair" --ostype win -d win-11 --dst /Users/x/Parallels --no-hdd

# 2. 挂旧盘（引用 Windows 11.pvm 的磁盘，不复制，不占额外空间）
prlctl set "Win11-repair" --device-add hdd \
  --image "/Users/x/Parallels/Windows 11.pvm/harddisk.hdd" --connect

# 3. 挂 PE ISO 到光驱（备用，实际未用到）
prlctl set "Win11-repair" --device-add cdrom \
  --image "/Users/x/code/backupRestore/artifacts/BackupRestorePE.iso" --connect
prlctl set "Win11-repair" --device-set cdrom0 \
  --image "/Users/x/code/backupRestore/artifacts/BackupRestorePE.iso" --connect

# 4. 启动
prlctl start "Win11-repair"
```

新 VM 默认 Boot order 为 `cdrom0 usb hdd0 cdrom1`（光驱优先）；本次实际是固件扫到 hdd0 的 ESP 直接引导 Windows，PE ISO 未参与。

## 验证（通过）

- `prlctl exec "Win11-repair" cmd /c ver` → `Microsoft Windows [版本 10.0.26200.9168]`
- C 盘卷序列号 `3C13-10DF`，可用 206,760,538,112 字节 ≈ **192.6 GB**（与 compact 后一致）
- `C:\Users\x` 存在（用户主目录完好）
- 桌面图标（BackupRestore、ADK_28000.1、摸鱼、Edge/Chrome 等）完整

## 遗留事项

1. **旧 VM（Windows 11）**：已停（引导损坏、磁盘被新 VM 引用，不能同时启动）。磁盘文件仍留在 `Windows 11.pvm/harddisk.hdd`，新 VM 引用之。后续可考虑把磁盘目录迁移进新 VM 目录并删除旧 VM（同卷 rename 瞬时，62G 不占额外空间），**迁移前先确认新 VM 稳定**。
2. **Windows 激活**：新 VM UUID 与旧 VM 不同，Windows 视为新硬件，激活可能失效（开发测试机影响小，可接受）。
3. **Parallels Tools**：提示「需要更新」，重启或关闭 Windows 时自动更新；更新后共享目录/剪贴板恢复正常。
4. **旧 VM 的 NVRAM 备份**：`/tmp/nvram-bak/NVRAM.dat`、`NVRAM.tnvs`（若需回退研究可保留）。
