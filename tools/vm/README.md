# tools/vm —— 实机用例脚本

> 在测试虚拟机（Parallels「Windows 11」）上验证产品行为的脚本。每个用例脚本**自己能跑、自己给结论**：
> 最后一行 `<用例名>=PASS|FAIL`，退出码与之一致。
> 2026-11-25 从被忽略的 `.test-artifacts/` 目录收进仓库——此前「改动-复验对应表」要求的实机项没有可重跑的脚本。

## 先读这三条

1. **虚拟机设置一律不改**（用户 2026-11-24 明令）。脚本只会在虚拟机被「空闲自动暂停」停住时 `prlctl resume`。
2. **动界面前脚本自己会问「有没有人在用」**（`tools/vm/user-idle.sh`）；有人在用就中止。
   你自己的上一次自动化点击也算输入——连着跑第二个界面脚本，要等空闲满 120 秒。
3. **判据只认结构化字段与原样取回的文件**。要读客体文件里的原文，一律经 `fetch`（共享盘原样拷出），
   不要拿 `vrun` 的输出（它只保留可打印字符）去搜内容。

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
| `gui-fix-check.sh` | 进入「还原数据到非 C 盘」默认目标不是光驱；A 入口三道确认最后点否、零副作用 | 约 3 分钟 | 只走到对话框为止，不创建任务 |
| `gui-choice-text.sh` | 离线选择框的正文跟着「格式化目标分区」变 | 约 2 分钟 | 读到正文后点取消，不创建任务 |
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
- `judge/`：判读程序（从脚本里抽出来的 Python，可离线验证）。
- `fixtures/`：真实抓到的输出样本，供 `selftest.sh` 用。
- `ps/`：在虚拟机里执行的包装脚本（测试、构建、同步界面辅助脚本）。**保持纯 ASCII**（PowerShell 5 对无签名的非通用编码文件会读错）。
- 界面辅助脚本（点击、枚举控件、读写控件文字、哨兵法的前后置检查）在 `tools/gui-check/ps/`；
  三道确认的驱动是 `tools/gui-check/restore-confirm.sh`。

证据（日志、原样取回的文件、判定文件）写在 `.test-artifacts/vm-runs/evidence/`（已 gitignore），文件名带「用途、版本号、时间戳」前缀——
每次运行都是一组新文件，既不会覆盖上一轮，也不会读到上一轮的旧文件。
