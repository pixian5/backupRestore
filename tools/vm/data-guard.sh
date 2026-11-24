#!/bin/bash
# tools/vm/data-guard.sh —— 「完整 Windows 镜像不能当数据镜像写进数据卷」的拒绝判据（v2.10.8 起的门禁）。
#
# 为什么单独一个用例：10-18 的安全闸曾拦得太宽，把合法的「数据镜像 → 数据卷」也拦了；修的时候不能把闸拆没了。
# 这里验证的是拆宽之后**危险的那一半仍然被拦**：拿一份完整 Windows 镜像（带它自己的副档）按「还原到原卷」
# 离线写进非系统卷，必须在准备期被拒 BR-IMG-009，不创建任务、不写任何卷、不动启动配置。
#
# 安全性：命令带 --no-reboot —— 即使拒绝失效（产品回归），准备也只会建出任务与启动项而**不会重启**，
#   目标卷上的写入只发生在重启进恢复环境之后。脚本发现「居然成功了」时，会立刻用 abandon-task 把这个任务收掉
#   （撤销启动项、删载荷），再核对启动配置读数。
#
# 用法：tools/vm/data-guard.sh [完整 Windows 镜像路径]     （默认用测试机上的基线系统镜像）
# 耗时：镜像带副档时不到 1 秒；副档丢了会走只读挂载探测（40 GB 镜像约 3 分钟），不是卡死。
# 退出码：0 = 被拒且无副作用；1 = 判据不满足；其它 = 步骤中止。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
SYS_WIM="${1:-$SYS_IMAGE}"
run_name "data-guard"

say "STEP=0 deployed binary"
deployed_hashes | head -1
say "STEP=1 readings before"
TASKS0=$(task_count)
bcd_readings > "$EV/$NAME-bcd-before.txt"
vrun "dir /b ${SYS_WIM}.index-1.metadata.json" 2 | tr -cd '\11\12\40-\176' | grep -a -c 'metadata.json' | sed 's/^/SIDECAR_FILES=/'

say "STEP=2 try to restore a complete Windows image onto the data volume with --no-reboot (must be refused)"
start=$(date +%s)
vraw "$EXE prepare --operation restore-existing --source-drive $SOURCE_DRIVE --target-drive $SOURCE_DRIVE --boot-menu-name BR-GUARD --image-path $SYS_WIM --wim-index 1 --skip-format --allow-destructive --no-reboot --json" "$NAME-prepare.json" \
  || { say "ABORT: the prepare command gave no output marker (reading failed is not a verdict)"; exit 3; }
say "PREPARE_RETURNED after $(( $(date +%s) - start ))s"
SUCCESS=$(json_field "$EV/$NAME-prepare.json" success)
CODE=$(json_field "$EV/$NAME-prepare.json" error_code)
TASK=$(json_field "$EV/$NAME-prepare.json" task_id)
printf 'PREPARE success=%s error_code=%s task_id=%s\n' "$SUCCESS" "$CODE" "$TASK" | tee "$EV/$NAME-verdict.txt"
printf 'PREPARE_ERROR=%s\n' "$(json_field "$EV/$NAME-prepare.json" error | head -c 200 | tr '\n' ' ')" | tee -a "$EV/$NAME-verdict.txt"

if [ "$SUCCESS" = "True" ]; then
  # 拒绝失效：任务已经建出来（没有重启）。立刻收掉它，免得留着一个「下次重启会把系统镜像写进数据卷」的任务。
  say "REFUSAL_MISSING: the prepare succeeded; collecting the task it created (--no-reboot was set, nothing was written)"
  [ -n "$TASK" ] && vraw "$EXE abandon-task H:\\brwork $TASK --json" "$NAME-abandon.json"
  sleep 2
fi

say "STEP=3 readings after"
TASKS1=$(task_count)
bcd_readings > "$EV/$NAME-bcd-after.txt"
SAME=$(bcd_same "$EV/$NAME-bcd-before.txt" "$EV/$NAME-bcd-after.txt")
printf 'TASKS_BEFORE=%s TASKS_AFTER=%s\n%s\n' "$TASKS0" "$TASKS1" "$SAME" | tee -a "$EV/$NAME-verdict.txt"

say "STEP=4 judge"
fail=0
if [ "$SUCCESS" = "False" ] && [ "$CODE" = "BR-IMG-009" ] && [ -z "$TASK" ]; then
  verdict DATA_GUARD_REFUSED PASS "refused with BR-IMG-009, no task id"
else
  verdict DATA_GUARD_REFUSED FAIL "success=$SUCCESS error_code=${CODE:-none} task=${TASK:-none}"; fail=1
fi
if [ "$TASKS0" = "$TASKS1" ] && [ "$SAME" = "BCD_READINGS_EQUAL=True" ]; then
  verdict DATA_GUARD_SIDE_EFFECTS PASS "task count and boot configuration unchanged"
else
  verdict DATA_GUARD_SIDE_EFFECTS FAIL "tasks $TASKS0 -> $TASKS1, $SAME"; fail=1
fi
[ $fail -eq 0 ] && verdict DATA_GUARD PASS || { verdict DATA_GUARD FAIL; exit 1; }
