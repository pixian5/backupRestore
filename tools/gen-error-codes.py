#!/usr/bin/env python3
"""从 `error_codes!` 宏体生成 `docs/ERROR-CODES.md`。

**改完 `error_code.rs` 的表就跑一次**，否则清单会和代码脱节：

    python3 tools/gen-error-codes.py

为什么不用手写清单：212 条码人工维护必然漏/错，而这份清单是用户报障时
对照用的——错一条就等于报障对不上号。唯一的真相源是宏体，清单只是投影。

退出码：0 正常；1 表解析失败（格式被改动时能用它及早发现）。
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "crates/backuprestore-cli/src/error_code.rs"
OUTPUT = ROOT / "docs/ERROR-CODES.md"

DOMAIN_NAMES = {
    "PE": "PE 通道与写盘契约",
    "RE": "恢复环境注册 / WinRE 关联",
    "VOL": "卷身份与盘符",
    "TSK": "任务、状态机、续跑",
    "IMG": "镜像文件与 DISM 探测",
    "APL": "应用镜像（DISM / wimlib）",
    "BOOT": "引导、BCD、启动菜单",
    "GUI": "图形界面",
}

# 结构性码：`TaskError::code()` 直接返回，不在宏表里，但要在清单里出现。
STRUCTURAL = [
    ("BR-IO-001", "`Io(_)`", "文件/系统调用失败（文案随系统语言，如「系统找不到指定的路径」）"),
    ("BR-IO-002", "`Json(_)`", "JSON 读写/解析失败"),
    ("BR-TSK-028", "`InvalidTransition{..}`", "状态机非法迁移"),
]

ENTRY = re.compile(
    r'(\w+) => \("(BR-[A-Z]+-\d{3})", "((?:[^"\\]|\\.)*)", "((?:[^"\\]|\\.)*)"\)'
)


def parse_entries(text: str):
    try:
        start = text.index("error_codes! {")
    except ValueError:
        print("找不到 error_codes! 宏体", file=sys.stderr)
        sys.exit(1)
    body = text[start : text.index("\n}\n", start)]
    entries = ENTRY.findall(body)
    if not entries:
        print("宏体里一条都没解析到——是不是改了条目格式？", file=sys.stderr)
        sys.exit(1)
    return entries


def build(entries) -> str:
    domains = {}
    for variant, code, zh, en in entries:
        domains.setdefault(code.split("-")[1], []).append((code, variant, zh, en))

    lines = [
        "# 错误码清单（`BR-<域>-<序号>`）",
        "",
        "> **自动生成，别手改**：`python3 tools/gen-error-codes.py`（真相源是"
        " `crates/backuprestore-cli/src/error_code.rs` 的 `error_codes!` 宏体）。",
        "> **码是契约**——发布后不得重排、不复用退役号；用户报障直接报码即可定位到唯一一行。",
        "",
        "## 怎么用",
        "",
        "| 场景 | 怎么做 |",
        "|---|---|",
        "| 用户报障 | 让 TA 报 `BR-xxx-nnn`（或截 `error:` 那行的方括号前缀） |",
        '| 翻日志 | `grep -a "\\[BR-" Recovery.log` |',
        "| 自动化判据 | 判 `--json` 的 `error_code` 字段，**不要**解析 `error` 文案 |",
        "| 看中文/英文原文 | 查下表，或直接 grep 码 |",
        "",
        "## 结构性码（`TaskError` 的结构变体，`Display` 不加前缀）",
        "",
        "由 `TaskError::code()` 直接返回，只在 `--json` 的 `error_code` 字段里出现——",
        "`Display` 不加 `[码] ` 前缀（文案是 OS 本地化串或 serde 原文，加前缀只会更难看）。",
        "",
        "| 码 | 变体 | 含义 |",
        "|---|---|---|",
    ]
    for code, variant, meaning in STRUCTURAL:
        lines.append(f"| `{code}` | {variant} | {meaning} |")
    lines += [
        "",
        "自动化用它区分**环境失败**（`BR-IO-*`）与**脚本写错**（`error_code` 为 `null`）。",
        "",
        "## core 层（`BR-CORE-NNN`）",
        "",
        "core 的码由 `TaskError::coded(\"BR-CORE-NNN\", …)` 直接写在构造点，",
        "文案是英文技术诊断（core 没有语言层）。**77 个语义码**，直接 grep 码定位：",
        "",
        "```",
        'grep -rn "BR-CORE-" crates/backuprestore-core/src/',
        "```",
        "",
        f"## CLI 层（共 {len(entries)} 条）",
        "",
    ]
    for domain in sorted(domains):
        items = domains[domain]
        lines.append(
            f"### `BR-{domain}` — {DOMAIN_NAMES.get(domain, domain)}（{len(items)} 条）"
        )
        lines += ["", "| 码 | 变体 | 中文 | English |", "|---|---|---|---|"]
        for code, variant, zh, en in items:
            lines.append(
                f"| `{code}` | `{variant}` | {zh.replace('|', chr(92) + '|')} "
                f"| {en.replace('|', chr(92) + '|')} |"
            )
        lines.append("")
    lines += [
        "## 刻意无码的一类（不会出现在上表）",
        "",
        "命令行**用法**错误（`task id is required`、`--target-drive is required`、",
        "`unsupported operation: xxx`，共 300+ 处）**故意不发码**：",
        "",
        "1. 本来就是英文技术提示，不需要翻译；",
        "2. 文案本身唯一且稳定，贴文案即可定位；",
        "3. 给 300 多条用法提示各发一个码，只会淹没真正有价值的运行期码。",
        "",
        "判读规则：**`err()` = 用法错误（无码）；`errc`/`errc_with` = 带码运行期失败。**",
        "",
    ]
    return "\n".join(lines)


def main() -> int:
    entries = parse_entries(SOURCE.read_text(encoding="utf-8"))
    OUTPUT.write_text(build(entries), encoding="utf-8")
    print(f"写入 {OUTPUT.relative_to(ROOT)}：{len(entries)} 条 CLI 码 + {len(STRUCTURAL)} 个结构性码")
    return 0


if __name__ == "__main__":
    sys.exit(main())
