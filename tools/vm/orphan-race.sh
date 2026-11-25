#!/bin/bash
# tools/vm/orphan-race.sh —— 「孤儿任务守卫 × 正在准备中的任务」竞态的复现与验证。
#
# 背景（2026-11-24 实机发现，v2.10.9 修复）：准备流程先把任务落盘（阶段 = 已准备），要到最后才写启动项记录，
# 中间约 20 秒。界面程序每次启动第一步是「孤儿任务守卫」，它把「非终态 + 没写目标卷 + 没有启动项记录」的任务
# 当孤儿：标成失败、删任务产物。开机自启的界面程序（或用户再点一次图标）恰好在这个窗口里启动，在办的准备就被清掉。
# 同一根因的另一条路：`abandon-task` 命令对在办任务照常执行。
#
# 做法：命令行起一个备份准备；2.5 秒后取最新的任务目录（= 在办任务），在窗口内做下面两件事中的一件或两件：
#   abandon  对它执行 `abandon-task`；
#   gui      在用户桌面会话里启动界面程序（用不结束任何进程的启动脚本——现成的 launch.ps1 会先结束所有同名进程，
#            而命令行准备进程也叫 BackupRestore.exe，照用就变成了「脚本杀的」）。
#
# 用法：
#   tools/vm/orphan-race.sh new both            日常验证（约 4 分钟，含一次备份往返）
#   tools/vm/orphan-race.sh new gui|abandon     只验其中一条
#   tools/vm/orphan-race.sh old gui|abandon     在有缺陷的旧二进制（v2.10.8 及以前）上复现；old 默认只做 gui
#
# 预期（new）：放弃被拒 BR-TSK-036；守卫日志有「仍在准备，没动」、没有「标失败」；准备成功；随后的备份里按 P 只记
#   「已忽略」；回到 Windows 后自启的界面程序没有把这个任务标失败；启动配置前后读数一致。
# 预期（old）：放弃在任务日志里留下「明确放弃」/ 守卫把这个任务标失败；准备失败。
#
# 前提：虚拟机在运行；部署位是待测的二进制；测试卷（默认 F）可牺牲，上面有足够空间放 20 GB 占位文件。
# 副作用：会重启虚拟机两次（准备成功后进恢复环境做一次备份再回来）；产生测试镜像与占位文件
#   （tools/vm/clean-fixtures.sh 可清）。
# 退出码：0 = 预期全部成立；1 = 有判据不满足；其它 = 步骤中止（原因见日志）。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
MODE="${1:?用法: orphan-race.sh old|new [gui|abandon|both]}"
case "$MODE" in old|new) ;; *) say "ABORT: mode must be old or new"; exit 2 ;; esac
PART="${2:-}"
[ -n "$PART" ] || PART=$([ "$MODE" = old ] && echo gui || echo both)
case "$PART" in gui|abandon|both) ;; *) say "ABORT: part must be gui, abandon or both"; exit 2 ;; esac
DO_ABANDON=0; DO_GUI=0
[ "$PART" != gui ] && DO_ABANDON=1
[ "$PART" != abandon ] && DO_GUI=1
run_name "orphan-race-$MODE-$PART"
fail=0

process_count() { vrun "tasklist /fi \"IMAGENAME eq BackupRestore.exe\" /fo csv /nh" 3 | tr -cd '\11\12\40-\176' | grep -a -c 'BackupRestore.exe'; }
newest_task() { vrun "dir /b /ad /o-d H:\\brwork\\tasks" 2 | tr -cd '\11\12\40-\176\r' | grep -a -E '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' | head -1 | tr -d '\r'; }

say "STEP=0 deployed binaries"
deployed_hashes

say "STEP=1 boot configuration: file-level backup, verified it reads offline; readings before"
bcd_backup "$NAME" | tee "$EV/$NAME-bcd-file.txt"
bcd_readings | tee "$EV/$NAME-bcd-before.txt"

say "STEP=2 nobody is using the desktop"
require_idle "$NAME-idle.txt" 120

if [ "$DO_GUI" = 1 ]; then
  say "STEP=3 resident elevated helper in the desktop session; helper scripts staged (byte counts compared)"
  gui_ready
fi

say "STEP=4 fixtures: a 20 GB zero pad file on the test volume (so the capture lasts long enough to press P); old test image removed"
# 这个用例自己不用夹具，但它备份测试卷、产出的镜像会被 pause-restore.sh 拿来用：夹具脏了就不出镜像。
require_clean_fixture no
ensure_pad_file
vrun "dir $PAD" 2 | tr -s ' ' | grep -a 'br-vm-tools-pad' | sed 's/^/  PAD_FILE /'
vrun "del /q $IMAGE $IMAGE.index-1.metadata.json" 2 | grep -a 'VM_DONE' | sed 's/^/  OLD_IMAGE_REMOVED /'
vrun "dir /b $IMAGE*" 2 | tr -cd '\11\12\40-\176' | grep -a -c 'br-vm-tools-test' | sed 's/^/  IMAGE_FILES_BEFORE=/'

say "STEP=5 stop the auto-started GUI so that the start time of the next one is controlled"
for i in 1 2 3; do vrun "taskkill /im BackupRestore.exe /f" 2 >/dev/null; sleep 1; n=$(process_count); [ "$n" = "0" ] && break; sleep 2; done
say "GUI_PROCESSES_RUNNING=${n:-<unreadable>}"
[ "${n:-1}" = "0" ] || { say "ABORT: a BackupRestore process is still running; the race test needs a quiet start"; exit 3; }
TASKS0=$(task_count); say "TASK_COUNT_BEFORE=${TASKS0:-<unreadable>}"

say "STEP=6 start a backup prepare from the command line (background); the task being prepared must be a NEW task directory"
# ★ 「在办任务」必须是准备启动之后**新出现**的任务目录。第一版直接取「最新的目录」：准备命令因通道静默没有执行时，
#   最新的目录是上一次任务留下的，脚本把它当成了在办任务、对它执行放弃命令（2026-11-25 实测；产品以 BR-TSK-003 拒绝，无害）。
# ★★ 什么时候才算「这次下发没有执行」：**客户端进程已经退出、输出是空的、再等 8 秒也没有新任务目录**。
#   客户端在命令执行期间是一直阻塞的；它还在等，就说明命令在跑（哪怕任务目录几秒后才出现）。
#   2026-11-25 实测：启动后 2.5 秒没见到任务目录就判成没执行并重发——其实第一次执行了（目录 4 秒后才出现），
#   第二次被产品的「存在未终结任务」闸门拦下（BR-TSK-005，无害）；而杀掉宿主上的客户端并不会停掉虚拟机里的命令，
#   那次准备继续走完并重启了机器。所以这里不杀任何进程，只看客户端是否已退出。
TASK_BEFORE=$(newest_task)
say "NEWEST_TASK_BEFORE=${TASK_BEFORE:-<unreadable>}"
TASK=""; PREPARE_PID=""; T0=0; attempt=0; STARTED=0
while [ $attempt -lt 3 ]; do
  attempt=$((attempt+1))
  rm -f "$EV/$NAME-prepare.json"
  "$PRL" exec "$VM" cmd /d /v:on /c "$EXE prepare --operation backup --source-drive $SOURCE_DRIVE --image-path $IMAGE --boot-menu-name BR-RACE-$MODE --json & echo VM_DONE=!ERRORLEVEL!" > "$EV/$NAME-prepare.json" 2>/dev/null &
  PREPARE_PID=$!
  say "PREPARE_LAUNCHED pid=$PREPARE_PID attempt=$attempt"
  t=0
  while [ $t -lt 40 ]; do                                   # 等新任务目录出现，最多 40 秒
    TASK=$(newest_task)
    if [ -n "$TASK" ] && [ "$TASK" != "$TASK_BEFORE" ]; then STARTED=1; break; fi
    if ! kill -0 "$PREPARE_PID" 2>/dev/null; then            # 客户端已经退出
      [ -s "$EV/$NAME-prepare.json" ] && break               # 有输出 = 执行过了（在建任务之前就失败了），交给后面的判读
      sleep 4                                                # 输出为空：多半是通道静默失效；再等 8 秒看任务目录
      TASK=$(newest_task)
      if [ -n "$TASK" ] && [ "$TASK" != "$TASK_BEFORE" ]; then STARTED=1; break; fi
      sleep 4
      TASK=$(newest_task)
      if [ -n "$TASK" ] && [ "$TASK" != "$TASK_BEFORE" ]; then STARTED=1; break; fi
      break                                                  # 这次确实没有执行：出去重发
    fi
    sleep 1; t=$((t+1))
  done
  [ "$STARTED" = 1 ] && break
  if kill -0 "$PREPARE_PID" 2>/dev/null; then
    say "PREPARE_NOTE: the client is still waiting after ${t}s but no task directory appeared; not sending it again"
    break
  fi
  [ -s "$EV/$NAME-prepare.json" ] && break
  say "PREPARE_ATTEMPT $attempt: the client exited with no output and no task directory appeared -- the command did not run; sending it again"
done
T0=$(date +%s)

say "STEP=7 inside the prepare window: identify the task being prepared, then abandon it and/or start the GUI"
# ★ 不靠宿主与虚拟机的时钟对齐：直接取新出现的任务目录当「在办任务」，之后所有日志都按这个编号去对。
say "TASK_BEING_PREPARED=${TASK:-<none>} attempt=$attempt"
if [ "$STARTED" != 1 ] || [ -z "$TASK" ] || [ "$TASK" = "$TASK_BEFORE" ]; then
  say "ABORT: no new task directory appeared -- the prepare did not run or failed before creating a task; nothing was poked"
  printf 'PREPARE_ERROR_HEAD=%s\n' "$(json_field "$EV/$NAME-prepare.json" error | head -c 300 | tr '\n' ' ')"
  exit 3
fi
ABANDON_ATTEMPTED=0; ABANDON_SUCCESS=""; ABANDON_CODE=""
if [ "$DO_ABANDON" = 1 ]; then
  vraw "$EXE abandon-task H:\\brwork $TASK --json" "$NAME-abandon.json" || say "WARNING: the abandon command gave no output marker"
  ABANDON_ATTEMPTED=1
  ABANDON_SUCCESS=$(json_field "$EV/$NAME-abandon.json" success); ABANDON_CODE=$(json_field "$EV/$NAME-abandon.json" error_code)
  printf 'ABANDON success=%s error_code=%s task=%s\n' "$ABANDON_SUCCESS" "$ABANDON_CODE" "$TASK" | tee "$EV/$NAME-abandon-summary.txt"
  say "ABANDON_DONE at +$(( $(date +%s) - T0 ))s"
fi
if [ "$DO_GUI" = 1 ]; then
  LAUNCH=$(elev launch-nokill.ps1 | tr '\n' ' ')
  say "GUI_LAUNCH at +$(( $(date +%s) - T0 ))s: $LAUNCH"
  printf '%s\n' "$LAUNCH" > "$EV/$NAME-gui-launch.txt"
fi

say "STEP=8 wait for the prepare process to finish (it either fails, or reboots the machine)"
t=0
while kill -0 "$PREPARE_PID" 2>/dev/null && [ $t -lt 180 ]; do keep_alive; sleep 2; t=$((t+2)); done
say "PREPARE_FINISHED after ${t}s"
grep -a -q 'VM_DONE=' "$EV/$NAME-prepare.json" && say "PREPARE_OUTPUT_MARK=$(grep -a -o 'VM_DONE=[0-9]*' "$EV/$NAME-prepare.json" | tail -1)" || say "PREPARE_OUTPUT_MARK=<none: the channel closed before the marker (expected when the machine reboots)>"
SUCCESS=$(json_field "$EV/$NAME-prepare.json" success)
OUT_TASK=$(json_field "$EV/$NAME-prepare.json" task_id)
printf 'PREPARE success=%s error_code=%s task_id=%s\n' "$SUCCESS" "$(json_field "$EV/$NAME-prepare.json" error_code)" "$OUT_TASK" | tee "$EV/$NAME-prepare-summary.txt"
printf 'PREPARE_ERROR_HEAD=%s\n' "$(json_field "$EV/$NAME-prepare.json" error | head -c 300 | tr '\n' ' ')" | tee -a "$EV/$NAME-prepare-summary.txt"
# 重新下发的那一次被「存在未终结任务」闸门拦下：在办的就是前一次下发的任务（它只是起得慢）。按它去等终态。
if [ "$SUCCESS" = "False" ] && [ "$(json_field "$EV/$NAME-prepare.json" error_code)" = "BR-TSK-005" ] && [ "$OUT_TASK" != "$TASK" ]; then
  say "PREPARE_NOTE: this attempt was refused by the unfinished-task gate (BR-TSK-005); task $TASK from an earlier attempt is the one in progress"
  SUCCESS=""
fi
if [ "$MODE" = new ] && [ -n "$OUT_TASK" ] && [ "$OUT_TASK" != "$TASK" ]; then
  say "ABORT: the prepare reported task $OUT_TASK but the newest task directory was $TASK; the readings cannot be tied together"; exit 3
fi

say "STEP=9 read the task's records back verbatim"
fetch "H:\\brwork\\tasks\\$TASK\\status.json" "$NAME-status.json" && say "fetched status.json" || say "status.json: not present"
fetch "H:\\brwork\\tasks\\$TASK\\prepare.log" "$NAME-prepare.raw.log" && say "fetched prepare.log" || say "prepare.log: not present"
fetch "H:\\brwork\\tasks\\$TASK\\recovery.log" "$NAME-task-recovery.raw.log" && say "fetched task recovery.log" || { say "task recovery.log: not present (nobody wrote to it)"; : > "$EV/$NAME-task-recovery.raw.log"; }
if [ "$DO_GUI" = 1 ]; then
  fetch "H:\\brwork\\logs\\gui.log" "$NAME-gui.raw.log" && say "fetched gui.log" || say "gui.log: not present"
fi
python3 -I - "$EV" "$NAME" "$TASK" <<'PY' | tee "$EV/$NAME-readings.txt"
import os, sys
ev, name, task = sys.argv[1], sys.argv[2], sys.argv[3]
def read(f):
    p = os.path.join(ev, f)
    return open(p, 'rb').read().decode('utf-8', 'replace').splitlines() if os.path.exists(p) else []
prep = read(name + '-prepare.raw.log')
print("PREPARE_LOG_STARTED=%d" % sum(1 for l in prep if 'Rust preparation started' in l))
print("ONESHOT_ARMED_IN_LOG=%d" % sum(1 for l in prep if 'one-shot bootsequence armed' in l))
for l in sorted({l.split('] ', 1)[1][:110] for l in prep if '[ERROR]' in l})[:6]:
    print("PREPARE_LOG_ERROR " + l)
# 放弃命令写的是任务目录里的 recovery.log。认它而不认事后的任务状态：放弃命令会先把任务标失败，
# 但在办的准备流程随后还会再写一次失败，把那句文案覆盖掉。
rlog = read(name + '-task-recovery.raw.log')
took = [l for l in rlog if 'User explicitly abandoned' in l]
refused = [l for l in rlog if 'Abandon refused' in l]
print("ABANDON_TOOK_EFFECT_IN_TASK_LOG=%d" % len(took))
print("ABANDON_REFUSED_IN_TASK_LOG=%d" % len(refused))
for l in took[:1] + refused[:1]:
    print("TASK_RECOVERY_LOG_LINE " + l[:200])
gui = read(name + '-gui.raw.log')
mine = [l for l in gui if 'orphan task guard' in l and task in l]
print("GUARD_MARKED_FAILED_THIS_TASK=%d" % sum(1 for l in mine if 'marking Failed' in l))
print("GUARD_LEFT_ALONE_THIS_TASK=%d" % sum(1 for l in mine if 'still being prepared' in l))
for l in mine[:3]:
    print("GUARD_LINE " + l[:220])
PY
reading() { sed -n "s/^$1=\(.*\)$/\1/p" "$EV/$NAME-readings.txt" | tail -1; }
TOOK=$(reading ABANDON_TOOK_EFFECT_IN_TASK_LOG); REFUSED=$(reading ABANDON_REFUSED_IN_TASK_LOG)
KILLED=$(reading GUARD_MARKED_FAILED_THIS_TASK); LEFT=$(reading GUARD_LEFT_ALONE_THIS_TASK)
STARTED=$(reading PREPARE_LOG_STARTED)

say "STEP=10 judge"
[ "${STARTED:-0}" -ge 1 ] || { say "ABORT: the newest task directory has no prepare log; it was not the task being prepared"; exit 3; }
if [ "$DO_ABANDON" = 1 ]; then
  if [ "$MODE" = old ]; then
    [ "${TOOK:-0}" -ge 1 ] && verdict ORPHAN_RACE_ABANDON PASS "(defect reproduced: abandon-task took effect on a task that was still being prepared)" \
      || { verdict ORPHAN_RACE_ABANDON FAIL "expected the abandon command to take effect on the old binary; no 'explicitly abandoned' line in the task log (success=$ABANDON_SUCCESS code=$ABANDON_CODE)"; fail=1; }
  else
    if [ "$ABANDON_SUCCESS" = "False" ] && [ "$ABANDON_CODE" = "BR-TSK-036" ] && [ "${REFUSED:-0}" -ge 1 ] && [ "${TOOK:-0}" -eq 0 ]; then
      verdict ORPHAN_RACE_ABANDON PASS "refused with BR-TSK-036, a refusal line in the task log, task left as it was"
    else
      verdict ORPHAN_RACE_ABANDON FAIL "success=$ABANDON_SUCCESS code=$ABANDON_CODE refused_line=${REFUSED:-none} took=${TOOK:-none}"; fail=1
    fi
  fi
fi
if [ "$DO_GUI" = 1 ]; then
  if [ "$MODE" = old ]; then
    [ "${KILLED:-0}" -ge 1 ] && verdict ORPHAN_RACE_GUARD PASS "(defect reproduced: the guard collected the task that was still being prepared)" \
      || { verdict ORPHAN_RACE_GUARD FAIL "expected the guard to mark this task failed on the old binary"; fail=1; }
  else
    if [ "${LEFT:-0}" -ge 1 ] && [ "${KILLED:-0}" -eq 0 ]; then
      verdict ORPHAN_RACE_GUARD PASS "the guard logged 'still being prepared; left alone' and did not mark it failed"
    else
      verdict ORPHAN_RACE_GUARD FAIL "left_alone=${LEFT:-none} marked_failed=${KILLED:-none}"; fail=1
    fi
  fi
fi
if [ "$MODE" = old ]; then
  [ "$SUCCESS" = "True" ] && { verdict ORPHAN_RACE_PREPARE FAIL "the prepare succeeded; the race was not reproduced"; fail=1; } \
    || verdict ORPHAN_RACE_PREPARE PASS "(defect reproduced: the prepare failed)"
else
  [ "$SUCCESS" = "True" ] && verdict ORPHAN_RACE_PREPARE PASS "the prepare succeeded while it was being poked" \
    || { verdict ORPHAN_RACE_PREPARE FAIL "success=$SUCCESS"; fail=1; }
fi

if [ "$SUCCESS" = "True" ]; then
  say "STEP=11 the backup runs in WinRE; P is pressed at 45/70/100/130 s"
  wait_terminal "$TASK" 1200 45 70 100 130 | tee "$EV/$NAME-terminal.txt"
  say "STEP=12 collect evidence verbatim"
  fetch "H:\\brwork\\tasks\\$TASK\\status.json" "$NAME-status.json" && say "fetched status.json"
  fetch "H:\\BRImages\\Recovery.log" "$NAME-Recovery.raw.log" && say "fetched Recovery.log"
  if [ "$DO_GUI" = 1 ]; then
    fetch "H:\\brwork\\logs\\gui.log" "$NAME-gui-after.raw.log" && say "fetched gui.log after the round trip"
  fi
  python3 -I - "$EV" "$NAME" "$TASK" <<'PY' | tee "$EV/$NAME-final.txt"
import json, os, sys
ev, name, task = sys.argv[1], sys.argv[2], sys.argv[3]
def read(f):
    p = os.path.join(ev, f)
    return open(p, 'rb').read().decode('utf-8', 'replace').splitlines() if os.path.exists(p) else []
try:
    s = json.loads(''.join(read(name + '-status.json')).lstrip('\ufeff'))
    # ★ 一行只放一个键值：取值函数按「行首的键=」取，两个键挤在一行会取串（2026-11-25 收脚本时实测，
    #   stage 被读成「success FINAL_PROGRESS=100」，一次通过的用例被判成失败）。
    print("FINAL_STAGE=%s" % s.get('stage'))
    print("FINAL_PROGRESS=%s" % s.get('progress'))
except (OSError, ValueError) as e:
    print("FINAL_STAGE=<unreadable: %s> FINAL_PROGRESS=<unreadable>" % str(e)[:60])
lines = read(name + '-Recovery.raw.log')
idx = [i for i, l in enumerate(lines) if task in l and 'Recovery.exe started' in l]
seg = lines[idx[0]:] if idx else []
print("RECOVERY_STEP_LINES=%d" % sum(1 for l in seg if 'STEP ' in l and '/4' in l))
print("PAUSE_IGNORED_LINES=%d" % sum(1 for l in seg if 'Pause ignored' in l))
print("PAUSED_LINES=%d" % sum(1 for l in seg if '恢复已暂停' in l))
for l in seg:
    if 'STEP ' in l and '/4' in l or 'Pause ignored' in l or 'stage=success' in l or 'ERROR' in l or 'BR-' in l:
        print("RECOVERY_LINE " + l[:200])
g = read(name + '-gui-after.raw.log')
print("GUARD_MARKED_FAILED_AFTER_TRIP=%d" % sum(1 for l in g if 'orphan task guard' in l and task in l and 'marking Failed' in l))
PY
  fin() { sed -n "s/^$1=\(.*\)$/\1/p" "$EV/$NAME-final.txt" | tail -1; }
  STAGE=$(fin FINAL_STAGE); IGN=$(fin PAUSE_IGNORED_LINES); PZ=$(fin PAUSED_LINES); STEPS=$(fin RECOVERY_STEP_LINES); AFT=$(fin GUARD_MARKED_FAILED_AFTER_TRIP)
  if [ "$STAGE" = "success" ] && [ "$STEPS" = "4" ]; then verdict BACKUP_COMPLETED PASS "stage=success, four STEP lines"; else verdict BACKUP_COMPLETED FAIL "stage=$STAGE step_lines=$STEPS"; fail=1; fi
  if [ "${IGN:-0}" -ge 1 ] && [ "${PZ:-0}" -eq 0 ]; then verdict BACKUP_PAUSE_IGNORED PASS "$IGN 'Pause ignored' line(s), the capture went on"; else verdict BACKUP_PAUSE_IGNORED FAIL "ignored=${IGN:-none} paused=${PZ:-none}"; fail=1; fi
  if [ "${AFT:-0}" -eq 0 ]; then verdict GUI_STARTUP_AFTER_TRIP PASS "the auto-started GUI did not mark this task failed"; else verdict GUI_STARTUP_AFTER_TRIP FAIL "marked_failed_after_trip=$AFT"; fail=1; fi
fi

say "STEP=13 readings after"
bcd_readings | tee "$EV/$NAME-bcd-after.txt"
# ★ 先把读数存进变量再判断：`读数 | tee` 的退出码是 tee 的，管道后面接 `|| fail=1` 永远不会触发。
SAME=$(bcd_same "$EV/$NAME-bcd-before.txt" "$EV/$NAME-bcd-after.txt")
printf '%s\n' "$SAME" | tee -a "$EV/$NAME-verdict.txt"
[ "$SAME" = "BCD_READINGS_EQUAL=True" ] || fail=1
WINRE=$(winre_enabled)
printf '%s\n' "$WINRE" | tee -a "$EV/$NAME-verdict.txt"
[ "$WINRE" = "WINRE_ENABLED=True" ] || { say "WARNING: could not confirm that Windows RE is still enabled"; fail=1; }

if [ $fail -eq 0 ]; then verdict ORPHAN_RACE PASS "mode=$MODE part=$PART task=$TASK"; else verdict ORPHAN_RACE FAIL "mode=$MODE part=$PART task=$TASK (see the lines above)"; fi
