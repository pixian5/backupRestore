#!/bin/bash
# tools/vm/pause-backup.sh —— 备份进行中按「暂停」：必须只记「已忽略」，备份照常完成。
# 同时产出下一步 pause-restore.sh 要用的测试镜像（测试卷的备份，镜像里含 20 GB 占位文件）。
#
# 背景（v2.10.8 修）：备份链里没有任何暂停检查点，旧版本按钮会翻成「继续」而备份照常跑完。
# 现在备份不兑现暂停：进度窗按钮禁用并写「暂停不可用」；按 P 只在恢复环境日志里留一行
#   Pause ignored: operation=backup does not support pause (via key P)
# 判据（全部来自原样取回的日志与状态文件）：
#   1. 任务 success / 100，恢复环境日志里有 4 行 STEP n/4；
#   2. 按过 P 之后至少有一行 Pause ignored；
#   3. 任务早期日志里有声明行 Pause support: operation=backup supported=false（v2.10.10 起才落盘）；
#   4. 镜像与副档都生成了；启动配置前后读数一致。
#
# 用法：tools/vm/pause-backup.sh            （约 4 分钟；会自动重启进恢复环境做备份再回来）
# 前提：虚拟机在运行；部署位是待测的二进制；测试卷（默认 F）可牺牲，有空间放 20 GB 占位文件。
# 退出码：0 = 全部判据通过；1 = 有判据不满足；其它 = 步骤中止。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
run_name "pause-backup"
fail=0

say "STEP=0 deployed binaries"
deployed_hashes

say "STEP=1 boot configuration: file-level backup, verified it reads offline; readings before"
bcd_backup "$NAME" | tee "$EV/$NAME-bcd-file.txt"
bcd_readings > "$EV/$NAME-bcd-before.txt"

say "STEP=2 nobody is using the desktop"
require_idle "$NAME-idle.txt" 120

say "STEP=3 fixtures: the 20 GB zero pad file (created if missing) and no stale test image"
if vrun "if exist $PAD (echo PAD_THERE) else (echo PAD_ABSENT)" 2 | grep -a -q 'PAD_ABSENT'; then
  vrun "fsutil file createnew $PAD $PAD_BYTES" 2 | grep -a 'VM_DONE' | sed 's/^/  PAD_CREATE /'
fi
pad=$(vrun "if exist $PAD (echo PAD_THERE) else (echo PAD_ABSENT)" 2 | tr -cd '\11\12\40-\176' | grep -a -o 'PAD_[A-Z]*' | head -1)
say "PAD_FILE=${pad:-<unreadable>}"
[ "$pad" = "PAD_THERE" ] || { say "ABORT: the pad file is missing (not enough free space on the test volume?)"; exit 3; }
vrun "del /q $IMAGE $IMAGE.index-1.metadata.json" 2 | grep -a 'VM_DONE' | sed 's/^/  OLD_IMAGE_REMOVED /'

say "STEP=4 prepare a backup of the test volume (reboots into WinRE by itself)"
run_prepare "$NAME-prepare.json" --operation backup --source-drive $SOURCE_DRIVE --image-path $IMAGE --boot-menu-name BR-PAUSE --json
prepare_summary "$NAME-prepare.json" "$NAME-prepare-summary.txt"
[ -n "$PREPARE_TASK" ] || { say "ABORT: prepare did not create a task"; exit 4; }

say "STEP=5 wait for the terminal stage; P is pressed at 45/70/100/130 s"
wait_terminal "$PREPARE_TASK" 1200 45 70 100 130 | tee "$EV/$NAME-terminal.txt"

say "STEP=6 collect evidence verbatim"
fetch "H:\\brwork\\tasks\\$PREPARE_TASK\\status.json" "$NAME-status.json" && say "fetched status.json"
fetch "H:\\BRImages\\Recovery.log" "$NAME-Recovery.raw.log" && say "fetched Recovery.log"
fetch "H:\\brwork\\tasks\\$PREPARE_TASK\\Recovery-early.log" "$NAME-task-early.raw.log" && say "fetched task early log"
vrun "dir /b $IMAGE*" 2 | tr -cd '\11\12\40-\176\r' | tr -d '\r' | grep -a 'br-vm-tools-test' | sed 's/^/IMAGE_FILE_AFTER /' | tee "$EV/$NAME-images.txt"

say "STEP=7 judge"
python3 -I - "$EV" "$NAME" "$PREPARE_TASK" <<'PY' | tee "$EV/$NAME-readings.txt"
import json, os, sys
ev, name, task = sys.argv[1], sys.argv[2], sys.argv[3]
def read(f):
    p = os.path.join(ev, f)
    return open(p, 'rb').read().decode('utf-8', 'replace').splitlines() if os.path.exists(p) else []
try:
    s = json.loads(''.join(read(name + '-status.json')).lstrip('\ufeff'))
    print("FINAL_STAGE=%s" % s.get('stage')); print("FINAL_PROGRESS=%s" % s.get('progress'))
except (OSError, ValueError) as e:
    print("FINAL_STAGE=<unreadable>"); print("FINAL_PROGRESS=<unreadable>")
lines = read(name + '-Recovery.raw.log')
idx = [i for i, l in enumerate(lines) if task in l and 'Recovery.exe started' in l]
seg = lines[idx[0]:] if idx else []
print("RECOVERY_STEP_LINES=%d" % sum(1 for l in seg if 'STEP ' in l and '/4' in l))
print("PAUSE_IGNORED_LINES=%d" % sum(1 for l in seg if 'Pause ignored' in l))
for l in seg:
    if ('STEP ' in l and '/4' in l) or 'Pause ignored' in l or 'ERROR' in l or 'BR-' in l:
        print("RECOVERY_LINE " + l[:200])
early = read(name + '-task-early.raw.log')
decl = [l for l in early if 'Pause support:' in l]
print("DECLARATION_LINES=%d" % len(decl))
for l in decl[:2]:
    print("DECLARATION_LINE " + l[:200])
PY
rd() { sed -n "s/^$1=\(.*\)$/\1/p" "$EV/$NAME-readings.txt" | tail -1; }
STAGE=$(rd FINAL_STAGE); PROGRESS=$(rd FINAL_PROGRESS); STEPS=$(rd RECOVERY_STEP_LINES); IGN=$(rd PAUSE_IGNORED_LINES); DECL=$(rd DECLARATION_LINES)
IMGN=$(grep -a -c 'IMAGE_FILE_AFTER' "$EV/$NAME-images.txt")

if [ "$STAGE" = "success" ] && [ "$PROGRESS" = "100" ] && [ "$STEPS" = "4" ]; then
  verdict BACKUP_COMPLETED PASS "stage=success progress=100, four STEP lines, task=$PREPARE_TASK"
else
  verdict BACKUP_COMPLETED FAIL "stage=$STAGE progress=$PROGRESS step_lines=${STEPS:-none} task=$PREPARE_TASK"; fail=1
fi
if [ "${IGN:-0}" -ge 1 ]; then
  verdict BACKUP_PAUSE_IGNORED PASS "$IGN 'Pause ignored' line(s); the capture went on to the end"
else
  verdict BACKUP_PAUSE_IGNORED FAIL "no 'Pause ignored' line for this task (were the keys sent while the capture was running?)"; fail=1
fi
# 声明行从 v2.10.10 起才落盘：没有它说明跑的是更早的二进制，或者补写逻辑坏了。
if [ "${DECL:-0}" -ge 1 ] && grep -a -q 'Pause support: operation=backup supported=false' "$EV/$NAME-task-early.raw.log"; then
  verdict PAUSE_DECLARATION PASS "the early log declares: backup does not honour pause"
else
  verdict PAUSE_DECLARATION FAIL "declaration_lines=${DECL:-none}"; fail=1
fi
if [ "$IMGN" -ge 2 ]; then
  verdict BACKUP_OUTPUT PASS "image and sidecar exist"
else
  verdict BACKUP_OUTPUT FAIL "image_files=$IMGN"; fail=1
fi

say "STEP=8 readings after"
bcd_readings > "$EV/$NAME-bcd-after.txt"
SAME=$(bcd_same "$EV/$NAME-bcd-before.txt" "$EV/$NAME-bcd-after.txt")
printf '%s\n' "$SAME" | tee -a "$EV/$NAME-verdict.txt"
[ "$SAME" = "BCD_READINGS_EQUAL=True" ] || fail=1
WINRE=$(winre_enabled)
printf '%s\n' "$WINRE" | tee -a "$EV/$NAME-verdict.txt"
[ "$WINRE" = "WINRE_ENABLED=True" ] || fail=1

if [ $fail -eq 0 ]; then verdict BACKUP_PAUSE PASS "task=$PREPARE_TASK"; else verdict BACKUP_PAUSE FAIL "task=$PREPARE_TASK (see the lines above)"; exit 1; fi
