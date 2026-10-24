# DISM 失败自动降级 wimlib 与错误模式分类——详细设计（D1/D2 合并轮）

> **定位**：本文是 [`20261024-1508-下一步详细开发方向`](./20261024-1508-下一步详细开发方向-Step-5-Preview.md)
> 中 D1/D2 两项的**展开设计**，可照着直接开工。所有「现状」描述均经本轮源码核对
> （2026-10-24 16:30，`git HEAD = 537b05e`）。
> 排期铁律不变：先测试卷（T: 5 G / F: 350 G）跑通再谈真实目标；破坏性操作前截图确认 VM 空闲。

---

## 0. 一句话结论

DISM 是全链主力 apply 引擎但**失败即任务失败、无兜底、无错误分类**；wimlib 通道已产品化
两轮端到端且自带提权。本轮给三个 apply 落点加「**DISM 失败 → 自动降级 wimlib 重试一次**」，
并给失败回执加**中文因果分类**。改完后：过去会失败的任务有一部分能救回；救不回的也能一眼
看懂为什么。纯函数 + 单测在 macOS 全过，VM 实机只动 T:/F: 测试卷。

---

## 1. 现状核对（2026-10-24 16:30 源码实测）

### 1.1 三个 apply 落点（DISM 主力通道）

| # | 落点 | 路由现状 | DISM 失败时的错误文本 |
|---|---|---|---|
| ① RE 恢复主链（A 入口） | `main.rs:3769` `run_logged("dism.exe", …)` | 按 `overlay_no_acl` 二选一（:3753 读 task.target 标记） | **只有 `dism.exe failed with {status}`**——stdout/stderr 虽 stream 进日志（`run_logged` :4451-4474），但**不回传**，现场归因要翻日志 |
| ② PE 覆盖式 | `pe_operation.rs:688 apply()` | `overlay_no_acl` 二选一（:696 wimlib / :715 DISM） | `command()` :373 已带全文：`{program} 失败：{outcome}\n{输出}`（`decode_output` 解码） |
| ③ 在线链 | `online_operation.rs:699-716` | 同款二选一 | `dism()` :140 只有 `DISM failed: {outcome}`（退出码），stderr 落日志不进错误 |

三处的 `overlay_no_acl` 路由是 #110 的既有语义（「已含 Windows + 取消格式化 → wimlib」），
**本设计不动它**；加的是「走了 DISM 却失败了 → 再试一次 wimlib」。

### 1.2 相关既有件（直接复用，不重写）

| 件 | 位置 | 用途 |
|---|---|---|
| `locate_wimlib` | `wimlib_apply.rs:73` | 校验 exe+dll 两文件，缺失时报**可操作**错误（点名两个文件名 + 「不会回退 DISM」） |
| `apply_overlay` | `wimlib_apply.rs:106` | 执行 wimlib apply（带 ACL 还原），统一走 `windows_command::run_program` |
| `CmdOutcome` | `windows_command.rs:31` | `Exited(code)` / `TimedOut{after_ms, terminated}`；`is_success() = Exited(0)` |
| `dism_timeout::wait_ms()` | `dism_timeout.rs:88` | 三入口可配（env `BACKUPRESTORE_DISM_TIMEOUT_SECS` / 任务 `DISM_TIMEOUT_SECS` / settings），**默认 0=不限制** |
| `MessageLanguage` | `pe_safety.rs:160` | 中英双语文案模式（failure_policy 同款） |
| `OnlineResult.detail` | `online_operation.rs:26/106-113` | 失败时 detail = `[ERROR] 在线操作未完成：{error}；…`，**GUI 直接弹窗展示**（`native_gui.rs:8416-8430` `show_message(result.detail)`）——中文因果进 TaskError message 即直达用户 |
| `failure_policy.rs` | 全模块 | 纯函数 + 跨平台单测的既有范式（不依赖 Windows），分类函数放这里 |

### 1.3 失败错误到达用户的路径（D2 的接线图）

- **在线链**：`execute_inner` 的 `Err(TaskError)` → `final_message` → `OnlineResult.detail` → GUI 弹窗 + 日志。**改 TaskError message 即可，GUI 零改。**
- **PE 链**：`pe_operation::execute` 的 `Err` → 进度窗错误展示 + 任务日志。同上。
- **RE 链**：`Err` → `Recovery.log` + failure_policy 的连败处理流程；用户靠 `prlctl exec` 翻日志。分类进 message 即进日志。

---

## 2. D2 设计：apply 失败错误模式分类 + 中文因果

### 2.1 纯函数签名（放 `failure_policy.rs`，跨平台）

```rust
/// apply 失败的错误模式。
pub(crate) enum ApplyFailureKind {
    AccessDenied,   // 0x80070005 / RestoreDirMetadata——wimlib 的主场，必须降级
    DiskFull,       // 0x80070070 / not enough space
    Busy,           // 0x80070020 / being used by another process——wimlib overlay 语义可能不受影响
    ImageCorrupt,   // /CheckIntegrity 相关 / data error / corrupt——重试无益，不降级
    TimedOut,       // CmdOutcome::TimedOut——降级（且 wimlib 侧不继承被玩坏的超时上限，见 §3.4）
    Unknown,        // 兜底
}

/// 从 apply 失败的输出尾部 + 退出码推断原因。`language` 决定中文/英文文案。
/// 判据优先序固定：AccessDenied > ImageCorrupt > DiskFull > Busy > TimedOut > Unknown
/// （一个文本可能命中多个模式时，取语义最强的；单测锁死）。
pub(crate) fn classify_apply_failure(
    output_tail: &str,
    exit_code: Option<i32>,
    timed_out: bool,
    language: MessageLanguage,
) -> (ApplyFailureKind, String);   // 第二元素是一句「中文/英文因果 + 处置建议」
```

**文案示例（zh）**：

| Kind | 因果文案（一句） |
|---|---|
| AccessDenied | 「目标卷上受保护目录的权限改写被拒（访问被拒）。这类失败正是 wimlib 通道的设计目标，将自动改用 wimlib 重试。」 |
| DiskFull | 「目标卷剩余空间不足，镜像无法完整写入。请清理目标卷或换用更大卷后重试；不要将部分写入视为成功。」 |
| Busy | 「目标卷正被其它程序占用（句柄未释放）。请关闭占用程序后重试。」 |
| ImageCorrupt | 「镜像文件损坏或不完整（校验未通过）。重新制作备份镜像；同一镜像换任何引擎都会失败。」 |
| TimedOut | 「apply 超过等待上限被终止（上限可由设置调整）。卷上可能留有半成品，重跑前建议先格式化目标卷。」 |
| Unknown | 「apply 失败，原因未识别。请把本日志发给维护者；原始输出已保留在下方。」 |

### 2.2 匹配模式表（DISM 与 wimlib 两套 stderr 形态都要覆盖）

- DISM：`0x80070005`/`0x80070070`/`0x80070020`、`Error: N`、`RestoreDirMetadata`、
  `not enough space`、`being used by another process`、`integrity`、`Data error`、`corrupt`。
- wimlib：行首计数教训（`text_parsing.rs` 的 `parse_wimlib_progress` 已处理：wimlib 行首
  带 `xxx/yyy` 计数），错误行形态 `wimlib-imagex: ERROR: ...`。
- 大小写不敏感；`exit_code` 作为辅助信号（DISM 的 5/2/… 只是弱信号，**以文本为主**）。

### 2.3 基础设施：让失败错误带上输出尾部（D2 的前置）

三处落点里两处的错误**不带输出**（§1.1 表 ①③），分类函数没有输入。最小改动：

- `main.rs:run_logged`（`#[cfg(windows)]` :4436，macOS 编译不受影响）：`!status.success()`
  分支里**读日志文件尾部 8 KB** 附进 `Err`：`{program} failed with {status}\n--- 输出尾部 ---\n{tail}`。
  child 已 `wait()`、两个 reader 线程已 `join()`，此刻读日志文件是安全的。
  收益不止 apply：所有 `run_logged` 调用点（bcdboot 等）的可归因性一起变好。
- `online_operation.rs:dism()` :122：失败分支同样读 `log_path` 尾部附进 `Err`。
  （`run_program(out=Some(log_path))` 把输出直接追加进任务日志，尾部即本次 DISM 输出。）
- `pe_operation.rs:command()` 已带全文，不改。

### 2.4 单测清单（macOS `cargo test` 全过才算完）

1. 六个 Kind 各至少两条真实形态样本（DISM 一条 + wimlib 一条）→ 锁死分类不漂移；
2. 判据优先序：构造同时命中 DiskFull 与 Busy 的文本 → 断言取优先者；
3. `timed_out=true` 时无视文本直接 TimedOut；
4. zh/en 两种 language 的文案非空且不同；
5. 尾部截断（8 KB 边界）不 panic；
6. `run_logged` 尾部附加逻辑的纯函数部分（`format_command_failure`）单独测——把「读文件」与
   「拼消息」分开，读文件部分留在 windows 侧。

---

## 3. D1 设计：DISM apply 失败 → 自动降级 wimlib 重试一次

### 3.1 三个降级点（接线位置）

| 落点 | 现状代码 | 降级接线 |
|---|---|---|
| RE 主链 | `main.rs:3769 run_logged("dism.exe",…)` 的 `?` | 改为 `match`：`Err(e)` → `maybe_fallback_to_wimlib(e, …)` |
| PE 链 | `pe_operation.rs:715 self.command("dism.exe",…)` 的 `?` | 同上，`self.command` 的 Err 进判据 |
| 在线链 | `online_operation.rs:716 dism(&[…], log_path)?` | 同上 |

三处**共用一个**判据/执行函数（放 `wimlib_apply.rs` 或新 `apply_fallback.rs`），
避免「PE 降级了、RE 没降级」的分叉前科（failure_policy 模块注释里写过的教训）。

### 3.2 降级判据（纯函数，macOS 单测锁死）

```rust
pub(crate) struct FallbackDecision {
    pub allow: bool,
    pub reason: &'static str,   // 进 WARN 日志
}

/// 是否允许把本次 DISM apply 失败降级为 wimlib 重试。
/// - `stage_is_apply`：只有 apply 阶段失败才降级（format/verify/boot 失败不降级——
///   那些阶段失败时卷状态不可假定，且 wimlib 也未必能救）；
/// - `format_already_done`：format 已执行（卷已空）→ wimlib 覆盖写无既有数据风险；
///   或本就是 overlay 语义（不删卷上文件）；
/// - `kind`：ImageCorrupt 不降级（同一镜像换引擎必然同样失败，省时间）；
///   TimedOut/AccessDenied/Busy/DiskFull/Unknown 一律降级一次。
pub(crate) fn decide_fallback(
    stage_is_apply: bool,
    format_already_done: bool,
    kind: ApplyFailureKind,
    wimlib_available: bool,   // locate_wimlib 两件是否就位
) -> FallbackDecision;
```

**决策表**：

| 条件 | 降级？ |
|---|---|
| 非 apply 阶段失败 | 否（按原错误失败） |
| ImageCorrupt | 否（直接失败 + 中文因果，WARN 不写降级行） |
| wimlib 缺件 | 否（报 `locate_wimlib` 的可归因错误——点名缺哪个文件） |
| 其它任何 kind（含 Unknown） | **是，降级一次** |

「一律降级一次」是有意的简单规则：多跑一次 wimlib 的代价是几分钟，换来一次真实救回机会
（0x80070005 类失败 wimlib 是主场）。DiskFull 救回概率低也降——DISM 的 scratch 行为
在某些临界场景会失败而 wimlib 能过，且双失败后的两条中文因果比一条更有归因价值。

### 3.3 降级执行与日志契约

```
[WARN] DISM apply 失败（{中文因果第一行}），目标卷已格式化，自动降级 wimlib 重试一次
[wimlib 开始（覆盖式应用，还原安全描述符）：…]        ← 既有 apply_overlay 日志
[wimlib 结果：退出码 0]
```

- 降级行必须出现在日志里（验收判据之一，证明降级链路真的跑了）；
- wimlib 也失败 → 任务失败，错误信息**合成两级**：
  `DISM apply 失败：{DISM因果}\n自动降级 wimlib 重试仍失败：{wimlib因果}\n{原始尾部}`；
- wimlib 成功 → 任务继续（verify/boot 不变），日志记 `降级成功`。

### 3.4 超时语义的设计决策（需用户认可，见 §6）

`dism_timeout::wait_ms()` 是**同一个**可配上限，DISM 和 wimlib 现在共用。若用户设了短上限
防 DISM 挂死，降级后的 wimlib 会**同样被秒杀**——降级失去意义。
**设计决策：降级路径下 wimlib 传不限制（INFINITE）超时**。理由：用户设短上限的意图是
「杀死挂死的 DISM」，不是「这个任务只许跑 N 秒」；且该决策让「DISM 超时 → wimlib 完成」
成为可实机验收的降级成功剧本（§4 层 2）。在 `apply_overlay` 加一个显式的 `wait_ms` 参数
（默认仍 `dism_timeout::wait_ms()`，降级调用点传 INFINITE），而不是偷偷改全局行为。

### 3.5 明确不做的事（边界，防范围蔓延）

- 不改 `overlay_no_acl` 的既有路由（#110 语义）；
- prepare 期/校验期/引导期失败不进降级判据；
- 不做「重试 N 次」——只降级一次，两次都失败即任务失败（连败计数交给 failure_policy 既有策略）；
- 不碰主备反转（远期项，本轮之后评估）。

---

## 4. VM 实机验收剧本（只动 T: 5 G / F: 350 G 测试卷）

每层开跑前：`prlctl capture` 截图确认 VM 空闲（磁盘清理后的桌面状态已知健康）。

### 层 1（必做）：损坏镜像 → 分类正确、不降级、双因可归因

1. T: 5 G 空卷；取 `br-tdata-2629.wim`（857 KB 冒烟素材）**截断复制**成损坏镜像；
2. CLI 还原（format 路径）→ DISM `/CheckIntegrity` 失败；
3. 验收：任务失败；日志有 ImageCorrupt 中文因果行；**无降级 WARN 行**（证明判据拦住了
   无益重试）；GUI/CLI 回执含同一句因果。

### 层 2（必做，降级成功主证）：DISM 超时 → wimlib 降级完成

1. F: 350 G 目标；用 `br-2629-20261023.wim`（37.8 GiB，当前 C: 血脉同款）；
2. 任务 env 注入 `DISM_TIMEOUT_SECS=90`（准备期落 `RecoveryTask.env`，或进程 env
   `BACKUPRESTORE_DISM_TIMEOUT_SECS=90`）；
3. DISM apply 跑到 90 秒被终止（TimedOut）→ 判据放行 → wimlib 以**不限制**超时跑完；
4. 验收：任务 success/100；日志有降级 WARN 行 + `降级成功` 行；**哨兵法**（F: 根放
   `BR-时分秒.txt`，还原后消失——注意 F: 若为格式化路径哨兵法适用）；卷序列号变化；
   EventID=1000 / WerFault 零新增；截图。
5. 本层同时验证「用户设短超时不再误杀合法还原」这一新能力。

### 层 3（必做，占用类救回）：锁句柄占用 → DISM 失败 → wimlib overlay 语义绕过

1. T: 5 G；放一个**不在镜像里**的占位文件 `T:\br-lock.bin`，用 PowerShell 起一个
   持有其句柄不退出的进程（`Start-Process` 挂 `[System.IO.File]::Open`）；
2. CLI 还原（format 路径）；
3. **分支预案**：若 diskpart format 先因占用失败（可能发生）→ 该剧本退化为
   「format 失败不降级」的正确行为展示（日志验证无降级行），换层 2 作为降级成功主证；
   若 format 过、DISM apply 失败（Busy 分类）→ wimlib 覆盖写不碰锁文件 → 降级成功；
4. 验收：锁文件还原后仍在（overlay 语义铁证）+ 镜像文件完整还原 + 任务 success。

### 环境与安全约束

- 宿主 209 GiB 可用（16:26 治理后），F: 37.8 G 还原无压力；**不拍快照**（血教训：
  CoW 会把 85 GB 重写量落增量层）；回滚 = 代码 revert + 测试卷重刷；
- 每层结束 `df -h /System/Volumes/Data` 记录空间。

---

## 5. 工作量与版本

| 步骤 | 内容 | 出口判据 |
|---|---|---|
| 5.1 | D2：分类函数 + 文案 + 单测（纯函数部分） | `cargo test` 全过（含新单测 ≥10 条） |
| 5.2 | D2 接线：`run_logged` / `dism()` 失败带输出尾部 | 现有单测不回归；RE/在线两处错误文本变长 |
| 5.3 | D1：判据函数 + 单测 + 三个降级点接线 + `apply_overlay` 超时参数 | 判据单测全过；`cargo check` 干净 |
| 5.4 | VM 实机三层剧本 | 三层判据全过 + 截图 + 日志证据 |
| 5.5 | 收尾：版本 +0.0.1（→ 2.6.34）、文档落档 + 三处索引 + 两级记忆 + commit+push | 同既有硬规矩 |

---

## 6. 需用户拍板项（本轮设计新增/更新）

1. **§3.4 超时语义**：降级路径下 wimlib 用「不限制」超时（推荐：是，否则短上限会连杀
   wimlib，降级失去意义）；
2. **§3.2 决策表**：DiskFull/Unknown 也一律降级一次（推荐：是，简单规则 + 双失败双因果）；
   ImageCorrupt 不降级（推荐：是）；
3. 1508 文档 §5 遗留拍板项仍在：`C:\Users\Public\BackupRestore-actual-20261005-0724` 去留；
   #38 全链复跑专场「彻底不做/以后再做」；D5 冒烟套件是否纳入每次 deploy 常规流程；
   主备反转是否记入 backlog。

---

## 7. 附：本轮之后的两张任务卡（简版，细节不变，指向 1508 文档）

- **D5 冒烟套件**：`tools/smoke/` 一键脚本（T: 备份 → B 入口还原 → 判据回读），0.5 天；
- **D4 命名分叉**：`online_log.rs` 按 operation 分叉 `BackupRestore-backup-*` / `-restore-*`，
  旧目录兼容读取 + 单测，0.5 天。
