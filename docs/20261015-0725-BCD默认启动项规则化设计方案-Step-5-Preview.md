# BCD 默认启动项行为规则化设计方案（2026-10-15 07:25，Step-5-Preview）

> 用户拍板的设计规则（2026-10-15）：
> 1. **还原本系统**（目标是本机当前系统分区）：不管镜像原本备份自哪个分区（第一分区自认 C:、
>    还是第三分区启动后自认 C:），还原完成后 **BCD default 必须指向还原后的自己**。
> 2. **新增第二系统**（目标是别的分区）：还原后 **default 保持指向原有主系统**，新系统启动项
>    以 `/addlast` **追加到菜单末尾**，不抢默认。
> 3. **禁止**：「本次还原的 Windows」到哪个分区都一律设为默认启动。
>
> 本文档 = 源码现状取证 + 与规则的差距 + 改动方案 + 验收矩阵。
>
> **实施状态（2026-10-15 07:40，v2.5.8）**：S1（default 策略单一权威）、G4（A 场景显式日志）
> 已实施；S2 被用户提出的「读取 WIM 菜单名预填」方案取代（见 §3-S5，待确认后实施）；
> S3（原 default 丢失时报错拒绝的边界）**待用户裁定**；§6 验收矩阵尚未实机重跑。

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
- apply 末尾 default 决策（v2.5.7 时位于 `boot_menu.rs:861-867`；v2.5.8 已按 S1 策略化，
  见 §3-S1 实施记录）。旧代码：
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
| G1 | 【部分修复】apply 的 default fallback（`else { target_id }`）静默：原 default 不在 selected 时不区分场景、不打日志就落到 target。v2.5.8 已加 WARN 留痕，「报错拒绝」边界待 S3 裁定 | 规则 2/3 | 第二系统场景若主系统 loader 意外未进 selected（探测失败/reagentc 缺失被跳过），default 静默漂到第二系统——2026-10-13 事故（repair_secondary 失败后 default 留在半成品系统）的同款形态 |
| G2 | 【已被 S5 方案取代】「本次还原的 Windows」硬编码命名不区分场景，第二系统（用户未起名）也会获得 | 规则 3（精神） | 菜单语义混乱：名字暗示「默认」但实际不是 |
| G3 | 【已修复 v2.5.8】default 决策逻辑原分散在 bcdboot 参数 + assert + preserve + apply 四处，无单一权威表述。现收敛为 `DefaultPolicy` 单一权威 + apply 唯一决策点 | 规则 1/2（可维护性） | 后续改动容易破坏「哪条路径保证哪条规则」的对应关系 |
| G4 | 【已修复 v2.5.8】A 场景原无显式日志声明「default 指向还原目标」。现 bcdboot 后写路由日志 + apply 写策略决策日志 | —（可观测性） | 验收时只能从 BCD 终态反推 |

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

**实施记录（v2.5.8，已落码）**：

- `boot_menu.rs`：新增 `DefaultPolicy` 枚举（文档注释即规则原文）+ `BootMenuPlan.default_policy`
  字段（`#[serde(default)]`，缺省取保守侧 `PreserveCurrentDefault`，旧方案文件可读）；
  `BootMenuSession::set_default_policy` 是唯一写入入口，设定随方案落盘（续跑任务重启后有效）。
- `main.rs`：`target_is_secondary` 判据上移至引导方案准备处——default 策略与收尾路由共用
  同一判据，消除两处各写一份的漂移风险；prepare 后立即显式设定策略；bcdboot 后新增路由日志
  （「收尾路由=重建唯一系统/第二系统：…」）。
- `online_operation.rs`：在线通道结构上只服务第二系统（活动系统盘被 execute_inner 顶部拒绝），
  固定 `PreserveCurrentDefault`。
- apply：default 决策收敛到唯一 match，三条分支全部写日志。`PointAtRestoreTarget` 分支
  **无条件指向 target**——这同时修正一种旧行为：旧代码在「原 default 恰好还在菜单里」时
  保留原值，若用户还原前的默认项是另一套系统（如第二系统），还原后 default 仍指那套，
  与规则 1 相悖。

### S2 命名场景化（**已被 S5 取代，留档备选**）

用户 2026-10-15 07:36 提出更优方案（S5：直接读 WIM 里的菜单名预填，撞名自动加时间后缀），
本节的「版本化中性名」退化逻辑并入 S5 第 4 条作为文本框留空时的兜底。原设计留档：

- discover 增加入参（或 prepare 透传）`target_is_active_system`：
  - `true`（A）→ target 默认名保持「本次还原的 Windows」（唯一系统语义，用户已验收的习惯）；
  - `false`（C）→ target 默认名改为**版本化中性名**：`volume_build_tag(root)`（如
    `Windows 11 Pro 26100.9549`），读不到退 `Windows ({letter}:)`；用户起过菜单名时
    `suggested_target_name` 优先（现状不变）。
- 效果：菜单里再也不会出现「叫本次还原却不是默认项」的条目；「本次还原的 Windows」
  成为 A 入口专属语义。

### S3 防御性加固（承接 G1）——**边界待用户裁定，暂不实施**

> 2026-10-15 07:36 用户追问「原 default 不在菜单时直接落到还原目标是什么？」——
> 即 G1 描述的 apply 兜底分支（旧代码 `else { target_id }`，静默执行）。v2.5.8 已先加
> WARN 留痕（行为不变）；以下「loader 还在 BCD 只是没勾选 → 报错拒绝」的边界尚未实施，
> 待用户裁定后落地。

- apply 在 `PreserveCurrentDefault` 且 `previous.default` 不在 selected 时：
  1. 先在日志中列出 previous.default、selected、previous.display_order 全量，便于取证；
  2. 若 previous.default 的 loader 仍在 BCD（`output.contains(id)`）只是没被勾选 →
     **报错拒绝继续**（不让「用户少勾一个框」把默认项偷偷换掉；确要换请用户在弹窗里
     显式操作或先改 BCD）；
  3. 若 loader 已不在 BCD（主系统被删/ESP 异常）→ WARN + 落 target（唯一可行 default）。
- `PointAtRestoreTarget` 场景若 previous.default 恰好指向目标分区 loader（bcdboot 复用
  旧条目时），视为等价，不报错。

### S4 文档与注释同步（v2.5.8 已部分完成）

- `boot_entry.rs` 模块头补一条设计原则：「default 归属由目标性质决定，与镜像来源分区、
  盘符无关；盘符会重排，loader 身份是 disk/partition GUID」。
- `boot_menu.rs` 的 apply 注释更新为新策略表述。✅ 已随 S1 实施完成（`DefaultPolicy`
  文档注释即规则原文，apply 决策点注释说明分支语义）。

### S5 读取 WIM 菜单名预填（用户 2026-10-15 07:36 提出，**待确认后实施**）

用户原话：「『还原系统到非 C 盘』tab 增加读取 wim 里面菜单名可以吗？读出并显示到文本框，
如果和现有任一启动菜单重名，就在后面加上当前时间年月日时分，用户也可以修改」。

**可行性评估：可以，所需数据全部现成：**

- WIM 索引名（image name）在「读取镜像」时就已解析——GUI 索引下拉的数据源就是它；
- 现有启动菜单名在准备阶段可读（`boot_menu.rs` 的 `existing_boot_menu_names` +
  `loader_description_for_partition`，discover 已在用）；
- C 入口已有菜单名文本框（控件 ID 1207），且 `task.boot_menu_name` 已贯通到
  windows_prepare / online_operation / bcdboot `/description` 全链。

**方案要点：**

1. **预填时机**：用户点「读取镜像」并选定索引后，把该索引的 WIM image name 填进菜单名
   文本框；用户已手改过则保留手改值，不覆盖输入。
2. **撞名处理**：点「创建任务」时（文本框已是最终值）与现有 BCD loader 描述逐一比对；
   重名则在末尾追加 `-YYYYMMDDHHMM`（如 `2026-10-14 21:40-202610150736`），追加后再验
   一次；最终值回显文本框，用户可继续改。
3. **后端兜底**：windows_prepare 落盘 task 前用**同一函数**再做一次撞名消解
   （CLI 直跑不过 GUI 时也必须消解），保证单一权威。
4. **命名质量**：本项目自己的备份把 WIM 索引名写成时间戳（如 `2026-10-14 21:40`），
   作为菜单名可读但不好看——这正是「读出后允许用户修改」的价值；文本框留空时退
   S2 的版本化中性名（`Windows 11 Pro 26100.9549`）。
5. **影响面**：仅 C 入口（create-secondary，在线/离线）；A 入口无菜单名文本框，不受影响。
   §6 的 V4 验收标准随之改为「新项名字 = WIM 索引名（撞名时带时间后缀）或用户改名」。

## 4. 边界情形表（方案预期行为）

| 场景 | 目标性质 | default 结果 | 菜单形态 |
|---|---|---|---|
| A 入口还原到当前系统盘（格式化） | 活动系统 | 指向还原后的 loader（v2.5.8 起无条件） | 既有系统项 + 目标项（目标名「本次还原的 Windows」） |
| A 入口还原到当前系统盘（跳过格式化） | 活动系统 | 指向目标分区 loader（bcdboot 复用旧条目则等价） | 同上 |
| A 入口，还原前默认项是另一套系统 | 活动系统 | 指向还原后的 loader（修正旧行为：旧代码会保留原默认） | 原默认系统仍在菜单（若勾选） |
| C 入口新增第二系统（用户起名） | 第二系统 | 保持原主系统 | 原菜单 + 新项 addlast（用户菜单名） |
| C 入口还原到别的 Windows 卷（未起名） | 第二系统 | 保持原主系统 | 原菜单 + 新项 addlast（S5 实施后 = WIM 索引名/撞名带时间后缀；S5 前 = 「本次还原的 Windows」） |
| 第二系统场景，用户在弹窗取消勾选主系统 | 第二系统 | validate 勾选守卫明确报错（现状已拦） | —（提示先恢复勾选） |
| 第二系统场景，原 default 的 loader 不在菜单 | 第二系统 | WARN 后落 target（v2.5.8）；S3 裁定后或升级为报错拒绝 | 视菜单实际内容 |
| 第二系统场景，主系统 loader 已不在 BCD | 第二系统 | WARN 后落 target（唯一可行 default） | 仅剩目标项 |
| B 入口数据卷还原 | 非 Windows 卷 | 不碰 BCD（skipping BCDBoot） | 不变 |

## 5. 改动清单（S1/G4 已于 v2.5.8 实施）

| 文件 | 改动 | 状态 |
|---|---|---|
| `boot_menu.rs` | ① `BootMenuPlan` 加 `default_policy` 字段 + serde 默认值（旧方案文件缺省按 `PreserveCurrentDefault`，保安全）② apply default 决策按策略分支 + 全分支日志（S1）③ `set_default_policy` 唯一写入入口 | ✅ 已完成 |
| `main.rs` | prepare 调用处按 `target_is_secondary`（判据上移共用）显式设定策略；bcdboot 后路由日志；删除重复判据声明 | ✅ 已完成 |
| `online_operation.rs` | 在线通道固定 `PreserveCurrentDefault`（结构上只服务第二系统） | ✅ 已完成 |
| `boot_menu.rs` 测试 | 方案构造补 `default_policy`；两种策略各补单测（含 fallback 路径） | ✅ 构造已补；策略单测待补 |
| `boot_entry.rs` | 模块头补设计原则注释（S4） | ⏳ 待办 |
| `boot_menu.rs`/`native_gui.rs` | S5：读取镜像后预填 WIM 索引名到菜单名文本框 + 撞名加时间后缀 + 后端同一函数兜底 | ⏳ 待用户确认后实施 |
| 文档 | 本文档随实现结果更新；`docs/README.md` 索引条目更新 | ✅ 已更新 |

## 6. 验收矩阵（实机）

> **尚未重跑**。阶段 1-5 的实机通过是在 v2.5.7 上取得的；v2.5.8 改的是 default 决策的
> 表述与日志（A 场景终态不变，另修正「还原前默认项是另一套系统」的边角），
> S5 实施后需补 V4。每例开跑前按快照纪律拍摄（A 入口动 C: 值得拍），验证通过即删。

| # | 用例 | 通过标准 |
|---|---|---|
| V1 | A 入口全链（格式化） | default = 目标分区 loader；菜单含「本次还原的 Windows」；序列号变+哨兵消失 |
| V2 | A 入口（跳过格式化，离线） | default = 目标分区 loader；哨兵法仍适用（覆盖式应用时哨兵必然消失） |
| V3 | C 入口在线新增第二系统（起名） | default = 原主系统；新项 addlast 且名字=用户菜单名 |
| V4 | C 入口还原到别的 Windows 卷（未起名） | default = 原主系统；新项 addlast 且名字=WIM 索引名（撞名带时间后缀）或用户改名（**不许**出现「本次还原的 Windows」——S5 实施后） |
| V5 | 弹窗取消勾选主系统（第二系统场景） | 明确报错，不静默改 default |
| V6 | 每例均过三件套（截图/EventID 1000/WerFault）+ BCD 三行基线记录 |

## 7. 与本轮其他事项的关系

- 快照纪律已按用户纠正更新（MEMORY.md）：快照=CoW，**拍摄几乎不占空间**；一次修改只付
  一次空间（旧块保留进对应父层），**不随层数翻倍**；层数多的代价是各快照增量各自累积、
  删前不释放。V1-V6 每例开跑前按需拍摄，**验证通过即删**。
- F: 第二系统已删除（2026-10-15 07:10 文档），V3/V4 需重新选目标卷（候选：E: 或重新
  建的卷；不再占用 F:）。
