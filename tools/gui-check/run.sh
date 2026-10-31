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
#   5) 错误码在界面上是否真的可见 -> RECORD_CODE / STATUS code / ADMIN_ROWS_WITH_CODE
#      （只需跑一次，与字号无关）
#
# 用法：
#   tools/gui-check/run.sh              # 全矩阵：5 页签 x 中英 x 12/14/16/18 号（约 15 分钟）
#   tools/gui-check/run.sh --quick      # 冒烟：1 个组合 + 4 个辅助窗口 + 错误码（实测约 66 秒）
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
#
# ★★ 2026-10-31 修：这里原来有两处「假绿」，合起来能让整轮矩阵跑在旧脚本上——
#   ① `${SHARE}\\tools\\...` 在双引号里 `\\` 被 bash 折成 `\`，UNC 路径变成
#      `\Mac\backupRestore\...`（驱动器相对路径），robocopy 报错但 `>nul` 吞了；
#   ② 判据是 `& echo SYNC_OK`——**只能证明命令执行过，不能证明复制成功**。
#   实测后果：`runas.ps1` 在 VM 里停了 12 小时没更新，而同步步骤一路绿。
#   现在的做法：路径用单反斜杠拼（拼出来才是真正的 UNC），判据改成
#   robocopy 的 errorlevel（0~7 算成功，≥8 失败）+ **内容比对**（新脚本里必须有
#   只有新版本才有的串）。复制没成功就让整轮 ABORT，绝不拿旧脚本出结论。
sync_ps() {
  local out out2 src='\\Mac\backupRestore\tools\gui-check\ps' want vm_size
  want=$(wc -c < "$(dirname "$0")/ps/runas.ps1" | tr -d ' ')
  for _ in 1 2 3; do
    out=$(prlctl exec "$VM" cmd /c "robocopy ${src} ${DST} /E /NFL /NDL /NJH /NJS /R:1 /W:1 >nul & if errorlevel 8 (echo SYNC_FAIL) else (echo SYNC_OK)" 2>&1)
    case "$out" in
      *SYNC_OK*)
        # ★ 内容判据：宿主与 VM 里 runas.ps1 的**字节数必须一致**。
        #   不用固定字符串（改个名字就误报），也不用时间戳（SMB 缓存会让它骗人）。
        vm_size=$(prlctl exec "$VM" powershell -NoProfile -Command \
                    "(Get-Item -LiteralPath '${DST}\runas.ps1' -ErrorAction SilentlyContinue).Length" 2>/dev/null \
                  | tr -d '\r\n ')
        case "$vm_size" in
          ''|*[!0-9]*) ;;
          *) [ "$vm_size" = "$want" ] && return 0 ;;
        esac
        ;;
    esac
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

# 需要**投递输入**的脚本必须提权跑。
#
# ★ UIPI：程序是提权运行的，未提权进程向更高完整性级别的窗口投递鼠标输入会被
#   **静默丢弃** —— SetCursorPos/mouse_event 全部正常返回，坐标也算对了，但页签
#   一个都没切。2026-10-30 的实测证据：非提权点击后「可见控件 id 集合」DIFF=0；
#   改提权后点 1101 → 集合从 17 个变 11 个（DIFF=14）、点 1106 → 14 个（DIFF=11）。
#   所以：只读取窗口信息的脚本可以不提权，凡是要点的必须提权。
vmps_elev() {
  local script="$1"; shift
  local extra="$*"
  local out
  for _ in 1 2 3; do
    out=$(prlctl exec "$VM" --current-user powershell -NoProfile -ExecutionPolicy Bypass \
          -File "${DST}\\runas.ps1" -Name "$script" -Extra "$extra" 2>&1)
    case "$out" in *__END__*) printf '%s' "$out"; return 0 ;; esac
    sleep 2
  done
  printf ''; return 1
}

# 一次 prlctl 调用里顺序跑多个探测（每个子进程 -NoNewWindow 继承控制台）。
#
# ★★ 为什么：每一次 `prlctl exec --current-user powershell` 都漏一对
#   conhost/OpenConsole（实测 2026-11-01：+1/次，提权与否一样——漏的是**客户端进程**）。
#   一个主窗口组合原来要 7 次调用、一个辅助窗口 5 次。合并成批以后次数直接除以 4，
#   泄漏也跟着少 4 倍（收尾还有 SYSTEM 通道清扫兜底）。
#
# 参数：一组 "脚本名|参数串"，用 ~~ 连接。任一条的输出没给 __END__ 都算通道没跑通。
vmps_batch() {
  local args=() i=0 spec out
  # ★ 每个任务一个独立参数（-J1..-J5）。不要用分隔符塞进一个参数：实测 prlctl exec
  #   会把 "@@" 整个吃掉、把 "~~" 变成空格，三个任务会静默并成一个（2026-11-01 踩过）。
  for spec in "$@"; do
    i=$((i+1))
    args+=("-J$i" "$spec")
  done
  for _ in 1 2 3; do
    out=$(prlctl exec "$VM" --current-user powershell -NoProfile -ExecutionPolicy Bypass \
          -File "${DST}\\batch.ps1" "${args[@]}" 2>&1)
    case "$out" in *BATCH=*) printf '%s' "$out"; return 0 ;; esac
    sleep 2
  done
  printf ''; return 1
}

num() { case "$1" in *"$2"*) : ;; esac; printf '%s' "${1#*"$2"}" | grep -oE '^[0-9]+' | head -1; }

# ---- 判据 6：四个辅助窗口 ----
# 为什么单列：主窗口矩阵只量「主窗口 × 5 页签」，而设置 / 备份文件管理 / 查看日志 /
# 任务与证据这四个窗口**从来没进过矩阵**（2.6.91 只在 100% 缩放下手量过一次默认字号）。
#
# ★ 模态陷阱：这四窗口的处理函数都是「先 EnableWindow(main,0)，再自己泵消息循环」。
#   所以 ① 开下一个之前必须先关掉上一个（主窗口禁用期间任何点击都不响应）；
#        ② 关闭走 WM_CLOSE，跨进程投递同样受 UIPI 约束，非提权会静默失效。
# ★ 只跑 12/18 两个极端档：控件宽度 = f(实测文本宽度) 随字号单调增，最小档装得下、
#   最大档也装得下，中间档不会更差（这条对「固定宽度」不成立，所以固定宽度的检查
#   仍由主窗口 4 档全矩阵负责）。
AUX_LIST=(
  "1007 BackupRestoreSettingsWindow 10"
  "1009 BackupRestoreImageAdminWindow 10"
  "1010 BackupRestoreLogsWindow 6"
  "1004 BackupRestoreTaskAdmin 6"
)

aux_pass() {
  local font="$1" lang="$2"
  # 全矩阵里只跑 12/18 两个极端档（理由见上）。--quick 只有 14 zh 一个组合，
  # 就让它也跑一遍——冒烟的意义是把整条链路走通，不是省那两分钟。
  case "$font" in
    12|18) ;;
    *) [ "$QUICK" = 1 ] || return 0 ;;
  esac
  local spec id cls minc tag o_open o_fit o_ov o_bot n_cl n_ov m_bot
  for spec in "${AUX_LIST[@]}"; do
    set -- $spec; id=$1; cls=$2; minc=$3
    tag="aux font=$font lang=$lang $cls"
    o_open=$(vmps_elev aux-open.ps1 -Id "$id" -Class "$cls") || { say "  SKIP $tag (open 通道没跑)"; aux_skipped=$((aux_skipped+1)); return 0; }
    case "$o_open" in
      *"ELEVATED=False"*)
        say "  SKIP $tag (判据没提权 -> UIPI 会吞掉点击，不是产品问题)"; aux_skipped=$((aux_skipped+1)); return 0 ;;
      *"OPENED=1"*) ;;
      *) say "  SKIP $tag (窗口没开: $(printf '%s' "$o_open" | grep -oE 'reason=[a-z_]+' | head -1))"
         vmps_elev aux-close.ps1 -Class "$cls" >/dev/null 2>&1 || true
         aux_skipped=$((aux_skipped+1)); return 0 ;;
    esac

    # 三个测量合并成一次调用（原来 3 次 = 3 个泄漏点）
    o_all=$(vmps_batch "dlg-fit.ps1|-FontSize $font -Class $cls" \
                       "overlap-all.ps1|-Class $cls -MinControls $minc" \
                       "fit-bottom.ps1|-Class $cls") || o_all=''
    o_fit=$o_all; o_ov=$o_all; o_bot=$o_all
    # 无论量成没量成，都要把窗口关掉，否则后面全部卡住
    vmps_elev aux-close.ps1 -Class "$cls" >/dev/null 2>&1 || true

    case "$o_fit" in *"SINGLE_LINE_CLIPPED="*) ;; *) say "  SKIP $tag (fit 没给出数字)"; aux_skipped=$((aux_skipped+1)); continue ;; esac
    case "$o_ov"  in *"OVERLAPS="*) ;;           *) say "  SKIP $tag (overlap 没给出数字)"; aux_skipped=$((aux_skipped+1)); continue ;; esac
    case "$o_bot" in *"MARGIN="*) ;;             *) say "  SKIP $tag (bottom 没给出数字)"; aux_skipped=$((aux_skipped+1)); continue ;; esac

    n_cl=$(num "$o_fit" 'SINGLE_LINE_CLIPPED=')
    n_ov=$(num "$o_ov" 'OVERLAPS=')
    m_bot=$(printf '%s' "$o_bot" | grep -oE 'MARGIN=-?[0-9]+' | head -1 | grep -oE '\-?[0-9]+')
    : "${n_cl:=0}"; : "${n_ov:=0}"; : "${m_bot:=0}"
    aux=$((aux+1))
    aux_clipped=$((aux_clipped+n_cl)); aux_overlaps=$((aux_overlaps+n_ov))
    if [ "$m_bot" -lt 0 ] 2>/dev/null; then aux_below=$((aux_below+1)); fi
    verdict=FITS
    if [ "$n_cl" -gt 0 ] || [ "$n_ov" -gt 0 ] || [ "$m_bot" -lt 0 ] 2>/dev/null; then verdict=FAIL; fi
    say "  $verdict $tag clipped=$n_cl overlap=$n_ov margin=$m_bot"
  done
}

say "GUI_CHECK start vm=$VM quick=$QUICK"
if ! sync_ps; then
  say "GUI_CHECK sync=FAIL -> ABORT（脚本没进 VM，任何结论都不可信）"
  exit 2
fi

# ★ 字号档位取自产品本身：dpi.rs 的 FONT_SIZE_CHOICES = [12,14,16,18]。
#   2.7.2 之前这里只跑 14/18 两档——**漏了 12 和 16**，是我自己工具的覆盖缺口。
#   判据的档位不能靠人记，应当跟着产品的可选档位走。
if [ "$QUICK" = 1 ]; then
  COMBOS=("14 zh")
  TABS=(1101)
else
  COMBOS=("12 zh" "12 en" "14 zh" "14 en" "16 zh" "16 en" "18 zh" "18 en")
  # ★ 页签 id 取自 native_gui.rs 的 ID_OPERATION_*：
  #   1101 备份 / 1102 还原系统到 C 盘 / 1104 安装 PE / 1105 还原数据到非 C 盘 /
  #   1106 还原系统到非 C 盘（第二系统）。
  #   2026-10-30 修正：原写 (1101 1102 1103 1104 1105) —— **1103 根本不存在**（历史遗留，
  #   探测入口 1107 已移除），而**真正的第二系统页签 1106 从来没被测过**。
  #   手写 id 集合就是这样错的：应该能从源码核对的，就别凭印象写。
  TABS=(1101 1102 1104 1105 1106)
fi

clipped=0; overlaps=0; below=0; untipped=0; skipped=0; combos=0
aux=0; aux_clipped=0; aux_overlaps=0; aux_below=0; aux_skipped=0
cand_min=999999

for combo in "${COMBOS[@]}"; do
  set -- $combo; FONT=$1; LANG_=$2
  for TAB in "${TABS[@]}"; do
    combos=$((combos+1))
    tag="font=$FONT lang=$LANG_ tab=$TAB"

    # ★★ 每一步都要**校验结果值**，不能只认 __END__。
    # 2026-10-30 踩的坑：click.ps1 因为类名 `GC` 撞上 .NET 的 System.GC 而全程失败，
    # 输出 `CLICK=NO_WINDOW ... __END__` —— 含 __END__，旧逻辑判定"跑过了"，
    # 于是 40 个组合**其实全在测同一个页签**（证据：margin 只随字号变、不随页签变，
    # 这本该是红旗）。所以：__END__ 只证明"命令被执行"，结果值才证明"事情做成了"。
    o_set=$(vmps set-combo.ps1 -Font "$FONT" -Lang "$LANG_") || { say "  SKIP $tag (set-combo 通道没跑)"; skipped=$((skipped+1)); continue; }
    case "$o_set" in
      *"COMBO font="*) ;;
      *) say "  SKIP $tag (set-combo 没写成: $(printf '%s' "$o_set" | head -1))"; skipped=$((skipped+1)); continue ;;
    esac

    o_launch=$(vmps launch.ps1) || { say "  SKIP $tag (launch 通道没跑)"; skipped=$((skipped+1)); continue; }
    # PID 后面必须是数字：PID=NONE / PID=MISSING 都算没起来
    pid=$(printf '%s' "$o_launch" | grep -oE 'PID=[0-9]+' | head -1)
    if [ -z "$pid" ]; then
      say "  SKIP $tag (程序没起来: $(printf '%s' "$o_launch" | grep -oE 'PID=[A-Za-z]*' | head -1))"
      skipped=$((skipped+1)); continue
    fi
    sleep 8

    o_click=$(vmps_elev click.ps1 -Id "$TAB") || { say "  SKIP $tag (click 通道没跑)"; skipped=$((skipped+1)); continue; }
    case "$o_click" in
      *"CLICK=OK"*) ;;
      *) say "  SKIP $tag (页签没切: $(printf '%s' "$o_click" | head -1))"; skipped=$((skipped+1)); continue ;;
    esac
    sleep 2

    # 四个测量合并成一次调用（原来 4 次 = 4 个泄漏点）。各自输出不变，
    # 下面照旧按各自的键解析；缺键仍然 SKIP，不会落到默认 0 假装通过。
    o_meas=$(vmps_batch "overlap-all.ps1|-Class BackupRestoreNativeGui" \
                        "dlg-fit.ps1|-FontSize $FONT -Class BackupRestoreNativeGui" \
                        "fit-bottom.ps1|-Class BackupRestoreNativeGui" \
                        "tipaudit.ps1|") \
      || { say "  SKIP $tag (measure 批次通道没跑)"; skipped=$((skipped+1)); continue; }
    o_ov=$o_meas; o_fit=$o_meas; o_bot=$o_meas; o_tip=$o_meas

    # 判据输出必须**真的带那个数字**。缺了就 SKIP —— 绝不能落到默认值 0 去"通过"：
    # 空值默认 0 会把"没量到"伪装成"量到 0 个问题"，那是最坏的假绿。
    case "$o_ov" in *"OVERLAPS="*) ;; *) say "  SKIP $tag (overlap 没给出数字)"; skipped=$((skipped+1)); continue ;; esac
    case "$o_fit" in *"SINGLE_LINE_CLIPPED="*) ;; *) say "  SKIP $tag (fit 没给出数字)"; skipped=$((skipped+1)); continue ;; esac
    case "$o_bot" in *"MARGIN="*) ;; *) say "  SKIP $tag (bottom 没给出数字)"; skipped=$((skipped+1)); continue ;; esac

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

  # 五个页签都量完之后，程序仍以这个字号/语言在跑 —— 顺手把四个辅助窗口也量了，
  # 不用再重启一次。
  aux_pass "$FONT" "$LANG_"
done

# ---- 判据 5：错误码在界面上到底看不看得见（G4）----
# 为什么单列一条：用户手册让用户"看错误码"，但从来没有人量过它是否真的显示出来。
# 这条判据不需要遍字号矩阵（与字号无关），跑一次即可。
errcode_na=0; errcode_missing=0
o_ec=$(vmps_elev errcode.ps1 -RefreshId 1004) || o_ec=''
if [ -z "$o_ec" ]; then
  say "  SKIP 错误码判据（通道没跑）"
  skipped=$((skipped+1))
else
  v_ec=$(printf '%s' "$o_ec" | grep -oE 'VERDICT=[A-Z_]+' | head -1)
  c_rec=$(printf '%s' "$o_ec" | grep -oE 'RECORD_CODE=[A-Za-z0-9-]+' | head -1 | cut -d= -f2)
  c_st=$(printf '%s' "$o_ec" | grep -oE 'STATUS_len=[0-9]+ code=[A-Za-z0-9-]+' | head -1)
  c_rw=$(printf '%s' "$o_ec" | grep -oE 'ADMIN_ROWS_WITH_CODE=[0-9]+ of [0-9]+' | head -1)
  say "  错误码判据 verdict=$v_ec record=$c_rec | $c_st | $c_rw"
  case "$v_ec" in
    VERDICT=NA) errcode_na=1 ;;
    VERDICT=VISIBLE_IN_BOTH|VERDICT=VISIBLE_IN_STATUS_ONLY|VERDICT=VISIBLE_IN_TASK_WINDOW_ONLY) ;;
    *)
      # 记录里根本没有错误码（没有失败任务）不算缺陷；有码却看不见才算。
      if [ "${c_rec:-none}" = "none" ]; then errcode_na=1; else errcode_missing=1; fi
      ;;
  esac
fi

# ★★ 收尾清扫（2026-11-01）：每一次 `prlctl exec --current-user powershell` 都会在用户
#   会话里新起一个控制台进程，Windows Terminal 的交接大概率失败 → 留下一个孤儿
#   conhost/OpenConsole 窗口。实测：5 次调用 conhost 1→6；提权与否都一样，助手也挡不住
#   （泄漏的是**客户端进程本身**）。而 **SYSTEM 通道（不带 --current-user）实测零泄漏**，
#   所以这里用它扫一次，把本轮攒下的孤儿窗口清掉，桌面不会留一屏空窗口。
#   注意：这只是收尾，不是根治——真正的根治要么让 prlctl 不开控制台，要么把探测批次
#   合并进更少的调用里（BACKLOG B-2694）。
SWEEP="$(dirname "$0")/../vm/postflight.sh"
if [ -x "$SWEEP" ]; then
  sweep_out=$("$SWEEP" clean 2>&1 | grep -oE 'ACTION=kill_orphan_console targets=[0-9]+' | tail -1)
  say "GUI_CHECK sweep=${sweep_out:-none}"
fi

say "GUI_CHECK combos=$combos clipped=$clipped overlaps=$overlaps below=$below untipped=$untipped skipped=$skipped candidates_min=$cand_min errcode_na=$errcode_na errcode_missing=$errcode_missing"
say "GUI_CHECK_AUX checked=$aux clipped=$aux_clipped overlaps=$aux_overlaps below=$aux_below skipped=$aux_skipped"

if [ "$skipped" -gt 0 ] || [ "$aux_skipped" -gt 0 ]; then
  say "RESULT=SKIP（有组合/辅助窗口通道没跑，不能当通过）"; exit 2
fi
if [ "$clipped" -gt 0 ] || [ "$overlaps" -gt 0 ] || [ "$below" -gt 0 ] || [ "$untipped" -gt 0 ] || [ "$errcode_missing" -gt 0 ] \
   || [ "$aux_clipped" -gt 0 ] || [ "$aux_overlaps" -gt 0 ] || [ "$aux_below" -gt 0 ]; then
  say "RESULT=FAIL"; exit 1
fi
say "RESULT=PASS"; exit 0
