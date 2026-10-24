# 不再往 C: 部署 exe + 自启项改指 H:\brwork（用户 2026-10-24 拍板）

- 撰写时间：2026-10-24 09:58
- 模型：Step-5-Preview
- 用户指令原文：「不再往 C: 部署 exe」（承接同日「TaskStore 搬离 C:」与「证据全留、载荷副本可清」两项拍板）
- 版本：**v2.6.32 不变**（本轮只改部署脚本口径 + 实机自启项，无产品代码改动）

## 一、为什么不再往 C: 部署 exe

「镜像带回旧状态」效应（10-22 V1、10-24 A 入口回归两度实测）：备份 C: 会把 C: 上**所有东西**打进 WIM，A 入口还原 C: 时整个 C: 被重写回「拍镜像那一刻」。历史上 `C:\Users\Public\backupRestore-package` 是部署位之一，于是：

1. **二进制回滚**：还原后 C: 包目录里的 exe 退回镜像拍摄时版本（10-24 07:5x 实测：自启实例 v2.6.31，而 `H:\brwork` 里是 v2.6.32）——每次还原后必须人工重署。
2. **任务状态回滚**（已于同日修掉）：`tasks` 曾在 C: 包目录，被一起打进镜像，还原后镜像里「可续跑态」的旧任务复活成弹框。tasks 迁到 `H:\brwork\tasks` 后此路已断。
3. **自启链路指错**：还原后自启的 GUI 走的正是 C: 包目录那份旧版。

「不再往 C: 部署 exe」从源头消灭 1 和 3 的物质基础：C: 上没有程序，镜像就带不回旧程序。

## 二、依赖面核查（只读，确认删 C: 不断链）

改动前逐条核查「谁依赖 C: 包目录」：

| 链路 | 是否依赖 | 证据 |
|---|---|---|
| Rust 代码路径解析 | **否** | `grep -rn "backupRestore-package\|package_dir\|package_root" crates/` 零命中（仅 `log_root.rs` 一句注释）；一切路径基于 `current_exe()` / `executable_dir` |
| WinRE 载荷暂存 | **否** | `winre_payload.rs:159` `winpeshl.ini` 由 `EXPECTED_WINRE_SHELL` 常量**内置生成**；`:161` `Recovery.exe` 取 `resolve_recovery_payload_source(executable_dir, current_exe)`，sibling 缺失时自动回退用正在运行的 exe 并告警；wimlib 两件套从 `executable_dir` 暂存（`H:\brwork` 已有） |
| wimlib 定位 | **否** | `wimlib_apply::locate_wimlib` 按 `executable_dir` 找 |
| PE 镜像构建（POC） | **否** | `poc/build-backuprestore-pe.ps1:43-48` 只探测 `C:\BackupRestorePE\package` 与 `C:\BackupRestoreBuild\package`（发布目录），与 C: 包目录无关 |
| PE 任务执行引擎 | **否** | `pe_operation.rs` 执行体不取包目录任何文件 |
| 自启 GUI | **是** | 用户 x 的 `HKCU\Run :: BackupRestoreTest = C:\Users\Public\backupRestore-package\BackupRestore.exe`（本轮改向） |
| `build-win.sh` 部署 | **是** | 原部署 C: 包目录五件（本轮去掉） |
| `windows/build-windows.ps1` 部署 | **是** | `$DeployPackageDir` 默认 C: 包目录（本轮去掉） |

## 三、★ 自启项真相（修正 07:55 文档的误判）

07:55 A 入口回归文档把「还原后自启 GUI 走 C: 包目录」归因为**程序内部行为**——**这个结论错了**。真相：

- 自启项是用户 x 注册表里的 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run :: BackupRestoreTest`，值为 `C:\Users\Public\backupRestore-package\BackupRestore.exe`。
- 它位于用户 hive（NTUSER.DAT，在 C: 上），**随备份进镜像、随还原复活**，属于「镜像带回」而非程序行为。
- 此前两轮取证都漏掉它，是因为查询用 `prlctl exec`（**SYSTEM 身份**）：SYSTEM 的 `HKCU:` 解析到 `HKEY_USERS\.Default`，看不到真实登录用户 x 的 hive。正确做法是枚举 `HKEY_USERS\S-1-5-21-*`。
- 其他位置均已排查干净：`Winlogon\Shell=explorer.exe`（标准值）、HKLM/HKCU Run 无 BR 项、启动文件夹空、计划任务无 BR 项（`rbrun*` 是 9-28 测试残留，跑 `C:\Users\Public\pkg\rb*.cmd`，最后运行 09/28）、251 个计划任务里无 BackupRestore。

## 四、改动清单

### 1. `build-win.sh`（宿主机交叉编译部署脚本）

- 去掉 C: 包目录的 mkdir、5 个 copy、4 条 SHA 采集；八联 SHA 校验收缩为 **H: 四联**。
- 新增步骤 2：`rmdir /s /q C:\Users\Public\backupRestore-package`（清历史残留，良性失败用 `exit /b 0` 收尾）——今后任何机器上跑 `--deploy` 都不会在 C: 留程序。
- `winpe-winpeshl.ini` 改为随 `H:\brwork` 部署（唯一部署位文件要成套）。
- 头部注释重写为「部署目标唯一 = H:\brwork」。
- 标签仍守无冒号铁律（`唯一部署位(H-brwork)`）。

### 2. `windows/build-windows.ps1`（VM 内构建部署脚本）

- 删除 `$DeployPackageDir` 参数；`Publish-Build` 去掉 `$PackageDir`，只留 `$WorkDir`。
- 原「清包目录历史启动件」循环改为 `Remove-Item C:\Users\Public\backupRestore-package -Recurse`（清残留）。
- targets 由 4 项缩为 2 项；wimlib 循环由两目录缩为单目录；`winpe-winpeshl.ini` 改随 WorkDir。
- PowerShell 解析校验通过（`Parser::ParseFile` → PARSE OK）。

### 3. 实机动作

| 动作 | 结果 |
|---|---|
| 自启项改向 | `BackupRestoreTest`：`C:\Users\Public\backupRestore-package\BackupRestore.exe` → **`H:\brwork\BackupRestore.exe`**（`RUNKEY_FIXED`） |
| `./build-win.sh --deploy` | 通过：`DEPLOYED SHA256=e79490f5… wimlib=fddb8c21…/b34549c6…`，四联 SHA 全对（`Finished in 0.10s` = 代码未变复用产物） |
| C: 包目录 | **已不存在**（`C_PKG_EXISTS=False`，脚本 rmdir 生效） |
| H: 五件套 | 全齐：`BackupRestore.exe`/`Recovery.exe`（各 2,877,440 B `e79490f5…`）、`wimlib-imagex.exe`（125,440 B `fddb8c21…`）、`libwim-15.dll`（432,640 B `b34549c6…`）、`winpe-winpeshl.ini`（61 B，内容 = `[LaunchApps]` + `%SYSTEMROOT%\System32\Recovery.exe,--pe-desktop`） |
| 删 `C:\Users\Public\BackupRestore.exe` | 10-11 散落旧 exe（2,308,096 B，无附属证据）已删 |
| 重启 VM 验证自启 | `prlctl restart` → 自启进程 `PID=5092 PATH=H:\brwork\BackupRestore.exe`，标题 v2.6.32、无模态框、无错误；EventID 1000（近 1h）=0、WerFault=0 |

## 五、已知遗留（诚实声明）

1. **`C:\Users\Public\BackupRestore-actual-20261005-0724`**（10-05 历史测试存档，64 文件 / 6.4 MB，含 logs/release-source/release-restored）**未删**——证据性质，按「证据全留」精神保留，是否清理待用户拍板。
2. **旧镜像仍带回 C: 包目录**：`br-2629-20261023.wim` 拍摄时 C: 上还有包目录（含 tasks），用它还原 C: 会重新出现该目录。但自启项已改指 H:，还原后自启走 H: 新版；下次 `--deploy` 的 rmdir 会清掉。**新拍镜像**（C: 已无包目录）不再带回。要彻底消除需重拍镜像（37.8 GB，用户未要求，未做）。
3. **备份排除表未改**：`build_capture_exclusions`（core/lib.rs:1631）不含程序目录，备份 C: 仍会把 `C:\Users\Public\BackupRestore-actual-20261005-0724` 等散件打进镜像。收益只剩「镜像更干净」，且会改变「还原后 C: 上有什么」的现状，需一次 A 入口回归，本轮未做。
4. **A 入口还原 C: 全量回归未重跑**：本轮改动不碰还原链路（TaskStore 机制、WinRE 交接均未变），07:55 的回归结论仍成立；「自启走 H:」已由重启实测单独验证。

## 六、证据

- `.test-artifacts/recon-38-lite-20261024/`：`sid.ps1`/`fixrun.ps1`（自启项取证与修复）、`verify-deploy.ps1`（部署后五件套核验）、`post-reboot.ps1`（重启后自启路径）、`pub.ps1`（C:\Users\Public 残留盘点）、`winlogon.ps1`/`winlogon2.ps1`（Winlogon 排查）、`hku.ps1`（全用户 Run 键扫描，抓到 BackupRestoreTest 的那份）、`vm-post-reboot.png`（自启 GUI 截图）、`vm-post-deploy.png`、`a2-progress.png`/`a5-final-gui.png`（07:55 回归截图）。
- 本轮 commit：见文末 git log。
