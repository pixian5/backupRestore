#!/bin/bash
# tools/vm/gui-fix-check.sh —— 界面专项两项（都只走到对话框为止、或最后点「否」，不创建任务）：
#   A. 进入「还原数据到非 C 盘」时，目标下拉的默认选中不能是光驱（v2.10.8 修的缺陷 2）。
#   C. A 入口（还原系统到 C 盘）用纯净界面走三道确认，最后点「否」→ 零副作用：任务列表与启动配置都不变。
# 选择框两种文案那一项在 tools/vm/gui-choice-text.sh。
#
# 判据全部来自外部测量（读取控件的真实选中项与文字）与副作用读数（任务目录列表、启动配置读数）。
# 用法：tools/vm/gui-fix-check.sh          （约 2 分钟）
# 前提：虚拟机在运行且没人在用；测试机上有基线系统镜像（C 项要读一份完整 Windows 镜像）。
# 退出码：0 = 两项全过；1 = 有判据不满足；其它 = 步骤中止。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
run_name "gui-fix"
fail=0

say "STEP=0 deployed binary"
deployed_hashes | head -1

say "STEP=1 nobody is using the desktop"
require_idle "$NAME-idle.txt" 120

say "STEP=2 resident elevated helper in the desktop session; helper scripts staged"
gui_ready
[ -n "$(vrun "dir /b $SYS_IMAGE" 2 | tr -cd '\11\12\40-\176\r' | tr -d '\r' | grep -a '\.wim')" ] || { say "ABORT: the system image for item C is missing: $SYS_IMAGE"; exit 3; }

say "STEP=3 readings before"
TASKS0=$(gui_task_list); echo "  TASKS_BEFORE=$(printf '%s' "$TASKS0" | wc -w | tr -d ' ')" | tee "$EV/$NAME-tasks.txt"
bcd_readings > "$EV/$NAME-bcd-before.txt"

say "STEP=A entering 'restore data to a non-C drive': the default target must not be the optical drive"
gui_launch
gui_click "$MAIN_CLASS" 1105
sleep 2
gui_combo_items 1204 > "$EV/$NAME-target-combo.txt"
cut -c1-200 "$EV/$NAME-target-combo.txt"
python3 -I - "$EV/$NAME-target-combo.txt" <<'PY' | tee "$EV/$NAME-verdict-A.txt"
import re, sys
text = open(sys.argv[1], encoding='utf-8', errors='replace').read()
m = re.search(r'^COMBO id=1204 count=(\d+) selected=(-?\d+)', text, flags=re.M)
items = {int(i): t for i, t in re.findall(r'^\s*ITEM\[(\d+)\]=(.*)$', text, flags=re.M)}
def optical(item):
    u = item.upper()
    return item.startswith('D:') or 'UDF' in u or 'CDFS' in u
if not m:
    print("A_RESULT=READ_FAILED reason=no COMBO line (reading failed is not a verdict)")
    print("DEFAULT_TARGET=FAIL")
else:
    sel = int(m.group(2)); chosen = items.get(sel, '<missing>')
    print("A_SELECTED_INDEX=%d" % sel)
    print("A_ITEM_COUNT=%s" % m.group(1))
    print("A_OPTICAL_INDEXES=%s" % sorted(k for k, v in items.items() if optical(v)))
    good = bool(items) and not optical(chosen)
    print("DEFAULT_TARGET=" + ("PASS" if good else "FAIL (the default target is the optical drive)"))
PY
grep -a -q '^DEFAULT_TARGET=PASS' "$EV/$NAME-verdict-A.txt" || fail=1

say "STEP=C three confirmations on entry A (restore Windows to C:), answering No at the last one"
bash "$HERE/../gui-check/restore-confirm.sh" 1102 "$SYS_IMAGE" 2002 no 2>&1 | cut -c1-220 | sed 's/^/  C /'
CRC=${PIPESTATUS[0]}
sleep 4
TASKS1=$(gui_task_list); echo "  TASKS_AFTER=$(printf '%s' "$TASKS1" | wc -w | tr -d ' ')" | tee -a "$EV/$NAME-tasks.txt"
bcd_readings > "$EV/$NAME-bcd-after.txt"
SAME=$(bcd_same "$EV/$NAME-bcd-before.txt" "$EV/$NAME-bcd-after.txt")
if [ "$CRC" = "0" ] && [ "$TASKS0" = "$TASKS1" ] && [ "$SAME" = "BCD_READINGS_EQUAL=True" ]; then
  verdict THREE_CONFIRMS_NO PASS "driven to the last confirmation and answered No; task list and boot configuration unchanged ($SAME)"
else
  verdict THREE_CONFIRMS_NO FAIL "driver_rc=$CRC tasks_before=$(printf '%s' "$TASKS0" | wc -w | tr -d ' ') tasks_after=$(printf '%s' "$TASKS1" | wc -w | tr -d ' ') $SAME"; fail=1
fi

if [ $fail -eq 0 ]; then verdict GUI_FIX PASS; else verdict GUI_FIX FAIL "(see the lines above)"; exit 1; fi
