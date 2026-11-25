#!/bin/bash
# tools/vm/selftest.sh —— tools/vm 的离线自检：不需要虚拟机，几秒钟跑完。
#
# 检查三样东西：
#   1. 公共读数函数在固定输入上的行为（本地编码解码、取控件文字、取 JSON 字段、读数比对）；
#   2. 命令行冒烟的判读程序 judge/cli-probe.py：拿真实抓到的输出样本（fixtures/）必须判 PASS；
#      对样本做变异（删掉「驱动器类型」字段）必须判 FAIL——不变红的判读等于没有判读；
#   3. 其余判读程序（恢复环境日志、界面日志、窗口清单、镜像信息、下拉框选中项、构建日志）：
#      用人工拼的输入或真实样本验证读数，并对每一个做变异——变异之后必须变红。
# 改了 tools/vm/lib/common.sh、tools/vm/judge/ 或样本之后跑一遍。
# 用法：tools/vm/selftest.sh          退出码：0 = 全过；1 = 有检查不通过。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
TMP=$(mktemp -d "${TMPDIR:-/tmp}/br-vm-tools-selftest.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
failed=0
check() { # $1 名称  $2 期望  $3 实际
  if [ "$2" = "$3" ]; then echo "SELFTEST $1 PASS"; else echo "SELFTEST $1 FAIL expected=[$2] got=[$3]"; failed=$((failed+1)); fi
}
check_in() { # $1 名称  $2 必须包含的串  $3 实际
  case "$3" in *"$2"*) echo "SELFTEST $1 PASS" ;; *) echo "SELFTEST $1 FAIL missing=[$2] got=[$3]"; failed=$((failed+1)) ;; esac
}

# ---- 1. 公共读数函数 -------------------------------------------------------------------
python3 -I -c "
import sys
sys.stdout.buffer.write(b'WINDOWS=1 class=X\r\n')
sys.stdout.buffer.write(b'  id=2003 class=Button text=' + '取消'.encode('gbk') + b'\r\n')
sys.stdout.buffer.write(b'  id=2000 class=Static text=' + '已选择离线通道'.encode('gbk') + b'\r\n')
sys.stdout.buffer.write(b'__END__\r\n')
" > "$TMP/gbk.txt"
decoded=$(decode_text < "$TMP/gbk.txt")
check_in "decode_text brings GBK control text back" "id=2003 class=Button text=取消" "$decoded"
check_in "decode_text keeps ASCII key lines" "WINDOWS=1 class=X" "$decoded"
check "control_text reads the Cancel button" "取消" "$(control_text "$decoded" 2003)"
check "control_text reads the body" "已选择离线通道" "$(control_text "$decoded" 2000)"
check "control_text of a missing control is empty" "" "$(control_text "$decoded" 9999)"
printf '%s\n' '{"success":true,"stage":"success"}' '中文正文（通用编码）' | decode_text > "$TMP/utf8.txt"
check "decode_text passes UTF-8 through" "$(printf '%s\n' '{"success":true,"stage":"success"}' '中文正文（通用编码）')" "$(cat "$TMP/utf8.txt")"
printf '\xff\xfe\x01\x02 not decodable\n' | decode_text > "$TMP/bad.txt"
check_in "decode_text flags undecodable lines instead of replacing silently" "<DECODE_FAILED>" "$(cat "$TMP/bad.txt")"

printf '{"command":"abandon-task","error":"x","error_code":"BR-TSK-036","success":false,"task_id":null}\nVM_DONE=1\n' > "$TMP/a.json"
check "json_field reads a value" "BR-TSK-036" "$(json_field "$TMP/a.json" error_code)"
check "json_field turns null into an empty string" "" "$(json_field "$TMP/a.json" task_id)"
check "json_field turns a missing field into an empty string" "" "$(json_field "$TMP/a.json" nope)"
check "json_field turns an unreadable file into an empty string" "" "$(json_field "$TMP/does-not-exist.json" x 2>/dev/null)"
check "json_field accepts Python-style booleans as text" "False" "$(json_field "$TMP/a.json" success)"

printf 'BCD_MD5=aa\nDEVICE_LINES=27\nBOOTSEQUENCE_LINES=0\n' > "$EV/selftest-bcd-1.txt"
printf 'BCD_MD5=aa\nDEVICE_LINES=27\nBOOTSEQUENCE_LINES=0\n' > "$EV/selftest-bcd-2.txt"
printf 'BCD_MD5=aa\nDEVICE_LINES=28\nBOOTSEQUENCE_LINES=0\n' > "$EV/selftest-bcd-3.txt"
check "bcd_same: identical readings" "BCD_READINGS_EQUAL=True" "$(bcd_same "$EV/selftest-bcd-1.txt" "$EV/selftest-bcd-2.txt")"
check "bcd_same: one differing line" "BCD_READINGS_EQUAL=False" "$(bcd_same "$EV/selftest-bcd-1.txt" "$EV/selftest-bcd-3.txt")"
rm -f "$EV"/selftest-bcd-*.txt

printf 'CHECK tag=pr serial=A412-9DAC\r\nCHECK sentinel_exists=True\r\nCHECK file1=BRDATA file1 v1\r\nCHECK payload_len=104857600 payload_sha256=C057B27E1C65\r\n' > "$TMP/fixture.txt"
check "fixture_field reads the serial from the tagged first line" "A412-9DAC" "$(fixture_field "$TMP/fixture.txt" serial)"
check "fixture_field reads a plain field" "True" "$(fixture_field "$TMP/fixture.txt" sentinel_exists)"
check "fixture_field keeps spaces inside a value" "BRDATA file1 v1" "$(fixture_field "$TMP/fixture.txt" file1)"
check "fixture_field reads a field that follows another field on the same line" "C057B27E1C65" "$(fixture_field "$TMP/fixture.txt" payload_sha256)"
check "fixture_field reads the first field of that line" "104857600" "$(fixture_field "$TMP/fixture.txt" payload_len)"
check "fixture_field of a missing field is empty" "" "$(fixture_field "$TMP/fixture.txt" nope)"
: > "$TMP/empty-fixture.txt"
check "fixture_field of an empty file is empty (the caller must not compare empty with empty)" "" "$(fixture_field "$TMP/empty-fixture.txt" serial)"

printf '\xef\xbb\xbf{"stage":"success","progress":100,"error":null}\n' > "$TMP/status.json"
check "status_field reads through a byte-order mark" "success" "$(status_field "$TMP/status.json" stage)"
check "status_field reads a number" "100" "$(status_field "$TMP/status.json" progress)"
check "status_field of a null field" "<missing>" "$(status_field "$TMP/status.json" error)"
check "status_field of an unreadable file" "<unreadable>" "$(status_field "$TMP/no-such-status.json" stage)"

# judge/restore-log.py：人工拼一份「跨任务累积」的恢复环境日志。两个任务，第一个带一次跨过午夜的暂停。
cat > "$TMP/rl.log" <<'LOG'
[2026-11-24T23:59:58.100000000+00:00] Pause support: operation=restore-existing supported=true
[2026-11-24T23:59:59.000000000+00:00] Recovery.exe started from env task=aaaaaaaa-0000-0000-0000-000000000001 operation=RestoreExisting
[2026-11-24T23:59:59.100000000+00:00] STEP 1/4 挂载卷、校验
[2026-11-24T23:59:59.200000000+00:00] STEP 2/4 还原镜像
[2026-11-25T00:00:02.000000000+00:00] 恢复已暂停（用户请求），停在：STEP 2/4 还原镜像
[2026-11-25T00:01:32.000000000+00:00] 用户按 P 继续：恢复将立即推进
[2026-11-25T00:01:33.000000000+00:00] 恢复已继续（用户请求）
[2026-11-25T00:01:33.100000000+00:00] STEP 3/4 校验
[2026-11-25T00:01:33.200000000+00:00] STEP 4/4 清理恢复启动项/配置
[2026-11-25T00:01:33.300000000+00:00] RESTORE_SELFCHECK target=E serial_before=A4129DAC serial_after=A4129DAC ntoskrnl=false
[2026-11-25T00:02:00.000000000+00:00] Recovery.exe started from env task=aaaaaaaa-0000-0000-0000-000000000002 operation=Backup
[2026-11-25T00:02:01.000000000+00:00] STEP 1/4 挂载卷、校验
LOG
printf 'Pause support: operation=restore-existing supported=true\n__END__\n' > "$TMP/early.log"
out=$(python3 -I "$HERE/judge/restore-log.py" "$TMP/rl.log" aaaaaaaa-0000-0000-0000-000000000001 --pause --early-log "$TMP/early.log")
check_in "restore-log judge: four step labels although the paused step is logged once" "RECOVERY_STEP_LABELS=1,2,3,4" "$out"
check_in "restore-log judge: the segment stops at the next task (its self-check line is found)" "SELFCHECK_LINE=RESTORE_SELFCHECK target=E serial_before=A4129DAC serial_after=A4129DAC ntoskrnl=false" "$out"
check_in "restore-log judge: seconds paused across midnight" "SECONDS_PAUSED=91" "$out"
check_in "restore-log judge: the declaration written just before the start line" "DECLARATION_IN_FORMAL_LOG=Pause support: operation=restore-existing supported=true" "$out"
check_in "restore-log judge: declaration lines in the task early log" "DECLARATION_IN_TASK_EARLY_LOG=1" "$out"
check_in "restore-log judge: paused and resumed lines are counted" "PAUSED_LINES=1" "$out"
check_in "restore-log judge: resumed line" "RESUMED_LINES=1" "$out"
out=$(python3 -I "$HERE/judge/restore-log.py" "$TMP/rl.log" aaaaaaaa-0000-0000-0000-000000000002); rc=$?
check "restore-log judge: the second task is found" "0" "$rc"
check_in "restore-log judge: the second task's labels do not leak from the first" "RECOVERY_STEP_LABELS=1" "$out"
python3 -I "$HERE/judge/restore-log.py" "$TMP/rl.log" aaaaaaaa-0000-0000-0000-000000000009 >/dev/null 2>&1; check "restore-log judge: an unknown task exits non-zero" "1" "$?"
# 变异：把「恢复已继续」那一行拿掉（模拟按了继续却没恢复）→ 继续行数必须变 0、停顿时长必须读成未知，不能沿用旧值。
grep -a -v '恢复已继续' "$TMP/rl.log" > "$TMP/rl-mutated.log"
out=$(python3 -I "$HERE/judge/restore-log.py" "$TMP/rl-mutated.log" aaaaaaaa-0000-0000-0000-000000000001 --pause --early-log "$TMP/early.log")
check_in "restore-log judge mutation: resume line removed -> no resumed line" "RESUMED_LINES=0" "$out"
check_in "restore-log judge mutation: resume line removed -> seconds paused unknown" "SECONDS_PAUSED=<unknown>" "$out"

# judge/gui-dispatch.py：人工拼一份「跨天累积」的界面日志（带一行没有时间戳的、一行只有一位小数秒的）。
cat > "$TMP/gui.log" <<'LOG'
[2026-11-23T20:59:38.430604700+00:00] offline data restore: Windows RE confirm result=6 (6=yes)
[2026-11-23T20:59:38.431049400+00:00] prepare args: --skip-format (overlay restore)
[2026-10-30T07:16:48.999457800+00:00] GUI action blocked: PE WIM file does not exist
[2026-11-25T12:33:46.000000000+00:00] GUI started; elevated native Rust window initialized
tooltips installed: added=37 missing=0
[2026-11-25T12:33:48.150828100+00:00] offline data restore: Windows RE confirm result=6 (6=yes)
[2026-11-25T12:33:48.152660600+00:00] prepare args: --skip-format (overlay restore)
[2026-11-25T12:33:49.5] task directory created
LOG
out=$(python3 -I "$HERE/judge/gui-dispatch.py" "$TMP/gui.log" 2026-11-25T12:33:46)
check_in "gui-dispatch judge: lines after the boundary (a line without a timestamp is not counted)" "GUI_LOG_LINES_AFTER=4" "$out"
check_in "gui-dispatch judge: only this run's records are counted" "GUI_LOG_SKIP_FORMAT_ARGS=1" "$out"
check_in "gui-dispatch judge: a two-day-old block is not this run's" "GUI_LOG_BLOCKED=0" "$out"
out=$(python3 -I "$HERE/judge/gui-dispatch.py" "$TMP/gui.log" 2026-10-01T00:00:00)
check_in "gui-dispatch judge: an early boundary sees the history (the reading channel is proven to see it)" "GUI_LOG_BLOCKED=1" "$out"
check_in "gui-dispatch judge: history count of confirmations" "GUI_LOG_CONFIRM_YES=2" "$out"
python3 -I "$HERE/judge/gui-dispatch.py" "$TMP/gui.log" 2027-01-01T00:00:00 >/dev/null 2>&1
check "gui-dispatch judge: a boundary in the future reads no line and exits non-zero" "1" "$?"
# 变异：界线之后出现一行「已拦截」→ 必须数到 1（模拟本次真的被拦了）。
printf '[2026-11-25T12:33:50.000000000+00:00] GUI action blocked: something\n' >> "$TMP/gui.log"
out=$(python3 -I "$HERE/judge/gui-dispatch.py" "$TMP/gui.log" 2026-11-25T12:33:46)
check_in "gui-dispatch judge mutation: a block after the boundary is counted" "GUI_LOG_BLOCKED=1" "$out"

# judge/gui-dispatch.py：备份入口的几类行。人工拼一段「系统盘备份走恢复环境」的界面日志，前面另带两行五天前的在线备份记录。
cat > "$TMP/guib.log" <<'LOG'
[2026-11-20T10:00:00.000000000+00:00] online started: op=backup source=F target=C
[2026-11-20T10:00:01.000000000+00:00] online completed: success=true
[2026-11-25T13:00:00.000000000+00:00] GUI started; elevated native Rust window initialized
[2026-11-25T13:00:05.000000000+00:00] GUI action started: create task; operation=backup
[2026-11-25T13:00:08.000000000+00:00] system drive choice returned: 2 (0=cancel 1=PE 2=RE)
[2026-11-25T13:00:12.000000000+00:00] system drive operation: Windows RE confirm result=6 (6=yes)
[2026-11-25T13:00:12.500000000+00:00] elevated prepare: launching (visible) operation=backup params=[112]
[2026-11-25T13:00:13.000000000+00:00] GUI action delegated: elevated prepare launched; operation=backup; source=C; target=C
LOG
line() { printf '%s\n' "$1" | sed -n "s/^$2=\(.*\)$/\1/p" | tail -1; }
out=$(python3 -I "$HERE/judge/gui-dispatch.py" "$TMP/guib.log" 2026-11-25T13:00:00)
check "gui-dispatch judge: system backup chain - Windows RE was chosen" "1" "$(line "$out" GUI_LOG_SYSTEM_CHOICE_RE)"
check "gui-dispatch judge: system backup chain - the final confirmation was answered yes" "1" "$(line "$out" GUI_LOG_SYSTEM_CONFIRM_YES)"
check "gui-dispatch judge: system backup chain - prepare launched and delegated" "1" "$(line "$out" GUI_LOG_DELEGATED)"
check "gui-dispatch judge: system backup chain - nothing was cancelled" "0" "$(line "$out" GUI_LOG_CANCELLED)"
check "gui-dispatch judge: system backup chain - it did not go online" "0" "$(line "$out" GUI_LOG_ONLINE_SPLIT)"
check "gui-dispatch judge: an online start five days old is not this run's" "0" "$(line "$out" GUI_LOG_ONLINE_STARTED)"
out=$(python3 -I "$HERE/judge/gui-dispatch.py" "$TMP/guib.log" 2026-11-20T09:00:00)
check "gui-dispatch judge: an early boundary sees the old online start (the reading channel is proven to see it)" "1" "$(line "$out" GUI_LOG_ONLINE_STARTED)"
# 变异：界线之后补三行——「已取消」、分流到在线、「在线已开始」——模拟在确认框上点了「否」之后在线备份照样开始。
printf '[2026-11-25T13:00:20.000000000+00:00] GUI action cancelled: destructive confirmation; operation=backup\n[2026-11-25T13:00:21.000000000+00:00] online operation: affected=F system=C\n[2026-11-25T13:00:21.500000000+00:00] online started: op=backup source=F target=C\n' >> "$TMP/guib.log"
out=$(python3 -I "$HERE/judge/gui-dispatch.py" "$TMP/guib.log" 2026-11-25T13:00:00)
check "gui-dispatch judge mutation: an online start after the boundary is counted" "1" "$(line "$out" GUI_LOG_ONLINE_STARTED)"
check "gui-dispatch judge mutation: the split to the online branch is counted" "1" "$(line "$out" GUI_LOG_ONLINE_SPLIT)"
check "gui-dispatch judge mutation: the cancelled line is counted" "1" "$(line "$out" GUI_LOG_CANCELLED)"
# 「还原数据到非 C 盘」在线、不格式化（gui-data-restore.sh online）用到的四个键：备份开始与在线还原开始必须分得清。
printf '[2026-11-25T13:00:30.000000000+00:00] GUI action started: create task; operation=restore-data\n[2026-11-25T13:00:31.000000000+00:00] channel split: op=restore-data online=true\n[2026-11-25T13:00:31.500000000+00:00] online params: format_target=false\n[2026-11-25T13:00:32.000000000+00:00] online started: op=restore-existing source=F target=F\n[2026-11-25T13:00:40.000000000+00:00] online completed: success=true\n' >> "$TMP/guib.log"
out=$(python3 -I "$HERE/judge/gui-dispatch.py" "$TMP/guib.log" 2026-11-25T13:00:00)
check "gui-dispatch judge: the data entry split to the online channel" "1" "$(line "$out" GUI_LOG_CHANNEL_ONLINE)"
check "gui-dispatch judge: the online channel got format_target=false" "1" "$(line "$out" GUI_LOG_ONLINE_PARAMS_NO_FORMAT)"
check "gui-dispatch judge mutation: the online restore start is counted, the backup start count does not change" "1" "$(line "$out" GUI_LOG_ONLINE_RESTORE_STARTED)"
check "gui-dispatch judge: backup start and restore start are told apart (backup count stays 1)" "1" "$(line "$out" GUI_LOG_ONLINE_STARTED)"
check "gui-dispatch judge: a cancel on any entry is counted regardless of the entry" "1" "$(line "$out" GUI_LOG_CANCELLED_ANY)"

# judge/combo-selected.py：人工拼一份「读取下拉框」的输出。
printf 'COMBO id=1202 count=4 selected=2 visible=True\n  ITEM[0]=C: | NTFS\n  ITEM[1]=D: | UDF\n  ITEM[2]=F: | NTFS | more\n__END__\n' > "$TMP/combo.txt"
check "combo-selected judge: the selected item's text" "F: | NTFS | more" "$(python3 -I "$HERE/judge/combo-selected.py" < "$TMP/combo.txt")"
printf 'COMBO id=1204 count=4 selected=0 visible=True\n  ITEM[0]=C: | NTFS\n' > "$TMP/combo0.txt"
check "combo-selected judge: the first item is not mistaken for 'nothing selected'" "C: | NTFS" "$(python3 -I "$HERE/judge/combo-selected.py" < "$TMP/combo0.txt")"
printf 'COMBO=NO_WINDOW id=1202\n__END__\n' > "$TMP/combo-none.txt"
check "combo-selected judge: no window gives an empty string (the caller must not treat it as a drive)" "" "$(python3 -I "$HERE/judge/combo-selected.py" < "$TMP/combo-none.txt")"
printf 'COMBO id=1202 count=3 selected=-1 visible=True\n  ITEM[0]=C: | NTFS\n' > "$TMP/combo-neg.txt"
check "combo-selected judge: selected=-1 gives an empty string" "" "$(python3 -I "$HERE/judge/combo-selected.py" < "$TMP/combo-neg.txt")"

# judge/windows.py：人工拼「点之前 / 点之后」的窗口清单。
printf 'WINDOWS=1 class=ANY\nWIN hwnd=111 class=BackupRestoreNativeGui title=BackupRestore\n  id=1003 class=Button visible=True text=x\n__END__\n' > "$TMP/wb.txt"
printf 'WINDOWS=1 class=ANY\nWIN hwnd=111 class=BackupRestoreNativeGui title=BackupRestore\n__END__\n' > "$TMP/wa-clean.txt"
printf 'WINDOWS=2 class=ANY\nWIN hwnd=111 class=BackupRestoreNativeGui title=BackupRestore\nWIN hwnd=222 class=#32770 title=Confirm\n__END__\n' > "$TMP/wa-box.txt"
printf 'WINDOWS=2 class=ANY\nWIN hwnd=111 class=BackupRestoreNativeGui title=BackupRestore\nWIN hwnd=333 class=BackupRestoreSystemDriveChoice title=Choose\n__END__\n' > "$TMP/wa-choice.txt"
printf 'HELPER=resident pid=5\n__END__\n' > "$TMP/wa-unread.txt"
out=$(python3 -I "$HERE/judge/windows.py" "$TMP/wb.txt" "$TMP/wa-clean.txt"); rc=$?
check "windows judge: a clean after-listing passes" "0" "$rc"
check "windows judge: clean - no message box" "0" "$(line "$out" MESSAGE_BOXES_AFTER)"
check "windows judge: clean - no new window class" "" "$(line "$out" NEW_WINDOW_CLASSES_AFTER)"
check "windows judge: clean - both listings were read" "both" "$(line "$out" WINDOWS_READ)"
out=$(python3 -I "$HERE/judge/windows.py" "$TMP/wb.txt" "$TMP/wa-box.txt")
check "windows judge: a message box still up is counted" "1" "$(line "$out" MESSAGE_BOXES_AFTER)"
check "windows judge: a message box is a new window class" "#32770" "$(line "$out" NEW_WINDOW_CLASSES_AFTER)"
out=$(python3 -I "$HERE/judge/windows.py" "$TMP/wb.txt" "$TMP/wa-choice.txt")
check "windows judge mutation: the handler-choice dialog appearing after NO is a new window class" "BackupRestoreSystemDriveChoice" "$(line "$out" NEW_WINDOW_CLASSES_AFTER)"
out=$(python3 -I "$HERE/judge/windows.py" "$TMP/wb.txt" "$TMP/wa-unread.txt"); rc=$?
check "windows judge: an unreadable after-listing exits non-zero" "1" "$rc"
check "windows judge: unreadable after-listing is not 'no message box'" "<unreadable>" "$(line "$out" MESSAGE_BOXES_AFTER)"
check "windows judge: which listing was read is named" "before_only" "$(line "$out" WINDOWS_READ)"
out=$(python3 -I "$HERE/judge/windows.py" "$TMP/wa-unread.txt" "$TMP/wa-clean.txt"); rc=$?
check "windows judge: an unreadable before-listing exits non-zero too (it would make 'new' empty and pass)" "1" "$rc"

# judge/wim-info.py：用命令行冒烟留下的真实样本。
cp "$HERE/fixtures/cli-probe-sample-wim-info.txt" "$TMP/wi.txt"
out=$(python3 -I "$HERE/judge/wim-info.py" "$TMP/wi.txt" 2.10.6); rc=$?
check "wim-info judge: the real sample is read" "0" "$rc"
check "wim-info judge: the command's own success flag (it is in the second JSON line)" "True" "$(line "$out" WIM_INFO_SUCCESS)"
check "wim-info judge: the sample has one index" "1" "$(line "$out" WIM_IMAGE_COUNT)"
check "wim-info judge: the sample was written by 2.10.6" "2.10.6" "$(line "$out" WIM_PROGRAM_VERSION)"
check "wim-info judge: version equal to the expected one" "True" "$(line "$out" WIM_VERSION_MATCH)"
out=$(python3 -I "$HERE/judge/wim-info.py" "$TMP/wi.txt" 2.10.11)
check "wim-info judge mutation: an image written by another version does not match" "False" "$(line "$out" WIM_VERSION_MATCH)"
python3 -I - "$TMP/wi.txt" "$TMP/wi-mut.txt" <<'PY'
import json, sys
lines = open(sys.argv[1], 'rb').read().decode('utf-8', 'replace').splitlines()
value = json.loads(next(l for l in lines if l.strip().startswith('{')))
del value['images']
open(sys.argv[2], 'wb').write((json.dumps(value) + '\nVM_DONE=0\n').encode('utf-8'))
PY
out=$(python3 -I "$HERE/judge/wim-info.py" "$TMP/wi-mut.txt" 2.10.6); rc=$?
check "wim-info judge mutation: no images field -> not read, exits non-zero" "1" "$rc"
check_in "wim-info judge mutation: no images field is named" "WIM_INFO_READ=False reason=no_images_field" "$out"

for f in run-kept.sh orphan-race.sh cli-probe.sh data-guard.sh pause-backup.sh pause-restore.sh data-restore.sh \
         gui-fix-check.sh gui-choice-text.sh gui-matrix.sh gui-data-restore.sh gui-backup.sh vm-unit.sh mutate.sh \
         mutation-guard.sh mutation-progress.sh rebuild-deploy.sh clean-fixtures.sh lib/vm.sh lib/common.sh \
         ../gui-check/restore-confirm.sh; do
  bash -n "$HERE/$f" 2>/dev/null || { echo "SELFTEST syntax $f FAIL"; failed=$((failed+1)); }
done
echo "SELFTEST syntax of every script PASS"

# 每段内嵌的判读程序：用到的标准库模块必须都引入了（2026-11-25 实测：给一段加了正则解析却没引入 re，
# 整段读数半途中断，步骤与自检行读成空——脚本响亮地报了失败，但那次实机运行白跑了）。
imports_missing=$(python3 -I - "$HERE" "$HERE/../gui-check/restore-confirm.sh" <<'PY'
import glob, re, sys
here = sys.argv[1]
bad = []
for path in sorted(glob.glob(here + '/*.sh') + [sys.argv[2]]):
    text = open(path, encoding='utf-8').read()
    for m in re.finditer(r"python3 -I[^\n]*<<'PY'[^\n]*\n(.*?)\nPY\n", text, flags=re.S):
        block = m.group(1)
        imported = set()
        for imp in re.findall(r'^import ([\w, ]+)', block, flags=re.M):
            imported.update(x.strip() for x in imp.split(','))
        used = set(re.findall(r'\b(json|os|re|sys)\.', block))
        if used - imported:
            bad.append('%s: missing %s' % (path.split('/')[-1], ','.join(sorted(used - imported))))
print('; '.join(bad))
PY
)
if [ -n "$imports_missing" ]; then
  echo "SELFTEST embedded judge blocks import what they use FAIL $imports_missing"; failed=$((failed+1))
else
  echo "SELFTEST embedded judge blocks import what they use PASS"
fi

# run_prepare 靠变量传结果，调用处接管道会让它跑进子进程、变量传不回来（准备却已经成功、机器正要重启）。
# 扫描时跳过本文件：检查条件自己的源码行就含有这个模式，不跳过会永远命中自己。
piped=""
for f in "$HERE"/*.sh; do
  [ "$(basename "$f")" = selftest.sh ] && continue
  hit=$(grep -n -E 'run_prepare .*[|]' "$f" | cut -c1-140 | tr '\n' ';')
  [ -n "$hit" ] && piped="$piped $(basename "$f"):$hit"
done
if [ -n "$piped" ]; then
  echo "SELFTEST run_prepare is piped at:$piped FAIL"
  failed=$((failed+1))
else
  echo "SELFTEST run_prepare is never piped PASS"
fi

# 三道确认的驱动（restore-confirm.sh）：真正会下发的那一次「是」之前必须有授权闸。
# 离线流程的闸在最后一道确认之前；纯在线流程（选择框 = none）没有第二道确认，破坏性确认上的「是」就是下发，
# 所以闸必须在 `click_ok MSGBOX 6` 之前。2026-11-26 发现「none」原本是个绕过授权的口子。
rc_driver="$HERE/../gui-check/restore-confirm.sh"
gate_line=$(grep -n 'online_yes_requires_BR_ALLOW_RESTORE' "$rc_driver" | head -1 | cut -d: -f1)
first_yes_line=$(grep -n '^click_ok MSGBOX 6$' "$rc_driver" | head -1 | cut -d: -f1)
final_gate_line=$(grep -n 'yes_requires_BR_ALLOW_RESTORE' "$rc_driver" | grep -v online_yes | head -1 | cut -d: -f1)
final_yes_line=$(grep -n '^  say "点击最后确认框的按钮' "$rc_driver" | head -1 | cut -d: -f1)
if [ -n "$gate_line" ] && [ -n "$first_yes_line" ] && [ "$gate_line" -lt "$first_yes_line" ] \
   && [ -n "$final_gate_line" ] && [ -n "$final_yes_line" ] && [ "$final_gate_line" -lt "$final_yes_line" ]; then
  echo "SELFTEST restore-confirm driver: both real dispatches are gated by BR_ALLOW_RESTORE PASS"
else
  echo "SELFTEST restore-confirm driver: both real dispatches are gated by BR_ALLOW_RESTORE FAIL gate=$gate_line first_yes=$first_yes_line final_gate=$final_gate_line final_yes=$final_yes_line"; failed=$((failed+1))
fi

# ---- 1b. 日志解析：按 PowerShell 5 的三段结构人工拼一份日志，解析后必须读得到结束标记与结束码 ---------------
# 真实踩过的坑：解析脚本按旧名字切结束标记，把新标记的前缀切掉了，一次通过的测试被记成失败。
python3 -I - "$TMP/synth.log" <<'PY'
import sys
head = b'\xef\xbb\xbfTOOLS_TEST_RUN_START tag=x filter=y 01:00:00\r\n'
body = 'running 1 tests\ntest a::b ... ok\ntest result: ok. 1 passed; 0 failed\n'.encode('utf-16-le')
tail = b'TOOLS_TEST_RUN_END tag=x rc=0 01:00:01\r\n'
open(sys.argv[1], 'wb').write(head + body + tail)
PY
python3 -I "$HERE/pslog.py" "$TMP/synth.log" > "$TMP/synth.txt" 2>/dev/null
check_in "pslog keeps the whole end marker" "TOOLS_TEST_RUN_END tag=x rc=0" "$(cat "$TMP/synth.txt")"
check_in "pslog decodes the UTF-16 body" "test result: ok. 1 passed; 0 failed" "$(cat "$TMP/synth.txt")"
check_in "pslog reports the marker" "END_MARKER_FOUND=1" "$(cat "$TMP/synth.txt")"
printf '\xef\xbb\xbfhead only\r\n' > "$TMP/bad.log"
check_in "pslog refuses a log without an end marker" "ERROR=unexpected_log_shape" "$(python3 -I "$HERE/pslog.py" "$TMP/bad.log" 2>&1)"

# ---- 2. 判读程序：真实样本必须通过，变异之后必须变红 --------------------------------------
FX="$HERE/fixtures"; J="$HERE/judge/cli-probe.py"
for f in "$FX"/cli-probe-sample-*.txt; do cp "$f" "$TMP/$(basename "$f" | sed 's/^cli-probe-sample-/sample-/')"; done
out=$(python3 -I "$J" "$TMP" sample); rc=$?
check "cli-probe judge: the real captured sample passes" "0" "$rc"
check_in "cli-probe judge: list-images is read as one object per image" "CLI_PROBE_LIST_IMAGES=PASS count=3" "$out"
check_in "cli-probe judge: the optical drive is recognised as cdrom" "VOLUME letter=D driveType=cdrom" "$out"

# 变异一：把样本里光驱的「驱动器类型」字段删掉 → 列卷那一条必须变红
python3 -I - "$TMP/sample-list-volumes.txt" <<'PY'
import json, sys
p = sys.argv[1]
lines = open(p, 'rb').read().decode('utf-8', 'replace').splitlines()
vols = json.loads(next(l for l in lines if l.strip().startswith('[')))
for v in vols:
    if v['letter'] == 'D':
        del v['driveType']
open(p, 'wb').write((json.dumps(vols, ensure_ascii=False) + '\nVM_DONE=0\n').encode('utf-8'))
PY
out=$(python3 -I "$J" "$TMP" sample); rc=$?
check "cli-probe judge mutation: driveType removed from the optical drive -> FAIL" "1" "$rc"
check_in "cli-probe judge mutation: the failing item is named" "CLI_PROBE_LIST_VOLUMES=FAIL" "$out"

# 变异二：把样本里一条记录整个删掉（模拟命令只输出了结论行）→ 必须变红
python3 -I -c "
import sys
p = sys.argv[1]
lines = open(p, 'rb').read().decode('utf-8', 'replace').splitlines()
keep = [l for l in lines if not ('data-bl-20261124' in l)]
open(p, 'wb').write(('\n'.join(keep) + '\n').encode('utf-8'))
" "$TMP/sample-list-images.txt"
out=$(python3 -I "$J" "$TMP" sample); rc=$?
# 此时的总体 FAIL 来自变异一（列卷那条还没恢复）；这里看的是 list-images 自己：少一条记录不算它失败，但计数要跟着变。
check "cli-probe judge: overall still FAIL while mutation one is in place" "1" "$rc"
check_in "cli-probe judge: one image record missing -> the item passes with count=2" "CLI_PROBE_LIST_IMAGES=PASS count=2" "$out"

# 恢复样本：重新拷一遍必须回到通过（证明变红是因为变异，不是样本本来就坏）
for f in "$FX"/cli-probe-sample-*.txt; do cp "$f" "$TMP/$(basename "$f" | sed 's/^cli-probe-sample-/sample-/')"; done
check "cli-probe judge: sample restored -> PASS again" "0" "$(python3 -I "$J" "$TMP" sample >/dev/null; echo $?)"

if [ $failed -eq 0 ]; then echo "VM_TOOLS_SELFTEST=PASS"; else echo "VM_TOOLS_SELFTEST=FAIL failed=$failed"; fi
exit $failed
