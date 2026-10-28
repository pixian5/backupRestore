#!/usr/bin/env bash
# baseline.sh —— 测试 VM 的「环境基线」检查 / 修复（幂等，清单式）
#
# 为什么需要它：
#   这台机器上有一批**长得像惯例、其实是前提**的东西：
#     · 「登录不还原窗口 + 关自动播放」三条注册表 —— 用户已为 V 盘弹窗报过两次；
#     · 两个验收夹具目录必须各只放**一个**任务 —— 否则 `boot_entry_fault` /
#       `secondary_menu_repair` 会「读到对方的夹具」而失败，症状看起来像产品坏了。
#   这些值此前全靠人记在对话里/文档里：重建 VM 就丢，换个人就不知道「本该是什么」。
#   本脚本把它们变成一条可执行、可校验、幂等的命令。
#
# 用法：
#   tools/vm/baseline.sh check     # 只检查，输出结构化结果（门禁 STEP 0 用这个）
#   tools/vm/baseline.sh ensure    # 检查并修复**可修复项**（注册表），再复核
#
# 输出（结构化，供 grep；**不输出依赖中文的判断**）：
#   ITEM=<id> status=pass
#   ITEM=<id> status=fail want=<期望> got=<实测> [remanual=1]
#   ITEM=<id> status=error reason=<...>
#   BASELINE=pass | fail | repaired（复核通过） | repair_failed
#   BASELINE=skip reason=vm_not_running name=<VM>
#   BASELINE=error reason=<...>
#
# 退出码：0 = pass；1 = fail；2 = ERROR（仪器故障，**不是**「不满足」）；3 = SKIP
set -uo pipefail

VM="${BR_VM_NAME:-Windows 11}"

# ★ 必须按**本脚本要用的那台 VM 名**查状态：早先写 `prlctl list -a | grep running`，
#   于是在 `BR_VM_NAME` 指向不存在的机器时，只要有别的 VM 在跑就照样往下走，
#   最后报成 `error user_sid_not_found`——把「这台机器不在」误报成「仪器故障」。
if ! prlctl list "$VM" -o status --no-header 2>/dev/null | grep -qi '^running'; then
    echo "BASELINE=skip reason=vm_not_running name=$VM"
    exit 3
fi

vm() { prlctl exec "$VM" cmd /d /c "$1" 2>/dev/null | LC_ALL=C tr -cd '\11\12\40-\176'; }

# 读一次查询，空就重试。★ `prlctl exec` 约 1/8 调用根本不执行命令，
# 而「空输出」既可能是「真没有」也可能是「命令没跑」——所以调用方必须有能力区分。
vm_read() {
    local i=0 out=""
    while [ $i -lt 3 ]; do
        i=$((i + 1))
        out=$(vm "$1")
        [ -n "$out" ] && break
        sleep 2
    done
    printf '%s' "$out"
}

MODE="${1:-check}"

# ---------- 用户 SID（注册表项要用）----------
# `prlctl exec` 的 HKCU 是盲区，只能走 HKU\<SID>（本项目老坑）。
sid=$(vm_read 'reg query HKU' | grep -oE 'S-1-5-21-[0-9-]+' | grep -v '_Classes' | head -1)
if [ -z "$sid" ]; then
    echo "BASELINE=error reason=user_sid_not_found"
    exit 2
fi

ADV="HKU\\$sid\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"
POL="HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\Explorer"

# ---------- 读值原语 ----------
# ★ 为什么查**整个键**而不是 `reg query <键> /v <名>`（2026-10-28 当场踩到）：
#   单值查询返回空有两种完全不同的原因——「这个值真不存在」和「这条命令根本没执行」。
#   只查单值就必然把后者误报成 `got=ABSENT`（我自己的检查器刚上线十分钟就把一台
#   明明正确的机器判成「基线不成立」）。整键查询的输出**不可能合法为空**（键存在就
#   至少打印键名）→ 空只可能是仪器没工作。
read_reg_value() { # $1=键 $2=值名 → 0x… / ABSENT / __ERROR__
    local text
    text=$(vm_read "reg query $1")
    if [ -z "$text" ]; then
        printf '%s' "__ERROR__"
        return
    fi
    printf '%s\n' "$text" | tr -d '\r' | awk -v n="$2" '
        $1 == n && $2 == "REG_DWORD" { print $NF; found = 1 }
        END { if (!found) print "ABSENT" }' | head -1
}

# 数一个目录下的**子目录**个数：输出数字 / MISSING / __ERROR__。
# ★ 用 BEGIN/END 哨兵包住：`dir /b` 在空目录时输出合法为空，与「命令没跑」分不开；
#   包上哨兵后，只有「连 BEGIN/END 都不全」才是仪器故障。
#   顺带避开 `find /c /v ""`——`prlctl exec` 传 `""` 会偶发 `PrlJob_GetRetCode: Invalid argument`。
count_subdirs() { # $1=路径
    local text
    text=$(vm_read "if exist $1 (echo BEGIN & dir /b /ad $1 & echo END) else (echo MISSING)")
    if [ -z "$text" ]; then
        printf '%s' "__ERROR__"
        return
    fi
    text=$(printf '%s\n' "$text" | tr -d '\r')
    # ★ 必须去掉行尾空白：`echo BEGIN &` 在 cmd 里输出的是 `BEGIN `（带一个尾空格），
    #   而下面按 `^BEGIN$` 匹配 —— 少了这一步，哨兵永远认不出来，每一项都报
    #   `dir_unreadable`（第一次跑就撞上，看着像目录读不到，其实是格式细节）。
    text=$(printf '%s\n' "$text" | sed 's/[[:space:]]*$//')
    if printf '%s\n' "$text" | grep -q '^MISSING$'; then
        printf '%s' "MISSING"
        return
    fi
    if ! printf '%s\n' "$text" | grep -q '^BEGIN$' || ! printf '%s\n' "$text" | grep -q '^END$'; then
        printf '%s' "__ERROR__"
        return
    fi
    printf '%s\n' "$text" | awk '
        /^BEGIN$/ { inside = 1; next }
        /^END$/   { inside = 0; next }
        inside && NF { n++ }
        END { print n + 0 }'
}

hex_to_dec() { printf '%d' "$1" 2>/dev/null || true; }

# 一个路径是否存在：输出 YES / NO / __ERROR__。
# ★ 两个分支都会打印东西（YES/NO），所以「空输出」只可能是命令没跑 → 无歧义。
#   对比 `dir` 那种「空目录时合法为空」的查询，这里不需要哨兵。
path_exists() { # $1=路径
    local text
    text=$(vm_read "if exist $1 (echo YES) else (echo NO)")
    if [ -z "$text" ]; then
        printf '%s' "__ERROR__"
        return
    fi
    text=$(printf '%s\n' "$text" | tr -d '\r' | sed 's/[[:space:]]*$//')
    case "$text" in
        *YES*) printf '%s' "YES" ;;
        *NO*)  printf '%s' "NO" ;;
        *)     printf '%s' "__ERROR__" ;;
    esac
}

# ---------- 清单 ----------
# 每项：id|kind|参数…
#   reg       : <键>|<值名>|<期望十进制>   可修复（ensure 写回）
#   dir_exact : <路径>|<期望子目录数>      **不修复**（删别人的任务目录是破坏性动作，只报告）
ITEMS=(
    # ① 登录不再还原文件夹窗口 —— 掐掉 V 盘弹窗复发机制的那一条
    "reg_persist_browsers|reg|$ADV|PersistBrowsers|0"
    # ②③ 所有驱动器类型 / 所有盘符禁用自动播放与自动运行
    "reg_nodrivetypeautorun|reg|$POL|NoDriveTypeAutoRun|255"
    "reg_nodriveautorun|reg|$POL|NoDriveAutoRun|4294967295"
    # ④⑤ 两条验收用例的夹具根：tasks/ 下**只能有一个**已完成任务
    "fixture_boot_fault|dir_exact|H:\\br-accept\\tasks|1"
    "fixture_menu_repair|dir_exact|H:\\br-accept2\\tasks|1"
    # ⑥ `S:` 必须真的能按这个盘符摸到**引导存储**
    #    项目里大量脚本硬编码 `S:\`（`bcdedit /store S:\EFI\Microsoft\Boot\BCD`、
    #    冒烟脚本、PE 交接……）。一旦 S: 被别的卷占了或 ESP 换了盘符，
    #    `bcdedit` 会把**不相干卷上的文件**当成引导存储来改——症状是「引导被改坏了
    #    但看起来一切正常」，比弹窗难查得多。
    #    ★ 判据选「BCD 文件在不在」，而不是「分区类型是不是 ESP」：前者才是脚本真正
    #      需要的东西，后者只是它的代理；而且 `if exist` 用 cmd 就能问，避开在
    #      prlctl 里跑 PowerShell 的引号/编码坑。
    "esp_bcd_on_s|must_exist|S:\\EFI\\Microsoft\\Boot\\BCD"
)

check_item() { # $1=id $2=kind，其余为参数；返回 0/1/2
    local id=$1 kind=$2
    shift 2
    if [ "$kind" = "reg" ]; then
        local key=$1 name=$2 want=$3 got got_dec
        got=$(read_reg_value "$key" "$name")
        case "$got" in
            __ERROR__) echo "ITEM=$id status=error reason=registry_unreadable"; return 2 ;;
            ABSENT)    echo "ITEM=$id status=fail want=$want got=ABSENT";        return 1 ;;
        esac
        got_dec=$(hex_to_dec "$got")
        if [ -z "$got_dec" ]; then
            echo "ITEM=$id status=error reason=unparsable_value raw=$got"
            return 2
        fi
        if [ "$got_dec" != "$want" ]; then
            echo "ITEM=$id status=fail want=$want got=$got_dec"
            return 1
        fi
        echo "ITEM=$id status=pass"
        return 0
    fi
    if [ "$kind" = "dir_exact" ]; then
        local path=$1 want=$2 got
        got=$(count_subdirs "$path")
        if [ "$got" = "__ERROR__" ]; then
            echo "ITEM=$id status=error reason=dir_unreadable path=$path"
            return 2
        fi
        if [ "$got" != "$want" ]; then
            # 不可自动修复：清别人的任务目录属于破坏性动作，只报告、交给人决定。
            echo "ITEM=$id status=fail want=$want got=$got remanual=1"
            return 1
        fi
        echo "ITEM=$id status=pass"
        return 0
    fi
    if [ "$kind" = "must_exist" ]; then
        local path=$1 got
        got=$(path_exists "$path")
        if [ "$got" = "__ERROR__" ]; then
            echo "ITEM=$id status=error reason=path_unreadable path=$path"
            return 2
        fi
        if [ "$got" != "YES" ]; then
            # 不可自动修复：把 ESP 重新挂到 S: 属于改机器挂载状态的动作，
            # 该由人来判断（而且产品侧走的是带租约的挂载入口，不该在这儿另开一条路）。
            echo "ITEM=$id status=fail want=present got=ABSENT path=$path remanual=1"
            return 1
        fi
        echo "ITEM=$id status=pass"
        return 0
    fi
    echo "ITEM=$id status=error reason=unknown_kind kind=$kind"
    return 2
}

run_checks() {
    local rc=0 c item id kind params
    for item in "${ITEMS[@]}"; do
        IFS='|' read -r id kind params <<<"$item"
        # 参数本来就是 `|` 分隔的，转成空格后交给 check_item（键名/路径里没有空格）。
        # shellcheck disable=SC2086
        check_item "$id" "$kind" $(printf '%s' "$params" | tr '|' ' ')
        c=$?
        case "$c" in
            2) rc=2 ;;
            1) if [ "$rc" = "0" ]; then rc=1; fi ;;
        esac
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
    # 只修**可修复项**（注册表三条）；目录类只报告，不动手。
    for item in "${ITEMS[@]}"; do
        IFS='|' read -r id kind params <<<"$item"
        [ "$kind" = "reg" ] || continue
        key=$(printf '%s' "$params" | cut -d'|' -f1)
        name=$(printf '%s' "$params" | cut -d'|' -f2)
        want=$(printf '%s' "$params" | cut -d'|' -f3)
        vm "reg add $key /v $name /t REG_DWORD /d $want /f" >/dev/null
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
    echo "BASELINE=repair_failed（注册表已尝试写回；剩余项见上，标了 remanual=1 的需人工）"
    exit 1
fi

if [ "$rc" = "1" ]; then
    echo "BASELINE=fail（注册表类可修：tools/vm/baseline.sh ensure；标了 remanual=1 的项需人工）"
    exit 1
fi
echo "BASELINE=error reason=instrument"
exit 2
