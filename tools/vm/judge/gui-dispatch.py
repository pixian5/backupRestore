# 从「原样取回」的界面日志（H:\brwork\logs\gui.log）里，数**某一时点之后**界面自己记下的结构化证据。
# 被 tools/vm/gui-data-restore.sh 调用；tools/vm/selftest.sh 用人工拼的日志离线验证它。
#
# 用法：python3 judge/gui-dispatch.py <gui.raw.log> <起算时点>
#   起算时点形如 2026-11-25T12:33:46（虚拟机自己的世界时，精确到秒），与日志行 [2026-11-25T12:33:48.15…+00:00]
#   里的时间戳按字符串比较（同为世界时、同为定长格式，所以可以直接比）。
#
# ★ 界面日志是跨天累积的：不划界就会把历史上的记录数进来（2026-11-25 实测：数整份文件时，
#   两天前的两次「已拦截」被当成了本次被拦截，一次通过的用例被判成失败）。
#
# 输出（一行一个键值）：
#   GUI_LOG_LINES_AFTER=<n>                 起算时点之后的日志行数（0 说明划界本身有问题，比如时钟读错了）
#   GUI_LOG_SKIP_FORMAT_ARGS=<n>            「prepare args: --skip-format (overlay restore)」的行数
#   GUI_LOG_CONFIRM_YES=<n>                 「Windows RE confirm result=6 (6=yes)」的行数
#   GUI_LOG_CHOICE_RE=<n>                   「user chose Windows RE (prepare chain)」的行数
#   GUI_LOG_BLOCKED=<n>                     「GUI action blocked」的行数
#   GUI_LOG_ERROR=<n>                       含 [ERROR] 的行数
#   GUI_EVIDENCE_LINE <原文>                上面几类里最先出现的几行（供人看）
import sys

KEYS = {
    "GUI_LOG_SKIP_FORMAT_ARGS": "prepare args: --skip-format (overlay restore)",
    "GUI_LOG_CONFIRM_YES": "offline data restore: Windows RE confirm result=6 (6=yes)",
    "GUI_LOG_CHOICE_RE": "offline data restore: user chose Windows RE (prepare chain)",
    "GUI_LOG_BLOCKED": "GUI action blocked",
    "GUI_LOG_ERROR": "[ERROR]",
}


def stamp(line):
    # 日志行形如 [2026-11-25T12:33:48.152660600+00:00] 正文：方括号里先是 19 位的「年-月-日T时:分:秒」，
    # 后面跟小数点（带小数秒）或右方括号（不带）。没有时间戳的行不参与计数（返回空串）。
    # ★ 第一版把「时间戳之后」写成了必须是右方括号，结果所有行都被当成没有时间戳，一行都数不到
    #   （2026-11-25 在真实日志上离线试跑时发现）。
    if len(line) > 21 and line[0] == "[" and line[20] in ".]":
        head = line[1:20]
        if len(head) == 19 and head[4] == "-" and head[10] == "T" and head[13] == ":":
            return head
    return ""


def judge(log_path, since, out=print):
    raw = open(log_path, "rb").read().decode("utf-8", "replace")
    after = [l for l in raw.splitlines() if stamp(l) and stamp(l) >= since]
    out("GUI_LOG_LINES_AFTER=%d" % len(after))
    for key, needle in KEYS.items():
        hits = [l for l in after if needle in l]
        out("%s=%d" % (key, len(hits)))
        if hits:
            out("GUI_EVIDENCE_LINE " + hits[0][:200])
    return len(after)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: judge/gui-dispatch.py <gui.raw.log> <since, e.g. 2026-11-25T12:33:46>")
    n = judge(sys.argv[1], sys.argv[2])
    sys.exit(0 if n > 0 else 1)
