# B 键 BCD 回退 `/import` 毒化缺陷：修复与 run 3 / run 4 双路径实机验收

- **撰写时间**：2026-10-18 22:52（local）
- **模型**：Step-5-Preview
- **版本**：v2.6.17 → **v2.6.18**（`VERSION` + 两个 crate `Cargo.toml` 三处同步）
- **性质**：缺陷修复 + 实机验收（C 入口在线还原 C:→E:，幂等镜像 `H:\BRImages\BR-sys-20261018.wim`）
- **前置**：v2.6.17 进度窗五按钮（L 日志 / M 引导修复 / K 命令提示符 / P 暂停 / B BCD 回退）加快捷键 + 缩尺寸 + run 2 全按钮验收（见第 6 节回顾）

## 1. 缺陷现象（run 2，2026-10-18 20:39–20:55 local）

用户在恢复进度窗按 **B 键（BCD 回退）**后，任务最终失败：

```
[12:53:01.632] plan A: verbatim BCD store path = \\?\Volume{d08d796f-…}\EFI\Microsoft\Boot\BCD
[12:53:01.653] running bcdedit.exe /import …\BackupRestore-online-3128-…\bcd-before-raw   ← raw copy 失败后的兜底
[12:53:02.249] Raw EFI BCD restore was locked; imported the saved BCD snapshot
[12:53:02.249] 用户按 B 键 BCD 回退：已恢复准备期 BCD 快照
…
[12:54:40.680] STEP 3.6/4 预建还原系统启动项
[12:54:40.919] [ERROR] 在线操作未完成：invalid task: bcdboot.exe 失败：退出码 32
               BFSVC Error: BcdOpenStore failed with unexpected error code, Status = [c0000043]
```

同一场景手动复现两次；**重启后 bcdboot 立即恢复（EXIT=0）**——毒化仅会话级。

## 2. 根因

`windows_prepare.rs::rollback_boot_request`（旧代码 2300-2307 行）：raw 快照拷贝失败且
`raw_os_error() == 32`（ERROR_SHARING_VIOLATION）时，兜底执行
`bcdedit /import <快照>`。该命令**逻辑上成功**（BCD 文件 mtime 变化），但会替换
**BFSVC 已打开的存储文件**，使整个会话的引导服务缓存失效——此后 bcdboot / bcdedit
全部 `BcdOpenStore failed`，直到重启。B 键按下去时恢复还在跑（STEP 3 应用期），
毒化直接掐死随后的 STEP 3.6 bcdboot，任务失败。

## 3. 附加缺陷：B 键失败时用户看不到任何反馈

`recovery_progress.rs::trigger_bcd_rollback` 旧实现把错误只 `append_log` 写进任务
日志——用户按了按钮/快捷键，界面上没有任何反应，与「按钮没接线」无法区分。

## 4. 修复（v2.6.18）

### 4.1 `windows_prepare.rs`：锁定时拒绝 `/import`，明确失败

- 错误 32 分支：删掉 `/import` 兜底，改为写日志
  `EFI BCD raw restore blocked: …` 并返回可操作错误
  （"ESP 上的 BCD 正被系统占用（错误 32，共享冲突），无法按字节回退。请重启进入
  恢复环境（WinRE/PE）后重试；在线会话下也可用「引导修复(M)」手工调整启动项"）。
  成功路径日志改为中性措辞 `Restored byte-for-byte EFI BCD snapshot`（该函数现同时
  服务准备期补偿与 B 按钮）。
- 遗留 `bcd-before-export` + `/import` 分支（旧任务专用，现行流程不可达）补毒性警示
  注释，行为不变。
- **保留 WinRE/PE 的 raw 主路径**：恢复环境里系统未运行，raw 拷贝本来就不会遇到锁。

### 4.2 `recovery_progress.rs`：失败弹模态错误框

- `trigger_bcd_rollback(hwnd, shared, via_key)` 增加 `hwnd` 参数；失败时
  `MessageBoxW(MB_OK | MB_ICONWARNING | MB_SETFOREGROUND)`，标题
  "BCD 回退 / BCD rollback"，正文即错误详情。
- 闭包执行在锁释放**之后**再弹（`bcd_rollback` 锁不跨模态持有）；模态只阻塞进度窗
  线程，恢复主流程在另一线程继续跑。
- 新增 `MessageBoxW` extern 声明与 `MB_OK / MB_ICONWARNING / MB_SETFOREGROUND` 常量；
  WM_COMMAND 与全局 B 键两个调用点同步更新。

### 4.3 构建部署

`./build-win.sh --deploy` 全量构建（2803712 字节），四处 SHA-256 比对一致
（包目录 BackupRestore.exe / Recovery.exe、运行目录 H:\brwork 同两名）。

## 5. 实机验收（C 入口在线还原 C:→E:，幂等镜像，非破坏性）

### 5.1 run 3（未锁场景，成功路径）

| 时刻(local) | 事件 |
|---|---|
| 21:41:16 | GUI 启动（pid 4864，H:\brwork 经 BRGUIC 计划任务） |
| 21:49:58 | 撞名模态框（「启动菜单名称已调整」）经 EnumWindows + PostMessage(VK_RETURN) 消除 |
| 21:49:59 | online started: op=create-secondary source=C target=E |
| 21:50:02 | STEP 3/4 DISM 应用开始 |
| **21:53:04** | **按 B**（应用期 ~36%，bcdboot 尚未跑）→ `plan A: verbatim BCD store path` → **`Restored byte-for-byte EFI BCD snapshot`（耗时 6ms）** → 「用户按 B 键 BCD 回退：已恢复准备期 BCD 快照」，**无模态框** |
| 22:03:08 | STEP 3.5 盘符绑定 |
| 22:03:09.53 | STEP 3.6 bcdboot 预建启动项（.80 完成） |
| 22:03:10.46/.49 | set description / **default {a5c45133-cafc-11f1-8997-e9cfd361e402}** |
| ~22:05:30 | 「执行完成」对话框（截图 run3-done-dialog.png） |

**铁证**：E: 序列号 `B2B5094D → 3492ACB8`；哨兵 `BR-214116.txt` 消失（仅剩 WIM 自带的
`BR-102220.txt`）；`E:\Windows\System32\ntoskrnl.exe` / `winload.efi` 在位。

### 5.2 锁机制定位（本轮新发现，修正旧认知）

- `windows_prepare::list_boot_entries`（GUI 撞名预检）跑 `bcdedit /enum all /v`
  **不带 /store** → 经 BFSVC 打开 ESP BCD 存储。**BFSVC 会持锁**：此期间 raw
  `fs::copy / fs::write` 全部报 32；实测持锁 ≥6 分钟且时长可变。
- **所有 bcdedit / bcdboot（带不带 /store）都走 BFSVC**，持锁期间照常工作
  （实测：`RAW_LOCK=HELD` 状态下 `bcdedit /store … /set {bootmgr} timeout 10`
  「操作成功完成」且内容未变）。
- run 3 的 B 键能成功，是因为准备期结束时的 mountvol 卸载（或持锁自然过期）已释放
  BFSVC 句柄；run 2 按 B 时锁仍在（list-boot-entries 于任务创建前 95 秒跑过）。
  run 2 当时记的「BCD mtime 20:39:22」实为**快照文件自身的 mtime**，并非存储被写——旧认知在此更正。
- **可按需复现锁**：跑一次 `bcdedit /enum {bootmgr}`（无 /store）即可，持锁数分钟。

### 5.3 run 4（强制持锁场景，失败路径——run 2 同场景复测）

| 时刻(local) | 事件 |
|---|---|
| 22:22:51 | GUI 启动（pid 576），哨兵 `BR-222451.txt`，E: 序列号 before `3492ACB8` |
| 22:24:5x | 撞名模态框消除（同 run 3） |
| 22:25:37 | online started；22:25:43 STEP 3 应用开始 |
| 22:27:5x | **强制持锁**：`bcdedit /enum {bootmgr}`（无 /store）→ `RAW_LOCK=HELD` |
| **22:28:19** | **按 B** → `plan A` → **`EFI BCD raw restore blocked: ESP 上的 BCD 正被系统占用（错误 32，共享冲突）…`** → 「用户按 B 键 BCD 回退失败：…」；**全程无 `/import` 行** |
| 22:28:19 | **模态框弹出：标题 "BCD 回退 / BCD rollback"**，经 PostMessage(VK_RETURN) 自动消除（MODAL_GONE） |
| 22:39:45 | STEP 3.5（任务未受影响，继续推进） |
| 22:39:47.20 | **STEP 3.6 bcdboot 成功**（无 BcdOpenStore 错——毒化彻底消失），新加载器 `{c3adc97f-cb01-11f1-8997-e9cfd361e402}` |
| 22:39:48 | description "…-202610182224" / default {c3adc97f} / resumeobject {708d1168} / displayorder / timeout 10，回读校验通过 |
| ~22:41 | 「执行完成」对话框（截图 run4-done.png） |

**铁证**：E: 序列号 `3492ACB8 → 468EBF7C`；哨兵 `BR-222451.txt` 消失（仅剩
`BR-102220.txt`）；ntoskrnl/winload 在位；**live BCD dump：default = {c3adc97f}
（新 E: 加载器）**、resumeobject = {708d1168}（C: 的，保留正确）、displayorder =
[E:, C:, PE markers…]、timeout 10。

### 5.4 结论矩阵

| 场景 | run 2（旧代码） | run 3（新·未锁） | run 4（新·持锁） |
|---|---|---|---|
| B 键行为 | /import 兜底 | 字节级回退成功（6ms） | 干净失败 + 模态错误框 |
| 会话状态 | **被毒化** | 正常 | 正常（bcdboot 11 分钟后仍成功） |
| 任务结局 | **失败**（bcdboot BcdOpenStore c0000043） | 完成（执行完成 + 铁证三连） | 完成（执行完成 + 铁证三连） |

## 6. v2.6.17 进度窗界面验收回顾（run 2，2026-10-18 20:39–21:00）

- 五按钮 + 快捷键全部实机通过：**L**（日志弹窗开关）、**M**（编辑器开 + 连按去重
  1→1 + Esc 关闭 1→0）、**编辑器内打字**（LKPmBz9 全落入名称框、零误触）、
  **K**（管理员 cmd 弹出）、**P**（暂停/继续文案翻转）、**B**（功能链走通——
  当时即暴露本文缺陷）。
- 证据截图：`run2-progress.png`、`run2-logdlg.png`、`run2-editor-typing.png`、
  `run2-editor-typing2.png`（`tools/win-clicker/_evidence/phase6-progress-ui/`）。
- 顺带**证伪** 20261018-2056 审查 H3 的一半：Esc 在编辑器上实机正常关窗，未销毁
  焦点控件/主进度窗（"暂停中关窗"分支未测，维持待办）。
- 另修：日志弹窗 WM_CLOSE 只隐藏不销毁导致的「关不掉」误判（判据改 IsWindowVisible）。

## 7. 过程事故（如实记录）

22:10 `_probe-lock.ps1` 探针直接对 ESP BCD 执行 raw copy，**成功**（任务已结束、
锁已释放），把 BCD 退回准备态（default=C:）——即无意中完成了一次真实的 B 回退，
状态自洽可启动。run 4 随后的完整还原重新确立了终态（default→E: 新加载器）。
教训：探针类脚本对目标文件做写操作前，必须先确认当前 live 状态可弃。

## 8. 遗留与待办

- **WinRE/PE 内的 B 键端到端未测**（本轮两次均在线会话）。PE 下无锁，走 raw 成功
  路径，与 run 3 同代码路径；但端到端仍待补。
- 撞名模态框会阻塞测试钩子约 90 秒（需 EnumWindows + PostMessage(VK_RETURN) 手动
  消除）——钩子只自动接受「破坏性确认」，不自动关信息框。可考虑给钩子补自动消除。
- 旧认知「BFSVC 永久持锁」已修正为「持锁时长可变（≥6min），mount/unmount 可强制
  释放」；`FindWindowW(class, $null)` 在本 agent 通道匹配不到带标题窗口（PowerShell
  把 `$null` 沦为空串标题），窗口发现一律用 EnumWindows。

## 9. 证据索引

`tools/win-clicker/_evidence/phase6-progress-ui/`：run2-online-log.txt（run 2 全程）、
run3-recovery.log / run3-tasklog.txt（run 3 任务日志）、run3-{launch,dismiss,bkey,
done-dialog.png,probe-lock,locktest,storewrite-result,verify,bcd-after,bootmgr-after,
findlog,guilog(-full)}.txt、run4-{launch,forcelock-result,done.png}.txt、
run3-bkey.txt（run 4 按压输出，脚本共用证据名）、bcd-clean-step*.txt（清理前 BCD 操作）、
bcd-pre3-*.txt（清理前后 BCD 枚举）。
