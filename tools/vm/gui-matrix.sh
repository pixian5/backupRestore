#!/bin/bash
# tools/vm/gui-matrix.sh —— 界面全矩阵：给 tools/gui-check/run.sh 套一层「虚拟机被暂停就恢复」，
# 并在结束后把界面设置恢复成「14 号字、中文」（全矩阵会让它停在 18 号字、英文）。
#
# 用法：
#   tools/vm/gui-matrix.sh              # 全矩阵：5 页签 × 中英 × 12/14/16/18 号（约 15 分钟）
#   tools/vm/gui-matrix.sh --quick      # 冒烟：1 个组合 + 4 个辅助窗口 + 错误码（约 1 分钟）
# 参数原样传给 run.sh。退出码：0 = RESULT=PASS 且设置已恢复；1 = 有判据变红；2 = 通道抽风（run.sh 的 SKIP）；其它 = 步骤中止。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
run_name "gui-matrix"
LOG="$EV/$NAME.log"
rm -f "$LOG"

say "STEP=0 nobody is using the desktop"
require_idle "$NAME-idle.txt" 120

"$REPO/tools/gui-check/run.sh" "$@" > "$LOG" 2>&1 &
runner=$!
say "GUI_MATRIX_STARTED pid=$runner args=$*"
while kill -0 "$runner" 2>/dev/null; do keep_alive; sleep 15; done
wait "$runner"; rc=$?
say "GUI_MATRIX_EXIT=$rc"
grep -a -E '^GUI_CHECK|^RESULT=|^  FAIL|SKIP' "$LOG" | cut -c1-200 | tail -40

say "restore the UI settings to 14 pt / Chinese"
out=""
for i in 1 2 3; do
  out=$("$PRL" exec "$VM" powershell -NoProfile -ExecutionPolicy Bypass -File "$SHARE_PS\\set-combo.ps1" -Font 14 -Lang zh 2>/dev/null | tr -d '\r')
  printf '%s' "$out" | grep -a -q '^COMBO font=14 lang=zh' && break
  sleep 2
done
printf '%s' "$out" | grep -a '^COMBO' | sed 's/^/  SETTINGS /'
vrun "type H:\\brwork\\BackupRestore.settings.ini" 3 | grep -a -E '^(font_size|ui_language)=' | sed 's/^/  INI /'

if grep -a -q '^RESULT=PASS' "$LOG" && printf '%s' "$out" | grep -a -q '^COMBO font=14 lang=zh'; then
  verdict GUI_MATRIX PASS "RESULT=PASS and the settings are back to 14 pt / Chinese"
else
  verdict GUI_MATRIX FAIL "run.sh rc=$rc; see $LOG"; exit 1
fi
