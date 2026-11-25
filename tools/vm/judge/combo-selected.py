# 从「读取下拉框」辅助脚本（comboitems.ps1）的输出里取出**当前选中项**的文字。标准输入 = 那份输出（已解码）。
# 被 tools/vm/lib/common.sh 的 gui_combo_selected 调用；tools/vm/selftest.sh 用人工拼的清单离线验证它。
#
# 输出格式（comboitems.ps1）：
#   COMBO id=1202 count=4 selected=0 visible=True
#     ITEM[0]=C: | NTFS | 卷标 无卷标 | …
#     ITEM[1]=…
# 标准输出 = 选中项的文字，不带换行。没有选中项（selected=-1）、清单读不到、选中项没有对应的条目：输出空串。
#
# ★ 调用方用之前必须先确认输出不是空串：读不到的时候「以 X: 开头」这类判断会全部落空。
import re
import sys


def selected_text(text):
    selected = None
    items = {}
    for line in text.splitlines():
        line = line.rstrip("\r")
        m = re.search(r"selected=(-?\d+)", line)
        if m:
            selected = int(m.group(1))
        m = re.match(r"\s*ITEM\[(\d+)\]=(.*)", line)
        if m:
            items[int(m.group(1))] = m.group(2)
    if selected is None or selected < 0:
        return ""
    return items.get(selected, "")


if __name__ == "__main__":
    sys.stdout.buffer.write(selected_text(sys.stdin.buffer.read().decode("utf-8", "replace")).encode("utf-8"))
