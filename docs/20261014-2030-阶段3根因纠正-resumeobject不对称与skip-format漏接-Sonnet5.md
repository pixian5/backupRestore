# 阶段 3 事故根因纠正：真因是 resumeobject 不对称，三个「实锤缺陷」均系误判（2026-10-14 20:30，v2.5.5）

> 模型：Sonnet 5｜前序：`20261014-1904-项目交接说明-当前状态与待改清单-GLM-5.md`（§3 三缺陷）、
> `20261014-1900-阶段3重跑事故全记录-GLM-5.md`
> 本轮性质：**接手复核 + 根因纠正 + 两个真实 bug 修复**，已部署 2.5.5（SHA256 `876da555…`）。

---

## 一、结论先行

GLM-5 交接文档 §3 列为「最高优先待修」的三个实锤缺陷，**经源码与实机日志双向核对，全部不成立**。
阶段 3 失败的真实根因是另一件事，文档完全没提到：

> **`preserve_primary_boot_manager` 对 `resumeobject` 的处理不对称**——
> 快照里没有该字段、而 BCDBoot 为新系统凭空加了一个时，代码既不删除它，
> 自己的三项联合校验又把这种情况判为失配，于是任务在 `BootRepaired` **确定性失败、重试必然同样失败**。

已修复，另外顺带发现并修掉**第二个真实 bug**：`--skip-format` 参数解析分支漏接，
导致「离线 + 取消格式化」整条通道在 prepare 阶段直接报错。

---

## 二、三个「缺陷」逐条核对

### 缺陷 A「assert_boot_default_preserved 检测到 default 被挪但只报错停机，不自愈」→ **不成立**

`main.rs` 的 `assert_boot_default_preserved`（本轮核对时在 3266 行）实现是**回读 + 自愈 + 再回读**：

```rust
let current = read_default(log)?;
if current.eq_ignore_ascii_case(&previous.default) { return Ok(()); }   // 没被改，直接过
append_log(log, "BCDBoot 改动了默认启动项（… -> …），立即改回；…")?;
run_logged("bcdedit.exe", &["/store", &store_arg, "/default", &previous.default], log)?;  // ← 真的回写
let after = read_default(log)?;
if !after.eq_ignore_ascii_case(&previous.default) {
    return Err(err("无法把默认启动项改回原主系统，拒绝继续后续破坏性步骤"));  // 只有回写失败才报错
}
```

它只在**自愈本身失败**时才报错。而且本次运行里它成功了（见下条证据）。

### 缺陷 B「任务失败后收尾不恢复 default，机器留在『默认启动指向半成品 F:』的危险态」→ **不成立**

三份独立证据都指向 default 已被正确恢复：

1. **RE 日志里的写入与返回**：
   ```
   running bcdedit.exe /store Z:\EFI\Microsoft\Boot\BCD /set {bootmgr} default {12e3701c-c361-11f1-90a2-c222b5348ef7}
   [stdout] 操作成功完成
   ```
   `{12e3701c…}` = 主系统 C: 的 loader（与基线一致）。

2. **失败前最后一次校验回读**（`/enum all /v` 里的 Windows 启动管理器块）：
   ```
   default                 {12e3701c-c361-11f1-90a2-c222b5348ef7}     ← 主系统，正确
   resumeobject            {66375ee0-c7bd-11f1-8245-e08a96f7d18f}     ← ⚠️ 唯一失配项
   displayorder            {152dcf4e-c674-11f1-895e-8d900a1bd081}
                           {12e3701c-c361-11f1-90a2-c222b5348ef7}
                           {66375ee1-c7bd-11f1-8245-e08a96f7d18f}     ← 与写入的三项完全一致，正确
   bootsequence            {d274226d-c7a5-11f1-8983-b6bb373e1952}     ← 该任务的 RE loader，仍武装
   ```

3. **当前实时 BCD**（本轮实测）：`default {12e3701c…}` 仍是主系统。

> 注意一个取证陷阱：用 `Select-String "displayorder"` 过滤日志会**滤掉 displayorder 的续行**
> （续行只有缩进 + GUID、没有字段名），看起来像「displayorder 只剩一项」。必须看未过滤的原始块。

### 缺陷 C「bcdboot 之后主动回读 bootmgr default，与快照不一致立即回写」→ **已实现**

这正是缺陷 A 那个函数在做的事，调用点就在 bcdboot 之后（`main.rs:2770`）。无需新增。

---

## 三、真实根因（证据链闭合）

### 机制

1. 任务快照 `bcd-before-bootmgr.txt` 里的 Windows 启动管理器块（= `previous` 的权威来源）：
   ```
   default                 {12e3701c-c361-11f1-90a2-c222b5348ef7}
   displayorder            {152dcf4e-c674-11f1-895e-8d900a1bd081}
                           {12e3701c-c361-11f1-90a2-c222b5348ef7}
   toolsdisplayorder       {b2721d73-1db4-4c62-bf78-c548a880142d}
   timeout                 10
   ```
   **没有 `resumeobject` 字段** → `previous.resume_object == None`（已用 GBK 解码精确提取确认）。

2. BCDBoot 为新 F: 系统**凭空新增**了 `resumeobject {66375ee0-…}`
   ——注意它与新 loader `{66375ee1-…}` 是兄弟 GUID（只差末位），是 BCDBoot 一起创建的那对对象。

3. 恢复代码只在快照**有**该字段时才写回，**从不删除** BCDBoot 新增的：
   ```rust
   if let Some(resume_object) = &previous.resume_object { /* 写回 */ }
   // 快照为 None 时：什么也不做 ← 问题在这
   ```

4. 而它自己的校验把这种情况判为失配：
   ```rust
   let resume_object_matches = match (&verified.resume_object, &previous.resume_object) {
       (None, None) => true,
       (Some(actual), Some(expected)) => actual.eq_ignore_ascii_case(expected),
       _ => false,          // ← (Some({66375ee0}), None) 落到这里
   };
   ```

5. → 报 `Boot Manager default or secondary display order could not be preserved`，
   任务停在 `stage=boot-repaired`。

### 为什么这是最关键的一点：失败是确定性的

条件不随时间变化（快照永远没有 resumeobject，BCDBoot 每次都会加一个），
所以 **`resume` 重试会在同一处以同一原因失败**。阶段 3 不修这个就永远过不去，
而文档指向的三处都不是阻塞点——按文档去改，会改掉本来正确的自愈逻辑，并且阶段 3 照样失败。

### 修法（已实施）

`preserve_primary_boot_manager` 补一个对称分支：快照没有、实时有 → 删掉。

```rust
} else if parse_boot_manager_state(&live).and_then(|s| s.resume_object).is_some() {
    run_logged("bcdedit.exe",
        &["/store", &store_arg, "/deletevalue", "{bootmgr}", "resumeobject"], log)?;
}
```

删掉它还有独立的正确性价值：留着会让**休眠恢复走进第二系统**。

---

## 四、顺带发现的第二个真实 bug：`--skip-format` 解析分支漏接

### 现象

GUI 取消勾选「格式化目标分区」+ 走离线通道时，prepare **直接失败**：
`error: unknown prepare option: --skip-format`。整条「离线 + 覆盖式应用」通道走不通。

### 根因

管线两头都接好了，唯独少了中间那一行解析分支：

| 位置 | 状态 |
| --- | --- |
| `native_gui.rs:5794` 推送 `--skip-format` | ✅ 有 |
| `main.rs:81` usage 字符串声明该参数 | ✅ 有 |
| `windows_prepare.rs:124` `PrepareOptions.format_target` | ✅ 有 |
| `windows_prepare.rs:543` 传入 PrepareOptions | ✅ 有 |
| `windows_prepare.rs:822` 写进 `TargetSpec.format_target` | ✅ 有 |
| **参数解析 `match` 里的 `"--skip-format"` 分支** | ❌ **缺失** |

兜底分支是 `other => return Err(err("unknown prepare option: {other}"))`——**硬报错，不是静默忽略**。

> 编译器其实一直在提示：`let mut format_target = true;` 从未被改写，
> 触发 `unused_mut` 警告。这个警告就是漏接的指纹。
>
> GLM 交接文档 §5 称「`windows_prepare.rs`：CLI `--skip-format`」已实现，这一条不符事实。
> 阶段 1a（取消格式化）之所以 5/5 通过，是因为它走的是**在线**通道（进程内执行，不经过
> prepare 命令行），没有触及这条路径。可选阶段 1c（离线 + 取消格式化）一跑就会撞上。

### 验证

修复后 `prepare --operation probe --source-drive T --skip-format` 不再报 unknown option，
而是走到了后面那个合法的「存在未终结任务」检查（见 §5）。

---

## 五、当前环境状态（本轮实测）

| 项 | 值 |
| --- | --- |
| 版本 | **2.5.5**（三处同步），部署 SHA256 `876da555…`（两处部署点一致） |
| BCD default | `{12e3701c-c361-11f1-90a2-c222b5348ef7}` = 主系统 C:（**从未被留在 F: 上**） |
| BCD 残留 | `resumeobject {66375ee0…}` 指向 F: 系统的恢复对象（本轮代码已能在下次任务中清掉）；displayorder 含 F: loader `{66375ee1…}` |
| 阻塞任务 | `95ab0a7f` stage=**BootRepaired** |
| F: | 375GB 卷、已用约 74GB（半成品系统） |
| 快照 | **无** |

### 关于阻塞任务的重要纠正

GLM 文档说「需先 `abandon-task` 或继续」。**`abandon-task` 会拒绝它**：

```rust
pub fn can_abandon_before_target_write(self) -> bool {
    matches!(self, Prepared | BootRequested | RecoveryStarted | Preflight)   // 不含 BootRepaired
}
```

`BootRepaired` 属于 `is_destructive_boundary()`（目标卷已被改写），只能 **`resume`**
（它在 `can_resume_after_interruption()` 里）或人工处置。

而 `resume` 会重跑失败的那一步——**正好是本轮修复的那一步**，所以它既是清除阻塞的唯一干净路径，
也是对修复的最佳验证。但它会写 BCD，按用户铁律必须**先拍快照 + 等用户确认**，本轮未执行。

---

## 六、对事故叙事的保留意见（证据不足，不写成结论）

GLM 文档的灾难链条是「重启后 F: 起不来 → 自动修复 → 触发挂起的系统还原 → 66% 卡死」。
与证据对不上的地方：

- 失败时 `bootsequence {d274226d…}` **仍武装着**，指向该任务自己的 RE loader
  （其 device options `{d274226c…}` 的 ramdisk 路径是 `\BackupRestoreRE\95ab0a7f-…\boot.sdi`）。
  一次性启动项优先于 default，所以下次重启的预期行为是**再进 RE 重试**，而不是进 F:。
- default 当时已是主系统 C:，不存在「开机被劫持进 F:」的机制。

可能的真实路径（未验证，不作结论）：进 RE 重试 → 在同一处确定性失败 → 循环；
或用户在启动菜单里手动选了 F: 条目。**按用户铁律「不把未验证因果当经验写文档」，此节仅作存疑记录。**
真要定论需要 `H:\BRImages\Recovery.log` 里该时段的后续 run 记录与 Windows 事件日志。

---

## 七、给接手方的提醒

1. **别按 GLM 文档 §3 去改 A/B/C**：A 的自愈逻辑是正确的，改了反而丢掉现有防线；C 已实现。
2. **取证别用关键字过滤 bcdedit 输出**：`displayorder` 的续行没有字段名，一过滤就看起来「只剩一项」。
   看 BCD 状态要取未过滤的整块。
3. **任务快照文件是 GBK**（`cmd >` 重定向），`Get-Content` 默认解码会把中文字段名弄坏，
   要用 `[IO.File]::ReadAllText($f, [Text.Encoding]::GetEncoding(936))`。
4. **留意编译器警告**：`unused_mut` 这次就是漏接参数的指纹。部署前跑一次
   `cargo check --all-targets` 看警告，不要只看「编译通过」。
5. **`BootRepaired` 不可 abandon、只可 resume**——别拿 abandon-task 去撞它。
