#!/bin/bash
# tools/vm/mutation-progress.sh —— 「切换日志时补写暂停声明行」的变异验证（虚拟机内、Windows 原生）。
# 把补写改成写到别处，对应的单元测试必须变红；变异后原样恢复并核对哈希。先跑一遍基线（未变异时必须是绿的）。
#
# 用法：tools/vm/mutation-progress.sh     （约 30 秒；只临时改工作树里的一个源码文件，结束时原样恢复）
# 退出码：0 = 基线绿 + 变异体红 + 已恢复；非 0 = 否。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
source "$HERE/mutate.sh"
F="$REPO/crates/backuprestore-cli/src/recovery_progress.rs"
TEST=switching_the_log_path_restates
OUT="$EV/mutation-progress-verdict.txt"
: > "$OUT"

say "baseline: the test must be green before the mutation"
bash "$HERE/vm-unit.sh" restates-base "$TEST" 2>&1 | grep -a -E 'VM_UNIT=|^ *test ' | cut -c1-210 | sed 's/^/  /' | tee "$EV/mutation-progress-baseline.txt"
grep -a -q '^VM_UNIT=PASS' "$EV/unit-restates-base-verdict.txt" || { say "ABORT: the baseline is not green; a mutation cannot prove anything"; exit 1; }

say "mutation: the restatement on log switch is written elsewhere -> the test must go red"
output=$(mutate "$F" \
  '            &new_path,
            &format!(
                "Pause support: operation={operation} supported={}",' \
  '            &std::env::temp_dir().join("br-mutant-discarded.log"),
            &format!(
                "Pause support: operation={operation} supported={}",' \
  restates-mut "$TEST")
mrc=$?
printf '%s\n' "$output" | tee -a "$OUT"
if [ "$mrc" -eq 0 ] && grep -a -q 'VERDICT=RED' "$OUT"; then
  echo "MUTATION_PROGRESS=PASS" | tee -a "$OUT"
else
  echo "MUTATION_PROGRESS=FAIL" | tee -a "$OUT"; exit 1
fi
