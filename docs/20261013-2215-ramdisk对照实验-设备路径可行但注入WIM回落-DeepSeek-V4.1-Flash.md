# ramdisk 对照实验：设备路径形式可行，但**注入后的 WIM** 回落（2.4.6 E2E 卡点收窄）

> 模型：DeepSeek-V4.1-Flash
> 日期：2026-10-13 22:15
> 基线：HEAD `f84ad66`，VERSION 2.4.6；部署件 `H:\brwork\BackupRestore.exe` = 包目录件 = **`6A60EA0B…`**
> 前序：[21:15 交接文档](./20261013-2115-交接-ramdisk设备路径修复E2E排查续-Sonnet5.md)

## 一、一句话结论

**交接文档的定因成立，且范围可以进一步收窄**：`\Device\HarddiskVolumeN` 设备路径形式在**数据卷 F:** 上
**确实能进 WinRE**（本轮实测进到了 WinRE 并弹出 Recovery 界面）；**同一个位置、同一套条目写法**，
换成产品**注入后**的那份 WIM 就回落。**变量锁定在「注入产物」本身，不在 BCD 构造、不在 `dos_device_of`。**

## 二、先排除的两项（都是交接清单第 1 步）

| 项 | 实测 | 结论 |
|---|---|---|
| 部署件是否真含修复 | `H:\brwork\BackupRestore.exe` 与包目录件 SHA-256 均为 `6A60EA0B…`，字节搜索 `QueryDosDeviceW` = **True** | 旧二进制不是原因 |
| BCD 基线 | `default={12e3701c…}`、`displayorder={152dcf4e…},{current}`、`timeout=10`、`device=\Device\HarddiskVolume2`、**无 bootsequence** | 与交接基线逐项一致 |

另确认：VM 里那个 GUI 进程（PID 11260）**没有** DISM/Recovery 子进程，任务目录停在 `boot-requested`，
`F:\BRImages` 里只有豆包 10:58 的产物 —— 它不是在跑备份，所以后续重启没有打断任何有效工作。

## 三、单变量对照实验（本轮核心证据）

两次实验**只有 WIM 内容不同**，条目构造、卷、路径形状、SDI 全部相同（均为设备路径形式 `[\Device\HarddiskVolume11]`，
`F:` = disk2/part2 = GPT `{f0753766-30a4-410e-944f-38d139113634}`）：

| 实验 | 载荷 | 大小 / SHA-256 | 结果 |
|---|---|---|---|
| **A**（手工新建条目，指向产品暂存件） | `F:\BackupRestoreRE\0d8f1ed5…\Winre.wim`（**注入后**） | 724,631,103 B / `26A1D415…` | ❌ **回落**（bootsequence 被消费、任务仍 boot-requested、无 WIM） |
| **B**（手工新建条目，指向原版副本） | `F:\BackupRestoreRE\b787fde9…\Winre.wim`（**原始注册件副本**，同目录形状 + 隐藏/系统属性） | 722,034,583 B / `02E802C8…` | ✅ **进 WinRE**（截图 `.test-artifacts/boot-menu-verify/state-2203-gpt61.png`：`BackupRestore 恢复需要处理` 窗口） |

> B 的弹窗内容为「I/O error 系统找不到指定的路径。(os error 3)」——那是**原版 WIM 里遗留的旧 env**
> （指向 2026-10-11 的任务 `8b117e77` / `H:\c-backup\c-system.wim`，且该 WIM 内**没有 task.json**）导致的，
> 与本次产品任务无关。**关键是：ramdisk 加载成功了，WinRE 起来了，Recovery.exe 也起来了。**

两次 WIM 都结构完好：`dism /Mount-Image /ReadOnly` 均挂载成功，`/Get-WimInfo` 两者都是
`Index 1 / WindowsPE / arm64 / 10.0.26100.8031 / WIM Bootable: No`。

## 四、注入件与原版件的内容差异（已实测，供下一步定因）

| 文件（`Windows\System32\`） | 注入件 | 原版件 |
|---|---|---|
| `winpeshl.ini` | 99 B，同哈希 `743F193A…` | 99 B，同哈希 `743F193A…` |
| `Recovery.exe` | **2,414,080 B**（2.4.6，`6A60EA0B…`） | 2,338,304 B（2.3.7，`A2A09A38…`） |
| `RecoveryTask.env` | 2,705 B（`939F2C5A…`，任务 0d8f1ed5 / ALLOW_DESTRUCTIVE=YES） | 2,696 B（`8E061277…`，旧任务 8b117e77 / NO） |
| `task.json` | 有（1,966 B） | **无** |

即：注入件多出/替换了 3 个文件，且 `Recovery.exe` 大了约 75 KB。**这些差异中哪一个让 bootmgr 拒绝加载 ramdisk，
尚未定因** —— 不要在下一轮直接断言。

## 五、下一步（两格实验，成本各一次重启）

交接文档第 2/3 步的 WMI/BCD 二进制 diff 已可跳过：**存储层差异不是原因**（同一写法在 B 上成功）。
直接做位置 × 内容的 2×2：

1. **注入件 + C:（系统卷）+ 盘符形式** —— 复现豆包 2.4.5 的成功格；若成功，说明「注入件本身可引导」，
   问题在「注入件 + 数据卷」的组合。
2. **原版件 + F: + 设备路径** —— 本轮 B 已做，成功。
3. 若第 1 格成功：把注入件的**注入内容**逐项剔除（先只留 `Recovery.exe` 不换 env，再只换 env），
   用同一格复测，锁定是哪一个文件触发的。

## 六、本轮现场处置（已完成）

- 我建的 4 个诊断条目 `{889cf423}/{889cf422}/{889cf425}/{889cf424}` 已按精确 GUID 删除并枚举回读确认消失；
- 测试副本 `F:\BackupRestoreRE\b787fde9-…` 已删除；
- **产品对象 `{889cf421}/{889cf420}` 与任务 `0d8f1ed5` 保留**（它是待判别的现场）；
- `{bootmgr}` 复核：default/displayorder/timeout 与基线逐项一致、**无 bootsequence**；
- VM 一度卡在「exec 报 `Unable to open new session`」，按交接文档处置（`stop --force` + 8s + `start`）恢复，
  已回到 Windows（`BACK_ON_WINDOWS`）。

## 七、证据边界

- 本轮**只做了只读诊断 + 手工条目的一次性引导实验**；未改产品源码、未部署、未跑 E2E 备份。
- 结论「设备路径形式在数据卷可行」由**单次进 WinRE 的截图**支撑（可回溯）；「注入件是变量」由
  两次对照（A/B）支撑，样本各 1 次，**重复性未验证**。
- 未使用 WMI BCD 原始元素读取（交接第 2 步），因为 B 组已证伪「存储层差异」这一分支。
