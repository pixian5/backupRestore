#!/bin/bash
# 监控 WinRE 还原进度与屏幕捕获脚本
set -u
VM="Windows 11"
DIR=".test-artifacts/captures/re-restore-v193"
mkdir -p "$DIR"
LOG="$DIR/monitor.log"

log() {
  local msg="$(date +%Y-%m-%d\ %H:%M:%S) $1"
  echo "$msg" | tee -a "$LOG"
}

log "=== 开始监控 RE 还原过程 ==="
OFFLINE_COUNT=0
START_TIME=$(date +%s)

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
    if [ "$OFFLINE_COUNT" -ge 2 ]; then
      log "检测到虚拟机已完成恢复任务并成功重启返回 Windows 桌面！耗时: ${ELAPSED}s"
      break
    else
      log "Windows 运行中 (Session 正常) - 已截图 $SCREEN_FILE"
    fi
  else
    OFFLINE_COUNT=$((OFFLINE_COUNT + 1))
    log "进入恢复环境或重启中 (Guest Tools 离线, 累计 $OFFLINE_COUNT 次) - 已截图 $SCREEN_FILE (耗时 ${ELAPSED}s)"
  fi
  
  # 最多等待 35 分钟（2100 秒）
  if [ "$ELAPSED" -gt 2100 ]; then
    log "超时 35 分钟退出监控"
    break
  fi
  
  sleep 8
done

log "=== 监控结束 ==="
