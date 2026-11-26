#!/bin/bash
# tools/vm/gui-choice-text.sh —— 界面专项：离线选择框的正文必须跟着「格式化目标分区」变（v2.10.8 修的缺陷 3）。
# 两种状态各读一次正文，读到之后核对「取消」按钮再点取消——不创建任务。
#
# ★ 这一项的直接被测对象就是对话框文字，所以判据落在文字上；其余读数（任务目录列表、启动配置）仍是结构化的。
# ★ 控件文字由常驻提权助手回传时是系统本地编码：一律经 common.sh 的 decode_text 解码后再比对
#   （2026-11-25 第一版按通用编码读，中文全成了替换字符，把两段明明不同的正文判成「无法识别」，
#   还把「取消」按钮认成了别的控件）。
# ★ 每一步点击都必须被确认（CLICK=OK）；没确认就中止本轮——带着没点成的一步往下走，只会干等一个不会出现的对话框
#   （2026-11-25 实测：点「创建任务」那一步通道静默没执行，脚本干等了 40 秒）。
#
# 用法：tools/vm/gui-choice-text.sh [数据镜像路径]     （约 2 分钟）
# 前提：虚拟机在运行且没人在用。
# 退出码：0 = 通过；1 = 有判据不满足；其它 = 步骤中止。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
DATA_WIM="${1:-$DATA_IMAGE}"
run_name "gui-choice"
fail=0

# judge_listing <已解码的枚举文件> <结论文件>：取正文（控件 2000）与「取消」按钮文字（控件 2003），给出键值行。
judge_listing() {
  python3 -I - "$EV/$1" <<'PY' > "$EV/$2"
import sys
body = cancel = None
want_body, want_cancel = 'id=2000 ', 'id=2003 '
for line in open(sys.argv[1], encoding='utf-8', errors='replace').read().splitlines():
    if 'text=' not in line:
        continue
    text = line[line.rfind('text=') + 5:].strip()
    if want_body in line:
        body = text
    if want_cancel in line:
        cancel = text
print("BODY=" + (body or '<missing>'))
print("CANCEL_OK=%s" % (cancel in ('取消', 'Cancel')))
# 勾选「格式化」：正文说目标卷将被格式化；未勾选：正文说明不格式化（PE 与 Windows RE 均适用）。
# 仍然先判「未勾选」，防止将来文案调整被抢先命中。
if body is None:
    kind = 'NONE'
elif ('未勾选' in body) or ('unchecked' in body):
    kind = 'NO_FORMAT'
elif ('将被格式化' in body) or ('will be formatted' in body):
    kind = 'FORMAT'
else:
    kind = 'UNKNOWN'
print("BODY_KIND=" + kind)
PY
  cut -c1-330 "$EV/$2"
}

# must_click <轮次> <类名> <控件编号>：点击必须被确认；没确认就打印原因并返回非零（本轮随即中止）
must_click() {
  local out
  out=$(gui_click "$2" "$3")
  echo "$out"
  case "$out" in
    *CLICK=OK*) return 0 ;;
    *) say "ABORT round $1: the click on control $3 was not confirmed (the channel did not run, or the helper refused to click)"; return 1 ;;
  esac
}

say "STEP=0 deployed binary"
deployed_hashes | head -1

say "STEP=1 nobody is using the desktop"
require_idle "$NAME-idle.txt" 120

say "STEP=2 resident elevated helper in the desktop session; helper scripts staged"
gui_ready
need_guest_file "$DATA_WIM" "the data image is missing: $DATA_WIM"

say "STEP=3 readings before"
TASKS0=$(gui_task_list); echo "  TASKS_BEFORE=$(printf '%s' "$TASKS0" | wc -w | tr -d ' ')" | tee "$EV/$NAME-tasks.txt"
bcd_readings > "$EV/$NAME-bcd-before.txt"

# run_round <轮次> <额外点击的控件编号…>：走一遍到选择框，读正文、核对「取消」后点取消。任何一步没走通都返回非零。
# 控件编号：1105 = 页签「还原数据到非 C 盘」；1213 = 通道选「离线」；1214 = 复选框「格式化目标分区」（默认勾选）；
#           1203 = 镜像路径输入框；1002 = 读取镜像；1003 = 创建任务；2000 = 选择框正文；2003 = 选择框的「取消」。
run_round() {
  local round="$1"; shift
  local id l st
  gui_launch
  must_click "$round" "$MAIN_CLASS" 1105 || return 1
  for id in "$@"; do must_click "$round" "$MAIN_CLASS" "$id" || return 1; done
  gui_set_image_path "$DATA_WIM" || { say "ABORT round $round: could not type the image path"; return 1; }
  must_click "$round" "$MAIN_CLASS" 1002 || return 1
  sleep 4
  must_click "$round" "$MAIN_CLASS" 1003 || return 1
  l=$(gui_wait_window MSGBOX 40) || { say "ABORT round $round: no destructive confirmation"; printf '%s\n' "$l" | head -8; return 1; }
  printf '%s\n' "$l" | grep -a -E '^WIN|id=(6|7) ' | cut -c1-200 | sed 's/^/  CONFIRM1 /'
  printf '%s\n' "$l" | grep -a -q 'id=6 ' || { say "ABORT round $round: no Yes button"; return 1; }
  must_click "$round" MSGBOX 6 || return 1
  l=$(gui_wait_window "$CHOICE_CLASS" 40) || { say "ABORT round $round: no choice dialog"; printf '%s\n' "$l" | head -8; return 1; }
  printf '%s\n' "$l" > "$EV/$NAME-choice-round$round.txt"
  judge_listing "$NAME-choice-round$round.txt" "$NAME-choice-round$round.verdict.txt"
  if grep -a -q '^CANCEL_OK=True' "$EV/$NAME-choice-round$round.verdict.txt"; then
    say "  control 2003 is the Cancel button (decoded); clicking it"
    must_click "$round" "$CHOICE_CLASS" 2003 || return 1
    sleep 3
  else
    say "  control 2003 is NOT the Cancel button or could not be decoded; not clicking anything (the next launch closes the GUI)"
    return 1
  fi
  return 0
}

for round in 1 2; do
  if [ "$round" = 1 ]; then label="format checked (default)"; else label="format checkbox clicked once (unchecked)"; fi
  say "STEP=4 round $round: $label"
  if [ "$round" = 1 ]; then run_round 1 1213 || fail=1; else run_round 2 1213 1214 || fail=1; fi
  TASKS_NOW=$(gui_task_list); echo "  TASKS_AFTER_ROUND_$round=$(printf '%s' "$TASKS_NOW" | wc -w | tr -d ' ')" | tee -a "$EV/$NAME-tasks.txt"
done

say "STEP=5 judge"
K1=$(sed -n 's/^BODY_KIND=\(.*\)$/\1/p' "$EV/$NAME-choice-round1.verdict.txt" 2>/dev/null | tail -1)
K2=$(sed -n 's/^BODY_KIND=\(.*\)$/\1/p' "$EV/$NAME-choice-round2.verdict.txt" 2>/dev/null | tail -1)
B1=$(sed -n 's/^BODY=\(.*\)$/\1/p' "$EV/$NAME-choice-round1.verdict.txt" 2>/dev/null | tail -1)
B2=$(sed -n 's/^BODY=\(.*\)$/\1/p' "$EV/$NAME-choice-round2.verdict.txt" 2>/dev/null | tail -1)
printf 'ROUND1_KIND=%s ROUND2_KIND=%s\n' "${K1:-NO_DIALOG}" "${K2:-NO_DIALOG}" | tee -a "$EV/$NAME-verdict.txt"
if [ "${K1:-}" = FORMAT ] && [ "${K2:-}" = NO_FORMAT ] && [ -n "$B1" ] && [ -n "$B2" ] && [ "$B1" != "$B2" ]; then
  verdict CHOICE_TEXT PASS "the body follows the checkbox: formatted / not formatted, two different texts"
else
  verdict CHOICE_TEXT FAIL "round1=${K1:-NO_DIALOG} round2=${K2:-NO_DIALOG}"; fail=1
fi
TASKS1=$(gui_task_list)
bcd_readings > "$EV/$NAME-bcd-after.txt"
SAME=$(bcd_same "$EV/$NAME-bcd-before.txt" "$EV/$NAME-bcd-after.txt")
if [ "$TASKS0" = "$TASKS1" ] && [ "$SAME" = "BCD_READINGS_EQUAL=True" ]; then
  verdict CHOICE_SIDE_EFFECTS PASS "task list and boot configuration unchanged"
else
  verdict CHOICE_SIDE_EFFECTS FAIL "$SAME"; fail=1
fi

if [ $fail -eq 0 ]; then verdict GUI_CHOICE PASS; else verdict GUI_CHOICE FAIL "(see the lines above)"; exit 1; fi
