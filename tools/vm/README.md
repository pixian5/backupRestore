# tools/vm —— 实机用例脚本

> 在测试虚拟机（Parallels「Windows 11」）上验证产品行为的脚本。每个用例脚本**自己能跑、自己给结论**：
> 最后一行 `<用例名>=PASS|FAIL`，退出码与之一致。
> 2026-11-25 从被忽略的 `.test-artifacts/` 目录收进仓库——此前「改动-复验对应表」要求的实机项没有可重跑的脚本。

## 先读这五条

1. **虚拟机设置一律不改**（用户 2026-11-24 明令）。脚本只会在虚拟机被「空闲自动暂停」停住时 `prlctl resume`。
2. **动界面前脚本自己会问「有没有人在用」**（`tools/vm/user-idle.sh`）；有人在用就中止。
   你自己的上一次自动化点击也算输入——默认连着跑第二个界面脚本要等空闲满 120 秒（无人值守连续自测若经用户明确许可可设 `BR_SKIP_IDLE=1` 跳过等待，默认阈值不改）。
3. **判据只认结构化字段与原样取回的文件**。要读客体文件里的原文，一律经 `fetch`（共享盘原样拷出），
   不要拿 `vrun` 的输出（它只保留可打印字符）去搜内容。
4. **脚本的输出只落到文件里，查看时按字节处理**（`LC_ALL=C grep -a` / `LC_ALL=C cut`）。助手回传的文字里混有
   系统本地编码的中文：2026-11-25 在外层接了一个按通用编码工作的 `cut`，它报「非法字节序列」退出，
   连带把正在跑的脚本弄断在「点完否」之后。
5. **真实鼠标点击落在屏幕坐标上最上层的窗口**，不一定是目标程序。点击辅助脚本（`uiclick.ps1`）会先把目标窗口
   提到最上层、再核对「这个坐标下是不是目标窗口」，不是就拒绝并回报 `CLICK=COVERED`。
   背景：2026-11-25 新启动的界面程序停在「设置」窗口后面，两次回报「已点击」的点击落进了「设置」
   （点到了账户头像、跳到了「帐户」页；设置项没有被动到）。另外，经 `-Extra` 传给常驻助手的整串参数**末尾不能是冒号**
   （会被弄坏、外层脚本报「参数缺值」）——`gui_select_combo` 已经替你去掉盘符末尾的冒号。

## 脚本一览

| 脚本 | 验证什么 | 耗时 | 副作用 / 前提 |
|---|---|---|---|
| `selftest.sh` | 工具自己的离线自检（不需要虚拟机） | 几秒 | 无。改了 `lib/`、`judge/` 或样本之后跑 |
| `run-kept.sh <脚本> [参数]` | 带保活地运行另一个脚本 | — | 无 |
| `cli-probe.sh` | 四条命令行探测命令的输出字段（列卷带驱动器类型、光驱认成 cdrom） | 10 秒 | 只读 |
| `data-guard.sh` | 完整 Windows 镜像写数据卷必须在准备期被拒 `BR-IMG-009` | 1 秒（副档丢了约 3 分钟） | 带 `--no-reboot`，不会重启 |
| `orphan-race.sh new both` | 准备进行中：放弃被拒 `BR-TSK-036` + 启动界面程序守卫不动 + 准备成功 + 备份时按 P 只记「已忽略」 | 约 4 分钟 | 重启两次；产生测试镜像与 20 GB 占位文件 |
| `orphan-race.sh old gui` | 在有缺陷的旧二进制（v2.10.8 及以前）上复现 | 约 1 分钟 | 准备失败，不重启 |
| `pause-backup.sh` | 备份时按 P 只记「已忽略」；声明行落盘；产出测试镜像 | 约 4 分钟 | 重启两次；产生测试镜像与占位文件 |
| `pause-restore.sh` | 数据卷离线还原：按 P 停住、再按才继续；占位文件回来、被改坏的文件复原 | 约 5 分钟 | 重启两次；先跑过 `pause-backup.sh` 或 `orphan-race.sh new both` |
| `data-restore.sh` | 数据卷离线还原（不按键）的哨兵法 | 约 2 分钟 | 重启两次；测试卷上要有夹具目录 `F:\BRDATA` |
| `gui-data-restore.sh [镜像] [offline\|online\|online-format]` | **界面驱动**的「还原数据到非 C 盘」全链。`offline`（默认，不格式化）：从点按钮、界面自己派发准备，到回到 Windows；`online`：在线不格式化；`online-format`：在线勾选格式化（清哨兵并校验卷序列号变更） | offline 约 4 分钟；online / online-format 约 2 分钟 | 都会真的还原；offline 重启两次；同样要有 `F:\BRDATA` |
| `gui-fix-check.sh` | 进入「还原数据到非 C 盘」默认目标不是光驱；A 入口三道确认最后点否、零副作用 | 约 3 分钟 | 只走到对话框为止，不创建任务 |
| `gui-choice-text.sh` | 离线选择框的正文跟着「格式化目标分区」变 | 约 2 分钟 | 读到正文后点取消，不创建任务 |
| `gui-backup.sh cancel` | **备份入口**的「破坏性确认」点「否」必须停下：系统盘分支不得弹处理方式选择框、在线分支不得开始备份 | 约 3 分钟 | 不建任务、不写镜像。v2.10.11 之前的二进制上必须判失败（缺陷复现；弹出来的窗口会被收掉） |
| `gui-backup.sh online` | **界面驱动**的在线备份全链：测试卷 → 镜像写到程序盘，从点按钮到「执行完成」 | 约 2 分钟 | 写一个很小的镜像（测试卷里主要是零文件），跑完删掉；不重启 |
| `gui-backup.sh system` | **界面驱动**的系统盘备份经恢复环境全链 | 约 20 分钟 | 重启两次。**先查空闲空间**（宿主 ≥ 估算镜像 × 2.5、测试卷 ≥ × 1.5），不够直接中止。⚠ 写好后尚未上机验证（2026-11-25 宿主空间不足） |
| `gui-negative.sh` | 界面「说不」矩阵：九个确认框各点一次「否」或「取消」，每一次都必须零副作用（界面记下「已取消」、不下发、任务目录与启动配置不变、无残留窗口） | 约 8 分钟 | 不建任务、不写镜像、不重启。已在 v2.10.11（`16db638b…`）上完成变异验证（`NEG_ONLY=a-first-no NEG_MUTATION_FIRST_YES=1` 判失败）与全量九项通过（2026-11-26 22:20） |
| `gui-matrix.sh [--quick]` | 界面全矩阵（`tools/gui-check/run.sh` 套保活，结束后恢复 14 号字、中文） | 15 分钟（`--quick` 约 1 分钟） | 改界面设置，结束时会恢复 |
| `vm-unit.sh <标签> [过滤串]` | 虚拟机里的 Windows 原生单元测试（定向或全量） | 定向十几秒；全量几分钟 | 无 |
| `mutation-guard.sh` / `mutation-progress.sh` | 对应单元测试的变异验证 | 约 1 分钟 | 临时改工作树里的源码文件，结束原样恢复 |
| `rebuild-deploy.sh [--check]` | 虚拟机内构建部署 + 三处哈希 + 部署前置检查 | 几分钟 | **先提交后部署**；没有源码改动不要跑（重编会改变二进制哈希） |
| `clean-fixtures.sh` | 清掉上面各用例留下的夹具与临时文件 | 10 秒 | 删之前先做启动配置健康核对 |

## 改了产品代码之后跑哪个

先问对应表：`python3 tools/test/reverify-map.py --since <上次实机验证的基线标签> --worktree`——输出里每一项都写着对应的脚本。

## 目录

- `lib/vm.sh`：通道函数（`vrun` / `vrun_user` / `vps` / `elev` / `ensure_helper` / `stage_gui_helpers` …）。
- `lib/common.sh`：用例公共函数（`fetch` / `json_field` / `bcd_readings` / `require_idle` / `wait_terminal` /
  `decode_text` / `control_text` / `run_prepare` / `verdict` …）。用例脚本开头 `source "$HERE/lib/common.sh"`。
  三条硬约定写在函数注释里：文件在不在一律走 `guest_exists`（在 / 不在 / **读不到**是三种结果）；
  有副作用的命令只在「客户端已退出、输出为空、再等几秒也没有新任务目录」时才重新下发；
  靠变量传结果的函数（`run_prepare`）调用处不接管道。
- `judge/`：判读程序（从脚本里抽出来的 Python，可离线验证）：`cli-probe.py` 命令行冒烟的输出；
  `restore-log.py` 恢复环境日志里某一个任务的段落（还原与备份用例共用）；`gui-dispatch.py` 界面日志里某一时点之后的记录；
  `windows.py` 「点之前 / 点之后」两份窗口清单的比较（多出了什么窗口）；`wim-info.py` 镜像信息命令的输出（几个索引、多大、哪个版本写的）；
  `combo-selected.py` 下拉框当前选中的条目。
- `fixtures/`：真实抓到的输出样本，供 `selftest.sh` 用。
- `ps/`：在虚拟机里执行的包装脚本（测试、构建、同步界面辅助脚本）。**保持纯 ASCII**（PowerShell 5 对无签名的非通用编码文件会读错）。
- 界面辅助脚本（点击、枚举控件、读写控件文字、选下拉项、哨兵法的前后置检查）在 `tools/gui-check/ps/`；
  三道确认的驱动是 `tools/gui-check/restore-confirm.sh`——**凡是会真正下发的那一次「是」，之前都必须有 `BR_ALLOW_RESTORE=yes`**
  （离线流程是最后一道确认；纯在线流程没有第二道确认，破坏性确认上的「是」就是下发，同样要授权；离线自检里有这条的静态检查与变异验证）。
  `lib/common.sh` 里对应的函数：`gui_click` / `gui_must_click` /
  `gui_wait_window` / `gui_windows` / `gui_set_image_path` / `gui_combo_items` / `gui_combo_selected` / `gui_select_combo`。
  ★ 历史注：跨进程 `GetWindowTextW` 会返回 `Edit` 控件创建时的占位文案，此前清单里的编辑框文本读取不到；
    `tools/gui-check/ps/uidlglist.ps1` 已于 2026-11-26 修复为对 `Edit` 控件发送 `SendMessageTimeoutW`（`WM_GETTEXT`），
    并附带输出 `Button` 控件的 `def=True|False` 默认按钮标记（`gui-backup.sh cancel` 与 `gui-negative.sh` 已实测利用）。

证据（日志、原样取回的文件、判定文件）写在 `.test-artifacts/vm-runs/evidence/`（已 gitignore），文件名带「用途、版本号、时间戳」前缀——
每次运行都是一组新文件，既不会覆盖上一轮，也不会读到上一轮的旧文件。
