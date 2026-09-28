# WinRE 进度窗口「冻住/不显示百分比」根因与修复（2026-09-29）

> 状态：已修复并交叉编译（aarch64-pc-windows-msvc release 通过，1,747,456 字节），离线单测 44+28 全绿。
> 触发：用户进入 WinRE 跑备份时，进度弹窗没有任何详细信息（无百分比、无步骤），疑似死机。

## 结论（先说答案）

进度窗口**是存在的**（`recover-env` 一进 WinRE 就在 `main.rs:750` 用 `recovery_progress::spawn` 弹出一个原生 Win32 窗口：
阶段文字 + 进度条 + 详情框），每 500ms 轮询活动 Recovery.log 刷新（`recovery_progress.rs:264`）。

但**进度条在备份/还原时是「冻在 0%」的**，根因是日志流的一处 bug，不是窗口不显示：

## 根因

DISM 的进度条用 `\r`（回车）**原地覆盖**写（`[=  12.3% =]\r[=  13.4% =]...`），**不带 `\n`**。
而日志流函数 `stream_to_log`（`main.rs`）原先用 `reader.read_until(b'\n')` 才把一行写进日志——
没有 `\n` 之前，所有 `[= xx% =]` 都卡在管道缓冲区里，**直到 DISM 跑完吐出 `\n` 才一次性落盘**。

进度窗口每 500ms 读日志，读到的自然是空的 → 进度条永远 0%、详情框看不到任何中间状态，
看上去就像死机。阶段文字「正在备份系统分区…」（来自 `run_logged` 立即 `append_log` 的
`running dism.exe ...` 那行）其实是有显示的，只是没有百分比，让人误判为卡死。

## 修复

1. **`stream_to_log` 按 `\r` 实时落盘**（`main.rs`）：把 `read_until(b'\n')` 改成逐字节读，
   遇到 `\n` 或**裸 `\r`** 都当作一行边界立即 `writeln!` + `flush`；`\r\n` 去重不重复写。
   这样 DISM 每次 `[= xx% =]` 刷新都会实时进入日志，进度窗口据此推进百分比。
   - 顺带把 `stream_to_log` 及其依赖的 `Read/BufReader/Arc/Mutex` 导入从 `#[cfg(windows)]`
     解门限——它是纯 std 实现、不依赖 Windows API，解门限后可在宿主端做离线单测。
2. **备份加编号步骤**（`main.rs` `Operation::Backup` 分支）：在 4 个阶段切点打
   `STEP n/N <名称>` 标记（准备环境 / 捕获镜像 / 校验 / 完成并重建恢复环境），
   `classify_log_line`（`text_parsing.rs`）识别后阶段标题显示「步骤 n/N：<名称>」，
   并在详情框过滤掉步骤行（避免与阶段标题重复）。
   - 还原路径同样受益：冻结修复让 `/Apply-Image` 的百分比实时可见；编号步骤本次未在还原分支补，
     后续如需一致可照搬（解析逻辑已通用）。

## 验证

- 新增单测 `dism_carriage_return_progress_streams_each_update`（`main.rs`）：用
  `[= 10% =]\r[= 20% =]\r[= 100% =]\nDone\n` 喂 `Cursor` 流，断言日志含 10/20/100 与 `Done`、
  且 `\r\n` 不重复写行 → 直接锁住回归。
- 新增单测 `step_markers_render_as_numbered_stage_and_stay_out_of_detail`（`text_parsing.rs`）。
- `cargo test`（cli+core）全绿；`cargo check --target aarch64-pc-windows-msvc` 干净；
  `build-win.sh` release 产物 1,747,456 字节。

## 待办

- VM 实机验收：在真实 C: 备份时截图确认窗口显示「步骤 2/4：捕获系统分区镜像」且进度条实时推进。
  （前序任务 WindowsRE 迁出方案的真实备份/还原验收仍待你授权建快照后进行。）
