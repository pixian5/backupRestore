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

# ---------- 用例表 ----------
# 每行：名字|filter|前提种类|前提参数
#   前提种类 none = 只需管理员；env = 需要一组环境变量；pe = 只能在 PE/RE 内跑
CASES=(
  "no_write_on_reject|reject_without_write|none|"
  "online_capture_indexes|online_capture_append_keep|none|"
  "wimlib_log_replay|replay_real_wimlib_log|none|"
  "secondary_registry_map|offline_secondary_mapping|env|BR_SECONDARY_MAPPING_ACK BR_SECONDARY_MAPPING_LETTER BR_SECONDARY_MAPPING_GUID BR_SECONDARY_MAPPING_LOG"
  "winre_offline_reg|offline_registration_on_test_partition|env|BR_WINRE_TEST_ACK BR_WINRE_TEST_TARGET BR_WINRE_TEST_PRIMARY BR_WINRE_TEST_GUID BR_WINRE_TEST_EFI BR_WINRE_TEST_LOG"
  "secondary_menu_repair|secondary_menu_repair_native|env|BR_ACCEPTANCE_SNAPSHOT BR_ACCEPTANCE_ROOT BR_ACCEPTANCE_TASK BR_ACCEPTANCE_EFI BR_ACCEPTANCE_SECONDARY"
  "boot_entry_fault|missing_payload_conflict_and_cleanup_retry|env|BR_ACCEPTANCE_SNAPSHOT BR_ACCEPTANCE_ROOT"
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

# ---------- 前置：编译（用户会话，因为 X: 只在用户会话里可见）----------
say "STEP 1/3 编译 Windows 测试目标（用户会话，源码在 X:）"
vm_user "del /f /q $BUILD_LOG" >/dev/null 2>&1
vm_user "cd /d $SRC_WIN && (cargo test -p backuprestore-cli --no-run --target-dir $TARGET_WIN > $BUILD_LOG 2>&1) & echo LAUNCHED" >/dev/null 2>&1

waited=0
while [ $waited -lt 300 ]; do
    sleep 5; waited=$((waited + 5))
    if [ -n "$(vm "findstr /c:\"Finished\" $BUILD_LOG")" ]; then break; fi
done

# 判据分离：编译是否成功看 build log 的 `Finished`；产物路径**不解析日志**，
# 直接取 deps 里最近修改的那个 exe（`dir /o-d` 降序取第一个）。
# ★ 不用正则抓日志里的括号路径：zsh 的 echo 会把 `\b` 当退格解释、BSD sed 对
#   行尾空白处理不一，两种都会静默取空——本轮为此白等过一个 300 秒超时。
exe_win=""
if [ -n "$(vm "findstr /c:\"Finished\" $BUILD_LOG")" ]; then
    exe_name=$(vm "dir /b /o-d $TARGET_WIN\\debug\\deps\\backuprestore_cli-*.exe" | head -1)
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
vm "copy /y \"$exe_win\" $BIN_WIN" >/dev/null 2>&1
if [ -z "$(vm "if exist $BIN_WIN (echo 1)")" ]; then
    say "ERROR: 复制测试二进制到 $BIN_WIN 失败（这不是「测试通过」）。"
    exit 2
fi

# ---------- #3 的 fixture：复刻 ../../.test-artifacts 两级结构 ----------
say "STEP 2/3 准备 wimlib 日志回放夹具（仅首次，85MB 一次）"
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
    local name=$1 filter=$2 cwd=$3
    local log_win="$BIN_DIR\\$name.log"
    vm "del /f /q $log_win" >/dev/null 2>&1
    vm "cd /d $cwd && (\"$BIN_WIN\" --ignored --nocapture $filter > $log_win 2>&1) & echo LAUNCHED" >/dev/null 2>&1

    local w=0 out=""
    while [ $w -lt 180 ]; do
        sleep 3; w=$((w + 3))
        out=$(vm "findstr /c:\"test result:\" $log_win")
        if [ -n "$out" ]; then break; fi
    done
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
        if [ -z "$(vm "if defined $v (echo 1)")" ]; then missing="$missing $v"; fi
    done
    printf '%s' "$missing"
}

say "STEP 3/3 逐条运行"
PASS=0; FAIL=0; SKIP=0; RAN=0
for row in "${CASES[@]}"; do
    IFS='|' read -r name filter kind arg <<<"$row"
    if [ -n "$ONLY" ] && [ "$ONLY" != "$name" ]; then continue; fi

    case "$kind" in
        pe)
            SKIP=$((SKIP + 1))
            say "CASE=$name=SKIP reason=requires_windows_pe_or_re"
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
    if run_one "$name" "$filter" "$cwd"; then
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
