# 灌入 WinRE 镜像建「第二系统」两次失败：暂存载荷被外部删除 + VM 并发干扰

- 时间：2026-10-12 17:55
- 模型：Step-5-Preview
- 状态：**已定位根因，实验被并发会话中断，待用户协调后重跑**

## 一、目标（用户的实验）

用产品「新增第二系统」（`prepare --operation create-secondary`）灌入一个 **WinRE WIM**，
看执行完能不能直接当一个「RE 硬盘版」用。要求全自动：产品自己重武装、重启、进 RE、执行、收尾，
**人不点任何东西**。

## 二、两次失败的完整证据链

### 第 1 次（任务 2f7d44c1，源镜像 H:\winre-test.wim）

| 时刻 | 事件 |
|---|---|
| 16:05:45–16:06:11 | prepare 成功：载荷注入 `stage\Winre.wim` → 暂存到 `H:\BackupRestoreRE\<id>\`（sha 17887e12）→ 建 BCD 项 {c6a081b3}/{c6a081b4} → `--no-reboot` 停在 prepared |
| 16:14:32–34 | `resume` 续跑：`rearm()` **亲自校验**暂存 Winre.wim+boot.sdi 存在且哈希一致 → 武装 bootsequence → `shutdown /r /t 0` |
| 16:14:46 | 重启。Kernel-Boot：**类型 0x0、无 WinRE 引导事件、直接进 Windows**（与 09-27 PoC 失败案例 A 同签名） |
| ~16:25 | 暂存目录已空；`stage\Winre.wim`、源镜像 `H:\winre-test.wim` 同时消失 |

### 第 2 次（任务 b7e42d68，源镜像 H:\winre-src.wim = 官方原版 WinRE 709,810,082B）

| 时刻 | 事件 |
|---|---|
| 17:37:52–57 | 全自动 prepare（不带 `--no-reboot`）：三次暂存写入 H:\BackupRestoreRE\<id>\Winre.wim（sha 00c06eb3） |
| 17:37:59.255 | bootsequence 武装于 {98284227-c620-11f1-8954-c5f92c9d8c2e} |
| 17:37:59.268 | `shutdown /r /t 0`（**距武装仅 13 毫秒**） |
| 17:38:15 | 重启。Kernel-Boot 又是「从 0x2 启动选项启动、类型 0x0」→ 回落 C: |
| ~17:45 | 暂存目录已空 |
| 17:45:41 | **`prl_vm_app`（PID 38691）被外部终止，VM 停机**（parallels.log 无停机记录，戛然而止） |

## 三、根因判定

**重启没进 WinRE，是因为 bootsequence 指向的暂存载荷在重启前已被删除**——winload 拿不到
ramdisk，bootmgr 丢弃一次性启动，回落默认项 C:。第 2 次尤其明显：文件在 shutdown 前 13 毫秒
还在，6 秒内被删不合常理，**但 3 分钟后确认已消失**，且两次都消失。

机制本身没问题：`docs/20260929-1300-pe-channel-poc-winre-wim-boots-from-image-volume.md`
已 PoC 证明「WinRE.wim 放镜像卷 + 自建 BCD 项 + bootsequence」能进 WinRE 并跑载荷，
且产品现役代码（`windows_prepare.rs` v1.7.11 新启动通道）就是按这个做的。

## 四、删除者特征（重要，别再浪费时间排查 VM 内部）

被打的文件有**精准路径特征**：
- `*\Recovery\WindowsRE\Winre.wim`（C: 注册位的、E: 第二系统的，都没了）
- `*\BackupRestoreRE\*\Winre.wim`（两次任务的暂存载荷）
- `H:\` 根目录的散落 wim（winre-test.wim，16:40:48 被删）

**幸存**的：`C:\Users\Public\winre-src\Winre.wim`、`C:\brtest\*`、`C:\Users\Public\br-test\*`、
`C:\BackupRestorePE\*`、`H:\c-backup\c-system.wim`（子目录内的都不受影响）。

已排除的 VM 内部可能：无进程/服务/计划任务/启动项（tasklist、schtasks、Run 键全查过）；
Prefetch 在删除窗口无异常程序；Defender 无威胁记录；USN 日志未开启（`fsutil usn createJournal`
在本机报 error 32，开不了）。

**且删除绕过了 NTFS 目录 mtime 语义**（实测本机增删文件都会更新父目录 mtime，但暂存目录
mtime 停在文件创建时刻）→ 指向**宿主侧直接操作磁盘**，不是 VM 内任何进程。

## 五、并发会话（当前最大障碍，有硬证据）

另一个会话正在**同时**操作同一仓库和同一台 VM：
- 提交：d864fd0（16:36 构建路径对比）、c4a93b6（16:59 修 build-win.sh 部署静默失败）、
  e3aa414（**17:26「引导修复接入在线还原路径」**——正在改引导/BCD 代码）
- 16:49–16:50 `H:\brwork\` 被创建并写入 2,437,632B 新二进制（build-win.sh --deploy 的行为）
- 16:40:48 `H:\winre-test.wim` 被删（我两侧会话均未执行任何删除）
- 17:45:41 `prl_vm_app` 被终止，VM 停机

他们的实验窗口与文件消失窗口完全重叠，是删除者的头号嫌疑。

## 六、VM 当前其他异常状态（实验前就已存在）

- **注册 WinRE 已坏**：`C:\Recovery\WindowsRE\Winre.wim` 缺失（16:03 就没了），
  `reagentc /info` 失败。ReAgent.xml 还在（2026/10/09 05:56，1,133B）。
- 可用镜像源：`C:\Users\Public\winre-src\Winre.wim`（微软官方原版，709,810,082B）。
- 诱饵 `H:\decoy\BackupRestoreRE\test-guid\` 未被动过（`H:\decoy\` 路径不在打击面）。

## 七、待清理 + 下一步

**待清理**（VM 停机中，开机后第一件事）：
- 任务 b7e42d68 的 BCD 对象 {98284227-c620-11f1-8954-c5f92c9d8c2e} / {98284226-c620-11f1-8954-c5f92c9d8c2e}
- `H:\BackupRestoreRE\b7e42d68-*`（空目录）、`F:\BR\tasks\b7e42d68-*`
- `H:\winre-src.wim`、`H:\decoy\`、诱饵文件

**下一步**（需用户先协调）：
1. 确认/暂停并发会话（两个 Agent 同开一台 VM 无法做需要重启的实验）；
2. 开机后清理上述残留；
3. 全自动重跑（约 5–8 分钟）：`prepare --operation create-secondary --source-drive E --target-drive P
   --image-path H:\winre-src.wim --wim-index 1 --boot-menu-name BR-WinRE-Exp --allow-destructive`；
4. 重启后立刻查：Kernel-Boot 事件（有无 WinRE 引导）、`recovery.log`（载荷跑没跑）、
   暂存目录完整性——三者一并判定。

**即便启动通道修好，也要预告**：create-secondary 对 WinRE 镜像走的是 apply + bcdboot，
而 WinRE 有「启动 ramdisk 路径 == ReAgent 注册位置」自校验（09-27 PoC），
装到 P: 后那个启动项大概率起不来。真正的「RE 硬盘版」形态是 ramdisk 启动项
（boot.sdi + wim），不是把 WinRE 当常规系统装——这点可以在实验里验证。
