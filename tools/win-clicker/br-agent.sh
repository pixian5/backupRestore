#!/bin/bash
# br-agent.sh —— VM 内常驻 GUI 执行器的宿主控制端
#
# 与 br-gui.sh（每次调用启动一次 PowerShell，约 1.4s）的区别：
# agent 常驻在 VM 里，C# 只编译一次，命令通过 Parallels 共享目录投递（不开端口），
# 单步往返约 0.1s。适合几十上百步的 UI 回归流程。
#
# 用法:
#   ./br-agent.sh start                启动常驻 agent（提权，空闲 15 分钟自动退出）
#   ./br-agent.sh status               看心跳
#   ./br-agent.sh click 960 540        单条命令
#   ./br-agent.sh windows              列窗口
#   ./br-agent.sh shot vm.png          agent 侧截图 → .test-artifacts/br-agent/vm.png
#   ./br-agent.sh batch <文件>         一次投递多行命令，按序执行，一次返回全部结果
#   ./br-agent.sh text '中文也行'       键入文本（走 UNICODE，中文可用）
#   ./br-agent.sh stop                 让 agent 退出
#
# 命令文件与结果文件都在 .test-artifacts/br-agent/ 下（宿主就是项目目录，
# VM 里对应 \\Mac\backupRestore\.test-artifacts\br-agent）。

set -u
VM="Windows 11"
PRL="/Applications/Parallels Desktop.app/Contents/MacOS/prlctl"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DIR="$ROOT/tools/win-clicker/_agent"
mkdir -p "$DIR"
CMD="$DIR/cmd.txt"
RES="$DIR/res.txt"

# 提权启动 agent：脚本要落到客体本地盘（runas 进程看不到盘符 X:，但能读 UNC）
start_agent() {
  "$PRL" exec "$VM" --current-user powershell -NoProfile -ExecutionPolicy Bypass \
    -File 'X:\tools\win-clicker\br-gui-exec.ps1' -Script br-agent.ps1 >/dev/null 2>&1
}

# 投递命令：每条用唯一文件名 cmd.<id>.txt，结果 res.<id>.txt。
# 不能复用同一个文件名——共享目录客户端会缓存旧内容十几秒，实测同名覆盖的
# 往返延迟 >12s，换唯一文件名后是毫秒级。
send() {
  local id="$1"; shift
  local tmp="$DIR/cmd.$id.tmp"
  : > "$tmp"
  for l in "$@"; do echo "$l" >> "$tmp"; done
  mv "$tmp" "$DIR/cmd.$id.txt"
  local resf="$DIR/res.$id.txt"
  local i=0
  while [ $i -lt 400 ]; do
    if [ -f "$resf" ]; then
      cat "$resf"
      rm -f "$resf"
      return 0
    fi
    sleep 0.02
    i=$((i+1))
  done
  echo "TIMEOUT: 等不到 id=$id 的结果（agent 还活着吗？先跑 ./br-agent.sh status）"
  return 1
}

CMDNAME="${1:-help}"
case "$CMDNAME" in
  start)
    rm -f "$DIR"/res.*.txt "$DIR"/cmd.*.txt 2>/dev/null
    start_agent
    # 等 agent 就绪（boot 结果出现）
    for i in $(seq 1 120); do
      if [ -f "$DIR/res.boot.txt" ]; then
        echo "agent 已就绪:"; cat "$DIR/res.boot.txt"; rm -f "$DIR/res.boot.txt"; exit 0
      fi
      sleep 0.25
    done
    echo "启动超时：agent 未就绪。检查 .test-artifacts/br-agent/heartbeat.txt"
    exit 1
    ;;
  status)
    cat "$DIR/heartbeat.txt" 2>/dev/null || echo "无心跳文件，agent 未启动"
    ;;
  batch)
    f="${2:?缺命令文件}"
    id="b$(date +%s)-$$"
    # 必须逐行读再当独立参数传出：直接 $(cat) 会被 shell 按空格再切一刀，
    # "move 300 400" 会变成三条命令（实测踩过）
    args=()
    while IFS= read -r line; do
      [ -n "$line" ] && args+=("$line")
    done < "$f"
    send "$id" "${args[@]}"
    ;;
  stop)
    send "q$(date +%s)-$$" "quit" >/dev/null 2>&1
    echo "已发送退出命令"
    ;;
  click|dbl|move)
    send "c$(date +%s)-$$-$RANDOM" "$CMDNAME ${2:?缺 X} ${3:?缺 Y}"
    ;;
  key|chord|text|shot|windows|cursor|tick)
    if [ "$CMDNAME" = "windows" ] || [ "$CMDNAME" = "cursor" ] || [ "$CMDNAME" = "tick" ]; then
      send "c$(date +%s)-$$-$RANDOM" "$CMDNAME"
    else
      send "c$(date +%s)-$$-$RANDOM" "$CMDNAME ${2:?缺参数}"
    fi
    ;;
  *)
    sed -n '2,18p' "$0"
    ;;
esac
