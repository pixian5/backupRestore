# sidecar 占位名流入 BCD 描述缺陷修复（v2.6.16）+ BCD 写入/reset 竞态教训

> **【v2.6.16 · 2026-10-18 06:25 · 代码已修+已部署 · C 入口重做验证待跑】**
> 阶段 3（C 入口）实机验收时抓出：BCD 菜单描述竟是 `Windows Backup (index 1)` 占位名。
> 根因 = sidecar 元数据快速路径合成占位名，且备份侧从不记录 `image_name`。
> 同轮结清一个误导性极强的「BCD default 回写」谜案——实为 **bcdedit 落盘与 prlctl reset 的竞态幻影**。

## 1. 缺陷现象（阶段 3 实证）

C 入口（系统还原到非 C 卷）跑完后，`bcdedit /enum` 显示新装条目描述：

```
{ e9da2caf-... }  description = Windows Backup (index 1)
```

而同一镜像 `H:\BRImages\BR-sys-20261018.wim` 的 DISM 真实索引名是
`Windows 11 26200.9550 20261018`（备份时 `/Name` 写进去的）。程序自己产的
`boot-menu-plan.json` 里 `display_name` 同样是占位名——**用户看到的启动菜单名将是
`Windows Backup (index 1)`**，与备份页展示给用户的索引名不一致。

证据：`tools/win-clicker/_evidence/phase3-c-entry/boot-menu-plan.json`、`bcd-before`。

## 2. 根因链

1. `windows_prepare.rs` `wim_info` 有 sidecar 快速路径：存在 `<wim>.index-N.metadata.json`
   时**跳过 DISM 直接输出镜像列表**（性能优化，正确）。
2. 但 `BackupMetadata`（core）**原本不记录 `image_name`** → 快速路径只能合成占位名
   `format!("Windows Backup (index {index})")`（原 401/410 行）。
3. 占位名经 `prefill_menu_name_from_image` 流入两处用户可见面：**BCD 条目的 description**
   与 `boot-menu-plan.json` 的 `display_name`。
4. 备份侧（在线 `online_operation.rs` + WinRE `main.rs`）构造 `BackupMetadata` 时
   手边就有 DISM `/Name` 的有效名，只是没存。

## 3. 修复（三处联动）

**core `BackupMetadata` 新增字段**（`crates/backuprestore-core/src/lib.rs`）：

```rust
/// DISM `/Name` 真正写进 WIM 的索引名（备份时的有效名，空 image_name 时
/// 备份侧记为 "Windows Backup"）。旧版副档（2.6.15 及以前）没有此字段，
/// 读取侧据此回退 DISM 取真名——不得用占位名冒充。
#[serde(default, skip_serializing_if = "Option::is_none")]
pub image_name: Option<String>,
```

`#[serde(default)]` 保证旧副档（无此字段）反序列化不炸。

**备份两侧写入有效名**（与各自 `/Name:{...}` 用同一个变量，杜绝分叉）：

- `online_operation.rs`（在线备份）：`image_name: Some(name.to_string())`
- `main.rs`（WinRE 备份，recover_env 路径）：`image_name: Some(image_name)`

**读取侧三级回退链**（`windows_prepare.rs`）：

```
副档记录的 image_name（trim 后非空）
  → DISM /Get-WimInfo 真名（任一索引缺 image_name 时跑一次，只读 TOC 不解压；失败静默）
  → 占位名（仅极端兜底，且仍带索引号）
```

新增纯函数 `sidecar_image_name` + `clean_sidecar_name` + `sidecar_dism_names`，
单测 `sidecar_image_name_fallback_chain` 5 断言锁死全部分支。

## 4. 验证

| 项 | 结果 |
|---|---|
| core 单测 | 43 项全过（含新测试） |
| VM 内 cli 测试二进制实跑 | **240 tests / 234 passed / 0 failed / 6 ignored** |
| release 构建 | 2,801,152 字节 |
| `--deploy` 4 处副本 | SHA-256 一致 `c062bd8da89c7e1ec1cdff06935faa997a08eea0e02ec8e42cff024e4ac13fa9` |

**待跑**：阶段 3 C 入口用 v2.6.16 重做——预期 BCD description = `Windows 11 26200.9550 20261018`。
现存两副档均为旧格式（无 image_name），正好**覆盖 DISM 回退分支**。

## 5. 附带结案：「BCD default 回写」谜案真相 = 竞态幻影

现象：05:43:36 执行 `bcdedit /set {bootmgr} default {72447cbc}` + `prlctl reset`，
重启后却进了 F: 系统（default 显示指回 `{e9da2caf}`），像被「回写」。

**真相**：两条命令间隔约 1 秒，**硬重置抢在 bcdedit 落盘前**，bootmgr 读到旧 default。

证据链：

1. `LastBootUpTime = 5:43:37`（/set 后 1 秒即被重置，BCD 文件根本没写完）
2. BCD 文件 mtime 前后均 **05:43:40** 且此后再未被改（写入发生但在 reset 之后）
3. 现枚举 `default = {72447cbc}`——我的第二次 /set 是**空操作**（值早已落盘）

**教训（测试方法级，已纳入 SOP）**：

- **bcdedit 写入后必须等命令返回（落盘）再 reset**，任何「改 BCD + 立刻硬重启」序列
  都要在中间留确认步。
- bcdedit 输出按当前会话盘符**重解析** letter-form device（`{e9da2caf}` 原始写
  `partition=F:`，F: 会话显示 `partition=C:`），排查时别被显示值骗。
- 双系统 MountedDevices 字母漂移：还原出的 F: 系统启动后启动卷取得 C:（其
  MountedDevices 无新卷 GUID 映射），原 C: 卷被改配 E:——跨系统对比序列号时按卷不看字母。
