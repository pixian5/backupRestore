# ProcMon 抓出 v1.7.11 阻塞真因：`ramdisk=` 值写畸形，而不是「产品进程上下文」

- 日期：2026-09-29
- 版本：v1.7.12（`ramdisk_spec` 修正）→ v1.7.13（`{bootmgr}` 别名）→ v1.7.14（`--no-reboot` 不再武装）
- 产物：`BackupRestore.exe` v1.7.14 = 1,769,472 B，SHA-256 `e34cde4c5db8bf202ea11bd163e403ef0d3433524855b0f23b4d8f1beaa1d55d`
- 证据目录：`.test-artifacts/pe-channel-poc/`（未入库）

## 一、结论先说

v1.7.11 的实机阻塞（产品进程内 `bcdedit /set <loader> device ramdisk=…` 稳定报
「指定的设备无效」，同一条命令从 cmd/PowerShell 手工执行立刻成功）**不是进程上下文问题**，
而是我们拼出来的 `ramdisk=` 值本身畸形。ProcMon 的 `Process Create` 详情列给出了产品的真实
命令行，一眼对出差异。

**正确的值形态**（以 `bcdedit /enum` 回显为准）：

```text
device    ramdisk=[F:]\BackupRestoreRE\Winre.wim,{设备选项对象}
osdevice  ramdisk=[F:]\BackupRestoreRE\Winre.wim,{设备选项对象}
```

**方括号里只包「卷」，`]` 紧跟卷后面闭合**，随后是卷内绝对路径，最后逗号接设备选项对象。

产品此前的两种错法，报错还不一样，正是它们把排查带偏的：

| 拼法 | bcdedit 报错 |
|---|---|
| `ramdisk=[F:\BackupRestoreRE\Winre.wim],{guid}`（`]` 在逗号前，包住整条路径） | 按规定设备无效 |
| `ramdisk=[F:\BackupRestoreRE\Winre.wim,{guid}`（发现错了之后把 `]` 直接删掉） | 指定的设备无效 ← v1.7.11 卡在这 |

第二错是从第一错"修"出来的：当时把 `]` **删掉**而不是**移动**，于是畸形串变成"有 `[` 无
`]`"，并据此形成了"这种值不需要闭合括号"的错误结论，写进了文档注释和单测。

## 二、怎么抓到的（ProcMon on Windows ARM64）

1. `Procmon64a.exe` 在本机可用（驱动正常加载、能捕获）。存 CSV 后列头：
   `"Time of Day","Process Name","PID","Operation","Path","Result","Detail"`。
2. **`Process Create` 行的 `Detail` 列形如 `PID: 6852, Command line: …`** —— 这就是拿真实 argv
   的唯一可靠途径。只看 `Process Name` 完全看不出参数。
3. 案例 A 的 CSV 有 155 MB / 924,243 行，用 PowerShell `StreamReader`/`StreamWriter` 流式过
   滤（`pm-mineA.ps1`），先按 `*bcdedit.exe*` 过滤得 385,972 行，再按 `Process Create` 过滤得
   **157 行**，落到 `caseA-invocations.txt`（35,738 B）。
4. **案例 A 一份 trace 里同时有成功和失败**：同一进程先成功写了 `device partition=C:`，
   紧接着的 `device ramdisk=` 失败。这比原本计划的 A/B 两个案例对照更严格——同进程、同父进程、
   同句柄继承关系，唯一变量就是参数。于是案例 B（shell）不再需要重跑。

> 案例 B 的 ProcMon 始终没抓到：两次采集都以 `data collector set not found` 结束。
> 事后查明是**宿主磁盘空间不足导致 Parallels 快照创建失败**（见第五节），与研究本身无关。
> 计划中的案例 A/B 因此实际被案例 A 内对照取代。

## 三、为什么一轮白干：被 `#[cfg(windows)]` 挡住的单测

`boot_entry.rs` 整个模块是 `#[cfg(windows)]`，所以它里面的单测**在 macOS 上一次都不会编译**。
那里躺着一条断言错误形态的单测
（`ramdisk_spec_has_no_trailing_bracket`，断言 `ramdisk=[F:\BootRestoreRE\Winre.wim],{…}`），
它不但没拦住 bug，反而把错的说法固化了。

修正后的三条单测全部放在 **macOS 也编译的 `text_parsing.rs`**：

- `ramdisk_spec_closes_the_bracket_right_after_the_volume` —— 断言完整串，且整串只有一个
  `[`、一个 `]`
- `ramdisk_spec_rejects_both_historical_malformed_shapes` —— 显式断言不含 `[F:\`、含 `[F:]`、
  不含 `],{`
- `ramdisk_spec_without_a_drive_letter_keeps_the_volume_empty`

`ramdisk_spec` 的实现只做三件事：把盘符切出来、补开头的 `\`、拼 `[卷]卷内路径,{devopts}`。

```rust
pub(crate) fn ramdisk_spec(wim_path: &str, devopts: &str) -> String {
    let (volume, inside) = split_volume_and_path(wim_path);
    format!("ramdisk=[{volume}]{inside},{devopts}")
}
```

**教训写在 `text_parsing.rs` 里了：先抓真实 argv，再谈进程上下文。** 因为值畸形而重试一万次
也还是报同一个错，所以「整个『改字段』块带退避重试」这套机制从来只是用来扛 BCD 存储偶发占用
的，绝不用来掩盖参数错误。

## 四、同一轮实机又暴露两个问题

修好 `ramdisk_spec` 之后实跑，立刻撞上第二、第三个问题。都不是原有那条阻塞，但都在**没有
真实跑过产品**之前不可能发现。

### 4.1 `{bootmgr}` 被 GUID 校验拒绝（v1.7.13）

```
error: invalid task: BCD enum: not a valid GUID: {bootmgr}
```

`bcdedit` 接受 `{bootmgr}` 这类知名别名（well-known identifier），但它们是字符串不是
8-4-4-4-12 形态，过不了 `require_guid`。而 `require_guid` 是「标识符为空时 bcdedit 静默作用于
`{default}`」这条硬约束的守门人，**绝不能整体放宽成接受任意别名**，否则 `{default}` 会从同一个
口子混进来。

修法：`text_parsing::require_identifier`，白名单里只放 `{bootmgr}` 一个
（`{default}`/`{current}`/`{ntldr}`/`{fwbootmgr}`/`{ramdiskoptions}` 全部仍然拒）。
同样的教训再次生效：校验函数放 macOS 也编译的 `text_parsing.rs`，配套两条单测
（`require_identifier_accepts_the_bootmgr_alias` /
`require_identifier_still_rejects_default_and_other_aliases`）。

### 4.2 `prepare --no-reboot` 仍然武装了 bootsequence（v1.7.14）

`create_entry` 的第 4 步无条件执行 `bcdedit /bootsequence <loader>`，于是**带
`--no-reboot` 也会改 bootmgr 的 bootsequence**，下一次重启就直接进 PE 了。注释明明写着
「一次性启动留到载荷注入并拷贝完成之后再武装」，实现却在 DISM 之前 arm；且 arm 发生在
`windows_prepare` 的 `if no_reboot { return }` 之前，所以 `--no-reboot` 完全挡不住它。

实机证据：`--no-reboot` 那次跑完，BCD 回显
`bootsequence {c0c8debb-bbff-11f1-88fc-cf8d5b22e12a}`，正是当次新建的条目。

修法：把 arm 抽成 `boot_entry::arm_one_shot(&entry, log)`，由 `windows_prepare` 在载荷已注入、
已覆盖到镜像卷、`boot-entry.json` 已落盘**之后**显式调用；`no_reboot` 分支在其之前返回。

v1.7.14 重跑 `--no-reboot`，`/enum {bootmgr}` 里已无 `bootsequence`，条目照建、
`device`/`osdevice` 回读正好等于 `ramdisk_spec` 的期望值。

## 五、宿主磁盘空间会静默让快照创建失败

`tools/vm-snapshot.sh` 用 `set -euo pipefail`，且把 prlctl 输出重定向进了记录文件，所以
**快照创建失败时它一声不出、什么都不打印**。两次失败都是同一个原因：

```
Unable to create the snapshot. There is not enough free space on the physical disk.
```

后果不只是"没快照可回退"：案例 B 的 ProcMon 采集前建快照失败，导致整轮验证没做，
当时被误记成"ProcMon 找不到数据收集器集"。

排查方法：不要看 stdout，直接读
`.test-artifacts/snapshots/<stamp>-<label>.txt`。快照成功后记录文件里会有一条
`Snapshot ID missing; STOP` 之外的 `VERIFIED_SNAPSHOT={…}`。

清出空间后（宿主当前 177 GiB 可用）恢复正常，本次三轮验证的快照：

| 时间 | 标签 | ID |
|---|---|---|
| 19:39 | ramdisk-spec-fix-v1712 | `{538467a0-b7398-4d05-a225-4a735f453622}` |
| 20:10 | verify-identifier-bootmgr-alias-v1713 | `{148b4c1a-0881-461a-aaf8-32d1ad80be0b}` |
| 20:41 | verify-bcd-delete-persistence-v1714 | `{cc4df418-e5e9-4281-946a-a1a940334c30}` |

## 六、两条宿主→客体的通道差异（本次踩到，后续省时间）

`prlctl exec "Windows 11"` 不带 `--current-user` 是 **SYSTEM** 通道：

- `bcdedit /enum all /v` 可用
- `mountvol X: /S` **会挂起不返回**（卡死 prlctl exec 通道，最后靠 `prlctl reset` 才恢复）
- 产品 `efi_identity()` 正是走 `mountvol /S`，所以在 SYSTEM 通道跑 `prepare` 必然拿不到
  ```text
  error: invalid task: system GPT EFI partition was not found
  ```
  这不是产品缺陷——用户实机是 UAC 提权的用户会话，`mountvol /S` 正常。验证时用
  `--test-efi-drive Y`（先用 diskpart 给 ESP 分配盘符）即可绕开，直击要验的目标。

`--current-user` 通道则是**非提权**令牌，`bcdedit` 会报「无法打开启动配置数据存储。拒绝访问。」

另外 `prlctl exec --current-user powershell` 本次整体不返回输出，`CreateProcessAsUser` +
链接令牌的方案（`tools/launch-in-session-elevated.ps1`）也返回 `launch_result=False`。
**本次未解决用户提权会话的自动化通道**，是已知缺口（见第七节）。

## 七、BCD 对象删除的持久性

一次"删完立即 `prlctl reset`"发现了**删除未持久化**：删除时 4 条 `操作成功完成`、
删后 `/enum` 也干净，重启后对象又回来了。原因推测是 ESP 上的 BCD 写入尚未 flush 就被软重启。

规避办法（本次验证有效）：删除后**先等 60–90 秒再重启**。这次等满 90 秒后重启，残留为 0。
单独等 45 秒不重启也不会复活（说明不是后台回写），所以复活只与"过早重启"相关。

## 八、清理与终态

- BCD：本项目自建对象（`BackupRestore task RE` osloader × 若干、同名 device options、
  `BR-DIAG` 一对、`PM loader`/`PM devopts` 一对）**全部删除**；`bootsequence` 已清；
  `prlctl reset` 后 `/enum all /v` 复核残留 0。
- 注册位未动：`reagentc /info` 仍是
  `\\?\GLOBALROOT\device\harddisk0\partition4\Recovery\WindowsRE`、BCD 标识 `b69adf69-…`。
- 临时盘符 Y:（为 `--test-efi-drive` 分配）已 `remove letter` 移除；`/enum` 里 bootmgr 的
  `device` 回到 `partition=\Device\HarddiskVolume2`。
- 重启后 `SYSTEMROOT=C:\Windows`（不是 PE 的 `X:\windows`），正常回桌面。
- VM 内仍待清理：`F:\brimg\v1713.wim`、`F:\brimg\v1714.wim`、
  `H:\brwork\tasks\{d10d860b…, 262d1a7c…}`、`C:\Users\Public\pkg\pmlog\` 下的 trace。

## 九、仍未闭环

1. **产品进程内跑通完整 `mountvol /S` 路径** —— 需要可用的用户提权会话通道（见第六节）。
2. restore 方向的任务闭环、断电续跑、BCDBoot 交互、Secure Boot、还原目标 WinRE 注册语义。
3. v1.7.11/v1.7.12 与另一条并行改动线的版本归属（都在往 `main` 推）。
