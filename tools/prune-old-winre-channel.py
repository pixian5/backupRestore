#!/usr/bin/env python3
"""按「顶层项」精确删除 main.rs 里的旧 WinRE 通道机制。

用括号配平找每个 fn/struct/impl 的起止，不靠行号猜区间；同时吃掉紧邻的
文档注释与属性，避免留下孤儿片段。删除前会打印将要删除的项名供核对。
"""
import re
import sys
import pathlib

PATH = pathlib.Path("crates/backuprestore-cli/src/main.rs")
text = PATH.read_text()
lines = text.split("\n")

# 1) 找出所有顶层项（行首非空、以 fn/pub fn/pub(crate) fn/struct/impl 开头）
item_starts = []
for i, line in enumerate(lines):
    if re.match(r"^(pub(\(crate\))? )?(fn|struct|impl|const|static|mod|enum) ", line):
        item_starts.append(i)


def item_end(start: int) -> int:
    """从 start 行的第一个 '{' 开始配平，返回项之后的第一行（含尾部空行）。"""
    depth = 0
    seen = False
    i = start
    while i < len(lines):
        for ch in lines[i]:
            if ch == "{":
                depth += 1
                seen = True
            elif ch == "}":
                depth -= 1
        if seen and depth <= 0:
            # 吃掉紧随的空行
            j = i + 1
            while j < len(lines) and lines[j].strip() == "":
                j += 1
            return j
        i += 1
    raise RuntimeError(f"unbalanced item at line {start + 1}")


# 无花括号的顶层项（const 等）：到下一个空行为止
def item_end_flat(start: int) -> int:
    j = start + 1
    while j < len(lines) and lines[j].strip() != "":
        j += 1
    while j < len(lines) and lines[j].strip() == "":
        j += 1
    return j


# 2) 建立「项名 -> (start_with_attrs, end)」
items = {}
for idx, start in enumerate(item_starts):
    name_m = re.match(r"^(?:pub(?:\(crate\))? )?(?:fn|struct|impl|const|static|mod|enum) (\w+)", lines[start])
    name = name_m.group(1) if name_m else f"<{idx}>"
    end = item_end(start) if "{" in "\n".join(lines[start:start + 40]) else item_end_flat(start)
    # 向前吞属性与文档注释
    s = start
    while s > 0 and (lines[s - 1].startswith("#[") or lines[s - 1].startswith("///") or lines[s - 1].startswith("//")):
        s -= 1
    if name in items:
        name = f"{name}#{idx}"
    items[name] = (s, end)

TARGETS = [
    "WinreRestoreGuard",
    "finalize_winre_after_task",
    "ensure_registered_is_payload",
    "read_resume_artifacts",
    "try_ensure_registered_is_payload",
    "write_env_values",
    "evacuate_registered_winre",
    "winre_role_conflict_at_execution",
    "restore_clean_winre_before_capture",
    "restore_original_winre_at",
    "restore_original_winre_for_task",
    "winre_registered_at_volume",
    "finish_pending_winre_rehome",
    "finish_pending_winre_rehomes",
    "winre_rehome",
    "finalize_evacuated_winre",
    "sanitize_scratch_winre",
    "write_winre_rehome_marker",
    "try_restore_original_winre_from_task",
    "winre_home_volume_from_env",
    "winre_task_is_evacuated",
]

missing = [n for n in TARGETS if n not in items]
if missing:
    print("NOT FOUND:", missing)
    sys.exit(1)

spans = sorted((items[n][0], items[n][1], n) for n in TARGETS)
print("will delete:")
for s, e, n in spans:
    print(f"  {n}: lines {s + 1}-{e}")

merged = []
for s, e, n in spans:
    if merged and s <= merged[-1][1]:
        merged[-1][1] = max(merged[-1][1], e)
        merged[-1][2].append(n)
    else:
        merged.append([s, e, [n]])

out = []
prev = 0
for s, e, names in merged:
    out.extend(lines[prev:s])
    prev = e
out.extend(lines[prev:])
PATH.write_text("\n".join(out))
print(f"deleted {len(TARGETS)} items; {len(lines)} -> {len(out)} lines")
