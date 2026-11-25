# 读一份「原样取回」的恢复环境日志，取出某一个任务那一段里的结构化读数。
# 被 tools/vm/data-restore.sh、pause-restore.sh、gui-data-restore.sh 调用；
# tools/vm/selftest.sh 用人工拼的日志片段离线验证它。
#
# 用法：python3 judge/restore-log.py <Recovery.raw.log> <任务编号> [--pause] [--early-log <任务早期日志>]
#   恢复环境日志是跨任务累积的：先找「本任务的启动行」，段落一直延伸到下一个任务的启动行或文件末尾。
#   「暂停声明行」是切换日志的瞬间补写的，排在本任务启动行的**前面**——往前找最近的一行。
#
# 输出（一行一个键值）：
#   RECOVERY_SEGMENT_FOUND=True|False
#   RECOVERY_STEP_LABELS=1,2,3,4          本任务日志里出现过的步骤编号
#   SELFCHECK_LINE=RESTORE_SELFCHECK …    自检行（没有则 <missing>）
#   RECOVERY_LINE …                       步骤行、暂停行、自检行、错误行（供人看）
#   带 --pause 时另加：
#   DECLARATION_IN_FORMAL_LOG=…           正式日志里的暂停声明行（截取从 Pause support 起）
#   DECLARATION_IN_TASK_EARLY_LOG=<n>     任务早期日志里的声明行行数（给了 --early-log 时）
#   PAUSED_LINES=<n>  RESUMED_LINES=<n>  SECONDS_PAUSED=<秒|unknown>
#   PAUSED_LINE …
#
# ★ 判「四个步骤都出现过」，不判「恰好四行」：暂停后恢复会把当前步骤再记一遍。
# ★ 停了多久直接量「恢复已暂停」到「恢复已继续」两行：暂停可能落在任何一个检查点上。
# ★ 时间按完整日期时间相减：跨过午夜的暂停不能用「时分秒」相减。
import re
import sys
from datetime import datetime


def moment(text):
    m = re.search(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)", text)
    return datetime.fromisoformat(m.group(1)) if m else None


def judge(log_path, task, pause=False, early_log=None, out=print):
    raw = open(log_path, "rb").read().decode("utf-8", "replace")
    log = raw.splitlines()
    starts = [i for i, l in enumerate(log) if task in l and "Recovery.exe started" in l]
    start = starts[0] if starts else None
    found = start is not None
    if found:
        end = next((j for j in range(start + 1, len(log)) if "Recovery.exe started" in log[j]), len(log))
        seg = log[start:end]
    else:
        seg = []
    out("RECOVERY_SEGMENT_FOUND=%s" % found)
    out("RECOVERY_STEP_LABELS=%s" % ",".join(sorted(set(re.findall(r"STEP (\d)/4", "\n".join(seg))))))
    sc = [l for l in seg if "RESTORE_SELFCHECK" in l]
    out("SELFCHECK_LINE=" + (sc[0][sc[0].find("RESTORE_SELFCHECK"):][:160] if sc else "<missing>"))
    for l in seg:
        if ("STEP " in l and "/4" in l) or "SELFCHECK" in l or "ERROR" in l or "BR-" in l or "Pause" in l:
            out("RECOVERY_LINE " + l[:200])
    if not pause:
        return found
    if found:
        decl = next((l for l in reversed(log[max(0, start - 6):start]) if "Pause support" in l), "")
    else:
        decl = ""
    out("DECLARATION_IN_FORMAL_LOG=" + (decl[decl.find("Pause support"):][:80] or "<missing>"))
    if early_log is not None:
        try:
            early = open(early_log, "rb").read().decode("utf-8", "replace").splitlines()
        except OSError:
            early = []
        out("DECLARATION_IN_TASK_EARLY_LOG=%d" % sum(1 for l in early if "Pause support" in l))
    paused = [l for l in seg if "恢复已暂停" in l]
    resumed = [l for l in seg if "恢复已继续" in l]
    out("PAUSED_LINES=%d" % len(paused))
    out("RESUMED_LINES=%d" % len(resumed))
    paused_at = moment(paused[0]) if paused else None
    resumed_at = moment(resumed[0]) if resumed else None
    out("SECONDS_PAUSED=%s" % (int((resumed_at - paused_at).total_seconds()) if paused_at and resumed_at else "<unknown>"))
    for l in paused[:1]:
        out("PAUSED_LINE " + l[:200])
    return found


if __name__ == "__main__":
    args = sys.argv[1:]
    pause = "--pause" in args
    early = None
    if "--early-log" in args:
        i = args.index("--early-log")
        early = args[i + 1]
        del args[i:i + 2]
    args = [a for a in args if a != "--pause"]
    if len(args) != 2:
        sys.exit("usage: judge/restore-log.py <Recovery.raw.log> <task-id> [--pause] [--early-log <file>]")
    sys.exit(0 if judge(args[0], args[1], pause, early) else 1)
