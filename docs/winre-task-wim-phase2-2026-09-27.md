# 阶段 2 拆分实施：任务专用 WinRE 副本（2026-09-27）

文档状态：**设计 + 实机勘察，方案已获用户选定（阶段 2 拆分做）**。代码改动按 2A → 2B 顺序推进，
每个子阶段独立交付、独立回滚。

前置：v1.7.6 已完成阶段 1（F1/F3 第一段拒绝 + 载荷同步）。本文档只讲阶段 2。

---

## 1. 为什么必须做阶段 2（不是可选优化）

阶段 1 把「离线备份 C:」和「还原到 C:」直接**拒绝**了。判定条件是
`源/目标卷 == 承载注册 WinRE 的卷`。

问题在于：**没有独立 Recovery 分区的机器非常常见**（本测试 VM 就是，WinRE 注册在
`harddisk0\partition4` = C:）。这类机器上：

- 产品的**主场景** = 系统盘完整备份/还原；
- 主场景必须走**离线路径**（在线 Capture 系统盘会被文件占用锁住）；
- 离线路径 = `mutates_registered_winre = true` → **被阶段 1 拒绝**。

结论：**不做阶段 2，最终验收在相当一部分真机上会当场失败**。阶段 2 不是能力提升，
是解锁主场景的必经之路。

---

## 2. 实机勘察（VM，2026-09-27）

`bcdedit /enum all /v` 中 WinRE 相关对象：

| 对象 | device / osdevice | 说明 |
|---|---|---|
| `{fa68c813-b854-11f1-88b9-da86a19ef236}` | `ramdisk=[C:]\Recovery\WindowsRE\Winre.wim,{fa68c814-…}` | **当前生效的 WinRE osloader**，`{current}` 的 recoverysequence 指向它 |
| 另两个 osloader | `ramdisk=[Y:]\Recovery\WindowsRE\Winre.wim,…` | 孤儿引用（Y: 是 840M FAT32 hidden 卷，**目录为空**），历史残留 |
| `{fa68c814-…}`（ramdiskoptions） | `ramdisksdidevice partition=C:` / `ramdisksdipath \Recovery\WindowsRE\boot.sdi` | 与 osloader 配对，改路径时要保留 |

`reagentc /info`：Windows RE 位置 `\\?\GLOBALROOT\device\harddisk0\partition4\Recovery\WindowsRE`，状态 Enabled。

**关键结论**：只要把 `{fa68c813-…}` 的 `device`/`osdevice` 从 `Winre.wim` 改成
`Winre-task.wim`（同目录下的任务副本），就能从任务副本启动，
**注册 WIM 全程不被覆盖** → F3 污染链从根上断掉。ramdiskoptions 对象不用动。

---

## 3. 阶段 2A：任务副本 + BCD 重定向 + Capture 排除（解锁 F3）

### 3.1 改动点

| # | 位置 | 改动 |
|---|---|---|
| 1 | `windows_prepare.rs::prepare_payload` | 注入后的 WIM 不再 `fs::copy` 到注册位置（删掉 790 行那次覆盖），改为写 `…\Recovery\WindowsRE\Winre-task.wim` |
| 2 | 新增 `redirect_winre_bcd()` | `reagentc /info` 取当前 recoverysequence GUID → `bcdedit /set {guid} device ramdisk=[R:]\Recovery\WindowsRE\Winre-task.wim,{opts}` 与 `osdevice` 同理 |
| 3 | 新增 `restore_winre_bcd()` | 任务结束/失败时改回注册 WIM 路径，并删除 `Winre-task.wim` |
| 4 | 恢复端（`main.rs::recover_windows` 收尾 / `restore_original_winre`） | 同上：改回 BCD → 删副本 → 校验注册 WIM 哈希 == `ORIGINAL_WINRE_SHA256` |
| 5 | `core::build_capture_exclusions` | 新增可选参数，追加 `\Recovery\WindowsRE\Winre-task.wim` 与任务目录；确保镜像里只有**原始** WIM |
| 6 | `core::validate_volume_roles` | 新策略 `task-wim-redirect` 下**放开** F3 对 `source == recovery` 的拒绝；旧任务（无策略标记）保持拒绝 |

### 3.2 为什么这样能解锁 F3

污染的唯一来源是「把注入后 WIM 覆盖回注册位置」。改成任务副本后：

- 注册 WIM 始终是原始的 → Capture 打进镜像的是原始 WIM，不含任务入口/env/可续跑记录；
- 任务副本本身进 Capture 排除表 → 镜像里连副本都没有；
- 任务结束 BCD 改回 + 副本删除 → 系统回到原状，可用哈希证伪。

### 3.3 验收标准

1. 备份全程（含 WinRE 阶段）注册 WIM 的 SHA-256 与准备前**完全一致**；
2. 产出的 WIM **挂载抽检**：内部 `\Recovery\WindowsRE\` 下无 `Winre-task.wim`，
   `Windows\System32` 下无 `RecoveryTask.env` / `task.json`；
3. 任务结束后 `bcdedit /enum` 显示 ramdisk 路径回到 `Winre.wim`，且 `Winre-task.wim` 已删除；
4. **离线备份 C:（小样本或测试卷）不再被拒** —— 这是解锁的直接判据；
5. `cargo test -p backuprestore-core` 全绿，新增 2A 相关单测。

### 3.4 风险与兜底

- **改共享 osloader**：若任务中途断电，WinRE 会指向一个残留副本。兜底：恢复端每次启动都校验
  BCD 现状，发现残留就改回；另有任务目录里的 `bcd-before-export` 与原始 BCD 字节快照可 import。
- **bcdedit 拒绝改写**：PoC 阶段验证；若失败则退到 2B 的独立对象方案。
- **改启动项前必须建快照**（AGENTS.md 硬约束）。

---

## 4. 阶段 2B：独立一次性 BCD 对象 + 还原后重建（解锁 F1）

2A 解决备份污染；2A **不解决** F1 —— 还原目标 = WinRE 宿主卷时，格式化依然会删掉恢复资产。

2B 范围：

1. 每任务 **新建独立 osloader + 独立 ramdiskoptions 对象**（`bcdedit /create /application osloader`），
   不改共享 `{ramdiskoptions}`、既有 recoverysequence 与默认项；
2. 任务结束删除自建对象；
3. 还原到 WinRE 宿主卷时：先把原始 WIM 与 boot.sdi 搬出到安全位置（任务目录），
   格式化/Apply 之后放回，离线 `reagentc /setreimage` + `/enable` 重新注册；
4. 全部通过后才放开 F1 的 `target == recovery` 拒绝。

前置：2A 完成且实机稳定。

---

## 5. 回滚

| 层级 | 动作 |
|---|---|
| 代码 | 回退到 `73214c2`（v1.7.6）即回到阶段 1 状态 |
| 载荷 | 只需保证版本一致，2A 不改载荷内容 |
| VM | 改 BCD 前的 Parallels 快照（见下节记录） |
| 运行时 | 任务结束时自动改回 BCD + 删副本；失败时走已有的 BCD 字节快照 import |

---

## 6. 待办与决策记录

- [x] 用户选定「阶段 2 拆分做」（2A 先，2B 后）
- [x] 建快照（改 BCD 前）：`{eb423a36-f670-4d59-9a2c-c5a517db8c6b}`
- [x] **PoC 完成：2A 原方案被证伪**（见第 7 节）
- [ ] **路线待用户裁定**（见第 8 节）
- [ ] 选定路线后实施 + 测试盘验证

---

## 7. PoC 实测记录（2026-09-27，VM 内 4 次重启 + 对照实验）

环境：Win11 26100（ARM64 VM），WinRE 注册在 `C:\Recovery\WindowsRE`，生效 osloader
`{fa68c813-…}`（recoverysequence），配套 ramdiskoptions `{fa68c814-…}`。

| # | 方案 | 结果 |
|---|---|---|
| 对照 | 注册位置原样 + `reagentc /boottore` + `bcdedit /bootsequence` | ✅ **进 WinRE**（蓝屏恢复菜单，实测截图） |
| 1 | 共享 osloader 的 `device/osdevice` 改指 `Winre-task.wim` + bootsequence | ❌ 重启后直接回 Windows |
| 2 | 先在注册位置设 bootstatus，再改路径（bootstatus 独立于路径假设） | ❌ 同上 |
| 3 | `bcdedit /copy` 克隆 osloader（继承全部元素）→ 克隆指任务副本 + bootsequence | ❌ 同上 |
| 4 | `reagentc /setreimage /path <任务目录>` + `/enable`（官方通道） | ❌ `/enable` 无视任务目录，重建 recoverysequence 指回注册位置 |
| 5 | `recoverysequence` 本身指向克隆（自洽链）+ bootsequence | ❌ 同上 |

### 7.1 硬结论

1. **WinRE 强校验「启动 ramdisk 路径 == ReAgent 注册位置」**，与 BCD 对象身份无关
   （克隆自己的对象、甚至把它放进 recoverysequence 都没用）。`ReAgent.log` 佐证：
   `BCD recovery entry points to invalid location (no winre.wim at C:\Recovery\WindowsRE\Winre-task.wim)`
   —— 它按注册位置的**文件名**核对。
2. `reagentc /boottore` 在路径不一致时返回 RC=2（"Windows RE 已禁用"），此时 WinRE
   即使被 bootsequence 拉起也会因 bootstatus 缺失**静默自退**回 Windows。
3. 官方 `/setreimage` 是给离线部署用的；在线场景 `/enable` 会重新扫描注册位置并重建条目。
4. **BCD 层没有绕过方案**。任务环境要不碰注册 WIM，只有两条真路：
   **换 WinPE**（没有 ReAgent 校验），或 **swap-in-place**（见 8.1）。

### 7.2 意外重大发现：Windows servicing 静默冲掉部署的载荷

当天上午部署的 v1.7.6 载荷（`5df96301…`，714,409,997B）在数小时内被 Windows 累积更新
替换为**官方原版 WinRE**（`709,810,082B`，内部无 `Recovery.exe`，`winpeshl.ini` 回到
官方 `recenv.exe`；文件 mtime 保留 servicing 组件时间 2026-09-24）。

已重新注入恢复（官方底版 + v1.7.6 `Recovery.exe`/`winpeshl.ini`，新哈希 `1060a552…`，
写回前后校验一致；恢复前快照 `{efef3f19-f79a-489d-9b2e-cecc5dcac483}`）。

**产品含义**：注册 WinRE 不是可靠的载荷宿主——任何部署都可能被 servicing 冲掉。
任务准备阶段必须校验载荷哈希（已有 `ORIGINAL_WINRE_SHA256` 链）并在不符时重部署；
这也进一步支持「任务环境独立于注册 WinRE」的方向。

---

## 8. 修订后的路线候选（待用户裁定）

### 8.1 路线 A：swap-in-place（仍用 WinRE，任务副本临时顶班）

任务准备时：官方 WIM 暂存任务目录 → 任务副本（注入后）**命名为
`C:\Recovery\WindowsRE\Winre.wim`**（路径/文件名/注册位置三者一致，ReAgent 满意）→
`reagentc /boottore` → WinRE 启动任务副本。任务结束：换回官方 WIM + 校验哈希。

- F3 处理：Capture 排除 `\Recovery\WindowsRE\Winre.wim`（此时是任务副本），镜像不含 WinRE；
  还原完成后由恢复端把 original WIM 放回目标卷并离线注册（即 2B 的「原恢复资产保护 + 重注册」）。
- 优点：不换环境，改动集中在 prepare/recover 两端。
- 缺点：任务期间注册位置是任务副本（断电时系统仍能进 WinRE，风险可控）；
  官方 WIM 的持久性仍受 servicing 威胁（7.2）。

### 8.2 路线 B：任务环境换 WinPE

PE 没有 ReAgent 校验，从任意路径 ramdisk 启动（GUI 已有 `install_pe_ramdisk` +
`{ramdiskoptions}` + boot.sdi 启动链）。注册 WinRE 全程不碰 → F3 根除。

- 优点：启动机制已被 GUI 实现并部分验证；注册 WinRE 与任务完全解耦。
- 缺点：PE 载荷需建设（此前裁定「PE 先不处理」，现在情况变化需重新裁定）；
  PE 与 WinRE 的环境差异（盘符、挂载、工具）需要回归。

### 8.3 路线 C：维持现状

接受 F3 第一段拒绝：无独立 Recovery 分区的机器上主场景（离线备份/还原系统盘）不可用。
验收风险自担，不推荐。
