#!/bin/bash
# tools/vm/pause-restore.sh —— 数据卷的离线还原经恢复环境：按 P 必须真的停住，再按 P 才继续并完成。
# 同时复验「还原数据到非 C 盘」走恢复环境这条链（v2.10.8 修）。
#
# 独立于任何日志的物理判据：还原前删掉测试卷上的 20 GB 占位文件（镜像里有它）→ 还原后它必须回来；
#   被改坏的文件回到原版；哨兵与镜像之外的多余文件还在（不格式化的覆盖式还原）；卷序列号不变。
# 日志判据（原样取回）：恢复环境日志里有「恢复已暂停」与「恢复已继续」；步骤 3/4 → 4/4 的间隔不短于保持时长；
#   声明行 Pause support: operation=restore-existing supported=true。
# 只发 P 键：B / K / L / M 在进度窗里是热键，不能碰。截图只用来决定「什么时候发键」，不作判据。
#
# 用法：HOLD_SECONDS=150 tools/vm/pause-restore.sh
# 前提：先跑过 tools/vm/pause-backup.sh（或 orphan-race.sh new both）——它产出含占位文件的测试镜像；
#       测试卷上有哨兵法用的夹具目录 F:\BRDATA（回归基线实测留下的；里面的 file1 必须是原版内容）。
# 耗时：约 5 分钟（保持时长 + 两次重启）。
# 退出码：0 = 全部判据通过；1 = 有判据不满足；其它 = 步骤中止。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
HOLD_SECONDS="${HOLD_SECONDS:-150}"
SHOT="${TMPDIR:-/tmp}/br-vm-tools-shot.png"
TAG="pr"          # 哨兵文件名用的标签：固定值，每跑一次覆盖同一个文件，不在测试卷上越留越多
run_name "pause-restore"
fail=0

say "STEP=0 deployed binaries"
deployed_hashes

say "STEP=1 preconditions: test image with sidecar, pad file on the test volume, fixture directory"
vrun "dir /b $IMAGE*" 2 | tr -cd '\11\12\40-\176\r' | tr -d '\r' | grep -a 'br-vm-tools-test' | sed 's/^/  IMAGE_FILE /'
need_guest_file "$IMAGE" "the test image is missing; run tools/vm/pause-backup.sh first"
need_guest_file "$IMAGE.index-1.metadata.json" "the sidecar of the test image is missing; run tools/vm/pause-backup.sh first"
need_guest_file "$PAD" "the pad file is not on the test volume; an image made without it cannot prove anything here"
need_guest_file 'F:\BRDATA\sub\file1.txt' "F:\\BRDATA (the sentinel-method fixture) is missing"
say "PRECONDITIONS_OK"

say "STEP=2 boot configuration: file-level backup, verified it reads offline; readings before"
bcd_backup "$NAME" | tee "$EV/$NAME-bcd-file.txt"
bcd_readings > "$EV/$NAME-bcd-before.txt"

say "STEP=3 nobody is using the desktop"
require_idle "$NAME-idle.txt" 120

say "STEP=4 pre-state: remove the pad file (it is in the image), put a sentinel and an extra file, damage file1"
vrun "del /f /q $PAD" 2 | grep -a 'VM_DONE' | sed 's/^/  PAD_DELETE /'
# 占位文件必须真的删掉了（它是还原的物理判据：还原后它必须回来）；读不到不算删掉了。
guest_exists "$PAD"; rc=$?
case $rc in
  1) say "  PAD_AFTER_DELETE=absent" ;;
  0) say "ABORT: the pad file is still there after deleting it; the restore could not be proven by its return"; exit 3 ;;
  *) say "ABORT: could not read whether the pad file is gone"; exit 3 ;;
esac
ps1 "$SHARE_PS\\c5prep.ps1" -Tag "$TAG" | grep -a -E '^PREP|__END__'
ps1 "$SHARE_PS\\c5check.ps1" -Tag "$TAG" | grep -a -E '^CHECK' | tee "$EV/$NAME-pre-check.txt"

say "STEP=5 prepare the offline data restore (reboots into WinRE by itself)"
run_prepare "$NAME-prepare.json" --operation restore-existing --source-drive $SOURCE_DRIVE --target-drive $SOURCE_DRIVE --boot-menu-name BR-PAUSE2 --image-path $IMAGE --wim-index 1 --skip-format --allow-destructive --json
prepare_summary "$NAME-prepare.json" "$NAME-prepare-summary.txt"
[ -n "$PREPARE_TASK" ] || { say "ABORT: prepare did not create a task"; exit 4; }
[ "$PREPARE_SUCCESS" != "False" ] || { verdict PAUSE_RESTORE FAIL "the prepare failed (see the summary above)"; exit 1; }
START=$(date +%s)

say "STEP=6 wait for the command channel to go down (the VM is really rebooting)"
t=0; down=0
while [ $t -lt 180 ]; do
  keep_alive
  if channel_alive; then sleep 3; t=$(( $(date +%s) - START )); else sleep 2; channel_alive || { down=1; break; }; fi
done
[ "$down" = 1 ] || { say "ABORT: never observed the reboot"; exit 5; }
DOWN_AT=$(date +%s)

say "STEP=7 wait for the progress window (frame size only decides WHEN to press), then press P at once"
# 数据卷的镜像只要 10 秒左右就应用完，第二个暂停检查点紧跟其后——窗口一出现就得发键，没有余量。
pressed=0; t=0
while [ $t -lt 120 ]; do
  keep_alive
  "$PRL" capture "$VM" --file "$SHOT" >/dev/null 2>&1
  sz=$(stat -f%z "$SHOT" 2>/dev/null || echo 0)
  if [ "$sz" -gt 22000 ] && [ "$sz" -lt 95000 ]; then cp "$SHOT" "$EV/$NAME-window-before-p.png"; say "progress window seen (frame $sz bytes)"; sleep 2; send_p; pressed=1; break; fi
  sleep 2; t=$(( $(date +%s) - DOWN_AT ))
done
if [ "$pressed" = 0 ]; then say "window not detected within 120 s; pressing P anyway"; send_p; fi
PRESSED_AT=$(date +%s)
printf 'P_PRESSED_AT=%s AFTER_DOWN_SECS=%s\n' "$(date -r "$PRESSED_AT" '+%H:%M:%S')" "$(( PRESSED_AT - DOWN_AT ))" | tee "$EV/$NAME-keys.txt"

say "STEP=8 hold for ${HOLD_SECONDS}s: if the pause works the VM must NOT come back during this time"
t=0; came_back=0
while [ $t -lt "$HOLD_SECONDS" ]; do
  keep_alive
  if channel_alive; then came_back=1; say "VM_BACK_EARLY after ${t}s of holding"; break; fi
  sleep 8; t=$(( $(date +%s) - PRESSED_AT ))
done
if [ "$came_back" = 0 ]; then
  say "VM_STILL_AWAY after ${HOLD_SECONDS}s -> treat as paused; taking a frame, then pressing P to continue"
  "$PRL" capture "$VM" --file "$EV/$NAME-window-held.png" >/dev/null 2>&1
  send_p; printf 'RESUME_KEY_AT=%s\n' "$(date '+%H:%M:%S')" | tee -a "$EV/$NAME-keys.txt"
fi
printf 'CAME_BACK_DURING_HOLD=%s\n' "$came_back" | tee -a "$EV/$NAME-keys.txt"

say "STEP=9 wait for the terminal stage"
wait_stage_terminal "$PREPARE_TASK" 900 | tee "$EV/$NAME-terminal.txt"

say "STEP=10 collect evidence verbatim"
fetch "H:\\brwork\\tasks\\$PREPARE_TASK\\status.json" "$NAME-status.json" && say "fetched status.json"
fetch "H:\\BRImages\\Recovery.log" "$NAME-Recovery.raw.log" && say "fetched Recovery.log"
fetch "H:\\brwork\\tasks\\$PREPARE_TASK\\Recovery-early.log" "$NAME-task-early.raw.log" && say "fetched task early log"

say "STEP=11 post-state on the test volume"
vrun "dir $PAD" 2 | tr -s ' ' | grep -a 'br-vm-tools-pad' | sed 's/^/PAD_FILE_AFTER /' | tee "$EV/$NAME-pad-after.txt"
ps1 "$SHARE_PS\\c5check.ps1" -Tag "$TAG" | grep -a -E '^CHECK' | tee "$EV/$NAME-post-check.txt"
bcd_readings > "$EV/$NAME-bcd-after.txt"

say "STEP=12 judge"
# 读数分两处：任务状态文件（两个键各一行）与恢复环境日志（判读程序 judge/restore-log.py，三个还原用例共用）。
{
  status_field "$EV/$NAME-status.json" stage | sed 's/^/FINAL_STAGE=/'
  status_field "$EV/$NAME-status.json" progress | sed 's/^/FINAL_PROGRESS=/'
  python3 -I "$HERE/judge/restore-log.py" "$EV/$NAME-Recovery.raw.log" "$PREPARE_TASK" --pause --early-log "$EV/$NAME-task-early.raw.log"
} | tee "$EV/$NAME-readings.txt"
rd() { sed -n "s/^$1=\(.*\)$/\1/p" "$EV/$NAME-readings.txt" | tail -1; }
chk() { fixture_field "$2" "$1"; }
STAGE=$(rd FINAL_STAGE); PROGRESS=$(rd FINAL_PROGRESS); LABELS=$(rd RECOVERY_STEP_LABELS)
PAUSED_N=$(rd PAUSED_LINES); RESUMED_N=$(rd RESUMED_LINES); PAUSED_S=$(rd SECONDS_PAUSED)
DECL=$(rd DECLARATION_IN_FORMAL_LOG); DECL_E=$(rd DECLARATION_IN_TASK_EARLY_LOG); SC=$(rd SELFCHECK_LINE)
CAME=$(sed -n 's/^CAME_BACK_DURING_HOLD=\(.*\)$/\1/p' "$EV/$NAME-keys.txt" | tail -1)
PADN=$(grep -a -c 'PAD_FILE_AFTER' "$EV/$NAME-pad-after.txt")
SERIAL_B=$(chk serial "$EV/$NAME-pre-check.txt"); SERIAL_A=$(chk serial "$EV/$NAME-post-check.txt")
FILE1_A=$(chk file1 "$EV/$NAME-post-check.txt")
SENT_A=$(chk sentinel_exists "$EV/$NAME-post-check.txt"); EXTRA_A=$(chk extra_exists "$EV/$NAME-post-check.txt")
HASH_B=$(chk payload_sha256 "$EV/$NAME-pre-check.txt"); HASH_A=$(chk payload_sha256 "$EV/$NAME-post-check.txt")
# 卷序列号在两处读数里写法不同：夹具检查带连字符（A412-9DAC），恢复环境自检行不带（A4129DAC）。比对自检行时去掉连字符。
SERIAL_B_N=$(printf '%s' "$SERIAL_B" | tr -d '-'); SERIAL_A_N=$(printf '%s' "$SERIAL_A" | tr -d '-')

if [ "$STAGE" = "success" ] && [ "$PROGRESS" = "100" ] && [ "$LABELS" = "1,2,3,4" ]; then
  verdict RESTORE_COMPLETED PASS "stage=success progress=100, STEP 1/4..4/4 all present, task=$PREPARE_TASK"
else
  verdict RESTORE_COMPLETED FAIL "stage=$STAGE progress=$PROGRESS step_labels=${LABELS:-none} task=$PREPARE_TASK"; fail=1
fi
if [ "${PAUSED_N:-0}" -ge 1 ] && [ "${RESUMED_N:-0}" -ge 1 ] && [ "$CAME" = 0 ] && [ "${PAUSED_S:-0}" -ge $(( HOLD_SECONDS - 40 )) ] 2>/dev/null; then
  verdict PAUSE_HONORED PASS "paused and resumed on request; paused for ${PAUSED_S}s with a hold of ${HOLD_SECONDS}s"
else
  verdict PAUSE_HONORED FAIL "paused=${PAUSED_N:-none} resumed=${RESUMED_N:-none} came_back_during_hold=$CAME seconds_paused=${PAUSED_S:-unknown}"; fail=1
fi
case "$DECL" in *'operation=restore-existing supported=true'*) verdict PAUSE_DECLARATION PASS "$DECL ($DECL_E line(s) in the task early log)";;
  *) verdict PAUSE_DECLARATION FAIL "formal=[$DECL] early_lines=${DECL_E:-none}"; fail=1 ;; esac
if [ "$PADN" -ge 1 ]; then verdict PAD_RETURNED PASS "the pad file deleted before the restore is back (the image was really applied)"; else verdict PAD_RETURNED FAIL; fail=1; fi
if [ "$FILE1_A" = "BRDATA file1 v1" ]; then verdict FILE1_RESTORED PASS "file1 is back to its original content"; else verdict FILE1_RESTORED FAIL "file1=[$FILE1_A]"; fail=1; fi
# ★ 比对之前先确认读到的不是空串：读不到的时候「空串 == 空串」会让这一项假通过。
if [ "$SENT_A" = "True" ] && [ "$EXTRA_A" = "True" ] && [ -n "$SERIAL_A" ] && [ "$SERIAL_B" = "$SERIAL_A" ] && [ -n "$HASH_A" ] && [ "$HASH_B" = "$HASH_A" ]; then
  verdict OVERLAY_KEPT PASS "sentinel and extra file kept, volume serial $SERIAL_A unchanged, payload hash unchanged"
else
  verdict OVERLAY_KEPT FAIL "sentinel=$SENT_A extra=$EXTRA_A serial=$SERIAL_B->$SERIAL_A hash_equal=$([ -n "$HASH_A" ] && [ "$HASH_B" = "$HASH_A" ] && echo True || echo False)"; fail=1
fi
case "$SC" in *"serial_before=$SERIAL_B_N serial_after=$SERIAL_A_N ntoskrnl=false"*)
  if [ -n "$SERIAL_B_N" ] && [ -n "$SERIAL_A_N" ]; then verdict SELFCHECK PASS "$SC"; else verdict SELFCHECK FAIL "the volume serial could not be read from the fixture check; [$SC]"; fail=1; fi ;;
  *) verdict SELFCHECK FAIL "[$SC] serial_from_fixture=$SERIAL_A_N"; fail=1 ;; esac
SAME=$(bcd_same "$EV/$NAME-bcd-before.txt" "$EV/$NAME-bcd-after.txt")
printf '%s\n' "$SAME" | tee -a "$EV/$NAME-verdict.txt"
[ "$SAME" = "BCD_READINGS_EQUAL=True" ] || fail=1
WINRE=$(winre_enabled)
printf '%s\n' "$WINRE" | tee -a "$EV/$NAME-verdict.txt"
[ "$WINRE" = "WINRE_ENABLED=True" ] || fail=1

if [ $fail -eq 0 ]; then verdict PAUSE_RESTORE PASS "task=$PREPARE_TASK"; else verdict PAUSE_RESTORE FAIL "task=$PREPARE_TASK (see the lines above)"; exit 1; fi
