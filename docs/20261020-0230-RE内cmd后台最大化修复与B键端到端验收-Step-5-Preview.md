# RE 内「cmd 后台最大化」根因修复 + B 键端到端实机验收（v2.6.21）

- **撰写时间**：2026-10-20 02:30（local）
- **模型**：Step-5-Preview
- **版本**：v2.6.21（源码内修复，未 bump；已部署并验收的二进制即 v2.6.21 + 本修复，
  2026-10-20 01:29 构建，`BackupRestore.exe` 2,862,080 B，载荷 `Recovery.exe` 同为 2,862,080 B）
- **性质**：缺陷修复（用户 2026-10-17 亲自提的需求此前三次实机失败）+ 端到端验收
- **前置**：`d6fb27d RE 内最大化后台命令控制台`（2026-10-17）；
  `20261018-2252 B 键 BCD 回退 import 毒化缺陷修复与 run3/run4 双路径实机验收`

## 1. 用户需求与三次失败史

用户 2026-10-17 明确要求：**进 WinRE 后自动打开的 cmd 必须「最大化 + 在后台」，
本程序（恢复进度窗）保持前台**。2026-10-19/20 三次实机（任务 2355190e / 6bf2ff22 /
7924c644）该需求全部落空：cmd 盖住进度窗，B/L/M/K/P 键全部打在控制台上，B 键测试三连败。

## 2. 根因：`SW_SHOWMAXIMIZED` 会激活窗口

旧实现用 `STARTF_USESHOWWINDOW + SW_SHOWMAXIMIZED` 创建 cmd。按 Win32 文档，
`SW_SHOWMAXIMIZED` **会激活窗口**——cmd 一起手就抢走前台，并且压在随后才创建的
进度窗之上。进度窗侧的 `MB_SETFOREGROUND` 只保得住模态框，普通主窗抢不过被激活的 cmd。

## 3. 修复内容（三个文件）

### 3.1 `main.rs::launch_background_console`：创建不抢前台，就绪后再最大化压底

- 创建标志由 `SW_SHOWMAXIMIZED(3)` 改为 **`SW_SHOWMINNOACTIVE(7)`**（最小化且**不激活**）；
- 新增 `wait_for_toplevel_window(pid, 3s)`：进程级静态 `TARGET_PID`/`FOUND_HWND`
  （AtomicU32/AtomicUsize）+ `enum_callback`（IsWindowVisible && PID 匹配即命中）+
  50ms 轮询 `EnumWindows`。**进程起来 ≠ 窗口就绪**（conhost 初始化要几十~几百 ms），
  直接 `ShowWindow` 会打在空句柄上；
- 窗口就绪后 `ShowWindow(SW_SHOWMAXIMIZED)` 放大 → 立即
  `SetWindowPos(HWND_BOTTOM, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE)` 压到 z 序最底层；
- 等不到窗口返回错误（不再静默）：控制台是退路，进程已活着，最坏是最小化。

### 3.2 `recovery_progress.rs::run_window`：进度窗显式抢回前台

新增 `claim_foreground(hwnd)`：非前台进程直接 `SetForegroundWindow` 会被系统静默拒绝，
先 `AttachThreadInput(前台线程, 本线程, 1)` 挂输入队列 → `SetForegroundWindow` →
`AttachThreadInput(..., 0)` 解挂 → `SetFocus`。窗口创建后立即调用——
「这里才是用户按 B 键的地方」。

### 3.3 构建告警清零（顺手修，本项目标准 0 警告）

- `main.rs` 的 `EnumWindows` 声明与 `native_gui.rs` 同 crate 双声明签名不一致
  （裸 `fn` vs `Option<fn>`）→ 对齐为 `Option<unsafe extern "system" fn(...)>`，
  消除 `clashing_extern_declarations`；
- `winre_registration.rs` 的 `at_location`/`verify` 只被 `#[cfg(test)]` 单测使用
  （b1a7f74 v2.6.20 重构遗留）→ 挂 `#[cfg(test)]`，消除 release 死代码告警。

验证：macOS 交叉编译干净（仅存量 LNK4099 PDB 告警）；macOS 单测 208 + 44 全过；
VM 原生构建 0 警告、BUILD_EXIT=0。

## 4. 端到端验收（任务 771b3ee2，2026-10-20 01:42–02:01）

### 4.1 部署与武装

- `H:\brwork\BackupRestore.exe` ← 新构建（2,862,080 B）；prepare 日志确认载荷取自
  **运行中的 exe**：`payload Recovery.exe is stale: ... != running executable ...;
  staging the running executable instead`；
- `prepare --operation create-secondary --source-drive C --target-drive F
  --image-path H:\BRImages\br-2619-cur.wim --wim-index 1
  --boot-menu-name "Windows BKeyTest" --allow-destructive --test-fault power-loss-window`
  → stage=boot-requested，bootsequence `{9f59a070-cbe4-11f1-8997-bc3df26eeae9}` 已设；
- 载荷目录 `Recovery.exe` = 2,862,080 B（新二进制进载荷，实证）。

### 4.2 进 WinRE：cmd 后台最大化 + 进度窗前台（用户需求，首次实证）

重启后第一张有效截图（01:50:42，`captures/arm3-01-boot.png`）：

- **进度窗「BackupRestore 恢复进度」在前台居中**：STEP 2/4 还原镜像、已用 00:55、
  五按钮（详细日志(L)/引导修复(M)/命令提示符(K)/暂停(P)/BCD 回退(B)）全部可见；
- **cmd「管理员: BackupRestore recovery console」最大化的窗框在屏幕最上方、
  位于进度窗背后**——「最大化 + 后台 + 程序前台」三项同时成立，用户需求闭环。

### 4.3 按 B 键：回退成功（01:51:23）

`vmkey.sh b` 命中进度窗，日志三行（镜像 `Recovery.log`，UTF-8）：

```
[2026-10-19T17:51:23.132401200+00:00] plan A: verbatim BCD store path = \\?\Volume{d08d796f-...}\EFI\Microsoft\Boot\BCD
[2026-10-19T17:51:23.145902700+00:00] Restored byte-for-byte EFI BCD snapshot
[2026-10-19T17:51:23.145999600+00:00] 用户按 B 键 BCD 回退：已按字节恢复准备期 BCD 快照
```

模态框（`captures/arm3-02-afterB.png`）：「BCD 回退已完成: ESP 上的 BCD 已恢复为任务
准备期保存的快照。」；关Modal后恢复主流程继续推进（STEP 2 应用期不打断 DISM，
设计如此：中断 apply 只会留下半写的 F:）。

### 4.4 回退后行为：BootRepaired 校验失败 → 取消 → 终态 + 重启回原系统

- STEP 2 完成、STEP 3 校验完成后，STEP 4 引导修复（BootRepaired）按方案重跑：
  BCD 已被用户回退（loader 条目 `{9f59a070}` 不存在），`rearm()` 前置校验
  `verify_chain` 判 `device` 不符 → `invalid task: 续跑启动链字段不符: device`
  → 失败框（重试/取消，累计 3 次自动逃出）；
- 取消 → disarm 尽力 + 任务标终态 + 重启回正常 Windows。**这是"用户自行回退 BCD"
  的合理后果**：恢复无法把引导写回一个用户主动还原掉的 BCD 上；
  auto-escape 闭环（disarm + 标终态 + 重启）工作正常；
- 终态实证：
  - `status.json`: stage=failed（error: 恢复在恢复环境中连续失败，已自动逃出并标记为终态，
    任务与现场保留）；
  - BCD：**bootsequence 已消失**、**loader `{9f59a070}` 已不存在**（"没有匹配的对象或存储为空"）、
    default = `{c46f6056-...}`（原系统）；
  - 镜像卷载荷 WIM 已删（PAYLOAD_WIM_GONE）；task 目录证据全留（bcd-before-raw、
    boot-entry.json、Recovery-early.log 等）；
  - VM 重启后回到原 C: 系统桌面——**用户回退意图被完整兑现**。

### 4.5 证据位置（重要：WinRE 主日志在内存盘，镜像才持久）

- WinRE 主日志路径在 X:（内存盘），**重启即失**；`session_log.rs` 的镜像机制把它
  落到第一个可写持久卷：本次为
  `C:\BackupRestore-logs\1532-1792432193341658400\`（会话目录名 = pid-纳秒时间戳，
  17:49:53 UTC 启动即本次运行）：
  `Recovery.log`（126,741 B，含 B 键三行关键证据）、`Recovery-early.log`、
  `BackupRestore-escape.log`（2,236 B，逃出决策记录）；
- 共享盘证据：`.test-artifacts/recon-20261019/` 下 `arm3-mirror-Recovery.log`、
  `arm3-Recovery-early.log`、`arm3-prepare.log`、`arm3-task.json`、
  `arm3-boot-menu-plan.json`、`captures/arm3-0[1-6]-*.png`、`final-collect.txt`、
  `bcd-endstate.txt`、`bcd-end-bootmgr.txt`、`bcd-end-osloader.txt`。

## 5. 结论

1. **用户 2026-10-17 的「cmd 后台最大化」需求首次真正成立**（截图 + B 键命中双重实证）；
2. **B 键端到端全通**：按键 → 日志行 → 模态框 → BCD 终态干净（bootsequence/loader
   条目消失、default 回原系统）→ 任务终态、证据保留、机器回原系统；
3. B 键回退后的失败框是**用户主动回退 BCD 的合理后续**，不是新缺陷；auto-escape
   收闭环有效；
4. 附带清零 3 个构建告警，`main.rs`/`recovery_progress.rs`/`winre_registration.rs`
   三文件改动共 +174/-4。

## 6. 本轮新固化环境坑（已写入项目记忆）

- `/usr/local/bin/prlctl`（prlsrvctl 同）是坏 bash 包装器：一切 prlctl 调用
  （含后台任务）先 `export PATH="/tmp/prlbin:$PATH"`；
- `powershell -File` 不剥离单引号：`-CargoTargetDir 'C:\...'` 会让 `GetFullPath`
  抛 NotSupportedException，构建秒退 BUILD_EXIT=1——投给 VM 的参数一律双引号；
- 投进 VM 的 `.cmd` 一律纯 ASCII（UTF-8 中文 search pattern 会被 cmd 按 GBK 解析，
  引号配对破坏 → 整脚本静默不执行）；
- zsh 双引号里 UNC 路径写四个反斜杠；SYSTEM 通道（无 `--current-user`）能跑
  bcdedit 但看不到 X:，交互会话相反。
