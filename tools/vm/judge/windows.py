# 比较界面进程「点击之前」与「点击之后」的两份顶层窗口清单，回答「点完之后界面上多出了什么」。
# 清单来自 tools/vm/lib/common.sh 的 gui_windows（已经解码）。
# 被 tools/vm/gui-backup.sh 调用；tools/vm/selftest.sh 用人工拼的清单离线验证它。
#
# 用法：python3 judge/windows.py <之前的清单> <之后的清单>
# 输出（一行一个键值）：
#   WINDOWS_BEFORE=<n> / WINDOWS_AFTER=<n>     两份清单里各有多少个可见顶层窗口
#   WINDOWS_READ=both|before_only|after_only|none   哪一份读到了（没有 WINDOWS= 行 = 那一次读数没执行）
#   MESSAGE_BOXES_AFTER=<n>                    之后的清单里还挂着的消息框（类 #32770）个数
#   NEW_WINDOW_CLASSES_AFTER=<类名,类名>        之后有、之前没有的窗口类，按类名比较（同一类出现多次只列一次）
#   WINDOW_LINE <原文>                          之后的清单里的窗口行（供人看）
#
# ★ 「之后的清单读不到」不等于「之后没有窗口」：读不到时 MESSAGE_BOXES_AFTER 输出 <unreadable>，
#   由调用方记成中止，不记成通过。
import os
import sys

MESSAGE_BOX_CLASS = "#32770"


def windows(path):
    """返回 (读到没有, [类名, …])。文件不存在或没有 WINDOWS= 行都算没读到。"""
    if not path or not os.path.exists(path):
        return False, []
    classes = []
    seen_header = False
    for line in open(path, "rb").read().decode("utf-8", "replace").splitlines():
        if line.startswith("WINDOWS="):
            seen_header = True
        elif line.startswith("WIN hwnd="):
            rest = line[len("WIN hwnd="):]
            if " class=" in rest:
                classes.append(rest.split(" class=", 1)[1].split(" ", 1)[0])
    return seen_header, classes


def judge(before_path, after_path, out=print):
    read_before, before = windows(before_path)
    read_after, after = windows(after_path)
    out("WINDOWS_BEFORE=%d" % len(before))
    out("WINDOWS_AFTER=%d" % len(after))
    if read_before and read_after:
        state = "both"
    elif read_before:
        state = "before_only"
    elif read_after:
        state = "after_only"
    else:
        state = "none"
    out("WINDOWS_READ=%s" % state)
    if read_after:
        out("MESSAGE_BOXES_AFTER=%d" % sum(1 for c in after if c == MESSAGE_BOX_CLASS))
        new = []
        for c in after:
            if c not in before and c not in new:
                new.append(c)
        out("NEW_WINDOW_CLASSES_AFTER=%s" % ",".join(new))
    else:
        out("MESSAGE_BOXES_AFTER=<unreadable>")
        out("NEW_WINDOW_CLASSES_AFTER=<unreadable>")
    return read_before and read_after


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: judge/windows.py <listing before> <listing after>")
    ok = judge(sys.argv[1], sys.argv[2])
    sys.exit(0 if ok else 1)
