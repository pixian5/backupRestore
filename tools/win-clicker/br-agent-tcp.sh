#!/bin/bash
# br-agent-tcp.sh —— 经局域网 TCP 直连 VM 内常驻 GUI 自动化 agent（宿主侧入口）
#
# 通道：宿主 --TCP--> 10.211.55.x:9124 --> VM 内提权 PowerShell agent --> 真实键鼠注入
# 前提：客体 Windows 防火墙放行该端口（用户已关闭防火墙则无需额外配置）
#
# 用法：
#   ./br-agent-tcp.sh start                 启动 VM 内 agent（提权，空闲 15 分钟自退）
#   ./br-agent-tcp.sh ping                  检查端口是否通
#   ./br-agent-tcp.sh click 960 540         单击（单步往返约 5~10ms）
#   ./br-agent-tcp.sh move 100 200          移动光标
#   ./br-agent-tcp.sh windows               列出窗口（完整中文标题）
#   ./br-agent-tcp.sh cursor / tick         光标位置 / 输入心跳
#   ./br-agent-tcp.sh text '中文也行'        UNICODE 输入
#   ./br-agent-tcp.sh key 0x0D              虚拟键
#   ./br-agent-tcp.sh chord ctrl,p          组合键
#   ./br-agent-tcp.sh shot /tmp/vm.png      截图回传并保存
#   ./br-agent-tcp.sh batch cmd.txt         一次发多行（每行一条命令）
#   ./br-agent-tcp.sh stop                  让 agent 退出
#
# 环境变量：VM_IP（不设则自动从客体 ipconfig 探测）、PORT（默认 9124）

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
PRL="/Applications/Parallels Desktop.app/Contents/MacOS/prlctl"
VM="${VM:-Windows 11}"
PORT="${PORT:-9124}"
IPFILE="$HERE/_agent-tcp/vmip.txt"
PY=/Users/x/.workbuddy/binaries/python/versions/3.13.12/bin/python3
mkdir -p "$HERE/_agent-tcp"

# 探测 VM 局域网 IP（Parallels 共享网络段 10.211.55.x），结果缓存
get_ip() {
  if [ -n "${VM_IP:-}" ]; then echo "$VM_IP"; return; fi
  if [ -f "$IPFILE" ]; then
    local c; c=$(cat "$IPFILE")
    if [ -n "$c" ]; then echo "$c"; return; fi
  fi
  local out
  out=$("$PRL" exec "$VM" --current-user cmd /c "chcp 437 >nul & ipconfig" 2>/dev/null \
        | LC_ALL=C tr -d '\r' | grep -a -o -E '10\.211\.55\.[0-9]+' | grep -v '\.1$' | head -1)
  if [ -z "$out" ]; then
    # 兜底：整个 ipconfig 里找第一个非网关的 IPv4
    out=$("$PRL" exec "$VM" --current-user cmd /c "chcp 437 >nul & ipconfig" 2>/dev/null \
          | LC_ALL=C tr -d '\r' | grep -a -o -E 'IPv4[^:]*:\s*[0-9]{1,3}(\.[0-9]{1,3}){3}' \
          | grep -o -E '[0-9]{1,3}(\.[0-9]{1,3}){3}' | grep -v '^127\.' | head -1)
  fi
  [ -n "$out" ] && echo "$out" > "$IPFILE"
  echo "$out"
}

start_agent() {
  # 走可靠链路：br-s1.sh 会先把宿主最新的 br-gui-exec.ps1 经 UNC 刷进 VM，再由
  # run-in-session(SYSTEM->Session1) + ShellExecute runas 提权启动。
  #
  # 不要再改回下面这种写法：
  #   "$PRL" exec "$VM" --current-user powershell -File 'X:\tools\win-clicker\br-gui-exec.ps1' ...
  # 实测（2026-09-27）它会静默失败：X: 盘上的 br-gui-exec.ps1 是旧副本（$src 为 null，
  # 实测报 "无法绑定参数 LiteralPath 因为该参数是空值"），且 --current-user 在本
  # Parallels 版本上持续不可用。结果是脚本什么都没部署，却启动了 9124 上的旧 agent，
  # 新命令（如 exec）报 "ERR unknown op"。
  "$HERE/br-s1.sh" br-launch-agent.ps1
}

wait_port() {
  local ip="$1" i=0
  while [ $i -lt 60 ]; do
    if nc -z -G 1 "$ip" "$PORT" 2>/dev/null; then return 0; fi
    sleep 0.5; i=$((i+1))
  done
  return 1
}

# 发命令：连接 → 读 READY → 逐条发 → 每条读到 END 为止
py_send() {
  local ip="$1"; shift
  local outpng="$1"; shift
  "$PY" - "$ip" "$PORT" "$outpng" "$@" << 'PYEOF'
import socket, sys, base64
ip, port, outpng = sys.argv[1], int(sys.argv[2]), sys.argv[3]
cmds = sys.argv[4:]
try:
    s = socket.create_connection((ip, port), timeout=15)
except Exception as e:
    print("CONNECT_FAILED %s:%s %s" % (ip, port, e)); sys.exit(1)
# 连接建立后放宽读超时：DISM 捕获/挂载这类命令可能跑几十秒到几分钟，
# 15s 的默认值会把"还在跑"误判成失败。
s.settimeout(900)
f = s.makefile('rwb')
f.readline()  # BRAGENT READY
for c in cmds:
    f.write((c + "\n").encode('utf-8')); f.flush()
    while True:
        raw = f.readline()
        if not raw: break
        line = raw.decode('utf-8', 'replace').rstrip('\r\n')
        if line == 'END': break
        if line.startswith('B64:'):
            if outpng:
                open(outpng, 'wb').write(base64.b64decode(line[4:]))
                print("shot saved -> %s" % outpng)
            else:
                print("shot b64len=%d" % (len(line) - 4))
        else:
            print(line)
s.close()
PYEOF
}

case "${1:-}" in
  start)
    start_agent
    ip=$(get_ip)
    [ -z "$ip" ] && { echo "拿不到 VM IP"; exit 1; }
    if wait_port "$ip"; then echo "agent 就绪 @ $ip:$PORT"; else
      echo "端口未通 $ip:$PORT —— 检查客体防火墙是否放行"
      exit 1
    fi
    ;;
  ping)
    ip=$(get_ip); [ -z "$ip" ] && { echo "拿不到 VM IP"; exit 1; }
    if nc -z -G 2 "$ip" "$PORT" 2>/dev/null; then echo "UP $ip:$PORT"; else echo "DOWN $ip:$PORT"; exit 1; fi
    ;;
  ip)
    get_ip ;;
  stop)
    ip=$(get_ip); py_send "$ip" "" quit ;;
  batch)
    f="${2:?缺命令文件}"; ip=$(get_ip)
    # macOS 自带 bash 3.2 没有 mapfile，用 while 逐行读入数组
    arr=()
    while IFS= read -r line || [ -n "$line" ]; do
      [ -z "$line" ] && continue
      arr+=("$line")
    done < "$f"
    py_send "$ip" "" "${arr[@]}" ;;
  shot)
    out="${2:?缺输出路径}"; ip=$(get_ip); py_send "$ip" "$out" shot ;;
  exec)
    # exec <本地 .ps1>  —— 把脚本 base64(UTF-8) 后交给 VM 内已提权的 agent 执行。
    # 用途：prl_tools_service 停掉后 prlctl exec 通道失效，改用这条 TCP 通道做
    # 校验/改盘/启 GUI 等操作。输出行带 O| 前缀。
    f="${2:?缺脚本路径}"; ip=$(get_ip)
    b64=$(base64 < "$f" | tr -d '\n')
    py_send "$ip" "" "exec $b64" | sed 's/^O|//' ;;
  screen|cursor|tick|windows)
    ip=$(get_ip); py_send "$ip" "" "$1" ;;
  click|dbl|move|key|chord|text)
    ip=$(get_ip); py_send "$ip" "" "$*" ;;
  *)
    cat << 'EOF'
用法：
  start / ping / ip / stop
  click <x> <y> | dbl <x> <y> | move <x> <y>
  key <vk hex>  | chord <mod,key> | text <字符串>
  windows | cursor | tick | screen
  shot <输出路径> | batch <命令文件>
  exec <本地.ps1>     （经 TCP 通道在 VM 内提权执行脚本，不依赖 prlctl exec）
EOF
    exit 1 ;;
esac
