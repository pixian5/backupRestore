#!/bin/bash
# tools/vm/mutation-guard.sh —— 「准备期在办锁」与「孤儿任务守卫」的变异验证（虚拟机内、Windows 原生）。
# 三个变异体，对应的三条单元测试必须变红；每个变异后原样恢复并核对哈希。
#   M1        把守卫里的在办检查拆掉         → 守卫的测试变红
#   M2-lock   在办判据改成恒「没人在办」     → 准备锁的测试变红
#   M2-guard  同上                           → 守卫的测试变红
# 先跑一遍基线：这些测试在未变异时必须是绿的。
#
# 用法：tools/vm/mutation-guard.sh        （约 1 分钟；只临时改工作树里的两个源码文件，结束时原样恢复）
# ★ 跑的中途被强杀会留下变异体：用 `git status --short crates/` 一眼能看出来，`git checkout -- <文件>` 即可恢复。
# 退出码：0 = 基线绿 + 三个变异体全红 + 都已恢复；非 0 = 否。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
source "$HERE/mutate.sh"
P="$REPO/crates/backuprestore-cli/src/pending_task.rs"
L="$REPO/crates/backuprestore-cli/src/prepare_lock.rs"
LOCK_TEST=prepare_lock
GUARD_TEST=orphan_guard
OUT="$EV/mutation-guard-verdict.txt"
: > "$OUT"
rc=0

# run_mutant <名字> <文件> <旧串> <新串> <测试名过滤串>：做一次变异并把结果留档、计入总结果。
# ★ 不能写成 `mutate … | tee … || rc=1`：管道的退出码是 tee 的，失败永远传不出来。先把输出收进变量再判断。
run_mutant() {
  local name="$1"; shift
  local output mrc
  output=$(mutate "$@")
  mrc=$?
  printf '%s\n' "$output" | tee -a "$OUT"
  say "MUTANT_SUMMARY $name rc=$mrc"
  [ "$mrc" -eq 0 ] || rc=1
}

say "baseline: the tests must be green before any mutation"
bash "$HERE/vm-unit.sh" guard-base "$LOCK_TEST" 2>&1 | grep -a -E 'VM_UNIT=|^ *test ' | cut -c1-210 | sed 's/^/  /' | tee "$EV/mutation-guard-baseline.txt"
grep -a -q '^VM_UNIT=PASS' "$EV/unit-guard-base-verdict.txt" || { say "ABORT: the baseline is not green; a mutation cannot prove anything"; exit 1; }
bash "$HERE/vm-unit.sh" guard-base-guard "$GUARD_TEST" 2>&1 | grep -a -E 'VM_UNIT=|^ *test ' | cut -c1-210 | sed 's/^/  /' | tee -a "$EV/mutation-guard-baseline.txt"
grep -a -q '^VM_UNIT=PASS' "$EV/unit-guard-base-guard-verdict.txt" || { say "ABORT: the baseline is not green; a mutation cannot prove anything"; exit 1; }

say "M1: remove the in-flight check from the guard -> the guard test must go red"
run_mutant M1 "$P" \
  'if crate::prepare_lock::in_flight(&entry.path()) {' \
  'if false && crate::prepare_lock::in_flight(&entry.path()) {' \
  mut1-guard "$GUARD_TEST"

say "M2: in_flight always answers 'nobody is preparing' -> the lock tests and the guard test must go red"
run_mutant M2-lock "$L" \
  'pub(crate) fn in_flight(task_dir: &Path) -> bool {' \
  'pub(crate) fn in_flight(task_dir: &Path) -> bool { if task_dir.exists() { return false; }' \
  mut2-lock "$LOCK_TEST"
run_mutant M2-guard "$L" \
  'pub(crate) fn in_flight(task_dir: &Path) -> bool {' \
  'pub(crate) fn in_flight(task_dir: &Path) -> bool { if task_dir.exists() { return false; }' \
  mut2-guard "$GUARD_TEST"

red=$(grep -a -c 'VERDICT=RED' "$OUT")
say "MUTATION_GUARD: $red of 3 mutants went red and were restored"
if [ $rc -eq 0 ] && [ "$red" -eq 3 ]; then echo "MUTATION_GUARD=PASS" | tee -a "$OUT"; else echo "MUTATION_GUARD=FAIL" | tee -a "$OUT"; exit 1; fi
