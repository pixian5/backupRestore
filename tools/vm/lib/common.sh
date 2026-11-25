#!/bin/bash
# tools/vm/lib/common.sh —— 实机用例的公共函数与约定。
# 用法：在用例脚本开头
#   HERE="$(cd "$(dirname "$0")" && pwd)"
#   source "$HERE/lib/common.sh"          # 不在 tools/vm/ 下的脚本按相对路径引入
# 之后可用：REPO / VERSION / EV（证据目录）/ NAME（本次运行的前缀，带版本号），以及下面这些函数。
#
# 用例脚本的统一形状：
#   1. STEP=0 读部署位两个文件的哈希（证明跑的是哪个二进制）；
#   2. 启动配置先做文件级备份并核验（bcd_backup），前后各读一次（bcd_readings）并用 bcd_same 比对；
#   3. 动界面前先 require_idle；
#   4. 判据只认结构化字段与原样取回的文件（fetch）；最后用 verdict 给出 `<用例名>=PASS|FAIL`，退出码与之对应。
# 证据文件名一律带版本号前缀（NAME=<用途>-v<版本>），两轮运行不会互相覆盖。
#
# ★ 退出码：0 = 全部判据通过；非 0 = 有判据没过或有步骤中止（日志里会写明是哪一种）。
#   「读不到」一律记成中止并点名，不记成通过，也不记成判据不满足。

HERE_VM="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"      # tools/vm
source "$HERE_VM/lib/vm.sh"

REPO="$(cd "$HERE_VM/../.." && pwd)"
VERSION="$(tr -d '\r\n' < "$REPO/VERSION")"
EV="$REPO/.test-artifacts/vm-runs/evidence"                     # 已 gitignore；文件名带版本号前缀
SHARE_EV='\\Mac\backupRestore\.test-artifacts\vm-runs\evidence'  # 同一个目录在虚拟机眼里的 UNC 路径
mkdir -p "$EV"
export LC_ALL=C

EXE="${BR_EXE:-H:\brwork\BackupRestore.exe}"   # 部署位唯一 H:\brwork
SOURCE_DRIVE="${BR_SOURCE_DRIVE:-F}"           # 可牺牲的测试卷（备份的源、还原的目标）
IMAGE='H:\BRImages\br-vm-tools-test.wim'       # 备份产出、还原消费的测试镜像（用例自己创建、自己删）
PAD='F:\br-vm-tools-pad.bin'                   # 20 GB 全零占位文件：让捕获足够长、能按到暂停；也是还原的物理判据
PAD_BYTES=21474836480
# 下面两份是测试机上的基线镜像（回归基线实测留下的），不在时用环境变量换一份：
DATA_IMAGE="${BR_DATA_IMAGE:-H:\BRImages\data-bl-20261124.wim}"   # 数据镜像：哨兵法还原用（F: 上要有配套的 F:\BRDATA）
SYS_IMAGE="${BR_SYS_IMAGE:-H:\BRImages\c-bl-20261124.wim}"         # 完整 Windows 镜像：必须被拒「写进数据卷」
VMKEY="$REPO/tools/win-clicker/vmkey.sh"

say() { printf '[%s] %s\n' "$(date '+%H:%M:%S')" "$*"; }

# run_name <用途>：设定本次运行的前缀 = <用途>-v<版本>-<时间戳>。用途只用字母、数字与连字符。
# ★ 为什么带时间戳：只带版本号的话，同一版本第二次运行时若某一步中止、没重写对应文件，
#   判读会读到上一次留下的旧文件（2026-11-25 收脚本时想到的；此前已经发生过两轮同名互相覆盖）。
run_name() { NAME="$1-v$VERSION-$(date +%Y%m%d-%H%M%S)"; }

# 虚拟机被「空闲自动暂停」停住了：只恢复运行，不改任何设置（用户明令不改虚拟机设置）。
keep_alive() {
  local st; st=$("$PRL" list -a 2>/dev/null | grep -i "$VM" | awk '{print $2}')
  if [ "$st" = "paused" ]; then "$PRL" resume "$VM" >/dev/null 2>&1; say "VM_PAUSED_RESUMED"; fi
}

# 以系统身份执行共享盘上的 ps1（tools/vm/ps/），必须收到结尾哨兵 __END__。
ps1() {
  local out="" i
  for i in 1 2 3 4; do
    out=$("$PRL" exec "$VM" powershell -NoProfile -ExecutionPolicy Bypass -File "$@" 2>/dev/null | tr -d '\r')
    printf '%s' "$out" | grep -a -q '^__END__' && break
    sleep 2
  done
  printf '%s\n' "$out"
}

# 把客体里的文件原样拷到宿主证据目录（经共享盘，不经过任何输出过滤）。
# $1 客体路径（Windows 的正常写法：反斜杠都是单个）  $2 宿主文件名
# 先删掉旧目标再拷：免得拷贝没执行时读到上一次的旧文件。
fetch() {
  local i
  rm -f "$EV/$2"
  for i in 1 2 3 4; do
    "$PRL" exec "$VM" cmd /d /c "copy /y $1 $SHARE_EV\\$2" >/dev/null 2>&1
    [ -s "$EV/$2" ] && return 0
    sleep 2
  done
  return 1
}

# 执行产品命令，把它的一行 JSON 原样存进文件（不过滤；必须带回显标记才算执行过）。
vraw() { # $1 命令  $2 宿主文件名
  local i
  rm -f "$EV/$2"
  for i in 1 2 3 4; do
    "$PRL" exec "$VM" cmd /d /v:on /c "$1 & echo VM_DONE=!ERRORLEVEL!" > "$EV/$2" 2>/dev/null
    grep -a -q 'VM_DONE=' "$EV/$2" && return 0
    sleep 2
  done
  return 1
}

# 取一行 JSON 里的一个字段。空值（null）、缺字段、文件读不到、不是 JSON：一律输出空串，原因写到标准错误。
# ★ 读不到不能长得像读到了：此前把 null 打成了 Python 的「None」，脚本把「没拿到任务编号」当成拿到了任务。
json_field() { # $1 文件  $2 字段
  python3 -I -c "
import json,sys
name=sys.argv[1]
def none(reason):
    sys.stderr.write('json_field: %s (%s)\n' % (reason, name))
    print('')
    sys.exit(0)
try:
    raw=open(name,'rb').read()
except OSError as e:
    none('cannot read the file: %s' % e)
line=next((l for l in raw.split(b'\\n') if l.strip().startswith(b'{')), b'')
if not line: none('no JSON line in the output')
try:
    d=json.loads(line.decode('utf-8'))
except ValueError as e:
    none('the line is not JSON: %s' % e)
v=d.get(sys.argv[2])
print('' if v is None else v)
" "$1" "$2"
}

# 部署位两个文件的哈希。
deployed_hashes() {
  vrun "certutil -hashfile H:\\brwork\\BackupRestore.exe SHA256" 3 | grep -a -E '^[0-9a-f]{64}$' | sed 's/^/DEPLOYED_SHA256=/'
  vrun "certutil -hashfile H:\\brwork\\Recovery.exe SHA256" 3 | grep -a -E '^[0-9a-f]{64}$' | sed 's/^/DEPLOYED_RECOVERY_SHA256=/'
}

# 启动配置的三项读数：启动管理器指纹、全部条目里 device 行数、一次性启动项行数。
bcd_readings() {
  local bcd entries seq
  bcd=$(vrun "bcdedit /enum {bootmgr} /v" 3 | grep -a -v 'VM_DONE' | md5 -q)
  entries=$(vrun "bcdedit /enum all /v" 3 | grep -a -i -c '^\s*device\s')
  seq=$(vrun "bcdedit /enum {bootmgr} /v" 3 | grep -a -i -c 'bootsequence')
  printf 'BCD_MD5=%s\nDEVICE_LINES=%s\nBOOTSEQUENCE_LINES=%s\n' "$bcd" "$entries" "$seq"
}

# bcd_same <前> <后>：两份读数是否逐行相同（输出 BCD_READINGS_EQUAL=True|False）
bcd_same() {
  local a b
  a=$(grep -a -E '^(BCD_MD5|DEVICE_LINES|BOOTSEQUENCE_LINES)=' "$1" | tr -d '\r')
  b=$(grep -a -E '^(BCD_MD5|DEVICE_LINES|BOOTSEQUENCE_LINES)=' "$2" | tr -d '\r')
  if [ -n "$a" ] && [ "$a" = "$b" ]; then echo "BCD_READINGS_EQUAL=True"; else echo "BCD_READINGS_EQUAL=False"; return 1; fi
}

# 启动配置的文件级备份：导出 → 用导出的文件离线枚举启动管理器 → 必须读得到 default 与 displayorder。
# 读不到就中止：备份失败不得继续。$1 = 标签（字母数字与连字符）
bcd_backup() {
  local label="$1" out="" i
  for i in 1 2; do
    vrun "bcdedit /export H:\\brwork\\bcd-pre-$label.bak" 2 | grep -a 'VM_DONE='
    out=$(vrun "bcdedit /store H:\\brwork\\bcd-pre-$label.bak /enum {bootmgr}" 2 | grep -a -v 'VM_DONE')
    printf '%s\n' "$out" | grep -a -q -i '^default ' && printf '%s\n' "$out" | grep -a -q -i '^displayorder ' && break
    sleep 2
  done
  printf '%s\n' "$out" | grep -a -i -E 'default|displayorder|timeout|resumeobject' | sed 's/^/BCD_FILE_/'
  printf '%s\n' "$out" | grep -a -q -i '^default ' && printf '%s\n' "$out" | grep -a -q -i '^displayorder ' && return 0
  say "ABORT: the boot configuration backup could not be verified offline; not continuing"; exit 3
}

# 没人在操作才继续。判据来自 tools/vm/user-idle.sh：它直接问用户会话「上一次键鼠输入是多久之前」。
# ★ 不要用 quser 的空闲时长：对本地控制台会话恒显示 none。
# ★ 你自己的自动化输入也算键鼠输入：连着跑第二个界面脚本会被这里拦下——等空闲满阈值，不要调低它。
require_idle() { # $1 证据文件名  $2 最少空闲秒（默认 120）
  local out rc
  out=$("$HERE_VM/user-idle.sh" "${2:-120}" 2>&1); rc=$?
  printf '%s\n' "$out" | tee "$EV/$1"
  case "$rc" in
    0) say "IDLE_VERDICT=nobody is using the desktop" ;;
    1) say "ABORT: someone typed within the last ${2:-120} seconds; not touching the desktop"; exit 3 ;;
    *) say "ABORT: could not read the idle time (reading failed is not the same as nobody is using it)"; exit 3 ;;
  esac
}

send_p() { "$VMKEY" p >/dev/null 2>&1; say "SENT_KEY_P"; }

# 等任务进入终态。$1 任务编号  $2 上限秒  $3…… 发 P 的时间点（秒，相对本函数开始；省略 = 不发键）。
# 先确认命令通道确实断过（真的在重启），再等通道回来且状态文件进入 success / failed。
wait_terminal() {
  local task="$1" limit="$2"; shift 2
  local marks=("$@") start mi=0 seen_down=0 final="" alive alive2 st elapsed
  start=$(date +%s)
  while :; do
    elapsed=$(( $(date +%s) - start ))
    [ "$elapsed" -ge "$limit" ] && break
    keep_alive
    while [ "$mi" -lt ${#marks[@]} ] && [ "$elapsed" -ge "${marks[$mi]}" ]; do send_p; mi=$((mi+1)); done
    alive=$("$PRL" exec "$VM" cmd /d /c "echo ALIVE & echo __END__" 2>/dev/null | tr -cd '\11\12\40-\176')
    if printf '%s' "$alive" | grep -q '__END__'; then
      st=$(task_stage "$task")
      if [ "$seen_down" = 1 ]; then
        case "$st" in success|failed) final="$st"; say "TERMINAL stage=$final after ${elapsed}s"; break ;; esac
      elif [ "$st" = "failed" ]; then
        # 还没重启过任务就已经是失败：准备期自己失败了（不会重启），不必再等。
        final=failed; say "TERMINAL stage=failed before any reboot (the preparation itself failed) after ${elapsed}s"; break
      fi
      sleep 5
    else
      if [ "$seen_down" = 0 ]; then
        sleep 3
        alive2=$("$PRL" exec "$VM" cmd /d /c "echo ALIVE & echo __END__" 2>/dev/null | tr -cd '\11\12\40-\176')
        printf '%s' "$alive2" | grep -q '__END__' || { seen_down=1; say "VM_DOWN_OBSERVED after ${elapsed}s"; }
      fi
      sleep 4
    fi
  done
  printf 'TERMINAL_STAGE=%s\n' "${final:-<none within limit>}"
  [ -n "$final" ]
}

task_count() { vrun "dir /b /ad H:\\brwork\\tasks" 3 | grep -a -c -E '^[0-9a-f]{8}-'; }

# 最新的任务目录编号（按修改时间倒序取第一个）。读不到输出空串。
newest_task() { vrun "dir /b /ad /o-d H:\\brwork\\tasks" 2 | tr -cd '\11\12\40-\176\r' | grep -a -E '^[0-9a-f]{8}-[0-9a-f]{4}-' | head -1 | tr -d '\r'; }

task_stage() { # $1 任务编号 → 输出阶段（success / failed / 其它）；读不到输出空串
  vrun "powershell -NoProfile -Command \"(Get-Content 'H:\\brwork\\tasks\\$1\\status.json' -Raw | ConvertFrom-Json).stage\"" 3 \
    | tr -cd '\11\12\40-\176\r' | tr -d '\r' | grep -a -E '^[a-z-]+$' | tail -1
}

# run_prepare <输出文件名> <prepare 的参数……>：执行 prepare 并辨认它建出的任务。
# 结果放在三个变量里：PREPARE_SUCCESS（True / False / 空串 = 读不到）、PREPARE_TASK、PREPARE_TASK_SOURCE。
# ★ 为什么不用 vraw：vraw 找不到回显标记会重试，而 prepare 有副作用——准备成功时它会自己请求重启，
#   通道在输出回显标记之前断开是常态；盲目重试等于在重启过程中再下发一次准备。
# ★ 输出里没有任务编号时（通道在输出之前就断了），用「最新的任务目录在调用前后变了」来辨认：
#   调用之前先记下最新的编号，之后如果多出一个更新的，那就是本次的。
# ★★ 唯一可以重新下发的情形：**客户端进程已经退出、输出是空的、再等 8 秒也没有新任务目录**。
#   prlctl exec 的客户端在命令执行期间是一直阻塞的；它退出了、又什么都没输出，才是「命令没有执行」
#   （prlctl exec 会静默不执行；2026-11-25 实测：虚拟机刚重启回来的一分钟里，连续两次准备的输出文件都是 0 字节）。
#   ★★ 「等了几秒没有输出、没有新目录」**不能**证明没执行：命令可能只是起得慢。2026-11-25 实测：后台下发后
#   2.5 秒没见到任务目录就判成没执行并重发，结果第一次其实执行了（任务目录 4 秒后才出现），第二次被产品的
#   「存在未终结任务」闸门拦下（BR-TSK-005，无害）；而且杀掉宿主上的客户端并不会停掉虚拟机里的命令。
#   有任何输出、或出现了新任务目录，就绝不重新下发。
#   如果重新下发的那一次被闸门拦下、而任务目录是本次调用之后新出现的：在办的就是前一次下发的任务，按它去等终态。
# ★★ 调用处不要给本函数接管道（`run_prepare … | tee …`）：那样函数跑在子进程里，
#   PREPARE_SUCCESS / PREPARE_TASK 传不回主脚本，严格模式下直接报「未定义变量」——而此时准备已经成功、
#   机器正要重启（2026-11-25 收脚本时实测）。摘要行本来就输出到标准输出，需要留档用 prepare_summary。
run_prepare() {
  local out="$1" before after attempt w code; shift
  before=$(newest_task)
  for attempt in 1 2 3; do
    rm -f "$EV/$out"
    # 这里是前台调用：客户端会一直阻塞到命令结束（或机器重启、通道断开）才返回。
    "$PRL" exec "$VM" cmd /d /v:on /c "$EXE prepare $* & echo VM_DONE=!ERRORLEVEL!" > "$EV/$out" 2>/dev/null
    [ -s "$EV/$out" ] && break                      # 有任何输出 = 执行过了
    after=""
    for w in 1 2 3 4; do                            # 客户端已退出而输出为空：再给它 8 秒，任务目录出现就说明它在跑
      after=$(newest_task)
      [ -n "$after" ] && [ "$after" != "$before" ] && break
      sleep 2
    done
    [ -n "$after" ] && [ "$after" != "$before" ] && break
    say "PREPARE_ATTEMPT $attempt: the client exited with no output and no task directory appeared within 8 s -- the command did not run$( [ $attempt -lt 3 ] && echo '; sending it again' || echo '' )"
  done
  PREPARE_SUCCESS=$(json_field "$EV/$out" success)
  PREPARE_TASK=$(json_field "$EV/$out" task_id)
  code=$(json_field "$EV/$out" error_code)
  if [ -n "$PREPARE_TASK" ]; then
    PREPARE_TASK_SOURCE=output
  else
    after=$(newest_task)
    if [ -n "$after" ] && [ "$after" != "$before" ]; then PREPARE_TASK="$after"; PREPARE_TASK_SOURCE="newest task directory"
    else PREPARE_TASK=""; PREPARE_TASK_SOURCE=none; fi
  fi
  if [ "$PREPARE_SUCCESS" = "False" ] && [ "$code" = "BR-TSK-005" ] && [ "$PREPARE_TASK_SOURCE" = "newest task directory" ]; then
    say "PREPARE_NOTE: this attempt was refused by the unfinished-task gate (BR-TSK-005); the task $PREPARE_TASK appeared after this call began, so it is the one in progress"
    PREPARE_SUCCESS=""                               # 当作读不到：让调用方按任务目录去等终态
  fi
  printf 'PREPARE_SUCCESS=%s PREPARE_TASK=%s PREPARE_TASK_SOURCE=%s ATTEMPTS=%s\n' "${PREPARE_SUCCESS:-<unreadable>}" "${PREPARE_TASK:-<none>}" "$PREPARE_TASK_SOURCE" "$attempt"
}

# prepare 的输出摘要 + 错误开头（写进 <前缀>-prepare-summary.txt）
prepare_summary() { # $1 输出文件名  $2 摘要文件名
  printf 'PREPARE success=%s error_code=%s task_id=%s\n' "$PREPARE_SUCCESS" "$(json_field "$EV/$1" error_code)" "${PREPARE_TASK:-}" | tee "$EV/$2"
  printf 'PREPARE_ERROR_HEAD=%s\n' "$(json_field "$EV/$1" error | head -c 300 | tr '\n' ' ')" | tee -a "$EV/$2"
}

# wait_stage_terminal <任务> <上限秒>：只轮询状态文件直到 success / failed（不要求先观察到重启）。
# 用在「已经确认过重启、之后又要等一次」的场合——wait_terminal 以「通道断过」为前提，不能重复用。
wait_stage_terminal() {
  local task="$1" limit="$2" t=0 st=""
  while [ $t -lt "$limit" ]; do
    keep_alive
    st=$(task_stage "$task")
    case "$st" in success|failed) say "TERMINAL stage=$st after ${t}s"; printf 'TERMINAL_STAGE=%s\n' "$st"; return 0 ;; esac
    sleep 6; t=$((t+6))
  done
  printf 'TERMINAL_STAGE=<none within limit>\n'; return 1
}

# channel_alive：命令通道现在通不通（通 = 返回 0）
channel_alive() {
  local alive
  alive=$("$PRL" exec "$VM" cmd /d /c "echo ALIVE & echo __END__" 2>/dev/null | tr -cd '\11\12\40-\176')
  printf '%s' "$alive" | grep -q '__END__'
}

# 恢复环境是否仍为已启用（输出 WINRE_ENABLED=True|False）。读不到记 False 并点名。
winre_enabled() {
  local n
  n=$(vrun "reagentc /info" 2 | tr -s ' ' | grep -a -i -c 'Enabled')
  if [ "$n" = "1" ]; then echo "WINRE_ENABLED=True"; else echo "WINRE_ENABLED=False reason=read_${n:-none}"; fi
}

# ---- 界面读数的公共读法 ---------------------------------------------------------------
# ★ 常驻提权助手回传的控件文字是**系统本地编码**（GBK），不是通用编码。按通用编码读会把「取消」读成替换字符，
#   于是脚本「认不出取消按钮、拒绝点击」。下面两个函数是唯一允许用来读控件文字的入口。
# decode_text：标准输入 = 助手回传的字节；标准输出 = 通用编码文本。逐行先按通用编码，解不开再按 GBK **严格**解码；
#   两种都解不开的行保留下来并在行尾标注 <DECODE_FAILED>（不悄悄替换成问号）。
#   键值行（WINDOWS= / CLICK= / COMBO …）全是 ASCII，不受影响。
decode_text() {
  python3 -I -c '
import sys
out = []
for line in sys.stdin.buffer.read().split(b"\n"):
    try:
        out.append(line.decode("utf-8"))
    except UnicodeDecodeError:
        try:
            out.append(line.decode("gbk"))
        except UnicodeDecodeError:
            out.append(line.decode("gbk", "replace") + "   <DECODE_FAILED>")
sys.stdout.buffer.write("\n".join(out).encode("utf-8"))
'
}

# control_text <已解码的枚举文本> <控件编号>：取该控件的 text= 后面的文字（没有则输出空串）
control_text() {
  printf '%s\n' "$1" | python3 -I -c "
import sys
want = 'id=%s ' % sys.argv[1]
for line in sys.stdin.read().splitlines():
    if want in line and 'text=' in line:
        print(line[line.rfind('text=') + 5:].strip())
        break
" "$2"
}

# verdict <键> <PASS|FAIL> [说明]：写出用例的最终结论（同时追加进 $NAME-verdict.txt），返回码与结论对应。
verdict() {
  local k="$1" v="$2"; shift 2
  printf '%s=%s%s\n' "$k" "$v" "${*:+ $*}" | tee -a "$EV/${NAME:-run}-verdict.txt"
  [ "$v" = PASS ]
}

# guest_exists <客体路径>：客体里这个文件（或目录）在不在。退出码 0 = 在；1 = 不在；2 = 读不到。
# ★★ 「读不到」必须单独成一种结果，不能并进「不在」：通道抽风时「在不在」的读数是空的。
#   2026-11-25 实测：占位文件明明建成了（创建命令成功返回，直接读也看得到），紧接着的读数两次都没读到，
#   脚本却报「占位文件缺失，可能空间不够」并中止——读不到被写成了不在，还附了一个错误的原因。
#   判据只认两个回显标记之一；连续 6 次都读不到才返回 2，由调用方点名中止。
guest_exists() {
  local i out
  for i in 1 2 3 4 5 6; do
    out=$("$PRL" exec "$VM" cmd /d /c "if exist $1 (echo BR_EXISTS_YES) else (echo BR_EXISTS_NO)" 2>/dev/null | tr -cd '\11\12\40-\176\r' | tr -d '\r')
    case "$out" in
      *BR_EXISTS_YES*) return 0 ;;
      *BR_EXISTS_NO*)  return 1 ;;
    esac
    sleep 2
  done
  return 2
}

# need_guest_file <客体路径> <不在时的中止说明>：这个文件必须在。不在 → 按给的说明中止；读不到 → 单独中止并点名。
need_guest_file() {
  guest_exists "$1"
  local rc=$?
  case $rc in
    0) return 0 ;;
    1) say "ABORT: $2"; exit 3 ;;
    *) say "ABORT: could not read whether $1 exists (reading failed is not the same as missing)"; exit 3 ;;
  esac
}

# require_clean_fixture [strict=yes|no]：**出测试镜像之前**夹具必须是干净的（file1 是原版内容）。不干净就中止。
# 为什么：测试镜像是从测试卷当前的状态备份出来的。夹具要是带着「被改坏」的内容进了镜像，之后的还原就会
#   忠实地把坏内容写回去，「被改坏的文件回到原版」这条判据必然失败——而产品与判读都没有错
#   （2026-11-25 实测：一次意外续跑的准备把当时已被改坏的 file1 备份进了镜像，下一轮还原把它写了回来）。
#   strict=no：夹具目录不存在时放行（该用例自己不用夹具，这道检查只是为了保护它产出的镜像）。
#   把夹具放回干净状态的办法：tools/vm/data-restore.sh（从基线数据镜像还原测试卷）。
require_clean_fixture() {
  local strict="${1:-yes}" f file1 exists
  f="${TMPDIR:-/tmp}/br-fixture-check.$$.txt"
  ps1 "$SHARE_PS\\c5check.ps1" -Tag fx > "$f"
  file1=$(fixture_field "$f" file1); exists=$(fixture_field "$f" brdata_exists)
  rm -f "$f"
  if [ "$file1" = "BRDATA file1 v1" ]; then
    say "FIXTURE_CLEAN=yes (file1 is the original content)"
  elif [ "$exists" = "False" ] && [ "$strict" = no ]; then
    say "FIXTURE_CLEAN=not applicable (the test volume has no fixture directory)"
  elif [ -z "$file1" ] && [ -z "$exists" ]; then
    say "ABORT: could not read the fixture state on the test volume (reading failed is not the same as damaged)"; exit 3
  else
    say "ABORT: the fixture on the test volume is not clean (file1=[$file1], fixture directory exists=$exists);"
    say "       a backup taken now would capture this state and every later restore would write it back."
    say "       Put it back first: tools/vm/data-restore.sh"
    exit 3
  fi
}

# ensure_pad_file：测试卷上的 20 GB 占位文件不在就造一个；最后必须读到「在」。
# 在不在一律走 guest_exists（读不到单独中止，不猜）。
ensure_pad_file() {
  local rc
  guest_exists "$PAD"; rc=$?
  [ $rc -ne 2 ] || { say "ABORT: could not read whether the pad file exists (reading failed is not the same as missing)"; exit 3; }
  if [ $rc -eq 1 ]; then
    say "creating the 20 GB zero pad file on the test volume"
    vrun "fsutil file createnew $PAD $PAD_BYTES" 3 | grep -a 'VM_DONE' | sed 's/^/  PAD_CREATE /'
    sleep 2
  fi
  guest_exists "$PAD"; rc=$?
  case $rc in
    0) say "PAD_FILE=there" ;;
    1) say "ABORT: the pad file was not created (free space on the test volume?)"; exit 3 ;;
    *) say "ABORT: could not read whether the pad file exists after creating it"; exit 3 ;;
  esac
}

# guest_utc_now：虚拟机自己的当前时间（世界时，形如 2026-11-25T12:33:46，精确到秒）。读不到输出空串。
# 用途：界面日志等文件是跨天累积的，要数「本次运行之后」的记录，就得用**客体自己的时钟**划界——
# 用宿主时钟去对客体的日志，两边的钟差多少就错多少。可与日志行里的时间戳直接按字符串比较。
guest_utc_now() {
  vrun "powershell -NoProfile -Command \"(Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ss')\"" 3 \
    | tr -cd '\11\12\40-\176\r' | tr -d '\r' | grep -a -E '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}$' | tail -1
}

# status_field <原样取回的 status.json> <字段>：读任务状态文件的一个字段。读不到输出 <unreadable>，缺字段输出 <missing>。
# 用 utf-8-sig 解码：取回的文件可能带字节序标记。
status_field() {
  python3 -I -c '
import json, sys
try:
    s = json.loads(open(sys.argv[1], "rb").read().decode("utf-8-sig"))
    v = s.get(sys.argv[2])
    print("<missing>" if v is None else v)
except (OSError, ValueError):
    print("<unreadable>")
' "$1" "$2"
}

# fixture_field <夹具检查文件> <字段>：从 c5check.ps1 的输出里取一个字段（哨兵法还原的前后置检查）。
# ★ 夹具检查脚本的输出一行里可能有多个字段（「CHECK tag=pr serial=…」「CHECK payload_len=… payload_sha256=…」），
#   所以按「空格 + 字段名 + 等号」切分，而不是只认行首。2026-11-25 两个版本都读丢过：先是不认行首的标签，
#   后是不认同行的前一个字段——前一次让「序列号未变」假通过（空串 == 空串），后一次让哈希比对恒失败。
#   用 Python 切分而不用 sed：macOS 自带的 sed 不认 `\?` 这类写法。同名出现多次取最后一个。
#   调用方比对之前必须先确认读到的不是空串。
fixture_field() {
  python3 -I -c '
import re, sys
field = sys.argv[2]
value = ""
for line in open(sys.argv[1], "rb").read().decode("utf-8", "replace").splitlines():
    if not line.startswith("CHECK "):
        continue
    for part in re.split(r" (?=[A-Za-z0-9_]+=)", line[6:].strip()):
        key, sep, val = part.partition("=")
        if sep and key == field:
            value = val.strip()
sys.stdout.buffer.write(value.encode("utf-8"))
' "$1" "$2"
}

# gui_ready：为界面驱动做准备——常驻提权助手必须在桌面会话（会话 1），辅助脚本必须已同步且字节数一致。
# 任何一项不满足就中止整个脚本。★ 先把返回码存进变量再判断：`准备 | 美化输出 || 中止` 里的中止永远不会触发。
gui_ready() {
  local h s rc
  h=$(ensure_helper); rc=$?
  printf '%s\n' "$h" | sed 's/^/  HELPER /'
  [ $rc -eq 0 ] || { say "ABORT: the elevated helper is not in the desktop session (got: ${h:-nothing})"; exit 3; }
  s=$(stage_gui_helpers); rc=$?
  printf '%s\n' "$s" | sed 's/^/  /'
  [ $rc -eq 0 ] || { say "ABORT: helper scripts could not be staged into the VM"; exit 3; }
}

# ---- 界面驱动的公共函数（都经常驻提权助手；调用前先 gui_ready）------------------
MAIN_CLASS=BackupRestoreNativeGui
CHOICE_CLASS=BackupRestoreSystemDriveChoice

# gui_launch：启动界面程序。★ 现成的 launch.ps1 会**先结束所有名为 BackupRestore 的进程**——命令行准备进程同名。
# 要在「准备进行中」启动界面，不能用这个函数，用 gui_launch_nokill。
gui_launch() { elev launch.ps1 | tr '\n' ' '; echo; sleep 5; }
gui_launch_nokill() { elev launch-nokill.ps1 | tr '\n' ' '; echo; }

# gui_click <类名> <控件编号>：用真实鼠标点击；辅助脚本在「不是恰好一个窗口匹配」时会拒绝点击。
# 输出助手回传的内容；**没有读到 CLICK=OK 就返回非零**——调用方必须据此中止流程，不能带着没点成的一步往下走。
gui_click() {
  local out
  out=$(elev uiclick.ps1 -Class "$1" -Id "$2" | tr '\n' ' ')
  echo "$out"
  sleep 1
  case "$out" in *CLICK=OK*) return 0 ;; *) return 1 ;; esac
}

# gui_wait_window <类名> <上限秒>：等到恰好一个可见的该类顶层窗口，输出**已解码**的控件枚举；超时输出最后一次枚举并返回 1。
gui_wait_window() {
  local cls="$1" limit="${2:-40}" t=0 out=""
  while [ $t -lt "$limit" ]; do
    out=$(elev uidlglist.ps1 -Class "$cls" 2>/dev/null | decode_text)
    printf '%s\n' "$out" | grep -a -q '^WINDOWS=1' && { printf '%s\n' "$out"; return 0; }
    sleep 3; t=$((t+3))
  done
  printf '%s\n' "$out"; return 1
}

# gui_set_image_path <镜像路径>：把控件 1203 的文字直接写成路径（不模拟打字，避开中文输入法），写入后必须回读一致。
gui_set_image_path() {
  local st="" attempt
  for attempt in 1 2 3 4; do
    st=$(elev uitext2.ps1 -Id 1203 -Text "$1")
    printf '%s' "$st" | grep -a -q 'match=True' && { printf '%s\n' "$st" | grep -a -E '^SETTEXT' | head -1; return 0; }
    sleep 2
  done
  printf '%s\n' "$st" | grep -a -E '^SETTEXT|RESULT=' | tail -1; return 1
}

# gui_combo_items <控件编号>：输出下拉框的选中项与全部条目（已解码）
gui_combo_items() { elev comboitems.ps1 -Id "$1" 2>/dev/null | decode_text; }

# gui_combo_selected <控件编号>：输出下拉框**当前选中项**的文字（已解码；读不到输出空串）。
# ★ 调用方用之前先确认不是空串：读不到的时候「以 X: 开头」这类判断会全部落空。
gui_combo_selected() { gui_combo_items "$1" | python3 -I "$HERE_VM/judge/combo-selected.py"; }

# gui_select_combo <控件编号> <条目文字的开头>：在主窗口的下拉框里选中「以这段文字开头的唯一一个条目」。
# 与用户手选走同一条处理路径（直选之后向父窗口发「选择已变化」通知，界面照常刷新卷详情与默认索引名）。
# 输出助手回传的 SELECT= 行；**没有读到 SELECT=OK 就返回非零**——调用方必须据此中止。
# ★ 选完用 gui_combo_selected 读回核对：通知发出去了不等于界面认了。
# ★ 末尾的冒号在这里去掉：经 -Extra 传给常驻助手的整串参数**末尾是冒号时必定被弄坏**（外层脚本报「参数缺值」、
#   什么都不返回；2026-11-25 实测两次重复都失败，冒号在中间就正常）。辅助脚本按「条目文字的开头」匹配，
#   盘符不带冒号只会匹配到那一个卷。
gui_select_combo() {
  local out text="${2%:}"
  out=$(elev uicombosel.ps1 -Class "$MAIN_CLASS" -Id "$1" -Text "$text" 2>/dev/null | decode_text | grep -a -E '^(SELECT=|SELECTED=)' | tr '\n' ' ')
  echo "$out"
  case "$out" in *SELECT=OK*) return 0 ;; *) return 1 ;; esac
}

# gui_windows <证据文件名>：列出界面进程当前**全部**可见顶层窗口（任意类名，已解码），
# 同时写进证据文件。用来回答「点完某个按钮之后，界面上是不是多出了一个窗口」。
gui_windows() { elev uidlglist.ps1 -Class ANY 2>/dev/null | decode_text | tee "$EV/$1"; }

# gui_window_count <类名>：界面进程当前该类的可见顶层窗口个数（已解码；读不到输出空串）。
gui_window_count() { elev uidlglist.ps1 -Class "$1" 2>/dev/null | decode_text | sed -n 's/^WINDOWS=\([0-9][0-9]*\).*/\1/p' | head -1; }

# windows_judge <标签>：对 <标签>-windows-before.txt 与 <标签>-windows-after.txt 两份清单做判读（judge/windows.py），
# 输出键值行：WINDOWS_READ / MESSAGE_BOXES_AFTER / NEW_WINDOW_CLASSES_AFTER …
windows_judge() { python3 -I "$HERE_VM/judge/windows.py" "$EV/$NAME-$1-windows-before.txt" "$EV/$NAME-$1-windows-after.txt"; }

# settle_windows <标签> <最多轮数>：点掉结果框之后，反复列窗口直到「没有消息框、也没有新窗口类」为止
# （每轮之间隔 3 秒；轮数用尽就用最后一次的清单）。进度窗口关得慢一点不该被判成残留，但一直不关必须判出来。
# 之前的那份清单取自 <标签>-windows-before.txt（点按钮之前列的）。
settle_windows() {
  local i out
  for i in $(seq 1 "$2"); do
    gui_windows "$NAME-$1-windows-after.txt" >/dev/null
    out=$(windows_judge "$1")
    if printf '%s\n' "$out" | grep -a -q '^MESSAGE_BOXES_AFTER=0$' && printf '%s\n' "$out" | grep -a -q '^NEW_WINDOW_CLASSES_AFTER=$'; then break; fi
    sleep 3
  done
  grep -a -E '^WIN' "$EV/$NAME-$1-windows-after.txt" | cut -c1-170 | sed 's/^/  WINDOW_AFTER /'
  say "WINDOWS_SETTLED_AFTER=$i round(s)"
}

# result_box_button_id <已解码的窗口清单>：结果框里**唯一**那颗可见按钮的控件编号（空格分隔；没有或不止一颗时调用方自己判断）。
# ★ 实测「执行完成」框的「确定」编号是 2，不是消息框惯用的 1——要点的编号从清单里读，不要猜。
result_box_button_id() { printf '%s\n' "$1" | sed -n 's/^  id=\([0-9]*\) class=Button visible=True .*/\1/p' | tr '\n' ' ' | sed 's/ $//'; }

# gui_close：结束界面程序（以系统身份结束进程；不弹任何窗口）。
gui_close() { vrun "taskkill /im BackupRestore.exe /f" 3 | grep -a 'VM_DONE'; }

# gui_must_click <轮次标签> <类名> <控件编号>：点击必须被确认（CLICK=OK）；没确认就打印原因并返回非零，
# 调用方必须据此中止本轮。★ 带着没点成的一步往下走，只会干等一个不会出现的对话框
#   （2026-11-25 实测：点「创建任务」那一步通道静默，脚本干等了 40 秒）。
# ★ 回显先解码：助手回传的窗口标题是系统本地编码，原样打印出来的话，外层按通用编码查看的管道会报
#   「非法字节序列」而中断，连带把正在跑的脚本弄断（2026-11-25 实测：点完「否」之后脚本就这样死了一回）。
gui_must_click() {
  local out
  out=$(gui_click "$2" "$3" | decode_text)
  echo "$out"
  case "$out" in
    *CLICK=OK*) return 0 ;;
    *) say "ABORT $1: the click on control $3 was not confirmed (the channel did not run, or the helper refused to click)"; return 1 ;;
  esac
}

# gui_task_list：任务目录编号列表（排序后，空格分隔）——比对「前后有没有多出任务」用
gui_task_list() { vrun "dir /b /ad H:\\brwork\\tasks" 3 | grep -a -E '^[0-9a-f]{8}-' | sort | tr '\n' ' '; }
