# PD 27→26.4.2 降级后的 VM 修复手册（2026-09-25）

> 场景：Parallels Desktop 从 27.x 降级到 26.4.2 后，`Windows 11` VM 每次启动
> 都掉进 UEFI 固件菜单（Select Language / Device Manager / Boot Manager /
> Continue / Reset），无法进系统。全程无键盘鼠标修复。

## 一、卡 UEFI 菜单：根因与修法

**根因**：NVRAM（`NVRAM.dat`/`NVRAM.tnvs`）里的 Boot#### 变量是 27.x 固件写的，
26.4.2 固件不消费（变量存储里 "Windows Boot Manager"/"UEFI Shell" 字符串都还在，
但固件当它不存在 → 无可引导项 → 进固件菜单）。ESP、bootmgfw.efi、BCD、磁盘全部完好
（`EFI\Boot\bootaa64.efi` 回退文件也在——Parallels 固件空 NVRAM 时**不会**走
UEFI 默认路径回退，别指望它）。

**修法（已验证）**：
```bash
# 1. VM 若是 suspended：先 resume 再 stop --kill（suspended 不能直接 stop/snapshot）
prlctl resume "Windows 11"; sleep 8; prlctl stop "Windows 11" --kill
# 2. 建快照（启动项变更前置要求）
prlctl snapshot "Windows 11" --name "pre-nvram-reset" ...
# 3. 重命名 NVRAM（留证），启动后固件自动重建并引导 Windows
mv NVRAM.dat NVRAM.dat.stuck-XXXX; mv NVRAM.tnvs NVRAM.tnvs.stuck-XXXX
prlctl start "Windows 11"
# 4. ~60s 后 prlctl exec cmd /c "echo TOOLS_OK" 验证
```
注意：**在 PD 27 时代删 NVRAM 重建是无效的**（vm-boot-repair-newvm.md），但
26.4.2 下有效——行为与固件版本绑定。

## 二、快照报 PRL_ERR_REVERT_IMPERSONATE_FAILED：先查磁盘锁

表象有欺骗性：日志同时打 `pthread_setugid_np failed` + `REVERT_IMPERSONATE_FAILED`，
看似权限问题。**真因往往在前几行**：
```
AddDisk() failed ... PRL_ERR_DISK_SHARING_VIOLATION ... harddisk.hdd
File ... is locked by process id XXXX   ← Parallels Mounter (Parallels Explorer -onoui)
```
之前用 Parallels Mounter 把 VM 磁盘挂到宿主机排查过，进程不退锁不放。
修法：`kill <mounter_pid>` + `umount /Volumes/.PEVolumes/PEVolume*`，快照立即恢复可用。
（附带收获：Mounter 会把客体各分区以 SMB 方式挂到 `/Volumes/.PEVolumes/`，宿主机
可直接读写 ESP/NTFS。）

**快照对 suspended VM 必失败**（CreateSnapshot error），先转 stopped 再建。

## 三、reagentc /enable 假成功：陈旧 ReAgent.xml

bcdboot 重建 BCD 之后，`reagentc /setreimage /path ...` + `/enable` 全部报
"Operation Successful" 但 `/info` 仍 Disabled、`{bootmgr}` 里也没有 WinRE 条目。
`/info` 里的 "Boot Configuration Data identifier" 是**旧 BCD 存储时代的残留 GUID**——
`C:\Windows\System32\Recovery\ReAgent.xml` 还拿着重建前的上下文。

修法：提权删除 `ReAgent.xml` → 重新 `setreimage` + `/enable` → 立即 Enabled。

## 四、Winre.wim 丢失后的重建（本轮两个副本都没了）

清理残留时把 Y:(FAT32 Recovery) 和 C:\Recovery\WindowsRE 的 Winre.wim 都清掉了。
宿主机重建路径（不进客体、不联网）：
```bash
wimlib-imagex extract /tmp/win11iso/sources/install.wim 1 \
  /Windows/System32/Recovery/Winre.wim --dest-dir=/tmp/winre-extract --no-acls
wimlib-imagex extract ... /Windows/Boot/DVD/EFI/boot.sdi ...
# 经 X: 共享拷进客体 C:\Users\Public\winre-src\，再提权 robocopy 到
# C:\Recovery\WindowsRE\，最后 setreimage + enable（配合第三节删 ReAgent.xml）
```

## 五、无键盘退出 WinRE

WinRE「选择一个选项」首项高亮"继续"时：
```bash
prlctl send-key-event "Windows 11" --scancode 28   # Enter → 回到 Windows
```
25s 后 Tools 上线。（PD26 CLI 此命令可用；此前"弹窗拦截按键"的问题只在
Parallels「未装操作系统」模态弹窗出现时发生。）

## 六、部署坑两则（复确认）

- 提权会话**看不到共享盘 X:**（`The system cannot find the drive specified`）。
  大文件先经 X: 落到客体本地（如 C:\Users\Public\），再本地对拷。
- `BackupRestore.exe` GUI 以**提权**身份常驻（普通 taskkill Access denied，
  需提权 taskkill）；替换 exe 前先杀进程，否则 copy 静默失败、哈希核验不过。

## 本轮产物

- 快照链（新增）：{0894c023} 卡菜单基线 → {f098edc0} 26.4.2 正常桌面（WinRE A/B 前基线）
- 证据：`.test-artifacts/stuck-uefi-menu-0403.png`（卡固件菜单）、
  `.test-artifacts/winre-boot-pd26-success-0420.png`（26.4.2 成功进 WinRE）、
  ab1~ab9 脚本与输出
- NVRAM 留证：pvm 目录内 `NVRAM.dat.stuck-0925` / `NVRAM.tnvs.stuck-0925`
- v1.6.8 已部署客体并核对哈希（BackupRestore.exe = Recovery.exe = 宿主 64859041…）
