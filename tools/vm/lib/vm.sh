#!/bin/bash
# tools/vm/lib/vm.sh —— 操控测试虚拟机的通道函数。只定义常量与函数，不产生任何副作用。
# 实机用例脚本不要直接引入它：一律引入同目录的 common.sh（它先引入本文件，再叠加用例层的公共函数）。
#
# 为什么有这个文件（2026-11-25）：这些函数原本只存在于被忽略的 .test-artifacts/ 目录里。
# 「改动-复验对应表」要求的实机项（守卫竞态、命令行探测冒烟、暂停……）因此没有可以重跑的脚本，
# 换台机器或清了目录就没了。收进仓库后，每个用例脚本自己能跑、自己给结论。
#
# 四条约定（都实测踩过）：
#   1. 每条命令都必须带回显标记才算执行过——`prlctl exec` 约 1/8 的调用会静默不执行，也会间歇性报
#      PrlJob_GetResult 错误。所以每个函数都重试，由调用方核对标记（`VM_DONE=` / `__END__` / `LAUNCHED`）。
#   2. 处理客体输出一律按字节（`LC_ALL=C`）：否则 tr / grep 遇到中文系统的输出会报 Illegal byte sequence，
#      把读数截断成「没有输出」。`vrun` 的输出只保留可打印字符——**它只能用来读哨兵和结构化字段**；
#      要读原文一律用 common.sh 的 `fetch`（经共享盘原样拷出）。
#   3. 一律用共享盘的 UNC 路径 `\\Mac\backupRestore\…`：盘符 `X:` 的映射不跨重启，而且系统身份通道里看不到它。
#   4. 进 `prlctl` 的命令串一律 ASCII；判据只认结构化字段。

VM=${BR_VM:-Windows 11}
PRL="/Applications/Parallels Desktop.app/Contents/MacOS/prlctl"
SHARE='\\Mac\backupRestore'                        # Parallels 共享名；单引号里才是真正的 UNC（开头两个反斜杠，其余单个）
SHARE_PS='\\Mac\backupRestore\tools\gui-check\ps'  # 界面辅助脚本（系统身份经共享盘直接执行）
SHARE_VM_PS='\\Mac\backupRestore\tools\vm\ps'      # 虚拟机侧的包装脚本
GUI_DIR='H:\brwork\gui-check'                      # 常驻提权助手从中运行脚本的目录
GUI_RUNAS='H:\brwork\gui-check\runas.ps1'

# 实机用例会用到的界面辅助脚本。stage_gui_helpers 按这份清单把它们拷进虚拟机，并逐个核对字节数。
GUI_HELPERS=(
  runas.ps1 helper.ps1 clickenv.ps1 launch.ps1 launch-nokill.ps1
  uiclick.ps1 uidlglist.ps1 comboitems.ps1 uitext2.ps1 uicombosel.ps1
  c5prep.ps1 c5check.ps1 helperkill.ps1 who.ps1
)

ascii() { LC_ALL=C tr -cd '\11\12\40-\176' | tr -d '\r'; }

# vrun "<cmd 命令体>" [次数]：以系统身份经 cmd 执行，必须带回显 VM_DONE 才算真跑过
vrun() {
    local n=${2:-4} out="" i
    for i in $(seq 1 "$n"); do
        out=$("$PRL" exec "$VM" cmd /d /v:on /c "$1 & echo VM_DONE=!ERRORLEVEL!" 2>/dev/null | ascii)
        printf '%s' "$out" | grep -q "VM_DONE=" && break
        sleep 3
    done
    printf '%s\n' "$out"
    printf '%s' "$out" | grep -q "VM_DONE="
}

# vrun_user：同上，但走用户会话（共享盘 X: 的映射、用户桌面里的东西只在这里可见）
vrun_user() {
    local n=${2:-4} out="" i
    for i in $(seq 1 "$n"); do
        out=$("$PRL" exec "$VM" --current-user cmd /d /v:on /c "$1 & echo VM_DONE=!ERRORLEVEL!" 2>/dev/null | ascii)
        printf '%s' "$out" | grep -q "VM_DONE=" && break
        sleep 3
    done
    printf '%s\n' "$out"
    printf '%s' "$out" | grep -q "VM_DONE="
}

# vps <脚本名.ps1> [user]：以系统身份执行 tools/gui-check/ps/ 下的脚本，必须收到结尾哨兵 __END__
vps() {
    local n=${2:-4} out="" i who=""
    [ "${3:-}" = user ] && who="--current-user"
    for i in $(seq 1 "$n"); do
        out=$("$PRL" exec "$VM" $who powershell -NoProfile -ExecutionPolicy Bypass -File "$SHARE_PS\\$1" 2>/dev/null | LC_ALL=C tr -d '\r')
        printf '%s' "$out" | grep -q "^__END__" && break
        sleep 3
    done
    printf '%s\n' "$out"
    printf '%s' "$out" | grep -q "^__END__"
}

# vgbk "<命令>"：执行输出为中文编码的命令（bcdedit / reagentc），转成通用编码后显示
vgbk() {
    local n=${2:-3} out="" i
    for i in $(seq 1 "$n"); do
        out=$("$PRL" exec "$VM" cmd /d /c "$1" 2>/dev/null | iconv -f GBK -t UTF-8 2>/dev/null | LC_ALL=C tr -d '\r')
        [ -n "$(printf '%s' "$out" | tr -d ' \t\n')" ] && break
        sleep 3
    done
    printf '%s\n' "$out"
}

vm_state() { "$PRL" list -a 2>/dev/null | grep -i "$VM" | awk '{print $2}'; }

# stage_gui_helpers：把 tools/gui-check/ps/ 里的辅助脚本拷进常驻助手的运行目录，并逐个核对字节数。
# 判据是「客体里的字节数 == 宿主里的字节数」：robocopy / copy 报成功不代表拷对了（旧文件没被覆盖过）。
stage_gui_helpers() {
    local out="" i f want have bad=""
    local ps_dir
    ps_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../gui-check/ps" && pwd)"
    for i in 1 2 3; do
        out=$("$PRL" exec "$VM" powershell -NoProfile -ExecutionPolicy Bypass -File "$SHARE_VM_PS\\stage-gui-helpers.ps1" "${GUI_HELPERS[@]}" 2>/dev/null | tr -d '\r')
        printf '%s' "$out" | grep -a -q '^__END__' && break
        sleep 2
    done
    printf '%s' "$out" | grep -a -q '^__END__' || { echo "STAGE=FAILED reason=no_end_marker"; return 1; }
    for f in "${GUI_HELPERS[@]}"; do
        want=$(wc -c < "$ps_dir/$f" | tr -d ' ')
        have=$(printf '%s\n' "$out" | sed -n "s/^STAGED $f bytes=\([0-9]*\)$/\1/p" | head -1)
        [ "$want" = "$have" ] || bad="$bad $f(want=$want got=${have:-none})"
    done
    if [ -n "$bad" ]; then echo "STAGE=MISMATCH$bad"; return 1; fi
    echo "STAGE=OK count=${#GUI_HELPERS[@]}"
}

# elev <脚本名.ps1> [参数...]：经常驻提权助手在用户桌面会话里执行 GUI_DIR 下的脚本。
# ★ 助手回传的控件文字是系统本地编码——要读中文先用 common.sh 的 decode_text 解码。
# ★ 返回值整体可能是空的：调用方要核对自己关心的键值行（CLICK= / WINDOWS= / __END__）。
# ★ 只在「一点输出都没有」时重试：那说明命令根本没执行（prlctl exec 约 1/8 的调用静默不执行）。
#   有输出但没有结尾哨兵 = 执行了、输出被截断——**不能重试**，点击可能已经发生，重点一次会把复选框翻回去
#   （2026-11-25 实测：点「创建任务」那一步通道静默，脚本干等 40 秒没等到确认框）。
elev() {
    local name="$1"; shift
    local extra="$*" out="" i
    for i in 1 2 3; do
        if [ -n "$extra" ]; then
            out=$("$PRL" exec "$VM" powershell -NoProfile -ExecutionPolicy Bypass -File "$GUI_RUNAS" -Name "$name" -Extra "$extra" 2>/dev/null | LC_ALL=C tr -d '\r')
        else
            out=$("$PRL" exec "$VM" powershell -NoProfile -ExecutionPolicy Bypass -File "$GUI_RUNAS" -Name "$name" 2>/dev/null | LC_ALL=C tr -d '\r')
        fi
        [ -n "$(printf '%s' "$out" | tr -d '[:space:]')" ] && break
        sleep 2
    done
    printf '%s\n' "$out"
    printf '%s' "$out" | grep -q "__END__"
}

# ensure_helper：重启后提权常驻助手会起在没有桌面的「会话 0」。先全部结束，再从用户桌面会话启动，
# 并核对它确实在会话 1。输出 HELPER=… session=N；会话不是 1 则返回非零。
ensure_helper() {
    vps helperkill.ps1 2 >/dev/null
    local out="" i tag sess
    for i in 1 2 3 4; do
        out=$("$PRL" exec "$VM" --current-user powershell -NoProfile -ExecutionPolicy Bypass -File "$GUI_RUNAS" -Name clickenv.ps1 2>&1 | ascii)
        printf '%s' "$out" | grep -q "HELPER=resident" && break
        sleep 3
    done
    tag=$(printf '%s' "$out" | grep -o "HELPER=[a-z]* pid=[0-9]*" | tail -1)
    sess=$(vps who.ps1 2 | grep -a "^ROLE=helper" | grep -ao "session=[0-9]" | head -1)
    echo "${tag:-HELPER=<unreadable>} ${sess:-session=<unreadable>}"
    [ "$sess" = "session=1" ]
}

# wait_windows [上限秒]：重启后轮询。先等到「命令通道断开」（确实在重启），再等到「通道恢复」。
wait_windows() {
    local limit=${1:-360} t=0
    while [ "$t" -lt 120 ]; do
        if out=$(vrun "echo ALIVE" 1) && printf '%s' "$out" | grep -q ALIVE; then sleep 10; t=$((t+10)); else echo "VM_DOWN_OBSERVED after ${t}s"; break; fi
    done
    t=0
    while [ "$t" -lt "$limit" ]; do
        if out=$(vrun "echo ALIVE" 1) && printf '%s' "$out" | grep -q ALIVE; then echo "WINDOWS_BACK after ${t}s"; return 0; fi
        sleep 15; t=$((t+15))
    done
    echo "WINDOWS_NOT_BACK after ${t}s"; return 1
}
