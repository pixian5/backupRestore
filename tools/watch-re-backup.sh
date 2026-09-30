#!/bin/bash
# 监控 WinRE 备份进度与屏幕捕获脚本
set -u
VM="Windows 11"
DIR=".test-artifacts/captures/re-6wim-v1818"
mkdir -p "$DIR"
LOG="$DIR/monitor.log"

log() {
  local msg="$(date +%Y-%m-%d\ %H:%M:%S) $1"
  echo "$msg" | tee -a "$LOG"
}

log "=== 开始监控 RE 备份过程 ==="
IN_RE=0
START_TIME=$(date +%s)
SCREEN_INDEX=0

while true; do
  NOW=$(date +%s)
  ELAPSED=$((NOW - START_TIME))
  STAMP=$(date +%H%M%S)
  SCREEN_FILE="$DIR/re-$STAMP.png"
  
  # 截取当前屏幕
  prlctl capture "$VM" --file "$SCREEN_FILE" >/dev/null 2>&1 || true
  
  # 检测客体操作系统响应能力
  RESP=$(prlctl exec "$VM" whoami 2>&1 || true)
  if echo "$RESP" | grep -q "nt authority\\\\system"; then
    if [ "$IN_RE" -eq 1 ]; then
      log "检测到虚拟机已成功重启返回 Windows 桌面！耗时: ${ELAPSED}s"
      break
    else
      log "Windows 运行中 (Session 正常) - 已截图 $SCREEN_FILE"
    fi
  else
    IN_RE=1
    log "进入恢复环境或重启中 (Guest Tools 离线) - 已截图 $SCREEN_FILE (耗时 ${ELAPSED}s)"
  fi
  
  # 最多等待 30 分钟（1800 秒）
  if [ "$ELAPSED" -gt 1800 ]; then
    log "超时 30 分钟退出监控"
    break
  fi
  
  sleep 15
done

log "=== 监控结束 ==="
