#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""check-doc-links.py —— 文档相对链接的入库检查（判据，不是清理工具）

背景（docs/20261026-2029-AGENTS.md过时条款重写）：
    10-26 发现 `文档索引.md` 给 `AGENTS.md` 写了相对链接，而 `AGENTS.md` 在
    `.gitignore:135` 里、从未入库。本机上点得开，任何「链接失效 0」的校验都通过；
    但别人 `git clone` 之后那个文件不存在，链接必然 404。
    ⇒「本机自洽」证明不了「仓库可用」，判据必须同时问「目标是否进了 git」。

两类问题分开报，因为修法完全不同：
    MISSING   = 目标文件在本机就不存在  → 链接写错了 / 文件被删
    UNTRACKED = 目标存在但没进 git      → 要么 commit 它，要么把链接降级成纯文本

为什么不用纯 bash（原方案是 `git grep -oh '](\\{0,1\\}[^)]*\\.md)' | ...`）：
    它会把**行内代码里的示例**当成真链接。10-26 实测唯一命中就是复盘文档里
    引用旧写法的反引号片段（`\\| [AGENTS.md](AGENTS.md) \\|`）——假阳性。
    判据里的假阳性会训练人忽略判据，所以必须跳过 fenced code block 与行内代码。

用法：
    python3 tools/doc/check-doc-links.py            # 只看报告，永远 exit 0
    python3 tools/doc/check-doc-links.py --strict   # 有 MISSING/UNTRACKED → exit 1

退出码：
    0 = 无问题（或非 strict）
    1 = 有问题（strict 模式）
    2 = 仪器故障：扫到 0 个链接 / 不在 git 仓库里（**不能当成「通过」**）

★ 本项目的铁律：读不到 ≠ 判据满足。解析器崩了必须 exit 2，不许输出一个
   好看的 `MISSING=0`。
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import urllib.parse

# ── Markdown 链接的两种写法 ───────────────────────────────────────────────────
# 行内式：[文字](目标 "可选标题")   引用式定义：[id]: 目标
RE_INLINE = re.compile(r"\[[^\]\n]*\]\(\s*(<[^>\n]*>|[^)\s]+)(?:\s+\"[^\"]*\")?\s*\)")
RE_REFDEF = re.compile(r"^\s{0,3}\[[^\]\n]+\]:\s*(\S+)")
RE_FENCE = re.compile(r"^\s{0,3}(```|~~~)")

# 行内代码（`...` 与 ``...``）：链接出现在里面就是示例，不是真链接
RE_INLINE_CODE = re.compile(r"(`+)(?:.+?)\1")

# 外部协议与本地绝对路径：都不是「仓库内相对链接」，跳过
SKIP_PREFIX = ("http://", "https://", "//", "mailto:", "tel:", "ftp://", "data:", "file:")

# ── 判据分级：导航文件严格，历史归档只报告 ────────────────────────────────────
# 为什么必须分级：
#   ① `docs/` 下的历史记录是**当天的快照**，里面引用的文件后来被删/改名是正常的
#      （实测 6 条：3 条指向已删的 docs/README.md、2 条指向搬迁前的代码路径、
#       1 条指向已改名的设计文档）。要求它们全绿，只会逼人去篡改历史记录。
#   ② 导航文件（README / 文档索引 / 经验）是**给人顺着走的路**，断一条就少一条路。
#   ③ `UNTRACKED` 与位置无关：任何文档链接到不入库文件，clone 后都是 404，一律阻塞。
NAV_EXACT = {"README.md", "文档索引.md", "BACKLOG.md"}
NAV_PREFIXES = ("经验/",)


def is_nav(path: str) -> bool:
    return path in NAV_EXACT or path.startswith(NAV_PREFIXES)


def git(*args: str) -> str:
    """跑一条 git 命令，失败返回空串（调用方自己判断）。"""
    try:
        out = subprocess.run(
            ["git", *args], capture_output=True, text=True, check=False
        )
        return out.stdout
    except FileNotFoundError:
        return ""


def strip_code(text: str) -> str:
    """去掉 fenced code block 与行内代码，只留下「真的会被渲染成链接」的部分。

    不做语法高亮级别的解析——判据只需要「不误报」，不需要完美。
    """
    kept: list[str] = []
    in_fence = False
    for line in text.splitlines():
        if RE_FENCE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        kept.append(RE_INLINE_CODE.sub("", line))
    return "\n".join(kept)


def normalize(target: str, source: str) -> str | None:
    """把链接目标变成相对仓库根的真实路径；返回 None 表示不该检查。"""
    t = target.strip()
    if t.startswith("<") and t.endswith(">"):
        t = t[1:-1].strip()
    if not t or t.startswith(SKIP_PREFIX) or t.startswith("#"):
        return None
    t = t.split("#", 1)[0]           # 去掉锚点
    t = t.split("?", 1)[0]           # 去掉查询串
    if not t:
        return None
    # ★ 必须做**完整**百分号解码：中文文件名在链接里是 %E9%A1%B9%E7%9B%AE… 这种，
    #   只替换 %20 会把 60 多个合法链接误报成 MISSING（首跑实测就是这个假阳性）。
    t = urllib.parse.unquote(t)
    if t.startswith("/"):            # 站内绝对路径，本项目不用，跳过
        return None
    # 相对「源文件所在目录」解析
    base = os.path.dirname(source)
    return os.path.normpath(os.path.join(base, t))


def main() -> int:
    strict = "--strict" in sys.argv[1:]
    root = git("rev-parse", "--show-toplevel").strip()
    if not root:
        print("ERROR: 不在 git 仓库里，无法判断「是否入库」", file=sys.stderr)
        print("DONE")
        return 2
    os.chdir(root)

    # 只扫**已入库**的 md：未入库文档的链接烂掉不影响 clone 后的仓库
    tracked_md = [p for p in git("ls-files", "-z", "*.md").split("\0") if p]
    if not tracked_md:
        print("ERROR: git ls-files 没返回任何 md 文件", file=sys.stderr)
        print("DONE")
        return 2

    tracked_set = set(tracked_md)
    issues: list[tuple[str, str, str]] = []      # (kind, target, source:line)
    seen: set[tuple[str, str]] = set()
    total_links = 0

    for src in tracked_md:
        try:
            with open(src, "r", encoding="utf-8", errors="replace") as f:
                raw = f.read()
        except OSError as exc:                  # 读不到 → 仪器故障，不是「没问题」
            print(f"ERROR: 读不到 {src}: {exc}", file=sys.stderr)
            print("DONE")
            return 2
        body = strip_code(raw)
        raw_lines = raw.splitlines()
        for lineno, line in enumerate(body.splitlines(), start=1):
            targets = [m.group(1) for m in RE_INLINE.finditer(line)]
            ref = RE_REFDEF.match(line)
            if ref:
                targets.append(ref.group(1))
            for target in targets:
                norm = normalize(target, src)
                if not norm:
                    continue
                total_links += 1
                key = (norm, src)
                if key in seen:
                    continue
                seen.add(key)
                if not os.path.exists(norm):
                    kind = "MISSING"
                elif norm not in tracked_set and not os.path.isdir(norm):
                    # 非 md 目标（脚本/图片/资源）用 ls-files 全量判断
                    if not git("ls-files", "--error-unmatch", norm).strip():
                        kind = "UNTRACKED"
                    else:
                        continue
                elif norm not in tracked_set and os.path.isdir(norm):
                    # 目录：只要目录下有已入库文件就算进库
                    if any(p.startswith(norm + "/") for p in tracked_set):
                        continue
                    kind = "UNTRACKED"
                else:
                    continue
                issues.append((kind, norm, f"{src}:{lineno}"))

    missing = [i for i in issues if i[0] == "MISSING"]
    untracked = [i for i in issues if i[0] == "UNTRACKED"]
    # 阻塞项：导航文件的死链 + 任何位置的「未入库」目标
    missing_nav = [i for i in missing if is_nav(i[2].rsplit(":", 1)[0])]
    archive_missing = [i for i in missing if not is_nav(i[2].rsplit(":", 1)[0])]

    print(f"MD_TRACKED={len(tracked_md)}")
    print(f"LINKS_CHECKED={total_links}")
    print(f"MISSING={len(missing)}")
    print(f"MISSING_NAV={len(missing_nav)}")
    print(f"ARCHIVE_MISSING={len(archive_missing)}")
    print(f"UNTRACKED={len(untracked)}")
    for kind, target, where in issues:
        print(f"ISSUE kind={kind} target={target} source={where}")
    print("DONE")

    # ★ 仪器自检：一个链接都没扫到 = 解析器坏了，不能输出「全绿」
    if total_links == 0:
        print("ERROR: LINKS_CHECKED=0，解析器没工作（不是「没问题」）", file=sys.stderr)
        return 2

    blocking = len(missing_nav) + len(untracked)
    if strict and blocking:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
