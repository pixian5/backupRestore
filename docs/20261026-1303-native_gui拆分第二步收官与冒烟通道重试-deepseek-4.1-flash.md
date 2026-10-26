# native_gui 拆分第二步收官（第 4 块：两个模态框）与冒烟脚本的通道重试

- 日期：2026-10-26 13:03
- 撰写模型：deepseek-4.1-flash
- 版本：2.6.58（**未升版**：纯结构搬迁，行为等价）
- 部署 SHA256：`90729146739d93304b41033482e942bff1c8888e0530bed337a6508ea5865f51`（VM 内构建，`H:\brwork`）

---

## 0. 一句话结论

评估文档里「五个自包含对话框」的最后两个搬完了 —— `native_gui.rs` 从 **11,628 → 9,580 行**，
四块子模块合计 2,144 行。同批修掉冒烟脚本一个会**误报失败**的缺陷：`prlctl` 通道偶发返回空串，
脚本只读一次就判定失败，而文件内容其实是对的。

---

## 1. 这一块搬了什么

| 项 | 内容 |
|---|---|
| 新模块 | `native_gui/choice_dialogs.rs`，**435 行** |
| 搬走 | `window_proc_system_choice`、`ask_system_drive_handler`、`window_proc_invalid_path_choice`、`ask_invalid_image_path`、`system_choice_log_path`，及 `CHOICE_LANGUAGE`/`CHOICE_GENERIC_OFFLINE`/`INVALID_PATH_LANGUAGE`/`INVALID_PATH_EMPTY` 四个静态量 |
| 留在父模块 | `ID_CHOICE_*` 六个常量 |
| 对外暴露 | `pub(super) unsafe fn ask_system_drive_handler(...)`、`pub(super) unsafe fn ask_invalid_image_path(...)`、`pub(super) static CHOICE_GENERIC_OFFLINE` |
| `native_gui.rs` | 9,986 → **9,580** 行（−406） |

**常量为什么必须留父模块**：`CONTROL_NAMES` 审计表（`WM_COMMAND` 入口统一记
`GUI action: control=<名> id=<id>`）和主窗口的 `WM_COMMAND` 分发都要按 id 反查名字。
Rust 子模块的项对父模块**不可见**，常量搬下去就只能复制一份 —— 而「同一份数据有两个副本」
正是这次拆分要消灭的东西。所以四块的规矩统一：**常量留父、函数搬走、入口标 `pub(super)`**。

逐字比对（脚本 `zip`，不靠肉眼）：414 行块 vs 子模块正文，**只有 3 处差异**，全是 `pub(super)` 前缀。

---

## 2. 运行期验收：为什么只验得到一个框

两个框的触发条件差别很大：

- **无效镜像路径**（`BackupRestoreInvalidPathChoice`）：镜像路径为空时点「读取镜像」就会弹，
  点【取消】直接返回 —— **零副作用**，可以自动化。
- **系统分区处理方式选择**（`BackupRestoreSystemDriveChoice`）：只在备份/还原目标是
  **活动系统卷**时出现，而点开它的那一下已经在「要真建任务」的路上了 —— 无法零副作用触发。

所以新脚本 `_t-choice-dialogs.ps1` 只测第一个框，判据：

```
image_path_len=0  precondition_empty=1
tasks_before=48
CHOICE_DIALOG_FOUND hwnd=2491068   waited_rounds=0
choose_button_present=1  cancel_button_present=1
closed=1
tasks_after=48  tasks_unchanged=1
main_window_alive=1
```

`tasks_unchanged` 是这一项真正的判据：取消必须**在建任务之前**返回，否则会在
`H:\brwork\tasks` 里留一个任务目录。`main_window_alive` 守的是那条老坑 ——
模态循环曾经用 `PostQuitMessage` 退出，会给共享同一消息队列的主窗口也投一个 `WM_QUIT`，
2026-09-30 抓到过「点完对话框整个程序消失」。

---

## 3. 同批修掉的假红：读一次就判失败

回归整条冒烟时，探测那一项红了，而且**输出是空白**：

```
-- 第六入口「探测」→ 选中且无关字段隐藏
   FAIL  [expected after: probe_checked=1]

```

单独 `type` 同一个文件，内容完好：

```
after: probe_checked=1
restored: backup_checked=1
```

根因是 `prlctl` 通道**偶发返回空串**（这条早就写在纪律里：「读文件/读状态必须重试，
否则抽风会被当成文件为空」），而 `gui-clicks.sh` 的 `run_case` 只读一次。

修法：读结果文件重试到非空（最多 5 次，间隔 1 秒）；真的读不到就报
`FAIL [READ_EMPTY after N attempts: <文件>]` —— **把「读不到」和「判据不满足」区分开**，
否则下次又会有人拿一条空输出去怀疑产品。

★ 这是同一个道理第三次出现：**判据红的时候，先证明读的那一侧是对的**。
今天上午是 GBK 吞换行、中午是 `0x00F2` 当成 `BM_GETCHECK`，这次是空返回。
三次的产物都是「一条看起来像产品缺陷的红判据」。

---

## 4. 拆分第二步整体账（现查 `wc -l`）

| 块 | 子模块 | 行数 | `native_gui.rs` 之后 |
|---|---|---|---|
| 起始 | — | — | 11,628 |
| 1 | `settings_dialog.rs` | 579 | 11,085 |
| 2 | `logs_viewer.rs` | 443 | 10,659 |
| 3 | `image_admin_window.rs` | 687 | 9,986 |
| 4 | `choice_dialogs.rs` | 435 | **9,580** |

合计搬出 **2,048 行**，主文件减重 **17.6%**。四块全部「常量留父、函数搬走」，全部实机点击验收过。

---

## 5. 剩下的候选（评估文档里明确不建议做的）

- `create_task`（1,258 行）：与 `State` 深度耦合，搬出去等于把 `State` 变成半公开；
  要动只能在**同文件内**按九步各提一个小函数，且必须与一次真实备份/还原验收绑定。
- `run_online_operation`（956 行）、`layout_operation`（518 行）：同属主路径，风险不对称。
- PE 桌面窗口（约 1,000 行）：另一套交互，与主窗口共用大量静态量与消息循环，拆开收益小。

**结论：拆分到此为止**，除非有明确诉求。评估文档第 3 节的反方论据仍然成立 ——
这个文件是稳态代码，拆分解决的是导航问题，不是缺陷问题。

---

## 6. 可复用的钉子

1. **「常量留在能被看见的那一层」**：子模块能用父的私有项，父看不见子的。凡是被
   `match`/审计表按值引用的 ID，一律留父，只把实现搬下去。四块一致，后续加块照抄。
2. **搬迁的验收要分两层**：编译只能证明符号解析，运行期必须点到；而点到需要
   **零副作用的触发路径** —— 找那种「点开就弹、点取消就回」的入口（读取镜像 + 空路径），
   实在没有（系统分区选择）就如实写明「没验到」，而不是拿编译过去凑。
3. **判据输出为空 ≠ 判据不满足**：任何跨进程/跨通道读取都要重试，并在失败信息里
   标明是「没读到」还是「值不对」。这一条今天第三次付学费。
4. **逐字比对脚本要断言首尾行**：块的首行是注释横幅、末行是 `}` 加前一行 `result`，
   四个断言行把「行号漂移」挡住 —— 这轮 `last` 的边界判断第一次就猜错了（少一个空行），
   是断言把它拦下来的。
