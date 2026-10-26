# `#[ignore]` 安全测试门禁化，并拆开一条「永远跑不到的安全判据」

**日期**：2026-10-27 02:05 ｜ **版本**：2.6.64（**未升版本**，依据见末节）
**结论**：8 条 `#[ignore]` 用例中 **3 条真实跑通并纳入门禁**（`pass=3 fail=0 skip=5 ran=3`）；
另发现一条被误标的测试——它把「Windows 可验的安全属性」和「必须 PE 才能验的回滚」塞在一个函数里，
导致**最该自动化的「不往源卷写」从未被跑过**，已拆分并首次跑通。

---

## 1. 起点：实际是 7 处，不是 5 处

整体分析时我按 5 处记的，这次 `grep -rn "#\[ignore"` 得 **7 处**：

| # | 位置 | 测试 | 声明的门槛 |
|---|---|---|---|
| 1 | `pe_operation.rs:983` | `disposable_vhd_preflight_and_index_two_apply` | 需要 Windows 管理员 |
| 2 | `secondary_registry.rs:237` | `offline_secondary_mapping` | 已核验快照 + 独立离线测试系统 |
| 3 | `winre_registration.rs:865` | `offline_registration_on_test_partition` | 真实 PE + 全新快照 + 独立夹具 |
| 4 | `text_parsing.rs:2677` | `replay_real_wimlib_log` | 依赖 `.test-artifacts` 真实日志 |
| 5 | `main.rs:5741` | `secondary_menu_repair_native` | 第二系统任务 + ESP + 快照 |
| 6 | `online_operation.rs:1891` | `online_capture_append_keep_and_data_apply` | Windows 管理员 + DISM |
| 7 | `boot_entry_fault_tests.rs:5` | `missing_payload_conflict_and_cleanup_retry` | 验收夹具 + 快照 |

在 mac 上 `cargo test` 只报 **1 个 ignored**——其余都在 `cfg(windows)` 文件里，编译器根本不打开。
这就是「cfg(windows) 零信息」这个老坑落在**安全判据**上的样子：它们不会红，也不会绿，是不存在。

## 2. 第一次真跑就撞出两个问题

用 SYSTEM 通道（管理员）跑 #1，**FAILED**：

```
panicked at crates\backuprestore-cli\src\pe_operation.rs:1144:9:
[BR-IMG-001] Windows reagentc.exe … 需要 Windows PE …
test result: FAILED. 0 passed; 1 failed
```

1144 行是 `pe_safety::run_restore(&mut backend).unwrap_err()` 之后的断言：
期望错误里含 `TEST_INJECTED_BOOT_FAILURE`，实际拿到的是「reagentc 需要 PE」。

**两个问题**：

1. **ignore 的理由不准**：写的是「需要 Windows 管理员」，实际还要求 **Windows PE/RE 环境**。
2. **更要紧**：1144 行**之前**的断言全部通过了——VHD 创建、DISM 捕获两次、数据索引应用、
   `payload=="second"`、哨兵存活、**卷序列号不变**、3 条错误路径（镜像缺失 / 索引越界 /
   目标卷上放镜像）都不写源卷。**这些正是「不往源卷写」这条安全属性**，它们在 Windows 下完全可验；
   只有最后「格式化后引导失败 → 回滚」那段需要 PE。

一个函数装两类门槛，结果就是**整条永远 skip**，可验的那一半也从没被自动跑过。

## 3. 拆法：一个函数 → 两个测试 + 两个 helper

```
create_disposable_vhd_fixture()      ← 建 128 MiB VHD、挂 V:、格式化、写哨兵、DISM 捕获首索引
teardown_disposable_vhd_fixture()    ← detach 并断言 V: 释放
disposable_vhd_preflight_index_two_apply_and_reject_without_write   ← Windows，进常规门禁
disposable_vhd_boot_failure_rollback_in_pe                          ← 需 PE，理由改写准确
```

`GetLogicalDrives` 的 `#[link]` 声明从函数内提到测试模块作用域，两个 helper 共用。

**验收标准是断言对账，不是「变绿」**（拆分的唯一风险是搬漏断言，产品代码一字未改）：

```
OLD=25  NEW=25  MISSING=0  EXTRA=0
```

原测试 25 条 `assert`，拆分后（helper + 两个测试）合计 25 条，逐条匹配
（`dir.join` → `fx.dir.join` 归一化后比较）。**一条没漏、一条没多。**

首次真跑结果：

```
PE_SAFETY_TEARDOWN_PASS
PE_SAFETY_NO_WRITE_ON_REJECT_PASS
test result: ok. 1 passed; 0 failed; 336 filtered out; finished in 5.46s
```

## 4. 门禁脚本 `tools/test/ignored-acceptance.sh`（218 行）

用法：`tools/test/ignored-acceptance.sh [--list | --only NAME]`

三态 + 退出码纪律：

- `CASE=<名>=PASS` / `=FAIL` / `=SKIP reason=<缺什么>`
- 有 FAIL → 退出 **1**
- **一条都没真跑（`ran=0`）→ 退出 2**：缺夹具不等于通过
- 判据只认 ASCII 结构化字段 `test result: ok. N passed; 0 failed`，不 grep 中文

实测：

```
CASE=no_write_on_reject=PASS
CASE=online_capture_indexes=PASS
CASE=wimlib_log_replay=PASS
CASE=secondary_registry_map=SKIP reason=missing_env: BR_SECONDARY_MAPPING_ACK …
CASE=winre_offline_reg=SKIP reason=missing_env: BR_WINRE_TEST_ACK …
CASE=secondary_menu_repair=SKIP reason=missing_env: BR_ACCEPTANCE_SNAPSHOT …
CASE=boot_entry_fault=SKIP reason=missing_env: BR_ACCEPTANCE_SNAPSHOT …
CASE=boot_failure_rollback=SKIP reason=requires_windows_pe_or_re
SUMMARY pass=3 fail=0 skip=5 ran=3
```

**5 条 SKIP 里有 4 条依赖第二系统或验收快照**——而 `F:` 第二系统分区已于 2026-10-27 00:35 删除。
这不是脚本的缺陷，但意味着「第二系统创建 / 菜单修复」这条产品主链的验收**在本机暂时无法重跑**，
要跑必须先重建测试分区。这一点记在账上，不假装 SKIP 是「通过」。

## 5. 两条通道不能混（本轮最费时间的坑）

| 需求 | 通道 | 原因 |
|---|---|---|
| 编译 | `--current-user`（用户会话） | 源码在 `X:`，它是**用户会话里的共享映射**；SYSTEM 账户 `cd /d X:\` 静默失败 |
| 运行 | `prlctl exec`（SYSTEM） | 建 VHD / DISM / diskpart 需管理员；`--current-user` 是标准 token，会拒 |
| 复制夹具 | `--current-user` | 源在 `X:`，用 SYSTEM 复制**静默失败**，会被误判成「夹具缺失」 |

所以流程固定为：**用户会话编译 → 复制 exe 到 `H:\br-ignored\` → SYSTEM 运行**。

## 6. 两个静默失败：从日志里抓路径

脚本第一版用 `sed -n 's/.*(\(.*\))[[:space:]]*$/\1/p'` 从
`Executable unittests src\main.rs (H:\...\exe)` 取路径，取空，白等了 **300 秒**超时才报错。

两个原因叠在一起：

1. `zsh` 的 `echo` 把路径里的 `\b` 当**退格**解释：`H:\br-target` 显示成 `H:r-target`；
2. BSD `sed` 的 `[[:space:]]*$` 对行尾空白处理与 GNU 不同。

**改法不是换正则，而是判据分离**：

- 编译是否成功 → 看 build log 里的 `Finished`
- 产物路径 → `dir /b /o-d H:\br-target\debug\deps\backuprestore_cli-*.exe | head -1`
  （按修改时间降序取最新），**完全不解析日志文本**

## 7. 变异验证（判据必须能单独失败）

| 变异 | 期望 | 实测 |
|---|---|---|
| 把 filter 改成不存在的名字 | `FAIL` + 退出 1 | `CASE=no_write_on_reject=FAIL`、`pass=0 fail=1`、`MUT1_EXIT=1` ✅ |
| `--only __no_such_case__`（无用例执行） | 退出 2 | `SUMMARY ran=0` + `ERROR: RAN=0`、`MUT2_EXIT=2` ✅ |

第一次做变异时 `MUT1_EXIT=2`，但那是脚本自身的路径解析 bug（编译产物取不到），**不是判据判红**——
按「判据红时先证明读数那一侧对」的纪律，先修了读数，再重做变异才成立。

## 8. 裁决：不进 pre-push

`ignored-acceptance.sh` **不接入 pre-push**：它需要 VM 在线、真建 128 MiB VHD、跑 3 次 DISM，
每次 push 跑不起（实测单次约 33 秒，但要占用 VM 且会写 `V:` 盘符）。
定位是**按需手动执行的阶段性验收**，与 `release-check.sh` / `sentinel.sh` 同级。

## 9. 为什么没升版本

产品代码**一字未改**：改的只有 `pe_operation.rs` 的 `#[cfg(test)]` 段与新增的工具脚本，
都不进二进制。按项目规则「只加/只改测试与工具 → 不升版本」，故保持 **2.6.64**。

单测数量：CLI **294** + core **56**（mac 侧不变，因为被拆的测试在 `cfg(windows)` 里；
VM 侧 `filtered out` 从 335 → 336，多出的正是拆出来的新测试）。

## 10. 遗留

- **4 条 SKIP 的前提是第二系统 / 验收快照**：`F:` 已删，要重跑须先重建测试分区
  （`secondary_registry_map` / `secondary_menu_repair` / `boot_entry_fault` 三条）。
- `winre_offline_reg` 需要独立测试卷 + 已挂载 ESP，同样未备。
- `boot_failure_rollback` 只能在 PE/RE 内跑；下次进恢复环境做备份/还原时可顺手执行一次。
- 门禁目前只在文档与经验库登记，**没有 CI 入口**；若要长期执行，需定节奏（如每次发版前）。
