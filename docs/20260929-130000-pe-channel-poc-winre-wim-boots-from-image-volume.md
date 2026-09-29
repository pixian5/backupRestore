# PoC：「把原 RE 副本注入载荷放镜像卷、用自建 BCD 条目启动」——**可行**

> 状态：**PoC 已通过；备份方向的完整任务闭环亦已实机跑通（2026-09-29 12:45 / 13:41 两次实机）**。未改任何产品代码。
> 证据：`.test-artifacts/pe-channel-poc/evidence.md`（启动 PoC）、`evidence-loop.md`（完整任务闭环）。
> **结论已被用户采纳为方向**：直接替换为新通道（不做双开关）；实机改造待实施。
> 环境：Parallels Desktop **26.4.2 (57518)**（已从 27 降级；09-13 之后的旧经验不采信）、v1.7.10 `9077e5e5…a2bd`。

## 1. PoC 命题与结论

命题：**能否把目标系统自己的 `Winre.wim` 拷一份、注入 `winpeshl`+`Recovery.exe`、放到镜像卷，
并用一条我们自建的 BCD 条目（独占设备选项对象 + 独占 osloader + 一次性 `bootsequence`）启动它，
从而完全不碰系统注册的 WinRE？**

**结论：能。** 实机 12:45:23 成功进入 PE，提示符 `X:\Windows\System32>`，注入的钩子跑起来，
把带 `SYSTEMDRIVE=X:` / `SYSTEMROOT=X:\windows` / `RECOVERY_EXE=PRESENT` 的标记写到了盘上；
随后在 PE 内执行真实产品二进制 `Recovery.exe` 返回 **RC=0**。全程
`C:\Recovery\WindowsRE\Winre.wim` 字节未变（712,111,529 B / `1060a552…`）、
`reagentc /info` 未变、BCD 与基线 **diff 为空**、自建对象已删、`{ramdiskoptions}` 未被改写。

## 2. 反直觉发现：旧结论「WinRE 强校验 ramdisk 路径 == 注册位置」需要修正

| 实验 | 臂 | 结果 |
|---|---|---|
| A | `bcdedit /bootsequence {ccb31ee5}`（系统自带 WinRE 条目，注册路径原样，**无 `reagentc /boottore`**） | **8 秒内被 bootmgr 丢弃**：bootsequence 已消费、Kernel-Boot 引导类型 0x0、无 WinRE 引导事件，直接进 Windows |
| C1 | 自建条目（**复制** `{ccb31ee5}`，仍带 `custom:46000010`/`displaymessage Recovery`），WIM 指向镜像卷上的副本 | **成功进 PE 并拉起载荷** |

推论：被校验的是「**ReAgent 登记的那个 BCD 对象 + `reagentc /boottore` 写下的 bootstatus**」，
不是「任意 BCD 条目的 ramdisk 路径」。而历史实验（`.test-artifacts/poc-bcd-*`，09-27）改的/克隆的
**都是被登记的那个对象**，从没试过「全新条目 + 新指针」这条路，所以才会四条绕法全部证伪。

**这条修正的含义**：现有 WinRE 通道为了 F1/F3 造的那整套机制——迁出（`reagentc /disable`→
`/setreimage`→`/enable`）、`WINRE_HOME_*`、待回家标记与桌面收尾、Plan D 捕获前翻转、F1/F3 两道闸、
以及与 Windows servicing 的竞态——在新通道里**全部不再需要**，因为注册位根本不被写。

## 3. 如果改成新通道，会怎样

### 3.1 消失（约等于把 v1.7.10 刚修的那片区域连同上游一起删掉）

- `evacuate_registered_winre` / `WINRE_HOME_*` / `WINRE_EVACUATED` / rehome 标记 /
  `finish_pending_winre_rehome` / `winre-rehome` CLI / `sanitize_scratch_winre`
- `restore_clean_winre_before_capture`（Plan D）+ 注册位时间分片三不变量
- `validate_volume_roles` 的 `RestoreTargetOnRegisteredWinre` / `BackupSourceOnRegisteredWinre`
  与执行层 `winre_role_conflict_at_execution`
- 「注册 WinRE 不是可靠载荷宿主」的 servicing 竞态（载荷在镜像卷，不在 servicing 管辖区）

### 3.2 新增/必须自己负责

- **BCD 条目生命周期**：创建/复用/一次性 `bootsequence` 重武装/终态清理/与既有一次性启动冲突检测
  （[winre-repair-implementation-plan-2026-09-25.md §6](winre-repair-implementation-plan-2026-09-25.md)
   已经写过这套设计，但**从未实现、也从未有任务走过 PE 通道**）。
- **按卷 GUID 定位，不能硬编码盘符**：本次 PoC 里 PE 的 `F:` 实际是 Windows 的 `H:` 卷（marker 落到了 `H:\pe-poc`）。
- 镜像卷常驻 ~700 MB（迁出方案本来也要，不是新增成本）。
- 验收自动化受阻：PE 内 `prlctl exec` 不可用（Parallels Tools 未起），要靠截图/键盘注入，或让 payload 自己落盘证据。

### 3.3 仍未验证（下一步必做）

1. **断电续跑**：续跑要靠我们自己的 `bootsequence` 而非 `reagentc /boottore`；掉电后能否自动重进 PE 续跑未验。
2. **BCDBoot 之后**：还原跑 `bcdboot` 修启动项会不会顺手改写/清掉我们的条目或 `{bootmgr}` 状态。
3. **Secure Boot / 签名**：本次 VM 未开 Secure Boot；改过的 WIM 走 ramdisk 是否被接受未验。
4. **产品级决策**：新通道不再「把干净原件补回目标系统」，还原后目标系统的恢复环境 = 镜像里那份
   （与 Ghost 类工具一致）。要不要保留一次性终态补回，需用户裁定。
5. **真 PE 形态 vs 恢复形态**：本次成功的条目是复制系统 WinRE 条目来的（带 `custom:46000010` 等）。
   要不要退成 `install_pe_ramdisk` 那种「纯 PE 形态」也能启动，尚未单独验证（两者都可用更稳，只有一种可用也行）。

## 4. 实现时可复用的 bcdedit 细节（本次踩过的坑）

1. `device`/`osdevice` 值形态 `ramdisk=[<路径>],{<设备选项对象>}`——**结尾没有 `]`**；
   多写一个 `]` 会报「按规定设备无效」。
2. 独占设备选项对象两条路都通：`bcdedit /copy {<WinRE 条目的设备选项对象>}`（保持「设备选项」类型）、
   `bcdedit /create {GUID} /device`。`install_pe_ramdisk` 注释说的「本平台拒绝 `/create /application ramdisk`」属实，但不是唯一路径。
3. `{ramdiskoptions}` 是「安装程序 Ramdisk 选项」对象（`{ae5534e0-…}`），与真实 WinRE 条目用的「设备选项」
   对象不是一回事，且 `bcdedit /enum all` **不列出它**（漏判它就以为共享对象不存在）。
4. `bcdedit /create` 的 GUID 最后一段必须是 12 位十六进制。
5. 一套完整已知良好的字段组合（本次成功启动的条目）：`device`/`osdevice` = `ramdisk=[…],{opts}`、
   `path \windows\system32\winload.efi`、`systemroot \windows`、`winpe yes`、`detecthal yes`、`nx OptIn`、
   `inherit {6efb52bf-1766-41db-a6b3-0ee5eff72bd7}`、`locale en-us`；
   设备选项对象：`ramdisksdidevice partition=<卷>:`、`ramdisksdipath \<子目录>\boot.sdi`。

## 5. 下一步（需用户裁定，不要擅自开工）

1. 是否把 PoC 升级为**测试卷上的完整任务闭环**（用新通道跑一次探测/备份，含 `power-loss-*` 注入验证续跑）？
2. 是否做成**双通道开关**（`--boot-channel winre|pe`，默认 winre，保留已端到端验证的旧通道作退路）？
3. 新通道下**目标系统的 WinRE 怎么处理**（§3.3 第 4 条）？
