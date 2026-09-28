# 还原目标承载 WinRE 的解决方案：迁出 → 还原 → 重建注册（保幂等续跑）

> 状态：设计定稿，待实现。对应真实 C: 还原（F3 最坏场景）的最后一道闸。
> 触发：用户要求「真实 C: 完整备份+还原」验收时，`prepare --operation restore-existing --target-drive C`
> 被 `VolumeRoleConflict::RestoreTargetOnRegisteredWinre` 拦截（见 docs/20260928-231000-*）。

## 0. 问题本质（先说清楚为什么「格式化后就断更」）

还原的**续跑/幂等**依赖 WinRE 这一启动源：

- 续跑靠 `reagentc /boottore` 重新武装一次性启动项（main.rs:333 `resume_pending_boot_task`），
  该启动项指向**注册位置**的 `Winre.wim`（`C:\Recovery\WindowsRE\Winre.wim`）。
- 应用镜像时 Recovery.exe 跑在 WinRE 的 RAM 盘（X:），磁盘上的注册 WIM 此刻不被读取——
  **所以正在跑的一次没问题**。
- 但如果在 `TargetErased`/`ImageApplied` 阶段掉电，下次启动由 Boot Manager 去加载
  `C:\Recovery\WindowsRE\Winre.wim` 进 WinRE 来续跑。若 C: 已被格式化，这个文件没了 →
  **进不了 WinRE → 无法续跑 → 卡死**。这就是用户说的「格式化后无法幂等、继续上一次断开位置」。

> 注：`RecoveryTask.env` 里的 `original/Winre.wim`（回滚原件）本来就存在**任务目录 H:** 上、
> 不在 C:，所以「回滚原件被删」其实是误报——真正断的是**续跑启动源**也落在 C: 上。

## 1. 解决思路：把「续跑启动源」迁出被格式化的目标分区

核心：让 WinRE 的**注册位置（启动源）**落在「不会被还原格式化」的卷上，
还原全程 payload 在那卷上跑，还原成功后再把**原版** WinRE 注册回 C:。

这正是已定型的「注册 WinRE 状态机」的时间分片思路（docs 状态机文档），只是把
「单一文件双角色」扩展到「双卷」：任务存活期 payload 在**难民卷**，终态原版回到 **C:**。

## 2. 实施步骤

### 2.1 准备层（prepare，正常 Windows 内，reagentc /disable+/enable 可用）

在 `validate_volume_roles` 判定 `RestoreTargetOnRegisteredWinre` 之前插入「WinRE 迁出」：

1. 选**难民卷 (refugee)**：一个持久、可写、且**不等于 target** 的卷。
   - 真实 C: 场景：workspace(`H:`) 与 image(`F:`) 都不会被 C: 格式化，二选一（优先 workspace，
     因为它本就承载任务目录与 payload 暂存）。记录其 GUID 进 `RecoveryTask.env` 的
     `RECOVERY_*`（让 resume/`ensure_registered_is_payload` 之后能按难民卷找，而非 C:）。
2. 把**载荷** WinRE（stage/Winre.wim，含 winpeshl→Recovery.exe）部署到
   `<refugee>:\Recovery\WindowsRE\Winre.wim`（即现有的 `ensure_registered_is_payload`，
   把 `recovery` 参数从 C: 的 identity 换成难民卷 identity 即可复用）。
3. `reagentc /disable`（清 C: 上的注册指针）→ `reagentc /setreimage /path <refugee>:\Recovery\WindowsRE`
   → `reagentc /enable`。此后 BCD `recoverysequence` 指向难民卷，`/boottore` 启动源在难民卷，
   **C: 格式化动不到它**。
4. 把「已迁出」标记写进任务状态（`WinreEvacuated`），并记日志。

### 2.2 执行层（recover_windows，WinRE 内）

- `format_target_partition`（main.rs:2605）照常格式化 C:——此时启动源在难民卷，安全。
- `TargetErased`/`ImageApplied`/`BootRepaired` 的持久化与重放逻辑**不变**（main.rs:2600-2644），
  因为它们已按「先写终态再动手」设计；难民卷启动源保证任一阶段掉电后都能回到 WinRE 续跑。
- **去掉**执行层 `winre_role_conflict_at_execution` 对 F1 的硬拒（main.rs:2575）——改为：
  若任务状态已是 `WinreEvacuated` 且难民卷可达，则放行；否则（旧任务/env 缺键/难民卷不可达）
  仍拒绝，避免绕过准备层时裸格式化 C:。

### 2.3 终态（finalize，首次健康 Windows 启动）

任务 `Success` 后、退出前：

1. 从任务目录 `original/Winre.wim` 拷回 `C:\Recovery\WindowsRE\Winre.wim`
   （校验哈希，防 Windows servicing 当天把它换成官方原版——见状态机文档「servicing 静默替换」一节）。
2. `reagentc /setreimage /path C:\Recovery\WindowsRE` + `reagentc /enable` → 注册回 C:，
   终态与还原前完全一致（系统恢复环境在 C:，原版 WinRE）。
3. 清掉难民卷上的 `<refugee>:\Recovery\WindowsRE`（任务目录里的 stage/original 保留作审计）。

## 3. 不变量与防护

- **A 难民卷必须是 payload 宿主且非 target**：由 2.1 步骤 1 保证；若找不到这样的卷，
  保留硬拒（当前行为），绝不静默格式化 C:。
- **B 续跑启动源永远在未被格式化的卷**：由 2.1 步骤 3 保证；这是幂等续跑能成立的前提。
- **C 终态必须是原版且注册在 C:**：由 2.3 保证，且带哈希校验。
- `reagentc /disable`+`/enable` 只在**正常 Windows** 跑（WinRE 内 `/disable` 不支持 rc=50，
  见状态机文档「WinRE 启动硬约束」）；迁出动作都在 prepare 阶段完成，符合该约束。

## 4. 与现有代码的对接点（落地时改这几处）

| 位置 | 现状 | 改法 |
|---|---|---|
| `windows_prepare.rs:509` `validate_volume_roles` | 命中 `RestoreTargetOnRegisteredWinre` 直接 `Err` | 命中且存在合法难民卷 → 先执行 2.1 迁出，再放行 |
| `main.rs:2575` `winre_role_conflict_at_execution` | F1 硬拒 | 任务已 `WinreEvacuated` 且难民卷可达 → 放行；否则拒 |
| `main.rs:2027` `ensure_registered_is_payload` | 按 `recovery` identity 部署 | 复用，仅把 identity 指到难民卷 |
| `main.rs:333` `resume_pending_boot_task` | 读 `RECOVERY_*` 找注册位 | 无需改（env 改成难民卷 GUID 即可） |
| finalize 路径 | 当前结尾 guard 还原原版到 C:（fcb48a1 回退后保留） | 在「还原原版到 C:」**之前**补一步 `reagentc /setreimage /path C:...`+`/enable` |

## 5. 验证方法（实现后）

- 在测试 VM 上把 WinRE 注册到某数据卷（模拟 C: 承载 RE 的最坏场景），
  跑 `restore-existing --target-drive C`：
  1. prepare 日志出现 `WinRE evacuated to <refugee>`，`reagentc /info` 显示注册在难民卷；
  2. 注入 `--test-fault power-loss-image-applied` 造中断 → 重启应自动进**难民卷** WinRE 续跑至 success；
  3. 终态 `reagentc /info` 显示注册回到 C:，`C:\Recovery\WindowsRE\Winre.wim` 哈希 == `original`。
- 负向：若难民卷不可达（如 target 是唯一可写卷），仍硬拒。
