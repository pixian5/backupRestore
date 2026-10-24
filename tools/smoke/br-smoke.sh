#!/usr/bin/env bash
# br-smoke.sh —— deploy 后最小冒烟套件（D5）
#
# 定位：每次 build-win.sh --deploy 之后的发布门禁。约 10-15 分钟，
# 覆盖「数据备份（WinRE 捕获）→ 在线格式化还原（apply 主链）」端到端，
# 六项判据全自动回读，末行输出 PASS/FAIL 结论。
#
# 为什么走 CLI 而不是 GUI 点击链（br-s1.sh）：
#   冒烟门禁的核心目的是 D1/D2 改过的 apply 主链回归——CLI 与 GUI 走的是
#   同一条 execute_inner 代码路径（D1/D2 三层验收即 CLI 实机验证）；
#   GUI 外壳（破坏性弹窗、下拉回读）属 UI 层回归，频率低、且依赖前台焦点，
#   不适合每次 deploy 自动跑。需要 GUI 专项时手动跑 tools/win-clicker/br-s1.sh。
#
# 用法：
#   tools/smoke/br-smoke.sh                    # 全链：备份 + 还原 + 判据
#   tools/smoke/br-smoke.sh --skip-backup      # 只跑还原步（用 --image 指定镜像）
#   tools/smoke/br-smoke.sh --drive T          # 测试卷盘符（默认 T）
#   tools/smoke/br-smoke.sh --keep             # 保留 T: 现场与镜像（调试用）
#   tools/smoke/br-smoke.sh --vm "Windows 11"  # 虚拟机名（默认 "Windows 11"）
#
# 退出码：0 = 六项判据全过；1 = 有判据失败；2 = 前置检查失败（未动 T:）。
#
# 纪律：只动测试卷（默认 T:）；不拍快照；跑前截图确认 VM 空闲；
#       判据不过时 T: 保持现场待查（除非传 --force-clean）。

set -u

VM="Windows 11"
DRIVE="T"
SMOKE_TAG="br-smoke-$(date +%Y%m%d-%H%M)"
KEEP=0
SKIP_BACKUP=0
FORCE_CLEAN=0
IMAGE=""
ART=".test-artifacts/smoke-$(date +%Y%m%d-%H%M%S)"

while [ $# -gt 0 ]; do
    case "$1" in
        --vm) VM="$2"; shift 2 ;;
        --drive) DRIVE="$2"; SMOKE_TAG="br-smoke-${DRIVE}-$(date +%Y%m%d-%H%M)"; shift 2 ;;
        --image) IMAGE="$2"; shift 2 ;;
        --skip-backup) SKIP_BACKUP=1; shift ;;
        --keep) KEEP=1; shift ;;
        --force-clean) FORCE_CLEAN=1; shift ;;
        -h|--help) sed -n '2,26p' "$0"; exit 0 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

[ -n "$IMAGE" ] || IMAGE="H:\\BRImages\\${SMOKE_TAG}.wim"
[ "$SKIP_BACKUP" = "1" ] && [ -z "${IMAGE:-}" ] && { echo "--skip-backup requires --image" >&2; exit 2; }

mkdir -p "$ART"
PASS=0; FAIL=0
declare -a VERDICTS

log()  { echo "[smoke $(date +%H:%M:%S)] $*"; }
die()  { echo "[smoke FATAL] $*" >&2; exit 2; }
verdict() { # verdict <名称> <PASS|FAIL> <证据摘要>
    local name="$1" res="$2" detail="$3"
    VERDICTS+=("$(printf '%-28s %-4s %s' "$name" "$res" "$detail")")
    if [ "$res" = "PASS" ]; then PASS=$((PASS+1)); else FAIL=$((FAIL+1)); fi
}

# ---------- VM 通道（编码策略只在这里定，调用处不再各自打补丁） ----------
# 2026-10-24 重构，全部实测（mac 宿主 / Windows 11 / prlctl exec）。
#
# 【PowerShell 侧：四件套，缺一不可】PowerShell 5.1 的默认值全是错的（实测）：
#   $OutputEncoding            = us-ascii   ← 向外部程序喂管道数据时中文会变 "?"
#   [Console]::OutputEncoding  = gb2312     ← PS 写 stdout 的编码，乱码根源
#   [Console]::InputEncoding   = gb2312     ← 读管道输入
#   再加上 $ProgressPreference，否则 PS 把 progress 流序列化成 CLIXML 混混输出。
# 所以 PS_PRE 一次设全，所有 PowerShell 调用统一带这个前缀。
#
# 【CMD 侧：无法统一，只能不依赖】实测三条，别再试了：
#   · `chcp 65001` 本身生效（`chcp` 回显 "Active code page: 65001"），
#     但 `dir /b T:\`（目录名含中文）的输出字节与 936 下**逐字节相同**（GBK）
#     → prlctl 的 stdout 是管道，cmd 内建命令对管道按系统 ANSI(936) 落字节，
#       chcp 只改控制台代码页、管不到这条路。
#   · `cmd /u`（Unicode 重定向）经该通道**输出直接丢失**，不可用。
#   · `cmd /c type <file>` 是字节透传，不受上述任何影响 —— 这是回读文件的正确通道。
# 结论：cmd 只用来「搬字节」和「做 ASCII 判定」；要读数据一律用 PowerShell 的
# .NET 等价命令（Get-ChildItem / Get-Volume / Get-Content …）。
#
# 【其它两条实测】
#   · prlctl stdout 字节透传、无长度上限：30KB ASCII / 6KB UTF-8 中文逐字节完整
#     → 回读文件不需要 base64 往返（旧版"~2.5KB 上限"是 base64 被 CLIXML 切碎的假象）。
#   · 命令行参数里不能带中文：prlctl 传 UTF-8 字节、VM 侧 cmd 按 936 解码。
#     需要中文一律走 -EncodedCommand（UTF-16LE base64）或从文件读。
#   · VM 可直读宿主共享目录（\\Mac\Home 或 \\psf\Home），脚本零传输，
#     顺带绕开命令行 ~3.4KB 上限；共享被关才回落到 base64 分块部署。
#   · 文件落盘显式无 BOM：WriteAllText(..., UTF8Encoding($false))。
#     PS 5.1 的 `Out-File -Encoding UTF8` 会写 BOM，宿主侧表现为行首 U+FEFF，
#     字段/行比较直接失配。
PS_PRE='$ProgressPreference="SilentlyContinue"; [Console]::OutputEncoding=[Text.Encoding]::UTF8; [Console]::InputEncoding=[Text.Encoding]::UTF8; $OutputEncoding=[Text.Encoding]::UTF8;'

ps_b64() { python3 -c "import base64,sys; print(base64.b64encode(sys.stdin.read().encode('utf-16-le')).decode())"; }

vm_cmd() { # cmd 内建命令（单写反斜杠纪律）。★ 中文输出恒为 GBK 且 chcp 救不了
    # （见上方实测），所以这里拿到的中文不可解析；只用于搬字节与 ASCII 判定，
    # 需要读数据一律改用 vm_ps_raw。
    prlctl exec "$VM" cmd /d /c "$1" 2>&1 | LC_ALL=C tr -d '\r' || true
}

vm_ps_raw() { # VM 内 PowerShell 短命令（≤2KB）：输出 UTF-8，可安全回读中文
    prlctl exec "$VM" cmd /d /c "powershell -NoProfile -EncodedCommand $(printf '%s' "${PS_PRE} ${1}" | ps_b64)" 2>/dev/null \
        | LC_ALL=C tr -d '\r' | grep -av '^<' | grep -av '^$' || true
}

vm_ps_exec() { # 同上但丢弃输出（执行 helper 用，结果另由 DONE 标记回读）
    prlctl exec "$VM" cmd /d /c "powershell -NoProfile -EncodedCommand $(printf '%s' "${PS_PRE} ${1}" | ps_b64)" > /dev/null 2>&1 || true
}

vm_read_file() { # 读 VM 内文件：type 字节透传（不转码），删 CR 便于按行处理
    # prlctl 通道偶发返回空（实测同一文件连续两次一空一满），空结果必须重试，
    # 否则调用方会把「通道抽风」当成「文件为空」。路径按调用方给定原样拼入
    # （当前调用点路径均无空格）。
    local path="$1" out="" t
    for t in 1 2 3; do
        out=$(prlctl exec "$VM" cmd /d /c "type ${path}" 2>/dev/null | LC_ALL=C tr -d '\r' || true)
        [ -n "$out" ] && break
        sleep 3
    done
    printf '%s' "$out"
}

vm_len_of() { # VM 内文件字节长度（纯数字输出）
    vm_ps_raw "(Get-Item '${1}').Length" | LC_ALL=C tr -d '\r\n'
}

vm_b64_len() { # VM 内 .b64 去空白后的字符数（仅 fallback 部署校验用）
    vm_ps_raw "(((Get-Content '${1}' -Raw) -replace '\\s','')).Length" | LC_ALL=C tr -d '\r\n'
}

# ---- 宿主脚本目录 → VM 共享路径（避免任何文件传输） ----
HOST_SMOKE_DIR=$(cd "$(dirname "$0")" && pwd)
SHARE_ROOT=""   # 形如 \\Mac\Home\code\backupRestore\tools\smoke

detect_share() { # 探测 VM 能否直读宿主脚本目录（Parallels 共享文件夹）
    local rel cand
    rel=$(printf '%s' "$HOST_SMOKE_DIR" | sed 's|^/Users/[^/]*||; s|/|\\|g')
    for cand in '\\Mac\Home' '\\psf\Home'; do
        if vm_cmd "if exist \"${cand}${rel}\\br-smoke-helper-seed.ps1\" (echo SHARE_OK) else (echo SHARE_NO)" | grep -q SHARE_OK; then
            SHARE_ROOT="${cand}${rel}"
            return 0
        fi
    done
    return 1
}

vm_put_file() { # 宿主机文件 → VM【仅当共享目录不可用时的回落路径】
    # 分块 echo 写 .b64 再解码，规避命令行长度上限；每轮校验
    # 「b64 字符数 + 解码后字节数」，不符整轮重来（通道会瞬时丢块）。
    local src="$1" dst="$2" b64tmp="$3"
    local b64
    b64=$(base64 -i "$src" | LC_ALL=C tr -d '\n')
    local want=${#b64}
    local want_bytes
    want_bytes=$(wc -c < "$src" | tr -d ' ')
    local attempt got dsz i first part chunk=1200
    for attempt in 1 2 3; do
        vm_cmd "del /q ${b64tmp} 2>nul >nul" > /dev/null 2>&1
        i=0; first=1
        while [ "$i" -lt "$want" ]; do
            part="${b64:$i:$chunk}"
            if [ "$first" = "1" ]; then
                vm_cmd "echo ${part}> ${b64tmp}" > /dev/null 2>&1; first=0
            else
                vm_cmd "echo ${part}>> ${b64tmp}" > /dev/null 2>&1
            fi
            i=$((i + chunk))
        done
        got=$(vm_b64_len "$b64tmp")
        if [ "$got" = "$want" ]; then
            vm_ps_exec "[IO.File]::WriteAllBytes('${dst}',[Convert]::FromBase64String((Get-Content '${b64tmp}' -Raw)))"
            dsz=$(vm_len_of "$dst")
            if [ "$dsz" = "$want_bytes" ]; then
                return 0
            fi
            log "  ⚠ ${dst} 解码后 ${dsz} != ${want_bytes} 字节，重试"
        else
            log "  ⚠ ${b64tmp} 分块写入 ${got}/${want} 字符，重试"
        fi
        sleep 3
    done
    return 1
}

vm_run_helper() { # <helper 文件名> <marker 文件> <变量注入语句>
    # 源码优先级：共享目录直读（零传输）→ H:\brwork 已部署副本。
    # iex 经通道偶发不生效（同命令手动重跑即成功），故以 helper 自写的
    # DONE 标记为成功判据，最多重试 3 次。
    local name="$1" marker="$2" vars="${3:-}"
    local src t m
    if [ -n "$SHARE_ROOT" ]; then src="${SHARE_ROOT}\\${name}"; else src="H:\\brwork\\${name}"; fi
    for t in 1 2 3; do
        vm_cmd "del /q ${marker} 2>nul >nul" > /dev/null 2>&1
        vm_ps_exec "\$c = Get-Content -Raw -Encoding UTF8 '${src}'; ${vars} iex \$c"
        m=$(vm_read_file "$marker" | LC_ALL=C tr -d '\r\n')
        if echo "$m" | grep -q "DONE"; then
            return 0
        fi
        log "  ⚠ helper 未产出 DONE（第 ${t} 次，标记=${m:-空}），重试"
        sleep 4
    done
    return 1
}

deploy_helpers() { # 让 helper 就位：优先共享目录直读，否则回落到 H:\brwork 部署
    if detect_share; then
        log "  helper 走共享目录直读（零传输）：${SHARE_ROOT}"
        return 0
    fi
    log "  ⚠ 共享目录不可用（Parallels 共享被关？）→ 回落 base64 分块部署"
    local base
    base="$HOST_SMOKE_DIR"
    vm_put_file "$base/br-smoke-helper-seed.ps1" 'H:\brwork\br-smoke-helper-seed.ps1' 'H:\brwork\smoke-seed.b64' \
        || die "helper seed.ps1 部署失败（分块写校验未通过）"
    vm_put_file "$base/br-smoke-helper-collect.ps1" 'H:\brwork\br-smoke-helper-collect.ps1' 'H:\brwork\smoke-collect.b64' \
        || die "helper collect.ps1 部署失败（分块写校验未通过）"
}

# ---------- STEP 0：前置检查 ----------
precheck() {
    log "STEP 0 前置检查"
    prlctl list -a 2>/dev/null | grep -q running || die "VM $VM 未运行"
    prlctl capture "$VM" --file "$ART/precheck.png" > /dev/null 2>&1 || die "截图失败"
    local free
    # fsutil 的输出是本地化 GBK，宿主 grep 中文会失配（旧版一直显示空），
    # 改从 PowerShell 取数字格式化（走 EncodedCommand，中文安全）。
    free=$(vm_ps_raw "\$v = Get-Volume -DriveLetter ${DRIVE}; '{0:N1} GiB 可用 / {1:N1} GiB' -f (\$v.SizeRemaining/1GB), (\$v.Size/1GB)" | tr -d '\r\n')
    log "  VM 运行中；${DRIVE}: ${free:-<读不到>}（截图 $ART/precheck.png）"
    # 宿主空间（Data 卷才是真实空闲）
    local hostfree
    hostfree=$(df -m /System/Volumes/Data 2>/dev/null | tail -1 | awk '{print $4}')
    [ -n "$hostfree" ] && [ "$hostfree" -lt 5120 ] && die "宿主 /System/Volumes/Data 空闲不足 5Gi（${hostfree}Mi）——先空间治理"
    log "  宿主空闲 ${hostfree}Mi"
    # 部署件存在性（冒烟依赖 H:\brwork 五件套；prlctl 通道偶发抽风，重试 3 次）
    local dep_ok=0
    for _try in 1 2 3; do
        vm_cmd "if exist H:\\brwork\\BackupRestore.exe (echo OK) else (echo MISSING)" | grep -q OK && { dep_ok=1; break; }
        sleep 4
    done
    [ "$dep_ok" = "1" ] || die "H:\\brwork\\BackupRestore.exe 缺失——先 ./build-win.sh --deploy"
    log "  部署件在位"
}

# ---------- STEP 1：备料（T: 造哨兵/BR38Data/100MB 载荷） ----------
seed() {
    log "STEP 1 备料（${DRIVE}: 哨兵 + BR38Data + 100MB 载荷）"
    vm_run_helper 'br-smoke-helper-seed.ps1' 'H:\brwork\smoke-seed-run.txt' \
        "\$Drive='${DRIVE}'; \$Out='H:\\brwork\\smoke-pre.json';" \
        || die "备料 helper 执行失败（3 次均未产出 DONE 标记）"
    local out
    out=$(vm_read_file 'H:\brwork\smoke-pre.json')
    echo "$out" | python3 -c "import json,sys; d=json.loads(sys.stdin.read().lstrip('\\ufeff')); print('  sentinel=%s serial=%s bigsha=%s… len=%s' % (d['sentinel'], d['serial'], d['bigsha'][:12], d['biglen']))" 2>/dev/null \
        || die "备料失败（VM 输出: ${out}）"
    echo "$out" > "$ART/pre.json"
}

# ---------- STEP 2：备份（WinRE 全链，含重启等待） ----------
# prepare --operation backup 会 shutdown /r 进 WinRE 执行捕获，完成后自动
# 重启回 Windows 续跑收尾。prlctl exec 在重启窗口不可用，需轮询任务终态。
BACKUP_OK=0
backup() {
    log "STEP 2 备份 prepare --operation backup（将重启进 WinRE，全程约 5-12 分钟）"
    # 任务目录快照（用于定位本次备份任务）；无 BOM 写入，否则首行任务名带
    # U+FEFF 会让下面的 -notcontains 匹配不上、把旧任务误判成本次任务。
    vm_ps_raw "[IO.File]::WriteAllLines('H:\brwork\smoke-tasks-before.txt', @(Get-ChildItem H:\brwork\tasks -Directory | Select-Object -ExpandProperty Name), (New-Object Text.UTF8Encoding \$false))" > /dev/null 2>&1
    vm_cmd "H:\\brwork\\BackupRestore.exe prepare --operation backup --source-drive ${DRIVE} --image-path ${IMAGE}" \
        > "$ART/backup.out" 2>&1
    log "  prepare 已返回（VM 进入重启序列），开始轮询任务终态"
    local deadline=$(( $(date +%s) + 1500 ))
    local task_dir="" stage="" progress=""
    while [ "$(date +%s)" -lt "$deadline" ]; do
        sleep 15
        local probe
        probe=$(vm_cmd "echo ready") || true
        if ! echo "$probe" | grep -aq "ready"; then
            log "  …VM 重启中（exec 暂不可用）"
            continue
        fi
        # 任务 state 直接走 stdout 回读（UTF-8 通道已通，无需中间文件）。
        local line
        line=$(vm_ps_raw '$d = Get-ChildItem H:\brwork\tasks -Directory | Where-Object { (Get-Content H:\brwork\smoke-tasks-before.txt) -notcontains $_.Name } | Sort-Object LastWriteTime | Select-Object -Last 1; if ($d) { $s = Get-Content "$($d.FullName)\status.json" -Raw | ConvertFrom-Json; "$($d.Name)|$($s.stage)|$($s.progress)" } else { "NONE" }' | LC_ALL=C tr -d '\r\n')
        if [ -z "$line" ] || [ "$line" = "NONE" ]; then
            log "  …exec 已恢复，等待任务目录出现"
            continue
        fi
        IFS='|' read -r task_dir stage progress <<< "$line"
        log "  任务 ${task_dir}：stage=${stage} progress=${progress}"
        if [ "$stage" = "success" ] && [ "$progress" = "100" ]; then
            BACKUP_OK=1; break
        fi
        case "$stage" in
            failed|aborted|error) break ;;
        esac
    done
    if [ "$BACKUP_OK" != "1" ]; then
        verdict "备份任务成功" FAIL "stage=$stage progress=${progress}（任务 ${task_dir:-未创建}，输出见 $ART/backup.out）"
        return 1
    fi
    # 镜像存在且可读索引（只读命令，通道抽风时重试）
    local info t
    for t in 1 2 3; do
        info=$(vm_cmd "H:\\brwork\\BackupRestore.exe wim-info ${IMAGE}")
        echo "$info" | grep -aqi "index" && break
        log "  ⚠ wim-info 未返回索引（第 ${t} 次），重试"
        sleep 5
    done
    if echo "$info" | grep -aqi "index"; then
        verdict "备份任务成功" PASS "stage=success/100 + wim-info 索引在（$(echo "$info" | grep -ai 'index' | head -1 | cut -c1-50)）"
    else
        verdict "备份镜像可读" FAIL "wim-info 无索引输出"
    fi
}

# ---------- STEP 3：在线还原（格式化 + apply 主链） ----------
restore() {
    log "STEP 3 在线还原 online --operation restore-existing --format-target"
    # 还原命令偶发被通道吞掉（stdout 只剩 "PrlJob_GetResult: Invalid argument"），
    # 判据会连锁失败——按幂等性重试（format+apply 可安全重跑）。
    local t
    for t in 1 2 3; do
        vm_cmd "H:\\brwork\\BackupRestore.exe online --operation restore-existing --source-drive ${DRIVE} --target-drive ${DRIVE} --image-path ${IMAGE} --index 1 --format-target" \
            > "$ART/restore.out" 2>&1
        if grep -aq "日志：" "$ART/restore.out" || grep -aq "操作完成" "$ART/restore.out"; then
            break
        fi
        log "  ⚠ 还原命令未返回完成语（第 ${t} 次：$(head -1 "$ART/restore.out" | cut -c1-60)），重试"
        sleep 6
    done
    # CLI stdout 只有收尾两行（「操作完成」+「日志：<Online.log 路径>」），
    # STEP 明细全在日志文件里——C3 判据必须回读该日志，不能只看 stdout。
    RESTORE_LOG=$(grep -aoE 'H:\\brwork\\logs\\[^ ]*\\Online.log' "$ART/restore.out" | head -1)
    if [ -n "$RESTORE_LOG" ]; then
        # 日志一般几十 KB，通道字节透传可整份回读；超大才在 VM 端截尾。
        local lsz
        lsz=$(vm_len_of "$RESTORE_LOG")
        if [ -n "$lsz" ] && [ "$lsz" -gt 524288 ]; then
            vm_ps_raw "Get-Content '${RESTORE_LOG}' -Tail 400" > "$ART/restore-Online.log"
            log "  任务日志较大（${lsz}B），仅回读尾部 400 行：${RESTORE_LOG}"
        else
            vm_read_file "$RESTORE_LOG" > "$ART/restore-Online.log"
            log "  还原退出，任务日志已回读（${lsz:-?}B）：${RESTORE_LOG}"
        fi
    else
        log "  ⚠ 未能从 stdout 解析任务日志路径（输出见 $ART/restore.out）"
    fi
    log "  stdout：$(head -1 "$ART/restore.out" | cut -c1-80)"
}

# ---------- STEP 4：判据回读（六项全自动） ----------
collect_post() {
    log "STEP 4 判据回读"
    vm_run_helper 'br-smoke-helper-collect.ps1' 'H:\brwork\smoke-collect-run.txt' \
        "\$Drive='${DRIVE}'; \$Pre='H:\\brwork\\smoke-pre.json'; \$Out='H:\\brwork\\smoke-post.json';" \
        || { verdict "判据回读" FAIL "collect helper 执行失败（3 次均未产出 DONE 标记）"; return 1; }
    local post
    post=$(vm_read_file 'H:\brwork\smoke-post.json')
    echo "$post" > "$ART/post.json"
    [ -z "$post" ] && { verdict "判据回读" FAIL "无法读取还原后状态"; return 1; }

    local pre
    pre=$(cat "$ART/pre.json")
    python3 - "$post" "$ART/restore.out" "$ART/restore-Online.log" "$SKIP_BACKUP" <<'PY' > "$ART/verdict.txt"
import json, sys, re, os
post_raw, restore_out, task_log, skip_backup = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
log_text = ''
if os.path.exists(task_log):
    log_text = open(task_log, encoding='utf-8', errors='replace').read()
# 判据以任务日志为主、CLI stdout 为兜底。注意：此处曾被一行
# `log = open(restore_out...)` 覆盖，导致 C3 永远看不到 STEP 4/4 而假性失败。
log = log_text if log_text else open(restore_out, encoding='utf-8', errors='replace').read()
post_d = json.loads(post_raw.lstrip('\ufeff'))
pre_d = {"sentinel": post_d["preSentinel"], "bigsha": post_d["preSha"], "biglen": post_d["preLen"], "serial": post_d["preSerial"]}
rows = []
def v(name, ok, detail): rows.append((name, 'PASS' if ok else 'FAIL', detail))
if skip_backup == '1':
    # 跳过备份时镜像来自上一轮，哨兵文本必然是本轮 seed 之外的旧值；
    # 此时只判「哨兵文件随镜像回到目标卷」。
    v('C1 哨兵重现（跳过备份=存在性）', bool(post_d['sentinel']),
      '镜像内的哨兵已回到目标卷：' + (post_d['sentinel'] or '<空>')[:44])
else:
    v('C1 哨兵重现', bool(post_d['sentinel']) and post_d['sentinel'] == pre_d['sentinel'],
      ('内容一致' if post_d['sentinel'] == pre_d['sentinel'] else '缺失或不一致') + '：' + (post_d['sentinel'] or '<空>')[:40])
v('C2 目标卷序列号变化', bool(post_d['serial']) and post_d['serial'] != pre_d['serial'],
  f"{pre_d['serial']} -> {post_d['serial'] or '<空>'}")
steps = sorted(set(re.findall(r'STEP ([0-9/]+)', log)))
v('C3 任务 success/100', bool(re.search(r'STEP 4/4', log)) and bool(re.search(r'操作完成', log)),
  ('任务日志 STEP ' + '/'.join(steps) + ' 且含「操作完成」') if (re.search(r'STEP 4/4', log) and re.search(r'操作完成', log))
  else ('任务日志仅到 STEP ' + ('/'.join(steps) or '<无>') + '，末行：' + (log.strip().splitlines()[-1][:50] if log.strip() else '<空>')))
v('C4 100MB 载荷字节级一致', post_d['bigsha'] == pre_d['bigsha'] and post_d['biglen'] == pre_d['biglen'],
  f"sha {'一致' if post_d['bigsha'] == pre_d['bigsha'] else '不一致'}，{post_d['biglen']}/{pre_d['biglen']} B")
v('C5 BR38Data 完整', bool(post_d['br38complete']),
  '四项全在' if post_d['br38complete'] else f"marker={post_d['br38']['marker']} f1={post_d['br38']['file1']} f2={post_d['br38']['file2']} nested={post_d['br38']['nested']}")
v('C6 崩溃判据（EventID1000/WerFault）', post_d['evt1000'] == 0 and post_d['werfault'] == 0,
  f"EventID1000={post_d['evt1000']}（2h）WerFault={post_d['werfault']}")
for name, res, detail in rows:
    print(f"{res}\t{name}\t{detail}")
PY
    while IFS=$'\t' read -r res name detail; do
        verdict "$name" "$res" "$detail"
    done < "$ART/verdict.txt"
}

# ---------- STEP 5：清理 ----------
cleanup() {
    if [ "$KEEP" = "1" ]; then
        log "STEP 5 --keep：保留 $DRIVE: 现场与镜像 $IMAGE"
        return 0
    fi
    if [ "$FAIL" -gt 0 ] && [ "$FORCE_CLEAN" != "1" ]; then
        log "STEP 5 判据有失败——$DRIVE: 现场保留待查（--force-clean 可强制清理）"
        return 0
    fi
    log "STEP 5 清理：$DRIVE: 格式化回空卷"
    vm_cmd "echo select volume ${DRIVE}> H:\\brwork\\fmt-smoke.txt && echo format fs=ntfs quick override>> H:\\brwork\\fmt-smoke.txt && diskpart /s H:\\brwork\\fmt-smoke.txt" > "$ART/format.out" 2>&1
    vm_cmd "del /q H:\\brwork\\fmt-smoke.txt H:\\brwork\\smoke-pre.json H:\\brwork\\smoke-post.json" > /dev/null 2>&1
    # diskpart 的输出是本地化 GBK 字节流：中文直配不可靠，且 UTF-8 locale 下
    # grep 会因非法字节整行跳过（实测 "100" 都搜不到），故用 LC_ALL=C 按字节匹配。
    LC_ALL=C grep -aq "100" "$ART/format.out" && log "  $DRIVE: 已清回空卷" || log "  ⚠ 格式化输出待人工确认：$ART/format.out"
    if [ "$SKIP_BACKUP" != "1" ]; then
        vm_cmd "del /q ${IMAGE} ${IMAGE}.index-1.metadata.json" > /dev/null 2>&1
        log "  冒烟镜像已删（--keep 可保留）"
    fi
}

# ---------- 汇总 ----------
summary() {
    echo
    echo "================ 冒烟判据汇总（${SMOKE_TAG}）================"
    local line
    for line in "${VERDICTS[@]:-}"; do
        [ -n "$line" ] && echo "  $line"
    done
    echo "-----------------------------------------------------------"
    if [ "$FAIL" -eq 0 ]; then
        echo "  PASS（${PASS}/${PASS}）—— apply 主链冒烟通过，证据目录 $ART"
        exit 0
    else
        echo "  FAIL（$PASS 过 / $FAIL 败）—— 证据目录 ${ART}，${DRIVE}: 现场见 STEP 5 说明"
        exit 1
    fi
}

# ---------- 主流程 ----------
precheck
deploy_helpers
seed
if [ "$SKIP_BACKUP" != "1" ]; then
    backup || true
else
    log "STEP 2 --skip-backup：跳过备份，直接用 --image ${IMAGE}"
fi
restore
collect_post
cleanup
summary
