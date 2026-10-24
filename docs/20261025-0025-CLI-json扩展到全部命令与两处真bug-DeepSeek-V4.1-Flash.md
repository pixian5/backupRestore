# CLI `--json` 扩展到全部命令（v2.6.41）：统一出口 + 两处真 bug

- **时间**：2026-10-25 00:25
- **版本**：v2.6.41（基线 commit `45d5bfc`，前一个 `272fc23` = v2.6.40）
- **模型**：DeepSeek-V4.1-Flash
- **性质**：改产品代码 + 冒烟脚本

## 一句话结论

`--json` 从「只有 `online` / `prepare` 两处各自接」改为**全命令单一出口**；过程中撞出两个真 bug——`prepare` 因严格参数解析**直接拒绝 `--json`**、以及**我自己上一轮引入的英文模式终态识别回归**（翻译了判据文案却没同步判据）。

---

## ① 统一出口：命令只「上报详情」，`main` 输出那一行

### 原来的毛病

v2.6.40 里 `online` 和 `prepare` 各自在函数末尾 `emit` 一行 JSON，其余命令（`hash` / `recover` / `resume` / `abandon-task` / …）在 json 模式下**一行输出都没有**，判据只能退回解析中文文案——正是 `--json` 要消灭的东西。

但如果让每个命令都自己 `emit`，`prepare` 会输出**两行**：一行自己的、一行 `main` 的。消费方要猜该读哪行（`tail -1` 是运气，不是契约）。

### 现在的做法

```rust
// cli_json.rs
static DETAIL: Mutex<Option<Detail>> = Mutex::new(None);
pub(crate) fn set_detail(task_id: Option<String>, log_path: Option<String>);
pub(crate) fn take_detail() -> Option<Detail>;   // 取走即清空
```

- 命令侧：`set_detail(...)`，**不再**自己 emit；
- `main` 侧：`into_iter()` 之前先留 `command_name` 与 `operation_arg`；命令跑完后若 `cli_json::enabled()` 则 `take_detail()` + `emit()` **一次**。

于是「每个命令在 json 模式下只可能有一行结论」成为结构性保证，而不是脚本约定。

### operation 口径

`operation` 从命令行 `--operation <值>` 推断，与命令自身理解的一致（`probe` / `backup` / `restore-existing` / `create-secondary`）。之前 `prepare` 是靠 `serde_json::to_value(&task.operation)` 转 kebab-case 取得，两套逻辑并存；现在只有一套。

`task_id` 位置不统一（各命令参数位置不同），无法统一推断 → 由命令自己 `set_detail` 上报，没有就 `null`。

### 实测

```
BackupRestore.exe hash --path H:\BRImages\x.wim --json   → {"command":"hash",...,"success":true,...}
BackupRestore.exe abandon-task --task-id 不存在 --json   → {"command":"abandon-task",...,"success":false,"error":"..."}
```

结论行**每个命令都有**了，包括失败路径。

---

## ② 真 bug 一：`prepare` 直接拒绝 `--json`

**现象**：`prepare --json` 报 `unknown prepare option`，但 `online --json` 一直好使。

**根因**：`prepare` 是**严格解析**——兜底分支是硬报错：

```rust
other => return Err(format!("unknown prepare option: {other}"))
```

而 `online` 用 `_ => {}` 忽略了未知选项。所以 `--json` 由 `main` 统一处理这件事，对 online 是「没人管也无所谓」，对 prepare 是「不认识就炸」。

**修法**：`windows_prepare.rs` 的匹配里加显式分支

```rust
// `--json` 由 main 统一处理，这里只须**接受**它
"--json" => {}
```

**教训**：那个位置本就有 2026-10-14 的注释记着「漏接参数分支」的同类事故（`--no-format` 漏接），同一个坑第二次踩。凡是「严格解析 + main 层统一处理」的参数，**main 能识别 ≠ 命令能接受**。

---

## ③ 真 bug 二：英文模式下终态识别失效（我自己引入的回归）

**现象**：`BACKUPRESTORE_LANG=en` 或界面切英文后，进度窗永远停在上一阶段，永远到不了「操作完成」。

**根因**：`text_parsing.rs:750` 用**中文原文**识别终态：

```rust
|| line.contains("操作完成：命令、回读和副档簿记均已通过")
```

而 v2.6.39 把这条文案交给了 i18n 表 → 英文模式下日志写的是英文 → 这行永远不命中。**判据依赖了文案，文案又被翻译，判据就断了**——这是「文案不是契约」的又一次实证。

**修法**：双语都认，并且文案从 i18n 表取（单一来源，两处硬编码不会不同步）：

```rust
|| line.contains(&crate::i18n::msg_in(MessageLanguage::Chinese, "online_done"))
|| line.contains(&crate::i18n::msg_in(MessageLanguage::English, "online_done"))
```

`stage` 文本同时改查表（新增 `stage_done` key：`操作完成` / `Operation complete`）。

**为什么不能只认英文**：历史日志、用户环境默认中文，中文必须继续命中。判据要「任一语言命中即通过」，而不是「跟着当前语言走」。

---

## ④ 冒烟脚本：切 `--json` + 快速失败

- `prepare` 调用加 `--json`，结论行提成 `backup.json`（`LC_ALL=C grep -a '^{"command"' | tail -1`）；
- **快速失败**：`prepare` 若自陈 `success=false` 就立刻 `die`。此前正是因为没有这个，参数被拒后仍空转轮询任务终态，**白等 13 分钟**才发现。

---

## ⑤ 一个只在交叉编译才暴露的错误（E0382）

`--json` 收尾要用命令名，但：

```rust
let mut args = arguments.into_iter();   // move 掉 arguments
...
let command_name = arguments.first()... // E0382: borrow of moved value
```

修法是**在 `into_iter()` 之前**先取 `command_name` / `operation_arg`。

**为什么本机看不见**：这段在 `#[cfg(windows)]` 里，mac 上 `cargo check` 直接把整块剔掉，只有 `./build-win.sh` 交叉编译才报。**凡是 `cfg(windows)` 内的改动，必须交叉编译验证，本机 check 通过不算数。**

---

## 验收

| 项 | 结果 |
|---|---|
| 单测 | 236 通过（v2.6.40 为 235，本轮 +1） |
| 交叉编译 | `./build-win.sh --deploy` 成功，部署 SHA256 `5f218fb6…` |
| 全链冒烟 | `run26` **7/7 PASS** |
| 全命令结论行 | `hash` → `success=true`；`abandon-task`（任务不存在）→ `success=false` + `error` |
| 英文模式 | 真实还原实测 `Operation complete: …` 已被识别为终态 |

---

## 遗留（不在本轮范围）

- **#121 GUI 切英文点击**仍未端到端跑通（会话隔离 + `ExecutionPolicy=Restricted` 两坑），见 `20261024-2355` 与 `tools/switch-gui-language.ps1` 注释；
- i18n 覆盖仍是 14 条，`append_log` 调用点 259 处（main.rs 132 / windows_prepare.rs 40 / boot_entry.rs 26 未接入）——按「只译结论性文案」的边界，不追全量。
