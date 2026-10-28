# 「安装 PE」（内存版）端到端实测：安装通过，重启进 PE 在 Parallels 不生效

> 补的是 `经验/07 §3` 里那条「『安装预安装环境』入口内存版/硬盘版端到端 —— **无实机记录**」。
> 结论先给：**安装链路端到端通过**；**「重启进入 PE」在 Parallels 上不生效**，
> 属已知虚拟化限制，不是产品缺陷。真实硬件仍待验证。

* * *

## 一、按硬约束先做可回滚准备（改 BCD 之前必须）

| 项 | 读数 |
| --- | --- |
| `bcdedit /export C:\br-bcd-before-pe.bak` | 文件 **487,424 字节** |
| 备份是否可用（**真判据**） | `bcdedit /store … /enum {bootmgr}` 能读出 `description/default/displayorder` → 可枚举 |
| 原始 `default` | `{a6273652-d15f-11f1-aa4b-ff70ff0b7794}` |
| 原始 `resumeobject` | `{a6273651-d15f-11f1-aa4b-ff70ff0b7794}` |
| 原始 `bootsequence` | 无 |

★ 同时验证了一个老坑：`bcdedit /export` 的**退出码不可靠**，判据必须是「文件非空 + 能枚举」。
这次文件 487KB 且可枚举，才继续往下走。

## 二、怎么触发的（不用手点，走既有测试钩子）

GUI 内置测试钩子：存在 `C:\br-test.json` 时按 JSON 自动填表并（`auto_install=true`）自动安装。
写入内容：

```json
{"tab":"pe","mode":"ram","pe_dir":"C:\\BackupRestorePE","pe_name":"BR-PE-E2E",
 "pe_image":"H:\\BRImages\\pe-boot.wim","auto_install":true}
```

然后以 `--test-hook` 启动 GUI（新增 `tools/win-clicker/start-gui-hook.ps1`，ASCII-only）。

约束提醒：**PE 目录不能放在运行本程序的卷上**（本次程序在 `H:`，所以放 `C:\BackupRestorePE`）。

## 三、结果一：安装链路 = 通过（四项判据）

| # | 判据 | 实测 |
| --- | --- | --- |
| 1 | 文件落地 | `C:\BackupRestorePE\boot\boot.sdi` + `C:\BackupRestorePE\sources\boot.wim` |
| 2 | BCD 新增项内容正确 | `osdevice ramdisk=[C:]\BackupRestorePE\sources\boot.wim,{…}`、`winpe Yes` |
| 3 | **不改默认启动** | `default` / `resumeobject` 与安装前**逐字一致** |
| 4 | 出现在启动菜单里 | `displayorder` 末尾**追加** PE 项（前两项保持） |
| 5 | 用户可见确认 | 界面弹「安装完成 / PE (RAM disk) 已安装到 C:\BackupRestorePE / 启动菜单已新增「BR-PE-E2E」/ 当前 Windows 默认启动未改动」 |

## 四、结果二：「重启进入 PE」= 在 Parallels 不生效

按控件 ID（`ID_PE_REBOOT_MAIN=1410`）点击按钮，GUI 日志给出完整证据链：

```
GUI action: control=ID_PE_REBOOT_MAIN id=1410 notification=0x0
PE reboot to PE: bcdedit.exe /set {bootmgr} bootsequence {17f55dc8-…} -> exit code= 0
PE reboot to PE: bootsequence set, auto reboot now
```

也就是说：**按钮逻辑正确**——把 `bootsequence` 指向**刚装的 PE 项**并重启。

但机器重启后**回到了原 Windows**（GUI 是新实例，标签页复位到「备份」），且 `bootsequence` 已被清空。

**判断：固件/虚拟机不消费 `bootsequence`，不是产品缺陷。**
这与产品界面上自述的限制一致（原文：「注意：Parallels 等虚拟机的 `bootsequence` 可能不被固件消费，真实硬件路径有效」）。

★ 未验证的部分要说清：**真实硬件/物理机上能否真的进 PE，本轮没有证据**。
要闭环需要一台能物理重启的机器，或手工在 Parallels 的启动菜单里选那一项（另一个动作，未做）。

## 五、清理与恢复（已复核）

| 动作 | 复核结果 |
| --- | --- |
| `bcdedit /delete {17f55dc8-…}` | `BR-PE-E2E` 残留 **0**、GUID 残留 **0** |
| `rmdir /s /q C:\BackupRestorePE` | 目录 `REMOVED` |
| 启动菜单 | `displayorder` 回到 **2 项**（Windows + 第二系统） |
| 默认项 | `default` = `{current}`（= 原 `{a6273652…}`）、`resumeobject` 未变 |
| 临时备份 | 复核通过后删除 `C:\br-bcd-before-pe.bak` |
| 钩子文件 | 重启前已删 `C:\br-test.json`（否则重启后会自动再装一遍） |

## 六、这轮踩到的三个读数坑（都值得记住）

1. **`displayorder` 是多行值。** 我用 `grep -Ei "displayorder"` 只看首行，于是「PE 项不见了」
   —— 差点断定产品在重启后把菜单项删了。**同一口径重读**（`sed` 取整块 + 缩进续行）才看清三项都在。
   ★ 又一次印证：判据红时先证明**读数那一侧**是对的。
2. **坐标点击不可靠。** 截图 2350×1938（物理像素），会话逻辑分辨率 1175×969，
   `SetCursorPos` 在 DPI 不感知的进程里会被虚拟化。改按**控件 ID 发 `BM_CLICK`**（新脚本
   `tools/win-clicker/_click-ctrl.ps1`），与 DPI/坐标无关。
3. **`_click-1410.ps1` 是个空壳**：它 `& C:\Users\Public\pkg\clickctrl.ps1`，而那个文件不存在，
   只报「系统找不到指定的文件」。看到 `launch code=0` 别以为成功——**退出码不是判据**。

## 七、状态

- 未改产品代码（只新增两个开发工具脚本）→ 版本仍 **2.6.74**。
- `经验/07 §3` 那条「安装 PE 端到端无实机记录」本轮**部分关闭**：
  内存版安装已实测；**硬盘版（会格式化独立分区）仍未测**；真实硬件启动进 PE 仍未测。
