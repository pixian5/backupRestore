#!/bin/bash
VM="Windows 11"
DIR=".test-artifacts/captures/v203-test/backup"
mkdir -p "$DIR"
START=$(date +%s)
echo "=== 开始监控全新系统备份进度与重启闭环 ==="

while true; do
  NOW=$(date +%s)
  ELAPSED=$((NOW - START))
  STAMP=$(date +%H%M%S)
  prlctl capture "$VM" --file "$DIR/frame_$STAMP.png" >/dev/null 2>&1 || true

  # 检查客体系统是否已经备份完成并重启返回 Windows 桌面
  RESP=$(prlctl exec "$VM" whoami 2>&1 || true)
  if echo "$RESP" | grep -q "nt authority\\\\system"; then
    echo "备份完成！Windows 已重启成功返回系统桌面！耗时: ${ELAPSED}s"
    prlctl capture "$VM" --file "$DIR/backup_desktop_success.png" >/dev/null 2>&1 || true
    break
  fi

  # 30 分钟超时保底
  if [ "$ELAPSED" -gt 1800 ]; then
    echo "监控达到 30 分钟上限"
    break
  fi
  sleep 15
done
