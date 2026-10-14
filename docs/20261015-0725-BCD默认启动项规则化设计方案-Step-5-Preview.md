# BCD 默认启动项行为规则化设计方案（2026-10-15 07:25，Step-5-Preview）

> 用户拍板的设计规则（2026-10-15）：
> 1. **还原本系统**（目标是本机当前系统分区）：不管镜像原本备份自哪个分区（第一分区自认 C:、
>    还是第三分区启动后自认 C:），还原完成后 **BCD default 必须指向还原后的自己**。
> 2. **新增第二系统**（目标是别的分区）：还原后 **default 保持指向原有主系统**，新系统启动项
>    以 `/addlast` **追加到菜单末尾**，不抢默认。
> 3. **禁止**：「本次还原的 Windows」到哪个分区都一律设为默认启动。
>
> 本文档 = 源码现状取证 + 与规则的差距 + 改动方案 + 验收矩阵。**方案待用户确认后实施**，
> 本轮不改代码。

## 1. 现状行为取证（源码，v2.5.7）

### 1.1 路由判据（已符合规则）

`windows_prepare.rs:826`：准备阶段（在线）落盘
`is_active_system: target.same_partition(&active_system_volume_identity()?)`；
RE 内 `%SystemDrive%` 是 RAM 盘 X: 探测不到，故离线侧只读该结论（字段注释明示）。
→ **按「目标分区 vs 活动系统分区」的同一性路由，与镜像来源分区无关**，正是规则 1 的前提。

### 1.2 还原本系统路径（A，is_active_system=true）

- `main.rs:2719-2768`：bcdboot **不带 `/d`**（注释：「还原到当前系统盘（重建唯一系统）不加
  /d：那时 default 本就该指向新 loader」）。bcdboot 默认行为即把新建 loader 设为
  `{bootmgr}.default`。
- `boot_menu.rs:861-867`（apply 末尾 default 决策）：
  ```rust
  let default = if selected.contains(&previous.default) {
      previous.default
  } else {
      target_id.ok_or_else(|| crate::err("缺少必选目标加载器"))?
  };
  ```
  此处 `previous` 是 **bcdboot 之后**回读的 BCD 状态，default 已是新 loader → 选中它 →
  default = 还原目标。✅ 规则 1 满足（阶段 5 实测：default = 「本次还原的 Windows」
  `{41326e72}`）。
- 回读校验（`boot_menu.rs:917-926`）：`actual.default != default` 即报错，default 写坏会被
  抓住。✅

### 1.3 第二系统路径（C，is_active_system=false）

四道防线叠加，default 均保持主系统：
1. bcdboot `/d /addlast`（`main.rs:2736-2737`）+ 注释说明 BFSVC 在 `/s` 场景可能忽略
   `/d`/`/addlast`，真正承重的是第 2 道；
2. `assert_boot_default_preserved`（bcdboot 后立即回读+自愈）；
3. `preserve_primary_boot_manager`（按任务快照恢复 default/displayorder）；
4. apply 的 `selected.contains(&previous.default)` 优先保留分支。
✅ 规则 2 满足（阶段 4 实测：主系统 default 未劫持，新项 addlast）。

### 1.4 与规则 3 相关的命名

- `boot_menu.rs:706`：discover 给 target 条目硬编码 `display_name: "本次还原的 Windows"`。
- `boot_menu.rs:493-505`：prepare 的 `suggested_target_name` 仅覆盖「existing_name 为 None
  的 target 条目」。调用方（`main.rs:2544-2551`）传 `target.boot_menu_name`——
  **CreateSecondary 才有菜单名**；`RestoreExisting` 到非活动 Windows 卷时该字段为 None。
- 后果：**「还原系统到非 C 盘」且用户未起菜单名时，新第二系统的菜单项也会叫
  「本次还原的 Windows」**。名字本身不改变 default（default 仍保留主系统），但语义混淆：
  用户看到「本次还原的 Windows」会预期它是默认启动项，而它不是。

## 2. 差距清单

| # | 差距 | 违反规则 | 风险 |
|---|---|---|---|
| G1 | apply 的 default fallback（`else { target_id }`）**静默**：原 default 不在 selected 时不区分场景、不打日志就落到 target | 规则 2/3 | 第二系统场景若主系统 loader 意外未进 selected（探测失败/reagentc 缺失被跳过），default 静默漂到第二系统——2026-10-13 事故（repair_secondary 失败后 default 留在半成品系统）的同款形态 |
| G2 | 「本次还原的 Windows」硬编码命名不区分场景，第二系统（用户未起名）也会获得 | 规则 3（精神） | 菜单语义混乱：名字暗示「默认」但实际不是 |
| G3 | default 决策逻辑分散在 bcdboot 参数 + assert + preserve + apply 四处，无单一权威表述 | 规则 1/2（可维护性） | 后续改动容易破坏「哪条路径保证哪条规则」的对应关系 |
| G4 | A 场景无显式日志声明「default 指向还原目标」 | —（可观测性） | 验收时只能从 BCD 终态反推 |

## 3. 设计方案

### S1 default 决策显式策略化（核心）

**新增** `BootMenuPlan.default_policy: DefaultPolicy`（序列化进 `boot-menu-plan.json`）：

```rust
enum DefaultPolicy {
    /// 目标是本机当前活动系统（A 入口/还原本系统）：
    /// default 必须 = 还原目标 loader（重建唯一系统语义）。
    PointAtRestoreTarget,
    /// 目标是第二系统（C 入口/完整镜像还原到别的 Windows 卷）：
    /// default 必须 = 原 default（主系统）；原 default 失效时按 §3.1 边界表处置。
    PreserveCurrentDefault,
}
```

- 由 `main.rs` 在 `BootMenuSession::prepare` 调用处设置：已知 `target_is_secondary`，
  `PointAtRestoreTarget` / `PreserveCurrentDefault` 二选一，**apply 不再自行推断**。
- apply 的 default 决策改为：
  ```rust
  let default = match plan.default_policy {
      PointAtRestoreTarget => target_id.ok_or(...)?,           // 唯一系统：指向自己
      PreserveCurrentDefault => {
          if selected.contains(&previous.default) { previous.default }
          else { /* §3.1 边界 B2：WARN 日志 + 拉回尝试 + 仍失败才落 target */ }
      }
  };
  ```
- `PointAtRestoreTarget` 分支写日志：「重建唯一系统：默认启动项指向还原目标 {id}」。
- `PreserveCurrentDefault` 的 fallback 落 target 时写 **WARN**：「原默认启动项不在菜单，
  默认已切换到第二系统 {id}（原因见上）」。

### S2 命名场景化

- discover 增加入参（或 prepare 透传）`target_is_active_system`：
  - `true`（A）→ target 默认名保持「本次还原的 Windows」（唯一系统语义，用户已验收的习惯）；
  - `false`（C）→ target 默认名改为**版本化中性名**：`volume_build_tag(root)`（如
    `Windows 11 Pro 26100.9549`），读不到退 `Windows ({letter}:)`；用户起过菜单名时
    `suggested_target_name` 优先（现状不变）。
- 效果：菜单里再也不会出现「叫本次还原却不是默认项」的条目；「本次还原的 Windows」
  成为 A 入口专属语义。

### S3 防御性加固（承接 G1）

- apply 在 `PreserveCurrentDefault` 且 `previous.default` 不在 selected 时：
  1. 先在日志中列出 previous.default、selected、previous.display_order 全量，便于取证；
  2. 若 previous.default 的 loader 仍在 BCD（`output.contains(id)`）只是没被勾选 →
     **报错拒绝继续**（不让「用户少勾一个框」把默认项偷偷换掉；确要换请用户在弹窗里
     显式操作或先改 BCD）；
  3. 若 loader 已不在 BCD（主系统被删/ESP 异常）→ WARN + 落 target（唯一可行 default）。
- `PointAtRestoreTarget` 场景若 previous.default 恰好指向目标分区 loader（bcdboot 复用
  旧条目时），视为等价，不报错。

### S4 文档与注释同步

- `boot_entry.rs` 模块头补一条设计原则：「default 归属由目标性质决定，与镜像来源分区、
  盘符无关；盘符会重排，loader 身份是 disk/partition GUID」。
- `boot_menu.rs` 的 apply 注释更新为新策略表述。

## 4. 边界情形表（方案预期行为）

| 场景 | 目标性质 | default 结果 | 菜单形态 |
|---|---|---|---|
| A 入口还原到当前系统盘（格式化） | 活动系统 | 指向还原后的 loader | 既有系统项 + 目标项（目标名「本次还原的 Windows」） |
| A 入口还原到当前系统盘（跳过格式化） | 活动系统 | 指向目标分区 loader（bcdboot 复用旧条目则等价） | 同上 |
| C 入口新增第二系统（用户起名） | 第二系统 | 保持原主系统 | 原菜单 + 新项 addlast（用户菜单名） |
| C 入口还原到别的 Windows 卷（未起名） | 第二系统 | 保持原主系统 | 原菜单 + 新项 addlast（版本化中性名） |
| 第二系统场景，用户在弹窗取消勾选主系统 | 第二系统 | **报错拒绝**（S3.2） | —（提示先恢复勾选） |
| 第二系统场景，主系统 loader 已不在 BCD | 第二系统 | WARN 后落 target | 仅剩目标项 |
| B 入口数据卷还原 | 非 Windows 卷 | 不碰 BCD（skipping BCDBoot） | 不变 |

## 5. 改动清单（待确认后实施）

| 文件 | 改动 |
|---|---|
| `boot_menu.rs` | ① `BootMenuPlan` 加 `default_policy` 字段 + serde 默认值（旧方案文件缺省按 `PreserveCurrentDefault`，保安全）② discover/prepare 增加目标性质入参，target 默认名场景化（S2）③ apply default 决策按策略分支 + 日志 + S3 加固 ④ 单测：两种策略各 3 例（含 fallback/拒绝路径） |
| `main.rs` | prepare 调用处传 `target_is_secondary`；日志补「默认启动项策略」 |
| `boot_entry.rs` | 模块头补设计原则注释（S4） |
| 文档 | 本文档随实现结果更新为「已实施」；`docs/README.md` 索引条目更新 |

## 6. 验收矩阵（实机）

| # | 用例 | 通过标准 |
|---|---|---|
| V1 | A 入口全链（格式化） | default = 目标分区 loader；菜单含「本次还原的 Windows」；序列号变+哨兵消失 |
| V2 | A 入口（跳过格式化，离线） | default = 目标分区 loader；哨兵法仍适用（覆盖式应用时哨兵必然消失） |
| V3 | C 入口在线新增第二系统（起名） | default = 原主系统；新项 addlast 且名字=用户菜单名 |
| V4 | C 入口还原到别的 Windows 卷（未起名） | default = 原主系统；新项 addlast 且名字=版本化中性名（**不许**出现「本次还原的 Windows」） |
| V5 | 弹窗取消勾选主系统（第二系统场景） | 明确报错，不静默改 default |
| V6 | 每例均过三件套（截图/EventID 1000/WerFault）+ BCD 三行基线记录 |

## 7. 与本轮其他事项的关系

- 快照纪律已按用户纠正更新（MEMORY.md）：快照=CoW，拍摄几乎不占空间；占用来自拍后
  修改且随层数乘法。V1-V6 每例开跑前按需拍摄（尤其 A 入口动 C:），**验证通过即删**。
- F: 第二系统已删除（2026-10-15 07:10 文档），V3/V4 需重新选目标卷（候选：E: 或重新
  建的卷；不再占用 F:）。
