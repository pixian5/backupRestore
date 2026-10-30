#!/usr/bin/env bash
# tools/gui-check/run.sh -- GUI 几何 / 文字 / 提示覆盖判据（一条命令，一行结论）
#
# 为什么有这个工具：GUI 占了全项目 22% 的代码（native_gui.rs 1.1 万行）却只有 3 个单测，
# 而它跑在 Windows 上、mac 侧编译为零信息。历史上一批缺陷（字体回退、18 号字裁切、
# 控件重叠、气泡不折行、提示漏挂）全靠临时脚本发现，脚本用完就散。这里把它们固定下来。
#
# 判据（全部来自"独立重建字体/枚举窗口"的外部测量，不读程序自陈）：
#   1) 控件两两重叠数            -> OVERLAPS
#   2) 单行文字被裁切数 / 多行块高度不够数 -> SINGLE_LINE_CLIPPED
#   3) 最低控件是否越出窗口底边  -> MARGIN + FITS/BELOW_WINDOW
#   4) 提示覆盖审计（候选数与未挂数） -> CANDIDATES / UNTIPPED
#
# 用法：
#   tools/gui-check/run.sh              # 全矩阵：5 页签 x 中英 x 14/18 号（约 8 分钟）
#   tools/gui-check/run.sh --quick      # 冒烟：1 个组合（约 25 秒）
#   BR_VM='Windows 11' tools/gui-check/run.sh
#
# 退出码：0=全绿；1=有真判据变红；2=通道抽风（记 SKIP，不当成红）
set -uo pipefail

VM=${BR_VM:-Windows 11}
SHARE='\\Mac\backupRestore'
DST='H:\brwork\gui-check'
QUICK=0
for a in "$@"; do
  case "$a" in
    --quick) QUICK=1 ;;
    -h|--help) sed -n '2,26p' "$0"; exit 0 ;;
  esac
done

say() { printf '%s\n' "$*"; }

# 把判据脚本推进 VM。★ 不依赖 X: 映射——那个盘符每次 VM 重启都会丢（踩过多次）。
sync_ps() {
  local out
  for _ in 1 2 3; do
    out=$(prlctl exec "$VM" cmd /c "robocopy ${SHARE}\\tools\\gui-check\\ps ${DST} /E /NFL /NDL /NJH /NJS /R:1 /W:1 >nul & echo SYNC_OK" 2>&1)
    case "$out" in *SYNC_OK*) return 0 ;; esac
    sleep 2
  done
  return 1
}

# 跑一个判据脚本，**必须看到 __END__** 才算真跑过。
# ★ 这是硬要求：空输出既可能是"没发现问题"也可能是"命令根本没执行"，
#   两者混在一起就等于把通道抽风记成通过。
vmps() {
  local script="$1"; shift
  local out
  for _ in 1 2 3; do
    out=$(prlctl exec "$VM" --current-user powershell -NoProfile -ExecutionPolicy Bypass \
          -File "${DST}\\${script}" "$@" 2>&1)
    case "$out" in *__END__*) printf '%s' "$out"; return 0 ;; esac
    sleep 2
  done
  printf ''; return 1
}

num() { case "$1" in *"$2"*) : ;; esac; printf '%s' "${1#*"$2"}" | grep -oE '^[0-9]+' | head -1; }

say "GUI_CHECK start vm=$VM quick=$QUICK"
if ! sync_ps; then
  say "GUI_CHECK sync=FAIL -> ABORT（脚本没进 VM，任何结论都不可信）"
  exit 2
fi

if [ "$QUICK" = 1 ]; then
  COMBOS=("14 zh")
  TABS=(1101)
else
  COMBOS=("14 zh" "14 en" "18 zh" "18 en")
  TABS=(1101 1102 1103 1104 1105)
fi

clipped=0; overlaps=0; below=0; untipped=0; skipped=0; combos=0
cand_min=999999

for combo in "${COMBOS[@]}"; do
  set -- $combo; FONT=$1; LANG_=$2
  for TAB in "${TABS[@]}"; do
    combos=$((combos+1))
    tag="font=$FONT lang=$LANG_ tab=$TAB"

    if ! vmps set-combo.ps1 -Font "$FONT" -Lang "$LANG_" >/dev/null; then
      say "  SKIP $tag (set-combo 通道没跑)"; skipped=$((skipped+1)); continue
    fi
    if ! vmps launch.ps1 >/dev/null; then
      say "  SKIP $tag (launch 通道没跑)"; skipped=$((skipped+1)); continue
    fi
    sleep 8
    if ! vmps click.ps1 -Class BackupRestoreNativeGui -Id "$TAB" >/dev/null; then
      say "  SKIP $tag (click 通道没跑)"; skipped=$((skipped+1)); continue
    fi
    sleep 2

    o_ov=$(vmps overlap-all.ps1 -Class BackupRestoreNativeGui) || { say "  SKIP $tag (overlap 通道没跑)"; skipped=$((skipped+1)); continue; }
    o_fit=$(vmps dlg-fit.ps1 -FontSize "$FONT" -Class BackupRestoreNativeGui) || { say "  SKIP $tag (fit 通道没跑)"; skipped=$((skipped+1)); continue; }
    o_bot=$(vmps fit-bottom.ps1 -Class BackupRestoreNativeGui) || { say "  SKIP $tag (bottom 通道没跑)"; skipped=$((skipped+1)); continue; }
    o_tip=$(vmps tipaudit.ps1) || o_tip='CANDIDATES=NA UNTIPPED=NA'

    n_ov=$(num "$o_ov" 'OVERLAPS=')
    n_cl=$(num "$o_fit" 'SINGLE_LINE_CLIPPED=')
    m_bot=$(printf '%s' "$o_bot" | grep -oE 'MARGIN=-?[0-9]+' | head -1 | grep -oE '\-?[0-9]+')
    n_cd=$(num "$o_tip" 'CANDIDATES=')
    n_ut=$(num "$o_tip" 'UNTIPPED=')

    : "${n_ov:=0}"; : "${n_cl:=0}"; : "${n_ut:=0}"; : "${m_bot:=0}"
    overlaps=$((overlaps+n_ov)); clipped=$((clipped+n_cl)); untipped=$((untipped+n_ut))
    if [ "$m_bot" -lt 0 ] 2>/dev/null; then below=$((below+1)); fi
    if [ -n "${n_cd:-}" ] && [ "${n_cd:-NA}" != "NA" ] && [ "$n_cd" -gt 0 ] 2>/dev/null && [ "$n_cd" -lt "$cand_min" ]; then
      cand_min=$n_cd
    fi

    verdict=FITS
    if [ "$n_ov" -gt 0 ] || [ "$n_cl" -gt 0 ] || [ "$n_ut" -gt 0 ] || [ "$m_bot" -lt 0 ] 2>/dev/null; then verdict=FAIL; fi
    say "  $verdict $tag overlap=$n_ov clipped=$n_cl margin=$m_bot cand=$n_cd untipped=$n_ut"
  done
done

say "GUI_CHECK combos=$combos clipped=$clipped overlaps=$overlaps below=$below untipped=$untipped skipped=$skipped candidates_min=$cand_min"

if [ "$skipped" -gt 0 ]; then
  say "RESULT=SKIP（有 $skipped 个组合通道没跑，不能当通过）"; exit 2
fi
if [ "$clipped" -gt 0 ] || [ "$overlaps" -gt 0 ] || [ "$below" -gt 0 ] || [ "$untipped" -gt 0 ]; then
  say "RESULT=FAIL"; exit 1
fi
say "RESULT=PASS"; exit 0
