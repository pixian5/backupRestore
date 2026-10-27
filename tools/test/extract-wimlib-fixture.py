#!/usr/bin/env python3
"""从超大的真实 wimlib 恢复日志里抽出入库规模的夹具（逐字节保真）。

为什么需要它
------------
`text_parsing::replay_probe::replay_real_wimlib_log` 用 2026-10-23 的端到端
恢复日志做回放取证，断言三条：每条 wimlib 进度片段都能解析出百分比、
同一次 apply 内进度不倒退、每次 apply 都走到 100%。

那份日志 **82 MB / 224 万行**（进度片段用 `\\r` 原地刷新，全挤在少数几个
「行」里），不可能入库。而原用例在日志不在时直接 `return`（= 静默通过），
于是这条判据在 CI 上是**恒绿但零信息**，在本机也可能悄悄消失。

这个脚本把日志压缩成几十 KB 的**逐字节样本**：每个 apply 段、每个阶段
保留首尾与均匀抽样的原始片段（原样保留 `\\r` 分隔与原始字节，不做
解码-重编码，避免把控制符或编码细节改掉）。

用法
----
    python3 tools/test/extract-wimlib-fixture.py \
        --log .test-artifacts/e2e-brimglog-227.log \
        --out crates/backuprestore-cli/tests/fixtures/wimlib-progress-227.log

换一份新日志时重跑即可（`--per-phase` 控制每阶段抽样条数，默认 10）。
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

# 与 text_parsing::parse_wimlib_progress 认识的三个阶段保持一致。
# 这里故意**重复一份常量**而不是去解析 Rust 源码：脚本只是取样器，
# 认不出第三阶段时的表现是「该阶段取样为 0」，会被下面的自检拦下。
PHASE_MARKERS = ("Creating files:", "Extracting file data:", "Applying metadata to files:")


def split_tokens(raw: bytes) -> list[bytes]:
    """按 `\\r` 与 `\\n` 切开——与 Rust 侧 `split(['\\n', '\\r'])` 同规则。"""
    out: list[bytes] = []
    buf = bytearray()
    for byte in raw:
        if byte in (0x0D, 0x0A):
            if buf:
                out.append(bytes(buf))
                buf.clear()
        else:
            buf.append(byte)
    if buf:
        out.append(bytes(buf))
    return out


def decode(token: bytes) -> str:
    return token.decode("utf-8", errors="replace")


def progress_percent(text: str) -> int | None:
    """复刻 parse_wimlib_progress 的取值方式：最后一个 '(' 到其后第一个 '%'。"""
    open_at = text.rfind("(")
    if open_at < 0:
        return None
    pct_at = text.find("%", open_at)
    if pct_at < 0:
        return None
    digits = "".join(c for c in text[open_at + 1 : pct_at] if c.isdigit() or c == ".")
    try:
        value = float(digits)
    except ValueError:
        return None
    if not 0.0 <= value <= 100.0:
        return None
    return int(value)


def phase_of(text: str) -> str | None:
    for marker in PHASE_MARKERS:
        if marker in text:
            return marker
    return None


def pick(samples: list[bytes], limit: int) -> list[bytes]:
    """首尾必留，中间均匀抽样，保持原始顺序。"""
    if len(samples) <= limit:
        return samples
    if limit <= 2:
        return [samples[0], samples[-1]]
    step = (len(samples) - 1) / (limit - 1)
    idx = sorted({round(i * step) for i in range(limit)})
    return [samples[i] for i in idx]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--per-phase", type=int, default=10, help="每个阶段每段保留的条数")
    args = parser.parse_args()

    if not args.log.is_file():
        print(f"ERROR: 日志不存在：{args.log}", file=sys.stderr)
        return 2

    tokens = split_tokens(args.log.read_bytes())
    if not tokens:
        print("ERROR: 切不出任何片段（空文件？）", file=sys.stderr)
        return 2

    # 按 "Applying image" 分段；段内的进度片段按阶段归桶。
    segments: list[dict[str, list[bytes]]] = []
    current: dict[str, list[bytes]] | None = None
    for token in tokens:
        text = decode(token)
        if "Applying image" in text:
            current = {"header": [token], "Creating files:": [], "Extracting file data:": [], "Applying metadata to files:": []}
            segments.append(current)
            continue
        phase = phase_of(text)
        if phase is None:
            continue
        # 只留**解析得出百分比**的片段：Rust 侧断言「每条命中 parse_wimlib_progress 的
        # 片段都必须解析出百分比」，混入解析不出的样本会让夹具与真实日志不等价。
        if progress_percent(text) is None:
            continue
        if current is None:
            current = {"header": [], "Creating files:": [], "Extracting file data:": [], "Applying metadata to files:": []}
            segments.append(current)
        current[phase].append(token)

    # 自检：真实日志里**不是每个 apply 段都带 wimlib 进度**（2026-10-23 那份 24 段
    # 里只有最后 2 段有，各 2,639 条）。所以只输出带进度的段——把只有 header 的
    # 空段写进夹具，等于给回放造噪声。但「一段都没有」是真问题，必须报错。
    segments = [s for s in segments if any(s[phase] for phase in PHASE_MARKERS)]
    if not segments:
        print("ERROR: 没有任何 apply 段含 wimlib 进度片段（取样器与日志不匹配？）", file=sys.stderr)
        return 2
    empty = [
        (index, phase)
        for index, segment in enumerate(segments)
        for phase in PHASE_MARKERS
        if not segment[phase]
    ]

    # 一次构造成文本：段内用 `\r`（进度原地刷新，与真实日志同形态），
    # 段间用 `\n`（真实日志里段边界本来就在换行之后）。
    # ★ 不要「先拼一个长列表再按区间切片」——那等于把同一段布局写两遍，
    #   两处一旦不同步就是静默错位（BR-IMG-008 的教训）。
    blocks: list[bytes] = []
    kept = 0
    for segment in segments:
        chunks: list[bytes] = []
        if segment["header"]:
            chunks.append(segment["header"][0])
        for phase in PHASE_MARKERS:
            picked = pick(segment[phase], args.per_phase)
            chunks.extend(picked)
            kept += len(picked)
        blocks.append(b"\r".join(chunks))
    body = b"\n".join(blocks)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_bytes(body + b"\n")

    print(f"apply 段数={len(segments)} 保留片段={kept} 空阶段={empty or '无'}")
    print(f"输出：{args.out}  {args.out.stat().st_size:,} 字节")
    if empty:
        print("WARN: 有阶段取不到样本，夹具覆盖不全（换 --per-phase 或检查日志）", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
