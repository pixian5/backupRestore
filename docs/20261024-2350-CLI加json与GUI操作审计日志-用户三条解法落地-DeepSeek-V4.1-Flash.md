# CLI 加 `--json` 与 GUI 操作审计日志：用户三条解法落地

- 撰写：2026-10-24 23:50
- 模型：DeepSeek-V4.1-Flash
- 版本：v2.6.38（改产品代码：CLI 输出 + GUI 日志）
- 触发：用户指出「可以改代码给 CLI 加 `--json` 输出，而且你在测试的时候也可以把界面调成英文啊。程序本来就是多语言化的呀。而且我之前不是说了吗，让你加日志，说了很多很多次了」

---

## 一、用户说的三条解法，逐一核实后的结论

| 用户的解法 | 核实结果 | 本次落地 |
|---|---|---|
| ① 给 CLI 加 `--json` 输出 | ✅ 成立，且是**终局解法** | 已实现（v2.6.38） |
| ② 测试时把界面调成英文 | ⚠️ **部分成立**——程序确实多语言化，但只管界面文案 | 边界已查清并记录 |
| ③ 给 GUI 所有操作加日志 | ✅ 成立，且是**历史欠账**（两份审查清单写着「未执行，供拍板」） | 已实现 |

**我之前的失误**：三条里两条我早该做——②我不知道程序有双语（从没往这想过）；③两份日志缺口审查文档（`20261016-2250`、`20261017-0125`）的 P0 清单一直挂着「未执行」，我反复绕开它去做别的。用户提了很多次，我没落。

---

## 二、① CLI `--json`（终局解法）

### 2.1 为什么它是终局解法

判据解析自然语言（中文文案）有两个硬伤：措辞随版本变、且要同时应付 GBK（cmd 内建）与 UTF-8（本程序 stdout）两种字节流。`--json` 让**程序自己声明成没成**，判据只认字段。

### 2.2 实现（`crates/backuprestore-cli/src/cli_json.rs`，新增）

- 进程级开关，`main` 里检测 `--json` 一次完成
- `command_result(command, operation, success, task_id, log_path, error)` —— **字段固定，缺失一律 `null`**（不省略键，消费方不必写两套解析）
- 输出恒为**单行** JSON；路径反斜杠交给 `serde_json` 转义，不手写拼接
- 单测 3 条：字段固定可空、Windows 路径转义后可反解、默认关闭

### 2.3 接线

| 命令 | 改动 |
|---|---|
| `online` | `OnlineResult` 增 `log_path`；`--json` 下 stdout 只打印一行结论，中文 `detail` 不再进 stdout |
| `prepare` | `--json` 下用一行 JSON 取代 `TASK_ID=`/`TASK_ROOT=` 两行；`Operation` 枚举走 serde kebab-case，与 `online --operation` 口径一致 |
| `native_gui` | `catch_unwind` 兜底构造的 `OnlineResult` 补 `log_path: None` |

### 2.4 边界（写进模块文档，务必知情）

> 只影响 **stdout 最后一行**；日志文件与 GUI 文案完全不变。
> `error` 字段仍是中文——它是**给人看的诊断串，不是判据输入**；判据只认 `success` / `log_path` / `operation` 这些 ASCII 字段。

### 2.5 实测输出

失败路径（`--image-path` 指向不存在的镜像）：

```json
{"command":"online","error":"[ERROR] 在线操作未完成：…","log_path":"H:\\brwork\\logs\\BackupRestore-online-10356-…\\Online.log","operation":"restore-existing","success":false,"task_id":null}
```

成功路径（全链冒烟 run23）：`success=true`，`error=null`。

---

## 三、② 「把界面调成英文」——成立，但有边界

### 3.1 程序确实是多语言化的（我之前不知道）

- `native_gui.rs:1001` `enum Language { Chinese, English }`
- `ui_text(language, key)` / `tooltip_text(language, key)` —— 中英双语齐全
- `selected_language(state)` 读语言下拉框（`ID_LANGUAGE`，`combo_index == 1` 即 English）
- tooltip 原文：`"Switch the UI language (Chinese / English); all labels, buttons and help text update immediately."`
- CLI 侧另有 `prepare … --ui-language <zh|en>`，写进 `UI_LANGUAGE` 传给 RE 环境（`MessageLanguage`）

### 3.2 边界（这是关键，别指望错了）

| 范围 | 能否切英文 |
|---|---|
| GUI 界面文案（标签/按钮/提示） | ✅ 能 |
| RE/PE 弹窗文案（`MessageLanguage`） | ✅ 能（经 `--ui-language`） |
| **日志文件** | ❌ **不能**——`append_log(path, message)` **不带 language 参数**，日志无语言维度 |
| **CLI stdout** | ❌ 不能（`online` 甚至没有 `--ui-language`，只有 `prepare` 有） |

**结论**：切英文能让**人**看界面时规避中文，但救不了**判据**——判据读的是日志和 CLI stdout，那两处没有语言维度。所以 `--json` 才是判据的解法，切英文是界面/人工排查的解法，两者解决的问题不同。

---

## 四、③ GUI 操作审计日志

### 4.1 做法：一处改动覆盖所有操作

逐个业务分支补日志会漏、会随新功能腐化。改为放在**主窗口 WM_COMMAND 入口**（业务分支之前）：

```rust
let control_id = w_param & 0xffff;
let notification = (w_param >> 16) & 0xffff;
audit_gui_action(state, control_id, notification);   // ← 新增
```

- 新增 `CONTROL_NAMES` 表（由 75 个 `const ID_*` 常量生成）+ `control_name(id)`，未知 ID 返回 `"?"`
- 日志行：`GUI action: control=<名字> id=<id> notification=0x<码>`

各分支**原有的语义日志保留不动**，两者互补：

- 入口这行 = 用户当时**做了什么**（一定在，不看业务成败）
- 分支那行 = 那件事**做成了没有**（业务自己决定写什么）

**教训来源（2026-09-30）**：用户报「点了没反应」，日志里一行都没有，分不清是点击没送达还是送达后静默失败——有入口这一行就能一眼区分。

### 4.2 ★ 顺带核实：旧日志缺口清单不能照抄

两份审查文档（`20261016-2250` 日志缺口 高4/中7/低1；`20261017-0125` 共 11 项）都写着「未执行，供用户拍板」。本次核实后发现**大部分已在不同轮次落地**，只是文档没更新：

| 原 P0 项 | 现状 |
|---|---|
| 装 `std::panic::set_hook` 把 panic 载荷写日志 | ✅ **已修**（`main.rs:453` `std::panic::set_hook`，`main.rs:126` 调用 `install_panic_logging()`） |
| `reg query MiniNT` 非零静默跳过 `wpeinit` | ✅ **已修**（`main.rs:1300` 已 `append_log` 记录退出码与输出摘要） |
| DISM 预检只取 `status`、stdout/stderr 全丢 | 参数缺陷已于 v2.6.22 修复（`/ImageFile:`→`/WimFile:`）；其余待逐条复核 |

→ **别照旧清单重复实施**。要动之前先 `grep` 核实现状。

---

## 五、验收

| 项 | 结果 |
|---|---|
| 单测 | **231 通过**（含 `cli_json` 新增 3 条） |
| 交叉编译 + 部署 | ✅ `SHA256=0307a727…`，2905600 B |
| `online --json` 失败路径 | ✅ 单行 JSON，`success=false` + `log_path` 齐全 |
| 全链冒烟（run23） | ✅ **7/7 PASS**；C3 = `--json success=true`，任务日志 STEP 到 4/4 |
| GUI 审计日志 | ✅ 重启后 GUI 自启（PID 4616 / Session 1），`gui.log` 实测：`GUI action: control=ID_MENU id=1207 notification=0x400` |

冒烟脚本同步改造：`restore` 命令加 `--json`；日志路径直接取 JSON 的 `log_path` 字段（不再用正则从中文 stdout 里抠）；C3 判据升级为「程序自陈 + 结构化交叉验证」。

---

## 六、遗留

- **日志文件仍无语言维度**。`--json` 解决了判据问题，但人看日志仍是中文。若要日志也能切英文，需给 `append_log` 加语言参数并逐条补英文案——工作量大，属产品级改造，待用户拍板。
- `online` 没有 `--ui-language`（只有 `prepare` 有）。若要统一，需给 `online` 补该参数。
- 旧两份审查清单的剩余条目需逐条 `grep` 复核后再定是否实施（本次只核实了 P0 三项）。
