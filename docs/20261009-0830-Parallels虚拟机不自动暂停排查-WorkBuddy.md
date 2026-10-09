# Parallels 虚拟机「Windows 11」没有自动暂停的原因排查

> 环境：macOS（宿主）+ Parallels Desktop 26.4.2 (57518) + VM「Windows 11」(ARM64)
> 初查：2026-10-09 08:30；**二次核实更正：2026-10-09 08:35（用户质疑定义，已纠正）**

> ## ⚠️ 更正声明（2026-10-09 二次核实）
> 初查时把触发条件误说成「Mac 闲置满 N 秒才暂停 VM」，并据此归因于「你一直在操作 Mac，idle 计时器没到」。
> **这是错的。** 实测把超时改成 10 秒、VM 窗口放到后台、等 13 秒仍不暂停，说明与 Mac 是否被操作无关。
> 真正根因见下方「二次核实结论」：**Windows 内你的 BackupRestore 程序一直在运行，Parallels 据此判定"有应用运行"，硬条件不满足 → 永不暂停。**
> 本文件以二次核实结论为准。

---

## 一、现象

VM「Windows 11」始终保持 `running`，即使长时间没在 VM 里做事、甚至把窗口切到后台，Parallels 也没有自动把它暂停（Pause / Suspend）。

---

## 二、Parallels 自动暂停的官方正确定义（KB 6860 + PD 26 文档）

自动暂停（Pause Windows when possible）要**同时满足**：

1. **VM 窗口处于「非活跃 / 失去焦点（在后台）」状态**超过设定时间；
2. **Windows 内没有运行任何应用程序**（Parallels 依据其应用/Dock 集成状态来判定，即"Dock 里没有 Windows 应用图标"）。

**前提条件**：
- 必须安装 **Parallels Tools**；
- **「隔离 Mac 与 Windows」(Isolate Mac from Windows) 必须关闭**。

> **关键澄清**：触发条件里的"闲置"指的是 **VM 窗口不在前台 + Windows 内无应用**，
> **不是**"Mac 整体无用户操作"。这是初查时理解错的地方。

---

## 三、排查与实测

### 1. 配置与前提（全部满足）

```bash
grep -iE "PauseIdle"  ~/Parallels/Windows\ 11.pvm/config.pvs
# <PauseIdleVM>1</PauseIdleVM>
# <PauseIdleVMTimeout>10</PauseIdleVMTimeout>   ← 用户已改成 10 秒
```

| 项 | 值 | 说明 |
|----|----|------|
| `PauseIdleVM` | `1` | 开关已开 ✓ |
| `PauseIdleVMTimeout` | `10` | 用户改成的 10 秒 ✓ |
| `IsolatedVm` (config.pvs) | `0` | 未隔离 ✓ |
| GuestTools | `installed 26.4.2` | Tools 已装 ✓ |

### 2. 条件1 + 前提已满足，但仍不暂停

把 VM 窗口切到后台（Mac 前台是别的 App），等 **13 秒**（远超 10 秒）：

```bash
prlctl list -a | grep "Windows 11"   # 仍是 running，没有暂停
```

→ 说明「窗口非活跃」这个条件其实不卡（已超时仍未触发），问题在**条件2**。

### 3. 条件2 核查：Windows 内是否有应用运行（真正的卡点）

进 VM 列出非系统进程：

```bash
prlctl exec "Windows 11" powershell -NoProfile -Command "Get-Process | ..."
```

关键输出（节选）：

```
BackupRestore      ← 你的测试程序在跑
powershell
cc-switch
conhost
...
```

**`BackupRestore` 进程正在运行**。这就是根因：Parallels 判定"Windows 内有应用运行" → 条件2永不达标 → VM 永不自动暂停。
（哪怕窗口在后台、超时设 10 秒也没用，因为"无应用运行"这一硬门槛一直过不去。）

---

## 四、二次核实结论（根因）

**不是 Mac 被操作的问题，也不是配置没开。**
真正原因是：**Windows 内你的 BackupRestore 程序（及 powershell 等）一直在运行**，Parallels 据此认为"有应用程序在运行"，自动暂停的硬条件（Windows 内无应用）永远不满足，所以 VM 始终 `running`。

只要 VM 里还开着任何 Windows 应用，这个功能就不会触发——无论你把超时设多短、把窗口放后台多久。

---

## 五、如何验证（自行确认）

1. 在 VM 内**关闭 BackupRestore 程序及所有 Windows 应用窗口**（退出，不只是最小化）；
2. 把 VM 窗口切到后台（Mac 焦点切到别的 App）；
3. 等待 `PauseIdleVMTimeout`（当前 10 秒）以上；
4. `prlctl list -a` 看状态应变 `paused` / `suspended`。

若关闭应用后如期暂停，即坐实根因。

---

## 六、可靠替代方案（不受"无应用运行"限制）

如果你想要"用完 VM 就让它释放资源"，最稳的是**手动/脚本挂起**，不依赖上面的自动判定：

```bash
prlctl suspend "Windows 11"     # 挂起（释放 CPU）
prlctl resume  "Windows 11"     # 恢复
```

也可用 `launchd` 定时（如离开电脑前）自动 `prlctl suspend`。

---

## 七、速查命令

| 目的 | 命令 |
|------|------|
| 看自动暂停开关 | `prlctl list -i "Windows 11" \| grep "Pause idle"` |
| 看阈值（秒） | `grep PauseIdleVMTimeout ~/Parallels/Windows\ 11.pvm/config.pvs` |
| 查隔离是否关闭 | `grep IsolatedVm ~/Parallels/Windows\ 11.pvm/config.pvs`（应为 0） |
| 查 VM 内运行的非系统进程 | `prlctl exec "Windows 11" powershell -NoProfile -Command "Get-Process \| Where-Object {\$_.Name -notmatch '^(System\|Idle\|...)$'} \| Select-Object Name"` |
| 手动挂起 / 恢复 VM | `prlctl suspend "Windows 11"` / `prlctl resume "Windows 11"` |
