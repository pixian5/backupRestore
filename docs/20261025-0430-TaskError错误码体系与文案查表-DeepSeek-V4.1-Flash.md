# TaskError 错误码体系：326 处运行期错误改查表

- 撰写：2026-10-25 04:30
- 版本：v2.6.46
- 提出：用户「给 TaskError 加错误码、文案按码查表等」

## 一、为什么做

v2.6.43 把**日志**文案全量多语言化了（97 → 0），但 `err()` 的 245 处中文
**一动不动**——当时的理由很实在：

> 这些是维护者诊断信息，且 26+ 处单测用中文断言错误文案，改文案要连带改一堆
> 断言，回归风险高于收益。

用户随后拍板：**那就给 `TaskError` 加错误码、文案按码查表**。于是有了这一轮。

目标拆开是三件事：

1. **码是契约**——用户报障说 `BR-VOL-002`，维护者直接定位到唯一一行，
   不用在 300 多行文案里靠关键词猜；文案改词也不影响定位。
2. **文案能换语言**——英文界面下错误信息不该还是中文。
3. **单测零改动**——历史 `contains("卷身份")` 断言必须继续命中。

## 二、设计

### 2.1 core：`TaskError` 加一个变体

```rust
#[error("[{code}] {rendered}")]
Coded { code: &'static str, rendered: String }
```

`Display` 渲染成 `[BR-VOL-002] 目标卷缺少稳定 GUID 路径，拒绝按盘符应用镜像`。

为什么是「构造时就把文案渲染成 String」：`TaskError` 在 core，core 不认识
语言层（`i18n` 在 cli）。构造点一定在 cli 侧，那里才有语言上下文。

`TaskError::Invalid(String)` **原样保留**——504 处引用、`#[from]` io/serde
转换都挂在它上面，动形状的收益远小于风险。

### 2.2 cli：一张宏生成的表

```rust
error_codes! {
    VolumeGuidMissing => ("BR-VOL-001", "目标卷缺少稳定 GUID 路径", "The target volume has no stable GUID path"),
    ...
}
```

宏一次展开出 `enum ErrCode` + `code()` / `zh()` / `en()` 三个 match + 单测用的
`ALL_CODES`。**每条纹目一行数据**，读起来就是一张表。

调用点：

```rust
crate::errc(ErrCode::VolumeGuidMissing)                 // 无占位
crate::errc_with(ErrCode::PartitionLetterMismatch, &[..]) // 带 {0}{1}
```

占位用 i18n 那套 `{0}` 位置式（复用 `msg_tpl` 的替换逻辑），
所以中英占位数量必须一致——单测直接卡住。

### 2.3 码的格式与稳定性

`BR-<域>-<三位序号>`，域：

| 域 | 含义 |
|---|---|
| `CFG` | 命令行参数 / 配置项（本轮**未启用**，见 §五） |
| `PRE` | 输入预检、路径组件 |
| `VOL` | 卷身份、盘符、几何 |
| `IMG` | 镜像文件、副档、索引、哈希 |
| `APL` | 应用镜像（DISM / wimlib） |
| `BOOT` | 引导、BCD、启动项、菜单 |
| `RE` | 恢复环境注册、WinRE 关联 |
| `PE` | PE 通道与写盘契约 |
| `TSK` | 任务状态机、续跑、任务目录 |
| `GUI` | 图形界面 |
| `CORE` | core 层的任务规格校验 |

**序号一旦发布不得重排**——重排等于改契约。新增在对应域末尾追加，不复用退役号。

## 三、覆盖范围

| 侧 | 数量 | 处理 |
|---|---|---|
| cli 含中文的 `err()` | **245** | 全部改 `errc`/`errc_with`，**中英双语** |
| core `TaskError::Invalid` | **81** | 全部改 `coded("BR-CORE-NNN")`，77 个语义码（同文案共用一码） |
| 合计 | **326** | |

core 侧的文案**不翻译**：那些是英文技术诊断（`unsupported task version {}`、
`EFI/MSR/Recovery partitions cannot be restore targets`），core 没有语言层，
硬塞一个只会得到两套并行的语言开关。用户可见的双语在 cli 表里。

## 四、关键约束：单测零改动

245 处中文文案**逐字保持原样**，只是 `Display` 多了 `[BR-XXX-NNN] ` 前缀。
历史断言全部是 `contains("中文片段")`，前缀不影响命中——**245+45 一次性全绿，
没改任何一条断言**。

如果当初图省事把中文改写了（哪怕只改标点），就要连带改 20 多条断言，
而断言一改，就再也分不清"是文案变了还是行为变了"。

## 五、边界：304 处英文 `err()` 故意不带码

剩余的 `err("...")` 分两类：

**带码（326 处）**：运行期失败。卷身份不符、镜像损坏、引导回读不一致、
WinRE 关联错误、任务状态机拒绝——用户真正会撞上，且文案要双语。

**不带码（304 处）**：命令行**用法**错误。`task id is required`、
`--target-drive is required`、`unsupported operation: xxx`。

不给它们发码的三条理由：

1. 本来就是英文技术提示，**不需要翻译**；
2. 文案本身唯一且稳定，用户贴文案就能定位；
3. 给 300 多条用法提示各发一个码，只会**淹没真正有价值的运行期码**。

所以 `crates/` 里仍然存在 `err(...)`。记住这条判读规则：

> **看到 `err` 就当"用法错误"，看到 `errc`/`errc_with` 才是带码的运行期失败。**

这条边界已写进 `error_code.rs` 的模块文档，避免后来者以为"漏了"。

## 六、顺带撞出的一个真 bug

验证错误码时随手跑了一条：

```
BackupRestore.exe recover C:\NoSuchWorkspace --json
→ 任务标识无效：--json（invalid task: task_id is not a UUID）
```

`--json` 被当成了 `recover` 的第一个位置参数。

根因是 v2.6.38 加 `--json` 时的错误假设，注释还写着：

> 子命令的解析循环遇到未知 `--json` 走 `_ => {}`，故无需从 arguments 摘除。

那只对 `online` 成立。`prepare` 是严格解析（上一轮已经因此撞过一次、
给它单独加了分支），而 `recover` / `abandon-task` 这些**按位置取参数**的
命令会把 `--json` 当成第一个业务参数。

修法：分发前统一 `arguments.retain(|a| a != "--json")`。一处修好，所有命令
受益；`prepare`/`online` 里那两个 `"--json" => {}` 分支留着当 no-op 也无害。

**教训**：全局开关的"识别"和"消费"是两件事。`main` 能识别 ≠ 命令能接受
（v2.6.41 已经因 `prepare` 拒绝 `--json` 吃过一次），也 ≠ 命令的参数表干净。

## 七、验收

| 项 | 结果 |
|---|---|
| 单测 | cli **245 passed**（+5 条错误码单测）+ core **45 passed**，**零断言改动** |
| 交叉编译 | 通过，零新增警告（两个跨平台未构造变体加 `#[allow(dead_code)]`） |
| 部署 | `2.6.46`，SHA256 `beb76bfa…` |
| 实机（中文） | `abandon-task` → 带码输出；`recover C:\x --json` → 不再把 `--json` 当参数 |
| 实机（英文） | `BACKUPRESTORE_LANG=en` → 英文文案 + 同一码 |
| 全链冒烟 | `--skip-backup` **6/6 PASS**（`smoke-20261025-044445`）——`--json` 摘除没破坏判据 |
| 表自检 | 码唯一 / 格式 `BR-域-三位` / 中英占位数量一致 / 英文无 CJK / 中英不相同 |

## 八、踩坑记录（过程，不是结论）

1. **括号匹配不能数括号**：`"unsupported task version {} (expected {})"` 里
   的 `(` `)` 会让深度计数错位，替换后语法炸掉。修法：扫描时**跳过字符串
   与字符字面量**。
2. **`.into()` 会提前闭合**：`Invalid("x".into())` 的 `.into(` 是真括号，
   深度在它的 `)` 处归零——正则会从 `Invalid(` 里截出 `"x".into(`。
   修法：body 以 `.into(` 结尾时**继续吃到下一个 `)`**。
3. **别用 `(.*?)` + `re.S` 修跨行结构**：non-greedy 仍会跨过整段代码，
   实测把 `Err(e.into())` 的 `.into()` 吃掉、变成 `Err(e)`。
   一次干净的正确解析，胜过三轮补丁——我第一次图快，连修三次才回到原点
   （最后 `git checkout` 重来）。
4. `TaskError::Invalid` 的 `Display` 自带 `invalid task: ` 前缀，
   所以历史实机输出是 `invalid task: task_id is not a UUID`；
   `Coded` 不带这个前缀（换成 `[码] `）。
