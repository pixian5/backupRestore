# GUI 审计扩展到全部入口 + i18n 扩到 apply 关键路径

- 撰写：2026-10-24 23:55
- 模型：DeepSeek-V4.1-Flash
- 版本：**v2.6.40**
- 触发：用户「根据需要继续做」——承接上一轮自陈的三项未完成（GUI 审计只覆盖主窗口 / i18n 只有 6 条文案 / 任务列表陈旧）

---

## 一、GUI 操作审计：从 1 处扩到全部 4 处 `WM_COMMAND` 入口

### 1.1 覆盖盘点

全仓只有 **4 处** `if message == WM_COMMAND`：

| 窗口过程 | 改造前 | 改造后 |
|---|---|---|
| `window_proc`（主窗口） | ✅ 已有 | ✅ |
| `window_proc_system_choice` | ❌ | ✅ |
| `window_proc_invalid_path_choice` | ❌ | ✅ |
| `window_proc_settings` | ❌ | ✅ |
| `window_proc_pe_dialog` / `system_info_window_proc` / `window_proc_pe` | — | 无 `WM_COMMAND` 分支，无处可加 |

### 1.2 关键改动：审计函数不再依赖 `State`

原来 `audit_gui_action(state: &mut State, ...)` 用不了——那三个对话框的 `GWLP_USERDATA` 存的是 `*mut i32`（只装用户点了哪个按钮的结果），不是 `State`。

改为**不依赖 `State`** 的 `audit_control_action(control_id, notification)`，直接落 `gui.log`。落点与格式与主窗口完全一致，四个入口共用同一个函数。

---

## 二、i18n 扩到 apply 关键路径（新增 8 条）

| key | 场景 |
|---|---|
| `dism_start` | `DISM 开始（等待上限 {0}）：{1}` |
| `dism_result` | `DISM 结果：{0}` |
| `wimlib_start` | `wimlib 开始（覆盖式应用，还原安全描述符）：{0} {1}` |
| `wimlib_result` | `wimlib 结果：{0}` |
| `wait_ms` / `wait_unlimited` | 等待上限的两态 |

新增 `msg_tpl(key, args)` 做 `{0}/{1}` 占位替换，并同步拆出纯函数 `msg_tpl_in`（与 `msg_in` 同理，避免单测里改进程级全局造成并行污染）。

### ★ 安全性核实：翻译会不会破坏 D1/D2 降级判据？

**结论：不会。** 依据：

- `failure_policy::strip_self_log_lines` 只剥「`[YYYY-` 时间戳行」与「命令行回显」（`/Apply-Image`、`/ImageFile:`、`/CheckIntegrity`、`wimlib-imagex apply`）——**全是 ASCII 参数，与语言无关**；
- 失败分类本身靠的是 DISM 的**英文错误串**（`Access is denied` 等）与 **NTSTATUS 码**（`c0000043` 等），不依赖中文；
- 那些 `DISM 结果：退出码 N` 的中文样本只出现在 `failure_policy` 的**单测输入**里，不是生产匹配模式。

→ 先核实再动手，否则很容易做出「翻译完降级判据全失效」的事故。

---

## 三、英语模式真实还原实测（v2.6.40）

```
STEP 2/4 格式化目标卷（diskpart quick）并校验格式化成…      ← 中文（未接入）
DISM start (wait limit unlimited): ["/English", "/Apply-Image", …]   ← 英文 ✅
DISM result: 退出码 0                                        ← 前缀英文，outcome 仍中文
STEP 4/4 完成在线还原（已格式化后写入）…                     ← 中文（未接入）
Operation complete: command, read-back and sidecar bookkeeping all passed  ← 英文 ✅
```

边界如实说明：`CmdOutcome` 的 Display 文本（「退出码 0」）与 STEP 2/4、4/4 的说明**仍是中文**——都在「适当」范围内（只译结论性文案，细节逐步补）。

---

## 四、任务 #121（实测 GUI 切英文）：**仍未完成，如实标记**

尝试了完整链条，受阻于两关（已写进 `tools/switch-gui-language.ps1` 注释）：

1. **会话隔离**：从 prlctl 的 SYSTEM 通道（session 0）跑，`EnumWindows` 只看到本会话窗口 → `MAIN_NOT_FOUND`。GUI 在 session 1，必须借 `launch-as-gui-token.ps1` / `launch-in-session-elevated.ps1` 进入。
2. **ExecutionPolicy=Restricted**：`& script.ps1` / `-File` 抛 `PSSecurityException (UnauthorizedAccess)`，只有 `iex` 内存执行可行；带 `param()` 的脚本要先剥掉 param 行、在外部设同名变量。

session 1 那次返回 `True` 但没产出 `BEFORE/AFTER`，随后**截图确认界面仍是中文**（「语言」下拉框显示"中文"、按钮"刷新环境"）→ **切换未生效，原因未定位**。

**因此「切英文」目前只有两条已被实证的路径**：`BACKUPRESTORE_LANG=en`（日志/CLI）与代码层面的 `selected_language()`。**GUI 界面点击切换这条，端到端未验证。**

---

## 五、验收汇总

| 项 | 结果 |
|---|---|
| 单测 | **236 通过**（新增模板测试 1 条） |
| 交叉编译 + 部署 | ✅ `SHA256=6bf9aaeb…`（截图确认标题 `V2.6.40`） |
| 全链冒烟 | **7/7 PASS**（run25，`--keep` 保留镜像） |
| 英语真实还原 | ✅ `DISM start (wait limit unlimited)` / `Operation complete` |
| GUI 切英文点击 | ❌ 未完成（见第四节） |

---

## 六、剩余

- **i18n 仍是 14 条 vs 259 个 `append_log` 调用点**。`main.rs`(132)、`windows_prepare.rs`(40)、`boot_entry.rs`(26) 未接入；STEP 2/4、4/4 说明、`CmdOutcome` 的 Display 文本都还没译。
- **GUI 切英文的端到端验证**未解决（第四节）。
- CLI `--json` 只接了 `online` / `prepare`；**冒烟脚本的 `prepare` 还没切到 `--json`**。
- 任务列表里 28 个 pending 多为早期轮次陈旧条目，**本轮未做批量清理**——逐个判断风险高于收益，需要时可按版本对照处理。
