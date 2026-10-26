#!/usr/bin/env bash
# run-body.sh —— 在 VM 会话 1（借 GUI 令牌提权）里跑一个 body 脚本，并把结果文件打回 stdout。
#
# 为什么需要它（2026-10-27 05:56 的教训）：
#   手工流程是「写钩子 → 启动 → 读文件」三步，其中**钩子过期**不会报错，
#   会安静地跑上一次的脚本，于是结果文件根本不会生成（或被旧内容骗）。
#   那天为此白查了两小时：先误判「窗口站/桌面差异」，实际是 ① 钩子指向旧脚本
#   ② 驱动脚本 C# 里写了 `[ref]` 导致 Add-Type 编译失败、症状同样是「找不到窗口」。
#   这里把四件事固定下来：钩子回读核对、先删旧结果、启动、轮询到 DONE。
#
# 用法：
#   tools/smoke/run-body.sh _t-task-admin-layout.ps1 H:\brwork\task-admin-layout-driver.txt
#   tools/smoke/run-body.sh _diag-enum-ctx.ps1 H:\brwork\diag-enum-ctx.txt 30
#
# 三态（与项目判据纪律一致）：
#   0 = 读到 DONE；1 = 跑完但没 DONE（结果文件有内容，交给调用方判）；
#   2 = 仪器没工作（GUI 不在 / 钩子没落地 / 文件始终为空）——**这不是「判据不满足」**。
set -uo pipefail

VM="${BR_VM_NAME:-Windows 11}"
UNC_BODY='\\Mac\Home\code\backupRestore\tools\win-clicker'
UNC_LAUNCH='\\Mac\Home\code\backupRestore\tools\run-gui-token.ps1'
HOOK='H:\brwork\gui-token-body.txt'

BODY="${1:?用法: run-body.sh <body.ps1> <结果文件:...txt> [超时秒]}"
OUT="${2:?缺结果文件路径（客体路径，如 H:\\brwork\\x.txt）}"
TIMEOUT="${3:-25}"

vm() { prlctl exec "$VM" cmd /d /c "$1" 2>/dev/null | LC_ALL=C tr -cd '\11\12\40-\176'; }
say() { printf '%s\n' "$*"; }

# ---------- #1 GUI 必须活着，否则一切免谈 ----------
if [ -z "$(vm 'tasklist /fi "imagename eq BackupRestore.exe" /fo csv /nh')" ]; then
    say "ERROR: GUI_NOT_RUNNING（会话 1 里没有 BackupRestore.exe，先 br-s1.sh start-gui.ps1）"
    exit 2
fi

# ---------- #2 写钩子并回读核对（防「过期钩子静默跑错脚本」）----------
# 用 cmd 的 echo 而不是 zsh 的：zsh 会把 \\Mac 之类解释掉，钩子内容被截断且不报错。
vm "echo ${UNC_BODY}\\${BODY}>${HOOK}" >/dev/null
hooked="$(vm "type ${HOOK}" | tr -d '\r' | head -1)"
want="${UNC_BODY}\\${BODY}"
if [ "$hooked" != "$want" ]; then
    say "ERROR: HOOK_MISMATCH（写进去的与实际读到的不一致，这是仪器故障）"
    say "  want=${want}"
    say "  got =${hooked}"
    exit 2
fi
say "HOOK_OK ${BODY}"

# ---------- #3 先删旧结果（脚本挂死时 type 会读回上一次的文件）----------
vm "del /f /q ${OUT}" >/dev/null

# ---------- #4 启动 ----------
vm "powershell -NoProfile -ExecutionPolicy Bypass -Command \"iex (Get-Content -Raw -Encoding UTF8 ${UNC_LAUNCH})\"" >/dev/null

# ---------- #5 轮询到 DONE ----------
text=""; w=0
while [ "$w" -lt "$TIMEOUT" ]; do
    sleep 2; w=$((w + 2))
    text="$(vm "type ${OUT}")"
    if printf '%s' "$text" | tr -d '\r' | grep -qx 'DONE'; then break; fi
    if printf '%s' "$text" | tr -d '\r' | grep -q '^DONE$'; then break; fi
done

if [ -z "$(printf '%s' "$text" | tr -d '[:space:]')" ]; then
    say "ERROR: RESULT_EMPTY（等了 ${TIMEOUT}s 结果文件仍不存在/为空——仪器故障，不是判据不满足）"
    say "  提示：启动日志 H:\\brwork\\gui-token-launch.txt 里有本次实际执行的 cmd="
    exit 2
fi

printf '%s\n' "$text" | tr -d '\r'
if printf '%s' "$text" | tr -d '\r' | grep -q '^DONE$'; then exit 0; fi
say "NOTE: 结果文件没有 DONE 行（可能中途异常退出）"
exit 1
