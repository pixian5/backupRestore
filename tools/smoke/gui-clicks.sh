#!/usr/bin/env bash
# 一键图形界面点击验收（2026-10-26）
#
# 为什么需要它：图形界面是 Windows 专属代码，mac 上 `cargo test` 对它零证明力；
# 每次改按钮/入口都靠人肉点一遍，回归成本高且容易漏。这里把 2026-10-26 建的三个
# 点击脚本串成一条命令：借图形界面**自己的令牌**提权到会话 1（绕开 UIPI），
# 点按钮 → 读断言文件 → 汇总 PASS/FAIL。
#
# 前置条件（缺一不可）：
#   ① 虚拟机在跑；
#   ② 图形界面已从 H:\brwork 启动（tools/win-clicker/br-s1.sh start-gui.ps1）。
#
# 用法：
#   tools/smoke/gui-clicks.sh              # 跑全部无副作用用例
#
# 判据纪律：只认脚本写出的 ASCII 断言行（LOGS_WINDOW_FOUND / OVERLAP_COUNT=0 …）。
# 窗口标题是中文，在 cmd 通道里是乱码，**绝不能拿它当判据**。
set -uo pipefail

VM="Windows 11"
UNC_BODY='\\Mac\Home\code\backupRestore\tools\win-clicker'
UNC_LAUNCH='\\Mac\Home\code\backupRestore\tools\run-gui-token.ps1'
HOOK='H:\brwork\gui-token-body.txt'
MODE="${1:-default}"

pass=0
fail=0

gui_alive() {
    prlctl exec "$VM" cmd /d /c 'tasklist /fi "imagename eq BackupRestore.exe" /fo csv /nh' 2>/dev/null \
        | grep -q BackupRestore.exe
}

# 断言可以给多条（第 3 个参数之后全部是期望子串），每条单独计分。
# 2026-10-26：原来只支持一条，而「镜像清单窗口」需要同时证明清单已填充、
# 提示非空、详情可读、窗口能关——一条子串证明不了这些互不相干的事。
run_case() {
    local body="$1" out="$2" label="$3"
    shift 3
    echo "-- ${label}"
    prlctl exec "$VM" cmd /d /c "echo ${UNC_BODY}\\${body}>${HOOK}" >/dev/null 2>&1
    prlctl exec "$VM" cmd /d /c "powershell -NoProfile -ExecutionPolicy Bypass -Command \"iex (Get-Content -Raw -Encoding UTF8 ${UNC_LAUNCH})\"" >/dev/null 2>&1
    # 子进程没有 stdout，结果只能从文件读；给它 7 秒写完。
    sleep 7
    # ★ 通道会偶发返回空（不是文件为空）。不重试的话，一次抽风就被记成一条
    #   红判据——2026-10-26 实测：probe-entry-check.txt 里明明是
    #   "after: probe_checked=1"，这里读回空串，于是报了 FAIL。
    #   重试到非空为止；真的读不到就报 READ_EMPTY，与「判据不满足」区分开。
    local text attempt
    for attempt in 1 2 3 4 5; do
        text="$(prlctl exec "$VM" cmd /d /c "type ${out}" 2>/dev/null)"
        [ -n "$(printf '%s' "$text" | tr -d '[:space:]')" ] && break
        sleep 1
    done
    if [ -z "$(printf '%s' "$text" | tr -d '[:space:]')" ]; then
        echo "   FAIL  [READ_EMPTY after ${attempt} attempts: ${out}]"
        fail=$((fail + 1))
        return
    fi
    local ok
    ok=1
    for expect in "$@"; do
        if printf '%s' "$text" | grep -qaF "$expect"; then
            echo "   PASS  [${expect}]"
            pass=$((pass + 1))
        else
            echo "   FAIL  [expected ${expect}]"
            ok=0
            fail=$((fail + 1))
        fi
    done
    [ "$ok" -eq 1 ] || printf '%s\n' "$text" | sed 's/^/         /'
}

if ! gui_alive; then
    echo "图形界面未运行。先从 H:\\brwork 启动它："
    echo "  tools/win-clicker/br-s1.sh start-gui.ps1"
    exit 2
fi

# ★ 布局判据（2026-10-26 补）：此前所有用例都只问「控件存在吗 / 点得动吗」，
#   没有一条问「两个控件是不是占了同一块像素」。于是「操作模式」6 个入口与顶栏
#   4 个按钮同处 y=60 这个缺陷活了很久——用户一眼看到的是第二排按钮压在入口上面。
#   OVERLAP_COUNT=0 是唯一能自动抓住这类问题的判据。
#   ★ 2026-10-26 v2：判据从「手挑 15 个控件、只看当前页」升级为
#     「枚举全部直接子窗口、每个页签各查一遍」。隐藏行会把别的行顶上去，
#     每页几何不同——只查一页等于只验了 1/5。PAGES_CHECKED=5 证明每页都真走过。
#   ★ 2026-10-26 v3：再加四条——不越界(OUTSIDE_COUNT)、不截断(TEXT_CLIPPED)、
#     装得下(TEXT_OVERFLOW)、不塌缩(ZERO_SIZE)。前三条各自做过变异验证：
#     越界 → 5、按钮压到 20px → 5、详情框改回 112px → 5、按钮宽 0 → ZERO_SIZE=5。
#     TEXT_OVERFLOW 用「行数 × 行距(14px)」而不是 tmHeight(12px)：按 12 算会
#     得出「9 行只要 108px 装得下」，而屏幕上最后一行明明被切成半截。
#   ★ 2026-10-26 v4：再加 MISSING_CONTROLS，真值来自 control-baseline.txt
#     （page|id|class，148 条，随仓库入库）。几何判据看不见「少了东西」——
#     不重叠、不越界、不溢出，只是界面比原来少一个控件。变异验证：基线多一条
#     → MISSING_CONTROLS=1；基线少一条 → BASELINE_STALE=1。
run_case _t-layout-rects.ps1 'H:\brwork\gui-layout-rects.txt' '五个页签的几何判据（不重叠/不越界/不截断/不塌缩）' \
    'OVERLAP_COUNT=0' 'OUTSIDE_COUNT=0' 'TEXT_CLIPPED=0' 'TEXT_OVERFLOW=0' 'ZERO_SIZE=0' \
    'MISSING_CONTROLS=0' 'PAGES_CHECKED=5' 'CLIENT_W='
# ★ 状态回执用例（2026-10-26 v2.6.64 补）：「刷新任务状态」此前**从不读 status.json**，
#   恢复环境里失败的任务回到 Windows 后界面连「失败」二字都没有。这里把四种结局
#   全部在实机上走一遍：带码失败 / 失败无原因 / 文件缺失 / 解析失败，并断言
#   状态框里真的出现了错误码与失败原因（ASCII 判据）。
#   副作用：运行期间会把 H:\brwork\last-task.json 临时换成测试版并在结尾还原，
#   脚本用 RESTORED=1 自证（还原失败会让这条判据变红，而不是静默留下脏数据）。
run_case _t-refresh-status.ps1 'H:\brwork\refresh-status-check.txt' '「刷新任务状态」四种结局（错误码 / 未记录原因 / 缺失 / 解析失败）' \
    'CASE_RESULT=S1_OK_WITH_CODE=PASS' 'CASE_RESULT=S2_OK_NO_REASON=PASS' \
    'CASE_RESULT=S3_MISSING_FILE=PASS' 'CASE_RESULT=S4_MALFORMED=PASS' \
    'RESTORED=1' 'DONE'
run_case _t-view-logs.ps1   'H:\brwork\logs-viewer-check.txt'  '顶栏「查看日志」→ 日志清单窗口' LOGS_WINDOW_FOUND
run_case _t-image-admin.ps1 'H:\brwork\image-admin-check.txt'  '顶栏「备份文件管理」→ 镜像清单窗口（零副作用）' \
    IMAGE_ADMIN_WINDOW_FOUND 'combo_matches_disk=1' 'hint_nonempty=1' 'detail_filled=1' 'closed=1'

echo ""
echo "结果：PASS=${pass} FAIL=${fail}"
[ "$fail" -eq 0 ] || exit 1
