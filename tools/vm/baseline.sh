#!/usr/bin/env bash
# baseline.sh —— 测试 VM 的「防弹窗基线」检查 / 修复（幂等）
#
# 为什么需要它（2026-10-27/28 两轮事故的落点）：
#   用户看到的「位置不可用 / 无法访问 V:\」根因是 **explorer.exe 在登录时还原上次
#   打开的文件夹窗口**——而 Windows 默认就开着这个功能（`PersistBrowsers` 值不存在
#   = 默认开）。只要有人曾经在某个临时卷（测试挂的 VHD、第二系统分区…）上开过窗口，
#   那个卷消失后，每次登录都会把它还原成一个报错窗口。
#
#   10-28 我是在**这台机器上手工**改的三条注册表把它掐掉的。手工改的东西有两个问题：
#     ① 重建 VM / 换测试机就没了，弹窗必然复发（用户已经报过两次）；
#     ② 没人知道「它应该是这个值」——下一个人看到 `PersistBrowsers=0` 可能以为是误改
#        又给改回去。
#   所以把它固化成一条可执行、可校验、幂等的命令。
#
# 用法：
#   tools/vm/baseline.sh check     # 只检查，输出结构化结果（门禁用这个）
#   tools/vm/baseline.sh ensure    # 检查并修复（手工用这个）
#
# 输出（结构化，供门禁 grep；**不输出依赖中文的判断**）：
#   BASELINE=pass
#   BASELINE=fail key=<名> want=<值> got=<值>
#   BASELINE=skip reason=vm_not_running
#   BASELINE=error reason=...          # 仪器故障（读不到注册表），不是「不满足」
#
# 退出码：0 = pass / 1 = fail / 2 = ERROR（仪器故障，别当成不满足）/ 3 = SKIP
set -uo pipefail

VM="${BR_VM_NAME:-Windows 11}"

# ★ 必须按**本脚本要用的那台 VM 名**查状态：早先写成 `prlctl list -a | grep running`，
#   于是在 `BR_VM_NAME` 指向不存在的机器时，只要有**别的** VM 在跑就照样往下走，
#   最后报成 `error user_sid_not_found`——把「这台机器不在」误报成「仪器故障」。
if ! prlctl list "$VM" -o status --no-header 2>/dev/null | grep -qi '^running'; then
    echo "BASELINE=skip reason=vm_not_running name=$VM"
    exit 3
fi

vm() { prlctl exec "$VM" cmd /d /c "$1" 2>/dev/null | LC_ALL=C tr -cd '\11\12\40-\176'; }

# 用户 SID：`prlctl exec` 的 HKCU 是盲区，只能走 HKU\<SID>（本项目老坑）。
sid=$(vm 'reg query HKU' | tr -d '\r' | grep -oE 'S-1-5-21-[0-9-]+' | grep -v '_Classes' | head -1)
if [ -z "$sid" ]; then
    echo "BASELINE=error reason=user_sid_not_found"
    exit 2
fi

MODE="${1:-check}"
fail=0

# 读一个 DWORD 值：输出 `0x…` / `ABSENT` / `__ERROR__`。
#
# ★ 为什么查**整个键**而不是 `reg query <键> /v <名>`（2026-10-28 当场踩到）：
#   单值查询返回空有两种完全不同的原因——「这个值真不存在」和「这条命令根本没执行」
#   （`prlctl exec` 约 1/8 调用不执行，本项目老坑）。只查单值就必然把后者误报成
#   `got=ABSENT`：我自己的检查器刚上线十分钟就把正确的机器判成「基线不成立」。
#   而**整键查询的输出不可能合法为空**（键存在就至少打印键名）→ 空只可能是仪器没工作。
read_value() { # $1=键 $2=值名
    local i=0 text=""
    while [ $i -lt 3 ]; do
        i=$((i + 1))
        text=$(vm "reg query $1")
        [ -n "$text" ] && break
        sleep 2
    done
    if [ -z "$text" ]; then
        printf '%s' "__ERROR__"
        return
    fi
    printf '%s\n' "$text" | tr -d '\r' | awk -v n="$2" '
        $1 == n && $2 == "REG_DWORD" { print $NF; found = 1 }
        END { if (!found) print "ABSENT" }' | head -1
}
hex_to_dec() { printf '%d' "$1" 2>/dev/null || true; }

check_one() { # $1=说明 $2=键 $3=值名 $4=期望十进制
    local got got_dec
    got=$(read_value "$2" "$3")
    if [ "$got" = "__ERROR__" ]; then
        echo "BASELINE=error reason=registry_unreadable key=$3"
        return 2
    fi
    if [ "$got" = "ABSENT" ]; then
        echo "BASELINE=fail key=$3 want=$4 got=ABSENT"
        return 1
    fi
    got_dec=$(hex_to_dec "$got")
    if [ -z "$got_dec" ]; then
        echo "BASELINE=error reason=unparsable_value key=$3 raw=$got"
        return 2
    fi
    if [ "$got_dec" != "$4" ]; then
        echo "BASELINE=fail key=$3 want=$4 got=$got_dec"
        return 1
    fi
    return 0
}

ensure_one() { # $1=key $2=name $3=十进制
    vm "reg add $1 /v $2 /t REG_DWORD /d $3 /f" >/dev/null
}

ADV="HKU\\$sid\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"
POL="HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\Explorer"

# ① 登录不再还原文件夹窗口 —— 这是**掐掉复发机制**的那一条
# ②③ 所有驱动器类型 / 所有盘符禁用自动播放与自动运行 —— 堵掉「卷出现就开窗口」
CHECK_LIST=(
    "$ADV|PersistBrowsers|0"
    "$POL|NoDriveTypeAutoRun|255"
    "$POL|NoDriveAutoRun|4294967295"
)

# 跑一轮检查：打印每个不成立的项，返回 0/1/2（2 = 仪器故障）。
# 抽成函数是为了让 `ensure` **能真的复核**——早先版本写完直接打印 "repaired" 就退出，
# 而 `ensure_one` 的 `reg add` 同样会被「1/8 命令没跑」打中 → 谎报修好。
run_checks() {
    local rc=0 c item key name want
    for item in "${CHECK_LIST[@]}"; do
        IFS='|' read -r key name want <<<"$item"
        check_one "$name" "$key" "$name" "$want"
        c=$?
        [ "$c" = "2" ] && rc=2
        if [ "$c" = "1" ] && [ "$rc" = "0" ]; then rc=1; fi
    done
    return "$rc"
}

rc=0
run_checks || rc=$?

if [ "$rc" = "0" ]; then
    echo "BASELINE=pass"
    exit 0
fi

if [ "$MODE" = "ensure" ]; then
    for item in "${CHECK_LIST[@]}"; do
        IFS='|' read -r key name want <<<"$item"
        ensure_one "$key" "$name" "$want"
    done
    # ★ 必须复核：写下去 ≠ 写成功。`reg add` 也会被「命令没跑」吃掉，
    #   不复查就会理直气壮地说「已修复」，而用户下次登录照样弹窗。
    echo "--- 复核 ---"
    rc2=0
    run_checks || rc2=$?
    if [ "$rc2" = "0" ]; then
        echo "BASELINE=repaired（复核通过）"
        exit 0
    fi
    echo "BASELINE=repair_failed（写回后复核仍不成立；重跑 ensure 或查上面 got 值）"
    exit 1
fi

if [ "$rc" = "1" ]; then
    echo "BASELINE=fail（修复：tools/vm/baseline.sh ensure）"
    exit 1
fi
exit 2
