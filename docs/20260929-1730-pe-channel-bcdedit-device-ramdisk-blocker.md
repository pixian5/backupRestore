# v1.7.11 PE 式通道阻塞：bcdedit 的 `device ramdisk=…` 只在「非本进程后代」里成功

> 状态：**阻塞（未解决），改造代码已在 main 但不可用**。本文件记录完整的测量过程，
> 供下次继续排查时直接从「还没排除什么」开始，不要再从头猜。
> 关联：[20260929-130000-pe-channel-poc-*.md](20260929-1300-pe-channel-poc-winre-wim-boots-from-image-volume.md)（PoC 通过）、
> [20260929-103500-*.md](20260929-1035-winre-finalize-wrote-to-scratch-not-home.md)（v1.7.10 修复）。

## 1. 现象

产品进程（`BackupRestore.exe prepare`）里执行：

```
bcdedit /set {我们自建的 loader} device ramdisk=[F:]\BackupRestoreRE\Winre.wim,{我们自建的设备选项对象}
```

**稳定失败**：退出码 1，输出「指定的设备无效 / 运行 "bcdedit /?" 获得命令行帮助 / 参数错误」。

同一条命令、同一对 BCD 对象，**从 cmd 或 PowerShell 里手工执行立刻成功**。
PoC 阶段全程用脚本建条目，所以那时没有遇到这个问题（见 130000 文档，任务闭环 PASS）。

## 2. 诊断电池记录的事实（同一次 prepare 里，逐条打日志）

| 调用 | 结果 |
|---|---|
| `/enum {loader} /v` | rc=0 正常读 |
| `/set {设备选项} description …` | rc=0 |
| `/set {loader} description …` | rc=0 |
| `/set {loader} path \windows\system32\winload.efi` | rc=0 |
| `/set {loader} device partition=C:` | **rc=0** |
| `/set {loader} device ramdisk=[F:]\…\Winre.wim,{ramdiskoptions}` | rc=1 |
| `/set {loader} device ramdisk=[F:]\…\Winre.wim,{自建设备选项}` | **rc=1** |
| 失败后原样重试（`.output()` / `.status()` / 经 `cmd /d /c`） | **全部 rc=1** |

另外两条独立证据：

1. **假 WIM 路径也能成功**：手工执行 `device ramdisk=[F:]\BootupRestorePE\NoSuch.wim,{opts}`
   （WIM 不存在）同样 rc=0 → **bcdedit 根本不打开 WIM**，所以问题不在文件。
2. **进程内独占打开 WIM/SDI 成功**（`share_mode(0)`）→ 没有锁冲突。

## 3. 已逐一测量排除的假设

| 假设 | 排除方式 | 结论 |
|---|---|---|
| DISM 刚跑过污染了进程 | 把建条目移到 DISM 之前 | 仍失败 |
| 当前目录/当前驱动器 | 子进程 `cd /d` 到 `F:\`、`F:\BackupRestoreRE`、`C:\Windows`、`H:\brwork` 各试 | 仍失败 |
| 调用形态（管道/stdout/控制台） | `.output()`、`.status()`、`Stdio::inherit()`、`CREATE_NO_WINDOW` 有无 | 仍失败 |
| 句柄继承 | `CreateProcessW(bInheritHandles=FALSE)` 原生 FFI 起进程 | 仍失败 |
| 程序名解析 | `bcdedit.exe`（PATH）与 `C:\Windows\System32\bcdedit.exe` | 仍失败 |
| 700MB 文件刚拷完 | 把拷贝移到 BCD 全部写完之后 | 仍失败 |
| ESP 挂载未释放 | 建条目移到 `snapshot_raw_bcd`（挂载 ESP 到 S:）之前 | 仍失败 |
| 计划任务（非本进程后代） | `schtasks /RU SYSTEM` 跑同一条 | 退出码 0 但**未生效**（引用号被吞，属无效实验，不计入排除） |
| 环境变量差异 | `cmd /c set` 全量 dump 与 shell 对比 | 未见差异（dump 曾被截断到 30 行，需重做完整对比） |
| 模板对象不对 | 换成 Y: 分区 WinRE 条目、`{ramdiskoptions}`、C: WinRE 条目 | 手工全部成功、进程内全部失败 |

**唯一稳定的相关性**：bcdedit 是否是 `BackupRestore.exe` 的后代。
是 → 失败；不是（cmd/PowerShell 直接起）→ 成功。与前面的 DISM、cwd、调用形态、句柄继承都无关。

## 4. 暂定结论与下一步建议

先按「**产品进程自身的某个状态会让子进程里的 bcdedit 拒绝 `ramdisk=` 设备值**」继续查，
优先级从高到低：

1. **完整环境变量对比**（上次 dump 被 `take(30)` 截断，必须重做全量 diff）。
2. **Process Monitor 抓一次**：分别抓「产品内失败的那次」和「cmd 里成功的那次」，
   对比 bcdedit 打开的文件/注册表/API 调用差异——这是唯一还没用过的观测手段。
3. **用非 Rust 子进程做同一写入**：例如产品 spawn `powershell -Command "& bcdedit …"`，
   若成功→问题在 Rust 进程创建；若失败→问题在产品进程本身。
4. 若定位为产品进程状态：把「建条目」整段挪到一个**一次性子进程**里（例如
   `BackupRestore.exe boot-entry-create --task <dir>`，父进程只等退出码）——
   注意：朴素地 spawn 自己的 exe 已试过，仍然失败（子进程仍继承产品进程的某些状态），
   所以这条要先靠第 3 步确认 Rust/CreateProcess 是否就是差异来源。

## 5. 对当前代码的影响

- main 上的 v1.7.11 改造**编译、单测都通过**（macOS + aarch64-pc-windows-msvc 双目标），
  但**实机跑不通**：prepare 会在建条目这一步失败。
- VM 里 `H:\brwork\BackupRestore.exe` 仍是**已验证的 v1.7.10** 构建，
  机器注册位在 C:、WinRE 正常，可继续用。
- 在解决第 4 节之前，**不要把 v1.7.11 用于任何实机备份/还原**。
