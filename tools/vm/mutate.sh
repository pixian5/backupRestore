#!/bin/bash
# tools/vm/mutate.sh —— 变异验证的公共函数（被 mutation-*.sh 引用，不要直接运行）。
# 做一件事：改一处源码 → 在虚拟机里跑指定测试 → 必须变红 → 原样恢复并核对文件哈希。
#
#   source tools/vm/mutate.sh 之后调用：
#     mutate <文件> <旧串> <新串> <标签> <测试名过滤串>      返回 0 = 变红且已恢复
#
# 判据：测试输出里有 FAILED 行且 vm-unit.sh 判 FAIL（红）；变异前后文件哈希一致（恢复无损）。
# ★ 旧串必须在文件里恰好出现一次，否则算「变异没打成」并返回非零——锚点对不上时不能假装验证过。
# ★ 变异的读数也要自检：匹配失败行时不能锚定行首（vm-unit.sh 的输出带缩进，三次真红曾被记成「未变红」）。
# ★ 变异不能拿手工模拟冒充：要改的是被测代码本身，然后让同一个测试在改动之下变红。
set -u
mutate() { # $1 文件  $2 旧串  $3 新串  $4 标签  $5 过滤串
  local f="$1" old="$2" new="$3" tag="$4" filter="$5" before after applied="" result="" red="" verdict
  before=$(shasum -a 256 "$f" | awk '{print $1}')
  cp "$f" "${TMPDIR:-/tmp}/br-mutant-backup.$$"
  applied=$(python3 -I - "$f" "$old" "$new" <<'PY'
import sys
p, old, new = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(p, encoding='utf-8').read()
n = s.count(old)
if n != 1:
    print('MUTATION_ANCHOR_COUNT=%d (must be exactly 1)' % n)
    sys.exit(0)
open(p, 'w', encoding='utf-8').write(s.replace(old, new))
print('MUTATION_APPLIED=1')
PY
)
  if [ "$applied" != "MUTATION_APPLIED=1" ]; then
    say "MUTANT $tag NOT APPLIED: $applied"; rm -f "${TMPDIR:-/tmp}/br-mutant-backup.$$"; printf 'MUTANT=%s APPLIED=0\n' "$tag"; return 1
  fi
  sleep 3                                   # 让共享盘看到新文件
  result=$(bash "$HERE_VM/vm-unit.sh" "$tag" "$filter" 2>&1)
  red=$(printf '%s' "$result" | grep -a -E '^ *test .* \.\.\. FAILED|^ *test result: FAILED' | cut -c1-200)
  printf '%s\n' "$result" | grep -a -E 'VM_UNIT=|^ *test |FAILED_TEST' | cut -c1-210 | sed 's/^/    /'
  cp "${TMPDIR:-/tmp}/br-mutant-backup.$$" "$f"; rm -f "${TMPDIR:-/tmp}/br-mutant-backup.$$"
  after=$(shasum -a 256 "$f" | awk '{print $1}')
  if [ -n "$red" ] && printf '%s' "$result" | grep -a -q '^VM_UNIT=FAIL'; then verdict=RED; else verdict=NOT_RED; fi
  say "RESTORED $tag sha256=$after equal=$([ "$before" = "$after" ] && echo True || echo False)"
  printf 'MUTANT=%s VERDICT=%s restored_sha_equal=%s\n' "$tag" "$verdict" "$([ "$before" = "$after" ] && echo True || echo False)"
  [ "$verdict" = RED ] && [ "$before" = "$after" ]
}
