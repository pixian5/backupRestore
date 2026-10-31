#!/usr/bin/env bash
# preflight.sh —— 开工前自检：**我接下来要测的，是不是最新那份载荷/部署？**
#
# 为什么需要它（2026-10-31 血的教训）：
#   测 G2 治本时，VM 里的 `H:\brwork\BackupRestore.exe` 是「治标已部署、治本还没写」的
#   中间态构建（mtime 22:36:43，而修复提交是 22:40:28，早 4 分钟）。用它跑同一套夹具，
#   结论是「治本失效」——**纯属测了一个不含修复的二进制**。
#   这类错误最贵的地方在于：它看起来像产品缺陷，会把人引去改代码。
#
# 判据（都不读程序自陈）：
#   1) VM 可达（`prlctl exec` 有约 1/8 概率**静默不执行**：空输出 + 退出码 0，看起来和
#      「真没有」一模一样，所以每个探测都要求回显 `__END__` 才算真跑过，最多 3 次）
#   2) 部署件 mtime **不早于**「最后一次改动产品代码的提交」（crates/ VERSION Cargo.toml
#      Cargo.lock）→ 否则判定「部署过期」。★ 基准**不是 HEAD**：纯文档/工具提交不改二进制，
#      拿 HEAD 当基准会让每次写完文档都变红，逼人做无意义的重构建。
#   3) 部署件里能不能搜到 VERSION 文件里的版本号（版本号对不上立刻报）
#   4) 宿主构建产物与部署件的尺寸差（**仅提示**：发布唯一路径是 VM 内构建，
#      宿主 `build-win.sh` 只做类型检查，两者不同属正常）
#
# 读数侧故障 vs 判据不满足（2026-10-31 修订，别混为一谈）：
#   status=error  = 探测没跑通（通道故障），**不代表**部署过期，重跑即可，不要据此重构建；
#   status=fail   = 探测跑通了，结论就是「过期 / 缺版本号」，要动。
#   两者分开报，且 error 必须带 raw= 原始回显，否则没法证明是通道问题。
#
# 用法：
#   tools/vm/preflight.sh          # 检查（默认）
#   BR_VM='Windows 11' tools/vm/preflight.sh
#
# 输出（结构化，供 grep；不输出依赖中文的判断）：
#   ITEM=<id> status=pass|fail|warn|error [key=value ...]
#   PREFLIGHT=pass | fail | error
#
# 退出码：0 = pass；1 = fail（部署过期/版本不符）；2 = ERROR（仪器或通道故障）
set -uo pipefail

VM=${BR_VM:-Windows 11}
DEST='H:\brwork\BackupRestore.exe'
LOCAL='target/aarch64-pc-windows-msvc/release/BackupRestore.exe'

say() { printf '%s\n' "$*"; }
fail=0
warn_reason=''   # 「通过但有一项没验到」——不能让 PREFLIGHT=pass 把它盖掉

# 把多行原始输出压成一行并截断，用于 raw=[...]
oneline() { printf '%s' "$1" | tr '\r\n' '  ' | cut -c1-200; }

# prlctl exec 有约 1/8 概率**静默不执行**（空输出、退出码 0），空输出分不清「真没有」与
# 「命令没跑」。所以每个探测都要求回显 `__END__` 才算真跑过，最多 3 次。
# ★ 三次都没等到哨兵时，**把最后一次的原始回显当标准输出交回去**（return 1），调用方用
#   `if ! x=$(run_ps ...)` 接住即可拿到 raw=。不要用「函数里赋值全局变量」的写法——
#   调用侧是 `$( )`，函数在子 shell 里跑，全局变量赋值全丢（2026-10-31 踩过：raw=[] 永远是空）。
run_ps() {
  local script="$1" out
  for _ in 1 2 3; do
    out=$(prlctl exec "$VM" powershell -NoProfile -Command "$script" 2>&1)
    case "$out" in *__END__*) printf '%s' "$out"; return 0 ;; esac
    sleep 2
  done
  oneline "$out"
  return 1
}

run_cmd() {
  local cmdline="$1" out
  for _ in 1 2 3; do
    out=$(prlctl exec "$VM" cmd /c "$cmdline" 2>&1)
    case "$out" in *__END__*) printf '%s' "$out"; return 0 ;; esac
    sleep 2
  done
  oneline "$out"
  return 1
}

# ---- 1) VM 可达 ----
if ! vm_out=$(run_cmd "echo VM_OK & echo __END__"); then
  say "ITEM=vm status=error reason=no_response attempts=3 raw=[$(oneline "$vm_out")] hint=prlctl_list_or_vm_paused"
  say "PREFLIGHT=error"
  exit 2
fi
case "$vm_out" in
  *VM_OK*) say "ITEM=vm status=pass name=$VM" ;;
  *)
    say "ITEM=vm status=error reason=sentinel_without_vm_ok raw=[$(oneline "$vm_out")]"
    say "PREFLIGHT=error"
    exit 2 ;;
esac

# ---- 2) 部署件 mtime / 尺寸 / 版本号串 ----
want_ver=$(cat VERSION 2>/dev/null | tr -d ' \r\n')
if ! probe=$(run_ps "
\$p='${DEST}'
if (-not (Test-Path -LiteralPath \$p)) { 'EXE=missing'; '__END__'; exit 0 }
\$i = Get-Item -LiteralPath \$p
'EXE=present'
'EXE_EPOCH=' + ([DateTimeOffset]\$i.LastWriteTime).ToUnixTimeSeconds()
'EXE_MTIME=' + \$i.LastWriteTime.ToString('yyyy-MM-dd HH:mm:ss')
'EXE_SIZE=' + \$i.Length
'EXE_SHA=' + (Get-FileHash -LiteralPath \$p -Algorithm SHA256).Hash.ToLower()
'__END__'
"); then
  say "ITEM=deploy_exe status=error reason=probe_failed attempts=3 raw=[$(oneline "$probe")]"
  say "  >> 这是**读数侧**故障（通道没执行），不代表部署过期，别据此重构建；重跑本脚本即可。"
  say "PREFLIGHT=error"
  exit 2
fi
case "$probe" in
  *EXE=missing*)
    say "ITEM=deploy_exe status=fail reason=missing path=$DEST"
    say "PREFLIGHT=fail"
    exit 1 ;;
  *EXE=present*) ;;
  *)
    say "ITEM=deploy_exe status=error reason=marker_absent raw=[$(oneline "$probe")]"
    say "PREFLIGHT=error"
    exit 2 ;;
esac
exe_epoch=$(printf '%s' "$probe" | grep -oE 'EXE_EPOCH=[0-9]+' | head -1 | cut -d= -f2)
exe_mtime=$(printf '%s' "$probe" | grep -oE 'EXE_MTIME=.*' | head -1 | cut -d= -f2-)
exe_size=$(printf '%s' "$probe" | grep -oE 'EXE_SIZE=[0-9]+' | head -1 | cut -d= -f2)
exe_sha=$(printf '%s' "$probe" | grep -oE 'EXE_SHA=[0-9a-f]+' | head -1 | cut -d= -f2)
say "ITEM=deploy_exe status=present mtime=$exe_mtime epoch=$exe_epoch size=$exe_size"

# ---- 3) 与「最后一次改动产品代码的提交」比时间 ----
# ★ 基准不是 HEAD，而是**最后一次碰到产品输入的那次提交**（2026-10-31 修订）。
#   理由：仓库里绝大多数提交是文档/工具，它们不改变二进制内容。若拿 HEAD 当基准，
#   每次写完文档 preflight 都会变红，逼人去重构建——正好落进「假红导致瞎折腾」。
#   产品输入 = 会进二进制的那些路径：crates/、VERSION、Cargo.toml、Cargo.lock。
PRODUCT_INPUTS='crates VERSION Cargo.toml Cargo.lock'
head_epoch=$(git log -1 --format=%ct 2>/dev/null)
head_ref=$(git log -1 --format='%h' 2>/dev/null)
head_when=$(git log -1 --format='%ad' --date=format:'%Y-%m-%d %H:%M:%S' 2>/dev/null)
prod_epoch=$(git log -1 --format=%ct -- $PRODUCT_INPUTS 2>/dev/null)
prod_ref=$(git log -1 --format='%h' -- $PRODUCT_INPUTS 2>/dev/null)
prod_when=$(git log -1 --format='%ad' --date=format:'%Y-%m-%d %H:%M:%S' -- $PRODUCT_INPUTS 2>/dev/null)
# 兜底：仓库里没有任何提交碰过产品输入（异常）时退回 HEAD，宁可严不可松。
if [ -z "${prod_epoch:-}" ]; then
  prod_epoch=$head_epoch; prod_ref=$head_ref; prod_when=$head_when
  basis='head_fallback'
else
  basis='product_code'
fi
if [ -z "${exe_epoch:-}" ] || [ -z "${prod_epoch:-}" ]; then
  say "ITEM=deploy_freshness status=error reason=cannot_compare"
  say "PREFLIGHT=error"
  exit 2
fi
say "ITEM=head_commit ref=$head_ref time=$head_when epoch=$head_epoch"
note=''
if [ "$head_ref" != "$prod_ref" ]; then
  note=' note=HEAD_newer_but_no_product_change'
fi
if [ "$exe_epoch" -ge "$prod_epoch" ]; then
  say "ITEM=deploy_freshness status=pass basis=$basis ref=$prod_ref ref_time=$prod_when${note}"
else
  delta=$((prod_epoch - exe_epoch))
  say "ITEM=deploy_freshness status=fail reason=deployed_older_than_product_commit basis=$basis ref=$prod_ref ref_time=$prod_when delta_sec=$delta${note}"
  say "  >> 部署件比「最后一次改动产品代码的提交」旧 ${delta} 秒：它很可能**不含这次改动**，此刻跑出来的结论不作数。"
  say "  >> 按项目规矩（2026-10-15 用户拍板）发布唯一路径 = VM 内 Windows 构建，先重构建再部署。"
  fail=1
fi

# ---- 4) 部署件里的版本号串 ----
if [ -n "$exe_sha" ] && [ -n "$want_ver" ]; then
  has_ver=''
  out=''
  if out=$(run_cmd "findstr /C:\"$want_ver\" ${DEST} >nul 2>&1 && (echo VER_HIT & echo __END__) || (echo VER_MISS & echo __END__)"); then
    case "$out" in
      *VER_HIT*) has_ver=hit ;;
      *VER_MISS*) has_ver=miss ;;
      *) has_ver='' ;;
    esac
  fi
  case "$has_ver" in
    hit) say "ITEM=deploy_version status=pass want=$want_ver hint=version_string_not_sufficient" ;;
    miss)
      say "ITEM=deploy_version status=fail want=$want_ver reason=version_string_absent"
      say "  >> 部署件里搜不到 VERSION 的 ${want_ver}：基本可以确定是旧版本。"
      fail=1 ;;
    *) say "ITEM=deploy_version status=warn reason=probe_failed attempts=3 raw=[$(oneline "$out")] hint=not_checked_not_absent"
       warn_reason='deploy_version_probe_failed' ;;
  esac
else
  say "ITEM=deploy_version status=warn reason=no_version_file_or_hash"
  warn_reason='no_version_file'
fi

# ---- 5) 宿主构建产物对比（仅提示） ----
if [ -f "$LOCAL" ]; then
  local_size=$(wc -c < "$LOCAL" | tr -d ' ')
  if [ "${local_size:-0}" = "${exe_size:-0}" ]; then
    say "ITEM=local_artifact status=same size=$local_size"
  else
    say "ITEM=local_artifact status=differs local_size=$local_size deployed_size=$exe_size"
    say "  >> 只是提示：宿主 build-win.sh 仅做类型检查，与 VM 内构建不同属正常，不要据此判断新鲜度。"
  fi
else
  say "ITEM=local_artifact status=absent"
fi

if [ "$fail" -gt 0 ]; then
  say "PREFLIGHT=fail"
  exit 1
fi
say "PREFLIGHT=pass${warn_reason:+ warn=$warn_reason}"
exit 0
