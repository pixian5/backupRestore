# C: 系统卷「真实备份+还原」验收 — 2026-09-15 阶段总结

> **历史受阻记录，不是当前启动协议。** 当时描述的 `RecoveryLauncher.cmd` 入口已移除。
> 当前产品 WinRE 使用 `winre-winpeshl.ini -> Recovery.exe recover-env`；自定义 PE 使用
> `winpe-winpeshl.ini -> Recovery.exe --pe-desktop`。见
> [current-status-2026-09-16.md](current-status-2026-09-16.md)。

## 目标
在测试 VM 对真实 C:（当前运行系统卷）做一次带防假标记的离线备份 + 篡改 + 还原，规避"假还原/空跑"。

## 关键机制（已核实）
- 防假标记：`C:\br-test\marker.txt`(`CDRIVE-VERIFY-MARKER`) + `marker-nonce.txt`(`N3ZJ4Gav7MIlkAugWh8pXJ0W`) + `keepfile.txt`(`KEEPFILE-SENTINEL-v1.5.10`)。
- 备份目标：`E:\br-cdrive-v1.wim`（E: = br.hdd 独立盘，卷标 `br`，空闲 210GB）。
- VM 配置：`prlctl list -i` → 光驱已挂 `BackupRestorePE.iso`，Boot order `cdrom0 hdd1 usb hdd0`；BIOS `efi64`，Secure boot off。
- PE-ISO 自动运行协议：`boot.wim` 内 `Windows\System32\winpeshl.ini` → `RecoveryLauncher.cmd` → 读 `RecoveryTask.env`（缺失则 exit 87）→ 有则 `Recovery.exe recover-env <config>`。当前 boot.wim **缺 RecoveryTask.env**。

## 本会话关键纠错（重要，勿再踩）
1. **`prlctl exec` 语法**：必须 **不加 `--`**、且名字含空格时用 **UUID**。正确：`prlctl exec <UUID> --user x --password 1 cmd /c "echo HI"`。加 `--` 或有空格名字 → 静默 exit 2（此前所有"exec 损坏"判断均因此误判）。
2. **引号剥除**：`prlctl exec` 会剥掉命令里的双引号 → 不要在 `-Command "..."` 里依赖双引号。可靠做法：把 PS 脚本写成文件再 **base64(-EncodedCommand)**（UTF-16LE → base64）传参，字母数字安全。
3. **exec 会话非管理员**：`p8b6\x` 的令牌是 **UAC 过滤（Admins deny-only，Medium，仅 SeShutdown）**。`bcdedit /enum`、`reagentc`、挂 `S:`、`schtasks /create /rl HIGHEST`、`runas` 全部失败：
   - 无法非交互提权。
   - 因此 Windows 侧 admin 预备（BCD/reagentc/ESP）走不通。
4. **computer-use 背景点击/按键不透传 guest**：对 Parallels 「Windows 11」窗口点「备份」tab、点「开始」均无效果（窗口非前台焦点时 VM 不捕获指针）。无法用它驱 GUI/点 UAC。

## 现状 / 阻塞
- 快照安全网：`{b682cf81-9137-4327-a57b-cf136e0b7695}`（cdrive-pe-verify-0915）。
- E: 干净（无 WIM / 无 Recovery.log）。`C:\br-test.json` 有残留（旧 WinRE 配置 `system_drive_choice:2`，未执行，待清理）。
- 三条离线通道受阻：
  - **WinRE/PE-BCD**：需 Windows 侧 admin（bcdedit/reagentc/挂 S:）→ exec 无法提权。
  - **GUI 驱动**：点击/输入不透传 guest → 无法点 UAC。
  - **PE-ISO 自引导**：零 admin 但需重建可引导 ISO（注入 RecoveryTask.env），且该 VM ARM64 固件引导历史上卡壳。

## 解锁任选其一
- **A**：提供可用的提权/交互通道（交互桌面给一个提权控制台 / 临时降该 VM UAC / 允许非交互提权）。
- **B**：授权重建 PE-ISO 注入备份任务（cdrom 首启；接受固件/工具风险）。
- **C**：宿主侧直接挂载/转换 Parallels hdd 卷做离线捕获/校验（较绕）。
