# online 主路径错误码被吞修复：`--json` 的 `error_code` 不再恒为 null

> 时间：2026-10-26 05:20 · 版本 **v2.6.51**（v2.6.50 → 2.6.51）
> 发现途径：P3-b wimlib 捕获 POC 的实机测试（撞到 DISM 40 分钟超时，回读 `--json` 发现无码）
> 部署：`H:\brwork` SHA `8d430d59…`，GUI 标题确认 `BackupRestore - Rust GUI v2.6.51`
> 关联：[P3-b 捕获侧对照 POC §4.1](20261026-0450-P3b-wimlib捕获POC-DISM与wimlib捕获侧实机对照-Step-5-Preview.md)

---

## 0. 一句话

**`online` 命令（备份/还原主路径）失败时，`--json` 的 `error_code` 恒为 `null`——
内部真实的带码错误被 `main` 一句无码文案整体替换掉了。**
错误码体系（337 处编码、CLI 双语表、`--json` 字段）**在最需要它的那条路径上从未生效过**。
已修复并实机验证：超时场景现在返回 `BR-APL-001`。

---

## 1. 现场（缺陷如何被撞出来）

P3-b POC 里用 40 分钟逃生阀跑 F: 卷的 DISM 在线捕获，超时终止后回读：

```json
{"command":"online",
 "error":"invalid task: online operation did not complete successfully",
 "error_code":null,
 "operation":"backup","success":false}
```

DISM 自己明明是**带码**失败（`dism()` 走 `errc_with(ErrCode::DismApplyFailedOnline, …)`），
但到 `--json` 这里码没了。

---

## 2. 根因

`crates/backuprestore-cli/src/main.rs`（修复前 547 行）：

```rust
let result = crate::online_operation::execute(&params);
...
if result.success {
    Ok(())
} else {
    Err(err("online operation did not complete successfully"))   // ← 吞码
}
```

`execute()` 返回的是 `OnlineResult { success, detail, log_path }`——
**只带了一个给人看的 `detail` 字符串，把原始 `TaskError` 丢掉了**。
于是无论内部是什么错，`main` 都用 `err()`（`TaskError::Invalid`，故意无码）重新包一遍。

按项目自己的错误码纪律（MEMORY.md 已固化）：

> `err()` = 命令行用法错误（**故意无码**）；`errc` / `errc_with` = **带码运行期失败**。

DISM 超时/失败是运行期失败，落进 `err()` 属于**纪律被违反**，而且是静默的——
没有任何编译告警、日志里也看不出来（日志写的是 `detail`，是对的）。

**影响面**：`online` 是 GUI 之外的全部命令行备份/还原入口，
也是 `tools/smoke/br-smoke.sh` 冒烟判据消费的主要命令。
码失效 = 自动化只能解析文案 = 与「判据必须用 `--json` 字段、不要依赖任何语言的文案」
这条 10-24 立下的纪律**直接冲突**。

---

## 3. 修法（**文案一字未改，只多带一个码**）

| 文件 | 改动 |
|---|---|
| `online_operation.rs` | `OnlineResult` 新增 `pub error_code: Option<&'static str>`；三处构造点（成功/日志目录失败/panic 兜底）分别填 `error.code()` / `TaskError::from(io_err).code()` / `None` |
| `main.rs` | 失败出口改为：`Some(code) => TaskError::coded(code, message)`，`None => err(message)`；`message` 与原文案**逐字相同** |
| `native_gui.rs` | panic 兜底的 `OnlineResult` 构造补 `error_code: None`（panic 不是业务失败，归因看 `panic.log`） |

两个实现细节（都是为了避免踩坑）：

- **码必须在 `result` 被 `match` 消费之前取**：
  `let mut error_code = result.as_ref().err().and_then(|e| e.code());`
  否则 `match result` 把 `TaskError` 移走了就拿不到。
- **日志目录创建失败那条路径的错误是 `io::Error` 不是 `TaskError`**，
  `io::Error` 没有 `code()`（只有 `raw_os_error()`）→ 交叉编译直接 E0599。
  先 `TaskError::from(error)` 再 `code()`，得到 `BR-IO-001`，与 `TaskError::code()` 对 `Io` 的口径一致。
- **文案不改**：`online operation did not complete successfully` 原样保留。
  （历史判据/断言可能仍在匹配这句英文；MEMORY.md 有「改文案前先想断言」的教训。）
  只在 `Coded` 变体下由 `Display` 自动加上 `[码] ` 前缀。

---

## 4. 顺带清掉的 3 条编译告警（v2.6.50 遗留）

| 告警 | 位置 | 处置 |
|---|---|---|
| `field index is never read` | `image_admin.rs` `IndexEntry` | 删除冗余字段（`entries` 是 `BTreeMap<u32, _>`，索引号本就是键） |
| `field modified is never read` | `image_admin.rs` `ImageSummary` | 删除（对话框展示用的是 sidecar 里的创建时间，不是文件 mtime）；连带删掉不再使用的 `SystemTime` import |
| `constant units should have an upper case name` | `main.rs` `format_size` | 改名 `UNITS` |

结果：**交叉编译告警从 4 条降到 1 条**（剩下的 1 条是 MSVC `libcmt.lib` 的 LNK4099 PDB 提示，属工具链固有、无法消除）。

---

## 5. 验证

| 项 | 结果 |
|---|---|
| macOS 单测 | **249 + 45 全绿** |
| 交叉编译（cfg(windows) 改动的硬规矩） | 通过，产物 3,133,440 字节 |
| 部署 | `H:\brwork` 两 exe SHA 均 `8d430d59…` |
| **失败场景 1：镜像不存在** | `error_code` = **`BR-PE-039`**（修复前 `null`），`error` = `[BR-PE-039] online operation did not complete successfully` |
| **失败场景 2：DISM 超时**（`BACKUPRESTORE_DISM_TIMEOUT_SECS=1` 逼出，4.1 GB 载荷） | `error_code` = **`BR-APL-001`**（修复前 `null`）—— **正是最初发现缺陷的那个场景** |
| 成功场景（回归） | `success:true`、`error`/`error_code` 均 `null`，无回归 |
| GUI | v2.6.51 启动正常，界面含「备份文件管理…」按钮，无报错 |

为逼出超时，中途在 T: 上重建了 4.1 GB 载荷；验证完已格式化回空卷（序列号 `EA2F-C666`），
并清理了全部 POC 镜像/副档/失败候选目录，H: 回到 141.5 GB。

---

## 6. 教训

- **「错误码是契约」只在出口也遵守时才成立。**
  编码侧做得再全（337 处），只要有一处用无码错误重新包装，整条路径就等于没码。
  **新加错误码后，应顺着调用链追到实际出口验证一次**，而不是只验证产生点。
- **带码错误经过「结果结构体」传递时，必须显式把码也带出来。**
  `Result` 能带码，但一旦降级成 `{ success, detail }` 这种扁平结构，码就丢了。
  凡是「把 Result 拍平成状态码 + 文案」的地方，都要问一句：码去哪了？
- **发现 A 现象时先往上追一层根因。**
  本轮起点是「超时不带码」，但真正的病是「`online` 主路径整体吞码」——
  只在超时点补一个码，其它失败场景仍然无码。