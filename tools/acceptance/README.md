# Windows ARM64 故障验收

只对明确选定的测试分区执行。默认目标是本项目的 T: 测试卷身份，盘符由现场 GUID 查找；主系统分区会被脚本拒绝。系统镜像模式用于已授权的独立第二系统测试分区，不会自动选择其它卷。

在项目虚拟环境执行 `tools/acceptance/run.py`。每个用例必须有新的 `snapshot`：脚本串行轮换最多两个本项目快照，未知来源快照会阻断；所有原始结果留在 `.test-artifacts/<run>/<case>/`。

```bash
.venv/bin/python tools/acceptance/run.py snapshot --case disk-full
.venv/bin/python tools/acceptance/run.py prepare --case disk-full --fault acceptance:apply:disk-full
.venv/bin/python tools/acceptance/run.py status --case disk-full
.venv/bin/python tools/acceptance/run.py retry --case disk-full
.venv/bin/python tools/acceptance/run.py retry --case disk-full
.venv/bin/python tools/acceptance/run.py verify --case disk-full
```

`prepare` 启动异步准备，必须确认阶段为 `prepared` 后才调用 `retry`。第一次空间不足应当返回 DISM 错误 112 且阶段保留在 `target-erased`；第二次从同一任务重新格式化并恢复。`verify` 回读任务状态、夹具摘要、哨兵删除、主系统启动配置和恢复原件。先看错误内容，不能只把非零退出码当通过。

`acceptance:apply:error` 是持续错误，直到在任务目录创建 `fault-released`；`errors.ps1 -Root <本用例目录>` 检查连续失败、镜像丢失、共享读取错误和同大小镜像损坏，然后解除故障并恢复同一任务。该脚本应接在本用例首次到达 `target-erased` 后执行。

## 真实断电

`--boot` 进入真实任务恢复环境。`acceptance:cleanup-matrix:hold` 依次暂停在清理启动请求、删除启动对象、删除载荷、成功写入前后；`acceptance:boot-matrix:hold` 暂停在 BCDBoot 前、运行中、完成后，以及注册写入后的检查点。每个检查点先持久化标记，因此恢复时只触发一次。运行中检查点先让真实 BCDBoot 执行 10 毫秒，再暂停仍存活的进程；必须看到 `BCDBOOT_PROCESS_SUSPENDED` 与对应检查点，不把已退出进程算作命中。

```bash
.venv/bin/python tools/acceptance/run.py snapshot --case cleanup-matrix
.venv/bin/python tools/acceptance/run.py prepare --case cleanup-matrix --boot --fault acceptance:cleanup-matrix:hold
.venv/bin/python tools/acceptance/run.py capture --case cleanup-matrix --checkpoint cleanup-sequence
# 必须读取截图或客体日志，确认已经到达对应检查点。
.venv/bin/python tools/acceptance/run.py cut --case cleanup-matrix --checkpoint cleanup-sequence
```

恢复完成后的清理中断会返回正常系统。再次调用产品 `resume <app-root> <task-id>` 或打开该任务目录中的程序，只执行清理；每次新的启动配置操作之前仍需新快照。宿主命令不能与快照创建/删除并行。`capture` 只保留缩小的 JPEG（压缩图片），`cut` 记录时间、截图摘要及真实 `--kill` 结果。

`acceptance:<point>:error-once` 只失败一次，真实恢复窗口可以点击“重试”；`error` 持续失败且不会自动重启；`reboot` 是软件重启注入，不能算真实断电。BCDBoot 前后检查点也不等于覆盖其内部任意写入瞬间。

## 续跑入口和补偿失败

Windows 原生测试中的 `boot_entry::fault_tests::missing_payload_conflict_and_cleanup_retry` 默认忽略。必须先成功完成一个小测试卷任务，再提供该程序目录 `BR_ACCEPTANCE_ROOT` 及刚核验的 `BR_ACCEPTANCE_SNAPSHOT`，用 `--ignored --exact ... --nocapture --test-threads=1` 显式运行。它实际创建本项目启动对象，验证缺失/损坏载荷被拒绝、其它启动请求不被覆盖、清理补偿失败不删除未知文件，并重试清理后比较完整原始启动配置。

## 状态写入与证据

核心回归测试在状态事务已持久化、仅 task.json 已更新、两份视图均更新但事务尚未删除三个边界重建现场，验证重新加载后统一得到成功状态；不是物理断电测试。

```bash
.venv/bin/python tools/acceptance/run.py collect --case cleanup-matrix
.venv/bin/python tools/acceptance/run.py pack --case cleanup-matrix
```

压缩包只包含小型日志、状态、截图和文字证据，排除 WIM（Windows 映像文件）、虚拟磁盘和完整注册表。收尾时恢复本次测试临时调整的虚拟机空闲暂停设置，并记录剩余快照与版本摘要。

## Windows 原生回归

采用 `build-win.sh` 的同一链接器及静态运行库参数，运行 `cargo test --workspace --all-targets --offline --target aarch64-pc-windows-msvc --no-run --message-format=json`，从编译事件中提取 `profile.test=true` 的两个 `executable`，转换为客体能读取的共享路径并保存 JSON 数组清单。不要根据文件时间猜选测试程序。

在正常 Windows 的 SYSTEM（系统账户）通道调用 `native.ps1 -Manifest <清单路径> -Evidence <证据目录>`。它按工作卷 GUID 定位，复制并比对摘要后，在虚拟机本地串行执行两个测试程序，保存输出并检查退出码。破坏性夹具仍须单独指定目标、快照和测试名。避免把完整脚本拼成过长的 `EncodedCommand`；宿主通道挂住且未产生客体输出时，不视为已执行。

`tests::secondary_menu_repair_native` 验证第二系统重建后的菜单收尾。先核验新快照并挂载 EFI 系统分区，提供 `BR_ACCEPTANCE_SNAPSHOT`、`BR_ACCEPTANCE_ROOT`、`BR_ACCEPTANCE_TASK`、`BR_ACCEPTANCE_EFI`、`BR_ACCEPTANCE_SECONDARY`，使用 `--ignored --exact tests::secondary_menu_repair_native --nocapture --test-threads=1`。该夹具要求已成功的第二系统任务，调用产品同一收尾函数并回读启动菜单；不重做镜像应用。

## 系统创建与实际启动

系统模式的 `case.ps1 verify` 只验证终态、格式化哨兵及主系统恢复原件，返回 `systemVerificationRequired=true`。继续调用 `system.ps1 -Action created -Config <用例配置> -Baseline <同一镜像的已验证基线目录>`，核对两系统关键文件、夹具、原默认项、仍存在的原菜单顺序、独立恢复对象及临时对象清理。基线必须包含 `primary-before-secondary.json` 和 `fixture.json`，不能换用其他镜像的摘要。

实际启动第二系统后以 `-Action booted` 再核验系统卷 GUID、C: 盘符、当前加载器、恢复注册、桌面及核心服务。任务成功和离线文件检查不能替代这一步。标准恢复菜单进入和返回仍需另建快照并保存实际画面与返回后的系统身份。

BCDBoot 完成后的特定中断点可能清除一次性请求，先启动已恢复的系统；此时使用产品 `resume` 显式续跑。记录自动与显式续跑的区别，不把六个检查点统称为自动续跑。
