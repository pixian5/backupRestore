#!/bin/bash
# tools/vm/run-kept.sh —— 带保活地运行另一个实机用例脚本。
#
# 为什么需要：测试虚拟机开着「空闲自动暂停」，`prlctl exec` 执行的命令不算活动，距最后一次键鼠注入满 30 分钟
# 就会被暂停；本身不带保活循环的脚本（界面驱动等）会在中途挂住。这里只做 `prlctl resume`，不改任何虚拟机设置。
#
# 用法：tools/vm/run-kept.sh <脚本路径> [参数...]
#   例：tools/vm/run-kept.sh tools/vm/orphan-race.sh new both
# 退出码 = 被包装脚本的退出码。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
script="$1"; shift
[ -f "$script" ] || { echo "ABORT: script not found: $script" >&2; exit 2; }
"$script" "$@" &
runner=$!
say "KEPT_RUN_STARTED pid=$runner script=$(basename "$script")"
while kill -0 "$runner" 2>/dev/null; do keep_alive; sleep 15; done
wait "$runner"; rc=$?
say "KEPT_RUN_EXIT=$rc"
exit "$rc"
