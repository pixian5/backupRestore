# 04 恢复环境与载荷

## 1. 载荷契约

- 恢复环境 shell 契约**内嵌在 Rust**（`winre_payload.rs` 的 `EXPECTED_WINRE_SHELL`）：只允许 `%SYSTEMROOT%\System32\Recovery.exe,recover-env %SYSTEMROOT%\System32\RecoveryTask.env`，准备时直接生成 `payload\winpeshl.ini`，不依赖外部模板；模板内容由构建前断言守护 —— 20260916-2105 [✅ 源码]
- 文件映射唯一运行时来源，每次复制做 SHA-256 核对，注入后再核对模板+全部必需文件，缺任一即失败且 DISM 不提交镜像 —— 20260916-2105 [✅]
- **每个 LaunchApps 程序都必须同步进注入清单**（历史两次因改入口不改清单而"DISM 注入成功、恢复环境起不来回 Windows"）；同一二进制两个文件名：`BackupRestore.exe` == `Recovery.exe`，部署必须同时 copy 成 Recovery.exe —— 20260913-0505 [✅]
- 旧 `BackupRestore.exe`/`RecoveryLauncher.cmd`/`winpeshl-boot.cmd` 会从任务镜像删除（`OBSOLETE_WINRE_FILES`）；产品恢复环境不再携带 .cmd 启动器 —— 20260916-2105 [✅]
- 载荷目录 `BackupRestoreRE\<task-id>\Winre.wim` **按任务 ID 隔离**（历史上所有任务共用一目录，准备第二个任务覆盖第一个载荷导致 rearm 哈希不符放弃续跑）；清理只删本任务子目录 + `prune_empty_parent` —— 20261010-1030 [✅]
- `BackupRestoreRE` 只是相对"运行时选定的镜像卷"的目录名，**代码没写死任何卷符**（注释里的 `F:\` 仅举例）—— 20261013-2356 [✅]
- **静态 CRT 是硬要求**：恢复环境/预安装环境/精简 Windows 无 VCRUNTIME140/UCRT，动态 CRT 产物进程创建即被加载器终止，症状=**零输出、零副作用、无日志无弹窗**，退出码 `4294967418`=`0xC0000135`=STATUS_DLL_NOT_FOUND；三步判断法：看退出码→解析预安装环境导入表→加 `+crt-static` 重构建比对 —— 20260925-0207 [✅]
- Windows servicing 会**静默替换注册 Winre.wim**（当天部署的载荷被累积更新冲掉为官方原版）→ 注册恢复环境不是可靠载荷宿主；这也是本项目载荷放任务自有启动配置数据条目的原因 —— 20260927-2015 [✅]
- "返回 Windows"（exit_pe_to_windows）只恢复/设置启动配置数据默认项、清一次性启动并重启，**不还原原始 Winre.wim** —— 20260925-1940 [✅]

## 2. 恢复环境注册机制（每系统一份，硬规则）

- **每个系统各注册一份自己的恢复环境，不能共用**：`ReAgent.xml` 的 `OsBuildVersion` 把恢复环境与系统版本硬绑定；启动配置数据恢复项按「分区身份 + 对象 GUID」定位 —— 20261012-1520 [✅]
- ReAgent 校验的真实对象 = **被登记的那个启动配置数据对象 + `reagentc /boottore` 写下的 bootstatus**，不是任意启动配置数据条目路径；自建条目指向镜像卷副本可成功进预安装环境；路径不一致的代价 = `/boottore` 返回 RC=2、bootstatus 不设、恢复环境被拉起也**静默自退** —— 20261011-2350 汇编（源自 09-27→09-29 修正）[⚠️ 单一权威转述]
- 注册位只能时间分片：`C:\Recovery\WindowsRE\Winre.wim` 同文件两互斥角色（载荷宿主 / 捕获纯净）；三条不变量 A 存活期必须注入件、B 捕获瞬间必须干净且**硬失败**、C 终态必须干净 —— 20261011-2350 [🔸 机理可信、单源]
- reagentc 顺序约束（恢复环境内对离线目标）：`/disable` rc=50（预安装环境不支持）、对 Enabled 目标 `/setreimage` rc=183、`/enable` 没有 `/target` 只有 `/osguid` → **顺序只能「先 setreimage（Disabled 态）→ 后 enable」**；`/setreimage` 的 `/target` 必须是正在运行的操作系统的 Windows 目录；`/setreimage` 是**移动**不是复制 —— 20260927-2015 §9.5.2 [✅]
- 预安装环境里 `reagentc /disable` rc=50「Windows 预安装环境中不支持此命令」——纯 WinPE 走 create-secondary 必须被**擦除前预检**拒绝（否则"目标卷已格式化+镜像已应用后才失败"）；预检双层：DISM 只读挂载探测（权威）不可用时降级镜像名启发式；**本机有 VHD 附加时 `DISM /Mount-Wim` 一律报 87** —— 20261013-1036/0815/0340 [✅]
- `winre_registration` preflight 要求目标有 `reagentc.exe` + SYSTEM hive，缺则报「目标缺少离线注册组件」——**代价是失败发生在擦除应用之后**（优先级 5：应前移到擦除前预检，**至今未修**）—— 20261011-2140/1216 [✅ 源码仍在]

## 3. 恢复环境内环境事实

- 恢复环境里 `%SystemDrive%` 是恢复环境自己的内存盘 `X:`；盘符会重排（桌面 T:/P:/H: → 恢复环境 D:/F:/G:）；有 `bcdedit`/`dism`，**没有 `reagentc`、没有 `whoami`**（rc=9009）—— 20260927-2015 §9.2 [✅]
- **活动系统卷绝不能做文件探测**：`C:\Windows\System32\config\SYSTEM` 被系统独占锁定，`pe_safety::probe_file` 对它是共享违例 os error 32（不是 NotFound）→ 会让整个 prepare 失败；修法 `target_is_active ||` 短路 —— 20261014-1052 [✅]
- **恢复环境内没有 reagentc.exe**，操作注册位必须全路径调离线目标操作系统自带的 `<目标>:\Windows\System32\reagentc.exe` —— 20261014-1216 [✅]
- 恢复环境窗口只有约 20 秒，宿主侧截图来不及也不必；**日志证据链比截图更强** —— 20261014-0200 [✅]
- 恢复环境日志分工：主日志在**镜像同目录** `H:\BRImages\Recovery.log`（追加式、含历史 run，排障第一站）；任务目录有 `prepare.log` / `Recovery-early.log`（只到挂载）/ `status.json`；`C:\BackupRestore-Recovery-early.log` 是历史累积别当唯一来源 —— 20261014-0200/1216 [✅]
- 判"是不是新通道"的终态指纹：旧通道 `WinRE cleanup completed`，新通道 `Boot entry cleaned` / `new boot channel: BCD objects removed` / `staging directory \\?\Volume{…}\BackupRestoreRE removed and verified` —— 20260930-0030 [✅]
- 探针纪律：`probe` 成功 ≠ 备份还原成功（只证明准备链+入口+清理）；Capture/Apply/格式化/BCDBoot/第二系统启动必须各自单独留证；`prepare --no-reboot` 不是无副作用预览（已建启动配置数据对象、已拷贝/挂载镜像，只是不 arm 不重启）—— 20260925-0025/20260930-1925 [✅]
- 探针/错注入方法：`--test-efi-drive`（隔离 EFI 临时挂载）与 `--test-fault`（`identity-env-mismatch`/`bcdboot-failure`/`power-loss-*`）仅开发命令行暴露；`--test-fault none` 不被接受；持续型故障（acceptance:apply:error）在恢复环境内**无法解除**（无 Tools 写不了 fault-released）→ 改用 `error-once` —— 20260819-impl-notes/20261010-1910 [✅]
- ⚠️ `.fault-<name>.triggered` 标记**按日志路径定位**而 `Recovery.log` 跨任务共享 → 同一故障一生只能触发一次，重测会静默不触发、差点被误判成"修好了"；**至今未修**，重测前手工删 —— 20260930-0320 [🔸 源码 main.rs 仍 log-path-based]
- 镜像时点状态会被还原：备份时 C: 的恢复环境注册本就 Disabled，还原后也 Disabled，新通道把注册镜像当只读资产来源 → prepare 直接失败；属固有代价不是代码缺陷 —— 20260930-0230 [✅]

## 4. 救援工具箱（虚拟机起不来/恢复环境卡死时）

- **`0xc000000e`/引导劫持三条救命命令**（预安装环境/恢复环境命令行逐条执行，每条先截图核对再回车）：
  `bcdedit /deletevalue {bootmgr} bootsequence` → `bcdedit /set {bootmgr} default <主系统GUID>` → `wpeutil reboot` —— 20261014-2244/2315 [✅ 实测走通]
- 预安装环境命令行能拿硬证据就别猜：`bcdedit /enum {bootmgr} /v` 直接看 default/bootsequence 实况 —— 20261014-2315 [✅]
- `prlctl send-key-event -s <十进制scancode>` 可向恢复环境/预安装环境全阶段注入键盘；Shift 组合需 press→sleep→key→sleep→release；实测 `-s 75`(K) 在恢复环境进度窗前弹出 cmd —— 20261014-2159 [✅]
- bootmgr「按任意键」阶段需多键×多次连发才响应；`prlctl send-key-event` 在 bootmgr/固件阶段完全无效，Boot Manager 菜单交互自动化仅剩 VNC/USB HID —— 20260909-2145/20261014-1900 [⚠️ 具体 scancode 映射用前先打条短命令截图核对]
- 预安装环境脚本光盘（效率远高于逐键）：宿主 `hdiutil makehybrid -iso -joliet` 造含 cmd 的 ISO → `--device-add cdrom` 挂第二光驱 → 预安装环境里 `for %p in (D E F G) do %p:\find.cmd` 执行 —— 20261014-1900/2244 [🔸 同一作者两篇、未交叉]
- `--device-bootorder "cdrom0"` 单设备形式有效；config.pvs 的 BootingOrder 对 UEFI 无效 —— 20261014-1900 [🔸 单源]
- 恢复环境/启动卡住**绝不反复强停重启**（触发系统还原挂起等次生灾害），正确动作=菜单「关机」后从宿主侧救援 —— 20261014-2107/1900 [✅ 多来源]
- 失败的恢复环境运行会重新武装一次性启动项形成恢复环境循环——**快照回滚是唯一干净脱身法**（启动配置数据三行回滚覆盖不了循环状态）—— 20261014-2105 [✅]
- 虚拟机引导变量损坏（Boot Manager 只剩 UEFI Shell）最快修法：新建虚拟机挂旧盘（`--device-add hdd --image <旧.pvm/harddisk.hdd>`），干净固件扫 ESP 直接引导；PD27 时代删 NVRAM 重建无效、26.4.2 下有效（与固件版本绑定）—— 20260913-0158/20260925-0424 [✅]

## 5. 恢复环境内 cmd「后台最大化」Win32 铁律与日志镜像（2026-10-20 实机确立）

- **★ `SW_SHOWMAXIMIZED`(3) 会激活窗口**。`CreateProcessW` + `STARTF_USESHOWWINDOW/SW_SHOWMAXIMIZED` 创建 cmd → 新窗口一起手就抢前台并压在**随后**创建的进度窗之上；进度窗侧 `MB_SETFOREGROUND` 只保得住模态框，普通主窗抢不过被激活的 cmd（用户 2026-10-17 需求三次实机失败的根因）—— 20261020-0230 [✅]
- **「创建时不抢、就绪后最大化且留后台」三步走**：①创建用 `SW_SHOWMINNOACTIVE`(7)；②按 PID 轮询 `EnumWindows` 等**顶层窗口就绪**（进程起来 ≠ 窗口就绪，conhost 初始化几十~几百 ms，直接 ShowWindow 会打空句柄）；③`ShowWindow(SW_SHOWMAXIMIZED)` + 立即 `SetWindowPos(HWND_BOTTOM, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE)` 压底 —— 20261020-0230 [✅]
- **非前台进程 `SetForegroundWindow` 会被系统静默拒绝**；进度窗显式抢前台必须 `AttachThreadInput(前台线程, 本线程, 1)` → `SetForegroundWindow` → `AttachThreadInput(..., 0)` → `SetFocus`（`recovery_progress::claim_foreground`）—— 20261020-0230 [✅]
- **同 crate 多个 `unsafe extern` 块声明同一符号，签名必须逐字一致**（含 `Option<fn>` 包装），否则 `clashing_extern_declarations`；本 crate 权威签名在 `native_gui.rs` 的 user32 块。另：只被 `#[cfg(test)]` 用的函数要挂 `#[cfg(test)]`（release 构建算死代码）—— 20261020-0230 [✅]
- **恢复环境内主日志在内存盘 X: 重启即失**：`session_log.rs` 双写镜像到第一个可写持久卷（选根「卷 GUID → 同卷子目录 → X:」三级），会话目录名 = `pid-纳秒时间戳`。恢复环境取证 = 找持久卷上的镜像副本，别在 X: 里找 —— 20261020-0230/20261017-0125 [✅]
- **B 键触发链**：`recovery_progress.rs` 消息循环 `WM_KEYDOWN → VK_B → trigger_bcd_rollback(hwnd, shared, true)`，受 `main_family` 焦点门（`msg_hwnd == hwnd || GetParent(msg_hwnd) == hwnd`）限制——**cmd 持焦点时按 B 无效**，这就是 cmd 后台化是 B 键验收硬前置的原因。`vmkey.sh b` 发一次 press+release 即触发（WM_KEYDOWN 单次即触发，不要按住）—— 20261019-1918/20261020-0230 [✅]
- **`--test-fault power-loss-window`** = prepare 的武装不重启故障点：跑完「武装 + BootRequested 落盘」后返回、不调 shutdown.exe——控制重启时机的正确入口（`prepare` 不带 `--no-reboot` 会在 arm 后立即 `shutdown /r /t 0`）—— 20261020 [✅]

## 6. 载荷生命周期与命令行续跑参数（2026-10-22 实机确立）

- **★ 载荷在任务存活期不会被自动删**（对照实验反证）：`remove_staging` 全仓只有 `boot_entry.rs:799` 一处调用（**终态清理**）。实测 prepare 06:28:04 完成 → 06:28:04.09 即记录 `PAYLOAD_OK size=714,894,729` → 后台监控每 3.1 s 采样持续 `PAYLOAD_PRESENT` → 06:33:59 任务 success 后才 `PAYLOAD_GONE`。**上一轮「prepare 后 3 分钟载荷离奇消失」不是代码行为**，更可能是外部因素（手动清理/其它进程），不必再在代码里找死循环 —— 20261022-0655 [✅]
- **★ 命令行显式续跑的正确姿势**：`prepare … --no-reboot` 后**立刻**（秒级）`resume <工作区根> <task_id>`；`resume` 内部做的事 = `BootRequested` 落盘 → `rearm`（含载荷 SHA256 校验，文件没了就 `os error 3`）→ `shutdown /r /t 0`。全程 1.8 秒，比重跑 prepare 稳 —— 20261022-0655 [✅]
- **★ `abandon-task` / `resume` / `recover` 的 root 参数都是「工作区根」**，不是 `…\tasks`：`TaskStore::task_dir` = `root.join("tasks").join(id)`，传 `H:\brwork\tasks` 会拼成 `H:\brwork\tasks\tasks\<id>` 而报错。正确：`abandon-task H:\brwork <id>` —— 20261022-0655 [✅]
- **一次性启动项不武装也能进自定义恢复环境**，走 `resume` 即可；`--no-reboot` 只是「不自动重启」，它**同时不武装 bootsequence**（日志明示 `Task prepared with --no-reboot; registered WinRE unchanged`），所以必须再 `resume` 或手工 `bcdedit /bootsequence` —— 20261022-0655 [✅]
- **恢复环境里的盘符映射与 Windows 侧完全不同**：本轮预安装环境内 `WORKSPACE=E:` / `IMAGE=C:` / `TARGET=F:` / `EFI=Z:`，日志里出现的 `E:\brwork\tasks\…` 就是 Windows 侧的 `H:\brwork\tasks\…`，**别当成两个任务**。恢复环境主日志也因此落在「镜像同目录」（预安装环境的 `C:\BRImages` = Windows 的 `H:\BRImages`）—— 20261022-0655 [✅]
