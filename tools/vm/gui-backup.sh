#!/bin/bash
# tools/vm/gui-backup.sh <cancel|online|system> —— 界面驱动的「备份」入口（纯净界面，不用测试钩子）。
#
# 为什么要有它（2026-11-25）：读代码时发现，备份入口的「破坏性确认」框点「否」并不中止——
#   这个框是「是 / 否」两个按钮、警示语以「确认继续？」结尾，但只有三个还原入口认「否」（从 2026-10-13 起就是这样）。
#   界面驱动一直只点「是」，所以没有暴露。另外，保留下来的界面日志里「创建任务 → 备份」这条下发路径
#   （界面自己拼参数、拉起提权准备进程）一次都没有走完过：12 次都停在「镜像路径为空」的引导框上。
#
# 三种模式：
#   cancel  「破坏性确认」点「否」必须停下。两种分支各一轮，都不该建任务、不该写镜像：
#             系统盘分支（源卷 = 当前系统盘，也是备份页签的默认源卷）：点「否」之后不得弹出处理方式选择框；
#             在线分支（源卷 = 测试卷）：点「否」之后界面日志里不得出现 online started。
#           约 2 分钟。v2.10.11 之前的二进制上这一项必须判失败（缺陷复现；脚本会把弹出来的窗口收掉）。
#   online  界面在线备份全链：源卷 = 测试卷，镜像写到程序盘 H:\BRImages，从点按钮到弹出「执行完成」。
#           测试卷里主要是 20 GiB 的全零占位文件，镜像很小（几百 MB）。约 5 分钟。跑完删掉镜像。
#   system  界面系统盘备份经恢复环境全链：源卷 = 系统盘，镜像写到测试卷，重启两次。约 20 分钟。跑完删掉镜像。
#           ★ 有空闲空间的前置检查：测试卷所在的虚拟磁盘在宿主机上按需增长，镜像多大宿主就涨多大。
#             宿主空闲不足「估算镜像大小 × 2.5」、或测试卷空闲不足「× 1.5」时直接中止，不硬跑。
#
# 控件编号：1101 = 页签「备份」；1202 = 源卷下拉框；1203 = 镜像路径输入框；1003 = 创建任务；
#           消息框（类 #32770）：6 = 是，7 = 否，1 = 确定；处理方式选择框：2003 = 取消。
#
# 用法：tools/vm/gui-backup.sh cancel|online|system
# 前提：虚拟机在运行且没人在用；部署位是待测的二进制。
# 退出码：0 = 全部判据通过；1 = 有判据不满足；其它 = 步骤中止。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
MODE="${1:-}"
case "$MODE" in
  cancel|online|system) ;;
  *) echo "用法: tools/vm/gui-backup.sh <cancel|online|system>"; echo "GUI_BACKUP=ABORT reason=usage"; exit 2 ;;
esac
run_name "gui-backup-$MODE"
fail=0
STAMP="$(date +%Y%m%d-%H%M%S)"
GUI_LOG='H:\brwork\logs\gui.log'
SRC=""

# ---- 读数与驱动的小函数 -------------------------------------------------------------------
system_letter() { vrun "echo %SystemDrive%" 3 | tr -cd '\11\12\40-\176' | grep -a -o -E '^[A-Za-z]:' | head -1 | tr -d ':' | tr 'a-z' 'A-Z'; }
# file_bytes <客体路径>：客体里一个文件的字节数。读不到输出空串。
file_bytes() { vrun "for %I in ($1) do @echo BYTES=%~zI" 3 | tr -cd '\11\12\40-\176\r' | tr -d '\r' | sed -n 's/^BYTES=\([0-9][0-9]*\).*/\1/p' | head -1; }
# guest_free_bytes <盘符>：客体里这个卷的空闲字节数。读不到输出空串。
guest_free_bytes() { vrun "powershell -NoProfile -Command \"(Get-Volume -DriveLetter $1).SizeRemaining\"" 3 | tr -cd '\11\12\40-\176\r' | tr -d '\r' | grep -a -E '^[0-9]+$' | head -1; }
# host_free_bytes：宿主机数据卷的空闲字节数（df -k 的单位是 1024 字节）。
host_free_bytes() { df -k /System/Volumes/Data | awk 'NR==2 {print $4*1024}'; }
# online_dirs：在线备份的候选目录（H:\BRImages\.br-online-*）。读不到输出空串。
online_dirs() {
  vrun "powershell -NoProfile -Command \"(Get-ChildItem -Force -Directory 'H:\\BRImages' -Filter '.br-online-*' | Sort-Object Name | ForEach-Object Name) -join ' '\"" 3 \
    | tr -cd '\11\12\40-\176' | tr -d '\r' | grep -a -v 'VM_DONE' | head -3 | tr '\n' ' '
}
# num <可能不是数字的串>：不是纯数字一律当 0（判据里「读不到」由专门的键点名，不靠这里）
num() { case "${1:-}" in ''|*[!0-9]*) echo 0 ;; *) echo "$1" ;; esac; }

# open_backup_tab：启动界面、切到页签「备份」，读出源卷下拉框当前选中的卷。失败返回非零。
open_backup_tab() {
  local title ver
  gui_launch
  gui_must_click "backup" "$MAIN_CLASS" 1101 || return 1
  # 界面标题里带着部署位的版本号：与宿主侧的版本号不一致时点名（证据文件名用的是宿主侧的版本号）。
  title=$(elev uidlglist.ps1 -Class "$MAIN_CLASS" 2>/dev/null | decode_text | sed -n 's/^WIN hwnd=[0-9]* class=[^ ]* title=//p' | head -1)
  ver=$(printf '%s' "$title" | sed -n 's/.* v\([0-9][0-9.]*\)$/\1/p')
  echo "DEPLOYED_GUI_VERSION=${ver:-<unreadable>} HOST_VERSION=$VERSION"
  [ "$ver" = "$VERSION" ] || say "WARNING: the deployed binary is v${ver:-<unreadable>} but the host-side VERSION is $VERSION; evidence file names carry the host-side version"
  SRC=$(gui_combo_selected 1202)
  [ -n "$SRC" ] || { say "ABORT: could not read which source volume is selected"; return 1; }
  say "SOURCE_SELECTED=$(printf '%s' "$SRC" | cut -c1-70)"
}

# select_source <盘符>：源卷下拉框选中的必须是指定的卷——不是就用消息直选，再读回核对。
select_source() {
  local out
  printf '%s' "$SRC" | grep -a -q -i "^$1:" && return 0
  gui_select_combo 1202 "$1:" | sed 's/^/  COMBO /'
  out=$(gui_combo_selected 1202)
  SRC="$out"
  printf '%s' "$out" | grep -a -q -i "^$1:" || { say "ABORT: the source combo did not end up on $1: (read back: [$(printf '%s' "$out" | cut -c1-70)])"; return 1; }
}

# open_confirm_box <标签> <镜像路径>：填镜像路径 → 记下当前的窗口清单 → 点「创建任务」→ 等破坏性确认框。
# 确认框里必须同时有「是」(6) 与「否」(7)。清单写进证据文件；失败返回非零。
open_confirm_box() {
  local l
  gui_set_image_path "$2" || { say "ABORT $1: could not type the image path"; return 1; }
  gui_windows "$NAME-$1-windows-before.txt" | grep -a '^WINDOWS=' | sed 's/^/  WINDOWS_BEFORE_LINE /'
  gui_must_click "$1" "$MAIN_CLASS" 1003 || return 1
  l=$(gui_wait_window MSGBOX 40) || { say "ABORT $1: the destructive confirmation did not appear"; printf '%s\n' "$l" | head -8; return 1; }
  printf '%s\n' "$l" > "$EV/$NAME-$1-confirm.txt"
  printf '%s\n' "$l" | grep -a -E '^WIN|id=(1|6|7) ' | cut -c1-200 | sed 's/^/  CONFIRM /'
  printf '%s\n' "$l" | grep -a -q 'id=6 ' && printf '%s\n' "$l" | grep -a -q 'id=7 '
}

# after_answer <标签> <秒数>：在确认框上答完之后等一小会儿，记下窗口清单，取回界面日志并判读
# （界面日志只数 $T0 之后的；窗口清单与「点创建任务之前」的那份比）。
after_answer() {
  local t=0
  while [ $t -lt "$2" ]; do sleep 3; t=$((t+3)); done
  gui_windows "$NAME-$1-windows-after.txt" | grep -a -E '^WIN' | cut -c1-170 | sed 's/^/  WINDOW_AFTER /'
  fetch "$GUI_LOG" "$NAME-$1-gui.raw.log" && say "fetched gui.log"
  {
    python3 -I "$HERE/judge/gui-dispatch.py" "$EV/$NAME-$1-gui.raw.log" "$T0"
    windows_judge "$1"
  } | tee "$EV/$NAME-$1-readings.txt"
}
# rd <键> <标签>：从某一轮的读数文件里取一个键的值（取不到输出空串）
rd() { sed -n "s/^$1=\(.*\)$/\1/p" "$EV/$NAME-$2-readings.txt" 2>/dev/null | tail -1 | tr -d '\r'; }
# （点掉结果框之后等窗口收干净：settle_windows；读结果框上那颗按钮的编号：result_box_button_id——都在 common.sh）

# judge_cancel_round <标签> <不许出现的那两个键> <结论键> <说明>：点「否」之后的判据。
judge_cancel_round() {
  local tag="$1" k1="$2" k2="$3" key="$4" what="$5" lines canc went1 went2 boxes new after blocked errlog tasks1 rc
  lines=$(num "$(rd GUI_LOG_LINES_AFTER "$tag")"); canc=$(num "$(rd GUI_LOG_CANCELLED "$tag")")
  went1=$(num "$(rd "$k1" "$tag")"); went2=$(num "$(rd "$k2" "$tag")")
  boxes=$(rd MESSAGE_BOXES_AFTER "$tag"); new=$(rd NEW_WINDOW_CLASSES_AFTER "$tag"); after=$(num "$(rd WINDOWS_AFTER "$tag")")
  blocked=$(num "$(rd GUI_LOG_BLOCKED "$tag")"); errlog=$(num "$(rd GUI_LOG_ERROR "$tag")")
  tasks1=$(gui_task_list); echo "TASKS_AFTER_$tag=$(printf '%s' "$tasks1" | wc -w | tr -d ' ')" | tee -a "$EV/$NAME-tasks.txt"
  guest_exists "$NEVER_IMAGE"; rc=$?
  echo "NEVER_IMAGE_EXISTS_$tag=$([ $rc -eq 0 ] && echo True || echo False) reading_rc=$rc" | tee -a "$EV/$NAME-$tag-readings.txt"
  if [ "$lines" -lt 1 ]; then
    verdict "$key" FAIL "no GUI log line after $T0 (the log could not be bounded to this run; reading failed is not a verdict)"; fail=1; return
  fi
  if [ "$boxes" = "<unreadable>" ] || [ -z "$boxes" ]; then
    verdict "$key" FAIL "the window listing after the answer could not be read (reading failed is not a verdict)"; fail=1; return
  fi
  # 「之后一个窗口都没有」不算通过：主窗口必须还在（读成 0 个窗口多半是没读到界面进程）。
  if [ "$canc" -eq 1 ] && [ "$went1" -eq 0 ] && [ "$went2" -eq 0 ] && [ "$(num "$boxes")" -eq 0 ] && [ -z "$new" ] && [ "$after" -ge 1 ] \
     && [ "$blocked" -eq 0 ] && [ "$errlog" -eq 0 ] && [ "$tasks1" = "$TASKS0" ] && [ $rc -eq 1 ]; then
    verdict "$key" PASS "NO on the confirmation stopped the $what backup: one 'cancelled' line, nothing went on, no new window, no task, no image"
  else
    verdict "$key" FAIL "$what: cancelled=$canc $(printf '%s' "$k1" | sed 's/^GUI_LOG_//')=$went1 $(printf '%s' "$k2" | sed 's/^GUI_LOG_//')=$went2 message_boxes_after=$boxes new_window_classes=[${new}] windows_after=$after blocked=$blocked error=$errlog tasks_$([ "$tasks1" = "$TASKS0" ] && echo same || echo CHANGED) never_image_exists=$([ $rc -eq 0 ] && echo True || echo False)"
    fail=1
  fi
}

# wim_readings <镜像路径>：产品自己的镜像信息命令（主读数）+ 镜像工具的信息命令（另一条代码路径）+ 文件清单。
wim_readings() {
  local rc1 rc2
  vraw "$EXE wim-info $1 --json" "$NAME-wim-info.txt"; rc1=$?
  vraw "H:\\brwork\\wimlib-imagex.exe info $1" "$NAME-wimlib-info.txt"; rc2=$?
  echo "WIM_INFO_COMMAND_RAN=$([ $rc1 -eq 0 ] && echo True || echo False) IMAGE_TOOL_COMMAND_RAN=$([ $rc2 -eq 0 ] && echo True || echo False)"
  vrun "dir /b $1*" 3 | tr -cd '\11\12\40-\176\r' | tr -d '\r' | grep -a -F "$IMAGE_KEY" | sed 's/^/IMAGE_FILE_AFTER /' | tee "$EV/$NAME-images.txt"
  {
    python3 -I "$HERE/judge/wim-info.py" "$EV/$NAME-wim-info.txt" "$VERSION"
    n=$(tr -cd '\11\12\40-\176\r' < "$EV/$NAME-wimlib-info.txt" | tr -d '\r' | sed -n 's/^Image Count:[[:space:]]*\([0-9][0-9]*\).*/\1/p' | head -1)
    echo "WIMLIB_IMAGE_COUNT=${n:-<unreadable>}"
  } | tee "$EV/$NAME-image-readings.txt"
}

# judge_image <结论键>：镜像的判据——镜像与副档都在；产品说只有 1 个索引、是当前版本写出来的；镜像工具也说只有 1 个索引。
judge_image() {
  local n cnt size ver wlib
  n=$(grep -a -c 'IMAGE_FILE_AFTER' "$EV/$NAME-images.txt")
  cnt=$(rd_any WIM_IMAGE_COUNT "$EV/$NAME-image-readings.txt"); size=$(rd_any WIM_INDEX1_SIZE "$EV/$NAME-image-readings.txt")
  ver=$(rd_any WIM_VERSION_MATCH "$EV/$NAME-image-readings.txt"); wlib=$(rd_any WIMLIB_IMAGE_COUNT "$EV/$NAME-image-readings.txt")
  if [ "$n" -ge 2 ] && [ "$(num "$cnt")" -eq 1 ] && [ "$(num "$size")" -gt 0 ] && [ "$ver" = "True" ] && [ "$(num "$wlib")" -eq 1 ] && [ "$wlib" != "<unreadable>" ]; then
    verdict "$1" PASS "image and sidecar exist; the program reports one index of $size bytes written by $VERSION; the image tool also reports one index"
  else
    verdict "$1" FAIL "image_files=$n indexes=[$cnt] size=[$size] version_match=[$ver] image_tool_count=[$wlib]"; fail=1
  fi
}
rd_any() { sed -n "s/^$1=\(.*\)$/\1/p" "$2" 2>/dev/null | tail -1 | tr -d '\r'; }

# remove_image <镜像路径>：删掉镜像与副档，并核对真的不在了（在 = 没删掉；读不到 = 单独点名）。
remove_image() {
  local rc
  vrun "del /f /q $1 $1.index-1.metadata.json" 3 | grep -a 'VM_DONE' | sed 's/^/  IMAGE_REMOVE /'
  guest_exists "$1"; rc=$?
  case $rc in
    0) echo "IMAGE_REMOVED=False" | tee -a "$EV/$NAME-verdict.txt"; fail=1 ;;
    1) echo "IMAGE_REMOVED=True" | tee -a "$EV/$NAME-verdict.txt" ;;
    *) echo "IMAGE_REMOVED=<unreadable>" | tee -a "$EV/$NAME-verdict.txt"; fail=1 ;;
  esac
}

# ==================================================================================================
say "STEP=0 deployed binaries (the version on the host side: $VERSION)"
deployed_hashes | head -1

say "STEP=1 boot configuration: file-level backup, verified it reads offline; readings before"
bcd_backup "$NAME" | tee "$EV/$NAME-bcd-file.txt"
bcd_readings > "$EV/$NAME-bcd-before.txt"
TASKS0=$(gui_task_list); echo "TASKS_BEFORE=$(printf '%s' "$TASKS0" | wc -w | tr -d ' ')" | tee "$EV/$NAME-tasks.txt"
TASK_BEFORE=$(newest_task); say "NEWEST_TASK_BEFORE=${TASK_BEFORE:-<unreadable>}"

say "STEP=2 nobody is using the desktop; resident elevated helper; helper scripts staged"
require_idle "$NAME-idle.txt" 120
gui_ready
SYSTEM_LETTER=$(system_letter)
[ -n "$SYSTEM_LETTER" ] || { say "ABORT: could not read the system drive letter"; exit 3; }
say "SYSTEM_DRIVE=${SYSTEM_LETTER}:  TEST_DRIVE=${SOURCE_DRIVE}:"

case "$MODE" in
# ------------------------------------------------------------------ cancel
cancel)
  NEVER_IMAGE='F:\br-gui-cancel-never.wim'

  say "STEP=3 cancel, round S (system branch): source = the system drive; NO on the confirmation"
  T0=""
  if open_backup_tab && printf '%s' "$SRC" | grep -a -q -i "^${SYSTEM_LETTER}:"; then
    T0=$(guest_utc_now); say "GUEST_UTC_AT_DRIVE_START=${T0:-<unreadable>}"
    if [ -n "$T0" ] && open_confirm_box sys "$NEVER_IMAGE" && gui_must_click sys MSGBOX 7; then
      after_answer sys 10
      # 缺陷的样子：点「否」之后照样弹出处理方式选择框。核对「取消」按钮后点掉，不让它挂着。
      if grep -a -q "class=$CHOICE_CLASS " "$EV/$NAME-sys-windows-after.txt"; then
        say "DEFECT: the handler-choice dialog appeared although NO was clicked (this is the v2.10.10 behaviour); dismissing it with Cancel"
        l=$(gui_wait_window "$CHOICE_CLASS" 10)
        if [ "$(control_text "$l" 2003)" = "取消" ] || [ "$(control_text "$l" 2003)" = "Cancel" ]; then gui_must_click sys "$CHOICE_CLASS" 2003; else say "control 2003 is not the Cancel button; not clicking anything"; fi
      fi
      judge_cancel_round sys GUI_LOG_SYSTEM_CHOICE_ANY GUI_LOG_ONLINE_STARTED GUI_BACKUP_CANCEL_SYSTEM "system-drive"
    else
      verdict GUI_BACKUP_CANCEL_SYSTEM FAIL "the system-drive round did not reach the confirmation (see the lines above)"; fail=1
    fi
  else
    verdict GUI_BACKUP_CANCEL_SYSTEM FAIL "the backup tab did not open, or its default source is not the system drive (read: [$(printf '%s' "$SRC" | cut -c1-70)])"; fail=1
  fi

  say "STEP=4 cancel, round O (online branch): source = the test volume; NO on the confirmation"
  NEVER_IMAGE='H:\BRImages\gui-cancel-never.wim'
  ONLINE_DIRS0=$(online_dirs); say "ONLINE_WORKSPACES_BEFORE=[${ONLINE_DIRS0}]"
  T0=""
  if open_backup_tab && select_source "$SOURCE_DRIVE"; then
    T0=$(guest_utc_now); say "GUEST_UTC_AT_DRIVE_START=${T0:-<unreadable>}"
    if [ -n "$T0" ] && open_confirm_box onl "$NEVER_IMAGE" && gui_must_click onl MSGBOX 7; then
      after_answer onl 10
      if [ "$(num "$(rd GUI_LOG_ONLINE_STARTED onl)")" -ge 1 ]; then
        # 缺陷的样子：点「否」之后在线备份真的开始了。结束界面程序，删掉它刚写的候选目录（只删本轮新出现的）
        # 和已经发布出来的镜像——测试卷的镜像很小，几秒就能做完（2026-11-25 实机：10 秒内已经弹出了「执行完成」）。
        say "DEFECT: the GUI started an online backup although NO was clicked (this is the v2.10.10 behaviour); closing the GUI and removing what it wrote"
        gui_close; sleep 4
        for d in $(online_dirs); do
          case " $ONLINE_DIRS0 " in *" $d "*) continue ;; esac
          vrun "rmdir /s /q H:\\BRImages\\$d" 3 | grep -a 'VM_DONE' | sed "s/^/  REMOVED_WORKSPACE $d /"
        done
        vrun "del /f /q $NEVER_IMAGE $NEVER_IMAGE.index-1.metadata.json" 3 | grep -a 'VM_DONE' | sed 's/^/  REMOVED_PUBLISHED_IMAGE /'
      fi
      judge_cancel_round onl GUI_LOG_ONLINE_SPLIT GUI_LOG_ONLINE_STARTED GUI_BACKUP_CANCEL_ONLINE "online"
    else
      verdict GUI_BACKUP_CANCEL_ONLINE FAIL "the online round did not reach the confirmation (see the lines above)"; fail=1
    fi
  else
    verdict GUI_BACKUP_CANCEL_ONLINE FAIL "could not select the test volume as the source (read: [$(printf '%s' "$SRC" | cut -c1-70)])"; fail=1
  fi
  ;;
# ------------------------------------------------------------------ online
online)
  IMAGE_WIN="H:\\BRImages\\gui-online-$STAMP.wim"
  IMAGE_KEY="gui-online-$STAMP"
  NEVER_IMAGE="$IMAGE_WIN"

  say "STEP=3 online: source = the test volume; YES on the confirmation; the GUI runs the backup in its own thread"
  open_backup_tab && select_source "$SOURCE_DRIVE" || { verdict GUI_BACKUP_ONLINE FAIL "could not open the backup tab with the test volume as the source"; exit 3; }
  T0=$(guest_utc_now); say "GUEST_UTC_AT_DRIVE_START=${T0:-<unreadable>}"
  [ -n "$T0" ] || { say "ABORT: could not read the guest clock"; exit 3; }
  open_confirm_box onl "$IMAGE_WIN" && gui_must_click onl MSGBOX 6 || { verdict GUI_BACKUP_ONLINE FAIL "did not get past the confirmation"; exit 3; }

  say "STEP=4 the GUI must have started the backup within 30 s (otherwise there is nothing to wait for)"
  started=0; t=0
  while [ $t -lt 30 ]; do
    sleep 5; t=$((t+5))
    fetch "$GUI_LOG" "$NAME-gui.raw.log" && n=$(python3 -I "$HERE/judge/gui-dispatch.py" "$EV/$NAME-gui.raw.log" "$T0" | sed -n 's/^GUI_LOG_ONLINE_STARTED=\([0-9]*\).*/\1/p' | tail -1)
    [ "${n:-0}" -ge 1 ] && { started=1; break; }
  done
  [ $started -eq 1 ] || { gui_windows "$NAME-windows-stuck.txt" | grep -a -E '^WIN' | cut -c1-170; verdict GUI_BACKUP_ONLINE FAIL "no 'online started' line within 30 s after YES (was the click confirmed?)"; exit 1; }
  say "ONLINE_STARTED=yes after ${t}s"

  say "STEP=5 wait for the result box (a message box with a single OK button)"
  l=$(gui_wait_window MSGBOX 1500) || { verdict GUI_BACKUP_ONLINE FAIL "no result box within 1500 s"; exit 1; }
  printf '%s\n' "$l" > "$EV/$NAME-result-box.txt"
  printf '%s\n' "$l" | grep -a -E '^WIN|id=1 ' | cut -c1-200 | sed 's/^/  RESULT_BOX /'
  # 结果框只有一个按钮；它的控件编号从清单里读出来再点——实测是 2，不是消息框惯用的 1（第一版写死了 1，
  # 结果框没被点掉，留在桌面上被判成残留）。不是恰好一颗按钮就不点，直接判失败。
  ok_ids=$(result_box_button_id "$l")
  if [ "$(printf '%s' "$ok_ids" | wc -w | tr -d ' ')" = 1 ]; then
    say "RESULT_BOX_BUTTON_ID=$ok_ids"; gui_must_click onl MSGBOX "$ok_ids"
  else
    say "ABORT: the result box does not have exactly one button (ids: [$ok_ids]); not clicking anything"; verdict GUI_WINDOWS_CLEAN FAIL "result box buttons=[$ok_ids]"; fail=1
  fi
  settle_windows onl 5

  say "STEP=6 collect evidence verbatim"
  fetch "$GUI_LOG" "$NAME-onl-gui.raw.log" && say "fetched gui.log"
  {
    python3 -I "$HERE/judge/gui-dispatch.py" "$EV/$NAME-onl-gui.raw.log" "$T0"
    python3 -I "$HERE/judge/windows.py" "$EV/$NAME-onl-windows-before.txt" "$EV/$NAME-onl-windows-after.txt"
  } | tee "$EV/$NAME-onl-readings.txt"
  wim_readings "$IMAGE_WIN"

  say "STEP=7 judge"
  L=$(num "$(rd GUI_LOG_LINES_AFTER onl)"); SPLIT=$(num "$(rd GUI_LOG_ONLINE_SPLIT onl)"); ST=$(num "$(rd GUI_LOG_ONLINE_STARTED onl)")
  DONE=$(num "$(rd GUI_LOG_ONLINE_DONE onl)"); BAD=$(num "$(rd GUI_LOG_ONLINE_FAILED onl)"); BL=$(num "$(rd GUI_LOG_BLOCKED onl)")
  ER=$(num "$(rd GUI_LOG_ERROR onl)"); AF=$(num "$(rd GUI_LOG_ACTION_FAILED onl)"); CA=$(num "$(rd GUI_LOG_CANCELLED onl)")
  BOXES=$(rd MESSAGE_BOXES_AFTER onl); NEW=$(rd NEW_WINDOW_CLASSES_AFTER onl)
  if [ "$L" -lt 1 ]; then
    verdict GUI_DISPATCH FAIL "no GUI log line after $T0 (the log could not be bounded to this run; reading failed is not a verdict)"; fail=1
  elif [ "$SPLIT" -ge 1 ] && [ "$ST" -ge 1 ] && [ "$DONE" -ge 1 ] && [ "$BAD" -eq 0 ] && [ "$BL" -eq 0 ] && [ "$ER" -eq 0 ] && [ "$AF" -eq 0 ] && [ "$CA" -eq 0 ]; then
    verdict GUI_DISPATCH PASS "since $T0: $L GUI log lines; split to the online branch, started, completed with success=true, nothing blocked or failed"
  else
    verdict GUI_DISPATCH FAIL "lines=$L split=$SPLIT started=$ST done=$DONE failed=$BAD blocked=$BL error=$ER action_failed=$AF cancelled=$CA"; fail=1
  fi
  if [ -n "$BOXES" ] && [ "$BOXES" != "<unreadable>" ] && [ "$(num "$BOXES")" -eq 0 ] && [ -z "$NEW" ]; then
    verdict GUI_WINDOWS_CLEAN PASS "after OK on the result box no message box and no new window is left"
  else
    verdict GUI_WINDOWS_CLEAN FAIL "message_boxes=$BOXES new_window_classes=[${NEW}]"; fail=1
  fi
  judge_image GUI_BACKUP_ONLINE_IMAGE
  ;;
# ------------------------------------------------------------------ system
system)
  IMAGE_WIN="F:\\br-gui-system-$STAMP.wim"
  IMAGE_KEY="br-gui-system-$STAMP"
  NEVER_IMAGE="$IMAGE_WIN"

  say "STEP=3 free space before a system image is written (the test volume's virtual disk grows on the host as it fills)"
  EST=$(file_bytes "$SYS_IMAGE")
  [ -n "$EST" ] || { say "ABORT: could not read the size of $SYS_IMAGE (used as the estimate); reading failed is not the same as no space needed"; exit 3; }
  HOST_FREE=$(host_free_bytes); GUEST_FREE=$(guest_free_bytes "$SOURCE_DRIVE")
  NEED_HOST=$(( EST * 5 / 2 )); NEED_GUEST=$(( EST * 3 / 2 ))
  printf 'ESTIMATE_BYTES=%s HOST_FREE_BYTES=%s NEED_HOST_BYTES=%s GUEST_FREE_BYTES=%s NEED_GUEST_BYTES=%s\n' "$EST" "${HOST_FREE:-<unreadable>}" "$NEED_HOST" "${GUEST_FREE:-<unreadable>}" "$NEED_GUEST" | tee "$EV/$NAME-space.txt"
  if [ -z "$HOST_FREE" ] || [ -z "$GUEST_FREE" ]; then
    say "ABORT: could not read the free space (reading failed is not the same as enough room)"; echo "GUI_BACKUP_SYSTEM=ABORT reason=space_unreadable"; exit 3
  fi
  if [ "$HOST_FREE" -lt "$NEED_HOST" ] || [ "$GUEST_FREE" -lt "$NEED_GUEST" ]; then
    say "ABORT: not enough free space for a system image of about $EST bytes (host needs $NEED_HOST, has $HOST_FREE; test volume needs $NEED_GUEST, has $GUEST_FREE)"
    say "       free space first (old snapshot layers, orphan disks, old test images); do not force it"
    echo "GUI_BACKUP_SYSTEM=ABORT reason=not_enough_free_space"; exit 3
  fi

  say "STEP=4 system: source = the system drive; YES, Windows RE, YES; the GUI dispatches the prepare itself"
  open_backup_tab && printf '%s' "$SRC" | grep -a -q -i "^${SYSTEM_LETTER}:" || { verdict GUI_BACKUP_SYSTEM FAIL "the backup tab did not open with the system drive as the source (read: [$(printf '%s' "$SRC" | cut -c1-70)])"; exit 3; }
  T0=$(guest_utc_now); say "GUEST_UTC_AT_DRIVE_START=${T0:-<unreadable>}"
  [ -n "$T0" ] || { say "ABORT: could not read the guest clock"; exit 3; }
  open_confirm_box sys "$IMAGE_WIN" && gui_must_click sys MSGBOX 6 || { verdict GUI_BACKUP_SYSTEM FAIL "did not get past the confirmation"; exit 3; }
  l=$(gui_wait_window "$CHOICE_CLASS" 40) || { verdict GUI_BACKUP_SYSTEM FAIL "no handler-choice dialog"; exit 3; }
  printf '%s\n' "$l" > "$EV/$NAME-choice.txt"
  printf '%s\n' "$l" | grep -a -E '^WIN|id=200[0-9] ' | cut -c1-240 | sed 's/^/  CHOICE /'
  gui_must_click sys "$CHOICE_CLASS" 2002 || { verdict GUI_BACKUP_SYSTEM FAIL "could not choose Windows RE"; exit 3; }
  l=$(gui_wait_window MSGBOX 40) || { verdict GUI_BACKUP_SYSTEM FAIL "no final confirmation"; exit 3; }
  printf '%s\n' "$l" > "$EV/$NAME-final-confirm.txt"
  printf '%s\n' "$l" | grep -a -E '^WIN|id=(6|7) ' | cut -c1-200 | sed 's/^/  FINAL_CONFIRM /'
  gui_must_click sys MSGBOX 6 || { verdict GUI_BACKUP_SYSTEM FAIL "could not confirm"; exit 3; }

  say "STEP=5 the task directory the GUI's prepare creates (the newest must change)"
  TASK=""; t=0
  while [ $t -lt 90 ]; do
    TASK=$(newest_task)
    [ -n "$TASK" ] && [ "$TASK" != "$TASK_BEFORE" ] && break
    keep_alive; sleep 2; t=$((t+2))
  done
  say "TASK_CREATED=${TASK:-<none>} after ${t}s"
  if [ -z "$TASK" ] || [ "$TASK" = "$TASK_BEFORE" ]; then
    gui_windows "$NAME-windows-stuck.txt" | grep -a -E '^WIN' | cut -c1-170
    verdict GUI_BACKUP_SYSTEM FAIL "the GUI did not create a task"; exit 1
  fi

  say "STEP=6 wait for the terminal stage (the machine reboots into WinRE and comes back)"
  wait_terminal "$TASK" 2400 | tee "$EV/$NAME-terminal.txt" || { verdict GUI_BACKUP_SYSTEM FAIL "no terminal stage within 2400 s; task=$TASK"; fail=1; }

  say "STEP=7 collect evidence verbatim"
  fetch "H:\\brwork\\tasks\\$TASK\\status.json" "$NAME-status.json" && say "fetched status.json"
  fetch 'H:\BRImages\Recovery.log' "$NAME-Recovery.raw.log" && say "fetched Recovery.log"
  fetch "$GUI_LOG" "$NAME-sys-gui.raw.log" && say "fetched gui.log"
  {
    status_field "$EV/$NAME-status.json" stage | sed 's/^/FINAL_STAGE=/'
    status_field "$EV/$NAME-status.json" progress | sed 's/^/FINAL_PROGRESS=/'
    python3 -I "$HERE/judge/restore-log.py" "$EV/$NAME-Recovery.raw.log" "$TASK" | grep -a -E '^(RECOVERY_SEGMENT_FOUND|RECOVERY_STEP_LABELS)='
    python3 -I "$HERE/judge/gui-dispatch.py" "$EV/$NAME-sys-gui.raw.log" "$T0"
  } | tee "$EV/$NAME-sys-readings.txt"
  wim_readings "$IMAGE_WIN"

  say "STEP=8 judge"
  STAGE=$(status_field "$EV/$NAME-status.json" stage); PROGRESS=$(status_field "$EV/$NAME-status.json" progress)
  LABELS=$(rd RECOVERY_STEP_LABELS sys)
  if [ "$STAGE" = "success" ] && [ "$PROGRESS" = "100" ] && [ "$LABELS" = "1,2,3,4" ]; then
    verdict GUI_BACKUP_SYSTEM_COMPLETED PASS "stage=success progress=100, STEP 1/4..4/4 all present, task=$TASK"
  else
    verdict GUI_BACKUP_SYSTEM_COMPLETED FAIL "stage=$STAGE progress=$PROGRESS step_labels=${LABELS:-none} task=$TASK"; fail=1
  fi
  L=$(num "$(rd GUI_LOG_LINES_AFTER sys)"); CH=$(num "$(rd GUI_LOG_SYSTEM_CHOICE_RE sys)"); CF=$(num "$(rd GUI_LOG_SYSTEM_CONFIRM_YES sys)")
  PL=$(num "$(rd GUI_LOG_PREPARE_LAUNCH sys)"); DG=$(num "$(rd GUI_LOG_DELEGATED sys)"); FE=$(num "$(rd GUI_LOG_PREPARE_FAILED_EARLY sys)")
  BL=$(num "$(rd GUI_LOG_BLOCKED sys)"); ER=$(num "$(rd GUI_LOG_ERROR sys)"); AF=$(num "$(rd GUI_LOG_ACTION_FAILED sys)"); OS=$(num "$(rd GUI_LOG_ONLINE_SPLIT sys)")
  if [ "$L" -lt 1 ]; then
    verdict GUI_DISPATCH FAIL "no GUI log line after $T0 (the log could not be bounded to this run; reading failed is not a verdict)"; fail=1
  elif [ "$CH" -ge 1 ] && [ "$CF" -ge 1 ] && [ "$PL" -ge 1 ] && [ "$DG" -ge 1 ] && [ "$FE" -eq 0 ] && [ "$BL" -eq 0 ] && [ "$ER" -eq 0 ] && [ "$AF" -eq 0 ] && [ "$OS" -eq 0 ]; then
    verdict GUI_DISPATCH PASS "since $T0: $L GUI log lines; chose Windows RE, confirmed, prepare launched and delegated, nothing blocked or failed"
  else
    verdict GUI_DISPATCH FAIL "lines=$L chose_re=$CH confirm_yes=$CF prepare_launch=$PL delegated=$DG failed_early=$FE blocked=$BL error=$ER action_failed=$AF online_split=$OS"; fail=1
  fi
  judge_image GUI_BACKUP_SYSTEM_IMAGE
  ;;
esac

say "STEP=9 readings after"
bcd_readings > "$EV/$NAME-bcd-after.txt"
SAME=$(bcd_same "$EV/$NAME-bcd-before.txt" "$EV/$NAME-bcd-after.txt")
printf '%s\n' "$SAME" | tee -a "$EV/$NAME-verdict.txt"
[ "$SAME" = "BCD_READINGS_EQUAL=True" ] || fail=1
WINRE=$(winre_enabled)
printf '%s\n' "$WINRE" | tee -a "$EV/$NAME-verdict.txt"
[ "$WINRE" = "WINRE_ENABLED=True" ] || fail=1

if [ "$MODE" = "online" ] || [ "$MODE" = "system" ]; then
  say "STEP=10 remove the image this run created"
  remove_image "$IMAGE_WIN"
fi
if [ "$MODE" = "system" ]; then
  say "host data volume after the run:"; df -h /System/Volumes/Data | tail -1 | sed 's/^/  HOST_DF /'
fi

if [ $fail -eq 0 ]; then verdict "GUI_BACKUP_$(echo "$MODE" | tr 'a-z' 'A-Z')" PASS "mode=$MODE"; else verdict "GUI_BACKUP_$(echo "$MODE" | tr 'a-z' 'A-Z')" FAIL "mode=$MODE (see the lines above)"; exit 1; fi
