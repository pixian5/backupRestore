# 可能的问题与修复方向（Claude 5）

备份还原全链路只读复核。

- 记录时间：2026-10-01 10:36，亚洲/新加坡。
- 执笔模型：Opus 5。
- 基线：`HEAD=960a9ef`（main），VERSION=2.0.5。复核开始时基线是 `d0fee75`/2.0.1，期间另外几个会话推进到 2.0.5；**行号与结论已重新对齐到 960a9ef**（我读的是落地后的工作区文件），其间的代码改动只有进度窗口时钟/呼吸解耦、`reset_start_time` 接线、`verify_hash` 按需哈希与注释里的文档路径改名，未触及本文任何一项。
- 方法：只读静态复核，逐行读取 `prepare → 启动通道 → WinRE 恢复 → 清理` 与 `在线 / 预装 PE` 三条入口的实际代码；每条结论都附行号，未实机复现。
- 性质：**本文只报结论，未修改任何执行逻辑，未改版本号**。

## 零、先说结论

主干设计扎实：卷 GUID 身份而非盘符、载荷四件套哈希、原子写、阶段状态机、破坏性操作前先落可续跑状态、`boot_cleanup` 的"删完回读"。这些不需要再动。

问题集中在**三类**：

1. **分支条件取自"探测结果"而不是"任务意图"** —— 于是失败被吸收成另一种"正常情况"并判成功（第 1、2 条）。
2. **"损坏"被当成"缺失"** —— 于是本该硬失败的场景降级成"跳过校验继续"（第 3 条）。
3. **同一纪律只落在部分入口** —— 包括我上一轮自己留下的两处不一致：新建的进程树约束没接到 RE 路径，新建的格式化复验没接到核心还原路径（第 4、8 条）。

共 17 项，其中 3 项我判为 P0。

## 一、P0：PE 系统卷保护在"真的是系统卷"时失效

`native_gui.rs:7695,7701`（格式化）与 `:7734,7740`（引导修复）。两个缺陷叠加：

```rust
"cmd /c if exist {d}:\\Windows\\System32\\Config\\SYSTEM (echo SYS) else (echo NOSYS) > S:\\format-check.txt"
...
is_system = text.contains("SYS");
```

- **重定向只绑定 `else` 分支**。cmd 把 `> file` 绑给紧邻的那条命令，也就是 `(echo NOSYS)`。命中 `if exist`（即目标**真的**是系统卷）时，`SYS` 写到 stdout，而这里 `out_file=None`，输出被丢弃，`format-check.txt` 根本不生成。
- **`"NOSYS".contains("SYS")` 为真**。子串判定对两种结果都成立，这个判据本身不具区分力。

合成后的实际行为，危险方向正好没被挡住：

| 目标卷 | 文件状态 | `is_system` | 结果 |
|---|---|---|---|
| 系统卷 | 不生成（首次） | `false` | **无需 `--allow-system` 就格式化系统卷** |
| 系统卷 | 上一轮残留 | 取决于陈旧内容 | 行为由历史文件决定 |
| 非系统卷 | 写入 `NOSYS` | `true`（子串命中） | 过度拒绝，需要 `--allow-system` |

`bcdboot` 处同形，后果是**静默跳过引导修复**：还原完的系统卷不会被修引导，而任务不报错。

修法：判据改成"读到完整单行 `SYS`"或 `trim()=="SYS"`，并把重定向落到整条 `if/else` 外层（或直接别用 cmd 分支，在 Rust 里 `Path::is_file()` 判定）。

## 二、P0：还原"无 SYSTEM hive"被当成数据卷还原并判成功

`main.rs:2320-2409`。分支条件是**对结果的探测**：

```rust
if matches!(task.status, Stage::ImageApplied | Stage::BootRepaired)
    && target_root.join("Windows").join("System32\\config\\SYSTEM").is_file()
{ /* 修引导 */ } else if matches!(...) {
    append_log(log, "data-volume restore: target has no SYSTEM hive; skipping BCDBoot")?;
    store.write_transition(task, Stage::Success)?;   // ← 判成功
}
```

关键：`TargetRole` 只有两个取值（`lib.rs:346-349`），且**完全由操作类型决定**（`windows_prepare.rs:698-702`：`CreateSecondary → NewWindows`，其余 `→ ExistingWindows`），与"目标是不是数据卷"毫无关系。所以这条 `else` 分支不是在服务某个合法的"数据卷还原"任务类型，而是在吸收"apply 没产出 Windows"这件事。

叠加分流规则后影响面更大：GUI 把非当前活动系统的卷全部分流到在线路径（`native_gui.rs:4758-4772`），所以 **WinRE 还原路径基本只用于系统卷**。在这条路径上"目标没有 SYSTEM hive"几乎只有一个含义——系统还原没成。现在的处置是写 `Stage::Success` 并重启，用户看到"成功"，机器起不来。`create-secondary` 同样落进这条分支。

修法：需要一个"这个镜像本该含 Windows"的权威信号，而不是探测结果。可用的最低成本方案是在 `prepare` 阶段（此时目标卷上的旧 Windows 还在）探测一次并记进 `TargetSpec`，apply 后据此断言；镜像侧另一个可选信号是在捕获时把"源卷含 Windows"写进副档。

## 三、P0：副档"损坏"被当成"不存在"，连带跳过目标容量预检

两处：

- `windows_prepare.rs:1462`：`let metadata = crate::read_index_metadata(...).ok();`
- `main.rs:1969`：`if let Ok(metadata) = read_index_metadata(path, image.index) {`

`read_index_metadata` 的失败原因至少有三种：文件不存在、JSON 损坏、`wim_index` 与请求索引不符。这两处把三者压成一个 `None`。而 core 的 `load_sidecars`（`image_metadata.rs:41-62`）恰恰明确区分过"缺失跳过、损坏硬错"——纪律在，没落到这里。

"缺失即视为第三方 WIM 并跳过校验"是**有意设计**（注释写明），所以问题不在跳过本身，而在**损坏也走同一条路**。后果链条完整：

```
副档 JSON 损坏/索引错位
  → metadata = None
  → minimum = 0（windows_prepare.rs:1476-1486）
  → 目标容量预检等于不存在
  → 格式化目标卷（旧系统此刻已被擦除）
  → DISM apply 中途空间不足失败
  → 任务 Failed，但旧系统已经没了
```

容量预检存在的全部意义就是阻止这条链，吞掉损坏正好让它失效。

GUI 侧同一问题还多一层：`native_gui.rs:4563` 用 `metadata_check.is_err()` 统一判定，`:4584` 的文案写死"未在此镜像旁找到备份档案文件（可能被移动或删除）"。副档明明在、只是坏了的时候，这句话是**事实错误**，而对话框默认引导用户点"仍要继续"。

## 四、P1：格式化回读弱于项目自有标准（我上轮留下的不一致）

`main.rs:2472-2514` 的 `verify_partition_identity_after_format` 比对磁盘/分区 GUID、分区类型、磁盘号、分区号、偏移、大小——**不查文件系统，不查卷序列号是否变化**。

这组比对能证明"还是那个分区"，但证明不了"真的被擦了"。而 diskpart 脚本内单条命令失败时进程退出码仍可能为 0，项目已经有"mountvol 退出码会说谎"的教训（v1.8.0）。于是一次什么都没做的"格式化"可以完整通过。

更直接的问题是：上一轮我新建的 `operation_safety::verify_formatted_volume`（要求 NTFS + 卷序列号必须变化，`operation_safety.rs:29-45`）**只接到了 GUI 的 PE 安装路径**（`native_gui.rs:3826`），核心还原路径没用上。强校验写好了却没接到最需要它的地方，这是我的疏漏。

顺带一个佐证：`mount_env_volume` 在续跑时显式传 `reformatted_ok=true` 容忍序列号变化（`main.rs:868-874`），说明"格式化后序列号会变"是项目已知事实——那它就能当正向证据用。

## 五、P1：BCD 回滚的目标存储是隐式的

`main.rs:1846` 与 `:1859`。两处都已经算出并（在复制分支里）校验过 `efi_store`，但 `/import` 不带 `/store`：

```rust
Err(error) if error.raw_os_error() == Some(32) => {   // ESP BCD 被占用
    run_logged("bcdedit.exe", &["/import", &snapshot_arg], log)?;
    append_log(log, "Raw EFI BCD restore was locked; imported the saved BCD snapshot")?;
}
```

不带 `/store` 时 bcdedit 作用于默认存储。WinRE 里默认存储不保证就是刚校验过的那个 ESP BCD（这段代码自己要把 ESP 挂到 Z: 才能访问它）。锁定回退分支尤其可疑：ESP BCD 正被占用说明它打不开，若 import 落到另一个未被占用的存储，bcdedit 返回 0，日志照样写"imported the saved BCD snapshot"——**回滚伪报成功，而这是引导修复已经失败后的最后一道兜底**。

对照 `windows_prepare.rs:1365,1393` 同样写法：那里跑在正常 Windows，默认存储就是系统 BCD，语义正确。所以这不是"到处都要改"，而是 WinRE 侧这两处要显式带上已经算好的 `/store`。

## 六、P1：状态双写撕裂，续跑静默放弃

`lib.rs:1081-1090`：

```rust
let status = task.transition(next)?;
write_json_atomic(self.task_path(...)?, task)?;      // 第一次写
write_json_atomic(self.status_path(...)?, &status)?;  // 第二次写
```

两次各自原子，合起来不是事务。断电落在中间 → `task.status = next` 而 `status.stage = 上一阶段`。

- `resume_pending_boot_task`（`main.rs:375-380`）要求 `status.stage == task.status`，不等则 `if` 不成立 → **既不入 pending 也不写任何日志**，`pending.is_empty()` → `Ok(false)` → GUI 正常启动，用户什么也看不到。此时自建 BCD 对象和镜像卷上的载荷 WIM（约 700MB）无人清理。
- `recover_env`（`main.rs:809-815`）只放行 `Prepared → BootRequested` 这一种偏差，其余直接 `Err("status.json stage does not match task.json")`，任务卡死需人工介入。

最小改法：撕裂窗口消不掉，但"发现撕裂"必须留痕——续跑扫描里给这个分支补一条日志；进一步可让 stage 只存单一权威文件，status.json 退化为派生视图。

## 七、P1：准备失败不清理已建的 BCD 对象与载荷

`windows_prepare.rs:744-750`：

```rust
let result = prepare_payload(...);
if let Err(error) = result {
    if task_dir.exists() {
        let _ = store.write_failure(&mut task, 1, error.to_string());
        let _ = append_log(&prepare_log, &format!("Preparation failed: {error}"));
    }
    return Err(error);
}
```

`create_entry` 按设计跑在 DISM 注入**之前**（`windows_prepare.rs:946` 注释说明顺序要点）。因此注入、`refresh_payload_hash`、`arm_one_shot` 任一失败时，loader + devopts 两个 BCD 对象和镜像卷上的 `BackupRestoreRE\Winre.wim` 都已存在，而这里只写 failure，不 `disarm`。多次失败会累积同名启动项和多份大文件。

因为没走到武装一步，机器不会误启动进去——所以这是残留问题而非启动风险，但正是项目反复强调的"僵尸启动项"。另外两处 `let _ =` 把 `write_failure` 和日志的错误也吞了：`write_failure` 失败时任务停在中间阶段，可能被续跑扫描捡起来。

## 八、P1：恢复与准备路径的外部命令没有超时，也没有进程树约束（我上轮留下的不一致）

`main.rs:2596-2630` 的 `run_logged` 用 `child.wait()`，无超时；`:2633-2650` 的 `capture_logged` 用 `.output()`，同样无超时。两者都不进作业对象。

DISM 挂载/提交、bcdedit、diskpart、shutdown、reagentc 任一挂住，准备或恢复就永久卡住。WinRE 里没有用户能介入，进度窗口停在某个阶段（它只读日志增量），只能硬复位。

上一轮我新建的 `windows_command`（作业对象 + 超时 + 进程树终止 + 真实退出码）只接了 GUI、在线和 PE 三条路，**最关键的 RE 恢复路径没接**。这里要的不是"固定分钟数就杀"，而是：进程树可终止 + 心跳/无输出超时 + 失败时能走 `rollback_boot_request`。

## 九、P1：`recover` 独立命令缺少 `recover_env` 的全部闸门

`main.rs:612-685`。与 `recover_env` 逐项对比，`recover` 不做：载荷清单校验（`validate_payload_files`）、运行中二进制自校验（`verify_running_recovery_binary`）、身份 env 核对（`verify_task_identity_env`）、payload task 与 workspace task 比对、`status.json` 与 `task.json` 一致性检查、终态 `finalize_boot_entry_after_task`。

却传 `finalize_success = true`，于是可以在**留着启动残留**的情况下把任务写成 `Success`。它是运维/开发命令（`usage()` 里有），但判成功的权力和 `recover_env` 一样大，闸门不该差这么多。

## 十、P1：`capture_logged` 绕过项目自有的控制台解码层

`main.rs:2641-2642` 直接 `String::from_utf8_lossy`，不走 `text_parsing::decode_windows_bytes` / `plan_console_bytes`——而项目正是为了中文 WinRE 的 GBK/UTF-16 输出才写了那一层（`recovery_progress.rs:261` 在用）。

两个后果：

1. 中文 WinRE 下 DISM/bcdedit 的中文输出在恢复日志里变乱码，可诊断性下降。
2. `verify_bcd_target`（`main.rs:2950`）通过它读 `bcdedit /enum`，其中匹配中文标签 `"设备"` / `"os 设备"`（`:2959-2960`）的分支在乱码下**不可能命中**。英文标签在已验证的镜像上可用，所以 v1.9.4 实机通过了——但本地化分支实际是死代码，这类"测试全绿掩盖半边失效"值得单独记一笔。

## 十一、P2 与优化

| # | 问题 | 位置 | 说明 |
|---|---|---|---|
| 11 | 在线备份缺空间预检 | `online_operation.rs:280` | 离线有，且已计入追加候选副本（`windows_prepare.rs:1411-1428`）；在线直接整份 `fs::copy` 候选，满盘时白跑很久才失败。这是我上轮引入候选事务时留下的缺口 |
| 12 | CLI 保留策略在正式镜像上就地删索引 | `main.rs:2224-2245` | 追加用了 `.append-candidate.wim`，保留策略却直接对 destination 跑 `/Delete-Image`；中途失败留下"索引已少、副档未重编号"的镜像。在线路径是候选上做完才发布，同一函数内两种安全等级 |
| 13 | 追加一次全量读写三遍 | `main.rs:2049-2051`、`:2162` | `previous_hash`（整份读）+ 候选复制（整份写）+ 最终哈希（整份读）+ `/CheckIntegrity`。60GB 镜像在机械盘上代价很大。`previous_hash` 只用于判断旧版目录级 `metadata.json` 归属，可先用大小/修改时间筛，必要时才算 |
| 14 | GUI 1.5 秒后不再跟踪 prepare | `native_gui.rs:4878-4912` | 文案没伪报成功（明确写了"this is not recovery success"），但之后失败只能靠用户手动刷新。另外 `:4881` 的 `GetExitCodeProcess` 返回值被丢弃，失败时 `exit_code` 保持 0 → 早退失败被当正常，与上轮修掉的 P0-1 同型 |
| 15 | `verify_bcd_target` 按盘符匹配 | `main.rs:2942-2962` | 与"盘符不是身份"的纪律相悖。目标无盘符时 bcdedit 渲染成 `\Device\HarddiskVolumeN`，匹配假阴性。当前靠"先挂到 W:"规避 |
| 16 | PE 备份静默删除已有 WIM | `native_gui.rs:7534` | `let _ = std::fs::remove_file(wim);` 错误被吞。用户把 PE 任务指向已有多索引备份时，历史索引全部消失，且不动副档 → 留下指向已不存在内容的副档 |
| 17 | 空间预检里 `unwrap()` | `windows_prepare.rs:1413,1425` | `source.drive_letter.unwrap()`，盘符缺失即 panic |

## 十二、不构成缺陷的几项（已核对，避免误修）

- **`ramdisk=[F:]\...` 用盘符**（`text_parsing.rs:896`、`boot_entry.rs:345`）：bcdedit 在 `/set` 时就把盘符解析成分区设备对象存入 BCD，存的不是字母，盘符重排不影响启动。回读比对也在同一上下文完成。**不是缺陷。**
- **`verify_hash=false` 时不比对副档哈希**（`main.rs:1970`）：`prepare` 在副档存在时直接把副档哈希抄进 `task.image.sha256`（`windows_prepare.rs:679-681`），所以这一行的比对本身是恒等式，**不是漏检**。
  但要注意同一机制的另一侧确实有缺口，已由并行复核记录：用户勾选严格哈希又选择"强制继续"时，`force_restore_hash` 只在准备阶段放行，任务里只存了 `verify_hash=true` 而没存这次例外，离线阶段用抄来的旧副档哈希再校验一次仍会拒绝（见 [可能的问题GPT6.md](可能的问题GPT6.md) 第 13 项）。本文不重复该项。
- **`windows_prepare.rs:1365,1393` 的 `/import` 不带 `/store`**：跑在正常 Windows，默认存储即系统 BCD，语义正确。只有 WinRE 侧（第 5 条）需要改。

## 十三、建议处理顺序

1. **第 1、2、3 条**：都是"失败被吸收成成功"或"保护在关键方向失效"，且改动面小、不涉及架构。
2. **第 4、8 条**：把上一轮已经写好并测过的 `verify_formatted_volume` 和 `windows_command` 接到核心还原/准备路径。属于补接线，不是新设计。
3. **第 5、6、7、9 条**：异常与回滚分支，优先补"失败必须留痕"，再谈事务性。
4. **第 10、15 条**：解码与身份判据统一。
5. **第 11~14、16、17 条**：按成本排。其中第 12、13 条互相牵制（候选副本更安全但更慢），**建议由你决定是否要为节省空间/时间换掉候选事务**，我不默默改回就地操作。

## 十四、与其他审计的关系（本文的新增点在哪）

- [19:29 Opus5 审计](20260930-1929-代码审计-伪报成功与静默失败-Opus5.md)：十项已在 v2.0.1 闭环，本文不重复。
- [20:50 华为 ds41 全量分析](20260930-2050-可能的问题-华为ds41.md)：确认其 H-CORE-1/2/3/4、H-PREP-1/4、M1 属实；**否证其 M13**（见第十二节）。
- [可能的问题GPT6.md](可能的问题GPT6.md)（2.0.5，14 项）：与本文有实质重叠，逐项对照如下。

**重叠但本文补了根因或扩大了影响面：**

| 本文 | 对方条目 | 本文补充了什么 |
|---|---|---|
| 第 1 条 | 第 14 项（证据 A） | 对方指出 `"NOSYS".contains("SYS")` 恒真，并提到"检测输出缺失需明确"。本文补上**为什么会缺失**：cmd 的 `>` 只绑定 `else` 分支，命中 `if exist` 时 `SYS` 根本没写进文件。这是两个独立缺陷叠加，且合成方向正好是"真系统卷不被拦" |
| 第 2 条 | 第 14 项（证据 B） | 对方框定为"`create-secondary` 未被排除"。本文补上更大的影响面：`TargetRole` 完全由操作类型决定（`lib.rs:346-349`、`windows_prepare.rs:698-702`），而 GUI 把非活动卷全部分流到在线（`native_gui.rs:4758-4772`），所以 WinRE 还原路径基本只服务系统卷——`restore-existing` 同样会被这条分支吞掉 |
| 第 6 条 | 第 7 项 | 一致。本文补上"续跑扫描静默跳过、连日志都不写"这一具体表现 |
| 第 8 条 | 第 5 项 | 一致（对方记为"部分修复"）。本文补充：问题不只在 PE 的十分钟限制，`run_logged`/`capture_logged` 在**准备与 WinRE 恢复主链**上完全无超时、不进作业对象 |
| 第 7 条 | 第 11 项 | 一致。本文补上 `create_entry` 跑在 DISM 注入之前这一顺序事实，说明残留为何必然包含 BCD 对象 + 大文件 |
| 第 4 条 | 第 1 项 | 对方强调"格式化前检查不足"，本文强调"格式化后回读不足"，两者互补：前检查防白格式化，后回读防假格式化 |

**本文独有、对方 14 项未覆盖：**

第 3 条（副档损坏当缺失 → 容量预检失效的完整链条）、第 5 条（WinRE 侧 BCD 回滚 `/import` 不带 `/store`）、第 9 条（`recover` 独立命令缺闸门却能判成功）、第 10 条（`capture_logged` 绕过解码层，连带 `verify_bcd_target` 中文分支是死代码）、第 12 条（CLI 保留策略就地删索引 vs 在线候选事务）、第 13 条（追加一次三遍全量读写）、第 15 条（`verify_bcd_target` 按盘符匹配）、第 16 条（PE 备份静默删除已有 WIM）、第 17 条（空间预检 `unwrap()`），以及第十二节的三项**否证**。

**对方独有、本文未覆盖**（不重复，按其文执行）：第 2 项（PE 任务存旧盘符/磁盘编号）、第 8 项（原恢复卷与目标同分区时旧序列号阻断续跑）、第 9 项（多任务共用载荷目录）、第 10 项（PE 辅助 ESP 定位与 `S:` 写死）、第 12 项（`bcdedit` 路径写死 `C:\Windows`）、第 13 项（强制继续未贯穿）。

## 十五、声明

- 本轮为只读复核：未改执行逻辑，未实机复现，未创建/删除虚拟机快照，未进行任何启动变更或格式化。
- 行号对应 `960a9ef`。仓库当前有多个会话并行推进，修改前请按当时基线复核行号。
- 第 1、2 条的严重度判定依赖"WinRE 还原路径基本只服务系统卷"这一分流事实（`native_gui.rs:4758-4772`）；若将来放开手工 `prepare` 对数据卷走 RE，第 2 条的修法需要同时保留合法数据卷还原的通路。
