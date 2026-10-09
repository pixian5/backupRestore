# Parallels 虚拟机「Windows 11」没有自动暂停的原因排查

> 排查时间：2026-10-09 08:30
> 环境：macOS（宿主）+ Parallels Desktop 26.4.2 (57518) + VM「Windows 11」(ARM64)
> 结论先行：**配置是开着的，不生效是因为 Mac 一直被操作，没达到「Mac 闲置 15 分钟」的触发阈值。**

---

## 一、现象

VM「Windows 11」始终保持 `running`，即使长时间没在 VM 里做事，Parallels 也没有自动把它暂停（Pause / Suspend）。

---

## 二、排查过程与命令

### 1. 确认 Parallels 工具与 VM 状态

```bash
prlctl --version          # prlctl version 26.4.2 (57518)
prlctl list -a            # Windows 11  running
```

### 2. 看 VM 的自动暂停相关配置

```bash
prlctl list -i "Windows 11" | grep -iE "pause|idle|auto"
```

关键输出：

```
Pause idle: on
```

再直接读 VM 配置文件 `config.pvs`（XML）：

```bash
grep -iE "PauseIdle" "/Users/x/Parallels/Windows 11.pvm/config.pvs"
```

结果：

```xml
<PauseIdleVM>1</PauseIdleVM>
<PauseIdleVMTimeout>900</PauseIdleVMTimeout>
```

| 字段 | 值 | 含义 |
|------|----|------|
| `PauseIdleVM` | `1` | 空闲暂停**已开启** |
| `PauseIdleVMTimeout` | `900` | 阈值是 **900 秒 = 15 分钟** |

### 3. 排除「被外部脚本持续驱动」

你的工作流里常有 br-agent 常驻、vmkey 键盘注入、br-gui 鼠标注入、`prlctl exec` 循环——这些都会让 Parallels 判定 VM 在忙，从而不触发暂停。实查宿主进程：

```bash
ps -axo pid,etime,command | grep -iE "br-agent|vmkey|br-gui|prlctl|parallels|win-clicker" | grep -v grep
```

结果：**没有**任何 br-agent / vmkey / br-gui / `prlctl exec` 常驻循环进程（只有 Parallels 自身进程）。当前也没有挂着的 `prlctl` 子进程。

### 4. 实测 Mac 当前是否「闲置」

Parallels 的 Pause idle 语义是「**当 Mac 自身闲置（用户停止操作 Mac）达到阈值，才暂停 VM**」，不是「VM 自己空闲就暂停」。直接量 Mac 的 HID 空闲时间：

```bash
idle=$(ioreg -c IOHIDSystem 2>/dev/null | awk '/HIDIdleTime/ {print $NF; exit}')
echo "约 $((idle/1000000000)) 秒（阈值 900 秒）"
```

实测输出：**约 119 秒**（远小于 900 秒）。说明你最近一直在操作 Mac（键鼠活动不停重置 idle 计时器）。

---

## 三、根因结论

1. **开关是开的**：`PauseIdleVM=1`，超时 `900s`（15 分钟）。
2. **没被你的自动化脚本干扰**：当前无 br-agent / vmkey / prlctl exec 常驻。
3. **真正原因**：Parallels 的「Pause idle」= **Mac 闲置满 15 分钟才暂停 VM**。你持续使用 Mac（本次排查就在 Mac 上操作），idle 计时器一直被重置，永远到不了 900 秒，所以 VM 不会暂停。

这不是故障，是设计行为。

---

## 四、如何验证机制本身正常

要让它真的暂停，只需：**停止操作 Mac 整整 15 分钟**（不碰键鼠），然后：

```bash
prlctl list -a        # Windows 11 状态应变 paused / suspended
```

若 15 分钟不碰 Mac 后仍不暂停，再排查别的（如 VM 内有持续 CPU/磁盘活动、外接设备、全屏应用等）。

---

## 五、可选优化（是否要做由你定）

如果你想要的是「**VM 在我忙着用 Mac 时也能省资源**」，Parallels 自带功能做不到（它只看 Mac 是否闲置）。可选方案：

1. **手动暂停**：不用时 `prlctl suspend "Windows 11"`（或写个快捷键/脚本一键挂起）。
2. **缩短阈值**：把 `PauseIdleVMTimeout` 调小（如 300 秒 = 5 分钟），让 Mac 一闲置就更快暂停：
   ```bash
   # 经用户确认后再执行，此处仅记录命令
   prlctl set "Windows 11" --pause-idle-timeout 300
   ```
3. **定时挂起**：用 `launchd` 在固定时段（如离开电脑前）自动 `prlctl suspend`。

> 注：你之前有「br-agent 空闲 15 分钟自退」的自有逻辑，那是 agent 进程自退，与 Parallels 的 VM 暂停是两回事，不要混淆。

---

## 六、速查命令

| 目的 | 命令 |
|------|------|
| 看 VM 自动暂停开关 | `prlctl list -i "Windows 11" \| grep "Pause idle"` |
| 看具体阈值（秒） | `grep PauseIdleVMTimeout ~/Parallels/Windows\ 11.pvm/config.pvs` |
| 测 Mac 当前闲置秒数 | `ioreg -c IOHIDSystem \| awk '/HIDIdleTime/ {print int($NF/1000000000); exit}'` |
| 手动挂起 VM | `prlctl suspend "Windows 11"` |
| 恢复 VM | `prlctl resume "Windows 11"` |
