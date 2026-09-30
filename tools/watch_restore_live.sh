#!/bin/bash
VM="Windows 11"
DIR=".test-artifacts/captures/re-restore-live"
mkdir -p "$DIR"
START=$(date +%s)
echo "=== 开始监控还原执行流程 ==="

while true; do
  NOW=$(date +%s)
  ELAPSED=$((NOW - START))
  STAMP=$(date +%H%M%S)
  prlctl capture "$VM" --file "$DIR/frame_$STAMP.png" >/dev/null 2>&1 || true

  # 检查客体系统是否已经重启返回 Windows 桌面
  RESP=$(prlctl exec "$VM" whoami 2>&1 || true)
  if echo "$RESP" | grep -q "nt authority\\\\system"; then
    echo "Windows 已重启成功返回系统桌面！耗时: ${ELAPSED}s"
    prlctl capture "$VM" --file "$DIR/restored_windows_desktop.png" >/dev/null 2>&1 || true
    break
  fi

  # 超时 30 分钟退出
  if [ "$ELAPSED" -gt 1800 ]; then
    echo "监控达到 30 分钟上限"
    break
  fi
  sleep 10
done
