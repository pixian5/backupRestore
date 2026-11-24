#!/bin/bash
# tools/gui-check/restore-confirm.sh —— 驱动图形界面走一遍还原流程到**最后一个确认框**（纯净界面，不用测试钩子）。
# 这是「界面三道确认」的驱动：破坏性确认 → 处理方式选择框 → 最后一个确认框。
#
# 用法：
#   tools/gui-check/restore-confirm.sh <页签控件编号> <镜像路径> <选择框按钮编号或none> <最后一个确认框: yes|no> [额外点击的控件编号...]
#     页签：1102=还原系统到 C 盘  1105=还原数据到非 C 盘  1106=还原系统到非 C 盘
#     选择框按钮：2001=进入 PE  2002=进入 Windows RE  none=预期不出现选择框（纯在线流程）
#
# ★★ 最后一个确认框答 yes = **真的下发任务并重启**（还原系统盘会抹掉系统盘上的改动）。
#    答 yes 必须同时设置环境变量 BR_ALLOW_RESTORE=yes，否则脚本在最后一步之前停下并判为中止。
#    日常验证用 no：走完三道确认、最后点「否」，任务列表与启动配置都必须不变（由调用方核对副作用）。
#
# 前提：tools/vm/lib/vm.sh 的 ensure_helper + stage_gui_helpers 已做过（脚本自己也会做一遍）。
# 退出码：0 = 流程走完且每一步都点到了；1 = 有点击没成功 / 窗口没出现；3 = 安全闸拦下（想答 yes 却没授权）。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../vm/lib/common.sh"
TAB="${1:?用法: restore-confirm.sh <页签> <镜像路径> <选择框按钮或none> <yes|no> [额外点击...]}"; IMAGE="$2"; CHOICE="$3"; FINAL="$4"; shift 4
ok_clicks=0; expected=0

click_ok() { # $1 类名  $2 控件编号：点击必须回 CLICK=OK
  local out
  expected=$((expected+1))
  out=$(elev uiclick.ps1 -Class "$1" -Id "$2" | tr '\n' ' ')
  case "$out" in *CLICK=OK*) ok_clicks=$((ok_clicks+1)); echo "$out" ;; *) echo "CLICK_FAILED class=$1 id=$2 $out" ;; esac
}
wait_window() { # $1 类名  $2 上限秒：输出已解码的控件枚举
  local cls="$1" limit=${2:-30} t=0 out=""
  while [ $t -lt "$limit" ]; do
    out=$(elev uidlglist.ps1 -Class "$cls" 2>/dev/null | decode_text)
    if printf '%s' "$out" | grep -a -q "^WINDOWS=1"; then printf '%s\n' "$out"; return 0; fi
    sleep 3; t=$((t+3))
  done
  printf '%s\n' "$out"; return 1
}

say "resident elevated helper in the desktop session; helper scripts staged"
gui_ready

say "启动纯净图形界面"; elev launch.ps1 | grep -a -E "PID=|MISSING|__END__"
sleep 5
say "切到页签 $TAB"; click_ok BackupRestoreNativeGui "$TAB"
for id in "$@"; do say "额外点击控件 $id"; click_ok BackupRestoreNativeGui "$id"; done

say "把镜像路径写进输入框（不模拟打字，避开中文输入法）"
st=""
for attempt in 1 2 3; do
  st=$(elev uitext2.ps1 -Id 1203 -Text "$IMAGE")
  printf '%s' "$st" | grep -a -q "match=True" && break
  say "写入路径第 $attempt 次没有得到确认，重试"; sleep 2
done
printf '%s\n' "$st" | grep -a -E "SETTEXT|settext" | head -3
printf '%s' "$st" | grep -a -q "match=True" || { say "路径写入后核对不一致，中止（未点创建任务）"; echo "RESTORE_CONFIRM=ABORT reason=image_path_not_confirmed"; exit 1; }

say "点击读取镜像"; click_ok BackupRestoreNativeGui 1002
sleep 4
say "索引下拉框状态"; elev comboitems.ps1 -Id 1206 | decode_text | grep -a -E "^COMBO|ITEM" | head -6
say "创建任务按钮文字"; printf '%s\n' "$(control_text "$(elev uidlglist.ps1 -Class BackupRestoreNativeGui | decode_text)" 1003)"

say "点击创建任务"; click_ok BackupRestoreNativeGui 1003
say "等待第一个确认框"; l=$(wait_window MSGBOX 30) || { say "没有出现确认框"; printf '%s\n' "$l" | head -12; echo "RESTORE_CONFIRM=FAIL reason=no_first_confirmation"; exit 1; }
printf '%s\n' "$l" | grep -a -E "^WIN|id=(1|2|3|6|7) " | cut -c1-200
printf '%s\n' "$l" | grep -a -q "id=6 " || { say "确认框里没有「是」按钮，中止"; echo "RESTORE_CONFIRM=FAIL reason=no_yes_button"; exit 1; }
click_ok MSGBOX 6

if [ "$CHOICE" != "none" ]; then
  say "等待处理方式选择框"; l=$(wait_window "$CHOICE_CLASS" 30) || { say "没有出现选择框"; printf '%s\n' "$l" | head -12; echo "RESTORE_CONFIRM=FAIL reason=no_choice_dialog"; exit 1; }
  printf '%s\n' "$l" | grep -a -E "^WIN|id=200[0-9] " | cut -c1-240
  say "点击选择框按钮 $CHOICE"; click_ok "$CHOICE_CLASS" "$CHOICE"
  say "等待最后一个确认框"; l=$(wait_window MSGBOX 30) || { say "没有出现确认框"; printf '%s\n' "$l" | head -12; echo "RESTORE_CONFIRM=FAIL reason=no_final_confirmation"; exit 1; }
  printf '%s\n' "$l" | grep -a -E "^WIN|id=(1|2|6|7) " | cut -c1-200
  btn=6; [ "$FINAL" = "no" ] && btn=7
  if [ "$btn" = 6 ] && [ "${BR_ALLOW_RESTORE:-}" != "yes" ]; then
    say "最后一个确认框要答「是」= 真的下发任务并重启；没有设置 BR_ALLOW_RESTORE=yes，停在这里不点"
    say "（选择框已经点过；界面停在这个确认框上，下一次启动界面程序即可复位）"
    echo "RESTORE_CONFIRM=ABORT reason=yes_requires_BR_ALLOW_RESTORE"
    exit 3
  fi
  say "点击最后确认框的按钮 $btn"; click_ok MSGBOX "$btn"
fi
say "流程驱动结束"
echo "RESTORE_CONFIRM_CLICKS_OK=$ok_clicks EXPECTED=$expected" | tee -a "$EV/gui-last-confirm.txt"
if [ "$ok_clicks" -eq "$expected" ] && [ "$expected" -gt 0 ]; then echo "RESTORE_CONFIRM=DRIVEN"; exit 0; fi
echo "RESTORE_CONFIRM=FAIL reason=clicks_not_all_ok"; exit 1
