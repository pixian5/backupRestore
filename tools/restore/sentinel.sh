#!/bin/bash
# 还原验收哨兵法：把「哨兵文件消不消失 + 卷序列号变不变」变成一条命令。
#
# 用法：
#   ./tools/restore/sentinel.sh arm    --drive <盘符> [--vm "Windows 11"]
#   ./tools/restore/sentinel.sh verify --drive <盘符> [--vm "Windows 11"]
#   ./tools/restore/sentinel.sh status --drive <盘符>
#
# 典型流程（离线还原系统盘时）：
#   1. 还原前：sentinel.sh arm --drive C
#        · 在 C:\ 写一个 BR-<时分秒>.txt
#        · 记下 C: 当前的卷序列号（fsutil 输出的 0x…）
#        · 记下当前 BCD default（若可读）
#   2. 跑还原（进 WinRE → apply → BCDBoot → 回来）
#   3. 回来后：sentinel.sh verify --drive C
#        · 哨兵文件应当**不见了**（目标卷被整体重写）
#        · 卷序列号应当**变了**
#        · 两者都满足才算 PASS
#
# ## 为什么是这两条
#
# 「序列号变化 + 哨兵消失」是唯一不依赖界面、不依赖日志措辞的铁证：
#   · 只看日志「还原成功」→ 日志可能来自上一次跑（本仓多次踩过读旧文件）
#   · 只看截图 → 弱信号，本仓已明确不算验收
#   · 只看 BCD → 改 BCD 是还原之后的一步，能过但不能证明卷被写了
#
# ## 为什么不直接用日志里的 RESTORE_SELFCHECK
#
# 那是**产品自己报的自检**。自己给自己盖章只能证明「它认为自己做了」。
# 这条脚本用**外部可观测的物理事实**（文件没了、序列号换了）来交叉验证。
#
# ## 判据是三态，不是两态
#
#   VERDICT=PASS    哨兵消失 且 序列号变化
#   VERDICT=WARN    只满足一条（列明是哪一条没满足）
#   VERDICT=FAIL    两条都没满足
#   VERDICT=ERROR   读不到（卷不存在 / 状态文件丢失 / 通道抽风）——与上面三种分开
#
# 「读不到」和「判据不满足」必须分开报，否则通道抽风会被记成产品缺陷。
set -uo pipefail
cd "$(dirname "$0")/../.."
ROOT=$(pwd)

CMD="${1:-}"; shift || true
DRIVE=""
VM_NAME="Windows 11"
while [ $# -gt 0 ]; do
  case "$1" in
    --drive) DRIVE="${2:?--drive 需要参数}"; shift ;;
    --vm)    VM_NAME="${2:?--vm 需要参数}"; shift ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
  shift
done

[ -n "$CMD" ] || { echo "用法：$0 {arm|verify|status} --drive <盘符>" >&2; exit 2; }
[ -n "$DRIVE" ] || { echo "缺少 --drive" >&2; exit 2; }
DRIVE=$(echo "$DRIVE" | tr -d ':' | tr '[:lower:]' '[:upper:]')
STATE_DIR="$ROOT/.br-sentinel"
STATE="$STATE_DIR/$DRIVE.state"

# 通道净化：Windows 侧中文输出是 GBK，混进管道会让 grep 把整段判成「二进制」
# 而拒绝输出匹配（实测：0x… 明明在，grep 却一个字符都不给）。这里只保留可打印
# ASCII 与换行——判据本来也只该在 ASCII 上做，中文标签是「数据」、不能参与判定。
# LC_ALL=C：macOS 的 tr 在 UTF-8 locale 下遇到 GBK 字节会直接报 Illegal byte sequence
# 并放弃整条管道——必须按字节处理。
# 不留 \r：行尾带 CR 会让 `grep -x` 永远不匹配（文件名明明在，判据说不在）。
vm_run() { prlctl exec "$VM_NAME" cmd /d /c "$1" 2>/dev/null | LC_ALL=C tr -cd '\11\12\40-\176'; }

# 卷序列号：fsutil 的输出是中文，但序列号本身带 0x 前缀、纯 ASCII。
# 只认这个结构，绝不 grep 中文标签。
volume_serial() {
  local out
  out=$(vm_run "fsutil fsinfo volumeinfo $DRIVE:" || true)
  echo "$out" | grep -o '0x[0-9a-fA-F]\{1,16\}' | head -1
}

sentinel_exists() {
  # if exist 的返回码不可靠（cmd 各版本不一致），改为列目录后精确匹配文件名。
  local out
  out=$(vm_run "dir /b $DRIVE:\\" || true)
  echo "$out" | grep -qxF "$1" && echo 1 || echo 0
}

bcd_default() {
  vm_run "bcdedit /enum {bootmgr}" | grep -i '^default' | awk '{print $2}' | tr -d '\r'
}

case "$CMD" in
  arm)
    mkdir -p "$STATE_DIR"
    NAME="BR-$(date '+%H%M%S').txt"
    SERIAL=$(volume_serial)
    if [ -z "$SERIAL" ]; then
      echo "ERROR：读不到 $DRIVE: 的卷序列号（卷不存在？通道抽风？）" >&2
      echo "VOLUME_NOT_FOUND=1"
      exit 2
    fi
    # 哨兵内容无关紧要，但写点东西便于人眼确认「这确实是那条」。
    vm_run "echo BackupRestore sentinel $NAME $SERIAL>$DRIVE:\\$NAME" >/dev/null
    if [ "$(sentinel_exists "$NAME")" != "1" ]; then
      echo "ERROR：哨兵写入 $DRIVE:\\$NAME 后读不回来" >&2
      exit 2
    fi
    DEF=$(bcd_default || true)
    {
      echo "DRIVE=$DRIVE"
      echo "SENTINEL=$NAME"
      echo "SERIAL=$SERIAL"
      echo "BCD_DEFAULT=${DEF:-unknown}"
      echo "ARMED_AT=$(date '+%Y-%m-%d %H:%M:%S')"
    } > "$STATE"
    echo "SENTINEL_ARMED=$NAME"
    echo "SERIAL_BEFORE=$SERIAL"
    echo "BCD_DEFAULT_BEFORE=${DEF:-unknown}"
    echo "状态已存：.br-sentinel/$DRIVE.state（不入库）"
    ;;

  verify)
    if [ ! -f "$STATE" ]; then
      echo "ERROR：找不到 .br-sentinel/$DRIVE.state —— 先跑 arm" >&2
      echo "STATE_MISSING=1"
      exit 2
    fi
    NAME=$(awk -F= '/^SENTINEL=/{print $2}' "$STATE")
    BEFORE=$(awk -F= '/^SERIAL=/{print $2}' "$STATE")
    AFTER=$(volume_serial)
    if [ -z "$AFTER" ]; then
      echo "ERROR：读不到 $DRIVE: 的当前卷序列号" >&2
      echo "VERDICT=ERROR"
      exit 2
    fi
    PRESENT=$(sentinel_exists "$NAME")

    changed=0
    [ "$BEFORE" != "$AFTER" ] && changed=1
    gone=0
    [ "$PRESENT" = "0" ] && gone=1

    echo "DRIVE=$DRIVE"
    echo "SENTINEL=$NAME"
    echo "SENTINEL_PRESENT=$PRESENT"
    echo "SENTINEL_GONE=$gone"
    echo "SERIAL_BEFORE=$BEFORE"
    echo "SERIAL_AFTER=$AFTER"
    echo "SERIAL_CHANGED=$changed"

    if [ "$gone" = "1" ] && [ "$changed" = "1" ]; then
      echo "VERDICT=PASS"
      exit 0
    elif [ "$gone" = "0" ] && [ "$changed" = "0" ]; then
      echo "VERDICT=FAIL"
      echo "  原因：哨兵还在（$DRIVE:\\${NAME}）、序列号也没变 —— 目标卷根本没被重写" >&2
      exit 1
    else
      echo "VERDICT=WARN"
      [ "$gone" = "0" ] && echo "  未满足：哨兵文件仍在 $DRIVE:\\$NAME" >&2
      [ "$changed" = "0" ] && echo "  未满足：卷序列号未变化（${BEFORE}）" >&2
      echo "  单条弱信号不足以验收：可能只删了文件、或只换了文件系统记录" >&2
      exit 1
    fi
    ;;

  status)
    if [ -f "$STATE" ]; then
      echo "== $DRIVE: 的布哨状态 =="
      cat "$STATE"
      echo "当前序列号：$(volume_serial)"
    else
      echo "没有 $DRIVE: 的布哨记录"
    fi
    ;;

  *)
    echo "未知子命令：${CMD}（arm | verify | status）" >&2
    exit 2
    ;;
esac
