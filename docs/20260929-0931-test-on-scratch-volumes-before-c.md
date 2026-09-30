# 测试纪律：先在测试盘验证，再谈真实 C:（2026-09-29）

> 起因：用户明确要求「每当你修了改了功能之后，你别直接对着 C 盘进行测试，你先通过对测试盘
> 进行测试，确定功能起码没问题之后，如果我要求你对 C 盘测试的话你再测试」。
> 在此之前，我在 WinRE 迁出方案（docs/20260928-233000-*）上**连续 7 次直接对真实 C: 跑还原**，
> 每次一轮几十分钟、每次失败都把 VM 的 WinRE 注册留在不一致态。此文档记录代价与后续纪律。

## 1. 为什么这条纪律是对的（实测代价）

真实 C: 一轮 = 72.6GB 源、镜像 31.2GB、耗时 20~40 分钟。失败不是"再来一次"那么简单：

| 失败留下的脏状态 | 后果 | 修复方式 |
|---|---|---|
| `reagentc /disable` 已执行、`/setreimage` 才失败 | **系统 WinRE 变 Disabled**，注册位指向一个不存在的文件 | 手工 `reagentc /enable`（或先 `/setreimage /path C:\Recovery\WindowsRE` 再 `/enable`） |
| 迁出到暂存卷后失败 | 注册指向 `<暂存卷>:\Recovery\WindowsRE\Winre.wim`，**而 C: 注册位已被清空/未还原** | 拷任务目录 `original\Winre.wim` 回 `C:\Recovery\WindowsRE`，再 setreimage+enable |
| 暂存卷被回收但注册还指着它 | `reagentc /info` 位置不可达，WinRE 起不来 | 同上 |
| 格式化前的失败 | C: 未动，但 bootstatus/`boottore` 可能残留 | `reagentc /disable` + `/enable` 循环复位 |

**2026-09-29 实际发生**：任务 `fc314c4e` 迁出成功后卡在 `boot-requested`，
WinRE 会话起来过（5 个卷全部挂载成功，见 `Recovery-early.log`）但没写出 `recovery.log` 就静默退出；
之后 VM 回到 Windows，而 **WinRE 处于 Disabled、暂存卷 F:\Recovery 已被清空**——
即"注册指向一个已删除的文件"。最终靠手工三步救回：

```cmd
copy /y H:\brwork\tasks\<id>\original\Winre.wim C:\Recovery\WindowsRE\Winre.wim
reagentc /setreimage /path C:\Recovery\WindowsRE
reagentc /enable
```

## 2. 纪律（已写入 .workbuddy/memory/MEMORY.md）

1. **改完/修完任何功能 → 先在测试盘跑通**，确认功能起码没问题。
2. **只有用户明确点名「对 C 盘测试 / 做 C 盘验收」才动 C:**。用户没说 = 默认测试盘。
3. **F1/F3（源或目标承载注册 WinRE）在测试盘上照样能复现**，不必拿 C: 冒险：

```cmd
rem 把注册临时搬到测试卷 P:，即可触发迁出路径
reagentc /disable
reagentc /setreimage /path P:\Recovery\WindowsRE
reagentc /enable
rem ……对 P: 跑备份/还原验证……
rem 测完务必搬回 C:
reagentc /disable
reagentc /setreimage /path C:\Recovery\WindowsRE
reagentc /enable
reagentc /info
```

4. 测试卷角色（沿用 2026-09-27 约定）：workspace=`H:\brwork`、image=`F:\`、source=`T:`(5GB)、target=`P:`(64GB)。

## 3. 顺带发现的待测 bug（转测试盘复现）

任务 `fc314c4e` 的 `Recovery-early.log` 显示 WinRE 内挂载全部成功：

```
mount RECOVERY complete via pre-mounted letter G:
mount SOURCE  complete via pre-mounted letter C:   (disk=0 partition=4)
mount IMAGE   complete via shared letter G:        (disk=2 partition=2 = F:)
mount TARGET  complete via shared letter C:
mount EFI     complete via mountvol in 786ms       (Z:)
```

之后**没有 `recovery.log`**，任务停在 `stage=boot-requested`。即：挂载成功后、写恢复日志前
进程就静默退出了。可能方向：① 挂载后第一个检查（F1 校验 / env 校验）失败但错误没落盘；
② `recovery_progress` 窗口线程异常导致进程退出；③ payload 的 `winpeshl.ini` 拉起后
`Recovery.exe` 提前返回。**这个必须在测试盘上复现定位，不要拿 C: 试。**

## 4. 当前 VM 状态（2026-09-29 09:3x）

- 已恢复到一致态：WinRE 注册 = `harddisk0\partition4`（C:），已启用，
  `C:\Recovery\WindowsRE\Winre.wim` = 任务 `fc314c4e` 的干净原件（712,111,529 字节）。
- 镜像 `F:\c-real2.wim` = 33,487,126,711 字节（真实 C: 备份产物，保留备用）。
- 无快照（为避免 C: 重写时增量爆盘已删）。
- 待用户明确要求后，才用 C: 做最终验收。
