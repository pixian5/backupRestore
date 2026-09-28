# Parallels 气球文件（Mac disk）撑爆宿主磁盘 — 事故复盘与处置

## 现象

- 宿主 `/` 只剩 **353MB / 393MB**（926GB 盘），Parallels 拒绝启动 VM：「虚拟机无法启动」。
- 客体每个 NTFS 卷根目录下都有一个名为 `Mac disk` 的巨型文件：E:/F:/H: 各 ~190-220GB，
  且**持续增长**（每次观察都在变大）。
- 删不掉：`del` 被拒（体积太大/被占用）；`format F:` 能清，但**重启后自动重建**。

## 根因

`Mac disk` 是 **Parallels 的磁盘气球（disk balloon）文件**，由 Parallels Tools 在每个卷上创建，
用途是把客体空闲空间"占满写零"，让宿主侧能回收/压缩 `.hdd`（在线压缩机制）。

- 触发条件是**宿主空间紧张**：宿主越满 → Parallels 越想回收 → 写越大的气球 → 宿主越满。
  **正反馈死循环**，这是它一夜之间吃掉几百 GB 的原因。
- `prlctl set --device-set hddN --online-compact off` **挡不住它**（四个盘都已 off，气球照样重建）。
- 关机重启、PendingFileRenameOperations、SYSTEM 计划任务删文件——都拦不住，
  因为 Tools 一启动就重建。

## 已验证的事实

1. **停掉 Parallels Tools Service 的瞬间，气球文件自动消失**（不是被删，是 Parallels 自己回收）：
   ```
   net stop "Parallels Tools Service"
   → E: free=237GB  balloon=False
     F: free=350GB  balloon=False
     H: free=176GB  balloon=False
   ```
2. 一起 Tools 就重建（20:59:32 开机重建过一次，实测确认）。
3. 因此**可利用的窗口** = Tools 停止期间：此刻各卷空闲是真实的，
   足够跑 BackupRestore 的 `prepare`（它会在 Windows 侧做空闲空间校验，然后自己重启进 WinRE）。
4. **WinRE 里没有 Tools，也就没有气球**——捕获阶段不受它影响。

## 处置（本次实际操作）

1. **宿主侧只删我自己产生的东西**，不动任何用户文件：
   - `rm -rf /Users/x/code/backupRestore/target`（8.7GB 构建产物，可重新编译）
   - `prlctl snapshot-delete` 删掉全部 Parallels 快照链（54GB）
   - 结果：宿主 393MB → **136GB 可用**，VM 正常启动。
2. 需要大空闲空间的操作一律放在「Tools 停止窗口」内执行（见下「可复用套路」）。
3. **不要再给虚拟盘扩容**——扩容只会给气球更大的成长空间，宿主死得更快。

## 可复用套路：在 Tools 停止窗口里跑大空间操作

停 Tools 会断掉 `prlctl exec` 通道，所以必须用 SYSTEM 计划任务异步编排：

```cmd
:: C:\Users\Public\pkg\rb2.cmd  （ASCII 正文，PowerShell -File 不吃中文）
@echo off
set LOG=C:\Users\Public\pkg\rb2.log
echo === run %DATE% %TIME% === > %LOG%
net stop "Parallels Tools Service" >> %LOG% 2>&1
H:\brwork\BackupRestore.exe prepare --operation backup --source-drive C ^
    --image-path F:\c-real.wim --compress fast >> %LOG% 2>&1
echo PREPARE_EXIT=%ERRORLEVEL% >> %LOG%
if not "%ERRORLEVEL%"=="0" net start "Parallels Tools Service" >> %LOG% 2>&1
```

触发（宿主侧）：

```bash
prlctl exec "Windows 11" cmd /d /c "copy /y \\\\Mac\\backupRestore\\.test-artifacts\\rb2.cmd C:\\Users\\Public\\pkg\\ >nul & \
  schtasks /create /tn rbrun2 /ru SYSTEM /rl HIGHEST /sc once /st 23:59 /tr \"cmd /c C:\\Users\\Public\\pkg\\rb2.cmd\" /f & \
  schtasks /run /tn rbrun2"
```

要点：
- 用 **cmd 批处理**而不是 PowerShell `& 'exe' ...`（后者在计划任务里执行不成功，现象是退出码为空）。
- 日志写客体本地路径 `C:\Users\Public\pkg\*.log`，宿主回读。
- 失败分支要 `net start` 把 Tools 拉回来，否则 `prlctl exec` 通道永久丢失。
- 之后轮询 `prlctl exec ... echo CH_OK` 判断通道是否恢复。

## 相关：真实 C: 备份第一次尝试的失败点

任务 `83bb9580`，DISM 捕获到 **36% 时中断**，正是宿主磁盘被撑满的时刻
（`F:\c-real.wim.partial` 停在 0 字节、`Recovery.log` 进度条停在 36%）。
注意：WinRE 里盘符会重排，日志里写的是 `ImageFile:H:\c-real.wim.partial`，
那个 H: 在 Windows 里就是 F:（镜像卷）——不是写错位置。
