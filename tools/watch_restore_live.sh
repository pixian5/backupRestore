#!/bin/bash
VM="Windows 11"
DIR=".test-artifacts/captures/v204-restore"
mkdir -p "$DIR"
START=$(date +%s)
echo "=== 开始监控还原及重启回桌面 ==="

for i in $(seq 1 40); do
  STAMP=$(date +%H%M%S)
  prlctl capture "$VM" --file "$DIR/frame_$STAMP.png" >/dev/null 2>&1 || true

  RESP=$(prlctl exec "$VM" whoami 2>&1 || true)
  if echo "$RESP" | grep -q "nt authority\\\\system"; then
    echo "系统还原成功！Windows 已重启并成功进入桌面！"
    prlctl capture "$VM" --file "$DIR/restore_desktop_success.png" >/dev/null 2>&1 || true
    exit 0
  fi
  sleep 10
done
echo "超时"
exit 1
