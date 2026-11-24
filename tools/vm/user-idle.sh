#!/usr/bin/env bash
# user-idle.sh —— 问虚拟机的用户会话：上一次键鼠输入是多久之前？用来判断「有没有人在用这台虚拟机」。
#
# 为什么不用 quser：本地控制台会话的 quser 在「空闲时间」一栏恒显示 none
# （2026-11-24 实测：唤醒后立刻读与几分钟后读完全一样），分不出「有人在敲」和「开机后没人碰过」。
# 所以在用户会话里直接问系统（GetLastInputInfo），并把「开机时长」一起看：重启后输入计时归零，
# 「空闲 70 秒」可能只是「刚开机 70 秒、没人碰过」。
#
# 用法：
#   tools/vm/user-idle.sh [阈值秒，默认 120]      # 必须从共享盘能被虚拟机访问的仓库里跑（\\Mac\backupRestore）
#   BR_VM='Windows 11' tools/vm/user-idle.sh
#
# 输出（结构化）：SESSION=  IDLE_MS=  IDLE_SECS=  UPTIME_MS=  LAST_INPUT_AFTER_BOOT_MS=  IDLE_VERDICT=
#   IDLE_VERDICT=NO_INPUT_SINCE_BOOT   开机以来没有任何输入 → 没人在用
#   IDLE_VERDICT=IDLE                  有过输入，但最后一次距今超过阈值 → 没人在用
#   IDLE_VERDICT=IN_USE                最后一次输入距今不足阈值 → 有人在用
#   IDLE_VERDICT=UNKNOWN               读不到（通道故障）
#
# 退出码：0 = 没人在用；1 = 有人在用；2 = 读不到。
# ★ 读不到 ≠ 没人在用：prlctl exec 会静默不执行、也会间歇性报 PrlJob_GetResult 错误，所以重试 5 次，
#   仍读不到就退出 2，不给出一个好看的「没人在用」。
set -uo pipefail
VM=${BR_VM:-Windows 11}
THRESHOLD=${1:-120}
SHARE='\\Mac\backupRestore'      # Parallels 共享名；单引号里才是真正的 UNC（两个反斜杠开头、其余单个）
export LC_ALL=C

out=""
for _ in 1 2 3 4 5; do
  out=$(prlctl exec "$VM" --current-user powershell -NoProfile -ExecutionPolicy Bypass \
        -File "${SHARE}\\tools\\vm\\user-idle.ps1" -ThresholdSeconds "$THRESHOLD" 2>/dev/null | tr -d '\r')
  case "$out" in
    *IDLE_VERDICT=*) break ;;
    *IDLE_READ=FAILED*) break ;;
  esac
  sleep 2
done

printf '%s\n' "$out" | grep -E '^(SESSION|IDLE_MS|IDLE_SECS|UPTIME_MS|LAST_INPUT_AFTER_BOOT_MS|IDLE_VERDICT|IDLE_READ)=' | tr '\n' ' '
echo ""
case "$out" in
  *IDLE_VERDICT=IN_USE*) exit 1 ;;
  *IDLE_VERDICT=*) exit 0 ;;
  *) echo "IDLE_VERDICT=UNKNOWN reason=reading_failed (not the same as nobody is using it)" ; exit 2 ;;
esac
