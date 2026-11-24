# 命令行探测命令冒烟的判读（被 tools/vm/cli-probe.sh 调用；也被 tools/vm/selftest.sh 拿真实输出样本离线验证）。
#
# 用法：python3 judge/cli-probe.py <证据目录> <文件前缀>
#   读取 <目录>/<前缀>-list-volumes.txt、-inspect-environment.txt、-wim-info.txt、-list-images.txt，
#   逐条输出 MARK / CONCLUSION / 键值结论，最后一行 CLI_PROBE=PASS|FAIL。
#
# ★ 四条命令的输出格式各不相同，读法是对着实现核过的（2026-11-25 第一次就把 list-images 读丢了）：
#   list-volumes        → 一个 JSON 数组
#   inspect-environment → 一个 JSON 对象
#   wim-info            → 一个 JSON 对象（images 是数组）
#   list-images         → 每个镜像一行 JSON 对象（不是数组），每行自带 success 字段；末尾另有一行结论（带 task_id 字段）
#   所以区分「记录行」与「结论行」不能看有没有 success，要看有没有 task_id / image。
# ★ 「读不到」一律记成 READ_FAILED，不记成通过，也不记成判据不满足。
import json
import os
import sys

COMMANDS = ("list-volumes", "inspect-environment", "wim-info", "list-images")


def judge(ev, prefix, out=print):
    ok_all = [True]

    def verdict(key, good, detail=""):
        out("%s=%s%s" % (key, "PASS" if good else "FAIL", (" " + detail) if detail else ""))
        ok_all[0] = ok_all[0] and good

    def load(cmd, want, record="all"):
        """读一条命令的输出，返回记录列表；读不到 / 不是 JSON 时返回 None。`record`='image' 表示记录行必须带 image 字段。"""
        path = os.path.join(ev, "%s-%s.txt" % (prefix, cmd))
        try:
            raw = open(path, "rb").read()
        except OSError as e:
            out("RESULT %s READ_FAILED (cannot read the file: %s)" % (cmd, str(e)[:60]))
            return None
        text = raw.decode("utf-8", "replace")
        done = next((l.strip() for l in text.splitlines() if "VM_DONE=" in l), "")

        def is_record(l):
            if not l.strip().startswith(want):
                return False
            return ('"image":' in l) if record == "image" else True

        lines = [l for l in text.splitlines() if is_record(l)]
        conclusion = [l for l in text.splitlines() if l.strip().startswith("{") and '"task_id"' in l]
        out("MARK %s %s" % (cmd, done or "<missing>"))
        out("CONCLUSION %s %s" % (cmd, conclusion[-1].strip()[:140] if conclusion else "<no conclusion line>"))
        if not lines:
            out("RESULT %s READ_FAILED (no record line; reading failed is not a verdict)" % cmd)
            return None
        try:
            return [json.loads(l) for l in lines]
        except ValueError as e:
            out("RESULT %s JSON_BROKEN %s" % (cmd, str(e)[:80]))
            return None

    recs = load("list-volumes", "[")
    if recs is None:
        verdict("CLI_PROBE_LIST_VOLUMES", False, "no readable output")
    else:
        try:
            vols = recs[0]
            for v in vols:
                out("  VOLUME letter=%s driveType=%s fs=%s hasWindows=%s" % (
                    v["letter"], v.get("driveType", "<missing>"), v["filesystem"], v["hasWindowsInstallation"]))
            has_field = all("driveType" in v for v in vols)
            good = isinstance(vols, list) and len(vols) >= 3 and has_field \
                and any(v["letter"] == "D" and v.get("driveType") == "cdrom" for v in vols)
            verdict("CLI_PROBE_LIST_VOLUMES", good, "count=%d has_drive_type_field=%s" % (len(vols), has_field))
        except (TypeError, KeyError, IndexError) as e:
            verdict("CLI_PROBE_LIST_VOLUMES", False, "unexpected structure: %s" % str(e)[:80])

    recs = load("inspect-environment", "{")
    if recs is None:
        verdict("CLI_PROBE_INSPECT_ENVIRONMENT", False, "no readable output")
    else:
        try:
            env = recs[0]
            keys = ("windows", "architecture", "winreAvailable", "volumes")
            vols = env.get("volumes", [])
            keys_ok = all(k in env for k in keys)
            field_ok = isinstance(vols, list) and len(vols) >= 3 and all("driveType" in v for v in vols)
            verdict("CLI_PROBE_INSPECT_ENVIRONMENT", keys_ok and field_ok,
                    "keys_ok=%s winreAvailable=%s volumes_with_drive_type=%s" % (keys_ok, env.get("winreAvailable"), field_ok))
        except (TypeError, KeyError, IndexError) as e:
            verdict("CLI_PROBE_INSPECT_ENVIRONMENT", False, "unexpected structure: %s" % str(e)[:80])

    recs = load("wim-info", "{")
    if recs is None:
        verdict("CLI_PROBE_WIM_INFO", False, "no readable output")
    else:
        try:
            info = recs[0]
            good = isinstance(info.get("images"), list) and len(info["images"]) >= 1 and "sha256" in info
            verdict("CLI_PROBE_WIM_INFO", good, "keys=%s images=%d" % (",".join(sorted(info.keys()))[:80], len(info.get("images", []))))
        except (TypeError, KeyError, IndexError) as e:
            verdict("CLI_PROBE_WIM_INFO", False, "unexpected structure: %s" % str(e)[:80])

    recs = load("list-images", "{", "image")
    if recs is None:
        verdict("CLI_PROBE_LIST_IMAGES", False, "no readable output")
    else:
        good = len(recs) >= 1 and all("image" in i and "indexes" in i for i in recs)
        verdict("CLI_PROBE_LIST_IMAGES", good, "count=%d" % len(recs))

    out("CLI_PROBE=" + ("PASS" if ok_all[0] else "FAIL"))
    return ok_all[0]


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: judge/cli-probe.py <evidence dir> <file prefix>")
    sys.exit(0 if judge(sys.argv[1], sys.argv[2]) else 1)
