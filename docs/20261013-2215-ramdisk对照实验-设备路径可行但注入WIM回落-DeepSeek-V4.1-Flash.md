# ramdisk 对照实验：设备路径形式可行，但**注入后的 WIM** 回落（2.4.6 E2E 卡点收窄）

> 模型：DeepSeek-V4.1-Flash
> 日期：2026-10-13 22:15
> 基线：HEAD `f84ad66`，VERSION 2.4.6；部署件 `H:\brwork\BackupRestore.exe` = 包目录件 = **`6A60EA0B…`**
> 前序：[21:15 交接文档](./20261013-2115-交接-ramdisk设备路径修复E2E排查续-Sonnet5.md)

## 一、一句话结论

**交接文档的定因成立，且范围可以进一步收窄**：`\Device\HarddiskVolumeN` 设备路径形式在**数据卷**上
**确实能进 WinRE**（本轮实测进到了 WinRE 并弹出 Recovery 界面）；而**换成产品注入后的那份 WIM，
在同一个数据卷上、同样写法就回落**。用户提出的「放程序目录」也已实测：**同样回落**。
**变量锁定在「注入产物」本身，不在 BCD 构造、不在 `dos_device_of`、也不在存放位置。**

### 四轮对照（同一套条目写法，只换载荷与位置）

| # | 载荷 | 位置 | 规格形式 | 结果 |
|---|---|---|---|---|
| A | 产品**注入后** WIM | `F:\BackupRestoreRE\<task>\`（镜像卷） | 设备路径 | ❌ 回落 |
| B | **原始注册件副本** | `F:\BackupRestoreRE\<新GUID>\`（镜像卷） | 设备路径 | ✅ **进 WinRE** |
| C | 产品**注入后** WIM | `H:\brwork\BackupRestoreRE\<task>\`（**程序目录/工作卷**） | 设备路径 | ❌ 回落 |
| D | 产品**注入后** WIM | `C:\BackupRestoreRE\<task>\`（**系统卷**，临时诊断） | **盘符形式** | ❌ 回落 |

**D 组是复现豆包 2.4.5「成功格」的实验，结果同样回落**——即「注入件 + 系统卷 + 盘符形式」
也不能解释豆包那次成功。四轮的结论只剩一个：**变量是注入产物本身**，与存放位置、规格形式都无关
（位置与形式各有两个数据点，载荷只有一轮互换）。

四组截图：A/B/C/D 分别在
`.test-artifacts/boot-menu-verify/{state-2203-gpt61.png, progdir-injected-gpt61.png, sysvol-injected-gpt61.png}`，
B 组是 WinRE 的 Recovery 窗口，A/C/D 三张都是 Windows 桌面。

D 组结束后已彻底清理：C: 上的副本删除、`C:\Recovery\WindowsRE\Winre.wim` 哈希复核仍为
`02E802C8…`（未被动）、诊断条目删除、`{bootmgr}` 与基线逐项一致。

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

| E4 | **什么都不注入**，只做 `/Mount-Image` → `/Unmount-Image /Commit` 往返，并尝试删 `OBSOLETE_WINRE_FILES` | ✅ **进 WinRE** | 截图 `elicit-obsolete-gpt61.png`（与 B 组字节一致）。**注意：`removed=` 为空——本机注册位 WIM 里根本没有那三个旧文件，删除步骤是空操作，可直接排除** |

**结论**：五个对照组（原版件、+env、+Recovery.exe、+task.json、纯 DISM 往返）**全部能进 WinRE**；
产品 WIM（三者同时注入）不能。**触发者 = 三个文件同时注入的组合效应**，而不是任何单个文件、
不是删除旧文件、不是 DISM 提交往返。

## 五之四、下一步（唯一定因实验）

在"能启动的那格"上做 2/2 组合，各一次重启：

1. **env + Recovery.exe**（不放 task.json）
2. **env + task.json**（不换 Recovery.exe）
3. （若 1、2 都 ✅）**Recovery.exe + task.json**（不放 env）

预期只有"三者齐"才失败；若某个 2/2 就先失败，触发组合直接定位。
一个值得先验证的猜想：**E2 单独替换 `Recovery.exe` 时 WIM 里的 env 仍是旧的**，
所以 2.4.6 的 Recovery.exe 还没被"env↔Recovery.exe 哈希对应"这条链路检验过；
而产品 WIM 三者齐时，env 声明的 `recoverySha256` 与 WIM 内 Recovery.exe 是匹配的。
若 2/2 组合里出现"能启动但 Recovery 报错"，说明失败点在 Recovery 内部而非引导。

## 五之五、待清理（E4 现场，未及清）

- BCD 对象 **`{889cf431}`（loader）+ `{889cf430}`（devopts）**
- 载荷目录 `F:\BackupRestoreRE\d4246c84-182c-4955-9cfe-b41fd4722004`
- 脚本复用 `.test-artifacts/ramdisk-elicit-cleanup-gpt61.ps1`

以「能启动的那格」为底（原版件副本 + 镜像卷 F: + 设备路径形式），每次**只加一个文件**：

| 格 | 只加入 | 结果 | 证据 |
|---|---|---|---|
| E1 | `RecoveryTask.env`（2.4.6） | ✅ **进 WinRE** | 截图 `elicit-env-gpt61.png`；Recovery 报 `sha256 mismatch: expected 6a60ea0b… got a2a09a38…`（**正确拒绝**：env 对应 2.4.6，而 WIM 里还是 2.3.7 的 Recovery.exe） |
| E2 | `Recovery.exe`（2.4.6，`6a60ea0b…`） | ✅ **进 WinRE** | 截图 `elicit-recovery-gpt61.png`（与 B 组字节一致：旧 env 触发的同一错误对话框） |
| E3 | `task.json` | ✅ **进 WinRE** | 截图 `elicit-taskjson-gpt61.png`（与 B 组字节一致） |

**含义**：单独注入 `RecoveryTask.env` / `Recovery.exe` / `task.json` **都不破坏引导**。
那么把三者**同时**注入的产品 WIM（`26A1D415…`）为什么进不去，只剩下两类候选：

1. **组合效应**：三者在同一份 WIM 里共同触发（各自单独无影响，合起来有影响）；
2. **注入流程里还没被单独复现的那一步**：`inject_winre_payload` 还会**删除** `OBSOLETE_WINRE_FILES`
   （`BackupRestore.exe`、`RecoveryLauncher.cmd`、`winpeshl-boot.cmd`）——三者单独注入的对照组**没有**这一步。

下一格直接验证候选 2：只做「原版件 + 删掉那三个 System32 旧文件」，不注入任何东西。

## 五之三、待清理（**接手第一步**）

E3 的现场对象未及清理（VM 当时卡在 WinRE 对话框、exec 不可用）：

- BCD 对象 **`{889cf42f}`（loader）+ `{889cf42e}`（devopts）**
- 载荷目录 **`F:\BackupRestoreRE\4a898314-65b8-4cdb-b329-14411189bdd3`**
- 挂载点 `C:\br-verify\elicit-4a898314-65b8-4cdb-b329-14411189bdd3`
- 产品任务 `0d8f1ed5` 与其对象 `{889cf421}/{889cf420}` 仍保留（待判别现场）
- 处置脚本可直接复用：`.test-artifacts/ramdisk-elicit-cleanup-gpt61.ps1`（传上面四个参数即可）

交接文档第 2/3 步的 WMI/BCD 二进制 diff 已可跳过：**存储层差异不是原因**（同一写法在 B 上成功）。

1. **【最关键】逐个剔除注入内容**，把"注入件为什么引导不了"定到具体文件。用**能启动的那一格**
   （原版件 + 镜像卷 + 设备路径 = B 组配置）为底，每次只加一样：
   a. 只注入 `RecoveryTask.env`（2.4.6 的 env，不动 Recovery.exe、不放 task.json）；
   b. 只替换 `Recovery.exe`（2.4.6）；
   c. 只放 `task.json`。
   哪一格开始回落，那一项就是触发者。**必须回到非系统卷做**（B 组已证明该格可启动）。
2. 若定位到触发者：先查它是否让 `winpeshl` 启动阶段就退出（可在同一格里改用"只注入该文件、
   但不修改 winpeshl"来控制变量），再决定修法（换打包方式、换注入时机、或改注入内容）。
3. **豆包 2.4.5 的成功仍是未解之谜**：同样的注入代码，10:58 那次成功、今晚四次全失败。
   已排除：位置（三处）、规格形式（两种）。可疑变量剩：当时那份注册位 WIM 的内容、其它会话
   并发改动、或 20:00 前后的环境变化。**不要在结论里把它当"注入件可引导"的证据。**

## 五之一、建议的修复方向（**待定因后**再动手，现在不要改代码）

用户已明确排除「载荷放 C:」：会污染系统卷，且 `restore-existing` 会格式化 C:，
导致载荷与 `C:\Recovery\WindowsRE\boot.sdi` 一起被抹掉、**断电续跑断链**。
因此修复方向只能是：**载荷留在非系统卷**（镜像卷或程序目录所在的卷），把"注入件为什么引导不了"解决掉。

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
