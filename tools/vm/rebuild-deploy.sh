#!/bin/bash
# tools/vm/rebuild-deploy.sh —— 在虚拟机内构建并部署（唯一发布路径），然后核对：
#   构建日志有结束标记并以 0 结束；部署位两个文件 == 产物包 == 构建清单（三处哈希一致）；tools/vm/preflight.sh 通过。
# 任何一步不满足就非零退出。
#
# 用法：
#   tools/vm/rebuild-deploy.sh             构建 + 部署 + 核对
#   tools/vm/rebuild-deploy.sh --check     只核对前置条件与路径（共享盘、包装脚本在虚拟机里读得到、界面程序在不在运行），不构建
#
# 实测坑（都踩过）：
#   - 开机自启的界面程序在运行时，构建脚本的部署步骤会**无声中断**（构建进程消失、没有错误输出、没有结束标记）——
#     所以部署前先用系统身份通道结束它；
#   - 共享盘 X: 的映射不跨重启——所以每次先映射，并以读到的 VERSION 与宿主一致为准；
#   - prlctl exec 约 1/8 的调用静默不执行——所以每个读数都重试并带回显确认；
#   - ★ 先提交、后部署：部署前置检查的新鲜度判据是「部署件修改时间 ≥ 最新提交时间」，顺序反了会假红；
#   - ★ 重新构建会改变二进制哈希（链接器把当前时间写进可执行文件头）：没有源码改动时不要跑它，
#     否则部署件不再等于已经验收的交付包。
# 退出码：0 = 构建部署核对全过；非 0 = 否。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
run_name "rebuild-deploy"
CHECK_ONLY=0
for a in "$@"; do case "$a" in --check) CHECK_ONLY=1 ;; *) say "ABORT: unknown argument $a"; exit 2 ;; esac; done
say "STEP=1 map the share as X: in the user session; the VERSION read through it must equal the host's"
v=""
for i in 1 2 3 4; do
  "$PRL" exec "$VM" --current-user cmd /d /c "net use X: \\\\Mac\\backupRestore /persistent:no" >/dev/null 2>&1
  v=$("$PRL" exec "$VM" --current-user cmd /d /c "type X:\\VERSION" 2>/dev/null | tr -cd '\11\12\40-\176\r' | tr -d '\r\n')
  [ "$v" = "$VERSION" ] && break
  sleep 2
done
say "X_VERSION=${v:-<unreadable>} HOST_VERSION=$VERSION"
[ "$v" = "$VERSION" ] || { say "ABORT: the share is not readable as X: (or shows a different VERSION)"; exit 2; }

say "STEP=2 the wrapper scripts must be readable from inside the VM (content criterion: byte counts equal)"
want=$(wc -c < "$HERE/ps/build-wrapper.ps1" | tr -d ' ')
have=$("$PRL" exec "$VM" powershell -NoProfile -Command "(Get-Item -LiteralPath '$SHARE_VM_PS\\build-wrapper.ps1' -ErrorAction SilentlyContinue).Length" 2>/dev/null | tr -d '\r\n ')
say "BUILD_WRAPPER_BYTES host=$want vm=${have:-<unreadable>}"
[ "$want" = "$have" ] || { say "ABORT: tools/vm/ps/build-wrapper.ps1 is not readable from the VM with the same byte count"; exit 2; }
# 构建脚本在不在必须核对，不能只打印：不在就中止（原来只打印一行，构建随后会在「没有结束标记」上失败，原因隔了一层）。
# 这里按共享盘的 UNC 路径查：系统身份通道看不到 X: 这个盘符。
need_guest_file '\\Mac\backupRestore\windows\build-windows.ps1' "windows/build-windows.ps1 is not readable through the share"

process_count() { vrun "tasklist /fi \"IMAGENAME eq BackupRestore.exe\" /fo csv /nh" 3 | tr -cd '\11\12\40-\176' | grep -a -c 'BackupRestore.exe'; }
if [ "$CHECK_ONLY" = 1 ]; then
  say "STEP=--check: GUI processes running now: $(process_count) (the real run stops them before deploying)"
  say "REBUILD_DEPLOY_CHECK=PASS (nothing was built or deployed)"
  exit 0
fi

say "STEP=3 stop the auto-started GUI (the deploy step dies silently while it is running)"
n=""
for i in 1 2 3; do vrun "taskkill /im BackupRestore.exe /f" 2 >/dev/null; sleep 1; n=$(process_count); [ "$n" = "0" ] && break; sleep 2; done
say "GUI_PROCESSES_RUNNING=${n:-<unreadable>}"
[ "${n:-1}" = "0" ] || { say "ABORT: the GUI is still running; deploying now would die silently"; exit 2; }

say "STEP=4 build + deploy inside the VM (the only release path)"
TAG="v$(printf '%s' "$VERSION" | tr -d '.')-$(date +%Y%m%d-%H%M%S)"
LOG="H:\\brwork\\build-$TAG.log"
say "BUILD_LOG=$LOG"
vrun "del /q $LOG" 2 | grep -a 'VM_DONE' | sed 's/^/OLD_LOG_REMOVED /'
launch=""
for i in 1 2 3; do
  launch=$("$PRL" exec "$VM" --current-user powershell -NoProfile -ExecutionPolicy Bypass -File "$SHARE_VM_PS\\build-launcher.ps1" -Tag "$TAG" 2>/dev/null | tr -d '\r')
  printf '%s' "$launch" | grep -a -q '^LAUNCHED pid=' && break
  sleep 2
done
say "LAUNCH: $(printf '%s' "$launch" | tr '\n' ' ')"
printf '%s' "$launch" | grep -a -q '^LAUNCHED pid=' || { say "ABORT: the build was not launched"; exit 2; }
t=0; end=""
while [ $t -lt 2400 ]; do
  keep_alive
  end=$(vrun "findstr /c:\"TOOLS_BUILD_RUN_END\" $LOG" 2 | tr -cd '\11\12\40-\176' | grep -a 'TOOLS_BUILD_RUN_END' | tail -1)
  [ -n "$end" ] && break
  sleep 8; t=$((t+8))
done
say "BUILD_WAITED=${t}s END_MARK=${end:-<none within limit>}"
[ -n "$end" ] || { say "ABORT: the build log has no end marker (the build died silently?)"; fetch "$LOG" "build-$TAG-incomplete.raw.log" && say "the log was saved as $EV/build-$TAG-incomplete.raw.log"; exit 1; }

say "STEP=5 read the build log back verbatim"
fetch "$LOG" "$NAME-build.raw.log" || { say "ABORT: could not fetch the build log verbatim"; exit 1; }
python3 -I "$HERE/pslog.py" "$EV/$NAME-build.raw.log" > "$EV/$NAME-build.txt" 2>"$EV/$NAME-build.err" || { say "ABORT: build log parse failed"; cat "$EV/$NAME-build.err"; exit 1; }
grep -a -E 'Finished|Compiling backup|OK|DEPLOYED|TOOLS_BUILD_RUN|error' "$EV/$NAME-build.txt" | cut -c1-170 | sed 's/^/  /'
grep -a -E -q '^TOOLS_BUILD_RUN_END (tag=[^ ]+ )?rc=0' "$EV/$NAME-build.txt" || { say "ABORT: the build did not end with rc=0"; exit 1; }

say "STEP=6 three-way hash: deployed (both names) == package == manifest"
want=$(python3 -I -c "
import json
m = json.load(open('$REPO/artifacts/windows/BackupRestore-windows-arm64-v$VERSION/build-manifest.json', encoding='utf-8-sig'))
print(m['binarySha256'])
" 2>/dev/null)
pkg=$(shasum -a 256 "$REPO/artifacts/windows/BackupRestore-windows-arm64-v$VERSION/BackupRestore.exe" 2>/dev/null | awk '{print $1}')
say "MANIFEST_SHA256=${want:-<unreadable>}"
say "PACKAGE_SHA256=${pkg:-<unreadable>}"
ok=1
[ -n "$want" ] && [ "$pkg" = "$want" ] || { say "MISMATCH: package != manifest"; ok=0; }
for f in BackupRestore.exe Recovery.exe; do
  h=""
  for i in 1 2 3 4 5; do h=$(vrun "certutil -hashfile H:\\brwork\\$f SHA256" 3 | grep -a -E '^[0-9a-f]{64}$'); [ -n "$h" ] && break; sleep 2; done
  say "DEPLOYED_SHA256[$f]=${h:-<unreadable>}"
  [ -n "$h" ] && [ "$h" = "$want" ] || { say "MISMATCH: deployed $f != manifest"; ok=0; }
done
[ "$ok" = 1 ] || exit 1

say "STEP=7 preflight"
"$HERE/preflight.sh" 2>&1 | grep -a -E 'deploy_freshness|deploy_version|PREFLIGHT' | sed 's/^/  /' | tee "$EV/$NAME-preflight.txt"
grep -a -q 'PREFLIGHT=pass' "$EV/$NAME-preflight.txt" || { say "ABORT: preflight not pass"; exit 1; }
say "REBUILD_DEPLOY=PASS sha256=$want"
