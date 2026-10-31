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
#   1) VM 可达（`prlctl exec` 有约 1/8 概率不执行，必须看到回显才算通）
#   2) 部署件 mtime **不早于** HEAD 提交时间  → 否则判定「部署过期」
#   3) 部署件里能不能搜到 VERSION 文件里的版本号（版本号对不上立刻报）
#   4) 宿主构建产物与部署件的尺寸差（**仅提示**：发布唯一路径是 VM 内构建，
#      宿主 `build-win.sh` 只做类型检查，两者不同属正常）
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

# ---- 1) VM 可达 ----
reach=''
for _ in 1 2 3; do
  out=$(prlctl exec "$VM" cmd /c "echo VM_OK" 2>&1)
  case "$out" in *VM_OK*) reach=yes; break ;; esac
  sleep 2
done
if [ "$reach" != yes ]; then
  say "ITEM=vm status=error reason=no_response hint=prlctl_list_or_vm_paused"
  say "PREFLIGHT=error"
  exit 2
fi
say "ITEM=vm status=pass name=$VM"

# ---- 2) 部署件 mtime / 尺寸 / 版本号串 ----
want_ver=$(cat VERSION 2>/dev/null | tr -d ' \r\n')
probe=$(prlctl exec "$VM" powershell -NoProfile -Command "
\$p='${DEST}'
if (-not (Test-Path -LiteralPath \$p)) { 'EXE=missing'; exit 0 }
\$i = Get-Item -LiteralPath \$p
'EXE=present'
'EXE_EPOCH=' + ([DateTimeOffset]\$i.LastWriteTime).ToUnixTimeSeconds()
'EXE_MTIME=' + \$i.LastWriteTime.ToString('yyyy-MM-dd HH:mm:ss')
'EXE_SIZE=' + \$i.Length
'EXE_SHA=' + (Get-FileHash -LiteralPath \$p -Algorithm SHA256).Hash.ToLower()
" 2>&1)
case "$probe" in
  *EXE=missing*)
    say "ITEM=deploy_exe status=fail reason=missing path=$DEST"
    say "PREFLIGHT=fail"
    exit 1 ;;
  *EXE=present*) ;;
  *)
    say "ITEM=deploy_exe status=error reason=probe_failed"
    say "PREFLIGHT=error"
    exit 2 ;;
esac
exe_epoch=$(printf '%s' "$probe" | grep -oE 'EXE_EPOCH=[0-9]+' | head -1 | cut -d= -f2)
exe_mtime=$(printf '%s' "$probe" | grep -oE 'EXE_MTIME=.*' | head -1 | cut -d= -f2-)
exe_size=$(printf '%s' "$probe" | grep -oE 'EXE_SIZE=[0-9]+' | head -1 | cut -d= -f2)
exe_sha=$(printf '%s' "$probe" | grep -oE 'EXE_SHA=[0-9a-f]+' | head -1 | cut -d= -f2)
say "ITEM=deploy_exe status=present mtime=$exe_mtime epoch=$exe_epoch size=$exe_size"

# ---- 3) 与 HEAD 提交时间比 ----
head_epoch=$(git log -1 --format=%ct 2>/dev/null)
head_ref=$(git log -1 --format='%h' 2>/dev/null)
head_when=$(git log -1 --format='%ad' --date=format:'%Y-%m-%d %H:%M:%S' 2>/dev/null)
if [ -z "${exe_epoch:-}" ] || [ -z "${head_epoch:-}" ]; then
  say "ITEM=deploy_freshness status=error reason=cannot_compare"
  say "PREFLIGHT=error"
  exit 2
fi
say "ITEM=head_commit ref=$head_ref time=$head_when epoch=$head_epoch"
if [ "$exe_epoch" -ge "$head_epoch" ]; then
  say "ITEM=deploy_freshness status=pass"
else
  delta=$((head_epoch - exe_epoch))
  say "ITEM=deploy_freshness status=fail reason=deployed_older_than_HEAD delta_sec=$delta"
  say "  >> 部署件比最新提交旧 ${delta} 秒：它很可能**不含这次改动**，此刻跑出来的结论不作数。"
  say "  >> 按项目规矩（2026-10-15 用户拍板）发布唯一路径 = VM 内 Windows 构建，先重构建再部署。"
  fail=1
fi

# ---- 4) 部署件里的版本号串 ----
if [ -n "$exe_sha" ] && [ -n "$want_ver" ]; then
  has_ver=''
  for _ in 1 2 3; do
    out=$(prlctl exec "$VM" cmd /c "findstr /C:\"$want_ver\" ${DEST} >nul 2>&1 && echo VER_HIT || echo VER_MISS" 2>&1)
    case "$out" in
      *VER_HIT*) has_ver=hit; break ;;
      *VER_MISS*) has_ver=miss; break ;;
    esac
    sleep 2
  done
  case "$has_ver" in
    hit) say "ITEM=deploy_version status=pass want=$want_ver hint=version_string_not_sufficient" ;;
    miss)
      say "ITEM=deploy_version status=fail want=$want_ver reason=version_string_absent"
      say "  >> 部署件里搜不到 VERSION 的 $want_ver：基本可以确定是旧版本。"
      fail=1 ;;
    *) say "ITEM=deploy_version status=warn reason=probe_failed" ;;
  esac
else
  say "ITEM=deploy_version status=warn reason=no_version_file_or_hash"
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
say "PREFLIGHT=pass"
exit 0
