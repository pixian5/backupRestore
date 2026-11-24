#!/bin/bash
# tools/vm/vm-unit.sh —— 在虚拟机里跑 Windows 原生单元测试（定向或全量），等它结束，把日志原样取回并解析。
#
# 为什么需要：`#[cfg(windows)]` 模块里的测试在 macOS 上一次都不会执行；本机的 Windows 目标类型检查只编译不运行。
#
# 用法：
#   tools/vm/vm-unit.sh <标签> <测试名过滤串>        定向（十几秒，不含首次编译）
#   tools/vm/vm-unit.sh <标签>                      全量（先重新编译测试目标，几分钟）
#   tools/vm/vm-unit.sh <标签> --allow-known        全量，且允许那两项已知的环境相关失败（见下）
# 标签只用字母、数字、下划线与连字符（用在虚拟机里的日志文件名上）。
#
# 判据：日志里有结束标记、有用例跑过、没有失败的用例。
# ★ 这台测试机上全量恒有 2 项环境相关失败（缺 sh、非提权会话认不了恢复分区；对照工作树与持续集成上都确认过，
#   见 经验/06）。默认它们算失败；加 --allow-known 才放行，且仍会把它们逐条打印出来——不许悄悄变绿。
# ★ 共享盘 X: 的映射不跨重启：每次先映射，并以「虚拟机里读到的 VERSION 与宿主一致」为准。
# ★ 空的过滤串不要传给 PowerShell（空参数会被命令行解析吃掉，报「参数缺值」，测试根本不启动）。
#   启动后 3 秒核对日志文件确实出现了——「启动回显」不算数。
# 退出码：0 = 通过；1 = 有用例失败；2 = 没启动 / 读不到日志。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
TAG="${1:?用法: vm-unit.sh <标签> [测试名过滤串] [--allow-known]}"; shift
FILTER=""; ALLOW_KNOWN=0
for a in "$@"; do
  case "$a" in
    --allow-known) ALLOW_KNOWN=1 ;;
    *) FILTER="$a" ;;
  esac
done
case "$TAG" in *[!A-Za-z0-9_-]*) say "ABORT: the tag may only contain letters, digits, '_' and '-'"; exit 2 ;; esac
LOG="H:\\brwork\\test-$TAG.log"
KNOWN_ENV_FAILURES="process_runner::tests::dispatch_failure_carries_its_code_even_without_json winre_registration::tests::recovery_object_accepts_only_the_registered_partition"

say "STEP=1 map the share as X: in the user session; the VERSION read through it must equal the host's"
v=""
for i in 1 2 3 4; do
  "$PRL" exec "$VM" --current-user cmd /d /c "net use X: \\\\Mac\\backupRestore /persistent:no" >/dev/null 2>&1
  # 这里不经 vrun_user：它会追加回显标记，把版本号读成「2.10.10VM_DONE=0」。读不到就是读不到，下面会中止。
  v=$("$PRL" exec "$VM" --current-user cmd /d /c "type X:\\VERSION" 2>/dev/null | tr -cd '\11\12\40-\176\r' | tr -d '\r\n')
  [ "$v" = "$VERSION" ] && break
  sleep 2
done
say "X_VERSION=${v:-<unreadable>} HOST_VERSION=$VERSION"
[ "$v" = "$VERSION" ] || { say "ABORT: the share is not readable as X: (or shows a different VERSION); the test cannot run"; exit 2; }

say "STEP=2 start the test (old log removed first: a half-written old log would look like a stable result)"
vrun "del /q $LOG" 2 | grep -a 'VM_DONE' | sed 's/^/OLD_LOG_REMOVED /'
launch_args=(-Tag "$TAG"); [ -n "$FILTER" ] && launch_args+=(-Filter "$FILTER")
launch=""
for i in 1 2 3; do
  launch=$("$PRL" exec "$VM" --current-user powershell -NoProfile -ExecutionPolicy Bypass -File "$SHARE_VM_PS\\test-launcher.ps1" "${launch_args[@]}" 2>/dev/null | tr -d '\r')
  printf '%s' "$launch" | grep -a -q '^LAUNCHED pid=' && break
  sleep 2
done
say "LAUNCH: $(printf '%s' "$launch" | tr '\n' ' ') filter=${FILTER:-<all>}"
sleep 3
[ -n "$(vrun "dir /b $LOG" 2 | tr -cd '\11\12\40-\176' | grep -a "test-$TAG.log")" ] || { say "ABORT: the test log does not exist 3 s after launch -- the test did not start"; exit 2; }

say "STEP=3 wait for the end marker"
t=0; end=""
while [ $t -lt 2400 ]; do
  keep_alive
  end=$(vrun "findstr /c:\"TOOLS_TEST_RUN_END\" $LOG" 2 | tr -cd '\11\12\40-\176' | grep -a 'TOOLS_TEST_RUN_END' | tail -1)
  [ -n "$end" ] && break
  sleep 8; t=$((t+8))
done
say "WAITED=${t}s END_MARK=${end:-<none within limit>}"
[ -n "$end" ] || exit 2

say "STEP=4 read the log back verbatim and parse it"
fetch "$LOG" "unit-$TAG.raw.log" || { say "ABORT: could not fetch the log verbatim"; exit 2; }
python3 -I "$HERE/pslog.py" "$EV/unit-$TAG.raw.log" > "$EV/unit-$TAG.txt" 2>"$EV/unit-$TAG.err" || { say "ABORT: log parse failed"; cat "$EV/unit-$TAG.err"; exit 2; }
grep -a -E '^test |^test result|TOOLS_TEST_RUN_END|FAILED|panicked|^error' "$EV/unit-$TAG.txt" | cut -c1-240 | sed 's/^/  /' | head -80

say "STEP=5 judge"
python3 -I - "$EV/unit-$TAG.txt" "$ALLOW_KNOWN" "$KNOWN_ENV_FAILURES" <<'PY' | tee "$EV/unit-$TAG-verdict.txt"
import re, sys
text = open(sys.argv[1], encoding='utf-8', errors='replace').read()
allow_known, known = sys.argv[2] == '1', set(sys.argv[3].split())
m = re.search(r'TOOLS_TEST_RUN_END tag=\S+ rc=(\d+)', text)
rc = m.group(1) if m else '<missing>'
results = re.findall(r'^test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored', text, flags=re.M)
failed = re.findall(r'^test (\S+) \.\.\. FAILED', text, flags=re.M)
passed = sum(int(r[1]) for r in results); ignored = sum(int(r[3]) for r in results)
print("END_RC=%s BINARIES_WITH_RESULTS=%d PASSED=%d IGNORED=%d FAILED=%d" % (rc, len(results), passed, ignored, len(failed)))
for name in failed:
    print("FAILED_TEST %s%s" % (name, "  (known environment-related)" if name in known else ""))
unexpected = [name for name in failed if not (allow_known and name in known)]
ok = rc == '0' or (rc != '<missing>' and allow_known and not unexpected and passed > 0)
if not results:
    ok = False
    print("NOTE=no 'test result' line: was the filter spelled right? (zero tests ran is not a pass)")
print("VM_UNIT=" + ("PASS" if ok else "FAIL") + (" (known environment-related failures allowed and listed above)" if ok and failed else ""))
PY
grep -a -q '^VM_UNIT=PASS' "$EV/unit-$TAG-verdict.txt" && exit 0
grep -a -q '^VM_UNIT=FAIL' "$EV/unit-$TAG-verdict.txt" && exit 1
exit 2
