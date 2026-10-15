# 交接复核落地：default 决策单测、S4 注释与三处文档订正（2026-10-15 22:20，Step-5-Preview）

> 用户指令：「②连同 B1/B2 一起做并跑本地测试」——即按
> [交接复核文档](./20261015-2109-当前状态与待办交接复核-Hy4.md) §4 步骤复核后，
> 落地 B1/B2 并修掉复核发现的三处文档缺陷。**A 类 5 项待办未擅动**（语义类问题等用户拍板）。

## 0. 做了什么

版本 **v2.6.1 → v2.6.2**（`VERSION` + 两 crate `Cargo.toml`，`Cargo.lock` 由 cargo 同步）。

1. **B2：`DefaultPolicy` 决策单测**（0725 S1 留的尾）
2. **B1：`boot_entry.rs` 模块头设计原则注释**（0725 S4 留的尾）
3. **三处文档缺陷订正**：交接文档基线陈述、README 落后两个需求、裸「MEMORY.md」断链

代码行为**逐字不变**（一次等价重构 + 注释 + 测试 + 文档），未触及 BCD、未拍快照、无实机变更。

## 1. B2 实现：把 default 决策抽成纯函数

### 1.1 为什么

`boot_menu::apply` 是 `#[cfg(windows)]`，macOS 侧 `cargo test` 完全跳过——而 default 归属
决策恰是 2026-10-13「default 静默漂移到半成品第二系统，用户每次开机被拖进去」事故的决策点。
0725 S1 当年只做了「策略结构化」，单测一直欠着。

### 1.2 做法（`crates/backuprestore-cli/src/boot_menu.rs`）

新增两个 platform 无关的条目：

- `DefaultDecision { default_id, note, warning }`：决策产物 + 两行日志文案。
- `decide_default(policy, target_id, previous_default, selected) -> Result<DefaultDecision, TaskError>`：
  与原 apply 内联 `match` 逻辑**逐字等价**（含 WARN 文案、`note` 文案、错误文案
  「缺少必选目标加载器」）。刻意用 `TaskError::Invalid` 而非 `crate::err`——后者是
  `#[cfg(any(windows, test))]`，本函数在 macOS 非测试构建里也要能编译。

`apply` 改为：

```rust
let decision = decide_default(
    plan.default_policy,
    target_id.as_deref(),
    &previous.default,
    &selected,
)?;
if let Some(warning) = &decision.warning {
    crate::append_log(log, warning)?;
}
let default = decision.default_id;
crate::append_log(log, &format!("默认启动项决策：{default}；{}", decision.note))?;
```

日志顺序与原实现一致（WARN 行在决策行之前），后续 `/default` 写入与回读校验逻辑未动。

### 1.3 单测 8 例（`boot_menu.rs` 测试模块末尾）

| 用例 | 守住什么 |
|---|---|
| `policy_default_is_the_conservative_side` | 缺省必须取保守侧（旧方案不擅自改 default） |
| `policy_is_persisted_with_the_plan_and_old_plans_still_load` | 策略随 `boot-menu-plan.json` 落盘；删掉 `default_policy` 字段的旧方案仍可读、取保守缺省 |
| `pointing_at_target_overrides_a_still_listed_previous_default` | **翻转语义**：原默认项还在菜单里也要翻到新系统（0725 规则 2 推翻后的核心行为） |
| `pointing_at_target_ignores_which_partition_the_image_came_from` | 还原前默认项是另一套系统（B3 边角用例的纯逻辑版）也翻转到目标 |
| `pointing_at_target_without_a_target_loader_is_an_error` | 目标 loader 缺失必须报错，绝不静默写空 GUID |
| `preserve_policy_keeps_the_previous_default_while_it_is_still_listed` | 保守策略正常保留路径 |
| `preserve_policy_falls_back_to_target_with_a_warning_when_previous_is_gone` | 2026-10-13 事故形状：回落 + 必须留 WARN |
| `preserve_policy_without_target_and_without_previous_is_an_error` | 双缺也必须报错 |

## 2. B1 实现（`boot_entry.rs` 模块头）

按 0725 S4 原文补两条设计原则：
1. **default 归属只看目标性质，与镜像来源分区、盘符无关**（两个场景已统一指向新还原的系统；
   权威表述在 `boot_menu::DefaultPolicy`，本模块自建条目不参与该决策）；
2. **盘符会重排，loader 身份是 disk/partition GUID**（自建条日记 `VolumeIdentity` 而非字母）。

## 3. 三处文档缺陷订正

### 3.1 交接文档基线陈述落后（问题①）
2109 文档 §0/§5 称 HEAD `7220303`「两个需求 + 备份端到端验证一并提交」。实际 v2.6.1 分四提交：
`7220303`（实现）→ `f01731a`（实机验收 + build 判定）→ `92abeae`（备份端到端）→ `bef9617`（交接文档）。
已在 2109 文档开头加「⚠️ 基线订正 + 复核进展」块，并把 §0/§3 B 项/§5 逐条推进到 v2.6.2 真实状态
（历史记录不覆写，只加订正块）。

### 3.2 README 落后两个需求（问题②，属 B4 延伸）
- 【当前开发进度】还写 v2.5.8 阶段 5，两个 v2.6.1 新需求只字未提；
- 【下一步待实现】第①项竟是「阶段 5（A 入口全链）」——而 `docs/20261015-0105` 已证明其完成；
- 全文无 2.6.1 字样（正文还写着 2.5.3）。
已重写两条横幅指向 [当前开发进度](./20261015-2230-当前开发进度-Step-5-Preview.md) /
[下一步待实现](./20261015-2245-下一步待实现-Step-5-Preview.md)，正文版本号改 2.6.2。

### 3.3 裸「MEMORY.md」引用断链（问题③）
`.workbuddy/` 与 `memory/` 都被 `.gitignore` 排除（`.gitignore:112/179`），而 2109 文档 A5/B3
裸写「见 MEMORY.md『已知未修』」——接手 AI 克隆仓库后**既找不到也读不到**。
处置：新建库内权威 [已知未修与长期守则清单](./20261015-2150-已知未修与长期守则清单落库-Step-5-Preview.md)
（13 项待办逐项现状 + 六类长期守则镜像 + 维护约定），2109 文档三处裸引用已改指该文件。

## 4. 本地验证结果

```
$ cargo test --workspace
test result: ok. 151 passed; 0 failed; 0 ignored   # backuprestore-cli（原 143，+8）
test result: ok. 43 passed; 0 failed; 0 ignored    # backuprestore-core
$ cargo check --target aarch64-pc-windows-msvc -p backuprestore-cli
Finished dev profile                                 # 无 error 无 warning
```

⚠️ **macOS 绿灯不等于验收**：本轮触及 `apply`（`#[cfg(windows)]`），实机前必须在 VM 内
Windows 侧跑 `cargo test` + `build-windows.ps1` 构建，并核对二进制内含新代码。

## 5. 未做 / 下一步

- A 类 5 项待用户裁定（见 [下一步待实现](./20261015-2245-下一步待实现-Step-5-Preview.md) §1），
  本次一个都没动。
- B3（V1/V2/V5/V6 实机重跑）需 VM；B5/B6 属新功能不紧急。
- 下次实机前记得：VM 内跑测试 → 构建 → 哈希比对确认新代码 → 三件套验收。
