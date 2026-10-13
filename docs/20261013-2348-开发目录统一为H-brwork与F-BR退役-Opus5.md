# 开发目录统一为 H:\brwork，F:\BR 退役（三套工作区交叉污染根因与清理）

> **一句话**：进 RE 弹出的 `I/O error: 系统找不到指定的路径。 (os error 3)`，根因是这台机器上
> **同时存在三套 BackupRestore 程序位置、两份不同构建**，它们共用机器上**唯一那份 `bootmgr` 一次性启动槽位**，
> 于是出现"A 的一次性启动请求还挂着、B 的任务目录却被清理"的交叉污染。
> 2026-10-13 用户裁定：**开发目录唯一 = `H:\brwork`**，`F:\BR` 退役删除，部署目标统一，H: 卷清掉镜像遗留。

- 版本：**2.4.7**（本轮由 2.4.6 递增；`VERSION` + 两个 crate `Cargo.toml` 三处同步）
- 执行时刻：2026-10-13 23:41–23:48
- 证据边界：全部结论来自 `prlctl exec` 实测读取（目录时间戳、`Get-FileHash`、`C:\BackupRestore-Recovery-early.log`、
  `bcdedit /enum {bootmgr}`），未凭假设下结论。

## 1. 起因：RE 里的 os error 3 弹窗

弹窗原文（`main.rs:852` 的 `recover_env_interactive` MessageBox，Retry 会循环）：

```
BackupRestore 恢复需要处理
恢复暂未完成：I/O error: 系统找不到指定的路径。 (os error 3)
```

`os error 3` = `ERROR_PATH_NOT_FOUND`。定位到 `recover_env`（`main.rs` 867 起）的流程：

1. 读 `X:\windows\System32\RecoveryTask.env` → 拿 `TASK_ID` / `WORKSPACE_ROOT_REL`
2. **挂载工作区卷**（打日志 `mount WORKSPACE OK -> X:`）
3. `store.load(task_id)` 读 `…\tasks\<id>\task.json` ← **这一步失败不打任何日志**
4. 才打 `task loaded status=… operation=…`

早期日志 `C:\BackupRestore-Recovery-early.log` 里，任务 `8b117e77-3d2d-4df6-bf83-60812985912e`
反复出现，每次都是：

```
=== recover-env START path='X:\windows\System32\RecoveryTask.env' ===
TASK_ID=8b117e77-3d2d-4df6-bf83-60812985912e workspace_root_rel=BR\tasks\8b117e77-…
mount WORKSPACE OK -> G:
        ← 到这里就断（既无 task loaded，也无 mount RECOVERY）
```

**只走到工作区挂载成功就断 = 该任务目录在已挂载的工作区上不存在** → `store.load` 报 os error 3。

## 2. 三套程序位置 / 两份构建（实测）

| 位置 | 所在卷 | 构建 SHA256 | 实测状态 |
|---|---|---|---|
| `C:\Users\Public\backupRestore-package` | **C: 系统盘** | `6A60EA0B…` | GUI 自启点，9/11 起，**当前正在运行的进程就是它** |
| `H:\brwork` | **H: 数据卷（标签 DATA，206G）** | `6A60EA0B…`（同上） | 工具链 `DeployWorkDir`，创建于 10/12 16:49，今天 20:41 更新；3 个任务 |
| `F:\BR` | **F: 镜像卷（标签 BRIMG，375G）** | `F29DFEDC…`（旧） | 10/11 创建；`tasks` 已空；**os error 3 的源头** |

`6A60EA0B…` 正是任务 `0d8f1ed5` 报的 `expected 6a60ea0b…`（载荷清单里 `Recovery.exe` 的期望值），
佐证"当前这一版"就是 `H:\brwork` / C: 包目录里这份，`F:\BR` 那份已过期。

卷角色（实测 `Get-CimInstance Win32_LogicalDisk`）：

| 盘符 | 标签 | 大小 / 可用 | 角色 |
|---|---|---|---|
| C: | — | 267G / 179G | 系统盘（注册 WinRE、GUI 包目录、新通道载荷暂存 `C:\BackupRestoreRE`） |
| E: | — | 289G / 210G | 大目标卷（可做第二 Windows 卷） |
| **F:** | **BRIMG** | 375G / **371G** | **镜像卷**（`F:\BRImages`）；已不再放程序 |
| **H:** | **DATA** | 206G / 163G | **工作区**（唯一开发目录 `H:\brwork`） |
| P: | — | 64G / 66G | 测试目标卷 |
| T: / Y: | DATA / WINRE | 5G / 0.8G | 小数据卷 / WinRE 宿主（别碰） |

## 3. 用户裁定与已执行动作

用户授权 5 项，全部执行完毕（每项都回读确认）：

| # | 动作 | 结果 |
|---|---|---|
| ① | 开发目录用 `H:\brwork` | 采纳（它本就是工具链 `DeployWorkDir`，且持有当前构建） |
| ② | 删除 `F:\BR\` | **已删**，回读 `FBR_GONE`。清点：62 个文件 / 17.8MB（旧构建 exe + `.bak`、12+ `.backuprestore-*.bcd.LOG*`、`online-run*.log`、空 `tasks`、`Recovery.log` 685KB），**无有价值镜像** |
| ③ | 统一部署目标 | `build-win.sh` 与 `windows/build-windows.ps1` 的 `DeployWorkDir` **本来就是 `H:\brwork`**，已加"F:\BR 已退役、禁止再往 F: 部署"的显式注释；全仓 grep 确认无其他处教人部署到 `F:\BR`。另把约定源头手册 `docs/20261012-0002-…-Fable5.md`（约 20 处 `F:\BR`）全部改为 `H:\brwork` 并修正其卷布局表 |
| ④ | 更新项目记忆磁盘布局 | `MEMORY.md` 里"F: 工作区/程序放 F:\BR、H: 镜像库"已校正为实测布局，并新增「开发目录唯一 = H:\brwork」章节（含 os error 3 排查抓手） |
| ⑤ | 清 `H:\BRImages`、`H:\winre-test.wim` | **均已删**。`winre-test.wim` 722,034,583 B（10/12 02:22），释放 722MB |

`F:\BackupRestoreRE` 在 **F: 卷根**、不属于 `F:\BR`，未动（其中 `d4246c84 / 0d8f1ed5 / 852c0dad / 6d41ee94`
四个载荷目录仍与 H:\brwork 的任务相关，留待确认后再处理）。

## 4. 踩到的坑（记下来别再踩）

- **`del /f /q` 对客体大文件会谎报"找不到"**：`del /f /q H:\winre-test.wim` 报
  `找不到 H:\winre-test.wim`，但 `if exist` 与 `dir` 都显示文件在（722,034,583 B）。
  改用 `Remove-Item -LiteralPath 'H:\winre-test.wim' -Force` 一次删掉。
  **以后删客体大文件优先用 PowerShell `Remove-Item -Force`，别信 cmd `del` 的报错。**
- **`prlctl exec` 在 RE 里会瞬时不可用**：刚进 RE 时报
  "Unable to open new session…Make sure your virtual machine has finished booting"，稍后重试即恢复
  （GuestTools state=installed 一直在）。不要据此断定"RE 里没有代理、只能键鼠注入"。
- **cmd 单行里串多个 `&` 的 `for` 循环输出不稳定**：同一个命令有的能出、有的返回空，
  定位盘符/目录时**拆成单条 `if exist … echo …`** 最可靠。

## 5. 遗留待定夺

- **`H:\c-backup\c-system.wim` = 36,964,295,481 B（36.9 GB）**，10/12 02:38 的**真实 C 盘备份产物**
  （同目录有 `metadata.json` / `c-system.wim.index-1.metadata.json` / `Recovery.log`）。
  用户本轮只授权删 `H:\BRImages` 与 `H:\winre-test.wim`，**未授权删它，故未动**。
  若要让 H: 真正只做程序/工作区，需把它迁到 F:（镜像卷，剩 371G 足够）——迁移属大文件操作，待用户拍板。
- `F:\BackupRestoreRE\` 下 4 个任务载荷目录（其中 `0d8f1ed5`、`852c0dad` 在 `H:\brwork\tasks` 仍有任务目录），
  是否清理待确认。
- 本轮只改了构建/部署脚本、手册与记忆，**没有改 Rust 源码**，因此 2.4.7 需重新
  `./build-win.sh --deploy` 后二进制才会报 2.4.7（否则客体上仍是 2.4.6 的构建）。

## 6. 相关文件

- 排查入口：`C:\BackupRestore-Recovery-early.log`（及 `C:\WinRE-PoC\` 下同名文件）
- 部署脚本：`build-win.sh`（宿主交叉编译 + 部署）、`windows/build-windows.ps1`（VM 内构建 + 部署）
- 被修正的手册：`docs/20261012-0002-虚拟机备份还原测试操作手册-Fable5.md`
- 关联：`docs/20261013-0945-WinRE硬盘版释放实测与开机循环处置-Opus5.md`（同日 2.4.2 实测）
