# v1.8.3：PE 侧三处也改零盘符 + 详细日志（方案 A 收尾）

- 日期：2026-09-30
- 版本：v1.8.3（1,800,704 B，DEPLOYED SHA256=`c454bcc7ba76363970861f6b57d086ad41a26a56150c7c11de8f1dc1da5246b7`）
- 快照：`{4ba4c5d3-9c44-486f-b2df-e7fa4961d418}`

## 一、改了什么

PE 侧剩下的三处 `mountvol S: /S` 全部改成零盘符优先：

| 位置 | 原行为 | 现行为 |
|---|---|---|
| `execute_pe_task_line` 的 `clean_bootsequence` | 挂 S: 再 `bcdedit /store S:\...` | 走 `esp_store_path_for_pe()`，零盘符 |
| `add-secondary-entry` | 挂 S: | 同上 |
| `pe_task_execute` 读 `pe-task.txt` | 挂 S: | 同上；`pe-task.txt.done` 的 rename 目标也改成同卷 |
| 该函数内 19 处 `> S:\xxx.txt` 重定向 | 写 S: | 统一过 `rewrite_s_root(cmd, esp_root)` |

配套细节：
- `pe_task_execute()` 末尾的 `mountvol S: /D` 改成**只在真挂了才卸**——零盘符路径下
  执行它是无意义操作，还可能卸掉别人挂的卷。
- `std::fs::rename(task_file, done)` 的 target 改为与 `task_file` 同卷：
  零盘符时 `task_file` 是卷路径，target 也必须是同一个卷路径。

## 二、详细日志（用户明确要求："增加详细日志帮助发现问题"）

`esp_store_path_for_pe()` 把每一步都写进 `detail`，并最终并进
`S:\pe-task-result.txt`：

```text
[ESP] zero drive letter: \\?\Volume{GUID}\EFI\Microsoft\Boot\BCD
[ESP] volume GUID not found; falling back to mountvol S: /S
[ESP] mountvol S: /S FAILED with exit code N
[UNMOUNT_ESP] not needed: zero drive letter path was used
[UNMOUNT_ESP] mountvol S: /D code=N
```

`pe_task_execute()` 读不到配置时也**不再静默 return**——原来直接
`return false`，现场被抹掉；现在会写下"no config at \<路径\>"加 detail。

按 2026-09-29 立下的原则，日志要能区分三种失败形态：
① 收集器根本没起来 ② 收集器跑命令失败（非零退出码）③ 跑成但结果错。
PE 是离线环境、看不到桌面，日志是唯一线索。

## 三、新增的两个纯函数 + 单测（都在 macOS 也编译的 `text_parsing`）

- `rewrite_s_root(command, esp_root)`：把命令串里的 `S:` 换成 ESP 卷根。
- `volume_path_to_device_path(path)`：卷路径 → 设备路径（v1.8.2 已加，本轮补了
  "尾部多一个反斜杠" 的用例）。

单测 `rewrite_s_root_only_replaces_the_esp_drive_letter` 覆盖四种输入：
正常替换、别的盘符路径里的字母 S 不能被碰（`H:\pe-wim1.wim`）、
退回盘符形态时原样返回、`System32` 里的 S 不能动。

**这个单测当场抓到一个真 bug**：第一版只把 `S` 换成卷根、没吃掉 `S:` 后那个反斜杠，
结果路径变成 `卷根\:` 与 `卷根\\`，Windows 直接不认。改成按三个字符吞掉才对。

## 四、实机验证状态

- 桌面侧完整主路径：v1.8.2 已 PASS（盘符前后一致、ESP 盘符零分配）。
- PE 侧：代码已改、编译与单测通过，**尚未在 PE 里实跑**（需要在 PE 启动后验证
  `pe-task.txt` 读写与三条动作）。设计上零盘符失败会自动回退 `mountvol S: /S`，
  行为不会比改动前更差。

## 五、教训（本轮耗时最长的部分）

`text_parsing.rs` 被多轮 shell heredoc / python 内联脚本编辑后变成"`cargo check`
通过但 `cargo test` 报 200+ 错误"的怪状态，报错行内容与磁盘逐字符矛盾。
根因是** heredoc 每层都在吃反斜杠**，把合法 Rust 写成了非法 token，
而错误信息指向的行号又被后续编辑搅乱，看起来像"编译器看错文件"。

有效解法（已固化成本轮做法）：
1. 任何含反斜杠的 Rust 代码，**先用 Write 工具写成 `.rs` 文件**，再 base64 传输，
   python 只做整块 replace，不做逐行 index 切片；
2. 每步之后立刻 `cargo test --no-run` 确认，不等攒一堆再查；
3. 一旦出现" unexplained 的批量错误"，立刻 `git checkout` 回干净基底重做，
   不要在坏文件上继续修补。
