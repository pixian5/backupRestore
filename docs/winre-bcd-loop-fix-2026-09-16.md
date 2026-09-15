# 2026-09-16 BCD 丢失导致引导循环的修复 + WinRE 引导结论

> 本次事故与本机（macOS + Parallels Desktop ARM）测试 VM「Windows 11」直接相关。
> 目标链路仍是：**真实 C: 离线备份 → 篡改 → 还原**（当前系统卷）。
> 本文记录：引导循环根因、WinPE 修复法、以及 WinRE 在 Parallels ARM 上的引导结论。

## 一、事故现象

调整 VM 内存（6GB→8GB）后，多次尝试从 WinRE 引导失败，随后实测到：

- VM 陷入**每 ~6 秒一次的系统级重启循环**（Parallels 日志持续出现
  `PSCI: reset requested!`，且 `OnExitBootServices()` 后约 4 秒即 reset）。
- 屏幕在黑屏与 Windows Logo(boot splash) 之间闪烁，永远进不了桌面。
- `prlctl exec` 一直报 "Unable to open new session... finished booting"。

此前会话已确认：8GB 内存不是原因（6GB 也一样失败）；连 C: 原始 Winre.wim 都
无法引导（排除了注入版 WIM / 分区 / 空间问题）；bootmgr 确实尝试创建 RAMDISK
（黑屏约 2 分钟）后失败回落。

## 二、根本原因

通过从 **Windows 11 ARM64 安装 ISO 启动 WinPE**（下载镜像：
`~/Downloads/Win11_25H2_Chinese_Simplified_Arm64_v2.iso`），进入 cmd 后
用 `bcdedit /store S:\EFI\Microsoft\Boot\BCD /enum` 检查，发现：

> **BCD 里 Windows 11 的正常引导条目丢失了**，只剩两个 WinRE/PE 的 ramdisk 条目。

- `{default}` 指向的是 **WinRE ramdisk**（`ramdisk=[C:]\Recovery\WindowsRE\Winre.wim,...`），
  不是 `partition=C:` 的 Windows 条目。
- bootmgr `displayorder` 里没有真正可引导的 Windows 系统条目。
- 于是 bootmgr 无有效 OS 可回落 → 每次尝试加载后系统自发 reset → 6 秒循环。

> 这个 Windows 引导条目**在之前反复 WinRE 引导失败 / bootmgr 自动修复期间被
> 移除**，才是"系统看起来死活起不来"的根源；跟 Winre.wim 本身、RAMDISK、内存
> 都没直接关系。

## 三、修复方法（macOS 宿主 + Parallels）

操作通道遵循 [operation-channels.md](operation-channels.md)：`prlctl` 管 VM、
`vmkey.sh` 注入键盘、`type_text.sh` 逐字符打字（见 tools/win-clicker/）。

1. 停 VM：`prlctl stop "Windows 11" --kill`。
2. 挂 ISO：`prlctl set "Windows 11" --device-set cdrom0 --image <iso路径> --connect`。
3. 调整引导顺序：`prlctl set "Windows 11" --device-bootorder "cdrom0 hdd0"`。
4. 启动，在 "Press any key to boot from CD or DVD..." 时立即注入 SPACE
   （`./vmkey.sh space`），进入安装程序 WinPE（X: 盘，`X:\sources`）。
5. `Shift+F10` 打开命令行（`./vmkey.sh shift+f10`）。
6. 挂载 ESP 作为 S:：
   ```cmd
   diskpart
   select volume 4      # 300MB FAT32 ESP（V:隐，用 list volume 确认编号）
   assign letter=S
   exit
   ```
7. 重建 Windows 引导条目（**关键：必须 `/s S:` 和 `/f UEFI`，否则不会写入正确 store**）：
   ```cmd
   bcdboot C:\Windows /s S: /f UEFI
   ```
   bcdboot 会：重建 `{default}` → 指向 `partition=C:`、description=`Windows 11`，
   并把 Windows 条目加进 displayorder。
8. 验证（用 cls 清屏再查，避免历史滚动污染）：
   `bcdedit /store S:\EFI\Microsoft\Boot\BCD /enum {default}`
   → 应看到 `device partition=C:` / `osdevice partition=C:` / `description Windows 11`。
9. 收尾：卸 ISO、恢复 `hdd0 cdrom0` 引导顺序，重启即可正常进 Windows。

### 验证结果
- VM 恢复 ALIVE 约 30 秒（此前 6 分钟都进不去）。
- Windows 11 正常进桌面；BackupRestore GUI（v1.5.12，PID 10268）随系统自启且
  正常显示，识别到 C: 249.5 GiB / 170.4 GiB 可用、Windows 安装=是。
- WinRE 资产完好未损坏：
  - `C:\Recovery\WindowsRE\Winre.wim`（800,986,231 B，原始）
  - `C:\Recovery\WindowsRE\Winre-injected.bak`（804,910,505 B）
  - `Y:\Recovery\WindowsRE\Winre.wim`（803,262,471 B，位于合法 GPT Recovery 分区）
  - `C:\Recovery\WindowsRE\boot.sdi`（3,170,304 B）、`ReAgent.xml`

## 四、WinRE（RAMDISK 引导）在 Parallels ARM 上的结论

综合多次受控试验，**在 Parallels Desktop（ARM / macOS）上，WinRE 通过
`ramdisk=[C:]\Recovery\WindowsRE\Winre.wim` 引导始终失败**：

- bootmgr 确实进入 RAMDISK 创建阶段（黑屏约 2 分钟），随后失败回落 Windows，
  且正常 Windows 已能启动（说明不是整机引导问题）。
- 8GB 内存无效；注入版 vs 原始 WIM 无效；WinRE 在 C: 或 Y: 皆无效。
- 直接证据：**从同样含 803MB WIM 的安装 ISO（UDF 光盘介质）引导 WinPE 则成功**，
  而同一文件放在 NTFS（C:）用 RAMDISK 引导则失败。

> 排查结论：问题基本可锁定在 **bootmgr 对本 VM 的 ntfs/bootmgr RAMDISK 大 WIM
> 读取/解压失败**（而非 WIM 内容损坏或内存不足）。这是测试环境（Parallels ARM）
> 的固件/启动栈限制，**不是 BackupRestore 软件本身的 bug**。

## 五、影响与后续方向

- BackupRestore 的"自动重启进 WinRE 离线备份"链路（`resume_pending_boot_task`）
  依赖 WinRE 引导；在当前 Parallels ARM VM 上该 Walk 受环境限制无法闭环。
- 后续可选方案：
  1. 在**真实 Windows 机器（x64/ARM）上验证** WinRE 离线备份/还原链路（最可靠）；
  2. 若必须在本 VM 内验证，退而用 WinPE 手工 `dism /Capture-Image` 代替
     WinRE 自动重启链路做端到端备份，但不算完全等价；
  3. 必要时新建一个干净 VM（用本仓库 `tools/` 的安装脚本）重演 WinRE，排除
     本次反复试验对 BCD/磁盘的历史污染。

## 六、关键命令速查（供后续复用）

```bash
# 挂 ISO / 改引导顺序 / 卸 ISO
prlctl set "Windows 11" --device-set cdrom0 --image <iso> --connect
prlctl set "Windows 11" --device-bootorder "cdrom0 hdd0"
prlctl set "Windows 11" --device-disconnect cdrom0
prlctl set "Windows 11" --device-bootorder "hdd0 cdrom0"

# 内存（改动需 VM 停止）
prlctl set "Windows 11" --memsize 6144   # 6GB，实测可用配置
# 当前 VM 8GB 已改回 6GB 以避免引导循环

# WinPE 里（Shift+F10）重建引导
diskpart / select volume 4 / assign S: / exit
bcdboot C:\Windows /s S: /f UEFI
bcdedit /store S:\EFI\Microsoft\Boot\BCD /enum {default}
```