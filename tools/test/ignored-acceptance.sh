#!/usr/bin/env bash
# 一键跑「被 #[ignore] 标记的验收测试」，三态 PASS / FAIL / SKIP。
#
# 为什么需要它：
#   在 mac 上 `cargo test` 只看到 1 个 ignored——其余都在 `cfg(windows)` 文件里，
#   编译器根本不打开（「零信息」老坑在安全判据上的表现）。这些测试里有几条直接
#   守破坏性安全（不往源卷写、改启动菜单能回滚），此前「只在有人记得手跑时才跑」。
#
# 判据纪律（本脚本的设计前提）：
#   1. 三态，SKIP **必须写出缺什么**，不允许静默跳过；
#   2. 一条都没跑成 → 退出码 2（仪器没工作 ≠ 没问题）；
#   3. 只认 ASCII 结构化字段（`test result: ok. 1 passed`），绝不 grep 中文文案；
#   4. 每次跑之前先删旧日志——脚本挂死时 `type` 会读回上一次的结果，
#      看着像「稳定复现」（本项目栽过）。
#
# 通道说明（这是本脚本最容易踩的地方）：
#   - 编译要用用户会话（`--current-user`）：源码在 X:，那是用户会话里的共享映射，
#     SYSTEM 账户看不到 X:；
#   - 运行要用 SYSTEM（`prlctl exec` 默认）：创建 VHD / DISM / diskpart 需要管理员，
#     而 `--current-user` 是标准 token，会被静默拒绝。
#   所以先「用户会话编译 → 复制 exe 到固定路径 → SYSTEM 运行」。
#
# 用法：
#   tools/test/ignored-acceptance.sh              # 全部
#   tools/test/ignored-acceptance.sh --list       # 只列用例与前提
#   tools/test/ignored-acceptance.sh --only NAME  # 只跑一条
set -u

VM_NAME="${BR_VM_NAME:-Windows 11}"
SRC_WIN='X:\'
TARGET_WIN='H:\br-target'
BUILD_LOG='H:\br-test-build.log'
BIN_DIR='H:\br-ignored'
BIN_WIN="$BIN_DIR\\cli-tests.exe"

# #3 需要 `../../.test-artifacts`（cargo test 默认 cwd 是 crate 目录）：
# 在 H: 上复刻同样的两级结构，exe 以绝对路径运行、cwd 设为 crate 目录。
REPLAY_ROOT='H:\igt'
REPLAY_CWD="$REPLAY_ROOT\\crates\\backuprestore-cli"
REPLAY_LOG="$REPLAY_ROOT\\.test-artifacts\\e2e-brimglog-227.log"

vm() {
    prlctl exec "$VM_NAME" cmd /d /c "$1" 2>/dev/null | LC_ALL=C tr -cd '\11\12\40-\176'
}
vm_user() {
    prlctl exec "$VM_NAME" --current-user cmd /d /c "$1" 2>/dev/null | LC_ALL=C tr -cd '\11\12\40-\176'
}

say() { printf '%s\n' "$*"; }

# ---------- 发一条 VM 命令并确认它**真的跑了** ----------
# ★ 2026-10-27/28 实测：约 1/8 的 `prlctl exec` 调用**根本不执行**命令（不是失败，
#   是没跑）。症状很有欺骗性——日志/结果文件连创建都没有，而脚本会一直等到超时，
#   再把「仪器没工作」误判成 FAIL。重启后的几十秒里这个比例明显更高（VM 忙）。
#   所以凡是「发出去 → 等它产出某个文件」的步骤，一律走这里。
# 参数：$1 通道(vm|vm_user)  $2 命令  $3 探测命令(输出非空=已生效)  $4 单次最多等秒数(默认30)
# 返回：0 = 已生效（同时把探测输出打到 stdout）；1 = 重试 3 次仍未生效
vm_launch_verified() {
    local channel=$1 cmd=$2 probe=$3 limit=${4:-30}
    local i=0 a got
    while [ $i -lt 3 ]; do
        i=$((i + 1))
        $channel "$cmd" >/dev/null 2>&1
        a=0; got=""
        while [ $a -lt "$limit" ]; do
            sleep 2; a=$((a + 2))
            got=$(vm "$probe")
            [ -n "$got" ] && break
        done
        if [ -n "$got" ]; then printf '%s' "$got"; return 0; fi
        say "  RELAUNCH=$i (command did not register)"
    done
    say "  RELAUNCH=exhausted"
    return 1
}

# 读一个**输出可能合法地为空**的查询（`dir` 查产物、`findstr` 查标记…）。
# ★ 这类查询没法只用「空不空」判断好坏：空可能是「真没有」，也可能是「命令没跑」
#   ——2026-10-28 实测就栽在这里：产物明明在（手工 `dir` 立刻列出来），门禁里那一次
#   `vm "dir /b /o-d …"` 返回空，于是判「编译未完成」。重试 3 次仍空才当成「真没有」。
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

# ---------- 用例表 ----------
# 每行：名字|filter|前提种类|前提参数
#   前提种类 none = 只需管理员；env = 需要一组环境变量；pe = 只能在 PE/RE 内跑
#
# ★ 2026-10-27 新增 secondary_mapping_rejections：原来只验「改配置」的成功路径，
#   需要真实第二系统分区（F: 已删）→ 永远 SKIP。它前面那三道守卫（身份不符 /
#   目标是当前系统盘 / 缺 hive）**纯拒绝、一个字节不写**，在现有机器上就能常态化跑。
#   这才是「最该自动化」的部分：改配置能不能成功，比「不该写的时候有没有写」次要得多。
CASES=(
  "no_write_on_reject|reject_without_write|none|"
  "online_capture_indexes|online_capture_append_keep|none|"
  "wimlib_log_replay|replay_real_wimlib_log|none|"
  "secondary_mapping_rejections|secondary_mapping_rejections|none|"
  "secondary_registry_map|offline_secondary_mapping|env|BR_SECONDARY_MAPPING_ACK BR_SECONDARY_MAPPING_LETTER BR_SECONDARY_MAPPING_GUID BR_SECONDARY_MAPPING_LOG"
  "winre_offline_reg|offline_registration_on_test_partition|env|BR_WINRE_TEST_ACK BR_WINRE_TEST_TARGET BR_WINRE_TEST_PRIMARY BR_WINRE_TEST_GUID BR_WINRE_TEST_EFI BR_WINRE_TEST_LOG"
  "secondary_menu_repair_rejections|secondary_menu_repair_rejections|none|"
  "secondary_menu_repair|secondary_menu_repair_native|env|BR_ACCEPTANCE_SNAPSHOT BR_ACCEPTANCE_ROOT BR_ACCEPTANCE_TASK BR_ACCEPTANCE_EFI BR_ACCEPTANCE_SECONDARY"
  "boot_entry_fault|missing_payload_conflict_and_cleanup_retry|env|BR_ACCEPTANCE_SNAPSHOT BR_BOOT_FAULT_ROOT"
  "boot_failure_rollback|boot_failure_rollback_in_pe|pe|"
)

if [ "${1:-}" = "--list" ]; then
    say "CASE_TABLE=${#CASES[@]}"
    for row in "${CASES[@]}"; do
        IFS='|' read -r name filter kind arg <<<"$row"
        say "  $name  filter=$filter  needs=$kind ${arg:-}"
    done
    exit 0
fi

ONLY=""
if [ "${1:-}" = "--only" ]; then ONLY="${2:-}"; fi

# ---------- 前置 0：测试 VM 的「防弹窗基线」----------
# ★ 2026-10-28：`PersistBrowsers=0` + 两条 AutoPlay 策略是**掐掉「位置不可用 /
#   无法访问 V:\」弹窗**的唯一手段（真因 = explorer 登录时还原上次的文件夹窗口，
#   而该值不存在时 Windows 默认就是开）。它们只存在于当前这台 VM 的注册表里——
#   重建/换机就会丢，弹窗必然复发（用户已经报过两次）。
#   ★ 这里只**检查**，不静默修改机器状态：改环境要显式敲 `tools/vm/baseline.sh ensure`。
say "STEP 0/5 检查测试 VM 环境基线（防弹窗 3 项 + 验收夹具根 2 项）"
bl_out=$(tools/vm/baseline.sh check 2>&1); bl_rc=$?
printf '%s\n' "$bl_out" | sed 's/^/  /'
case "$bl_rc" in
    0) : ;;
    3) say "  BASELINE_SKIP=vm_not_running（VM 没跑，后面本来也跑不了）" ;;
    1) say "ERROR: 基线不成立，弹窗会复发。修复：tools/vm/baseline.sh ensure"; exit 2 ;;
    *) say "ERROR: 基线判据自身故障（rc=$bl_rc）——这不是「通过」"; exit 2 ;;
esac

# ---------- 前置：编译（用户会话，因为 X: 只在用户会话里可见）----------
say "STEP 1/5 编译 Windows 测试目标（用户会话，源码在 X:）"
vm_user "del /f /q $BUILD_LOG" >/dev/null 2>&1
if ! vm_launch_verified vm_user \
        "cd /d $SRC_WIN && (cargo test -p backuprestore-cli --no-run --target-dir $TARGET_WIN > $BUILD_LOG 2>&1) & echo LAUNCHED" \
        "if exist $BUILD_LOG (echo 1)" 60 >/dev/null; then
    say "ERROR: 编译命令始终没有被 VM 执行（仪器故障，不是编译失败）。这不是「测试通过」。"
    exit 2
fi

# ★ 循环里把结果**存下来复用**，不要在循环外再读一次：
#   2026-10-27 实测出现过「循环里读到 Finished → 循环外重读为空 → 判成编译失败」
#   （同一文件两次读取结果不同，机制未查清）。读一次、判一次。
finished=""
waited=0
while [ $waited -lt 300 ]; do
    sleep 5; waited=$((waited + 5))
    finished=$(vm "findstr /c:\"Finished\" $BUILD_LOG")
    [ -n "$finished" ] && break
done

# 判据分离：编译是否成功看 build log 的 `Finished`；产物路径**不解析日志**，
# 直接取 deps 里最近修改的那个 exe（`dir /o-d` 降序取第一个）。
# ★ 不用正则抓日志里的括号路径：zsh 的 echo 会把 `\b` 当退格解释、BSD sed 对
#   行尾空白处理不一，两种都会静默取空——本轮为此白等过一个 300 秒超时。
exe_win=""
if [ -n "$finished" ]; then
    exe_name=$(vm_read "dir /b /o-d $TARGET_WIN\\debug\\deps\\backuprestore_cli-*.exe" | head -1)
    if [ -n "$exe_name" ]; then
        exe_win="$TARGET_WIN\\debug\\deps\\$exe_name"
    fi
fi
if [ -z "$exe_win" ]; then
    say "ERROR: 编译未完成，或 deps 里找不到测试产物。这不是「测试通过」。"
    vm "type $BUILD_LOG" | tail -12
    exit 2
fi
say "  BUILT=$exe_win"

vm "if not exist $BIN_DIR mkdir $BIN_DIR" >/dev/null 2>&1
# ★ 同 STEP 1 / run_one：核对动作本身也会被 `prlctl exec` 偶发「不执行」打中
#   （2026-10-28 实测：文件明明已经复制过去了，`if exist` 却返回空 → 判「复制失败」）。
if ! vm_launch_verified vm "copy /y \"$exe_win\" $BIN_WIN" \
        "if exist $BIN_WIN (echo 1)" 10 >/dev/null; then
    say "ERROR: 复制测试二进制到 $BIN_WIN 失败（这不是「测试通过」）。"
    exit 2
fi

# ---------- #3 的 fixture：复刻 ../../.test-artifacts 两级结构 ----------
say "STEP 2/5 准备 wimlib 日志回放夹具（仅首次，85MB 一次）"
if [ -z "$(vm "if exist $REPLAY_LOG (echo 1)")" ]; then
    vm "if not exist $REPLAY_ROOT\\.test-artifacts mkdir $REPLAY_ROOT\\.test-artifacts" >/dev/null 2>&1
    vm "if not exist $REPLAY_CWD mkdir $REPLAY_CWD" >/dev/null 2>&1
    # ★ 源在 X:，而 X: 只是**用户会话**里的共享映射（SYSTEM 通道看不到），
    #   所以复制必须走 vm_user。用 SYSTEM 复制会静默失败、判成「夹具缺失」。
    vm_user "copy /y \"$SRC_WIN\\.test-artifacts\\e2e-brimglog-227.log\" $REPLAY_LOG" >/dev/null 2>&1
fi
if [ -z "$(vm "if exist $REPLAY_LOG (echo 1)")" ]; then
    say "  FIXTURE_MISSING=1（回放用例将判 SKIP）"
else
    say "  FIXTURE_LOG=present"
fi

# ---------- 跑 ----------
# 判据只看结构化字段：`test result: ok. 1 passed` / `test result: FAILED`。
run_one() {
    local name=$1 filter=$2 cwd=$3 envs=$4
    local log_win="$BIN_DIR\\$name.log"
    vm "del /f /q $log_win" >/dev/null 2>&1
    # ★ 把 env 前提**真的交给被测进程**。上一版只在宿主检查变量存在就放行，
    #   却不把它们传给 VM 里的测试二进制 —— 测试读不到变量会 panic，
    #   而症状看起来像「产品坏了」。`set` 与测试在同一条 cmd 会话，子进程继承。
    #   判据纪律：检查了前提，就必须把前提交给被测对象。
    local prefix="" kv val
    for kv in $envs; do
        eval "val=\${$kv}"
        # ★ 必须写成 `set "K=V"`：cmd 的 `set K=V &` 会把 `&` 前那个空格
        #   算进值里，于是 ACK 变成 "…secondary "（尾随空格）→ 断言不等。
        prefix="$prefix set \"$kv=$val\" &"
    done
    # ★ 先确认「命令真的发出去了」（2026-10-27 实测）：约 1/8 的 `prlctl exec`
    #   调用**根本没执行**（日志文件连创建都没有，两次门禁分别落在
    #   secondary_mapping_rejections 与 secondary_menu_repair 上，而同一条用例
    #   手工重跑都是 0.01s / 1.8s 全绿）。认不出这一点，就只能干等 180 秒超时
    #   然后把「仪器没工作」误判成 FAIL。所以发完立刻查日志是否诞生，没诞生就重发。
    local attempt=0 launched=""
    while [ $attempt -lt 3 ]; do
        attempt=$((attempt + 1))
        vm "$prefix cd /d $cwd && (\"$BIN_WIN\" --ignored --nocapture $filter > $log_win 2>&1) & echo LAUNCHED" >/dev/null 2>&1
        local a=0
        launched=""
        while [ $a -lt 16 ]; do
            sleep 2; a=$((a + 2))
            launched=$(vm "if exist $log_win (echo 1)")
            [ -n "$launched" ] && break
        done
        [ -n "$launched" ] && break
        say "  RELAUNCH=$attempt (prlctl exec did not run the command)"
    done

    local w=0 out=""
    while [ $w -lt 180 ]; do
        sleep 3; w=$((w + 3))
        out=$(vm "findstr /c:\"test result:\" $log_win")
        if [ -n "$out" ]; then break; fi
    done
    # ★ 短任务不能只靠后台（2026-10-27 实测）：`prlctl exec` 返回时会把 `(...) &`
    #   起的子进程一起带走 —— 0.01s 就跑完的用例，日志文件**根本没被创建**
    #   （`secondary_mapping_rejections` 在门禁里判 FAIL，手工重跑 0.01s 全绿）。
    #   后台没拿到结果就**同步再跑一次**取真值；同步也没有才判失败。
    #   注意：这只是「仪器没读到」的补救，不是把失败改判成通过——判据仍是那一行。
    if [ -z "$out" ]; then
        say "  RETRY_SYNC=background_run_produced_no_log"
        vm "$prefix cd /d $cwd && \"$BIN_WIN\" --ignored --nocapture $filter > $log_win 2>&1" >/dev/null 2>&1
        out=$(vm "findstr /c:\"test result:\" $log_win")
    fi
    local full
    full=$(vm "type $log_win")
    if [ -z "$out" ]; then
        # 没跑到 test result 行：可能是挂死/崩溃。绝不能因为「没看见 FAILED」就记 PASS。
        say "  EVIDENCE="
        printf '%s\n' "$full" | tail -6 | sed 's/^/    /'
        return 1
    fi
    if printf '%s\n' "$out" | grep -Eq "test result: ok\. [1-9][0-9]* passed; 0 failed"; then
        say "  EVIDENCE=$(printf '%s\n' "$out" | tr -d '\r' | tail -1)"
        return 0
    fi
    say "  EVIDENCE=$(printf '%s\n' "$out" | tr -d '\r' | tail -1)"
    printf '%s\n' "$full" | grep -E "panicked|^failures:" -A 2 | head -8 | sed 's/^/    /'
    return 1
}

env_missing() {
    local missing=""
    for v in "$@"; do
        [ -z "$v" ] && continue
        # ★ 查**宿主**，不查 VM：真正的传递由 run_one 的 `set` 前缀完成。
        #   上一版查 VM 内 `if defined`，而 run_one 又不传 —— 两头都没打通，
        #   于是「宿主配了变量也永远 SKIP」，5 条 env 用例从来没真跑过。
        # 用 `${!v:-}`：脚本是 `set -u`，对**未定义**变量的 `${!v}` 会直接
        # unbound error 退出（实测：跑到 winre_offline_reg 时整个门禁中断，
        # 看起来像「跑到一半没了」而不是「这条 SKIP」）。
        if [ -z "${!v:-}" ]; then missing="$missing $v"; fi
    done
    printf '%s' "$missing"
}

# ---------- 用例存在性：缺用例不能被 SKIP 掩盖 ----------
# ★ 2026-10-27 血泪：往测试模块里插新用例时，用「`#[ignore] fn X` 到函数体」当锚点，
#   把原用例的 `#[cfg(windows)] #[test]` 抢给了新函数 —— 原用例**不再是测试**。
#   而门禁在「环境变量缺失」那一步就 SKIP 了，永远不会发现它根本不在这份二进制里。
#   所以：跑之前先 `--ignored --list` 拿真值，任何一条 filter 不在列表里 = FAIL
#   （这是缺陷，不是环境缺失；也不许记成 SKIP）。
say "STEP 3/5 核对每条用例都真在测试二进制里（不存在 ≠ SKIP）"
LIST_WIN="$BIN_DIR\\ignored-tests.list"
vm "del /f /q $LIST_WIN" >/dev/null 2>&1
# ★ 同步执行，**不要**用 `(... ) & echo` 后台起：`prlctl exec` 返回时会把
#   子进程一起带走，于是文件常常是空的或只有前几行——实测完整门禁偶发
#   「取不到 --ignored --list」（单独跑同一条却成功，典型竞争）。
#   `--list` 是秒级任务，没有异步的必要；真要异步的只有 STEP 4 那些跑
#   DISM 的用例，那里有 180 秒轮询兜着。
if ! vm_launch_verified vm "cd /d $BIN_DIR && \"$BIN_WIN\" --ignored --list > $LIST_WIN 2>&1" \
        "findstr /c:\"benchmarks\" $LIST_WIN" 30 >/dev/null; then
    say "ERROR: 取不到 --ignored --list（仪器故障，不等于用例缺失）。这不是「测试通过」。"
    exit 2
fi
list_text=""
for _ in 1 2 3 4 5; do
    list_text=$(vm "type $LIST_WIN")
    # 完整标志是 libtest 结尾那行 `N tests, 0 benchmarks`（边输出边刷新，
    # 看到第一行就 break 会只拿到前几条，造出「用例不存在」的假红）。
    printf '%s' "$list_text" | grep -q "benchmarks" && break
    sleep 1
done
if ! printf '%s' "$list_text" | grep -q "benchmarks"; then
    say "ERROR: 取不到 --ignored --list（仪器故障，不等于用例缺失）。这不是「测试通过」。"
    exit 2
fi
say "STEP 4/5 逐条运行"
PASS=0; FAIL=0; SKIP=0; RAN=0
for row in "${CASES[@]}"; do
    IFS='|' read -r name filter kind arg <<<"$row"
    if [ -n "$ONLY" ] && [ "$ONLY" != "$name" ]; then continue; fi

    # 用例必须真在这份二进制里：`--ignored --list` 打印 `tests::<函数名>: test`。
    # 缺了就是缺陷（被删、或丢了 #[test]），不能因为「环境变量没配」而记成 SKIP。
    if ! printf '%s\n' "$list_text" | grep -F -- "$filter" | grep -q ": test"; then
        FAIL=$((FAIL + 1))
        say "CASE=$name=FAIL reason=test_not_in_binary(filter=$filter)"
        continue
    fi

    case "$kind" in
        pe)
            # ★ 2026-10-27 实测：RE 里**没有 Parallels Tools**，`prlctl exec`
            #   通道不存在（连 `echo` 都返回空），自动化无法在 PE 内启动测试
            #   二进制。可以进 RE（产品 prepare 机制已验证），但要在 RE 内
            #   执行命令只能靠「命令提示符」+ 键盘注入，不在本门禁能力内。
            #   手工举证步骤见 docs/20261027-2245-解锁剩余 ignore 用例-*.md。
            SKIP=$((SKIP + 1))
            say "CASE=$name=SKIP reason=requires_windows_pe_manual_in_re"
            continue
            ;;
        env)
            miss=$(env_missing $arg)
            if [ -n "$miss" ]; then
                SKIP=$((SKIP + 1))
                say "CASE=$name=SKIP reason=missing_env:$miss"
                continue
            fi
            ;;
    esac

    cwd="$BIN_DIR"
    if [ "$name" = "wimlib_log_replay" ]; then
        if [ -z "$(vm "if exist $REPLAY_LOG (echo 1)")" ]; then
            SKIP=$((SKIP + 1))
            say "CASE=$name=SKIP reason=fixture_log_missing"
            continue
        fi
        cwd="$REPLAY_CWD"
    fi

    RAN=$((RAN + 1))
    say "RUN $name (filter=$filter)"
    if run_one "$name" "$filter" "$cwd" "$arg"; then
        PASS=$((PASS + 1)); say "CASE=$name=PASS"
    else
        FAIL=$((FAIL + 1)); say "CASE=$name=FAIL"
    fi
done

say "SUMMARY pass=$PASS fail=$FAIL skip=$SKIP ran=$RAN"
say "DONE"

if [ "$FAIL" -gt 0 ]; then exit 1; fi
# 一条都没真跑 = 门禁没工作。这种情况绝不能返回 0。
if [ "$RAN" -eq 0 ]; then
    say "ERROR: RAN=0，没有任何用例真正执行（缺夹具不等于通过）"
    exit 2
fi
exit 0
