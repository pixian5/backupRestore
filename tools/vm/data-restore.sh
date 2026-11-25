#!/bin/bash
# tools/vm/data-restore.sh —— 「还原数据到非 C 盘」走恢复环境（离线、不格式化、不按键），哨兵法。
# v2.10.8 修的缺陷 1 的正向验证：这条链以前在准备期被「目标必须是系统卷」的闸拦死。
#
# 哨兵法：还原之前在测试卷上放一个哨兵文件、一个镜像里没有的多余文件，再把一个文件改坏；还原之后——
#   被改坏的文件必须回到镜像里的原版（证明真的写入了）；哨兵与多余文件必须还在（证明没有格式化）；
#   100 MB 载荷的哈希、卷序列号、目录里的文件数都必须不变。
# 日志判据（原样取回）：步骤 1/4–4/4 齐全；自检行 RESTORE_SELFCHECK … serial_before=<前> serial_after=<后> ntoskrnl=false。
#
# 用法：tools/vm/data-restore.sh [数据镜像路径]     （默认用测试机上的基线数据镜像；约 2 分钟，含两次重启）
# 前提：虚拟机在运行；测试卷上有夹具目录 F:\BRDATA（回归基线实测留下的：file1 / file2 / payload-100MB.bin，
#       内容与镜像里的一致）。
# 退出码：0 = 全部判据通过；1 = 有判据不满足；其它 = 步骤中止。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
DATA_WIM="${1:-$DATA_IMAGE}"
TAG="dr"
run_name "data-restore"
fail=0

say "STEP=0 deployed binaries"
deployed_hashes

say "STEP=1 preconditions"
need_guest_file "$DATA_WIM" "the data image is missing"
need_guest_file 'F:\BRDATA\sub\file1.txt' "F:\\BRDATA (the sentinel-method fixture) is missing"
say "PRECONDITIONS_OK"

say "STEP=2 boot configuration: file-level backup, verified it reads offline; readings before"
bcd_backup "$NAME" | tee "$EV/$NAME-bcd-file.txt"
bcd_readings > "$EV/$NAME-bcd-before.txt"
TASKS0=$(task_count)

say "STEP=3 nobody is using the desktop"
require_idle "$NAME-idle.txt" 120

say "STEP=4 pre-state on the test volume: sentinel, a file that is not in the image, a damaged file"
ps1 "$SHARE_PS\\c5prep.ps1" -Tag "$TAG" | grep -a -E '^PREP|__END__'
ps1 "$SHARE_PS\\c5check.ps1" -Tag "$TAG" | grep -a -E '^CHECK' | tee "$EV/$NAME-pre-check.txt"

say "STEP=5 prepare the offline data restore (reboots into WinRE by itself)"
run_prepare "$NAME-prepare.json" --operation restore-existing --source-drive $SOURCE_DRIVE --target-drive $SOURCE_DRIVE --boot-menu-name BR-DATA --image-path $DATA_WIM --wim-index 1 --skip-format --allow-destructive --json
prepare_summary "$NAME-prepare.json" "$NAME-prepare-summary.txt"
[ -n "$PREPARE_TASK" ] || { say "ABORT: prepare did not create a task"; exit 4; }
[ "$PREPARE_SUCCESS" != "False" ] || { verdict DATA_RESTORE FAIL "the prepare failed (see the summary above)"; exit 1; }

say "STEP=6 wait for the task to reach a terminal stage (reboot -> WinRE -> back)"
wait_terminal "$PREPARE_TASK" 1500 | tee "$EV/$NAME-terminal.txt"

say "STEP=7 collect evidence verbatim"
fetch "H:\\brwork\\tasks\\$PREPARE_TASK\\status.json" "$NAME-status.json" && say "fetched status.json"
fetch "H:\\BRImages\\Recovery.log" "$NAME-Recovery.raw.log" && say "fetched Recovery.log"

say "STEP=8 post-state on the test volume; readings after"
ps1 "$SHARE_PS\\c5check.ps1" -Tag "$TAG" | grep -a -E '^CHECK' | tee "$EV/$NAME-post-check.txt"
bcd_readings > "$EV/$NAME-bcd-after.txt"
TASKS1=$(task_count)

say "STEP=9 judge"
# 读数分两处：任务状态文件（两个键各一行）与恢复环境日志（判读程序 judge/restore-log.py，三个还原用例共用）。
{
  status_field "$EV/$NAME-status.json" stage | sed 's/^/FINAL_STAGE=/'
  status_field "$EV/$NAME-status.json" progress | sed 's/^/FINAL_PROGRESS=/'
  python3 -I "$HERE/judge/restore-log.py" "$EV/$NAME-Recovery.raw.log" "$PREPARE_TASK"
} | tee "$EV/$NAME-readings.txt"
rd() { sed -n "s/^$1=\(.*\)$/\1/p" "$EV/$NAME-readings.txt" | tail -1; }
chk() { fixture_field "$2" "$1"; }
STAGE=$(rd FINAL_STAGE); PROGRESS=$(rd FINAL_PROGRESS); LABELS=$(rd RECOVERY_STEP_LABELS); SC=$(rd SELFCHECK_LINE)
SERIAL_B=$(chk serial "$EV/$NAME-pre-check.txt"); SERIAL_A=$(chk serial "$EV/$NAME-post-check.txt")
FILE1_A=$(chk file1 "$EV/$NAME-post-check.txt")
SENT_A=$(chk sentinel_exists "$EV/$NAME-post-check.txt"); EXTRA_A=$(chk extra_exists "$EV/$NAME-post-check.txt")
HASH_B=$(chk payload_sha256 "$EV/$NAME-pre-check.txt"); HASH_A=$(chk payload_sha256 "$EV/$NAME-post-check.txt")
COUNT_B=$(chk brdata_file_count "$EV/$NAME-pre-check.txt"); COUNT_A=$(chk brdata_file_count "$EV/$NAME-post-check.txt")
SERIAL_B_N=$(printf '%s' "$SERIAL_B" | tr -d '-'); SERIAL_A_N=$(printf '%s' "$SERIAL_A" | tr -d '-')

if [ "$STAGE" = "success" ] && [ "$PROGRESS" = "100" ] && [ "$LABELS" = "1,2,3,4" ]; then
  verdict DATA_RESTORE_COMPLETED PASS "stage=success progress=100, STEP 1/4..4/4 all present, task=$PREPARE_TASK"
else
  verdict DATA_RESTORE_COMPLETED FAIL "stage=$STAGE progress=$PROGRESS step_labels=${LABELS:-none} task=$PREPARE_TASK"; fail=1
fi
# ★ 比对之前先确认读到的不是空串：读不到的时候「空串 == 空串」会让比对假通过。
if [ "$FILE1_A" = "BRDATA file1 v1" ] && [ "$SENT_A" = "True" ] && [ "$EXTRA_A" = "True" ] && [ -n "$SERIAL_A" ] && [ "$SERIAL_B" = "$SERIAL_A" ] \
   && [ -n "$HASH_A" ] && [ "$HASH_B" = "$HASH_A" ] && [ -n "$COUNT_A" ] && [ "$COUNT_B" = "$COUNT_A" ]; then
  verdict DATA_RESTORE_CONTENT PASS "file1 back to the image content; sentinel and extra file kept; volume serial, payload hash and file count unchanged"
else
  verdict DATA_RESTORE_CONTENT FAIL "file1=[$FILE1_A] sentinel=$SENT_A extra=$EXTRA_A serial=$SERIAL_B->$SERIAL_A hash_equal=$([ -n "$HASH_A" ] && [ "$HASH_B" = "$HASH_A" ] && echo True || echo False) count=$COUNT_B->$COUNT_A"; fail=1
fi
case "$SC" in *"serial_before=$SERIAL_B_N serial_after=$SERIAL_A_N ntoskrnl=false"*)
  if [ -n "$SERIAL_B_N" ] && [ -n "$SERIAL_A_N" ]; then verdict DATA_RESTORE_SELFCHECK PASS "$SC"; else verdict DATA_RESTORE_SELFCHECK FAIL "the volume serial could not be read from the fixture check; [$SC]"; fail=1; fi ;;
  *) verdict DATA_RESTORE_SELFCHECK FAIL "[$SC] serial_from_fixture=$SERIAL_A_N"; fail=1 ;; esac
SAME=$(bcd_same "$EV/$NAME-bcd-before.txt" "$EV/$NAME-bcd-after.txt")
printf 'TASKS_BEFORE=%s TASKS_AFTER=%s\n%s\n' "$TASKS0" "$TASKS1" "$SAME" | tee -a "$EV/$NAME-verdict.txt"
[ "$SAME" = "BCD_READINGS_EQUAL=True" ] || fail=1
WINRE=$(winre_enabled)
printf '%s\n' "$WINRE" | tee -a "$EV/$NAME-verdict.txt"
[ "$WINRE" = "WINRE_ENABLED=True" ] || fail=1

if [ $fail -eq 0 ]; then verdict DATA_RESTORE PASS "task=$PREPARE_TASK"; else verdict DATA_RESTORE FAIL "task=$PREPARE_TASK (see the lines above)"; exit 1; fi
