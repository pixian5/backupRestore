#!/bin/bash
# tools/vm/cli-probe.sh —— 命令行探测命令冒烟：四条命令各跑一次，核对输出字段。只读——不创建任务、不改启动配置。
# 判据：每条命令都有回显标记、输出是合法 JSON、结构符合预期；列卷与环境探测里每个卷都带「驱动器类型」字段，
#       光驱被认成 cdrom。判读逻辑在 tools/vm/judge/cli-probe.py（tools/vm/selftest.sh 会拿真实输出样本离线验证它）。
# 用法：tools/vm/cli-probe.sh [镜像路径] [镜像目录]     （默认用基线数据镜像与 H:\BRImages）
# 退出码：0 = 四条全过且没有副作用；1 = 有判据不满足；其它 = 步骤中止。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
DATA_WIM="${1:-$DATA_IMAGE}"
DIR="${2:-H:\\BRImages}"
run_name "cli-probe"

say "STEP=0 deployed binary"
deployed_hashes | head -1
say "STEP=1 readings before (the four commands must not create a task or touch the boot configuration)"
TASKS0=$(task_count)
bcd_readings > "$EV/$NAME-bcd-before.txt"

say "STEP=2 run the four probe commands"
for cmd in "list-volumes --json" "inspect-environment --json" "wim-info $DATA_WIM --json" "list-images $DIR --json"; do
  key=$(printf '%s' "$cmd" | cut -d' ' -f 1)
  vraw "$EXE $cmd" "$NAME-$key.txt" || say "WARNING: $key gave no output marker (the judge will report READ_FAILED)"
done

say "STEP=3 judge the output"
python3 -I "$HERE/judge/cli-probe.py" "$EV" "$NAME" | tee "$EV/$NAME-verdict.txt"
JRC=${PIPESTATUS[0]}

say "STEP=4 readings after"
TASKS1=$(task_count)
bcd_readings > "$EV/$NAME-bcd-after.txt"
SAME=$(bcd_same "$EV/$NAME-bcd-before.txt" "$EV/$NAME-bcd-after.txt")
printf 'TASKS_BEFORE=%s TASKS_AFTER=%s\n%s\n' "$TASKS0" "$TASKS1" "$SAME" | tee -a "$EV/$NAME-verdict.txt"
[ "$TASKS0" = "$TASKS1" ] && [ "$SAME" = "BCD_READINGS_EQUAL=True" ] || { echo "CLI_PROBE_SIDE_EFFECTS=FAIL" | tee -a "$EV/$NAME-verdict.txt"; exit 1; }
[ "$JRC" -eq 0 ]
