#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""reverify-map.py —— 「改动-复验对应表」的机器版：改了这些文件，必须重跑哪些实机项

背景：
    测试台账（docs/20261029-2329-项目全景与测试台账）§九 有两张「改动-复验对应表」，是给人读的：
    定位这次改的是哪个模块 → 只重跑右列。
    2026-11-24 的教训：靠人去对照，漏了一半。改了备份链 `recovery_backup.rs`，表上明明写着
    「至少一轮系统盘备份**和**一轮系统盘还原」，结果备份跑了三轮、还原一轮没跑，
    是事后写文档时人工对表才发现的。
    ⇒ 把对照做成脚本：输入基线标签，输出必须重跑的项，以及**这张表没覆盖到的改动文件**。

与台账 §九 的关系：
    规则的唯一数据源是本文件的 RULES（含各条的前提说明）；台账 §九 是人读版。
    改规则时两处一起改；`--dump-table` 会按当前规则打印一张 Markdown 表，方便与台账逐行比对。

用法：
    python3 tools/test/reverify-map.py                              # 从默认基线标签到 HEAD 的已提交改动
    python3 tools/test/reverify-map.py --since <标签或提交> [--head <提交>] [--worktree]
                                                                      # --worktree：把未提交的改动也算上
    python3 tools/test/reverify-map.py --files a.rs b.md ...        # 不看 git，直接给文件列表（假设性分析）
    python3 tools/test/reverify-map.py --selftest                   # 自检：固定用例 + 一条变异验证
    python3 tools/test/reverify-map.py --dump-table                 # 打印当前规则的 Markdown 表

输出（键用 ASCII，便于 grep；说明用中文）：
    CHANGED=<路径>
    REVERIFY=<条目编号>  <说明>                  无条件必跑
    REVERIFY=<条目编号>  when=<前提>  <说明>     有前提的：只有满足前提才要跑
    UNMAPPED=<路径>                              这张表没覆盖到的改动文件——要人来判断，不是「不用重跑」
    NOTE=<...>
    REVERIFY_COUNT=<n> UNMAPPED_COUNT=<n>
    RESULT=NO_PRODUCT_CHANGE | CLEAN | MAPPED_WITH_UNMAPPED

退出码：
    0 = 跑通了（无论结果如何）。它是**报告型**工具，不进 pre-push——
        一直红的检查最后会被整个删掉，比没有更坏（见 经验/06 §6）。
    2 = 仪器故障：不在 git 仓库里 / 基线标签不存在 / 读文件名时 git 报错。
        ★ 本项目的铁律：读不到 ≠ 没有改动。读不到必须 exit 2，不许输出一个好看的 RESULT=CLEAN。

★ 三条设计约束：
    ① 没覆盖到的文件必须报出来（UNMAPPED），不能静默忽略。
    ② 「有前提的项」与「无条件必跑的项」分开输出，并带上前提原文——不要让脚本替人决定前提成不成立。
    ③ 自检里带一条变异验证：拆掉一条规则，对应的必跑项必须从输出里消失。不变红的判据等于没有判据。
"""

from __future__ import annotations

import fnmatch
import subprocess
import sys

DEFAULT_SINCE = "verified/vm-20261124"
CLI = "crates/backuprestore-cli/src/"
CORE = "crates/backuprestore-core/src/"

# ── 条目：编号 → 说明（输出顺序以此为准）────────────────────────────────────
ITEMS = {
    "win-backup": "系统盘备份经恢复环境一轮",
    "win-restore": "系统盘还原经恢复环境一轮：RESTORE_SELFCHECK 命中、BCD 清理回读、status=success/100",
    "core-unit": "核心库：macOS 与 Windows 两处单元测试 + 变异验证",
    "smoke-status": "一轮冒烟，看 status.json 契约（stage / progress / errorCode）",
    "hash-backend": "改到哈希入口：恢复环境里一轮备份，日志有 Image hash computed: backend=bcrypt，哈希三方一致",
    "power-resume": "断电续跑用例",
    "gate-pair": "完整 Windows 门禁双组（缺件硬拒 + 完整镜像放行）与 A 入口硬拒探针",
    "reagentc": "系统盘还原后 reagentc /info Enabled、recoverysequence 核对",
    "missing-re-047": "缺件场景必须是 BR-RE-047，且一次注册写入都没有",
    "b-entry": "B 入口数据还原：离线那一条跑 tools/vm/data-restore.sh（哨兵法）；在线不格式化 / 在线格式化两条暂无脚本",
    "c-entry-online": "C 入口在线还原到非 C 盘",
    "overlay": "覆盖式还原：六条判据 + 保留访问控制列表的逐字节比对 + 进度条四帧单调",
    "pe-chain": "装预安装环境全链 + 装前自检（换版本必须重出镜像）",
    "gui-matrix": "界面全矩阵：tools/vm/gui-matrix.sh（约 15 分钟；--quick 约 1 分钟）",
    "gui-confirm": "纯净界面三道确认（含最后点「否」的取消路径）：tools/vm/gui-fix-check.sh",
    "dpi-200": "200% 缩放下截图核对",
    "parse-unit": "解析类代码：macOS 与 Windows 两处单元测试 + 变异验证",
    "progress-pe": "恢复进度窗：预安装环境内目验 + 冒烟；按 B 回退的三条日志",
    "bcd-roundtrip": "启动配置：bcdedit /export 备份 → 改 → /import 回导 → 重启验证",
    "second-system": "拒绝路径三守卫（不写一个字节）+ V2 还原到第二系统全链 + 终态默认项与序列号",
    "b-key": "按 B 回退一轮",
    "deploy": "虚拟机内重新构建部署，并通过 tools/vm/preflight.sh：跑 tools/vm/rebuild-deploy.sh（先提交后部署）",
    "mutation": "判据脚本自身：变异验证（拆守卫或改值后新判据变红，恢复后变绿）；改了 tools/vm 先跑 tools/vm/selftest.sh",
    "doc-gates": "文档：tools/doc/check-doc-links.py --strict 三个字段归零 + tools/doc/stale-claims.sh",
    "table-unit": "错误码与日志文案两张表：macOS 与 Windows 两处单元测试（码唯一、中英占位一致）；改了错误码要重新生成 docs/ERROR-CODES.md",
    "cli-probe": "命令行探测命令的冒烟（list-volumes、inspect-environment、wim-info、list-images）：跑 tools/vm/cli-probe.sh",
    "orphan-race": "守卫竞态：准备进行中放弃被拒、启动界面程序守卫不动、准备成功：跑 tools/vm/orphan-race.sh new both（约 4 分钟，重启两次）",
    "guard-unit": "准备锁与孤儿任务守卫：Windows 单元测试 + 变异验证：跑 tools/vm/mutation-guard.sh",
}

# ── 规则：(编号, 匹配模式（| 分隔，fnmatch 语法）, [(条目, 前提或 None), ...], 备注) ──
# 与台账 §九「拆分后模块对照」一一对应；最后几行是本脚本补的（台账没写到的文件类型）。
RULES = [
    ("R01", f"{CORE}fast_hash.rs",
     [("core-unit", None), ("hash-backend", None)], "哈希快速路径（v2.10.7 起）"),
    ("R02", f"{CORE}*.rs",
     [("core-unit", None), ("smoke-status", None)], "核心库其余模块；改到副档契约时补「旧副档兼容」用例"),
    ("R03", f"{CLI}recovery_engine.rs",
     [("win-backup", None), ("win-restore", None), ("b-key", "改到回退或失败策略时")], "恢复环境主链"),
    ("R04", f"{CLI}recovery_backup.rs",
     [("win-backup", None), ("win-restore", None)], "★ 备份链：还原也要重跑（2026-11-24 漏的就是这一半）"),
    ("R05", f"{CLI}recovery_interactive.rs",
     [("win-backup", None), ("win-restore", None), ("b-key", "改到回退或失败策略时")], "恢复环境交互与失败策略"),
    ("R06", f"{CLI}recovery_volume.rs",
     [("win-backup", None), ("win-restore", None)], "恢复环境里的卷解析与挂载"),
    ("R07", f"{CLI}recovery_bcd.rs",
     [("win-backup", None), ("win-restore", None), ("b-key", "改到回退或失败策略时")], "恢复环境里的引导修复"),
    ("R08", f"{CLI}recovery_progress.rs",
     [("win-backup", None), ("win-restore", None), ("progress-pe", None)], "进度窗与后台控制台"),
    ("R09", f"{CLI}pending_task.rs|{CLI}boot_entry.rs|{CLI}boot_cleanup.rs|{CLI}boot_record.rs",
     [("win-backup", None), ("win-restore", None), ("power-resume", None),
      ("orphan-race", "改到孤儿任务守卫时"), ("guard-unit", "改到孤儿任务守卫时")], "启动项与续跑"),
    ("R10", f"{CLI}boot_menu.rs",
     [("win-restore", None), ("bcd-roundtrip", None)], "启动菜单与默认项"),
    ("R11", f"{CLI}prepare_*.rs|{CLI}windows_prepare.rs|{CLI}windows_command.rs|{CLI}process_runner.rs",
     [("win-backup", None), ("win-restore", None), ("gate-pair", "改到门禁时"),
      ("orphan-race", "改到任务落盘、启动项记录、失败补偿的先后时")], "准备链（备份、还原都要经过）"),
    ("R12", f"{CLI}winre_registration.rs",
     [("reagentc", None), ("missing-re-047", None)], "恢复环境注册"),
    ("R13", f"{CLI}online_operation.rs|{CLI}wimlib_apply.rs",
     [("b-entry", None), ("c-entry-online", None), ("overlay", None)], "在线操作与覆盖式通道"),
    ("R14", f"{CLI}pe_operation.rs|{CLI}pe_safety.rs|{CLI}pe_desktop.rs|{CLI}pe_installer.rs|{CLI}native_gui/pe_desktop.rs|{CLI}native_gui/pe_installer.rs",
     [("pe-chain", None)], "预安装环境链"),
    ("R15", f"{CLI}native_gui.rs|{CLI}native_gui/*.rs|{CLI}dpi.rs",
     [("gui-matrix", None), ("gui-confirm", None), ("dpi-200", "动了坐标或字号时")], "界面与缩放"),
    ("R16", f"{CLI}text_parsing.rs|{CLI}*_parse.rs|{CLI}step_progress.rs|{CLI}progress_text.rs",
     [("parse-unit", None), ("progress-pe", "解析结果影响进度显示时")], "解析类"),
    ("R17", f"{CLI}main.rs",
     [("win-restore", None)], "入口分发与恢复环境主链"),
    ("R18", f"{CLI}secondary_registry.rs",
     [("second-system", None)], "第二系统的盘符绑定"),
    ("R19", "tools/test/*|tools/gui-check/*|tools/vm/*|tools/doc/*|tools/release/*|tools/smoke/*|tools/acceptance/*|tools/restore/*|tools/git-hooks/*"
            "|tools/*.ps1|tools/*.sh|tools/*.py|tools/*.cmd|tools/win-clicker/*",
     [("mutation", None), ("deploy", "改的是构建或部署脚本时")], "判据、门禁、驱动与审计脚本自身：改了就要验证它还判得住"),
    ("R20", "windows/wimlib-aarch64/*",
     [("overlay", None), ("deploy", None)], "覆盖式通道的两个二进制"),
    ("R21", "windows/*|build-win.sh",
     [("deploy", None)], "构建脚本与预安装环境镜像"),
    ("R22", "VERSION|Cargo.toml|Cargo.lock|crates/*/Cargo.toml",
     [("deploy", "版本或依赖变了：要重新构建才能产出对应版本的交付包")], "版本与依赖"),
    ("R23", ".github/workflows/*|rust-toolchain.toml",
     [("core-unit", "工具链或持续集成配置变了"), ("parse-unit", "工具链或持续集成配置变了")], "工具链与持续集成"),
    ("R24", "*.md|docs/*|经验/*|日记/*|.gitignore|.gitattributes",
     [("doc-gates", None)], "文档与仓库配置：没有实机项"),
    ("R25", f"{CLI}error_code.rs|{CLI}i18n.rs",
     [("table-unit", None)], "错误码表与日志文案表（2026-11-24 补：此前是表外文件）"),
    ("R26", f"{CLI}volume_device.rs",
     [("cli-probe", None), ("gate-pair", "改到卷身份或驱动器类型的判定时")], "列卷与卷身份（2026-11-24 补：此前是表外文件）"),
    ("R27", f"{CLI}image_admin.rs",
     [("cli-probe", None), ("gate-pair", "改到镜像完整性探测时")], "镜像信息与镜像完整性探测（2026-11-24 补：此前是表外文件）"),
    ("R28", f"{CLI}prepare_lock.rs",
     [("orphan-race", None), ("guard-unit", None)], "准备期在办锁（2026-11-24 实机缺陷后新增的模块）"),
    ("R29", f"{CLI}environment_admin.rs",
     [("cli-probe", None), ("orphan-race", "改到放弃任务的入口时")], "环境探测、启动项枚举与放弃任务（2026-11-24 补：此前是表外文件）"),
]


def _git(args: list[str]) -> bytes:
    """跑一条 git 命令并返回原始输出；失败时抛 SystemExit(2)，绝不吞掉错误返回空集。"""
    proc = subprocess.run(["git", *args], capture_output=True)
    if proc.returncode != 0:
        tail = proc.stderr.decode("utf-8", "replace").strip().replace("\n", " ")[:200]
        print(f"ERROR=git_failed args={' '.join(args)} rc={proc.returncode} stderr={tail}")
        print("RESULT=ERROR")
        sys.exit(2)
    return proc.stdout


def changed_files(since: str, head: str, worktree: bool) -> list[str]:
    """从 git 读改动过的文件名。git 报错就退出 2——读不到不等于没改动。"""
    if subprocess.run(["git", "rev-parse", "--is-inside-work-tree"], capture_output=True).returncode != 0:
        print("ERROR=not_a_git_repo")
        print("RESULT=ERROR")
        sys.exit(2)
    _git(["rev-parse", "--verify", "--quiet", f"{since}^{{commit}}"])
    names: list[str] = []
    names += [p.decode("utf-8", "surrogateescape") for p in _git(["diff", "--name-only", "-z", f"{since}...{head}"]).split(b"\0") if p]
    if worktree:
        names += [p.decode("utf-8", "surrogateescape") for p in _git(["diff", "--name-only", "-z", head]).split(b"\0") if p]
        names += [p.decode("utf-8", "surrogateescape") for p in _git(["ls-files", "--others", "--exclude-standard", "-z"]).split(b"\0") if p]
    # 去重并保持稳定顺序（同一个文件可能在多个范围里各出现一次）
    seen: set[str] = set()
    ordered: list[str] = []
    for name in names:
        if name and name not in seen:
            seen.add(name)
            ordered.append(name)
    return ordered


def resolve(files: list[str], rules: list = RULES, unconditional_wins: bool = True) -> tuple[dict[str, set], list[str], list[str]]:
    """把改动文件对照规则。返回（条目 → 前提集合，空集合表示无条件必跑；命中的规则编号；没覆盖到的文件）。

    ★ 同一条目只要有一条规则说「无条件必跑」，它就是无条件必跑，别的规则给它加的前提不再输出
      （`unconditional_wins=True`，默认）。此前是「有前提的集合非空就按有前提输出」：一条必跑项会被
      另一条规则降级成「要人判断前提」，而降级正是这张表要防的那种遗漏
      （2026-11-24 第二次补规则时发现并订正）。开关只给自检的变异验证用：关掉它就是旧行为。
    """
    items: dict[str, set] = {}
    forced: set[str] = set()
    hit_rules: list[str] = []
    unmapped: list[str] = []
    for path in files:
        matched = []
        for rule_id, patterns, entries, _note in rules:
            if any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns.split("|")):
                matched.append(rule_id)
                for item, condition in entries:
                    items.setdefault(item, set())
                    if condition is None:
                        forced.add(item)
                    else:
                        items[item].add(condition)
        if matched:
            hit_rules.extend(r for r in matched if r not in hit_rules)
        else:
            unmapped.append(path)
    if unconditional_wins:
        for item in forced:
            items[item] = set()
    return items, hit_rules, unmapped


def report(files: list[str]) -> None:
    items, hit_rules, unmapped = resolve(files)
    for path in files:
        print(f"CHANGED={path}")
    for item, description in ITEMS.items():
        if item not in items:
            continue
        conditions = items[item]
        if not conditions:
            print(f"REVERIFY={item}  {description}")
        for condition in sorted(conditions):
            print(f"REVERIFY={item}  when={condition}  {description}")
    for path in unmapped:
        print(f"UNMAPPED={path}")
    print(f"RULES_HIT={','.join(hit_rules) if hit_rules else '-'}")
    if unmapped:
        print("NOTE=有改动文件不在这张表里：要人来判断它们影响什么，不等于不用重跑（表在 tools/test/reverify-map.py 的 RULES）")
    must = sum(1 for conditions in items.values() if not conditions)
    print(f"REVERIFY_COUNT={len(items)} REVERIFY_UNCONDITIONAL={must} UNMAPPED_COUNT={len(unmapped)}")
    if not files:
        print("RESULT=NO_PRODUCT_CHANGE")
    elif unmapped:
        print("RESULT=MAPPED_WITH_UNMAPPED")
    else:
        print("RESULT=CLEAN")


def dump_table() -> None:
    print("| 规则 | 匹配的文件 | 无条件必跑 | 有前提的项 | 备注 |")
    print("| --- | --- | --- | --- | --- |")
    for rule_id, patterns, entries, note in RULES:
        plain = "、".join(ITEMS[item] for item, cond in entries if cond is None) or "—"
        conditional = "；".join(f"{ITEMS[item]}（{cond}）" for item, cond in entries if cond is not None) or "—"
        print(f"| {rule_id} | `{patterns.replace('|' , '` `<br>`')}` | {plain} | {conditional} | {note} |")


# ── 自检 ─────────────────────────────────────────────────────────────────────
# 固定用例：期望值只针对「无条件必跑」的条目；有前提的条目不计入（它们要人判断前提）。
SELFTEST_CASES = [
    ("改了备份链与核心库哈希（2026-11-24 那次）",
     [f"{CLI}recovery_backup.rs", f"{CORE}task_store.rs", f"{CORE}fast_hash.rs", f"{CORE}lib.rs",
      "VERSION", "README.md"],
     {"win-backup", "win-restore", "core-unit", "smoke-status", "hash-backend", "doc-gates"}, set()),
    ("只改文档", ["README.md", "docs/20261124-x.md", "经验/02-VM操作与传输纪律.md"], {"doc-gates"}, set()),
    ("只改界面", [f"{CLI}native_gui/task_executor.rs"], {"gui-matrix", "gui-confirm"}, set()),
    ("表里没有的文件", [f"{CLI}some_new_module.rs"], set(), {f"{CLI}some_new_module.rs"}),
    ("错误码表、日志文案表与卷设备模块", [f"{CLI}error_code.rs", f"{CLI}i18n.rs", f"{CLI}volume_device.rs", f"{CLI}image_admin.rs"],
     {"table-unit", "cli-probe"}, set()),
    ("同一条目既被无条件规则命中、又被有前提规则命中（无条件必须胜出）",
     [f"{CLI}recovery_progress.rs", f"{CLI}text_parsing.rs"],
     {"win-backup", "win-restore", "progress-pe", "parse-unit"}, set()),
    ("改了孤儿任务守卫与准备锁（2026-11-24 第二次）",
     [f"{CLI}pending_task.rs", f"{CLI}prepare_lock.rs", f"{CLI}prepare_payload.rs", f"{CLI}environment_admin.rs",
      f"{CLI}error_code.rs", f"{CLI}main.rs"],
     {"win-backup", "win-restore", "power-resume", "orphan-race", "guard-unit", "table-unit", "cli-probe"}, set()),
    ("没有任何改动", [], set(), set()),
]


def selftest() -> int:
    failed = 0
    for title, files, want_items, want_unmapped in SELFTEST_CASES:
        items, _rules, unmapped = resolve(files)
        got = {item for item, conditions in items.items() if not conditions}
        ok = got == want_items and set(unmapped) == want_unmapped
        print(f"SELFTEST case={title} {'PASS' if ok else 'FAIL'} got={','.join(sorted(got)) or '-'} unmapped={','.join(sorted(unmapped)) or '-'}")
        failed += 0 if ok else 1

    # 变异验证：拆掉「备份链」那条规则，备份与还原两个必跑项必须从输出里消失；
    # 核心库的项由别的规则提供，必须还在——这同时证明「还原欠着」正是由这条规则把关的。
    case = next(c for c in SELFTEST_CASES if c[0].startswith("改了备份链"))
    before, _r, _u = resolve(case[1])
    mutated = [rule for rule in RULES if rule[0] != "R04"]
    after, _r2, _u2 = resolve(case[1], mutated)
    gone = {"win-backup", "win-restore"}.issubset(before) and not ({"win-backup", "win-restore"} & set(after))
    kept = "core-unit" in after and "hash-backend" in after
    restored, _r3, _u3 = resolve(case[1])
    back = {"win-backup", "win-restore"}.issubset(restored)
    ok = gone and kept and back
    print(f"SELFTEST mutation=drop_R04 {'PASS' if ok else 'FAIL'} gone={gone} kept={kept} restored={back}")
    failed += 0 if ok else 1

    # 变异验证二：关掉「无条件胜出」开关（= 旧行为：有前提的集合非空就按有前提输出），
    # 「无条件必须胜出」那条用例必须变红——必跑项 progress-pe 被降级成有前提；开关打开时必须绿。
    demoted_case = next(c for c in SELFTEST_CASES if c[0].startswith("同一条目"))
    good_u, _r, _u = resolve(demoted_case[1])
    good_ok = {item for item, conditions in good_u.items() if not conditions} == demoted_case[2]
    old_u, _r2, _u2 = resolve(demoted_case[1], RULES, unconditional_wins=False)
    old_got = {item for item, conditions in old_u.items() if not conditions}
    red = "progress-pe" not in old_got and old_got != demoted_case[2]
    ok = good_ok and red
    print(f"SELFTEST mutation=unconditional_loses {'PASS' if ok else 'FAIL'} green_with_fix={good_ok} red_without_fix={red}")
    failed += 0 if ok else 1

    # 变异验证三：拆掉「准备锁」那条规则，守卫竞态与它的单元测试必须不再是「无条件必跑」
    # （别的规则还以「有前提」的形式提到它们，所以查的是无条件身份，不是条目还在不在）；
    # 备份与还原由别的规则提供，必须还在。
    guard_case = next(c for c in SELFTEST_CASES if c[0].startswith("改了孤儿任务守卫"))
    before_g, _r, _u = resolve(guard_case[1])
    mutated_g = [rule for rule in RULES if rule[0] != "R28"]
    after_g, _r2, _u2 = resolve(guard_case[1], mutated_g)
    def unconditional(items: dict[str, set]) -> set[str]:
        return {item for item, conditions in items.items() if not conditions}
    gone_g = {"orphan-race", "guard-unit"}.issubset(unconditional(before_g)) and not ({"orphan-race", "guard-unit"} & unconditional(after_g))
    kept_g = "win-backup" in unconditional(after_g) and "table-unit" in unconditional(after_g)
    restored_g, _r3, _u3 = resolve(guard_case[1])
    back_g = {"orphan-race", "guard-unit"}.issubset(unconditional(restored_g))
    ok = gone_g and kept_g and back_g
    print(f"SELFTEST mutation=drop_R28 {'PASS' if ok else 'FAIL'} gone={gone_g} kept={kept_g} restored={back_g}")
    failed += 0 if ok else 1

    print(f"SELFTEST_CASES={len(SELFTEST_CASES) + 3} SELFTEST_FAILED={failed}")
    print("SELFTEST=PASS" if failed == 0 else "SELFTEST=FAIL")
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    mode = ""
    since, head, worktree = DEFAULT_SINCE, "HEAD", False
    files: list[str] = []
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg == "--selftest":
            mode = "selftest"
        elif arg == "--dump-table":
            mode = "dump"
        elif arg == "--worktree":
            worktree = True
        elif arg in ("--since", "--head"):
            if i + 1 >= len(argv):
                print(f"ERROR={arg}_needs_a_value"); return 2
            if arg == "--since": since = argv[i + 1]
            else: head = argv[i + 1]
            i += 1
        elif arg == "--files":
            mode = "files"
        elif arg.startswith("-") and arg != "-":
            print(f"ERROR=unknown_option option={arg}"); return 2
        elif mode == "files":
            files.append(arg)
        else:
            print(f"ERROR=unexpected_argument arg={arg} hint=use_--files"); return 2
        i += 1
    if mode == "selftest":
        return selftest()
    if mode == "dump":
        dump_table(); return 0
    if mode == "files":
        report(files); return 0
    print(f"SINCE={since} HEAD={head} WORKTREE={int(worktree)}")
    report(changed_files(since, head, worktree))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
