# 阶段 3 重跑通过：resumeobject 修复实锤生效 + resume 复用旧载荷的验证陷阱（2026-10-14 21:05，v2.5.5 验证轮）

> 模型：Sonnet 5｜前序：[阶段 3 根因纠正](20261014-2030-阶段3根因纠正-resumeobject不对称与skip-format漏接-Sonnet5.md)
> 本轮性质：**纯验证轮（无代码改动，版本不升）**——阶段 3（C 入口·离线·create-secondary 到 F:）端到端通过，
> 2.5.5 的 P1 收尾路由与 resumeobject 修复在实机上闭环。同时清掉了阻塞任务与 BCD 残留。

---

## 一、结果

任务 `e28ecb92`（create-secondary，源 C: → 目标 F:，镜像 `c-full-20261014.wim` 31.6GB，菜单名「第二系统-1014」）：
**stage=success，progress=100**。验收五项全过：

| 验收项 | 结果 |
| --- | --- |
| BCD `default` | ✅ `{12e3701c}` 主系统，未被劫持 |
| bootmgr `resumeobject` | ✅ 无残留（修复分支执行了 `/deletevalue`） |
| F: 新 loader | ✅ `{2ad660f3}` desc=「第二系统-1014」 |
| F: 内容 | ✅ 完整 Windows（SYSTEM hive 在，已用 68.8GB） |
| 收尾 | ✅ 一次性启动项清除、暂存目录删除、`Recovery completed` |

**修复生效的直接日志证据**（`H:\BRImages\Recovery.log`）：

```
[12:53:03.38] running bcdedit.exe /store Z:\EFI\Microsoft\Boot\BCD /deletevalue {bootmgr} resumeobject
[12:53:03.97] 引导修复完成：菜单 3 项，显示 10 秒；现有 PE 入口保留
[12:53:05.37] Boot entry cleaned; task marked successful
```

时间线：prepare 20:37 → RE 20:38 → 应用+收尾 ~15 分钟 → 12:53:05(UTC) success → 回桌面。
`task.json` 关键字段：`isActiveSystem=false`、`formatTarget=true`、`role=new-windows` —— 全部正确。

**这同时是 P1（收尾按目标性质路由）的实机验证**——2.4.9 交接清单的「待办 1（最高优先）」就此关闭。

## 二、本轮经过（含两次弯路，都有教训）

### 1. resume 验证是个陷阱（重要经验）

先按用户选择走了「拍快照 → resume」：RE 里报的却是**一模一样的错**。根因：
**resume 只重新武装启动项，复用 prepare 当时暂存的载荷 WIM**（`H:\BackupRestoreRE\<任务>\Winre.wim`），
里面的 Recovery.exe 是**封在 WIM 里的旧二进制**。`build-win.sh --deploy` 只刷 `H:\brwork` 和包目录，不动已暂存载荷。

> **铁律：改了 RE 侧恢复代码后，resume 验证不了修复——必须全新 prepare（才会重新暂存载荷）。**
> 顺带：那次 RE 日志出现「BCDBoot /d 已生效：默认启动项仍指向原主系统」，
> 证明 `assert_boot_default_preserved` 自愈逻辑工作正常（再证 GLM 文档缺陷 A 不成立）。

失败运行会重新武装一次性启动项形成 RE 循环——**靠快照回滚才干净脱身**（这次快照救的是「重试循环状态」，
不是 BCD——BCD 三行回滚覆盖不了这种）。用户判断「下午的灾难不一定因为快照」是对的。

### 2. 阻塞任务的清理（用户拍板的人工路径）

任务 `95ab0a7f`（BootRepaired）**不可 abandon**（不在 `can_abandon_before_target_write`），只能 resume 或人工。
按用户拍板执行人工路径：**备份任务目录 → status.json/task.json 的 stage/status 改 `failed`**。
`incomplete_tasks` 只认终态，改完阻塞即解除——这是设计内的逃生口（程序错误提示本身写着「人工恢复」）。

BCD 残留清理（共删 17 个对象，用户拍板）：
- 本次任务产物：F: loader `{66375ee1}` + 其 resume `{66375ee0}` + 任务 RE loader `{d274226d}` + devopts `{d274226c}`
- 历史「BackupRestore task RE」残留 11 个（SVTestC/ElicitObsolete 等）
- 孤儿 loader 2 个（device=unknown，不在菜单里）
- 摘除 `{bootmgr}` resumeobject 引用（先摘引用再删对象）

保留：主系统 `{12e3701c}` 及其 WinRE 链（`{f530b9e0}` 是主 loader 的 recoverysequence）；
`{152dcf4e}`（device=unknown 孤儿但**在菜单里**，非本次产物，留观察）；F: 上的 WinRE 残条目（阶段 3 格式化后自然失效）。
清理前全量 BCD 存证：`H:\brwork\bcd-before-cleanup-20261014-2020.txt`（4117 行）。

### 3. 两个小事故（记录避免重犯）

- **probe 忘带 `--no-reboot`**：step 验证脚本里 `prepare --operation probe` 没加该参数，prepare 默认武装+立即重启，
  机器自己进了一趟 RE（非破坏性，顺带实测了 2.5.5 载荷能正常引导）。
- **VM 神秘停机一次**：阶段 3 首次进 RE 后约 8 分钟，画面静止、快照层零写入，随后 VM 直接停机（不是重启）。
  重启后任务正常执行完毕。原因未定（可能与我发的一枚 Enter 键、或 Parallels 抖动有关）。
  **这也意味着 GLM 文档「快照+bootsequence 交互导致 RE 不自动执行」的嫌疑不能成立为结论**——本次有快照、任务最终执行了；
  但 20:38-20:46 那段未启动的原因同样未明，两者都只能记为未解。

## 三、环境终态

| 项 | 值 |
| --- | --- |
| VM | Windows 11 C: 在线正常 |
| BCD | default=主系统；displayorder = 主系统 / 第二系统-1014(F:) / {152dcf4e} 孤儿；无 bootsequence、无 bootmgr resumeobject |
| F: | 68.8GB 完整 Windows（本次任务产物，**尚未引导进桌面**） |
| 任务 | e28ecb92=success；旧 95ab0a7f/8c01bf9a=failed（备份在 tasks-abandoned\） |
| 快照 | `br-stage3-clean-2.5.5`（1 个；F: 引导测试前的安全网，测完可删） |
| 镜像 | `H:\BRImages\c-full-20261014.wim` 31.6GB 保留（阶段 5 物料） |

## 四、剩余验收与下一步

1. **F: 引导进桌面**（阶段 3 验收最后一块）：`bcdedit /bootsequence {2ad660f3}` 一次性引导 → 进 F: 桌面 →
   wevtutil 无崩溃循环 + WerFault ≤ 1 → 重启自动回 C:。快照在位。**未做，需用户点头或下轮做。**
2. 阶段 1c（B 入口·离线·取消格式化）——`--skip-format` 修复的实测。
3. 阶段 4（C 入口·在线）、阶段 5（A 入口全链，风险最高放最后）。
4. P3/P4 文案与字面 'C' 技术债；P5 WinRE 注册降级。
