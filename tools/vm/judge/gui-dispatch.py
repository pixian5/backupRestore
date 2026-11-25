# 从「原样取回」的界面日志（H:\brwork\logs\gui.log）里，数**某一时点之后**界面自己记下的结构化证据。
# 被 tools/vm/gui-data-restore.sh 与 tools/vm/gui-backup.sh 调用；tools/vm/selftest.sh 用人工拼的日志离线验证它。
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
#   —— 备份入口（tools/vm/gui-backup.sh）——
#   GUI_LOG_CANCELLED=<n>                   「GUI action cancelled: destructive confirmation; operation=backup」的行数
#                                           （破坏性确认框上点了「否」之后界面自己记下的那一行）
#   GUI_LOG_SYSTEM_CHOICE_ANY=<n>            「system drive choice returned:」的行数（处理方式选择框**弹出过**；答了什么不论）
#   GUI_LOG_SYSTEM_CHOICE_RE=<n>             同上，且答的是「进入 Windows RE」
#   GUI_LOG_SYSTEM_CONFIRM_YES=<n>          「system drive operation: Windows RE confirm result=6 (6=yes)」的行数
#   GUI_LOG_PREPARE_LAUNCH=<n>              「elevated prepare: launching (visible) operation=backup」的行数
#   GUI_LOG_PREPARE_FAILED_EARLY=<n>        「elevated prepare failed early」的行数（准备进程 1.5 秒内异常退出）
#   GUI_LOG_DELEGATED=<n>                   「GUI action delegated: elevated prepare launched; operation=backup」的行数
#   GUI_LOG_ONLINE_SPLIT=<n>                「online operation: affected=」的行数（确认框之后分流到了在线分支）
#   GUI_LOG_ONLINE_STARTED=<n>              「online started: op=backup」的行数
#   GUI_LOG_ONLINE_DONE=<n>                 「online completed: success=true」的行数
#   GUI_LOG_ONLINE_FAILED=<n>               「online completed: success=false」的行数
#   GUI_LOG_ACTION_FAILED=<n>               「GUI action failed」的行数
#   —— 「还原数据到非 C 盘」在线、不格式化（tools/vm/gui-data-restore.sh online）——
#   GUI_LOG_CANCELLED_ANY=<n>               「GUI action cancelled: destructive confirmation」的行数（任何入口在破坏性确认上答了「否」）
#   GUI_LOG_CHANNEL_ONLINE=<n>              「channel split: op=restore-data online=true」的行数（数据入口分流到了在线通道）
#   GUI_LOG_ONLINE_PARAMS_NO_FORMAT=<n>     「online params: format_target=false」的行数（在线通道收到的是「不格式化」）
#   GUI_LOG_ONLINE_RESTORE_STARTED=<n>      「online started: op=restore-existing」的行数（在线还原已开始）
#   —— 界面「说不」矩阵（tools/vm/gui-negative.sh）——
#   GUI_LOG_CANCELLED_RESTORE_EXISTING=<n>  「GUI action cancelled: destructive confirmation; operation=restore-existing」
#   GUI_LOG_CANCELLED_RESTORE_DATA=<n>      同上，operation=restore-data
#   GUI_LOG_CANCELLED_CREATE_SECONDARY=<n>  同上，operation=create-secondary
#   GUI_LOG_OVERLAY_DECLINED=<n>            「GUI action blocked: overlay restore (wimlib route) declined by the user」
#   GUI_LOG_OFFLINE_CONFIRM_NO=<n>          「offline data restore: Windows RE confirm result=7 (6=yes)」
#   GUI_LOG_OFFLINE_CONFIRM_DECLINED=<n>    「offline data restore: Windows RE confirm declined」
#   GUI_LOG_METADATA_DECLINED=<n>           「restore cancelled: metadata check declined」
#   GUI_LOG_IMAGE_ON_SOURCE_BLOCKED=<n>     「GUI action blocked: image volume matches source volume」
#   GUI_LOG_PATH_GUIDANCE_CANCELLED=<n>     「GUI image path guidance: user choice=0」（引导框上点了「取消」）
#   GUI_LOG_SYSTEM_CHOICE_CANCELLED=<n>     「system drive operation cancelled by user」
#   GUI_EVIDENCE_LINE <原文>                上面几类里最先出现的几行（供人看）
import sys

KEYS = {
    "GUI_LOG_SKIP_FORMAT_ARGS": "prepare args: --skip-format (overlay restore)",
    "GUI_LOG_CONFIRM_YES": "offline data restore: Windows RE confirm result=6 (6=yes)",
    "GUI_LOG_CHOICE_RE": "offline data restore: user chose Windows RE (prepare chain)",
    "GUI_LOG_BLOCKED": "GUI action blocked",
    "GUI_LOG_ERROR": "[ERROR]",
    # ---- 备份入口 ----
    "GUI_LOG_CANCELLED": "GUI action cancelled: destructive confirmation; operation=backup",
    "GUI_LOG_SYSTEM_CHOICE_ANY": "system drive choice returned:",
    "GUI_LOG_SYSTEM_CHOICE_RE": "system drive choice returned: 2 (0=cancel 1=PE 2=RE)",
    "GUI_LOG_SYSTEM_CONFIRM_YES": "system drive operation: Windows RE confirm result=6 (6=yes)",
    "GUI_LOG_PREPARE_LAUNCH": "elevated prepare: launching (visible) operation=backup",
    "GUI_LOG_PREPARE_FAILED_EARLY": "elevated prepare failed early",
    "GUI_LOG_DELEGATED": "GUI action delegated: elevated prepare launched; operation=backup",
    "GUI_LOG_ONLINE_SPLIT": "online operation: affected=",
    "GUI_LOG_ONLINE_STARTED": "online started: op=backup",
    "GUI_LOG_ONLINE_DONE": "online completed: success=true",
    "GUI_LOG_ONLINE_FAILED": "online completed: success=false",
    "GUI_LOG_ACTION_FAILED": "GUI action failed",
    # ---- 「还原数据到非 C 盘」在线、不格式化（tools/vm/gui-data-restore.sh online）----
    "GUI_LOG_CANCELLED_ANY": "GUI action cancelled: destructive confirmation",          # 任何入口在破坏性确认上答了「否」
    "GUI_LOG_CHANNEL_ONLINE": "channel split: op=restore-data online=true",             # 数据入口分流到了在线通道
    "GUI_LOG_ONLINE_PARAMS_NO_FORMAT": "online params: format_target=false",            # 在线通道收到的是「不格式化」
    "GUI_LOG_ONLINE_RESTORE_STARTED": "online started: op=restore-existing",            # 在线还原已开始
    # ---- 界面「说不」矩阵（tools/vm/gui-negative.sh）：用户点「否」或「取消」之后界面自己记下的那一行 ----
    "GUI_LOG_CANCELLED_RESTORE_EXISTING": "GUI action cancelled: destructive confirmation; operation=restore-existing",
    "GUI_LOG_CANCELLED_RESTORE_DATA": "GUI action cancelled: destructive confirmation; operation=restore-data",
    "GUI_LOG_CANCELLED_CREATE_SECONDARY": "GUI action cancelled: destructive confirmation; operation=create-secondary",
    "GUI_LOG_OVERLAY_DECLINED": "GUI action blocked: overlay restore (wimlib route) declined by the user",
    "GUI_LOG_OFFLINE_CONFIRM_NO": "offline data restore: Windows RE confirm result=7 (6=yes)",
    "GUI_LOG_OFFLINE_CONFIRM_DECLINED": "offline data restore: Windows RE confirm declined",
    "GUI_LOG_METADATA_DECLINED": "restore cancelled: metadata check declined",
    "GUI_LOG_IMAGE_ON_SOURCE_BLOCKED": "GUI action blocked: image volume matches source volume",
    "GUI_LOG_PATH_GUIDANCE_CANCELLED": "GUI image path guidance: user choice=0",
    "GUI_LOG_SYSTEM_CHOICE_CANCELLED": "system drive operation cancelled by user",
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
