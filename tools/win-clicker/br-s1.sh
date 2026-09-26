#!/bin/bash
# br-s1.sh —— 在 VM 的交互桌面（Session 1）以提权(High IL)身份运行 PS 脚本
#
# 为什么需要它：原先的通道是 `prlctl exec --current-user`（它恰好落在 Session 1，
# 再由 br-gui-exec.ps1 用 ShellExecute runas 提权）。2026-09-27 实测该参数持续失败
# （"The virtual machine could not be found"，Parallels 26.4.2 的 Tools 集成问题），
# `--user x --password 1` 只能落到 Session 0（无桌面，无法注入 GUI）。
#
# 现在的链路（全部用系统自带 API，不依赖 Parallels Tools 的用户登录）：
#   prlctl exec (SYSTEM, Session 0)
#     -> run-in-session.ps1  (WTSQueryUserToken + CreateProcessAsUser -> Session 1)
#        -> br-gui-exec.ps1  (ShellExecute runas -> 同桌面 High IL)
#           -> 目标脚本
#
# prlctl exec 会剥掉双引号，所以会话内命令行走文件传递（run-in-session.ps1 -CommandFile）。
# 脚本以 GBK 落地：PowerShell 5.1 的 -File 按系统 ANSI 解码源文件。
#
# 用法：./br-s1.sh <脚本名.ps1> [脚本参数...]
#   例：./br-s1.sh br-inject-probe.ps1 -Tag test

set -u
VM="${VM:-caee9cb3-bac7-41e2-85f2-32b3a7369114}"
PRL="/Applications/Parallels Desktop.app/Contents/MacOS/prlctl"
HERE="$(cd "$(dirname "$0")" && pwd)"
STAGE="$HERE/_s1"
REPO_SHARE='\\Mac\backupRestore\tools\win-clicker\_s1'
PKG='C:\Users\Public\backupRestore-package'
PUB='C:\Users\Public\pkg'
mkdir -p "$STAGE"

run() { "$PRL" exec "$VM" cmd /c "$1" 2>&1 | LC_ALL=C tr -d '\r'; }

to_gbk() { iconv -f UTF-8 -t GBK "$1" > "$2" || { echo "GBK_CONVERT_FAIL $1"; exit 3; }; }

to_gbk "$HERE/run-in-session.ps1" "$STAGE/run-in-session.ps1"
to_gbk "$HERE/br-gui-exec.ps1"   "$STAGE/br-gui-exec.ps1"

run "copy /y $REPO_SHARE\\run-in-session.ps1 $PKG\\run-in-session.ps1" >/dev/null
run "copy /y $REPO_SHARE\\br-gui-exec.ps1 $PKG\\br-gui-exec.ps1" >/dev/null

SCRIPT="${1:?缺脚本名}"; shift || true
ARGS="$*"
LOGNAME="$(basename "$SCRIPT" .ps1).out.log"
B64ARG=""
[ -n "$ARGS" ] && B64ARG="-B64 $(printf '%s' "$ARGS" | base64 | tr -d '\n')"

printf 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File %s\\br-gui-exec.ps1 -Script %s %s -Log %s' \
    "$PKG" "$SCRIPT" "$B64ARG" "$LOGNAME" > "$STAGE/s1-cmd.txt"

run "copy /y $REPO_SHARE\\s1-cmd.txt $PUB\\s1-cmd.txt" >/dev/null
run "del /q $PUB\\$LOGNAME" >/dev/null
echo ">> launch $SCRIPT (log=$LOGNAME)"
run "powershell -NoProfile -ExecutionPolicy Bypass -File $PKG\\run-in-session.ps1 -SessionId 1 -CommandFile $PUB\\s1-cmd.txt"
sleep 3
echo ">> ---- $LOGNAME ----"
run "type $PUB\\$LOGNAME"