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
2. **Windows 内没有运行任何应用程序**（KB 6860 原话：**"Dock 里没有 Windows 应用程序的图标"**）。

**前提条件**：
- 必须安装 **Parallels Tools**；
- **「隔离 Mac 与 Windows」(Isolate Mac from Windows) 必须关闭**。

> **关键澄清 1**：触发条件里的"闲置"指的是 **VM 窗口不在前台 + Windows 内无应用**，
> **不是**"Mac 整体无用户操作"。这是初查时理解错的地方。
>
> **关键澄清 2（2026-10-09 用户追问后补）**：这里的"应用程序"指的是**带窗口、会出现在
> macOS Dock 里的 Windows 桌面程序**。**后台 / 托盘常驻进程（无窗口）不会出现在 Dock，
> 不计入条件 2**——所以"VM 里有进程在跑"不等于"会被判定为有应用运行"。

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
BackupRestore      ← 你的测试程序在跑（Win32 桌面 GUI，有主窗口 → 占 Dock 图标）
powershell
cc-switch          ← 后台/托盘常驻，无主窗口，不占 Dock 图标（见下方澄清）
conhost
...
```

> **澄清（重要）**：上面这一列里，`powershell`、`cc-switch`、`conhost` 都是**后台 / 托盘常驻进程**，
> 没有 Windows 桌面窗口，因此**不会出现在 macOS Dock**，Parallels 的"无应用运行"判定**不计它们**。
> 真正卡住条件 2 的是 **`BackupRestore`**——它是 `native_gui.rs` 手写的 **Win32 桌面 GUI 程序**，
> 只要窗口开着，就会在 Dock 占一个 Windows 应用图标 → Parallels 据此判定"有应用运行" → 条件 2 永不达标。
>
> 这一点已被用户实测反证：用户在 **只关掉 BackupRestore、CC Switch 保持运行** 的情况下，
> VM 正常自动暂停了。说明 cc-switch 这类无窗口后台进程**不阻断**自动暂停，只有带窗口的
> 桌面程序（BackupRestore）才阻断。

---

## 四、二次核实结论（根因）

**不是 Mac 被操作的问题，也不是配置没开，更不是"程序正在执行某个操作"。**

真正原因是：**BackupRestore 是一个带窗口的 Windows 桌面 GUI 程序，只要它开着，
就会在 macOS Dock 里占一个 Windows 应用图标；Parallels 据此判定"有应用运行"，
自动暂停的硬条件（Dock 里无 Windows 应用图标）永远不满足 → VM 始终 `running`。**

要点拆解（回答两个常见误解）：

- **❓"是不是因为这个程序在执行什么操作（比如正在备份/还原）？"**
  **不是。** 判定只看"有没有桌面程序的窗口/Dock 图标"，**不看程序当下在不在干活**。
  即便 BackupRestore 完全空闲、什么都没做，只要它的窗口还开着（占了 Dock 图标），
  Parallels 就认为"有应用运行"，照样不暂停。所以**解决方法是"关掉这个程序"，不是"停掉某个任务"**。
- **❓"CC Switch 我也没关，为什么能正常休眠？"**
  **因为 CC Switch 是后台 / 托盘常驻工具（HKCU\Run 自启动，无主窗口），不会在 Dock 出现图标**，
  Parallels 的"无应用运行"判定不计它。同理 `powershell`、`conhost`、各种服务进程也不计。
  只有**带桌面窗口的程序**（如 BackupRestore、Chrome、资源管理器窗口等）才会阻断自动暂停。

一句话：**自动暂停的"无应用" = "Dock 里没有 Windows 应用图标"；窗口类程序开着就挡，后台类进程开着不挡。**

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
