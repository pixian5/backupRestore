# BackupRestore

Windows 一键系统备份还原工具（**开发测试版**）。

当前系统卷选择「进入 Windows 恢复环境」后，程序准备独立恢复镜像副本与任务启动项 → 重启进入恢复环境 →
自动拉起 `Recovery.exe` 离线捕获/应用镜像 → 系统还原时修复引导 → 清理临时启动配置 → 重启回 Windows。
非当前系统卷由界面在线执行；预装预安装环境是另一条简化路径，能力差异见完整流程文档。

当前仓库版本 **2.6.44**（2026-10-25 01:25 时点）：**修 RE 主链 DISM 根本没有超时——`--dism-timeout-secs` 此前形同虚设**（[记录 2026-10-25 01:25](./docs/20261025-0125-RE主链DISM超时形同虚设修复与降级实证-DeepSeek-V4.1-Flash.md)）。为补旧待办「RE 主链降级未单独实机验证」而真跑了一次：`--dism-timeout-secs 1` + 38GB 镜像 → **修前 6 分钟正常完成、日志零降级行**。根因：RE apply 走的 `run_logged` 里是无条件 `child.wait()`，`dism_timeout::wait_ms()` 从没被传进去——上限只被"记录"没被"使用"。后果两层：① 用户以为配了逃生阀，卡死照样卡死；② 没有 TimedOut 失败 → `classify_apply_failure` 归不到该类 → **D1 的 wimlib 降级在 RE 主链永不触发**。修法：`run_logged` → `run_logged_timeout(..., wait_ms)` + 新增 `wait_child`（`u32::MAX` = 等到底，其它 11 个调用点零回归；超时 kill 后仍 join 读线程保住输出尾部）。实证（同命令修后）：`[ERROR] 分类 TimedOut` → `[WARN] 自动降级 wimlib` → wimlib 79GB/4 分 49 秒 → 退出码 0 → **success/100%**（任务 `49a99148`），RE 进度窗同步显示 `正在展开系统文件 24 GiB of 79 GiB`。★ **静态审查发现不了「配置读了但没接线」**，得用极端参数跑一次。现场已恢复：C: 249GB 原系统、default 指回 `{af5ee73f}`。此前 01:10 时点：**日志中文硬编码 97 → 0、i18n 表 15 → 124 条，B6 与 CLI 语言来源一并结清**（[记录 2026-10-25 01:10](./docs/20261025-0110-日志全量多语言化与B6目录命名分叉-DeepSeek-V4.1-Flash.md)）。① 日志：失败因果六类 + wimlib 降级、RE 进度窗（暂停/继续/逃出/BCD 回退/命令提示符）、STEP 1-4、在线与 PE 主链、引导修复方案、盘符解析、副档发布全部双语。★ **统计口径踩坑**：第一遍只数 `append_log` 得出「97 → 0」，**实机读日志仍有中文**——`online_operation` / `pe_operation` 各自有内部 `log()`（41 处），补完才真到 0；**说「全量」前先数清有几种写日志的方式**。★ 翻译前先扫 83 处中文判据逐类核实（STEP 解析只认 `STEP ` 前缀与数字、`bcdedit` 解析的是系统本地化输出、失败分类只认 DISM/wimlib 原文），`cause_*` 的英文措辞刻意避开分类模式整串（如 `access is denied`），防 Busy 被认成 AccessDenied。② **B6 结清**：备份任务目录不再叫 `BackupRestore-online-*`，`resolve_dir(operation)` 按白名单分叉（不认识回落 `online`，绝不把外部串拼进路径），实机目录 `BackupRestore-restore-*`。③ **CLI 也读界面语言**：`BACKUPRESTORE_LANG` > ini `ui_language` > 中文；实机在线日志整段英文（`STEP 1/4 Validating parameters…` / `Format verified: serial 54A9DBE2 -> 56E51A05`）。④ 修同一天第二次踩的门控坑：`SETTINGS_FILE_NAME` 抽到无条件模块 `settings_file`。⑤ 顺手统一「清理re启动项/配置」笔误（3 处写入 + 进度窗标题 + 解析器测试输入）。验收：单测 240+45、交叉编译 SHA `18ed4709…`、冒烟 6/6 PASS。此前 00:40 时点：**GUI 语言偏好持久化 + 修 v2.6.41 引入的本机构建缺陷**（[记录 2026-10-25 00:40](./docs/20261025-0040-GUI语言偏好持久化与修本机构建缺陷-DeepSeek-V4.1-Flash.md)）。① 语言切了重启又回中文 = 半截功能 → `BackupRestore.settings.ini` 新增 `ui_language=zh|en`：启动读偏好定下拉框初始项、只在 `CBN_SELCHANGE` 保存。★ **必须合并写**——该文件与 `failure_policy` / `dism_timeout` 共享，整文件重写会静默冲掉别人的键；注释也只剔除自己那条（别人的留着、自己的不累积）。实测种 `failure_threshold=7` / `failure_action=pause` / `dism_timeout_secs=300` 三个键，切语言后全部保留、连切两次文件仍 6 行。② 顺带修 v2.6.41 的编译缺陷：`text_parsing` 无条件编译却引用 `cfg(any(windows,test))` 的 `i18n`/`pe_safety` → macOS `cargo check` 直接 E0433，而 **`cargo test` 因为带 test cfg 反而全绿**。修法 = `MessageLanguage` 搬到 `i18n`（语言枚举属于语言层）、`pe_safety` 用 `pub(crate) use` 重导出（既有引用一行都不用改）、`i18n` 改无条件编译。★ 教训：**`cargo test` 通过 ≠ 构建通过**。验收：`cargo check` 零 unused warning、单测 236+45、部署 SHA `2e0871a0…`、实机 en/zh 双向重启保持、全链冒烟 6/6 PASS。此前 00:50 时点：**GUI 切英文点击（#121）端到端打通，真正根因是 UIPI**（[记录 2026-10-25 00:50](./docs/20261025-0050-GUI切英文点击端到端打通-UIPI才是真正根因-DeepSeek-V4.1-Flash.md)）。双向实测通过（`BEFORE=[刷新环境]` → `AFTER=[Refresh environment]`，反向亦然）+ 整窗英文截图 + `gui.log` 的 `GUI action: control=ID_LANGUAGE id=1005 notification=0x1`。★ **订正上一轮的说法**：卡住的既不是会话隔离（`prlctl exec --current-user` 实测直接就在 session 1，无需任何令牌借用脚本），也不是 `ExecutionPolicy`（它只挡 `-File`，`-Command` 不受限），而是 **UIPI**——GUI 以管理员运行（`GUI_elevated=True`）而测试通道是标准用户（`SELF_elevated=False`），`SendMessage` 被静默拒：`CB_GETCOUNT` 返回 0、`GetLastError=5`，**界面明明显示「中文」却读到 `count=0`**，差一点误判成「语言下拉框从来没填过项」的产品 bug；换成提权执行体后同一查询变成 `COUNT=2`、`ITEM[0]=[中文]`、`ITEM[1]=[English]`。另记两坑：`CB_SETCURSEL` 只改选中项不发通知，必须补 `CBN_SELCHANGE`（否则 `apply_language` 不被调用）；`CreateProcessAsUser` 起的进程**没有 stdout**，「返回 True 但无输出」就是输出丢了、不是脚本没跑。工具收敛为 `tools/run-elevated-session1.ps1`（借 GUI 令牌提权到 session 1）+ `tools/switch-gui-language.ps1`（执行体）+ `tools/_diag-uipi.ps1`（一条命令判定 UIPI）。此前 00:25 时点：**CLI `--json` 扩到全部命令 + 统一出口**（[记录 2026-10-25 00:25](./docs/20261025-0025-CLI-json扩展到全部命令与两处真bug-DeepSeek-V4.1-Flash.md)）。此前只有 `online`/`prepare` 各自接 `--json`，其余命令（`hash`/`recover`/`resume`/`abandon-task`/…）在 json 模式下**零输出**、判据只能退回解析中文。改为「命令 `set_detail(task_id, log_path)` 上报 + `main` 单一出口 `emit`」——各命令自己 emit 会让 `prepare` 输出两行 JSON，消费方被迫猜读哪行。顺带撞出**两个真 bug**：① `prepare` 是严格解析（兜底 `unknown prepare option` 硬报错）→ **直接拒绝 `--json`**（`online` 用 `_ => {}` 忽略未知选项，所以从没暴露）——「main 能识别 ≠ 命令能接受」；② **我自己上一轮引入的回归**：英文模式终态识别失效——`text_parsing` 拿中文原文当判据，v2.6.39 却把该文案交给了 i18n → 英文日志永不命中、进度窗停在上一阶段；改为**双语都认 + 文案查表**（不能只认英文，历史中文日志必须继续命中）——「文案不是契约」的又一次实证。另记：`cfg(windows)` 内的改动 mac `cargo check` 看不见，E0382 只在交叉编译暴露，**交叉编译才算验证过**。验收：单测 236、部署 SHA256 `5f218fb6…`、全链冒烟 7/7 PASS、`hash` 与 `abandon-task`（失败路径）结论行实测。此前 23:55 时点：**GUI 审计扩到全部 4 个 `WM_COMMAND` 入口 + i18n 文案扩到 apply 关键路径**（[记录 2026-10-24 23:55](./docs/20261024-2355-GUI审计扩展到全部入口与i18n扩到apply关键路径-DeepSeek-V4.1-Flash.md)）。① GUI 审计：全仓只有 4 处 `if message == WM_COMMAND`，此前只覆盖主窗口，现补齐另外三个对话框；审计函数改为 `audit_control_action` **不再依赖 `State`**——那三个对话框的 `GWLP_USERDATA` 存的是 `*mut i32` 而非 State，取 State 的写法它们用不了。② i18n 新增 8 条（DISM/wimlib 的开始与结果、等待上限两态）+ `msg_tpl` 占位替换。★ 动手前先做的安全核实：`strip_self_log_lines` 只剥时间戳行与 ASCII 命令行回显、失败分类靠 DISM 英文错误串与 NTSTATUS 码 → **翻译不会破坏 D1/D2 自动降级判据**。英语模式真实还原实测：`DISM start (wait limit unlimited): …`、`Operation complete: …`。★ **任务 #121（GUI 切英文点击）端到端仍未跑通**：卡在会话隔离（session 0 枚举不到 session 1 窗口）与 ExecutionPolicy=Restricted（须 iex），截图确认界面仍中文，坑已写进 `tools/switch-gui-language.ps1`。单测 236、全链冒烟 7/7 PASS。此前 23:45 时点：**新建全局语言层 `i18n`，界面之外的日志与 CLI 结论文案也能切英文**（[记录 2026-10-24 23:45](./docs/20261024-2345-全局语言层i18n与日志多语言化-DeepSeek-V4.1-Flash.md)），对应你要求的「其它地方适当的多语言化」。改造前只有界面双语（GUI `Language` + RE `MessageLanguage`），**日志与 CLI stdout 没有语言维度**（`append_log` 不带 language 参数，258 个调用点中文硬编码）。新增 `i18n.rs`：进程级语言（优先级 `set()` > env `BACKUPRESTORE_LANG` > 默认中文）+ `(key, 中文, 英文)` 文案表，接入 `main`（读 env）、RE 侧（`UI_LANGUAGE` 同步）、GUI `apply_language`（**界面切英文后日志也跟着切**，此前切了没用）、`online_operation`（STEP 1 / 日志位置 / 收尾结论）。★「适当」的边界：**只译结论性文案**，深层细节（如 `dism.exe 失败：退出码 2`）仍是中文——258 个调用点全量翻译是灾难级改动，改法不是改签名而是需要双语的点主动查表。★ 查表抽成纯函数 `msg_in(lang, key)`：Rust 单测并行，测试里改进程级全局会污染断言中文措辞的其它用例。实测中英对照：env=en → `[ERROR] Online operation did not complete: …`、日志 `Log location: …` / `STEP 1/4 Validating …`；不设 env → 中文**逐字不变**。单测 235、全链冒烟 7/7 PASS。此前 23:50 时点：**用户三条解法落地——CLI `--json` 结构化输出 + GUI 操作审计日志 + 查清"切英文"的边界**（[记录 2026-10-24 23:50](./docs/20261024-2350-CLI加json与GUI操作审计日志-用户三条解法落地-DeepSeek-V4.1-Flash.md)）。① **CLI `--json`（终局解法）**：新增 `cli_json.rs`，字段固定（command/operation/success/task_id/log_path/error）、缺失一律 `null`、单行输出、路径反斜杠交给 serde 转义；判据只认结构化字段，不再解析中文文案。② **GUI 操作审计日志**：在 `WM_COMMAND` **入口**统一调用 `audit_gui_action`，一处覆盖所有操作（`GUI action: control=<控件名> id=<id> notification=0x<码>`），配 75 个控件 ID 名称表；各分支原有语义日志保留，两者互补（入口=用户做了什么，分支=做成了没有）。③ **"把界面调成英文"部分成立**：程序确实多语言化（`native_gui.rs:1001 Language` 枚举、`ui_text`/`tooltip_text` 双语齐全、`selected_language` 读 `ID_LANGUAGE` 下拉框、`prepare --ui-language zh|en`），**但 `append_log` 不带 language 参数，日志与 CLI stdout 无语言维度**——切英文救不了判据，这正是 `--json` 存在的理由。★ 顺带核实旧日志缺口清单：P0 的 panic hook（`main.rs:453 set_hook`）与 MiniNT（`main.rs:1300` 已 append_log）**早已修好**，两份审查文档却仍写"未执行"——**旧清单不能照抄，动之前先 grep 核实**。验收：单测 231 通过、全链冒烟 7/7 PASS、`gui.log` 实测出现审计行。此前 23:00 时点：**冒烟脚本完成 ASCII 化与判据纪律固化**（[记录 2026-10-24 23:00](./docs/20261024-2300-编码问题为何躲不开中文-ASCII边界与判据纪律-DeepSeek-V4.1-Flash.md)）。用户追问「编程时为什么还涉及 UTF-8？写 CMD 测试都用 ASCII 不就彻底解决了？」——**对了一半，且是对的、该照做的那一半**：进入 `prlctl` 命令行的字符串与全部判据已 **100% ASCII**（精确扫描：VM 命令体 0 处非 ASCII；脚本内中文判据 0 处）。但**中文不在代码里，在被测系统的回答里**：脚本一个中文都没有时，`dir /b T:\` 仍回 GBK 的「找不到文件」（`D5D2 B2BB B5BD CEC4 BCFE`）、主程序仍回 UTF-8 的「操作完成」——**编码问题是数据的属性，不是代码的属性**。三件实锤：① 我用 `tr -d '\200-\377'` 剥非 ASCII 字节后看到 `DISM  0`，一度当成纯 ASCII 退出码判据，**真实字节是 `DISM 结果：退出码 0`**——用 ASCII 的眼睛看中文数据，看到的是残缺假象；② `restore.out` 剥掉中文后只剩一行日志路径，想从 stdout 拿结论就绕不开中文，故判据改到结构化源；③ C3 判据「名不副实」：叫 success/100 却判中文文案，从没读过 `status.json` 的 stage/progress。**四层纪律**：代码层进 `prlctl` 的串全 ASCII / 判定层只认结构化特征（`STEP 4/4`、DISM 的 `/English` 英文成功行、数字、哈希、序列号）**绝不 grep 中文文案** / 传输层 `cmd /c type` 字节透传不转码 / 展示层中文只落 mac 终端。**`LC_ALL=C` 环境下全链 7/7 PASS**，即判据与 locale 无关。此前 22:50 时点：**冒烟套件（D5）通道重构——编码统一下沉到入口，废掉 base64 往返**（[重构记录 2026-10-24 22:50](./docs/20261024-2250-冒烟通道重构-入口统一编码替代base64往返-DeepSeek-V4.1-Flash.md)）。用户质疑「为什么老在拼 Base64、不一开始就调编码」，实测三条定论：① `prlctl` stdout 是**字节透传、无长度上限**（30KB ASCII / 6KB UTF-8 中文逐字节完整回读），回读文件只需 `cmd /c type`；② PowerShell 侧编码要**四件套**（PS5.1 默认 `$OutputEncoding=us-ascii`、`[Console]::OutputEncoding/InputEncoding=gb2312` 全是错的）、CMD 侧实测**改不了**（`chcp 65001` 虽生效但 `dir` 输出与 936 逐字节相同、`cmd /u` 输出直接丢失）只能不依赖；③ VM 可直读宿主 `\\Mac\Home`/`\\psf\Home` → **脚本零传输**，顺带绕开命令行 ~3.4KB 上限。据此删掉约 60 行 base64 搬运（分块写 + 长度校验 + 多段拼接 + 重试），helper 部署降为「共享不可用」时的 fallback；`-EncodedCommand` 保留（它解决引号/长度/中文传参，不是编码手段）。**期间挖出四个真 bug**：C3 判据的 `log` 被 CLI stdout 覆盖（一直误报「任务日志没到 STEP 4/4」）、回读重试被误删（空结果=通道抽风）、`grep` 在 UTF-8 locale 下会整行跳过 GBK 字节流（需 `LC_ALL=C`）、`fsutil` 中文输出失配（改 `Get-Volume`）。全链冒烟 **7/7 PASS**（备份 → WinRE 捕获 → 在线格式化还原 → 六判据）。此前 21:26 时点：**D1/D2 韧性加固轮——DISM 失败自动降级 wimlib + 错误分类，三层实机验收通过**（[实现与验收 2026-10-24 21:26](./docs/20261024-2126-D1D2实现与三层实机验收-Step-5-Preview.md)）。四个 apply 落点（恢复环境主链 / 预安装环境 / 在线数据卷 / 在线第二系统）共用一个降级编排函数：DISM 失败 → 纯函数分类六类因果（访问被拒/空间不足/占用/镜像损坏/超时/未识别）→ ImageCorrupt 不降级、其余自动降级 wimlib 重试一次（不限制超时）。实机三层：① 损坏镜像判 ImageCorrupt 明确不降级；② `DISM_TIMEOUT_SECS=90` 超时 → wimlib 17 分钟写完 79 GiB/658023 文件、任务成功，**镜像外文件未被删**（overlay 语义铁证）；③ 锁句柄占用双失败、两条中文因果均正确归因 Busy。**实机挖出并修复三个真 bug**：分类被自家日志行 `/CheckIntegrity` 污染（Busy 误判 ImageCorrupt、降级几乎全失效，已加净化 + 4 条回归单测）、wimlib NTSTATUS 码无模式、`apply_overlay` 的 Err 不带输出尾部。版本线 2.6.34→2.6.37。此前：**部署链路清理轮——用户拍板三项全部落地**。① [TaskStore 搬离 C:](./docs/20261024-0550-TaskStore搬离C与证据保留边界拍板-Hy4.md)：任务存储 100% 落 `H:\brwork\tasks`，A 入口回归铁证（哨兵消失 + C: 序列号 `BCD4-EAAB`→`5698-64F6` + 任务 success/100% + 任务目录在 H:，见 [0755 回归文档](./docs/20261024-0755-A入口还原C盘回归验证TaskStore搬离效果-Hy4.md)）；② [不再往 C: 部署 exe](./docs/20261024-0958-不再往C部署exe与自启项改指H-Step-5-Preview.md)：两个部署脚本去掉 C: 包目录（每次 deploy 顺带清残留），自启项 `HKCU\Run\BackupRestoreTest` 改指 `H:\brwork\BackupRestore.exe`，重启实测无弹框；③ [备份排除表排除程序部署件](./docs/20261024-1050-备份排除表增加程序部署件-Step-5-Preview.md)（v2.6.33）：5 个部署件文件只排文件不排目录、tasks/logs 证据全留，★RE 内必须传 `store.root`，格式化还原后「程序目录零二进制」端到端铁证。其间版本：v2.6.30 B 键恢复环境内整链 / v2.6.31 按 B 回退后重试对话框加警示 / v2.6.32 备份确认框不再冒充还原目标 + `--tab` 编号订正。**下一步开发方向见 [下一步详细开发方向 2026-10-24 15:08](./docs/20261024-1508-下一步详细开发方向-Step-5-Preview.md)**（宿主空间治理 → apply 失败自动降级 wimlib → deploy 冒烟套件 → 命名分叉四步，附 5 项待拍板）。v2.6.29 查漏补缺轮记录见 [本轮记录](./docs/20261023-0328-v2.6.29-wimlib依赖库校验缺口修复与交接文档高危项订正-Hy3.md)。上一版 **2.6.27**：**wimlib「保留访问控制列表」产品化完成并端到端实机通过（2026-10-23 02:21）**——`wimlib_args` 去掉 `--no-acls`（`apply_no_acl` 改名 `apply_overlay`），图形用户界面确认框改为「镜像目录访问控制列表/属主完整还原」，即覆盖式应用现在**连镜像内的安全描述符一起还原**（属主 / 访问控制列表 / protected），不再带「微软商店应用权限可能异常」的代价。实机部署件 = 虚拟机原生构建 `3ef72e73…5b82`（增量 32.22 秒，八联 SHA 全对）；端到端任务 `356f0b55` `success/100`：恢复日志第 2,203,313 行 wimlib 行**行尾无 `--no-acls`**、退出码 0、本会话零 DISM apply，哨兵存活 + 覆写标记消失 + 序列号 `6A2D-BA03` 不变，**访问控制列表判据 8/8 逐字节一致**（含原生微软商店应用条件 ACE + app package SID + TrustedInstaller，与 `--no-acls` 裸默认基线可区分），事件标识符/WerFault 零新增，环境已恢复（default 回原系统 `{077a3904}`、删 `BR-acl-227`、`F:` 已格式化 `BE7F-3EC3`），详见 [端到端实机通过记录](./docs/20261023-0230-wimlib保留访问控制列表端到端实机通过-Step-5-Preview.md)。上一版 **2.6.25**：**#110 wimlib --no-acls 覆盖式还原通道集成完成 + 实机验收通过（六条判据全过 + 三项判据零新增，环境已恢复）**——用户 2026-10-22 裁定「换 wimlib --no-acls」解决「覆盖式应用（取消格式化）打到已含 Windows 的卷被 DISM 拒绝（RestoreDirMetadata 0x80070005）」的能力缺口。四处 apply 全部路由就绪（恢复环境任务链 / 在线链 / 预安装环境入口 / 数据卷不涉及），prepare/online/图形用户界面三处原「硬拒」改为「路由/确认框」，新模块 `wimlib_apply.rs`（缺失二进制明确报错、绝不静默回退 DISM），`TargetSpec.overlay_no_acl` 落盘，恢复环境载荷注入 wimlib 两文件，`windows/wimlib-aarch64/` 二进制入库 + 虚拟机内 `build-windows.ps1 -Deploy` 八联 SHA 校验；macOS 单测 255 通过、交叉编译 2,869,248 B（SHA256 `1423beba…`，仅本地类型检查用途）；**实机部署件 = 虚拟机原生构建 `87ad04d6…bfc03`**，端到端任务 `ab95a79a` success/100，详见 [#110 集成记录](./docs/20261022-1719-wimlib覆盖式还原通道集成任务110-Step-5-Preview.md) 与 [端到端实机通过记录](./docs/20261022-2030-wimlib覆盖式还原端到端实机通过-Step-5-Preview.md)。上一版 **2.6.24**：**#38 A/B 入口 + 备份 + 预安装环境安装全链复跑实机通过（六步零缺陷收口）**，叠加此前的 V1/V5 实机验收 + D4 修复首验 + 历史单测平台门控——#40 的 V 系列（V1–V6）全部结清。#38（用户 10-22 拍板合并专场一轮）：① 快照 `{72ea9997}` 拍后即删 → ② 系统备份（任务 `0ee37f85`，31.96GB 镜像，索引名「Windows 11 26200.9550 20261022」）+ 数据备份（T:，索引名「Windows (T:) 20261022」降级规则）→ ③ B 入口在线还原到 T:（哨兵消失 + 序列号 `08835EE0→E880494B`）→ ④ A 入口还原 C:（任务 `62c4902d` `success/100%`，三道确认全真实点击，哨兵消失 + 序列号 `8E2F5FC1→BCD4EAAB` + default 指向新系统 + 菜单名「本次还原的 Windows」）→ ⑤ 预安装环境双模式（内存落 `H:\BackupRestorePE` + 硬盘落 F:，双启动配置数据项齐全）→ ⑥ 删快照 + 崩溃事件收口（事件标识符 1000 零新增、WerFault 0），详见 [#38 全链复跑验收](./docs/20261022-1200-38全链复跑AB入口备份预安装环境安装实机验收-Step-5-Preview.md)。此前：**V1（A 入口还原到 C: 活动系统盘）+ V5（图形用户界面弹窗取消勾选主系统）实机验收通过**。V1（`restore-existing` → 活动系统盘 C:，离线预安装环境内执行）任务 `b4cec607` `success/100%`：哨兵消失 + 序列号 `48D5196E → 8E2F-5FC1` + default 指向本次还原系统 + 菜单名「本次还原的 Windows」+ 事件标识符/WerFault 零新增；**D4 修复（恢复环境独立恢复分区重接）在 A 入口首次实机验证**。V5（任务 `742cfd28`）：预安装环境进度窗按 M 打开引导修复编辑器，对「当前默认启动项·不可取消」的主系统按空格取消勾选**被拒绝**（√ 不丢失），对「正在还原中的系统」同样拒绝；终态 default 指向新系统、老系统完整留在 displayorder、菜单名保持不变——无任何静默 default 篡改。另发现**镜像「带回旧状态」效应**：还原系统盘会连程序部署位一起回滚到镜像拍摄时刻（包目录回到 v2.6.19、旧任务 02138f14 复活成弹框）——已补部署 v2.6.24 两处 + abandon 收口，详见 [V1 验收与 D4 验证](./docs/20261022-0845-V1-A入口还原C盘实机验收与D4修复验证-Step-5-Preview.md) 与 [V5 验收](./docs/20261022-0915-V5-图形用户界面弹窗钉住项实机验收-Step-5-Preview.md)。上一版 **2.6.23**：**门禁性能分级（prepare 8 分 44 秒 → 29 秒）+ V4 默认菜单名验收**。v2.6.22 修好挂载参数后，门禁每次 prepare 都要挂一次 32GB 镜像（实测 8 分钟），本版加快速通道：先花约 2 秒取镜像显示名，**只有名字可疑才做权威挂载探测**；实测完整镜像 29 秒放行、纯预安装环境镜像 10 秒硬拒、缺件镜像 29 秒放行（恢复期优先级 5 降级跳过仍是硬兜底）。同一轮还实机通过 **V4**（C 入口未起名 → 新项名字 = 镜像索引名 `Windows Backup`，`default` 指向新系统、原系统仍在 displayorder、哨兵消失 + 序列号变化、事件标识符/WerFault 零新增）——见 [V4 验收与门禁快速通道](./docs/20261022-0745-V4默认菜单名验收与门禁快速通道v2.6.23-Step-5-Preview.md)。上一版 **2.6.22**：**「完整 Windows 门禁」参数缺陷修复（`wim_mount_markers` 把 `/Mount-Wim` 的参数写成 `/ImageFile:`，应为 `/WimFile:`，DISM 恒报 87 → 该权威判据自 v2.4.2 起从未生效、静默降级为镜像名启发式）+ 缺件镜像硬拒实机验证**。`wim_mount_markers` 把 `/Mount-Wim` 的参数写成了 `/ImageFile:`（那是 `/Apply-Image` 的选项），DISM 恒报 87 → 这条权威判据**自 v2.4.2 引入起从未生效**，一直静默降级为「镜像名里有没有 winpe」的弱启发式，且日志无痕。本版：①参数改 `/WimFile:` 并抽成 `wim_mount_args` 纯函数 + 单测锁死；②两处调用点补「通过/降级」留痕；③订正「VHD 附加导致 87」的错误归因。实机双组对照：缺件镜像（删 `reagentc.exe`）现在**准备期硬拒、零写入**（EXITCODE=1，任务目录数不变），完整镜像正常通过并留痕（见[门禁修复与硬拒验证](./docs/20261022-0715-门禁参数修复v2.6.22与缺件镜像硬拒验证-Step-5-Preview.md)）。⚠️ **代价：每次 prepare 实测多约 8 分钟**（32GB 镜像挂载 6–7 分钟），优化方案待用户拍板（§五三条路线）。上一版 **2.6.21**：**恢复环境内「cmd 后台最大化」根因修复 + B 键端到端实机验收 + 全环节验收核查（含优先级 2 硬拒命令行、20 天零崩溃弱信号、部署 SHA 一致三项补收）**。用户 2026-10-17 提的需求（进恢复环境后自动开的 cmd 必须「最大化 + 在后台」、程序保持前台）此前三次实机失败——根因是 `SW_SHOWMAXIMIZED` 创建即激活窗口，cmd 抢前台盖住进度窗；本版改为 `SW_SHOWMINNOACTIVE` 创建 → 等窗口就绪 → 最大化并 `SetWindowPos(HWND_BOTTOM)` 压底，进度窗创建时 `AttachThreadInput` + `SetForegroundWindow` 显式抢回前台（详见[cmd 后台化修复与 B 键验收](./docs/20261020-0230-RE内cmd后台最大化修复与B键端到端验收-Step-5-Preview.md)）。**六项主流程全部有过成功实机验收**（10-18 总报告 + v2.6.18~v2.6.21 每轮针对性复验）；当前二进制 SHA256 `62f19b24…` 与构建产物逐字节一致（2,862,080 B），单测 208+44 全过；剩余 4 项均需实机成本或用户拍板（见【下一步待实现】）。历史沿革（v2.6.1–v2.6.20）：v2.6.2 把 default 决策抽成纯函数 `boot_menu::decide_default` + 8 个单测护栏、`boot_entry.rs` 补 S4 设计原则注释、订正交接复核发现的三处文档缺陷（详见[本轮记录](./docs/20261015-2220-交接复核落地-default决策单测S4注释与文档订正-Step-5-Preview.md)）；**v2.6.3**（用户拍板 A3/A4）备份/还原进度文案按 operation 分叉（进度窗标题「备份进度/恢复进度」、STEP 1/4 备份=只读源卷不格式化，同时订正旧措辞「不改动主系统默认启动项」的事实错误）+ 引导方案里「当前默认启动项」钉住项加行尾说明与顶部说明行，并修掉一个中文 Windows 上偶发失败的既有测试（详见[A3/A4 落地记录](./docs/20261015-2330-A3文案分叉与A4钉住说明落地-Step-5-Preview.md)）。更早的「还原入口三分」+「格式化目标分区」选项覆盖 A/B/C 三入口×在线离线全通道（默认勾选并校验格式化成功；取消=覆盖式应用）见[还原入口三分轮文档](./docs/20261014-1253-还原入口三分UI拆分A通道B数据C他盘-GLM-5.md)与[文案修正轮](./docs/20261014-1315-入口文案改回全称与语言栏右对齐-GLM-5.md)。

2.4.3 复核文档：[复核另一会话改动](./docs/20261013-1036-复核另一会话改动-预检被放行致P卷被毁与恢复-Haiku4.5.md)；2.4.2 待办五项全做记录见[2.4.2 轮文档](./docs/20261013-0340-2.4.2-待办五项全做-门禁措辞PE预检ESP多候选鼠标验收与清理-Opus5.md)。

上一版本 2.4.1。**在线还原到第二个 Windows 卷全流程实机通过**：STEP 1→4（含新增 STEP 3.5 盘符绑定）+ 重启后第二系统桌面正常（零崩溃）+ 恢复环境已注册 + 主系统未受影响。
**2.4.1 引导修复图形用户界面前台实机验收通过**：测试钩子全自动链路（无人工点击）跑完 STEP 1→4，界面弹出「执行完成」，启动配置数据五项独立回读全部通过（主系统默认项、resumeobject、displayorder、timeout、菜单名）。此前连报四轮的“原有启动菜单项丢失”根因是被删旧 loader 以**退出码 0 + 空文案**返回、被误判为丢失外来项，详见[连续还原三轮同错根因](./docs/20261013-0152-连续还原三轮同错根因-被删旧loader误判为菜单丢失-Opus5.md)与[验收通过记录](./docs/20261013-0219-引导修复GUI前台实机验收通过-Opus5.md)。
2.3.9 新增【引导修复】：还原第二阶段起，进度窗出现「引导修复」按钮，用户可勾选要加入启动菜单的系统、排序、改名、设菜单秒数（**无倒计时**，本次还原系统必选，其它默认全选），选择落盘后于映像应用完成时一次性写入启动配置数据并回读校验。

> **⚠️ 跑实机测试前必读（磁盘空间硬规则）**：
> [Parallels 快照吃满磁盘根因与空间硬规则](./docs/20261012-1320-Parallels快照吃满磁盘根因与空间硬规则-GPT-5.md)
> —— 快照会把第二系统盘（~87GB）整份复制，拍 3 层就吃掉 265GB。
> **改启动配置数据用 `bcdedit /export` 导文件代替拍快照；跑 87GB 镜像前保证 ≥220GB 空闲。**

> **文档导航（精简版）**：
> - 【**代码审查（最新）**】：[全项目代码审查（v2.6.17 工作区）](./docs/20261018-2056-全项目代码审查-DeepSeek-V4.1-Flash.md) —— **全量只读复审**（约 3.8 万行产品代码，含 `backuprestore-core` 4 + `backuprestore-cli` 27 文件）：**高危 3 / 中危 8 / 低危 11**。高危 = ①A 入口目标 combo 自动锁定失效且全链无硬闸（界面显示的卷可能≠实际被格式化的卷，沿用 `20261018-0247` 未修）；②`(RecoveryComplete, Failed)` 状态转移缺失致该阶段失败后任务永久悬置（沿用未修）；③Esc 被转给编辑器专用的 `handle_key` 会销毁焦点控件/主进度窗并绕过「暂停中关窗」防卡死处理（本轮新增，⚠️需实机复现）。逐条复检了 `20261018-0247` 全部结论现状，文末给 P0–P3 处置顺序。**引用它之前先读经验库 `经验/07-墓碑与争议清单.md`。**
> - 【**首选入口**】：[项目全景与知识浓缩](./docs/20261011-2350-项目全景与知识浓缩-Hy4.md) —— 定位、三条入口差异、架构代码地图、状态机与断电续跑、启动链硬约束与禁令、能力边界、已知缺陷，全部浓缩于此。**新会话先读它。**
> - 【**经验库**】：[经验/README.md](./经验/README.md) —— docs/ 全量 219 篇合并提炼为 8 类可复用经验（项目状态 / VM操作与传输 / BCD与启动链 / WinRE与载荷 / 验收判据 / 开发构建 / 墓碑与争议清单 / 根因速查），每条带可信度标注。**引用任何旧文档前先查 `经验/07-墓碑与争议清单.md`**；docs 原文与经验库冲突时以经验库为准。
> - 【**接手 AI / 新会话优先读**】：[20261025-0125-RE主链DISM超时形同虚设修复与降级实证-DeepSeek-V4.1-Flash.md（2026-10-25 01:25）](./docs/20261025-0125-RE主链DISM超时形同虚设修复与降级实证-DeepSeek-V4.1-Flash.md)（**「配置读了但没接线」是最难查的一类缺陷**：上限只在启动时记了一行日志、真正等子进程的地方是无条件 `child.wait()`。查法是**用极端参数跑一次**：把上限设成 1 秒 + 大镜像，能降级就说明接上了。另记 `wait_child` 的正确超时姿势：kill + 回收之后**仍要 join 读线程**，否则输出尾部丢失、失败无法归因）→ [20261025-0110-日志全量多语言化与B6目录命名分叉-DeepSeek-V4.1-Flash.md（2026-10-25 01:10）](./docs/20261025-0110-日志全量多语言化与B6目录命名分叉-DeepSeek-V4.1-Flash.md)（**多语言化三条硬教训**：① **「全量」前先数清有几种写日志方式**——只数 `append_log` 会漏掉模块自带的 `log()`；② 翻译前先扫中文判据，STEP 解析只认前缀与数字、`bcdedit` 读的是系统输出、失败分类只认 DISM 原文；③ `cause_*` 英文措辞要避开分类模式的整串，否则 Busy 会被认成 AccessDenied。另：目录名/语言偏好的白名单与优先级都在这里）→ [20261025-0040-GUI语言偏好持久化与修本机构建缺陷-DeepSeek-V4.1-Flash.md（2026-10-25 00:40）](./docs/20261025-0040-GUI语言偏好持久化与修本机构建缺陷-DeepSeek-V4.1-Flash.md)（**v2.6.42 两条经验**：① `BackupRestore.settings.ini` 是**多模块共享**文件，任何写它的人都必须**合并写**（只替换自己那行、只剔除自己那条注释），整文件重写会静默冲掉 `failure_threshold`/`dism_timeout_secs`；② **`cargo test` 通过 ≠ 构建通过**——被 `cfg(any(windows,test))` 门控的模块一旦被无条件模块引用，只有不带 test 的 `cargo check`（mac 上）才报 E0433）→ [20261025-0050-GUI切英文点击端到端打通-UIPI才是真正根因-DeepSeek-V4.1-Flash.md（2026-10-25 00:50）](./docs/20261025-0050-GUI切英文点击端到端打通-UIPI才是真正根因-DeepSeek-V4.1-Flash.md)（**驱动 GUI 的完整前提**：① 进 session 1 用 `prlctl exec --current-user` 就够，不需要令牌借用；② 但 GUI 是提权进程，**标准用户发 `SendMessage` 会被 UIPI 静默拒**（`CB_GETCOUNT` 回 0 + `err=5`，看起来像控件是空的）→ 必须借 GUI 自己的令牌提权；③ `CB_SETCURSEL` 不发通知，要补 `CBN_SELCHANGE`；④ `CreateProcessAsUser` 无 stdout，结果落文件）→ [20261025-0025-CLI-json扩展到全部命令与两处真bug-DeepSeek-V4.1-Flash.md（2026-10-25 00:25）](./docs/20261025-0025-CLI-json扩展到全部命令与两处真bug-DeepSeek-V4.1-Flash.md)（**判据的三条硬纪律**：① `--json` 全命令统一出口——命令只 `set_detail` 上报、`main` 单点 emit，别让各命令自己打；② **严格解析的命令要显式接受统一参数**（`prepare` 因兜底硬报错拒了 `--json`，"main 能识别 ≠ 命令能接受"）；③ **判据绝不能依赖会被翻译的文案**——v2.6.39 翻译了终态文案，英文模式判据当场失效，改为双语查表；④ `cfg(windows)` 内改动 mac check 看不见，必须交叉编译）→ [20261024-2355-GUI审计扩展到全部入口与i18n扩到apply关键路径-DeepSeek-V4.1-Flash.md（2026-10-24 23:55）](./docs/20261024-2355-GUI审计扩展到全部入口与i18n扩到apply关键路径-DeepSeek-V4.1-Flash.md)（**GUI 自动化两坑**：session 0 枚举不到 session 1 窗口须借 token 进入；ExecutionPolicy=Restricted 只能用 iex。扩展多语言前先确认文案是否被分类/判据依赖）→ [20261024-2345-全局语言层i18n与日志多语言化-DeepSeek-V4.1-Flash.md（2026-10-24 23:45）](./docs/20261024-2345-全局语言层i18n与日志多语言化-DeepSeek-V4.1-Flash.md)（**多语言与判据纪律**：新增文案进 `i18n::MESSAGES` 表；判据仍优先用 `--json` 结构化字段、不依赖任何语言的文案；测多语言用纯函数 `msg_in` 别改全局）→ [20261024-2350-CLI加json与GUI操作审计日志-用户三条解法落地-DeepSeek-V4.1-Flash.md（2026-10-24 23:50）](./docs/20261024-2350-CLI加json与GUI操作审计日志-用户三条解法落地-DeepSeek-V4.1-Flash.md)（**判据优先用程序自陈的结构化字段**——CLI 用 `--json` 的 success/log_path；审计日志放消息入口不逐分支补；**旧审查清单动前先 grep 核实**）→ [20261024-2300-编码问题为何躲不开中文-ASCII边界与判据纪律-DeepSeek-V4.1-Flash.md（2026-10-24 23:00）](./docs/20261024-2300-编码问题为何躲不开中文-ASCII边界与判据纪律-DeepSeek-V4.1-Flash.md)（**编码边界纪律**：进 `prlctl` 的串全 ASCII、判据只认结构化特征不认中文文案、用 `LC_ALL=C` 复跑验证 locale 免疫）→ [20261024-2250-冒烟通道重构-入口统一编码替代base64往返-DeepSeek-V4.1-Flash.md（2026-10-24 22:50）](./docs/20261024-2250-冒烟通道重构-入口统一编码替代base64往返-DeepSeek-V4.1-Flash.md)（**冒烟通道编码纪律**：PowerShell 一律带 `$ProgressPreference="SilentlyContinue"; [Console]::OutputEncoding=[Text.Encoding]::UTF8;`；参数不放中文；读文件用 `type` 不转 base64；脚本从 `\\Mac\Home` 直读；GBK 字节流 grep 加 `LC_ALL=C`）→ [备份排除表增加程序部署件（2026-10-24 10:50，最新）](./docs/20261024-1050-备份排除表增加程序部署件-Step-5-Preview.md)（**v2.6.33**：排除表显式排除程序部署件 5 文件——只排文件不排目录、tasks/logs 证据全留；★RE 内必须传 `store.root` 不能传 `current_exe`（实测五个部署件全进镜像的根因）；格式化还原后端到端铁证「程序目录零二进制」）→ [不再往 C: 部署 exe 与自启项改指 H:\brwork（2026-10-24 09:58）](./docs/20261024-0958-不再往C部署exe与自启项改指H-Step-5-Preview.md)（**C: 上零程序 exe**：两个部署脚本去掉 C: 包目录 + 每次 deploy 清残留；★修正误判——自启走 C: 是用户 `HKCU\Run :: BackupRestoreTest` 随镜像带回、非程序行为，已改指 H:）→ [A 入口还原 C: 全量回归——验证 TaskStore 搬离效果（2026-10-24 07:55）](./docs/20261024-0755-A入口还原C盘回归验证TaskStore搬离效果-Hy4.md)（**哨兵消失 + 序列号 `BCD4-EAAB`→`5698-64F6` + 任务 success/100% + 任务目录在 `H:\brwork\tasks`**；★新发现：还原后自启的 GUI 仍走 C: 包目录旧版，自启链路的 TaskStore 根仍是 C:）→ [TaskStore 搬离 C: 与证据目录保留边界拍板（2026-10-24 05:50）](./docs/20261024-0550-TaskStore搬离C与证据保留边界拍板-Hy4.md)（**任务存储已 100% 落在 `H:\brwork\tasks`**，C: 包目录 `\tasks` 已清空删除、回收 1.35 GB；证据目录全留 + 载荷副本可清）→ [38-lite 数据备份与 B 入口在线还原实机验收（2026-10-24 04:30）](./docs/20261024-0430-38-lite数据备份与B入口在线还原实机验收-Step-5-Preview.md)（**#38-lite 已结清**：① T: 数据备份 + ② B 入口在线还原全部实机通过，含三个工具层发现与 T: 清理、历史启动项结案）→ [接手 AI 交接说明（v2.6.32）](./docs/20261024-0350-接手AI交接说明-v2.6.32-Step-5-Preview.md)（项目定位 + 两条 apply 通道分界 + 标准改一轮流程 + 环境坑 5 类 20 条 + 待办清单 + 状态快照 + 产物 SHA，自包含；其 §4.2 的 #38-lite 两步已被上文结清）→ [旧版交接（v2.6.3，历史）](./docs/20261015-2350-接手AI交接说明-Step-5-Preview.md) → [当前状态与待办交接复核](./docs/20261015-2109-当前状态与待办交接复核-Hy4.md)（v2.6.1 已验收边界 + A/B 两类待办 + 10 条铁律，文首有 v2.6.2/v2.6.3 复核订正块）→ [已知未修与长期守则清单](./docs/20261015-2150-已知未修与长期守则清单落库-Step-5-Preview.md)（替代库外 `.workbuddy/memory/MEMORY.md`）。
> - 【当前开发进度】：**v2.6.44——修 RE 主链 DISM 无超时（`--dism-timeout-secs` 形同虚设）+ 降级首次实证**（[记录 2026-10-25 01:25](./docs/20261025-0125-RE主链DISM超时形同虚设修复与降级实证-DeepSeek-V4.1-Flash.md)）。**v2.6.43——日志全量多语言化（97→0）+ B6 目录命名分叉 + CLI 读界面语言**（[记录 2026-10-25 01:10](./docs/20261025-0110-日志全量多语言化与B6目录命名分叉-DeepSeek-V4.1-Flash.md)）：i18n 表 15→124 条、单测 240+45、冒烟 6/6。**v2.6.42——GUI 语言偏好持久化 + 修本机构建缺陷**（[记录 2026-10-25 00:40](./docs/20261025-0040-GUI语言偏好持久化与修本机构建缺陷-DeepSeek-V4.1-Flash.md)）：`ui_language=zh|en` 落 ini（合并写，实测保住三个既有键）、`MessageLanguage` 搬到 i18n 修掉 macOS `cargo check` 的 E0433；单测 236+45、冒烟 6/6。**#121 GUI 切英文点击已打通，真正根因是 UIPI**（[记录 2026-10-25 00:50](./docs/20261025-0050-GUI切英文点击端到端打通-UIPI才是真正根因-DeepSeek-V4.1-Flash.md)）——双向实测 + 截图 + 审计日志三重铁证；旧结论「会话隔离卡住」已订正（`--current-user` 本来就在 session 1）。**v2.6.41——CLI `--json` 扩到全部命令、统一出口**（[记录 2026-10-25 00:25](./docs/20261025-0025-CLI-json扩展到全部命令与两处真bug-DeepSeek-V4.1-Flash.md)），顺带修两处真 bug（`prepare` 拒绝 `--json`、英文模式终态识别失效），单测 236、全链 7/7 PASS。**v2.6.40——GUI 审计覆盖全部 4 个入口、i18n 扩到 apply 关键路径**（[记录 2026-10-24 23:55](./docs/20261024-2355-GUI审计扩展到全部入口与i18n扩到apply关键路径-DeepSeek-V4.1-Flash.md)），英语真实还原实测通过、全链 7/7 PASS；#121（GUI 切英文点击）端到端未跑通。**v2.6.39——新建全局语言层 `i18n`，日志与 CLI 结论文案可切英文**（[记录 2026-10-24 23:45](./docs/20261024-2345-全局语言层i18n与日志多语言化-DeepSeek-V4.1-Flash.md)），中英对照实测通过、默认中文逐字不变、全链 7/7 PASS。**v2.6.38——CLI `--json` + GUI 操作审计日志已落地**（[记录 2026-10-24 23:50](./docs/20261024-2350-CLI加json与GUI操作审计日志-用户三条解法落地-DeepSeek-V4.1-Flash.md)），全链冒烟 7/7 PASS、单测 231 通过。**冒烟脚本 ASCII 化完成**（[记录 2026-10-24 23:00](./docs/20261024-2300-编码问题为何躲不开中文-ASCII边界与判据纪律-DeepSeek-V4.1-Flash.md)）——VM 命令体 0 处非 ASCII、判据 0 处中文依赖，`LC_ALL=C` 下仍 7/7 PASS。**冒烟套件 D5 已可当 deploy 门禁**（[重构记录 2026-10-24 22:50](./docs/20261024-2250-冒烟通道重构-入口统一编码替代base64往返-DeepSeek-V4.1-Flash.md)）——`tools/smoke/br-smoke.sh` 全链 7/7 通过，`--skip-backup --image` 可复用镜像快速复跑；通道层已从 base64 搬运改为「入口统一编码 + 共享目录直读」。上一版 **v2.6.37**（[D1D2实现与三层实机验收 2026-10-24 21:26](./docs/20261024-2126-D1D2实现与三层实机验收-Step-5-Preview.md)）——DISM 失败自动降级 wimlib + 错误分类，三层实机验收通过（损坏不降级 / 超时降级成功 / Busy 双失败双因果），含三个真 bug 修复。**上一版见 [备份排除表增加程序部署件 2026-10-24 10:50](./docs/20261024-1050-备份排除表增加程序部署件-Step-5-Preview.md)（v2.6.33）**：`build_capture_exclusions` 增加「程序部署件」排除（5 个文件名，只排文件不排目录——tasks/logs 证据全留；历史分发目录 `\Users\Public\backupRestore-package` 整目录固定排除）。★实测踩坑：RE 内捕获必须传 `store.root`（= Windows 侧运行目录），传 `current_exe` 会因 RE 暂存路径与捕获卷不同卷导致排除项永不生成（第一版五个部署件全进镜像）。端到端铁证：格式化还原后 `T:\brwork-test` 只剩证据与用户数据、零二进制。**上一版见 [不再往 C: 部署 exe 2026-10-24 09:58](./docs/20261024-0958-不再往C部署exe与自启项改指H-Step-5-Preview.md)**：用户拍板「不再往 C: 部署 exe」落地——`build-win.sh` 与 `windows/build-windows.ps1` 去掉 C: 包目录部署（八联 SHA → H: 四联，每次 deploy 顺带 rmdir 清残留），H: 五件套齐（新增 winpe-winpeshl.ini）；★修正 07:55「自启是程序行为」的误判：真相是用户 `HKCU\Run :: BackupRestoreTest`（此前两轮取证因 SYSTEM 身份看不到真实用户 hive 而漏掉），已改指 `H:\brwork\BackupRestore.exe`，重启实测自启 PID=5092 PATH=H:、v2.6.32、无弹框、EventID 1000/WerFault 零新增。**上一版见 [A 入口还原 C: 全量回归 2026-10-24 07:55](./docs/20261024-0755-A入口还原C盘回归验证TaskStore搬离效果-Hy4.md)**：A 入口实机还原 C: 判据全过（哨兵消失 + 序列号 `BCD4-EAAB`→`5698-64F6` + 任务 `c770b1a4` success/100% + **任务目录在 `H:\brwork\tasks`** + default 指向新系统 + 崩溃判据零新增）；本次因宿主仅 22 Gi **未拍快照**（拍了会因增量层撑爆宿主）。**上一版见 [TaskStore 搬离 C: 与证据目录保留边界拍板 2026-10-24 05:50](./docs/20261024-0550-TaskStore搬离C与证据保留边界拍板-Hy4.md)**：交接文档 §5.1 两项拍板已落地——证据目录全留（例外：载荷副本可清）+ TaskStore 搬离 C:（4 个终态任务双校验迁入 `H:\brwork\tasks`，21→25 个；C: 包目录 `\tasks` 删除；GUI 回归确认从 `H:\brwork` 起、无待续跑弹框）；`build-win.sh` 与本文档部署表口径已固化为「权威运行目录 H:\brwork / 分发副本 C: 包目录」。**上一版见 [#38-lite 数据备份与 B 入口在线还原实机验收 2026-10-24 04:30](./docs/20261024-0430-38-lite数据备份与B入口在线还原实机验收-Step-5-Preview.md)**：#38-lite 两步实机通过并结清（① T: 数据备份 success，索引名 `Windows (T:) 20261024` 降级规则、sha `4722ef4c…`；② B 入口在线还原 success，哨兵重现 + 序列号 `E880494B→143E3564` + 100MB 载荷字节级一致 + BCD default/displayorder 不变 + EventID 1000 零新增；T: 已清回空卷、历史启动项结案）；另含三个工具层发现（dump 脚本 `WM_GETTEXTLENGTH`+`GetWindowTextW` 混用被 UIPI 静默拦空、确认框点击后必须复验、完成框确定按钮 ID=2）。**上一版 v2.6.29 见** [v2.6.29 查漏补缺轮 2026-10-23 03:28](./docs/20261023-0328-v2.6.29-wimlib依赖库校验缺口修复与交接文档高危项订正-Hy3.md)：wimlib 依赖库（`libwim-15.dll`）校验缺口修复 + 订正交接文档把 H1/H3 误判为未修（二者实际均已修）；单测 cli 211 过 / core 44 过、交叉编译产物 2,875,392 B / SHA `888951c9…`。**上一版 v2.6.27 最新见** [wimlib 保留访问控制列表端到端实机通过 2026-10-23 02:30](./docs/20261023-0230-wimlib保留访问控制列表端到端实机通过-Step-5-Preview.md)：「保留访问控制列表」产品化 + 端到端通过（任务 `356f0b55`，访问控制列表 8/8 逐字节一致）+ 环境恢复；虚拟机部署件 `3ef72e73…5b82`。上一版 v2.6.25（#110 六条判据全过 + 三项判据零新增）见[端到端实机通过 2026-10-22 20:30](./docs/20261022-2030-wimlib覆盖式还原端到端实机通过-Step-5-Preview.md)（历史沿革）。
> - 【下一步待实现】：**~~CLI 加 `--json` 结构化输出~~（v2.6.38 已落地）；~~日志加语言维度~~（v2.6.39 已落地，`i18n` 模块 + `BACKUPRESTORE_LANG`）。剩余：① **把双语扩展到更多日志点**——258 个 `append_log` 调用点里目前只覆盖了 online 路径的结论性文案，`main.rs`（132 处）、`windows_prepare.rs`（40 处）、`boot_entry.rs`（26 处）等尚未接入，扩展方式明确（搬进 `MESSAGES` 表 + 调用点查表）；② `online` 补 `--ui-language`（目前只有 `prepare` 有）；③ 旧两份日志缺口清单需**逐条 grep 复核**后再定是否实施（P0 多数已修、文档未同步）。**D5 冒烟套件已落地**。剩余相关 backlog：① **给日志加语言维度**（`append_log` 加 language 参数 + 补英文案，属产品级改造，待拍板）；② `online` 补 `--ui-language`（目前只有 `prepare` 有）；③ 旧两份日志缺口清单需**逐条 grep 复核**后再定是否实施（P0 多数已修、文档未同步）。**D5 冒烟套件已落地**：`restore.out` 全是中文文案，加 JSON 后判据可完全脱离日志文本（不必再认 `STEP 4/4`），是编码问题的终局解法；需改产品代码，待用户拍板。**D5 冒烟套件已落地**（`tools/smoke/br-smoke.sh`，全链 7/7；尚待用户拍板是否在 `build-win.sh --deploy` 末尾自动调用）→ 再见[下一步详细开发方向 2026-10-24 15:08](./docs/20261024-1508-下一步详细开发方向-Step-5-Preview.md)（权威版，取代 10-22 待实现清单）；其中 **D1/D2（DISM 失败自动降级 wimlib + 错误分类）的展开设计见 [2026-10-24 16:36 详细设计文档](./docs/20261024-1636-DISM失败自动降级wimlib与错误分类详细设计-Step-5-Preview.md)（可照此开工，待用户拍板 §6 三项）**。**当前无大块代码任务**——**#38-lite（数据备份 + B 入口在线还原）已于 2026-10-24 04:30 实机结清**（见[验收文档](./docs/20261024-0430-38-lite数据备份与B入口在线还原实机验收-Step-5-Preview.md)）；**代码审查三项高危 H1/H2/H3 已于 2026-10-23 复核确认全部结清**（H1 目标硬闸 `native_gui.rs:6599`+`windows_prepare.rs:2521`、H2 `lib.rs:146`、H3 `recovery_progress.rs:1906-1913`），不要再当未修项重复实现；门禁缺陷修复（`/ImageFile:`→`/WimFile:`）经源码核对确认已于 v2.6.22（commit `1ef5e2e`）完成并实机验证，1719 文档该项为陈旧条目。剩余：**① 用户拍板项**（~~32GB noRE 副本删除~~已不在；~~#110 是否复跑~~两轮端到端已满足；~~当前二进制全链专场~~不必；#38 全链复跑专场——用户已判不做，仅剩一句彻底确认）——**~~tasks 证据目录保留边界~~（已拍板：全留，例外=载荷副本可清）**、**~~TaskStore 搬离 C:~~（2026-10-24 05:50 已完成，见 [拍板文档](./docs/20261024-0550-TaskStore搬离C与证据保留边界拍板-Hy4.md)）**、**~~不再往 C: 部署 exe~~（2026-10-24 09:58 已完成，见 [落地文档](./docs/20261024-0958-不再往C部署exe与自启项改指H-Step-5-Preview.md)，含自启项改指 H: 与重启实测）**、**~~备份排除表增加程序部署件~~（2026-10-24 10:50 已完成，见 [落地文档](./docs/20261024-1050-备份排除表增加程序部署件-Step-5-Preview.md)，v2.6.33，含 RE 内 current_exe 坑修复与格式化还原端到端铁证）**——测试卷清理与**「保留访问控制列表」产品化均已完成****→ ② 零星项**（`pe_safety` 死文案；启动配置数据历史 osloader 清理已结案：两个指向 C: 的条目 `{c76d3c5a}`/`{ef55d218}` 非悬空、删除无收益，结论不动）**→ ③ 清理项**（32GB noRE 副本；~~包目录 3 个终态任务~~已随「不再往 C: 部署 exe」整目录删除而消失；`C:\Users\Public\BackupRestore-actual-20261005-0724` 历史存档待用户定去留）**#110 实机验收已于 10-22 20:19 完成**；**「保留访问控制列表」端到端已于 10-23 02:21 完成**（访问控制列表判据 8/8 逐字节一致，见 `20261023-0230`）；**#38-lite 已于 10-24 04:30 完成**（见 `20261024-0430`）。**已完成**（历史）：#38/#40/#39/#41/#110 全结清、门禁快速通道 v2.6.23、V1–V6 实机、虚拟机内 243 单测。上一版待办见[下一步待实现 2026-10-22 17:19](./docs/20261022-1719-下一步待实现-Step-5-Preview.md)（历史沿革）。
> - 【2026-10-15 07:08 纠错轮（Step-5-Preview）】：实测定论「在线态 `bcdedit /export` 提权即可用（首选备份手段）」，推翻 AGENTS.md 旧记载；README 同步把已作废的 1935 根因改引 `20261014-0003` 定论，详见[本轮文档](./docs/20261015-0708-在线态BCD备份手段实测定论与两处文档口径修正-Step-5-Preview.md)。
> - 【最新实机】：[在线还原第二系统全流程通过](./docs/20261012-1505-在线还原第二系统全流程通过-GPT-5.md)（**2.3.8 收尾：桌面正常、零崩溃、恢复环境已注册**）、[2.3.9 引导修复与虚拟机原生编译路径](./docs/20261012-1510-2.3.9引导修复与VM原生编译路径-Fable5.md)（含**中文 bcdedit 字段名缺陷修复**与虚拟机内编译踩坑）、[根因修正-在线路径漏调盘符绑定](./docs/20261012-1435-根因修正-在线路径漏调盘符绑定-GPT-5.md)、[Parallels 快照吃满磁盘根因与空间硬规则](./docs/20261012-1320-Parallels快照吃满磁盘根因与空间硬规则-GPT-5.md)、[灌入WinRE镜像建第二系统两次失败根因与VM并发干扰](./docs/20261012-1755-灌入WinRE镜像建第二系统两次失败根因与VM并发干扰-Step-5-Preview.md)（**「载荷被删」结论已被 1935 文档修正为隐藏属性误判；仍保留并发会话证据**）、[「载荷必须在系统卷」的说法已作废：机理与更正](./docs/20261014-0003-载荷必须在系统卷的说法已作废-机理与更正-Opus5.md)（**现行定论：变量是 ramdisk 规格形态而非卷——盘符形式在数据卷上 bootmgr 解析不了，设备路径形式系统卷/数据卷都能引导；载荷已回镜像卷。`20261012-1935`「数据卷暂存必失败、暂存改放系统卷」的根因判定已作废**）。
> - 【实机·系统卷/数据卷】：[系统卷与引导修复验收](./docs/20261010-2200-系统卷与引导修复验收-Opus5.md)（`BCDBoot` 已通过）、[新载荷路径实机验收](./docs/20261010-1910-新载荷路径实机验收-Opus5.md)（数据卷，跳过 `BCDBoot`）。
> - 【问题与方案】：[可能的问题GPT6.md](./docs/可能的问题GPT6.md)（14 项，已全部处理）、[可能的问题claude5.md](./docs/可能的问题claude5.md)（17 项 + W01～W12，**只设计未实施**）。
> - 【⚠️ 读历史文档前先看】：[代码与文档全量差异核对](./docs/20261012-0038-代码与文档全量差异核对-Hy4.md)（基线 2.3.3）与 [docs 文档与源代码可能的差异](./docs/20261013-1230-可能的差异-GLM-5.2.md)（**基线 2.4.4，最新**）——列出「文档说法 vs 当前代码」的差异，避免把旧版本说法当成当前行为。
> - 【完整目录】：根目录 [文档索引.md](./文档索引.md) 逐条列出全部文档；`docs/README.md` 为主题导航。
>
> ⚠️ **判读提醒**：版本号是标签不是文档 ID。2.2.2 同时挂在 `20261010-1910`（数据卷）和 `20261010-2200`（系统卷）上，覆盖范围须按**文档内容**判定，勿按版本号推断。

> 版本号规则（用户 2026-09-29 重申）：每次修改 +0.0.1，**每一位满十才进位**。
> 因此 `1.7.9` 之后是 `1.8.0`（第三位满十，进给第二位），不是 `1.7.10`。
> 历史上 `1.7.10`~`1.7.14` 是旧习惯记法，等价于 `1.8.0`~`1.8.4`；自 v1.8.0 起按规则进位。

---

## 一、产品流程

```text
正常 Windows
  └─ 打开 BackupRestore.exe，点「系统备份」/「系统还原」
       └─ 当前系统卷选择「进入 Windows RE」
            └─ prepare：校验 + 保存 BCD 副本 + 注入独立 WinRE 副本
                 └─ 镜像卷 BackupRestoreRE + 临时 BCD 条目 + 一次性启动
                      └─ 重启 → WinRE 自动启动 Recovery.exe
                           └─ 离线执行 DISM 捕获/应用 WIM
                                └─ 系统还原时 BCDBoot 修复引导
                                     └─ 清理任务启动项和临时副本
                                          └─ 重启回正常 Windows
```

当前准备流程只读系统注册的恢复环境，使用独立任务镜像启动；系统还原在 BCDBoot 后修复并核验目标恢复环境注册和新加载器关联，终态只清理本任务临时对象。
在线还原仅应用镜像、预装预安装环境自动还原固定索引 1，均不能等同恢复环境完整任务链；具体历史路径差异见 [v1.9.9 流程快照](./docs/20260930-1925-备份还原完整流程-GPT-6.md)，当前行为须对照源码。

### V1 支持的四种任务

| 任务 | 作用 | 破坏性 |
|---|---|---|
| `probe` | 只验证任务、载荷、卷挂载和恢复环境自动入口 | 否 |
| `backup` | 从现有 Windows 分区用 DISM 捕获镜像 | 否（写入指定镜像卷） |
| `restore-existing` | 还原到原 Windows 分区并修复引导 | 是 |
| `create-secondary` | 还原到独立分区并加入第二启动项（双系统） | 是 |

### V1 范围边界

**支持**：Windows 10/11、UEFI + GPT、NTFS 系统分区、镜像、Windows 自带恢复环境（也支持自定义预安装环境）、
DISM 作为镜像引擎、BCDBoot/启动配置数据作为启动管理。

**明确不做**（不要擅自扩展成完整 Ghost/预安装环境工具）：分区布局重构、网络备份、增量/差异镜像、
Legacy BIOS、自动修改 BitLocker。

**实现限制**：第二系统盘符绑定的 `secondary_registry::gpt_mapping()` 只支持 GPT 盘
（blob 为 `DMIO:ID:` + PARTUUID 小端 16 字节）；**MBR 盘（12 字节 blob：磁盘签名 + 分区偏移）走不了这条链**，
`repair()` 会因 GUID 解析失败而报错。与「UEFI + GPT」的支持边界一致，但接手方在 MBR 机器上要知道这是硬边界，
不是可降级的路径。机制详见 [盘符与MountedDevices-经验总结](./docs/盘符与MountedDevices-经验总结.md)。

---

## 二、代码结构

产品代码 **全部是 Rust**，两个 crate，约 1.66 万行。依赖只有 `serde` / `chrono` / `serde_json` /
`sha2` / `thiserror` / `uuid` —— **没有任何 .NET/dotnet/C# 运行时依赖**。

```
crates/
├── backuprestore-core/     库：任务模型、载荷契约、元数据、SHA-256、错误类型
└── backuprestore-cli/      可执行：原生 GUI + 准备 + WinRE 恢复环境
    ├── native_gui.rs       原生 Win32 GUI（8325 行）
    ├── main.rs             入口与任务分发（3080 行）
    ├── windows_prepare.rs  正常 Windows 侧准备：校验/注入载荷/设置一次性 WinRE 启动（2136 行）
    ├── text_parsing.rs     命令行输出（DISM/bcdedit/mountvol）本地化解析（1015 行）
    ├── online_operation.rs 在线候选镜像事务、回读及副档同步
    ├── windows_command.rs  作业对象约束的命令与进程树管理
    ├── recovery_progress.rs WinRE/在线进度上报
    ├── winre_payload.rs    WinRE 载荷组织
    └── winre_registration.rs 系统还原后的注册与恢复关联核验
```

PowerShell / .NET 只出现在**测试脚手架**（`tools/win-clicker/`）和一次性探针（`.test-artifacts/`），
不进产品、不影响交付物。

---

在线追加先操作候选副本，需额外空间；失败保留候选及日志。正常完成要求镜像和副档回读都通过。
早期在线路径验收只覆盖独立目录部署；该历史轮次未执行格式化或重启。本轮恢复环境系统验收见上方最新文档。历史详情见[修复与验证记录](./docs/20260930-2130-在线执行安全与副档一致性修复-GPT-6.md)。

## 三、构建

构建目标是 **Windows ARM64**（`aarch64-pc-windows-msvc`），静态链接 CRT。

> **📌 用户 2026-10-15 拍板：构建完全走虚拟机内 Windows（唯一路径，macOS 侧不再是发布路径）。**
>
> 沿用 2026-10-12 的实测依据：macOS 侧 clean 全量构建只快 1.8 倍（4.60s vs 8.24s，绝对差 3.6 秒），
> 对开发循环没有意义；而代价是质变的——**macOS 有 59% 的代码根本编译不到**
> （8 个 `#[cfg(windows)]` 模块共 15696 行），那 138 项绿灯对它们证明力为零。
> 2026-10-12 引导修复实机验证时，**每一个阻断缺陷都在 macOS 编译不到的文件里**。
> 2026-10-15 同轮修复：`windows/build-windows.ps1` 的 `Invoke-Cargo` 此前未设
> `+crt-static`（该 flag 只存在于 macOS 侧 `build-win.sh`），虚拟机内路径产物缺静态 CRT——
> 现已幂等补齐，静态链接 CRT 是本路径的必需组成部分。

### 常规构建 + 部署 + 核对（虚拟机内，一条命令）

```powershell
# 在 VM 的 Windows 11 里执行
& X:\windows\build-windows.ps1 -Architecture arm64 -Deploy
```

一条命令完成：构建 → 打包 → 部署到两个运行目录 → 逐个核对 4 个 SHA-256。
不部署就去掉 `-Deploy`。

部署目标与判据：

| 目标 | 定位 | 文件 |
|---|---|---|
| **唯一部署位 `H:\brwork\`** | 所有启动入口都从这里起；任务存储随之落在 `H:\brwork\tasks` | `BackupRestore.exe` + `Recovery.exe` + `winpe-winpeshl.ini` + `wimlib-imagex.exe` + `libwim-15.dll` |

> ⚠️ **不再往 C: 部署 exe**（2026-10-24 用户裁定）：历史部署位
> `C:\Users\Public\backupRestore-package` 停止写入，`build-win.sh --deploy` 与
> `windows/build-windows.ps1 -Deploy` 每次部署都会清掉其残留。原因：备份 C: 会把
> 该目录连旧 exe 一起打进镜像，A 入口还原 C: 后自启 GUI 从那里带回旧版二进制
> （2026-10-24 实测自启 v2.6.31 而 H: 里是 v2.6.32）；自启项（用户 `Run` 键
> `BackupRestoreTest`）同轮已改指 `H:\brwork\BackupRestore.exe`，重启实测自启
> 进程 PATH = H:。

> ⚠️ **任务目录跟着"从哪启动"走**（2026-10-24 用户裁定「TaskStore 搬离 C:」）：
> TaskStore 的根 = 可执行文件所在目录，任务目录 = `<运行目录>\tasks`。
> 从 C: 包目录起 → 任务落回 C:，会被 A 入口还原系统盘一起回滚，还会让镜像把旧任务状态带回来。
> 所以启动一律走 `H:\brwork`；包目录的 `\tasks` 已于 2026-10-24 清空（4 个终态任务迁入 `H:\brwork\tasks`）。

**复制成功不算数**，判据只有"四个文件的 SHA-256 与刚构建的成品逐个相等"。
2026-09-30 踩过「只更新包目录、运行目录还是旧件」的坑，唯一症状是镜像元数据里的
`programVersion` 偏低，静态检查看不出来。

可用 `-CargoTargetDir` / `-OutputRoot` / `-DeployPackageDir` / `-DeployWorkDir` 覆盖默认路径。

### 跑测试

```powershell
cargo test --manifest-path X:\Cargo.toml            # 全量
cargo test --manifest-path X:\Cargo.toml -p backuprestore-cli
```

> ⚠️ **macOS 侧 `cargo test` 的绿灯不能作为这些代码的验证依据。**
> `online_operation`、`native_gui`、`windows_prepare`、`windows_command`、
> `recovery_progress`、`pe_operation`、`boot_entry`、`recovery_fault`
> 八个模块都是 `#[cfg(windows)]`，macOS 上静默跳过。
> 改到它们必须在 Windows 侧跑一遍测试——macOS 抓不到的错包括类型不匹配、
> borrow 检查、以及本会话全部 7 个阻断缺陷。

### macOS 侧（已废止为发布路径，仅限本地快速类型检查）

用户 2026-10-15 拍板「编译完全走虚拟机内 Windows」。macOS 侧 `./build-win.sh`
**不再用于构建/部署发布产物**；仅保留供本地快速确认 `backuprestore-core` 等跨平台
代码"没把公共库改崩"（`cargo check` 级用途），产物不得部署、不得作为发布版。
历史实测对比（两边产物均 14 DLL、零 CRT 外部依赖、大小差 16KB/89.6KB）见
[macOS 交叉编译与虚拟机内原生编译实测对比](./docs/20261012-1640-macOS交叉编译与VM内原生编译实测对比-Fable5.md)。
**macOS 的绿灯对 `#[cfg(windows)]` 模块（图形用户界面/准备/恢复/在线/预安装环境/boot_entry）零证明力。**

> ⚠️ **`+crt-static` 不能漏**：漏了产物会依赖 `VCRUNTIME140.dll` 与 `api-ms-win-crt-*`，
> 进不了精简恢复环境/预安装环境（退出码 0xC0000135，症状=零输出零副作用）。虚拟机内
> `build-windows.ps1` 已幂等内置该 flag；手打裸 `cargo build` 才会踩到。

构建规则、静态 CRT、Windows 软件开发工具包路径等细节见 `docs/20260819-2245-windows-build.md`；
虚拟机内编译环境与踩坑见 [2.3.9 引导修复与虚拟机原生编译路径](./docs/20261012-1510-2.3.9引导修复与VM原生编译路径-Fable5.md)。

> **环境是否齐全**（2026-10-13 实测，两条路径都实跑过）：见
> [macOS 与虚拟机编译工具链核查](./docs/20261013-1725-macOS与虚拟机编译工具链核查-DeepSeek-V4.1-Flash.md)。
> 结论：工具齐全、无必需下载；macOS 与虚拟机三处 Rust 已统一为 **rustc 1.98.1**；
> **虚拟机两个 profile 的 cargo 源已配 USTC 镜像**（含可直接迁移的 4 行 TOML 与实证）。
> 另记两个入口坑：`X:` 只在虚拟机交互会话存在（SYSTEM 通道要用 UNC `\\Mac\backupRestore`），
> 且 `if exist \\Mac\backupRestore\NUL` 恒为假、不能用来判共享可达。

---

## 四、目录说明

| 路径 | 内容 |
|---|---|
| `crates/` | 产品源码（Rust，唯一产品语言） |
| `docs/` | 流程、开发进度、操作记录及历史方案，索引见 `docs/README.md` |
| `tools/win-clicker/` | 测试脚手架：操控虚拟机键鼠的自动化通道（PowerShell，不进产品） |
| `build-win.sh` | macOS 交叉构建（**已废止为发布路径**，仅限本地 `cargo check`；构建唯一入口是虚拟机内 `windows/build-windows.ps1 -Deploy`） |
| `artifacts/` | 预安装环境构建产物（`BackupRestorePE.iso/wim`、`BaseWinPE.iso`） |
| `pe-assets/` | 预安装环境源镜像 / SDI |
| `windows/` | Windows 侧辅助脚本与 `winpeshl.ini` |
| `target/` | Rust 构建产物（8G+，不入库） |
| `.test-artifacts/` | 测试证据与一次性探针（5G+，已 gitignore） |
| `VERSION` | 当前版本号 |
| `AGENTS.md` | 开发验证硬约束（**改启动项前必须建快照**） |

---

## 五、开发约束（必读）

1. **动启动项前必须建快照**。修改启动配置数据、默认项、一次性启动序列、恢复环境注册或预安装环境部署之前，
   先创建并核验 Parallels 快照，记录快照 ID、目的和变更范围。快照失败不得继续。详见 `AGENTS.md`。
2. **启动成功 ≠ 备份还原成功**。验证要跑到 DISM 实际完成、启动项修复、回正常 Windows 为止。
3. **测试只能在可回滚的虚拟机快照里做**，尤其是 `restore-existing` / `create-secondary`。
4. **不触碰 C:** 的范围是：不把 `C:` 作为 backup 源分区或还原目标分区；读取 C: 状态、使用其
   恢复环境/启动配置数据、写任务和日志是允许的。
5. **大文件下载前先查网络**。热点环境必须逐次取得用户授权。
6. 每次改动后：本地测试 → Windows ARM64 构建 → 部署 → 核对哈希。

---

## 六、文档索引

- **首选入口**：[项目全景与知识浓缩](./docs/20261011-2350-项目全景与知识浓缩-Hy4.md)（新会话先读）
- **完整目录**（逐份说明）：根目录 [`文档索引.md`](./文档索引.md)
- **主题导航**：[`docs/README.md`](docs/README.md)

高频主题入口：

| 主题 | 文档 |
|---|---|
| 操作虚拟机键鼠（自动化点击/按键/截图） | [`vm-input-control-guide`](./docs/20260927-0016-vm-input-control-guide.md) |
| ARM64 构建规则 | [`windows-build`](./docs/20260819-2245-windows-build.md) |
| 恢复环境引导 / PD27 固件回归证明 | [`winre-boot-failure-parallels27`](./docs/20260925-0025-winre-boot-failure-parallels27.md) |
| S 盘弹窗 / unmount 竞态与卷路径改造 | [`S盘自动打开问题定位与修复方案`](./docs/20260929-2014-S盘自动打开问题定位与修复方案.md) |
| 历史开发路线图 | [`development-roadmap`](./docs/20260926-2337-development-roadmap.md) |
| v1.9.9 流程快照（**历史，非执行合同**） | [`备份还原完整流程`](./docs/20260930-1925-备份还原完整流程-GPT-6.md) |
| 完整需求原文 | [`Windows 一键系统备份还原 V1——完整开发需求.md`](Windows%20一键系统备份还原%20V1——完整开发需求.md) |

> 轮次快照（40 轮 × 2 份「当前开发进度/下一步待实现」）已归档到 `docs/90-archive/round-snapshots/`，
> 只在追溯某版本当时状态时查看，日常不必读。

---

## 七、历史开发进度（截至 2026-09-30）

> **已归档。** 2026-09-30 及更早的开发流水账（约 300 行：v1.8.x 各轮修复记录、S 盘弹窗排查、已知限制等）已整体移入
> [docs/90-archive/历史开发流水账-20260930.md](./docs/90-archive/历史开发流水账-20260930.md)，内容未作改写。
> 其中「尚未闭环」「待实施」、盘符、虚拟机版本和清理建议均为当时快照，**不应直接执行**；当前状态见本 README 上方进度入口。

仓库：https://github.com/pixian5/backupRestore
