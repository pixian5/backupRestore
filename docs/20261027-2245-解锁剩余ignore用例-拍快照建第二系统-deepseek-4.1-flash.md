# 解锁剩余 ignore 用例：拍快照、建第二系统、3 条 SKIP 首次真跑通

- 日期：2026-10-27 22:45（收尾 2026-10-28 00:15）
- 版本：**2.6.69（未升版本）**
- 提交：`e6b0af1`（门禁 env 传递三处修复）
- 前序：`docs/20261027-2210-真实日志回放判据夹具化-*.md`

## 0. 一句话

用户明确要求「建系统/快照，别再拿环境当借口」。本轮拍 1 个整机快照、在 disk2 上建分区并**真装了一套第二系统**，
把 3 条长期 SKIP 的用例跑成 PASS（`offline_secondary_mapping` / `boot_entry_fault` / `secondary_menu_repair`）；
剩 2 条卡在「RE 里没有 `prlctl exec` 通道」这个物理限制上，需要手工举证。

## 1. 快照（安全网）

```
prlctl snapshot "Windows 11" -n "pre-acceptance-20261027-2215"
→ {2d17d282-a17c-44f2-aee2-4f95ec55cdcf}
```

停机状态下拍（文件一致），宿主 224 GiB 空闲、VM 数据 88 G：**只拍 1 个**（项目铁律「全程最多 1 个」）。
这个 GUID 同时满足 `BR_ACCEPTANCE_SNAPSHOT` 的 `is_guid` 校验（**必须带花括号**，`is_guid` 的第一句就是 `strip_prefix('{')`）。

## 2. 环境建设（三件事，全部实测）

### 2.1 第二系统分区

`disk2`（350 G）在 10-27 删掉旧 F: 后只剩 MSR，正好用来做第二系统：

```powershell
New-Partition -DiskNumber 2 -UseMaximumSize -DriveLetter F
Format-Volume -Partition $p -FileSystem NTFS -NewFileSystemLabel BRSecondary
→ F: 350G
```

### 2.2 「不装 Windows 也能验 repair」——用 reg save 造真 hive

`offline_secondary_mapping` 只要卷上有**真 hive**，不要求真装系统：

```
reg save HKLM\SYSTEM   F:\Windows\System32\config\SYSTEM   /y   → 14,229,504 字节
reg save HKLM\SOFTWARE F:\Windows\System32\config\SOFTWARE /y   → 111,181,824 字节
```

`repair()` 只改 **F: 上的 hive 副本**（加载 → 改 `MountedDevices` → 卸载），不碰本机。
实测 8.86 s 跑通，并在 F: 留下 `BackupRestore-secondary-original\{SYSTEM,SOFTWARE}`（与源同字节数）作为副作用证据。

### 2.3 镜像：DISM 在线抓取不行，只能走 RE

```
dism /Capture-Image /ImageFile:H:\BRImages\... /CaptureDir:C:\
→ Error 0x80070020（ERROR_SHARING_VIOLATION，文件被占用）
```

这正是产品「活动系统盘必须离线处理」的设计。改用**产品自己的 prepare 机制**：

```
BackupRestore.exe prepare --operation backup --source-drive C \
    --image-path H:\BRImages\c-sec-20261027.wim --compress fast --allow-destructive --json
→ task b561cee4-914d-4fe0-aa38-4d84fc70fe77，success
```

它自己建 BCD 项、重启进 RE、在 RE 里跑 GUI（截图确认 `2/4 捕获镜像 42%`、`CAPTURE_HEARTBEAT` 滚动）、
备份完重启回系统。**镜像 31.34 GB**，任务 `status=success`。

> 顺带修好了一个真问题：`Y:\Recovery\WindowsRE\{Winre.wim,boot.sdi}` 都是 **0 字节**、
> `C:\Windows\System32\Recovery\Winre.wim` **不存在** —— WinRE 根本起不来（所以 `reagentc /boottore`
> 与手动 `bootsequence` 全部回落 Windows）。用 `C:\Users\Public\winre-src\Winre.wim`（677 MB，DISM 确认是
> `Microsoft Windows Recovery Environment (arm64)`）+ `C:\Windows\Boot\DVD\EFI\boot.sdi` 复制回去，
> `reagentc /disable` + `/enable`（**退出码 0**）后重建为 712 MB，`C:\Recovery\WindowsRE\` 文件齐备。

### 2.4 装第二系统

```
online --operation create-secondary --source-drive C --target-drive F \
    --image-path H:\BRImages\c-sec-20261027.wim --index 1 --format-target --json
→ success，22 分 05 秒，F: 写入 71 GB
```

结果：`F:\Windows\System32\ntoskrnl.exe` 存在，BCD 新增第二系统 loader `{b28feb7c-d219-11f1-89d0-e29942074205}`。
**这是一套真能启动的 Windows**，不是仿造的目录树。

## 3. 门禁的三处修复（否则「配了也永远 SKIP」）

配齐环境变量后门禁仍报 SKIP，根因是**两头都没打通**：

| 位置 | 原来 | 问题 |
| --- | --- | --- |
| `env_missing()` | 查 **VM 内** `if defined` | 宿主设的变量传不进 VM，永远「缺」 |
| `run_one()` | 起测试时**不传环境变量** | 即使查到也传不过去 |

修法：

1. `env_missing` 查宿主（`${!v}`），传递交给 `run_one`；
2. `run_one` 增加第 4 参数，用 `set "K=V" &` 前缀与测试同会话启动（子进程继承）；
   ★ 必须写 `set "K=V"` —— `set K=V &` 会把 `&` 前的空格算进值里，实测 ACK 变成 `"…secondary "`（尾随空格）→ 断言不等；
3. `pe` 类 SKIP 理由改准（见 §5）。

> 教训与 §13.3 同源：**检查了前提，就必须把前提交给被测对象**。SKIP 的理由写「缺环境」而代码根本读不到环境，
> 是「报告通过但什么都没验」的又一种变体。

## 4. 三条用例的实测结果

| 用例 | 前提 | 耗时 | 关键证据 |
| --- | --- | --- | --- |
| `offline_secondary_mapping` | F: 真 hive + 分区 GUID | 8.86 s | F: 留下 `BackupRestore-secondary-original\{SYSTEM,SOFTWARE}`，字节数与源相同 |
| `boot_entry_fault` | 独立 root（**只含 1 个 Success 任务**）+ `stage\Winre.wim` + `boot.sdi` | 159.86 s | 6 条 `FAULT_PASS`：missing-payload / corrupt-payload / boot-请求冲突 / 补偿失败重试 / 字段写入失败留残留簿记 / 补偿删净；BCD `bootsequence` 回空 |
| `secondary_menu_repair` | create-secondary 任务 + ESP + **真实** secondary loader GUID | 1.14 s | 日志 `Restored 1 original display-order entries and appended secondary loader {b28feb7c…}` |

`boot_entry_fault` 的独立 root 构造要点（踩过才知道）：

- `root/tasks` 下**必须只有 1 个**任务（用例硬性检查），所以每条用例单独一个 root（`H:\br-accept`、`H:\br-accept2`）；
- 任务必须是 `status=success` 且带 `stage\Winre.wim`；
- `H:\BackupRestoreRE` 里的**空目录残壳**会被判成「存在恢复载荷」→ 先清掉；
- `BR_ACCEPTANCE_SNAPSHOT` 用真快照 GUID（`is_guid` 要求带花括号）。

`secondary_menu_repair` 的第一个坑：直接用 10-24 那个任务的 `bcd-before-bootmgr.txt` 会报 **`BR-BOOT-029`**
（主/副 loader 缺失）—— 因为那份快照记录的第二系统早随 F: 删除而消失。按**当前真实状态**重造一份
「只含主系统」的快照（主 loader `{a6273652}` + 它的 `resumeobject`）后才能跑通。

## 5. 为什么还剩 2 条（`winre_offline_reg` / `disposable_vhd_boot_failure_rollback_in_pe`）

这两条要求 `reg query HKLM\SYSTEM\CurrentControlSet\Control\MiniNT` 成功（= 真在 PE 里）。
本轮实测：**RE 里没有 Parallels Tools，`prlctl exec` 通道不存在** —— 连 `cmd /d /c 'echo ALIVE'` 都返回空
（备份期间连续 7 次空，回到系统后第 8 次恢复）。可以进 RE（prepare 机制已验证），但要在 RE 内执行命令
只能靠「命令提示符」+ 键盘注入，不在门禁能力内。

因此 `pe` 类的 SKIP 理由改为 `requires_windows_pe_manual_in_re`，并保留手工举证口径：
在 RE 的「命令提示符」里 `set` 变量后跑 `cli-tests.exe --ignored --nocapture <filter>`。

## 6. 一个需要记住的 BCD 现象（未定性为产品缺陷）

`secondary_menu_repair` 跑完当场读到 `displayorder` 两项（主 + 副），用例也 PASS（它内部对
`verified.display_order` 做了**项数 + 逐项**校验）。但在我随后设 `bootsequence` 并重启（机器启动了第二系统，
盘符整体平移：C:=350G、主系统变成 E:）之后，再读 `displayorder` **只剩一项**。

- 产品写菜单用的是 `bcdedit /displayorder <g1> <g2>`（不是 `/addlast`），且读回校验后才返回 Ok，
  所以**不是静默丢项**；
- 变化发生在我手动操作/重启阶段，机制未查清（如实记录，不猜）；
- 已修回：`displayorder = [{a6273652}(主), {b28feb7c}(副)]`、`default = {a6273652}`，
  重启验证 **C: 回到 249 G**（主系统），环境完好。

> 方法论：`bcdedit` 的 `default` 显示成 `{default}` 是**别名缩写**（值等于 displayorder 第一项时），
> 不能用它判断「默认项是哪个宿主」。判据要用 `/enum all /v` 的 GUID，并注意**启动上下文会改变盘符映射**
> （同一条 `partition=C:` 在不同启动的系统里指向不同物理盘）。

## 7. 现场与遗留

| 项 | 状态 |
| --- | --- |
| 快照 | `{2d17d282-…}` 保留（用户要求；宿主仍有 ~150 GiB） |
| F: | 350 G，**装好第二系统**（71 G），BCD 菜单第二项 |
| 镜像 | `H:\BRImages\c-sec-20261027.wim` 31.34 GB |
| 测试夹具 | `H:\br-accept`（boot_entry_fault）、`H:\br-accept2`（menu repair） |
| WinRE | 已修好（`C:\Recovery\WindowsRE\` 712 MB），`reagentc /info` Enabled |
| 剩余 SKIP | 2 条 PE（需手工在 RE 内执行） |

手工举证命令（进 RE 后，在「命令提示符」里）：

```
set BR_WINRE_TEST_ACK=fresh-snapshot-test-volume-only
set BR_WINRE_TEST_TARGET=F
set BR_WINRE_TEST_PRIMARY=C
set BR_WINRE_TEST_GUID={356501b2-1ed7-4d1b-b3b3-9145d7942203}
set BR_WINRE_TEST_EFI=<RE 里 ESP 的盘符>:\
set BR_WINRE_TEST_LOG=H:\br-ignored\winre-offline-reg.log
H:\br-ignored\cli-tests.exe --ignored --nocapture offline_registration_on_test_partition
```

（`disposable_vhd_boot_failure_rollback_in_pe` 同法，filter 换成函数名；它自建 128 MiB VHD 到 V:。）

* * *

## 补记：门禁最终跑通（2026-10-27 23:5x）

拍完快照、建好第二系统后，完整门禁三连跑：

| 轮次 | 结果 | 说明 |
| --- | --- | --- |
| 1 | `pass=6 fail=2 skip=2` | 两条 FAIL，见下 |
| 2 | `pass=7 fail=1 skip=2` | `boot_entry_fault` 转绿 |
| 3 | **`pass=8 fail=0 skip=2`** | 全绿 |

### 两条 FAIL 各自的真因（都不是产品缺陷）

1. **`boot_entry_fault` = 夹具不自愈。**
   `H:\br-accept\boot-fault-fixture` 是上一轮留下的，`fs::create_dir` 直接
   `ERROR_ALREADY_EXISTS(183)`。更隐蔽的第二层：残留的 `boot-entry.json` 会让
   `create_entry` 命中「复用已有条目」分支，注入点根本不执行——正是该用例里
   「问题 11」注释已经写明的那个坑，只是它自己没自愈。
   改：开头整目录删掉重建，并按任务隔离清掉上一轮的本任务载荷再过「禁止覆盖」守卫
   （守卫本身不动：别的任务载荷仍在时照样拦）。修后 **188s / 194s 两次全绿**。

   ★ 教训：**「用例第一次真跑」和「用例存在」是两件事**。这条用例此前一直是
   `SKIP reason=missing_env`，环境配齐后第一次执行就暴露了它从来没被跑过。

2. **`secondary_mapping_rejections` / `secondary_menu_repair` 轮流红一次 = 仪器问题。**
   共同特征是**日志文件根本没被创建**，而同一条用例手工重跑是 `0.01s` / `1.8s` 全绿。
   真因：约 **1/8** 的 `prlctl exec` 调用**根本没执行**命令（不是失败，是没跑）。
   认不出这一点，就只能干等 180 秒超时，把「仪器没工作」误判成 FAIL。
   改：发完命令立刻查日志是否诞生，没诞生就重发（最多 3 次，`RELAUNCH=n` 自证）；
   仍没有再同步跑一次取真值。**判据仍是那一行 `test result`，没有把失败改判成通过。**

   ★ 这跟本仓库已有的坑是同一个形状（「短任务别用 `(...) &` 后台起」），
   之前只在 `--list` 那一步修过，跑到 STEP 4 就又踩了一次。

### 剩余 2 条 SKIP（真实原因，不是忘了配）

- `winre_offline_reg`：需要一个**一次性测试分区**并明确 ACK（`BR_WINRE_TEST_*`），
  本机没有 → 有意为之地不跑。
- `boot_failure_rollback`：**RE 内没有 Parallels Tools**，`prlctl exec` 通道不存在，
  自动化无法在 PE 内启动测试二进制。可进 RE（产品 prepare 机制已验证），但要在 RE 内
  执行命令只能靠「命令提示符 + 键盘注入」，不在本门禁能力内 → 只能手工。
