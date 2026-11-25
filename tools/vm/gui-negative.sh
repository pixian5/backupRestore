#!/bin/bash
# tools/vm/gui-negative.sh —— 界面「说不」矩阵：每一个确认框上点「否」或「取消」之后，都必须什么都没发生。
#
# 为什么要有它（2026-11-26）：备份入口的「破坏性确认」点「否」不中止这个缺陷从 10-13 起存在了六周，
#   因为界面用例只点「是」。这里把界面上每一个「说不」的出口都点一遍，判据是「零副作用」：
#   界面自己记下了「已取消 / 已拒绝」的那一行；没有下发准备、没有开始在线操作；任务目录不变；
#   没有残留的消息框；启动配置前后读数一致；不该写出来的文件不存在。
#
# 九个子用例（都不创建任务、不写镜像、不重启）：
#   a-first-no          「还原系统到 C 盘」：破坏性确认点「否」
#   b-offline-first-no  「还原数据到非 C 盘」离线：破坏性确认点「否」
#   c-first-no          「还原系统到非 C 盘」：破坏性确认点「否」
#   a-overlay-no        「还原系统到 C 盘」取消勾选格式化：覆盖式还原确认点「否」（这道在破坏性确认之前）
#   b-offline-final-no  「还原数据到非 C 盘」离线：处理方式选「进入 Windows RE」后，最后一道确认点「否」
#   a-metadata-no       「还原系统到 C 盘」：副档缺失提示点「否」（临时复制一份没有副档的镜像）
#   backup-on-source    「备份」：镜像放在源卷上必须被拦下（提示框点「确定」）
#   backup-path-cancel  「备份」：镜像路径不合法 → 引导框点「取消」
#   a-choice-cancel     「还原系统到 C 盘」：处理方式选择框点「取消」
#
# ★ 为什么副档缺失这一条走「还原系统到 C 盘」而不是数据入口：万一提示没有出现、流程继续往下走，
#   系统盘入口的下一道只是处理方式选择框（会停下来等人），而数据入口的在线通道会当场开始还原。
#   用例要保证「即使被测程序不按预期走，也不会真的下发」。
#
# 用法：tools/vm/gui-negative.sh [数据镜像路径]     约 8 分钟
#   NEG_ONLY="a-first-no a-choice-cancel"   只跑列出的子用例
#   NEG_MUTATION_FIRST_YES=1                变异验证：第一个子用例故意点「是」。这一次运行**必须**判
#                                           NEG_a-first-no=FAIL、GUI_NEGATIVE=FAIL、退出码 1（判不出失败 = 判据恒绿）。
#                                           系统盘入口点完「是」之后只是处理方式选择框，脚本用「取消」收掉，不会下发。
# 前提：虚拟机在运行且没人在用；数据镜像与它的副档都在。
# 退出码：0 = 全部子用例通过；1 = 有子用例不满足；其它 = 步骤中止。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
DATA_WIM="${1:-$DATA_IMAGE}"
run_name "gui-negative"
fail=0
GUIDE_CLASS=BackupRestoreInvalidPathChoice          # 镜像路径引导框；「取消」按钮编号 3002
NEG_DIR='H:\BRImages\br-neg'
NEG_NOSIDE='H:\BRImages\br-neg\nosidecar.wim'
NEG_ONSOURCE='F:\BRImages\br-neg-on-source.wim'
BOX="" ; T0="" ; SET_MENU=""
CASES_RUN=0; CASES_PASS=0
# 每个子用例都不许出现的行：真的往下走了才会有的那几类。
NEVER="GUI_LOG_DELEGATED GUI_LOG_PREPARE_LAUNCH GUI_LOG_ONLINE_RESTORE_STARTED GUI_LOG_ONLINE_STARTED GUI_LOG_ONLINE_SPLIT \
GUI_LOG_ONLINE_DONE GUI_LOG_ONLINE_FAILED GUI_LOG_ACTION_FAILED GUI_LOG_ERROR GUI_LOG_PREPARE_FAILED_EARLY"

say "STEP=0 deployed binaries"
deployed_hashes | head -1

say "STEP=1 preconditions: the data image and its sidecar"
need_guest_file "$DATA_WIM" "the data image is missing"
need_guest_file "$DATA_WIM.index-1.metadata.json" "the data image's sidecar is missing"
say "PRECONDITIONS_OK"

say "STEP=2 boot configuration: file-level backup, verified it reads offline; readings before; task list before"
bcd_backup "$NAME" | tee "$EV/$NAME-bcd-file.txt"
bcd_readings > "$EV/$NAME-bcd-before.txt"
TASKS0=$(gui_task_list); echo "TASKS_BEFORE=$(printf '%s' "$TASKS0" | wc -w | tr -d ' ')" | tee "$EV/$NAME-tasks.txt"

say "STEP=3 nobody is using the desktop; resident elevated helper; helper scripts staged"
require_idle "$NAME-idle.txt" 120
gui_ready

say "STEP=4 a copy of the data image WITHOUT its sidecar, in a directory of its own"
say "      (a missing per-index sidecar falls back to the directory-level metadata.json, so the copy must not sit next to one)"
vrun "mkdir H:\\BRImages\\br-neg" 2 | grep -a 'VM_DONE'
vrun "copy /y $DATA_WIM $NEG_NOSIDE" 2 | grep -a 'VM_DONE'
need_guest_file "$NEG_NOSIDE" "the sidecar-less copy of the data image was not created"
guest_exists "$NEG_NOSIDE.index-1.metadata.json"; rc=$?
[ $rc -eq 1 ] || { say "ABORT: the copy's sidecar exists, or it could not be read (rc=$rc)"; exit 3; }
say "NOSIDE_CAR_COPY_READY"

# ---- 驱动的小函数 -----------------------------------------------------------------------
# click_single_button <标签>：当前恰好一个消息框、且只有一颗按钮 → 读出它的编号并点掉。
click_single_button() {
  local l id
  l=$(gui_wait_window MSGBOX 20) || return 1
  printf '%s\n' "$l" > "$EV/$NAME-$1-single-box.txt"
  id=$(result_box_button_id "$l")
  [ -n "$id" ] && [ "$(printf '%s' "$id" | wc -w | tr -d ' ')" = 1 ] || { say "  the box does not have exactly one button (ids: [$id]); not clicking"; return 1; }
  gui_must_click "$1" MSGBOX "$id"
}

# open_tab <页签> [额外点击的控件编号…]：启动界面、切页签、额外点击。
open_tab() {
  local tab="$1"; shift
  gui_launch
  gui_must_click neg "$MAIN_CLASS" "$tab" || return 1
  local id; for id in "$@"; do gui_must_click neg "$MAIN_CLASS" "$id" || return 1; done
}

# read_image_then_create <镜像路径>：填路径 → 读取镜像 → 这时不得冒出消息框 → （可选）填第二系统名称 → 点「创建任务」。
read_image_then_create() {
  gui_set_image_path "$1" || { say "  could not type the image path"; return 1; }
  gui_must_click neg "$MAIN_CLASS" 1002 || return 1
  sleep 4
  if [ "$(gui_window_count MSGBOX)" != "0" ]; then
    say "  a message box is up after reading the image (it should not be):"
    click_single_button neg
    return 1
  fi
  if [ -n "$SET_MENU" ]; then gui_set_control_text 1207 "$SET_MENU" || { say "  could not type the second-system name"; return 1; }; fi
  gui_must_click neg "$MAIN_CLASS" 1003
}

# wait_yesno：等到同时有「是」(6) 与「否」(7) 的消息框，清单输出到标准输出。
#   返回 2 = 冒出来的消息框不是「是 / 否」（由调用方收掉并判失败）。
wait_yesno() {
  local l
  l=$(gui_wait_window MSGBOX 40) || { say "  no message box within 40 s"; return 1; }
  if ! printf '%s\n' "$l" | grep -a -q 'id=6 ' || ! printf '%s\n' "$l" | grep -a -q 'id=7 '; then
    printf '%s\n' "$l" | grep -a -E '^WIN|id=[0-9]* class=Button' | cut -c1-200 | sed 's/^/    /'
    say "  the message box that came up has no Yes/No pair"
    return 2
  fi
  printf '%s\n' "$l"
}

# wait_box_or_choice <上限秒>：等「是 / 否」的消息框或处理方式选择框之中的任意一个；结果写进 WHICH（yesno / choice）与 BOX。
wait_box_or_choice() {
  local limit="${1:-30}" t=0 m c
  WHICH=""; BOX=""
  while [ $t -lt "$limit" ]; do
    m=$(gui_window_count MSGBOX); c=$(gui_window_count "$CHOICE_CLASS")
    if [ "$m" = "1" ]; then WHICH=yesno; BOX=$(gui_wait_window MSGBOX 10); return 0; fi
    if [ "$c" = "1" ]; then WHICH=choice; BOX=$(gui_wait_window "$CHOICE_CLASS" 10); return 0; fi
    sleep 3; t=$((t+3))
  done
  say "  neither a message box nor the handler-choice dialog within ${limit}s"
  return 1
}

# drive_to_box <页签> <镜像路径> [额外点击…]：启动、切页签、额外点击、填路径、读镜像、点「创建任务」，等到带「是 / 否」的框。
#   成功时清单在 BOX 里；失败返回非零（原因已打印，冒出来的框已收掉）。
drive_to_box() {
  local tab="$1" image="$2"; shift 2
  BOX=""
  open_tab "$tab" "$@" || return 1
  read_image_then_create "$image" || return 1
  BOX=$(wait_yesno); local rc=$?
  [ $rc -eq 0 ] && return 0
  [ $rc -eq 2 ] && click_single_button neg
  return 1
}

# button_is_cancel <已解码的窗口清单> <控件编号>：该控件的文字是不是「取消」/「Cancel」（是才允许点——点错了会打开别的窗口）。
button_is_cancel() {
  local t; t=$(control_text "$1" "$2")
  [ "$t" = "取消" ] || [ "$t" = "Cancel" ]
}
# 处理方式选择框的「取消」是 2003；镜像路径引导框的「取消」是 3002（它的 3001 是「选择镜像位置」，点了会打开文件浏览框）。
choice_cancel_is_safe() { button_is_cancel "$1" 2003; }
guide_cancel_is_safe() { button_is_cancel "$1" 3002; }

# rdk <键> <子用例>：从某个子用例的读数文件里取一个数值键（取不到输出空串）
rdk() { sed -n "s/^$1=\([0-9][0-9]*\).*/\1/p" "$EV/$NAME-$2-readings.txt" 2>/dev/null | tail -1; }

# finish_case <子用例> <说明> <必须出现的键…> -- <不许出现的键…>：
#   等界面把日志写完，取回界面日志（只数本子用例开始之后的），核对「必须出现」与「不许出现」，再核对零副作用。
finish_case() {
  local name="$1" what="$2"; shift 2
  local must="" mustnot="$NEVER" k n bad="" mode=must tasks1 boxes main
  for k in "$@"; do
    if [ "$k" = "--" ]; then mode=mustnot; continue; fi
    if [ "$mode" = must ]; then must="$must $k"; else mustnot="$mustnot $k"; fi
  done
  sleep 5
  fetch 'H:\brwork\logs\gui.log' "$NAME-$name-gui.raw.log" && say "  fetched gui.log"
  python3 -I "$HERE/judge/gui-dispatch.py" "$EV/$NAME-$name-gui.raw.log" "$T0" > "$EV/$NAME-$name-readings.txt"
  grep -a -E '^GUI_LOG_' "$EV/$NAME-$name-readings.txt" | grep -a -v '=0$' | cut -c1-200 | sed 's/^/  /'
  n=$(rdk GUI_LOG_LINES_AFTER "$name")
  [ "${n:-0}" -ge 1 ] || bad="$bad no-GUI-log-line-after-$T0"
  for k in $must; do n=$(rdk "$k" "$name"); [ "${n:-0}" -ge 1 ] || bad="$bad missing:$k=${n:-none}"; done
  for k in $mustnot; do n=$(rdk "$k" "$name"); [ "${n:-0}" -eq 0 ] || bad="$bad present:$k=$n"; done
  boxes=$(gui_window_count MSGBOX); main=$(gui_window_count "$MAIN_CLASS")
  [ "$boxes" = "0" ] || bad="$bad message_boxes_left=${boxes:-<unreadable>}"
  [ "$main" = "1" ] || bad="$bad main_windows=${main:-<unreadable>}"
  tasks1=$(gui_task_list); echo "TASKS_AFTER_$name=$(printf '%s' "$tasks1" | wc -w | tr -d ' ')" | tee -a "$EV/$NAME-tasks.txt"
  [ "$tasks1" = "$TASKS0" ] || bad="$bad task-list-changed"
  CASES_RUN=$((CASES_RUN+1))
  if [ -z "$bad" ]; then
    CASES_PASS=$((CASES_PASS+1)); verdict "NEG_$name" PASS "$what: the GUI logged its refusal and nothing went on"
  else
    verdict "NEG_$name" FAIL "$what:$bad"; fail=1
  fi
}

# ==================================================================================================
# 九个子用例各是一个函数，下面的循环按顺序跑。只跑其中几个：NEG_ONLY="a-first-no a-choice-cancel" tools/vm/gui-negative.sh
ALL_CASES="a-first-no b-offline-first-no c-first-no a-overlay-no b-offline-final-no a-metadata-no backup-on-source backup-path-cancel a-choice-cancel"

# dismiss_next <标签>：在某个确认框上答完之后，把随后冒出来的窗口**不下发地**收掉——选择框点「取消」，消息框点「否」。
dismiss_next() {
  if wait_box_or_choice 30; then
    if [ "$WHICH" = choice ]; then choice_cancel_is_safe "$BOX" && gui_must_click "$1" "$CHOICE_CLASS" 2003
    else gui_must_click "$1" MSGBOX 7; fi
  fi
}

# first_no <子用例> <页签> <必须出现的键> <另外两个入口的「已取消」键> [额外点击的控件编号…]
#   三个还原入口共用：破坏性确认点「否」。判据按入口精确匹配——界面内部的入口名是 restore-existing（还原系统到 C 盘）、
#   restore-data（还原数据到非 C 盘）、create-secondary（还原系统到非 C 盘）；另外两个入口的「已取消」行不许出现，
#   证明记下的是这一次、这一条路。
first_no() {
  local name="$1" tab="$2" key="$3" others="$4"; shift 4
  SET_MENU=""; [ "$name" = c-first-no ] && SET_MENU="BR-NEG"
  T0=$(guest_utc_now)
  if drive_to_box "$tab" "$DATA_WIM" "$@"; then
    printf '%s\n' "$BOX" > "$EV/$NAME-$name-box.txt"
    printf '%s\n' "$BOX" | grep -a -E '^WIN|id=(6|7) ' | cut -c1-200 | sed 's/^/  BOX /'
    if [ "$name" = a-first-no ] && [ "${NEG_MUTATION_FIRST_YES:-}" = "1" ]; then
      # 变异验证（NEG_MUTATION_FIRST_YES=1）：故意点「是」。系统盘入口的下一道只是处理方式选择框，用「取消」收掉，不会下发。
      # 这一次运行必须判 NEG_a-first-no=FAIL、GUI_NEGATIVE=FAIL；判不出失败，说明这组判据是恒绿的。
      say "MUTATION: clicking YES on purpose -- this sub-case must go red"
      gui_must_click "$name" MSGBOX 6
      dismiss_next "$name"
    else
      gui_must_click "$name" MSGBOX 7
    fi
  fi
  SET_MENU=""
  finish_case "$name" "NO on the destructive confirmation" "$key GUI_LOG_CANCELLED_ANY" -- "$others GUI_LOG_SYSTEM_CHOICE_ANY GUI_LOG_OVERLAY_DECLINED GUI_LOG_METADATA_DECLINED"
}
case_a-first-no()         { first_no a-first-no 1102 GUI_LOG_CANCELLED_RESTORE_EXISTING "GUI_LOG_CANCELLED_RESTORE_DATA GUI_LOG_CANCELLED_CREATE_SECONDARY"; }
case_b-offline-first-no() { first_no b-offline-first-no 1105 GUI_LOG_CANCELLED_RESTORE_DATA "GUI_LOG_CANCELLED_RESTORE_EXISTING GUI_LOG_CANCELLED_CREATE_SECONDARY" 1213; }
case_c-first-no()         { first_no c-first-no 1106 GUI_LOG_CANCELLED_CREATE_SECONDARY "GUI_LOG_CANCELLED_RESTORE_EXISTING GUI_LOG_CANCELLED_RESTORE_DATA"; }

case_a-overlay-no() {   # 「还原系统到 C 盘」取消勾选格式化：覆盖式还原确认（在破坏性确认之前）点「否」
  T0=$(guest_utc_now)
  if drive_to_box 1102 "$DATA_WIM" 1214; then
    printf '%s\n' "$BOX" > "$EV/$NAME-a-overlay-no-box.txt"
    printf '%s\n' "$BOX" | grep -a -E '^WIN|id=(6|7) ' | cut -c1-200 | sed 's/^/  BOX /'
    gui_must_click a-overlay-no MSGBOX 7
  fi
  finish_case a-overlay-no "NO on the overlay-restore (wimlib) confirmation" "GUI_LOG_OVERLAY_DECLINED" -- "GUI_LOG_CANCELLED_ANY GUI_LOG_SYSTEM_CHOICE_ANY"
}

case_b-offline-final-no() {   # 「还原数据到非 C 盘」离线：破坏性确认「是」→ 处理方式选「进入 Windows RE」→ 最后一道确认「否」
  T0=$(guest_utc_now)
  if drive_to_box 1105 "$DATA_WIM" 1213; then
    gui_must_click b-offline-final-no MSGBOX 6
    if wait_box_or_choice 30 && [ "$WHICH" = choice ]; then
      printf '%s\n' "$BOX" > "$EV/$NAME-b-offline-final-no-choice.txt"
      if choice_cancel_is_safe "$BOX" && gui_must_click b-offline-final-no "$CHOICE_CLASS" 2002 && wait_box_or_choice 30 && [ "$WHICH" = yesno ]; then
        printf '%s\n' "$BOX" | grep -a -E '^WIN|id=(6|7) ' | cut -c1-200 | sed 's/^/  BOX /'
        gui_must_click b-offline-final-no MSGBOX 7
      else
        say "  the last confirmation did not come up; dismissing whatever is there"
        dismiss_next b-offline-final-no
      fi
    else
      say "  no handler-choice dialog after YES; dismissing whatever is there"
      dismiss_next b-offline-final-no
    fi
  fi
  finish_case b-offline-final-no "Windows RE chosen, then NO on the last confirmation" "GUI_LOG_CHOICE_RE GUI_LOG_OFFLINE_CONFIRM_NO GUI_LOG_OFFLINE_CONFIRM_DECLINED" -- "GUI_LOG_CONFIRM_YES"
}

case_a-metadata-no() {   # 「还原系统到 C 盘」+ 没有副档的镜像：破坏性确认「是」→ 副档缺失提示「否」
  T0=$(guest_utc_now)
  if drive_to_box 1102 "$NEG_NOSIDE"; then
    gui_must_click a-metadata-no MSGBOX 6
    if wait_box_or_choice 30 && [ "$WHICH" = yesno ]; then
      printf '%s\n' "$BOX" > "$EV/$NAME-a-metadata-no-prompt.txt"
      printf '%s\n' "$BOX" | grep -a -E '^WIN|id=(6|7|65535) ' | cut -c1-200 | sed 's/^/  BOX /'
      gui_must_click a-metadata-no MSGBOX 7
    else
      # 提示没有出现、流程往下走了（或者什么都没出来）：不下发地收掉，留给判据去红。
      say "  the missing-metadata prompt did not come up; dismissing whatever is there"
      dismiss_next a-metadata-no
    fi
  fi
  finish_case a-metadata-no "NO on the missing-metadata prompt" "GUI_LOG_METADATA_DECLINED" -- "GUI_LOG_SYSTEM_CHOICE_ANY GUI_LOG_CONFIRM_YES"
}

case_backup-on-source() {   # 「备份」：镜像放在源卷上 → 创建任务后被拦下（单按钮提示框）
  local src
  T0=$(guest_utc_now)
  if open_tab 1101 && src=$(gui_select_source "$SOURCE_DRIVE") && gui_set_image_path "$NEG_ONSOURCE" && gui_must_click backup-on-source "$MAIN_CLASS" 1003; then
    say "  SOURCE_SELECTED=$(printf '%s' "$src" | cut -c1-60)"
    click_single_button backup-on-source
  fi
  finish_case backup-on-source "an image path on the source volume is blocked before anything is written" "GUI_LOG_IMAGE_ON_SOURCE_BLOCKED" -- "GUI_LOG_CANCELLED_ANY GUI_LOG_SYSTEM_CHOICE_ANY"
  guest_exists "$NEG_ONSOURCE"; local rc=$?
  if [ $rc -eq 1 ]; then verdict NEG_NO_IMAGE_WRITTEN PASS "no image was written to the source volume"; else verdict NEG_NO_IMAGE_WRITTEN FAIL "exists_rc=$rc (0 = it is there, other = unreadable)"; fail=1; fi
}

case_backup-path-cancel() {   # 「备份」：镜像路径不合法 → 引导框点「取消」（控件 3002）
  local l
  T0=$(guest_utc_now)
  if open_tab 1101 && gui_set_image_path 'br-negative-relative.wim' && gui_must_click backup-path-cancel "$MAIN_CLASS" 1003; then
    if l=$(gui_wait_window "$GUIDE_CLASS" 30); then
      printf '%s\n' "$l" > "$EV/$NAME-backup-path-cancel-guide.txt"
      printf '%s\n' "$l" | grep -a -E '^WIN|id=[0-9]+ class=Button' | cut -c1-200 | sed 's/^/  BOX /'
      if guide_cancel_is_safe "$l"; then gui_must_click backup-path-cancel "$GUIDE_CLASS" 3002; else say "  control 3002 is not the Cancel button; not clicking"; fi
    else
      say "  no guidance dialog; dismissing whatever is there"
      click_single_button backup-path-cancel
    fi
  fi
  finish_case backup-path-cancel "Cancel on the invalid-image-path guidance dialog" "GUI_LOG_PATH_GUIDANCE_CANCELLED" -- "GUI_LOG_CANCELLED_ANY GUI_LOG_SYSTEM_CHOICE_ANY"
}

case_a-choice-cancel() {   # 「还原系统到 C 盘」：破坏性确认「是」→ 处理方式选择框「取消」
  T0=$(guest_utc_now)
  if drive_to_box 1102 "$DATA_WIM"; then
    gui_must_click a-choice-cancel MSGBOX 6
    if wait_box_or_choice 30 && [ "$WHICH" = choice ]; then
      printf '%s\n' "$BOX" > "$EV/$NAME-a-choice-cancel-choice.txt"
      choice_cancel_is_safe "$BOX" && gui_must_click a-choice-cancel "$CHOICE_CLASS" 2003
    else
      say "  no handler-choice dialog after YES; dismissing whatever is there"
      dismiss_next a-choice-cancel
    fi
  fi
  finish_case a-choice-cancel "Cancel on the handler-choice dialog" "GUI_LOG_SYSTEM_CHOICE_ANY GUI_LOG_SYSTEM_CHOICE_CANCELLED" -- "GUI_LOG_CONFIRM_YES GUI_LOG_SYSTEM_CONFIRM_YES"
}

EXPECTED_CASES=0
for name in $ALL_CASES; do
  if [ -n "${NEG_ONLY:-}" ]; then case " $NEG_ONLY " in *" $name "*) ;; *) continue ;; esac; fi
  EXPECTED_CASES=$((EXPECTED_CASES+1))
  say "STEP=5.$EXPECTED_CASES $name"
  "case_$name"
done

say "STEP=6 remove the sidecar-less copy; readings after"
vrun "rmdir /s /q $NEG_DIR" 2 | grep -a 'VM_DONE' | sed 's/^/  COPY_REMOVED /'
guest_exists "$NEG_DIR"; rc=$?
case $rc in
  1) echo "COPY_REMOVED=True" | tee -a "$EV/$NAME-verdict.txt" ;;
  0) echo "COPY_REMOVED=False" | tee -a "$EV/$NAME-verdict.txt"; fail=1 ;;
  *) echo "COPY_REMOVED=<unreadable>" | tee -a "$EV/$NAME-verdict.txt"; fail=1 ;;
esac
bcd_readings > "$EV/$NAME-bcd-after.txt"
SAME=$(bcd_same "$EV/$NAME-bcd-before.txt" "$EV/$NAME-bcd-after.txt")
printf '%s\n' "$SAME" | tee -a "$EV/$NAME-verdict.txt"
[ "$SAME" = "BCD_READINGS_EQUAL=True" ] || fail=1
WINRE=$(winre_enabled)
printf '%s\n' "$WINRE" | tee -a "$EV/$NAME-verdict.txt"
[ "$WINRE" = "WINRE_ENABLED=True" ] || fail=1
echo "CASES_RUN=$CASES_RUN CASES_PASS=$CASES_PASS EXPECTED=$EXPECTED_CASES" | tee -a "$EV/$NAME-verdict.txt"
[ "$CASES_RUN" -eq "$EXPECTED_CASES" ] && [ "$CASES_RUN" -ge 1 ] || { echo "CASE_COUNT_UNEXPECTED=$CASES_RUN" | tee -a "$EV/$NAME-verdict.txt"; fail=1; }

if [ $fail -eq 0 ]; then verdict GUI_NEGATIVE PASS "$CASES_PASS sub-cases: every No / Cancel stopped the flow and nothing was written"
else verdict GUI_NEGATIVE FAIL "$CASES_PASS of $CASES_RUN sub-cases passed (see the lines above)"; exit 1; fi
