# E3 主备反转评估：DISM vs wimlib 实机对照（结论：维持现状，不反转，2026-10-25 07:00，Step-5-Preview）

> 承接 [`20261025-0505-下一步开发方向`](./20261025-0505-下一步开发方向-Step-5-Preview.md) 的 E3。
> 用户已拍板「按推荐路线四步推进」。本文 = 对照实验全记录 + 对比表 + 反转建议。
> **结论：两条通道判据打平，建议维持「DISM 默认 + 失败自动降级 wimlib」现状，不反转。**

---

## 1. 实验设计（同一镜像、同一目标卷、双通道）

| 项 | 值 |
|---|---|
| 镜像 | `H:\BRImages\br-2629-20261023.wim`（index 1，40,566,232,539 B，展开约 79 GiB，capturedUsed 85 GiB，v2.6.31 捕获） |
| 目标卷 | F:（350GiB 可牺牲测试卷） |
| Round A | `online --operation restore-existing --target-drive F: --index 1 --format-target` → **DISM**（格式化→应用） |
| Round B | 同命令去掉 `--format-target` → **wimlib 覆盖式**（目标已是 Windows 卷，走 v2.6.27 覆盖语义） |
| ACL 判据 | 六条路径 SDDL 逐字节比对（镜像只读挂载侧 vs 还原卷侧）：卷根 / Users / Program Files / Program Files\WindowsApps / Windows\System32 / Windows\System32\config\SYSTEM。工具 `tools/br-acl-compare.ps1`（ASCII 脚本，SDDL 不含 SACL——两通道都不还原 SACL，是共同边界不进判据） |
| 其它判据 | `--json` success、哨兵、卷序列号、ERROR/WARN 行数、进度行形态 |

## 2. 原始数据

### Round A（DISM，格式化+应用）

- 起止 21:32:37 → 22:00:40，**总时长 1683s（28.1min）**；STEP 3/4 应用阶段 21:32:41→22:00:37 = **27m56s**。
- `--json`：`success:true`，`error_code:null`；任务日志 **0 条 ERROR/WARN**。
- 哨兵 `F:\BR-0545.txt`（还原前放置）：**消失**（格式化语义 ✓）。
- F: 序列号 `7E57-BFDF` → `9432-A8C3`（**变化**，格式化语义 ✓）。
- ACL：**6/6 与镜像逐字节一致**（含 WindowsApps 条件 ACE、SYSTEM hive 的 TrustedInstaller DACL）。
- 进度行：DISM 百分比条 `[====31.0%====]` 形态；2 行 `The operation completed successfully`。
- 收尾自动完成：引导快照 → 格式化校验 → 应用 → 身份回读 → bcdboot → 菜单命名/默认项。

### Round B（wimlib，覆盖式）

- 起止 22:13:42 → 22:40:22，**总时长 1601s（26.7min）**；STEP 3/4 应用阶段 22:13:43→22:40:20 = **26m37s**。
- `--json`：`success:true`；任务日志 **0 条 ERROR/WARN**。
- 哨兵 `F:\BR-0600.txt`（覆盖前放置）：**存活**（镜像中没有它，覆盖语义 ✓）。
- F: 序列号 `9432-A8C3` **不变**（未格式化 ✓）。
- ACL：**6/6 与镜像逐字节一致**。
- 进度行：wimlib 英文行 `Extracting file data: 35 GiB of 79 GiB (45%) done`（**带绝对量**，信息量优于纯百分比条），共 101 条。
- 收尾同样自动完成全链（盘符重绑 → bcdboot → 菜单命名/默认项回读）。

## 3. 对比表

| 维度 | DISM（Round A） | wimlib（Round B） | 判定 |
|---|---|---|---|
| 总时长 / apply 阶段 | 1683s / 27m56s | 1601s / 26m37s | wimlib 快 ~4%（噪声级，打平） |
| 终态 | success/100 | success/100 | 平 |
| ACL 6 路径 | 6/6 逐字节一致 | 6/6 逐字节一致 | 平 |
| 哨兵语义 | 消失（格式化） | 存活（覆盖） | 语义不同，各自正确 |
| 序列号 | 变化 | 不变 | 语义不同，各自正确 |
| 进度行可读性 | 百分比条 | 英文+绝对量 | wimlib 略优 |
| 失败模式 | 超时可配、失败自动降级 wimlib（v2.6.37/44 实证） | 失败即终局（无二级降级） | DISM 优 |
| 覆盖式到 Windows 卷 | **必然失败**（0x80070005，#110 根因） | 唯一通道 | wimlib 优（分工已定） |
| 实机履历 | A/B/C 入口全过、十余轮 | 5 次端到端（v2.6.25/27/37/44 + 本轮） | DISM 优 |

## 4. 结论与建议：**不反转**

1. **数据上打平**：时长差 4% 在磁盘噪声范围内；ACL 同为 6/6；终态同为 success/100。没有任何一项 wimlib 明显更优。
2. **风险不对称**：反转后 wimlib 成为唯一主力，它**没有二级降级**（wimlib 失败即终局）；而现状下 DISM 失败有 wimlib 兜底、wimlib 只在覆盖式场景出场（DISM 在该场景本就必败，不存在「把本来能成的搞砸」的风险。
3. **覆盖式的分工已经是对的**：DISM 必败的场景（覆盖到 Windows 卷）由 wimlib 独占，DISM 主力的场景（格式化还原）它全链路验证过十余轮。
4. **「无证据不反转」门槛未达到增益条件**：门槛要的是「wimlib 连续零缺陷 + 判据对比报告」——报告有了（打平），但打平不构成「换默认」的理由，只构成「维持」的理由。

**记录在案的反转触发条件**（任一成立再重议）：① DISM 出现一类**无法降级**的确定性失败（降级本身也失败的场景连续复现）；② wimlib 再积累 ≥10 次端到端零缺陷且出现 DISM 没有的能力需求（如显著速度/体积增益的实证）；③ 用户明确要求。

## 5. 现场恢复与产物

- 两轮都自动改了 default 指向 F:；实验后已 `bcdedit /default {af5ee73f-cf3b-11f1-9e5a-cb0d9617e33d}` 改回原系统并**重启验证**（BOOT_OK + default 回原 + C: 序列号 `5698-64F6`）。
- 证据：`.test-artifacts/e3-acl-20261025/`（image/dism/wimlib 三份 SDDL JSON、两轮完整任务日志、重启前截图）。
- F: 现存系统 = Round B 覆盖结果（测试卷，可牺牲）；T: 空卷；菜单项保留（含两轮命名结果），未额外清理。
- VM helper 副本 `H:\brwork\br-acl-compare.ps1` 已删（部署目录只留五件套）。
