# v1.8.5：PE 进度窗口显示全部步骤/当前步骤/总进度 + 修掉断电续跑死结

- 日期：2026-09-30
- 版本：v1.8.5（1,812,992 B，SHA-256 `e9b795b8bb142a38ad713895710ea8efa622c3abcdd5de8761904f5cef13ca39`）

## 一、用户反馈与根因

> PE 里跑，屏幕显示「正在准备恢复环境...」，没有详细信息，应该显示更多内容：
> 全部步骤、到哪步了、当前步骤的进度（百分比和进度条）

根因不是"没写显示代码"，而是**编号步骤从来没被识别出来**：

`classify_log_line` 用 `line.strip_prefix("STEP ")` 只匹配**行首**，而真实日志行是

```text
[2026-09-29T03:36:32.259862400+00:00] STEP 1/4 准备备份环境（挂载卷、校验、必要时代原恢复环境）
```

`STEP` 前面带着方括号时间戳，所以这条分支从来没进去过，进度窗口只能靠
"running dism.exe" / "Recovery.exe started" 这几个模糊关键词猜阶段，
DISM 一开跑就长期停在「正在准备恢复环境… / 正在还原系统分区…」。

## 二、改了什么

### 1. 统一的步骤标记解析（纯函数，macOS 可测）

`text_parsing::parse_step_marker(line) -> Option<(编号, 总数, 名称)>`，
在行内找 `STEP ` 而不要求行首。`classify_log_line` 与步骤跟踪器共用它。

认不出的（无编号、`0/4`、`9/4`、无名称）一律返回 `None`——没有 `n/N`
就谈不上"第几步"，给它一个标题反而会让用户误以为它在序列里。

### 2. 步骤进度计算（纯函数 + 5 条单测）

- `StepProgress { steps, total, current, current_percent, overall_percent }`
- `StepTracker`：**跨增量累积**。进度窗口每次只读到日志增量，
  而"一共几步/已完成几步"必须累积，否则两次刷新之间就忘了走到哪。
- 总进度 = `(已完成整步数 + 当前步骤内百分比/100) / 总步数`，夹在 0–100。

### 3. 进度窗口显示（`recovery_progress.rs`）

```text
✓ 1/4. 准备备份环境（挂载卷、校验、必要时代原恢复环境）
▶ 2/4. 捕获系统分区镜像（DISM，百分比见进度条）
· 3/4. 校验镜像并计算哈希、写入元数据
· 4/4. 完成（自建启动项/载荷的清理在 WinRE 出口统一做）
—— 当前 66% · 总进度 42% ——

<日志尾部>
```

- 进度条改显示**总进度**（单看当前步骤百分比会让人以为卡住：第 4 步的 90%
  其实整体早过 70%）；没有编号步骤时退回用当前百分比。
- 详情区 314 → 414 像素，窗口 820×500 → 860×600。
- `ProgressShared` 增加 `steps: Mutex<StepTracker>`。

## 三、顺带修掉一个致命缺陷：断电续跑根本走不通

同一轮实机测 `--test-fault power-loss-image-applied`（镜像灌完、启动项未修的
最深断电点），拿到**可复现的失败**：

```text
Detected durable interrupted task ... stage=ImageApplied
Pending boot recovery abandoned: our boot entry is unusable:
  invalid task: payload WIM hash differs from the prepared one; refusing to re-arm
```

根因：`create_entry` 按设计在 DISM 注入**之前**跑（v1.7.11 顺序要点——
建条目只需要"镜像卷上有一个合法 WIM"，干净副本就够），所以 `boot-entry.json`
记下的是**注入前**干净原件的哈希（`1060a552…`）。注入后载荷 WIM 被覆写，
哈希变成 `7164305a…`，而记录没刷新。于是 `rearm()` 拿活载荷比对过期记录
必然不符 → 拒绝重武装 → 机器永久停在 `image-applied/75`。

**这条路径在修复前对任何任务都是死的**，不只是断电场景。

修复：`boot_entry::refresh_payload_hash()`，在注入完成、覆盖到镜像卷之后
把簿记刷新成实际哈希（幂等，哈希没变就不写文件）。`ReBootEntry::write`
相应改成 `pub(crate)`。

## 四、测试

`cargo test --workspace`：cli 72 + core 23 全绿。新增 8 条：

- `parse_step_marker_works_on_real_timestamped_log_lines` —— **回归锁**，
  用真实带时间戳的日志行
- `parse_step_marker_returns_none_for_non_step_lines`
- `step_progress_reads_numbered_steps_across_increments`
- `step_progress_deduplicates_repeated_steps`
- `step_progress_ignores_malformed_step_lines`
- `step_progress_without_numbered_steps_reports_nothing`
- `step_progress_clamps_percentages`
- 更新一条过期断言（无编号的 `STEP xxx` 不再算编号步骤）

## 五、仍未验证

- **新进度窗口还没在 PE 里实看**（需要再跑一次完整任务）；
- **断电续跑修复后还没重测**——要重跑 `power-loss-*` 故障并确认能续跑到 success；
- 容量/性能、`create-secondary`、Secure Boot 仍未验。
