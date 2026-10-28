#!/usr/bin/env bash
# source-guard.sh —— 源码静态守卫：防止「挂了盘符不管」这条老路被重新写出来
#
# 为什么需要它（2026-10-27/28 两轮实机事故的结论）：
#   `mountvol <letter>: <卷>` 与 diskpart `assign letter=` 写的都是**持久**记录
#   （`HKLM\SYSTEM\MountedDevices`）。用完不撤，盘符就永久留在系统里；等那个分区
#   后来被删掉，记录还在，Windows 会为这个并不存在的卷反复弹「X:\ 无法访问」。
#   本机一次查出 6 条，测试夹具还每跑一次留一条——全部都是「挂了没撤」。
#
#   代码层面已经做了结构收敛：不返回租约的 `ensure_volume_mounted` 已删除，
#   只留 `ensure_volume_mounted_leased`。但「再有人绕开入口直接调 mountvol」是
#   编译器管不到的，所以这里补一道静态守卫。
#
# 判据（三态，与项目纪律一致）：
#   A. 出现 `assign letter=` 或 `Command::new("mountvol.exe")` 的文件必须在白名单里
#      —— 白名单是**棘轮**：只许减不许增。新增即 FAIL，逼人先想清楚生命周期。
#   B. `windows_prepare.rs` 必须仍有 `ensure_volume_mounted_leased`，且**不得**
#      再出现不返回租约的 `pub(crate) fn ensure_volume_mounted(`。
#   C. 列出 `hold_for_process()` 的全部调用点（逃生口是显式的，必须随时一眼数清）。
#
# 退出码：0 = 通过；1 = 违反；2 = 仪器故障（找不到源码树 / 判据本身读不到）
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="$ROOT/crates/backuprestore-cli/src"
WP="$SRC/windows_prepare.rs"

if [ ! -d "$SRC" ]; then
    echo "ERROR=source_tree_missing path=$SRC"
    exit 2
fi
if [ ! -f "$WP" ]; then
    echo "ERROR=windows_prepare_missing path=$WP"
    exit 2
fi

fail=0
say() { printf '%s\n' "$*"; }

# ---------- A. 裸挂载调用只许出现在白名单文件里 ----------
# 白名单与理由（新增条目必须写清楚为什么它是安全的）：
#   windows_prepare.rs —— 唯一实现（mount_volume_guid / identity_from_diskpart）
#   main.rs            —— RE 内的挂载助手（hive 是内存盘，不落盘、不跨重启）
#   pe_operation.rs    —— 测试夹具自建 VHD（同一文件里必须有 remove letter= 收尾）
ALLOWED="windows_prepare.rs main.rs pe_operation.rs"

hits=$(grep -rln -e 'assign letter=' -e 'Command::new("mountvol.exe")' "$SRC" 2>/dev/null | sort)
say "--- A. 裸挂载调用分布 ---"
if [ -z "$hits" ]; then
    say "ERROR=no_mount_call_sites_found（这不可能：连唯一实现都没了，判据本身坏了）"
    exit 2
fi
while IFS= read -r file; do
    [ -z "$file" ] && continue
    base=$(basename "$file")
    # ★ 必须只看**代码**，不只看字符串：注释里提到 `assign letter=` 的文件
    #   （text_parsing.rs 就有一句解释 diskpart 语义的注释）不该被算成调用点。
    real=$(awk '
        /assign letter=|Command::new\("mountvol\.exe"\)/ {
            code = $0
            sub(/\/\/.*$/, "", code)
            if (code ~ /assign letter=|Command::new\("mountvol\.exe"\)/) print FNR": "$0
        }' "$file")
    if [ -z "$real" ]; then
        say "SKIP file=$base reason=only_in_comments"
        continue
    fi
    ok=0
    for allowed in $ALLOWED; do
        [ "$base" = "$allowed" ] && ok=1
    done
    if [ "$ok" = "0" ]; then
        say "VIOLATION=unlisted_file file=$base"
        printf '%s\n' "$real" | sed 's/^/  /'
        say "  说明：这个文件直接挂了盘符却没有进白名单。请改用"
        say "        windows_prepare::ensure_volume_mounted_leased（它会自动撤销），"
        say "        确有特殊理由再加进本脚本的白名单并写明原因。"
        fail=1
    else
        say "OK file=$base"
    fi
done <<< "$hits"

# 测试夹具必须自己收尾：给了哪个字母，就必须撤哪个字母。
# ★ 两层坑，本轮都当场踩到：
#   ① 直接 grep 字符串会被**注释**满足（本文件里就有一句「修法：先 remove letter=V」）；
#   ② 只数「有没有 remove」也不够——`Xremove letter=V` 这种改坏写法仍包含子串，
#      而且撤错字母（assign V / remove W）同样留残留。
#   所以剥注释后按**字母逐一配对**：assign 的每个字母都必须出现在 remove 里。
if grep -q 'assign letter=' "$SRC/pe_operation.rs" 2>/dev/null; then
    letters_of() {
        awk -v key="$1" '
            {
                code = $0
                sub(/\/\/.*$/, "", code)
                needle = "letter="
                pos = 1
                while ((idx = index(substr(code, pos), key needle)) > 0) {
                    # idx 是 1-based 相对 pos 的位置：字母在 `pos + idx - 1 + len(needle)`。
                    # ★ 这里曾经少减 1，取到的是字母后面那个 `\`，于是一个字母都提不出来，
                    #   而「空集」又让下面的配对检查全部跳过 —— 判据自己造出「空集通过」。
                    start = pos + idx + length(key needle) - 1
                    ch = substr(code, start, 1)
                    if (ch ~ /[A-Za-z]/) print toupper(ch)
                    pos = start
                }
            }' "$SRC/pe_operation.rs" | sort -u | tr -d '\n'
    }
    assign_letters=$(letters_of "assign ")
    say "  assign_letters=$assign_letters"
    # ★ 空集不是「通过」：文件里明明有 assign，却一个字母都解析不出来，说明**判据坏了**
    #   （本轮就踩过：偏移差 1，提取全空，配对检查全部跳过，报了个假 PASS）。
    if [ -z "$assign_letters" ]; then
        say "ERROR=assign_letters_not_parsed（判据故障，不是「通过」）"
        exit 2
    fi
    remove_letters=$(letters_of "remove ")
    say "  remove_letters=$remove_letters"
    for letter in $(printf '%s' "$assign_letters" | fold -w1); do
        if ! printf '%s' "$remove_letters" | grep -q "$letter"; then
            say "VIOLATION=fixture_letter_not_removed file=pe_operation.rs letter=$letter"
            # ★ 变量一律写成 `${letter}`：`$letter，` 里那个中文逗号是多字节，
            #   bash 会把它的字节吞进变量名 → `set -u` 报 unbound variable。
            say "  说明：夹具 assign 了 ${letter}: 却没有 remove letter=${letter}，"
            say "        每跑一次就留一个指向已销毁 VHD 分区的幽灵盘符。"
            fail=1
        fi
    done
    [ "$fail" = "0" ] && say "OK fixture_cleanup=pe_operation.rs letters=$assign_letters"
fi

# ---------- B. 那扇门必须保持关闭 ----------
say "--- B. 唯一入口仍在、旧入口仍关 ---"
if ! grep -q 'pub(crate) fn ensure_volume_mounted_leased(' "$WP"; then
    say "VIOLATION=leased_entry_missing file=windows_prepare.rs"
    fail=1
else
    say "OK leased_entry_present"
fi
if grep -q 'pub(crate) fn ensure_volume_mounted(' "$WP"; then
    say "VIOLATION=unleased_entry_reopened file=windows_prepare.rs"
    say "  说明：不返回租约的 ensure_volume_mounted 又回来了。它是「挂了不管」的入口，"
    say "        2026-10-28 已删除，不要恢复。"
    fail=1
else
    say "OK unleased_entry_closed"
fi

# ---------- C. 逃生口清单（信息项，不判失败）----------
say "--- C. hold_for_process 调用点（字母会逸出的地方）---"
# 只数**真正的调用**（行尾 `hold_for_process();`），不要把注释里的提及算进来。
hold_sites=$(grep -rn 'hold_for_process();' "$SRC" 2>/dev/null \
    | grep -v '^\s*//' \
    | grep -Ev ':\s*//' || true)
if [ -n "$hold_sites" ]; then
    printf '%s\n' "$hold_sites" | sed "s|$SRC/||" | sed 's/^/  /'
fi
count=$(printf '%s' "$hold_sites" | grep -c . || true)
say "HOLD_FOR_PROCESS_COUNT=${count:-0}"

if [ "$fail" != "0" ]; then
    say "RESULT=FAIL"
    exit 1
fi
say "RESULT=PASS"
exit 0
