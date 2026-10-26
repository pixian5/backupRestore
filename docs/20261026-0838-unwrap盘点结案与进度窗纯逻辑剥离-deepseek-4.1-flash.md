# unwrap/expect 盘点结案 + 进度窗纯逻辑剥离（v2.6.57）

- 时间：2026-10-26 08:38
- 版本：2.6.56 → **2.6.57**
- 单测：macOS **267 + 49**（新增 8，此前 259 + 49 全绿）
- 交叉编译：`./build-win.sh` 通过，产物 **3,185,664 字节**

---

## 1. 这一轮要解决的两个问题

上一轮收尾时留下两件事，都挂在「代码质量」名下、都不产出新功能：

1. **`unwrap/expect` 到底有多少、有没有真风险**——此前的数字在文档里有三个版本（592 / 605 / 72），谁也说不上哪个对，因为统计口径没定义。
2. **三个「零测试高危模块」怎么补测**——`recovery_progress.rs`（1,996 行）、`boot_entry.rs`（810 行）、`boot_menu_window.rs`（382 行）号称 0 用例。

结论先摆出来：**第 1 件是虚惊（72 处里没有一处能被外部输入打崩），第 2 件的前提是错的（这三个模块在本机根本编译不进去，测试写了也永远不跑）**。两件事的真实形态都比待办里描述的温和，但也因此才需要写清楚，否则下一轮还会有人照着重做一遍。

---

## 2. unwrap/expect 盘点：口径、数据、结论

### 2.1 口径必须先定义，否则数字没有意义

「605 处」和「72 处」都对，差别在**统计对象**：

- **605 处** = 全仓 `.unwrap()` / `.expect(` 字面出现次数（含测试代码）。
- **71 处** = 其中位于**生产代码**里的（534 处在测试代码，无害）。

切分方法一开始也踩了两次坑，记在这里免得重犯：

| 切分规则 | 结果 | 为什么错 |
|---|---|---|
| 找**最后一个** `#[cfg(test)]` 之后算测试 | 生产 89 | 有的文件测试模块不在末尾（`boot_entry.rs` 的 `mod fault_tests` 后面还有函数） |
| 找**第一个** `#[cfg(test)] mod` 之后算测试 | 生产 64 | 一个文件可以有**多个**测试模块，切早了会把后面的函数当成测试 |
| **花括号深度追踪**，逐行判定是否落在 `#[cfg(test)] mod` 块内 | **生产 71 / 测试 534** | 正确 |

第三个口径的脚本已跑过一遍，数字与人工抽查一致（`recovery_progress.rs` 42、`native_gui.rs` 11、`windows_prepare.rs` 4、`main.rs` 3、`boot_menu.rs` 3、`recovery_fault.rs` 3、`online_operation.rs` 2、`winre_registration.rs` 2、`pe_operation.rs` 1）。

**此后引用请用「71 生产 / 534 测试 / 合计 605」**，不要再写 592。

### 2.2 71 处生产代码里，56 处是最廉价的类别

| 类别 | 处数 | 说明 |
|---|---|---|
| `.lock().unwrap()`（Mutex 中毒） | **56** | 锁毒化只可能由「持锁线程 panic」造成，而那种情况下进程本来就在崩溃路径上。改 `?` 传播只是把 panic 换个地方发生，收益为零 |
| 其余 | **15** | 逐个核实见 §2.3 |

56 处分布在：`recovery_progress.rs` 42（其中 2 处写成跨行 `.lock()` + `.unwrap()`，grep 单行匹配会误判成「非 Mutex」）、`native_gui.rs` 8、`boot_menu.rs` 3、`recovery_fault.rs` 3。

> ★ 顺带纠正一个易错点：写脚本分类时**不能只看单行**是否含 `.lock()`。`recovery_progress.rs:741` 和 `1840` 是
> ```
> let online = shared
>     .operation
>     .lock()
>     .unwrap()
> ```
> 这种跨行写法，单行匹配 `'.lock()' not in line` 会把它归到「非 Mutex」，于是得出「42 处里有 2 处不是锁」的错误结论。

### 2.3 剩下 15 处：逐个核实，没有一处能被外部输入打崩

| 位置 | 形式 | 守卫在哪 | 风险 |
|---|---|---|---|
| `main.rs:1124` | `pending.pop().expect("pending length checked above")` | 同函数上方 `pending.is_empty()` 已返回 | 无 |
| `main.rs:3571` | `task.image.as_ref().expect("image path implies image")` | `operation` 决定 `image` 必为 `Some` | 无 |
| `main.rs:4337` | `previous_boot_manager.as_ref().expect(...)` | 上一步刚判过 secondary BCD 状态 | 无 |
| `native_gui.rs:7299` | `tasks.into_iter().next().expect("tasks 非空")` | 上方 `if tasks.is_empty() { return None; }` | 无 |
| `native_gui.rs:7632` | `recheck.unwrap().to_string()` | 上方 `if recheck.is_none() { …return; }` | 无（已改，见 §3） |
| `native_gui.rs:10007` | `win_guid.as_ref().unwrap()` | 上一行刚 `win_guid = Some(candidate)` | 无 |
| `windows_prepare.rs:1142` | `image_path.as_ref().expect("restore image path validated")` | 上游 `validate_operation_inputs` 已返回 Err 分支 | 无 |
| `windows_prepare.rs:1466` | `boot_entry.expect("checked above")` | 同函数上方已判 `is_some` | 无 |
| `windows_prepare.rs:2479` | `source.drive_letter.unwrap()` | 见下 | **已改** |
| `windows_prepare.rs:2491` | `image.drive_letter.unwrap()` | 见下 | **已改** |
| `winre_registration.rs:650` | `destination.parent().expect("恢复目录")` | `destination` 由 `join` 构造，parent 必为 `Some` | 无 |
| `winre_registration.rs:700` | `backup_parts.last_mut().expect("配置文件名")` | `parts` 是非空字面量 vec | 无 |
| `online_operation.rs:158` | `raw.chars().next().unwrap()` | 同一函数上一行 `raw.len() != 1` 已 `return Err` | 无 |
| `online_operation.rs:1395` | `legacy.as_ref().unwrap()` | 上方 `is_some_and(...)` 为真 | **已改** |
| `pe_operation.rs:553` | `sidecar.parent().unwrap()` | `sidecar` 由 `target.join(...)` 构造 | 无 |

**结论：本项目不存在「可被外部输入触发 panic」的 `unwrap` 路径。** 这 15 处全部是「本地代码已经判过、只是没让编译器知道」的不变式。把它们全改成 `ok_or_else` 属于纯风格收益，不值得为它改动 15 个文件、再冒一次回归风险。

---

## 3. 只动 4 处：改「不变式离得太远」的，不改「一眼能看见」的

判断标准不是「有没有 unwrap」，而是**守卫与本处之间的距离**。距离近（同一个 `if` 上下三行内）的留着——它读起来就是一条断言，改成 `?` 反而让读者以为这里真会失败；距离远的必须改。

### 3.1 `windows_prepare.rs` 两处盘符（跨 400 行）

```rust
// 改前：靠 400 行外一行**为 BitLocker 写**的守卫顺带保证
assert_bitlocker_off(
    image_volume.drive_letter.ok_or_else(|| err("image volume has no drive letter"))?,
)?;
…
.saturating_sub(volume_free_bytes(source.drive_letter.unwrap())?)
…
if volume_free_bytes(image.drive_letter.unwrap())? < required {
```

```rust
// 改后：在本函数开头就地取，并说明为什么不靠那条守卫
// 盘符在这里**显式**取，不靠 400 行外那条为 BitLocker 写的
// `assert_bitlocker_off(...ok_or_else...)` 顺带保证：那条守卫
// 一旦被重构掉，这里就会退化成 panic。
let source_letter = source.drive_letter.ok_or_else(|| err("backup source volume has no drive letter"))?;
let image_letter = image.drive_letter.ok_or_else(|| err("image volume has no drive letter"))?;
```

`VolumeIdentity::drive_letter` 是 `Option<char>` **不是** `char`，而且项目里确实存在 `drive_letter: None` 的身份（`volume_identity_at_path` 走 verbatim 卷路径时，注释明写「身份里**绝不**记盘符」）。也就是说这个 `None` 分支是真实存在的一种状态，只是当前调用序不会走到——这正是最该显式化的形状。

### 3.2 `online_operation.rs:1395`：判定与取值拆成两处

```rust
// 改前：条件在 is_some_and 里，取值在下面 unwrap —— 改任一条件都可能脱钩
let own_legacy = legacy.as_ref().is_some_and(|m| { … });
if own_legacy {
    let value = legacy.as_ref().unwrap();
```

```rust
// 改后：判定与取值走同一趟 filter，Option 本身就是"证据"
let own_legacy = legacy.as_ref().filter(|m| { … });
if let Some(value) = own_legacy { … }
```

后面另有两处只判真假，随之为 `own_legacy.is_some()`。

### 3.3 `native_gui.rs:7632`：`is_none` 早退后仍要 unwrap

原写法是「先 `if recheck.is_none() { …; return; }` 再 `recheck.unwrap()`」。改成 `match` 后错误分支与取值分支在同一个表达式里，编译器帮着保证穷尽。

---

## 4. 零测试高危模块：前提就是错的

待办里写着「`recovery_progress.rs` 1,996 行 0 用例、`boot_entry.rs` 810 行 0 用例、`boot_menu_window.rs` 382 行 0 用例」。行数对，但**「补测试」这件事本身做不了**，因为：

```
main.rs:97   #[cfg(windows)]
main.rs:98   mod online_operation;      ← 整份文件在 mac 上根本不参与编译
main.rs:47   #[cfg(windows)]
main.rs:48   mod boot_entry;            ← 同上
main.rs:110  mod recovery_progress;     ← 挂在 native_gui 的 cfg 分支下，同样只在 Windows
```

在这些文件里写 `#[cfg(test)] mod tests`，在 macOS 上**一行都不会被执行**——不是「跑过但没覆盖到」，是编译器压根没打开这个文件。这比没写测试更糟：目录里躺着一个 `mod tests`，`cargo test` 全绿，看代码的人会以为这块有保障。

`boot_entry.rs` 唯一的 `#[test]` 也印证了这点：

```rust
#[test]
#[ignore = "修改测试虚拟机启动对象；需要 BR_ACCEPTANCE_ROOT 和 BR_ACCEPTANCE_SNAPSHOT"]
fn missing_payload_conflict_and_cleanup_retry() { … }
```

它需要一台专门的验收虚拟机 + 已核验快照，平时永远不会跑。

### 4.1 为什么不直接把门控放宽成 `#[cfg(any(windows, test))]`

这是最直觉的解法，也有先例（`boot_cleanup.rs`、`online_log.rs`、`pe_safety` 都是这么写的）。但这三个模块不行：

- `recovery_progress.rs` 通篇是 Win32 FFI（`MessageBoxW`、`SetWindowTextW`、`CreateWindowExW`…），`extern` 块能编译，但 `cargo test` 要**链接**，未解析符号会直接让链接失败。
- `boot_entry.rs` 的 `disarm` / `create_entry` 直接调 `bcdedit.exe` 与卷挂载，去掉 `cfg` 后还会反向依赖一串同样门控的项——上一轮记忆里已经记过这个坑（「别急着把门控模块改无条件——它可能反向依赖别的同样门控项」）。

### 4.2 采用的解法：把**纯逻辑**剥到无条件模块

这与项目里 `text_parsing.rs` 的路子完全一致：那边把「日志解析成状态」剥出来（本机可测），这边把「状态排成给人看的字」剥出来。新模块 `progress_text.rs`：

| 函数 | 职责 | 为什么值得测 |
|---|---|---|
| `format_hms(secs)` | `MM:SS` / `HH:MM:SS` | 3600 是唯一分叉点，少一秒与整一小时的分支不同 |
| `estimate_remaining_secs(elapsed, percent)` | 线性外推剩余秒数 | 进度 0 时是除零、满 100% 时无意义；百分比不单调（日志重放/阶段回退）时总估会小于已用，减法会 panic 或绕回天文数字 |
| `stage_skeleton(&SkeletonInput)` | 4 阶段骨架清单 | 三档标记（✓/▶/·）、隔秒闪烁占位、按操作分叉的阶段标题、步骤数不满 4 时不补行、最后一步 100% 不等于完成 |

调用侧只留一行：

```rust
let body = crate::progress_text::stage_skeleton(&crate::progress_text::SkeletonInput {
    is_backup, online, progress: &progress,
    all_done: latest_stage.as_deref() == Some("操作完成"),
    elapsed_secs: elapsed, latest_percent,
});
```

`SkeletonInput` 把六个输入**显式列出**而不是在函数里回读 `ProgressShared`——这是让它可以被单测任意组合驱动的关键：一旦函数自己去拿全局状态，就再也造不出「进度 0 但不满足条件」这种组合。

### 4.3 8 个用例钉住的行为

1. `format_hms`：`0 / 9 / 59 / 60 / 3599 / 3600 / 3661 / 86399` 全边界。
2. `estimate_remaining_secs`：0% 与 100% 都返回 `None`；50%/100 秒 → 还剩 100；25% → 还剩 300；**1% 时总估小于已用必须收敛到 0 而不是 panic**；已用 0 秒不出错。
3. 骨架：完成/进行中/未开始三档标记 + 记录的时间戳原样附在名称后。
4. 骨架：**奇数秒当前步骤用全角空格占位**（这是为了防横向抖动，删箭头是最容易回归的一条）。
5. 骨架：**最后一步 100% 也不打全勾**，只有终态出现才全勾——提前打满勾会骗人。
6. 骨架：`all_done` 时整列 ✓。
7. 骨架：任务自报两步时不凭空多出 3/4、4/4 两行。
8. 阶段标题按「备份/还原」「在线/恢复环境」两个分叉取正确文案。

行为等价性怎么保证：新函数的实现是**逐字符照抄**原 `refresh_from_log` 里那段内联代码（包括 `format!("{mark} {idx}/{total_steps} {name}{time_str}")` 的空格位置），测试断言用的是**原文案原格式**的完整字符串，而不是子串匹配。这样任何排版漂移都会立刻失败。

---

## 5. 验证

| 项 | 结果 |
|---|---|
| `cargo test`（macOS） | cli **267 passed / 1 ignored**，core **49 passed**（新增 8 个，此前 259 + 49） |
| `cargo check --workspace` | 通过（28 条既有告警，全部为 `cfg(windows)` 假阳性） |
| `./build-win.sh` 交叉编译 | 通过，产物 **3,185,664 字节**（LNK4099 为既有 PDB 引用告警） |
| 版本三处同步 | `VERSION` / `backuprestore-cli/Cargo.toml` / `backuprestore-core/Cargo.toml` 全 `2.6.57`，`Cargo.lock` 已刷新 |

---

## 6. 本轮工具坑：mac 上的 cargo 永远不会打开 `cfg(windows)` 文件

这轮最有价值的发现，值得单列。

改 `online_operation.rs` 时把 `own_legacy` 从 `bool` 改成了 `Option<&BackupMetadata>`，漏改了三处 `if own_legacy`。然后：

- `cargo check --workspace` → **通过，0.07 秒「Finished」**，只有既有告警。
- `cargo test` → **267 + 49 全绿。**
- 交叉编译 `./build-win.sh` → **E0308 ×2**（改第一次后还剩 1 处）。

原因不是 cargo 偷懒，而是：`#[cfg(windows)] mod online_operation;` 在 mac 上**编译器从不打开这个文件**，于是它不出现在 cargo 的 dep-info 里，改了内容也不触发重编——连「编译不过」这件事都不会被发现。

推论（比记忆里那条更强）：

1. `cargo check` / `cargo test` **对 `cfg(windows)` 文件是零信息**，不是「覆盖不足」而是「完全不可见」。
2. `cargo check` 输出的那 28 条告警是**缓存回放**（cargo 会重放上次的 diagnostics），看到它打印告警 + `Finished` 不代表它刚编过。
3. **凡改动落在 `#[cfg(windows)]` 文件里，只有 `./build-win.sh` 能给出结论。** 本轮两次 E0308 全部只有交叉编译能抓到，是本条纪律的又一次实证。

另记一条轻坑：`online_operation.rs` 里 `own_legacy` 的第二个使用点（`:1565`）在闭包里、第三个（`:1422`）在闭包外，**grep 时要一次列全**（我改了第一处就以为改完了，结果交叉编译连报两轮）。

---

## 7. 遗留与下一步

- **`unwrap` 盘点到此结案**，不再作为待办项。后续评审若要新增约束，方向应该是「禁止新增**跨函数**不变式的 unwrap」，而不是「禁止 unwrap」。
- **`boot_menu_window.rs`（382 行）本轮没动**：它的键盘导航（`move_up` / `move_down` / `toggle` / `rename`）实际在 `boot_menu.rs` 里，那边已有 20 个单测；窗口侧剩下的只有 `cursor.min(len-1)` 这类一行夹取，没有值得剥离的纯逻辑。**结论：该模块不需要补测**，从「零测试高危模块」名单里移除。
- **`boot_entry.rs` 仍未测**，可剥的部分是簿记 JSON（`ReBootEntry` 的 serde 字段名、`read`/`write`、`to_residue`）。价值中等（serde 改名会静默破坏续跑），但需要把结构体搬出 `cfg(windows)` 模块，改动面比本轮大，留作独立一轮。
- **`recovery_progress.rs` 已剥出三块纯逻辑**（时间/剩余/骨架），剩下的是 Win32 消息循环与日志增量读取，没有可本机验证的成分。
- 下一项按自定顺序进入：`native_gui.rs` 拆分第二步（5 个自包含对话框，约 2,040 行）。
