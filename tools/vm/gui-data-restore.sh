#!/bin/bash
# tools/vm/gui-data-restore.sh —— 界面驱动的「还原数据到非 C 盘」（不格式化）全链：从在界面上点按钮，到还原做完、
# 用硬判据核对。两种模式：
#   offline（默认）：通道选「离线」→ 三道确认答「是」→ 界面自己派发准备 → 重启进恢复环境还原 → 回到 Windows。
#   online：通道保持默认的「在线」→ 破坏性确认答「是」→ 界面当场开始在线还原（不重启）→ 等到「执行完成」结果框并点掉。
#     （2026-11-26 新增：对应表里「B 入口在线不格式化」此前没有脚本。）
#
# 为什么单独一个用例（2026-11-25）：缺陷 1（离线数据卷还原走不通）当初是在**界面上**报出来的，
# 修复后验证的却是命令行下发的准备；界面上只走到对话框、最后点了「否」。界面自己拼参数、派发子进程、
# 等它请求重启这一段，在新版本上没有跑到过。界面的数据入口在内部叫 restore-data，下发时才翻译成命令行的
# restore-existing，参数比命令行多出启动菜单名、失败策略，可能还有 --verify-hash。
#
# 做法：哨兵法。还原之前在测试卷上放哨兵和一个镜像里没有的多余文件、把一个文件改坏 →
#   界面：页签「还原数据到非 C 盘」→ 取消勾选「格式化目标分区」→ 填镜像路径 → 读取镜像 → 创建任务 →
#         破坏性确认点「是」→（离线：处理方式选「进入 Windows RE」→ 最后一道确认点「是」→ 界面派发准备 → 机器自己重启 → 回来）
# 判据：
#   1. 界面自己记下的证据（只数本次驱动之后的）：离线 = 选择框正文说的是「未勾选格式化、不格式化」、
#      「prepare args: --skip-format」「Windows RE confirm result=6」；在线 = 「channel split … online=true」
#      「online params: format_target=false」「online started: op=restore-existing」「online completed: success=true」；两种都不许有拦截与错误行；
#   2. 任务 success / 100、恢复环境日志步骤 1/4–4/4、自检行卷序列号前后一致、目标卷上没有系统内核文件（仅离线）；
#      在线还原完成后弹「执行完成」、点掉后没有残留的消息框（仅在线）；
#   3. 被改坏的文件回到原版；哨兵与多余文件还在；卷序列号、载荷哈希、文件数不变；
#   4. 启动配置前后读数一致；恢复环境仍为已启用。
#
# ★ 这两种模式都会真的还原。三道确认的驱动（tools/gui-check/restore-confirm.sh）要求显式设置 BR_ALLOW_RESTORE=yes
#   才会在真正下发的那一道确认上点「是」——本脚本替你设置了它，因为它只跑数据卷、不格式化这条链。
# ★ 界面默认是勾选「格式化目标分区」的：本脚本明确取消勾选。要测格式化那两条链，另写用例，不要改这里。
#
# 用法：tools/vm/gui-data-restore.sh [数据镜像路径] [offline|online]     或     tools/vm/gui-data-restore.sh online
#   offline 约 4 分钟（含两次重启）；online 约 2 分钟（不重启）。
# 前提：虚拟机在运行且没人在用；测试卷上有夹具目录 F:\BRDATA（与镜像内容一致）。
# 退出码：0 = 全部判据通过；1 = 有判据不满足；其它 = 步骤中止。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
# 第一个参数是 offline / online 时直接当模式（镜像用默认的）；否则第一个参数是镜像路径、第二个是模式。
if [ "${1:-}" = offline ] || [ "${1:-}" = online ]; then MODE="$1"; DATA_WIM="$DATA_IMAGE"; else DATA_WIM="${1:-$DATA_IMAGE}"; MODE="${2:-offline}"; fi
case "$MODE" in
  offline|online) ;;
  *) echo "用法: tools/vm/gui-data-restore.sh [数据镜像路径] [offline|online]"; echo "GUI_DATA_RESTORE=ABORT reason=usage"; exit 2 ;;
esac
# 证据文件名前缀：离线沿用原来的 gui-data-restore（已有的证据不被改名），在线另起一个。
if [ "$MODE" = offline ]; then run_name "gui-data-restore"; else run_name "gui-data-restore-online"; fi
TAG="gd$MODE"
fail=0

say "STEP=0 deployed binaries"
deployed_hashes | head -1

say "STEP=1 preconditions: the data image and the fixture directory on the test volume"
need_guest_file "$DATA_WIM" "the data image is missing"
need_guest_file 'F:\BRDATA\sub\file1.txt' "F:\\BRDATA (the sentinel-method fixture) is missing"
say "PRECONDITIONS_OK"

say "STEP=2 boot configuration: file-level backup, verified it reads offline; readings before"
bcd_backup "$NAME" | tee "$EV/$NAME-bcd-file.txt"
bcd_readings > "$EV/$NAME-bcd-before.txt"
TASK_BEFORE=$(newest_task); say "NEWEST_TASK_BEFORE=${TASK_BEFORE:-<unreadable>}"

say "STEP=3 nobody is using the desktop; resident elevated helper; helper scripts staged"
require_idle "$NAME-idle.txt" 120
gui_ready

say "STEP=4 pre-state on the test volume: sentinel, a file that is not in the image, a damaged file"
ps1 "$SHARE_PS\\c5prep.ps1" -Tag "$TAG" | grep -a -E '^PREP|__END__'
ps1 "$SHARE_PS\\c5check.ps1" -Tag "$TAG" | grep -a -E '^CHECK' | tee "$EV/$NAME-pre-check.txt"

# ★ 界面日志是跨天累积的（里面有 11-23 报缺陷 1 时答「是」的记录、10-30 的「已拦截」记录）。
#   下面数「界面自己记下的证据」时只数本次驱动开始之后的：界线取虚拟机自己的时钟。
#   第一版数的是整份文件，把两天前的记录也数了进来，于是「历史上有过拦截」被当成了本次被拦截（2026-11-25 实测）。
T0_GUEST=$(guest_utc_now)
say "GUEST_UTC_AT_DRIVE_START=${T0_GUEST:-<unreadable>}"
[ -n "$T0_GUEST" ] || { say "ABORT: could not read the guest clock; the GUI log could not be bounded to this run"; exit 3; }

TASK=""
if [ "$MODE" = offline ]; then
  say "STEP=5 drive the GUI all the way: the last confirmation is answered YES (the GUI then dispatches the prepare itself)"
  drive=$(BR_ALLOW_RESTORE=yes bash "$HERE/../gui-check/restore-confirm.sh" 1105 "$DATA_WIM" 2002 yes 1213 1214 2>&1)
  DRC=$?
  printf '%s\n' "$drive" | cut -c1-240
  [ "$DRC" -eq 0 ] || { verdict GUI_DRIVE FAIL "driver rc=$DRC (see the lines above)"; fail=1; }
  # 选择框正文（控件 2000）：必须说的是「未勾选格式化」。驱动脚本输出的是已解码的枚举，这里可以按文字找。
  choice_body=$(printf '%s\n' "$drive" | grep -a 'id=2000 ' | sed -E 's/.*text=//' | tr -d '\r' | tail -1)
  case "$choice_body" in
    *未勾选*|*unchecked*) verdict NO_FORMAT_AT_DIALOG PASS "the choice dialog said: format is unchecked" ;;
    *) verdict NO_FORMAT_AT_DIALOG FAIL "choice body=[$choice_body]"; fail=1 ;;
  esac

  say "STEP=6 identify the task the GUI created (the newest task directory must change)"
  t=0
  while [ $t -lt 90 ]; do
    TASK=$(newest_task)
    [ -n "$TASK" ] && [ "$TASK" != "$TASK_BEFORE" ] && break
    keep_alive; sleep 2; t=$((t+2))
  done
  say "TASK_CREATED=${TASK:-<none>} after ${t}s"
  if [ -z "$TASK" ] || [ "$TASK" = "$TASK_BEFORE" ]; then
    say "no new task directory: listing the GUI's windows to capture whatever it is showing"
    elev uidlglist.ps1 -Class ANY 2>/dev/null | decode_text | cut -c1-240 | head -30
    verdict GUI_DATA_RESTORE FAIL "the GUI did not create a task"; exit 1
  fi

  say "STEP=7 wait for the task to reach a terminal stage (the machine reboots into WinRE and comes back)"
  wait_terminal "$TASK" 1500 | tee "$EV/$NAME-terminal.txt"

  say "STEP=8 collect evidence verbatim"
  fetch "H:\\brwork\\tasks\\$TASK\\status.json" "$NAME-status.json" && say "fetched status.json"
  fetch "H:\\BRImages\\Recovery.log" "$NAME-Recovery.raw.log" && say "fetched Recovery.log"
  fetch "H:\\brwork\\logs\\gui.log" "$NAME-gui.raw.log" && say "fetched gui.log"
else
  say "STEP=5 drive the GUI: the channel stays ONLINE (its default), format is unchecked, YES on the destructive confirmation -- the GUI then restores online at once"
  # 「纯在线流程」没有第二道确认：破坏性确认上的「是」就是真正的下发（驱动脚本因此同样要求 BR_ALLOW_RESTORE=yes）。
  drive=$(BR_ALLOW_RESTORE=yes bash "$HERE/../gui-check/restore-confirm.sh" 1105 "$DATA_WIM" none yes 1214 2>&1)
  DRC=$?
  printf '%s\n' "$drive" | cut -c1-240
  [ "$DRC" -eq 0 ] || { verdict GUI_DRIVE FAIL "driver rc=$DRC (see the lines above)"; fail=1; }
  # 处理方式选择框不该出现（在线流程不进恢复环境）。驱动结束时若它还挂着，说明分流错了：点「取消」收掉，留给判据去红。
  if [ "$(gui_window_count "$CHOICE_CLASS")" = "1" ]; then
    say "DEFECT: the handler-choice dialog is up although the channel is online; dismissing it with Cancel"
    l=$(gui_wait_window "$CHOICE_CLASS" 10)
    if [ "$(control_text "$l" 2003)" = "取消" ] || [ "$(control_text "$l" 2003)" = "Cancel" ]; then gui_must_click online "$CHOICE_CLASS" 2003; fi
    verdict NO_CHOICE_DIALOG FAIL "the handler-choice dialog appeared on the online channel"; fail=1
  else
    verdict NO_CHOICE_DIALOG PASS "no handler-choice dialog on the online channel"
  fi

  say "STEP=6 the GUI must have logged 'online started' within 30 s (otherwise there is nothing to wait for)"
  started=0; t=0
  while [ $t -lt 30 ]; do
    sleep 5; t=$((t+5))
    fetch "H:\\brwork\\logs\\gui.log" "$NAME-gui.raw.log" && n=$(python3 -I "$HERE/judge/gui-dispatch.py" "$EV/$NAME-gui.raw.log" "$T0_GUEST" | sed -n 's/^GUI_LOG_ONLINE_RESTORE_STARTED=\([0-9]*\).*/\1/p' | tail -1)
    [ "${n:-0}" -ge 1 ] && { started=1; break; }
  done
  [ $started -eq 1 ] || { gui_windows "$NAME-windows-stuck.txt" | grep -a -E '^WIN' | cut -c1-170; verdict GUI_DATA_RESTORE_ONLINE FAIL "no 'online started: op=restore-existing' line within 30 s after YES (was the click confirmed?)"; exit 1; }
  say "ONLINE_STARTED=yes after ${t}s"

  say "STEP=7 wait for the result box (a message box with a single button), then click that button"
  l=$(gui_wait_window MSGBOX 1200) || { verdict GUI_DATA_RESTORE_ONLINE FAIL "no result box within 1200 s"; exit 1; }
  printf '%s\n' "$l" > "$EV/$NAME-result-box.txt"
  printf '%s\n' "$l" | grep -a -E '^WIN|id=[0-9]* class=Button ' | cut -c1-200 | sed 's/^/  RESULT_BOX /'
  ok_id=$(result_box_button_id "$l")
  if [ -n "$ok_id" ] && [ "$(printf '%s' "$ok_id" | wc -w | tr -d ' ')" = 1 ]; then
    say "RESULT_BOX_BUTTON_ID=$ok_id"; gui_must_click online MSGBOX "$ok_id"
  else
    say "ABORT: the result box does not have exactly one button (ids: [$ok_id]); not clicking anything"; verdict GUI_WINDOWS_CLEAN FAIL "result box buttons=[$ok_id]"; fail=1
  fi
  sleep 3

  say "STEP=8 collect evidence verbatim"
  fetch "H:\\brwork\\logs\\gui.log" "$NAME-gui.raw.log" && say "fetched gui.log"
  # 点掉结果框之后：不得再挂着消息框（最多看 5 轮），主窗口必须还在。
  left=""; for i in 1 2 3 4 5; do left=$(gui_window_count MSGBOX); [ "$left" = "0" ] && break; sleep 3; done
  main=$(gui_window_count "$MAIN_CLASS")
  # ★ 一行一个键值（判读按行取键；2026-11-26 第一版把两个键写在了同一行，判读读成「0 MAIN_WINDOWS=1」而判了失败）。
  printf 'MESSAGE_BOXES_LEFT=%s\nMAIN_WINDOWS=%s\n' "${left:-<unreadable>}" "${main:-<unreadable>}" | tee "$EV/$NAME-windows.txt"
fi

say "STEP=9 post-state on the test volume; readings after"
ps1 "$SHARE_PS\\c5check.ps1" -Tag "$TAG" | grep -a -E '^CHECK' | tee "$EV/$NAME-post-check.txt"
bcd_readings > "$EV/$NAME-bcd-after.txt"

say "STEP=10 judge"
{
  if [ "$MODE" = offline ]; then
    status_field "$EV/$NAME-status.json" stage | sed 's/^/FINAL_STAGE=/'
    status_field "$EV/$NAME-status.json" progress | sed 's/^/FINAL_PROGRESS=/'
    python3 -I "$HERE/judge/restore-log.py" "$EV/$NAME-Recovery.raw.log" "$TASK"
  fi
  python3 -I "$HERE/judge/gui-dispatch.py" "$EV/$NAME-gui.raw.log" "$T0_GUEST"
} | tee "$EV/$NAME-readings.txt"
rd() { sed -n "s/^$1=\(.*\)$/\1/p" "$EV/$NAME-readings.txt" | tail -1; }
chk() { fixture_field "$2" "$1"; }
SERIAL_B=$(chk serial "$EV/$NAME-pre-check.txt"); SERIAL_A=$(chk serial "$EV/$NAME-post-check.txt")
FILE1_A=$(chk file1 "$EV/$NAME-post-check.txt")
SENT_A=$(chk sentinel_exists "$EV/$NAME-post-check.txt"); EXTRA_A=$(chk extra_exists "$EV/$NAME-post-check.txt")
HASH_B=$(chk payload_sha256 "$EV/$NAME-pre-check.txt"); HASH_A=$(chk payload_sha256 "$EV/$NAME-post-check.txt")
COUNT_B=$(chk brdata_file_count "$EV/$NAME-pre-check.txt"); COUNT_A=$(chk brdata_file_count "$EV/$NAME-post-check.txt")
SERIAL_B_N=$(printf '%s' "$SERIAL_B" | tr -d '-'); SERIAL_A_N=$(printf '%s' "$SERIAL_A" | tr -d '-')
LINES_AFTER=$(rd GUI_LOG_LINES_AFTER); BLOCKED=$(rd GUI_LOG_BLOCKED); ERRLOG=$(rd GUI_LOG_ERROR)

if [ "$MODE" = offline ]; then
  STAGE=$(rd FINAL_STAGE); PROGRESS=$(rd FINAL_PROGRESS); LABELS=$(rd RECOVERY_STEP_LABELS); SC=$(rd SELFCHECK_LINE)
  ARGSKIP=$(rd GUI_LOG_SKIP_FORMAT_ARGS); CONFIRM=$(rd GUI_LOG_CONFIRM_YES)
  if [ "$STAGE" = "success" ] && [ "$PROGRESS" = "100" ] && [ "$LABELS" = "1,2,3,4" ]; then
    verdict GUI_DATA_RESTORE_COMPLETED PASS "stage=success progress=100, STEP 1/4..4/4 all present, task=$TASK"
  else
    verdict GUI_DATA_RESTORE_COMPLETED FAIL "stage=$STAGE progress=$PROGRESS step_labels=${LABELS:-none} task=$TASK"; fail=1
  fi
  # ★ 先证明「划界之后的日志」读得到：一行都读不到说明时钟或取回出了问题，不能拿「拦截 0 行」当通过。
  if [ "${LINES_AFTER:-0}" -lt 1 ]; then
    verdict GUI_DISPATCH FAIL "no GUI log line after $T0_GUEST (the log could not be bounded to this run; reading failed is not a verdict)"; fail=1
  elif [ "${ARGSKIP:-0}" -ge 1 ] && [ "${CONFIRM:-0}" -ge 1 ] && [ "${BLOCKED:-0}" -eq 0 ] && [ "${ERRLOG:-0}" -eq 0 ]; then
    verdict GUI_DISPATCH PASS "since $T0_GUEST: $LINES_AFTER GUI log lines; --skip-format passed to prepare, final confirmation answered yes, nothing blocked, no [ERROR] line"
  else
    verdict GUI_DISPATCH FAIL "lines_after=${LINES_AFTER:-none} skip_format_log=${ARGSKIP:-none} confirm_yes_log=${CONFIRM:-none} blocked_log=${BLOCKED:-none} error_log=${ERRLOG:-none}"; fail=1
  fi
else
  DONE=$(rd GUI_LOG_ONLINE_DONE); BAD=$(rd GUI_LOG_ONLINE_FAILED)
  if [ "${DONE:-0}" -ge 1 ] && [ "${BAD:-0}" -eq 0 ]; then
    verdict GUI_DATA_RESTORE_ONLINE_COMPLETED PASS "online completed: success=true; the program reports command, read-back and sidecar bookkeeping all passed"
  else
    verdict GUI_DATA_RESTORE_ONLINE_COMPLETED FAIL "online_completed_success=${DONE:-none} online_completed_failed=${BAD:-none}"; fail=1
  fi
  PNF=$(rd GUI_LOG_ONLINE_PARAMS_NO_FORMAT); CHON=$(rd GUI_LOG_CHANNEL_ONLINE); STON=$(rd GUI_LOG_ONLINE_RESTORE_STARTED)
  CANC=$(rd GUI_LOG_CANCELLED_ANY); AF=$(rd GUI_LOG_ACTION_FAILED); OFFSKIP=$(rd GUI_LOG_SKIP_FORMAT_ARGS)
  if [ "${LINES_AFTER:-0}" -lt 1 ]; then
    verdict GUI_DISPATCH FAIL "no GUI log line after $T0_GUEST (the log could not be bounded to this run; reading failed is not a verdict)"; fail=1
  elif [ "${PNF:-0}" -ge 1 ] && [ "${CHON:-0}" -ge 1 ] && [ "${STON:-0}" -ge 1 ] && [ "${CANC:-0}" -eq 0 ] && [ "${AF:-0}" -eq 0 ] \
       && [ "${BLOCKED:-0}" -eq 0 ] && [ "${ERRLOG:-0}" -eq 0 ] && [ "${OFFSKIP:-0}" -eq 0 ]; then
    verdict GUI_DISPATCH PASS "since $T0_GUEST: $LINES_AFTER GUI log lines; split to the online channel with format_target=false, started, nothing cancelled, blocked or failed (and the offline prepare-args line is absent)"
  else
    verdict GUI_DISPATCH FAIL "lines_after=${LINES_AFTER:-none} online_no_format=${PNF:-none} channel_online=${CHON:-none} started=${STON:-none} cancelled=${CANC:-none} action_failed=${AF:-none} blocked_log=${BLOCKED:-none} error_log=${ERRLOG:-none} offline_skip_format_line=${OFFSKIP:-none}"; fail=1
  fi
  LEFT=$(sed -n 's/^MESSAGE_BOXES_LEFT=\(.*\)$/\1/p' "$EV/$NAME-windows.txt" | tail -1); MAINN=$(sed -n 's/^MAIN_WINDOWS=\(.*\)$/\1/p' "$EV/$NAME-windows.txt" | tail -1)
  if [ "$LEFT" = "0" ] && [ "$MAINN" = "1" ]; then
    verdict GUI_WINDOWS_CLEAN PASS "after the result box was clicked no message box is left and the main window is still there"
  else
    verdict GUI_WINDOWS_CLEAN FAIL "message_boxes_left=${LEFT:-none} main_windows=${MAINN:-none}"; fail=1
  fi
fi
# ★ 比对之前先确认读到的不是空串：读不到的时候「空串 == 空串」会让比对假通过。
if [ "$FILE1_A" = "BRDATA file1 v1" ] && [ "$SENT_A" = "True" ] && [ "$EXTRA_A" = "True" ] && [ -n "$SERIAL_A" ] && [ "$SERIAL_B" = "$SERIAL_A" ] \
   && [ -n "$HASH_A" ] && [ "$HASH_B" = "$HASH_A" ] && [ -n "$COUNT_A" ] && [ "$COUNT_B" = "$COUNT_A" ]; then
  verdict GUI_DATA_RESTORE_CONTENT PASS "file1 back to the image content; sentinel and extra file kept; volume serial, payload hash and file count unchanged"
else
  verdict GUI_DATA_RESTORE_CONTENT FAIL "file1=[$FILE1_A] sentinel=$SENT_A extra=$EXTRA_A serial=$SERIAL_B->$SERIAL_A hash_equal=$([ -n "$HASH_A" ] && [ "$HASH_B" = "$HASH_A" ] && echo True || echo False) count=$COUNT_B->$COUNT_A"; fail=1
fi
if [ "$MODE" = offline ]; then
  case "$SC" in *"serial_before=$SERIAL_B_N serial_after=$SERIAL_A_N ntoskrnl=false"*)
    if [ -n "$SERIAL_B_N" ] && [ -n "$SERIAL_A_N" ]; then verdict GUI_DATA_RESTORE_SELFCHECK PASS "$SC"; else verdict GUI_DATA_RESTORE_SELFCHECK FAIL "the volume serial could not be read from the fixture check; [$SC]"; fail=1; fi ;;
    *) verdict GUI_DATA_RESTORE_SELFCHECK FAIL "[$SC] serial_from_fixture=$SERIAL_A_N"; fail=1 ;; esac
fi
SAME=$(bcd_same "$EV/$NAME-bcd-before.txt" "$EV/$NAME-bcd-after.txt")
printf '%s\n' "$SAME" | tee -a "$EV/$NAME-verdict.txt"
[ "$SAME" = "BCD_READINGS_EQUAL=True" ] || fail=1
WINRE=$(winre_enabled)
printf '%s\n' "$WINRE" | tee -a "$EV/$NAME-verdict.txt"
[ "$WINRE" = "WINRE_ENABLED=True" ] || fail=1

if [ "$MODE" = offline ]; then FINAL_KEY=GUI_DATA_RESTORE; else FINAL_KEY=GUI_DATA_RESTORE_ONLINE; fi
if [ $fail -eq 0 ]; then verdict "$FINAL_KEY" PASS "mode=$MODE task=${TASK:-<online: no task directory>}"; else verdict "$FINAL_KEY" FAIL "mode=$MODE (see the lines above)"; exit 1; fi
