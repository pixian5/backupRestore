#!/bin/bash
# 在 Windows 客体里执行 PowerShell 脚本，**不弹可见控制台窗口**。
#
# 背景（2026-10-19 用户指示）：以前用
#   prlctl exec "Windows 11" cmd /d /c '<prog> > file 2>&1 & type file'
# 这种方式会在客体交互会话里生成一个 cmd 控制台窗口并**抢到前台**，把
# BackupRestore 主程序界面盖住，用户得先最小化/还原窗口才能看到主程序。
#
# 现在统一走 `powershell -WindowStyle Hidden -EncodedCommand`：窗口隐藏，
# stdout 由 prlctl exec 原样带回宿主机，既不需要 `type` 也不会有窗口闪现。
#
# 用法：br-ps.sh <宿主机上的 .ps1 脚本路径>
set -e
if [ $# -lt 1 ]; then
    echo "usage: $0 <script.ps1>" >&2
    exit 2
fi
SCRIPT="$1"
[ -f "$SCRIPT" ] || { echo "script not found: $SCRIPT" >&2; exit 2; }
# -EncodedCommand 需要 UTF-16LE 的 base64
B64=$(iconv -t UTF-16LE "$SCRIPT" | base64 | tr -d '\n')
prlctl exec "Windows 11" powershell -NoProfile -WindowStyle Hidden -EncodedCommand "$B64" 2>&1
