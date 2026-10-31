#!/usr/bin/env bash
# postflight.sh —— 收工自检：**这一轮搞完，有没有把残留留在地上？**
#
# 为什么需要它（2026-10-31 用户明确要求「以后每次搞完都要清理残留」）：
#   这台 VM 是长期复用的测试机，临时夹具不清会反过来污染下一轮判据：
#     · 挂着的 WIM 会让下一次 `dism /Mount-Image` 撞车；
#     · `diskpart assign letter=` 留下的**幽灵盘符**（`\DosDevices\V:`）会在删掉分区后
#       继续弹「找不到磁盘」——用户为这类弹窗报过两次；
#     · 遗留的 `*.vhdx` / 挂载目录会被后续脚本当成真实卷；
#     · 卡住的提权 PowerShell 会把下一次 `prlctl exec` 一起拖住（曾卡满 40 秒）。
#
# 用法：
#   tools/vm/postflight.sh            # 只检查（默认，只读）
#   tools/vm/postflight.sh clean      # 清掉**可安全自动清**的两类：卸载的 WIM、幽灵盘符
#                                     # 文件/目录只列出来，不代删（删文件风险不对等）
#   BR_VM='Windows 11' tools/vm/postflight.sh
#
# 输出（结构化，供 grep）：
#   ITEM=<id> status=pass|fail|warn|error [key=value ...]
#   ACTION=<what> detail=<...>          （clean 模式下逐条打印做过什么）
#   POSTFLIGHT=clean | leftovers | error
#
# 退出码：0 = clean；1 = 有残留；2 = ERROR（通道/仪器故障）
set -uo pipefail

VM=${BR_VM:-Windows 11}
MODE=${1:-check}
say() { printf '%s\n' "$*"; }

run_ps() {
  # 通道有约 1/8 概率不执行：必须看到 __END__ 才算真跑过
  local script="$1" out
  for _ in 1 2 3; do
    out=$(prlctl exec "$VM" powershell -NoProfile -Command "$script" 2>&1)
    case "$out" in *__END__*) printf '%s' "$out"; return 0 ;; esac
    sleep 2
  done
  printf ''; return 1
}

probe() {
  run_ps "
\$fmt='{0}:{1}:{2}'
" >/dev/null 2>&1
}

probe_state() {
  run_ps "
\$vol = @(Get-Volume -ErrorAction SilentlyContinue | Where-Object { \$_.DriveLetter } | ForEach-Object { \$_.DriveLetter })
'VOLS=' + ((\$vol | Sort-Object) -join ',')
\$reg = @((Get-ItemProperty 'HKLM:\SYSTEM\MountedDevices' -ErrorAction SilentlyContinue).PSObject.Properties.Name |
         Where-Object { \$_ -match '^\\\\DosDevices\\\\[A-Z]:\$' } |
         ForEach-Object { \$_.Substring(12,1) })
\$ghost = @(\$reg | Where-Object { \$vol -notcontains \$_ } | Sort-Object -Unique)
'GHOST=' + (\$ghost -join ',')
\$mw = (& dism.exe /English /Get-MountedWimInfo 2>&1) | Out-String
if (\$mw -match 'No mounted images') { 'MOUNTED=0' } else { 'MOUNTED=1' }
\$vhd = @(Get-ChildItem 'H:\brwork' -Filter '*.vhdx' -Force -ErrorAction SilentlyContinue)
'STRAY_VHD=' + \$vhd.Count
\$vhd | ForEach-Object { 'STRAY_VHD_ITEM=' + \$_.Name }
\$mnt = @(Get-ChildItem 'H:\brwork' -Directory -Force -ErrorAction SilentlyContinue | Where-Object { \$_.Name -match 'mnt\$' })
'STRAY_MNT=' + \$mnt.Count
\$mnt | ForEach-Object { 'STRAY_MNT_ITEM=' + \$_.Name }
\$img = @(Get-ChildItem 'H:\BRImages' -Force -ErrorAction SilentlyContinue | Where-Object { \$_.Name -match '^g2-|^\.br-online' })
'STRAY_IMG=' + \$img.Count
\$img | ForEach-Object { 'STRAY_IMG_ITEM=' + \$_.Name }
\$ps = @(Get-Process powershell -ErrorAction SilentlyContinue)
'PS_COUNT=' + \$ps.Count
'__END__'
"
}

state=$(probe_state) || { say "ITEM=channel status=error reason=no_response"; say "POSTFLIGHT=error"; exit 2; }
# ★ 去掉 CR：PowerShell 经管道出来是 CRLF，`GHOST=\r` 会被 `-n` 判成「非空」，
#   于是「没有幽灵盘符」被误报成 fail（本次实测踩到）。
state=$(printf '%s' "$state" | tr -d '\r')
case "$state" in
  *__END__*) ;;
  *) say "ITEM=channel status=error reason=no_end_marker"; say "POSTFLIGHT=error"; exit 2 ;;
esac

field() { printf '%s' "$state" | grep -oE "$1=[^ ]*" | head -1 | cut -d= -f2-; }
ghost=$(field GHOST)
mounted=$(field MOUNTED)
n_vhd=$(field STRAY_VHD)
n_mnt=$(field STRAY_MNT)
n_img=$(field STRAY_IMG)
n_ps=$(field PS_COUNT)

say "ITEM=volumes status=info letters=$(field VOLS)"

left=0
if [ "$mounted" = "1" ]; then
  say "ITEM=mounted_wim status=fail reason=still_mounted"
  left=1
else
  say "ITEM=mounted_wim status=pass"
fi

if [ -n "${ghost:-}" ]; then
  say "ITEM=ghost_letter status=fail letters=$ghost"
  say "  >> 这些盘符在注册表 MountedDevices 里，但对应卷已不存在（典型来源 diskpart assign letter）。"
  left=1
else
  say "ITEM=ghost_letter status=pass"
fi

if [ "${n_vhd:-0}" -gt 0 ] 2>/dev/null; then
  say "ITEM=stray_vhd status=fail count=$n_vhd detail=$(printf '%s' "$state" | grep -oE 'STRAY_VHD_ITEM=[^ ]*' | cut -d= -f2- | tr '\n' ',')"
  left=1
else
  say "ITEM=stray_vhd status=pass"
fi

if [ "${n_mnt:-0}" -gt 0 ] 2>/dev/null; then
  say "ITEM=stray_mount_dir status=fail count=$n_mnt detail=$(printf '%s' "$state" | grep -oE 'STRAY_MNT_ITEM=[^ ]*' | cut -d= -f2- | tr '\n' ',')"
  left=1
else
  say "ITEM=stray_mount_dir status=pass"
fi

if [ "${n_img:-0}" -gt 0 ] 2>/dev/null; then
  say "ITEM=stray_image status=fail count=$n_img detail=$(printf '%s' "$state" | grep -oE 'STRAY_IMG_ITEM=[^ ]*' | cut -d= -f2- | tr '\n' ',')"
  left=1
else
  say "ITEM=stray_image status=pass"
fi

if [ "${n_ps:-0}" -gt 2 ] 2>/dev/null; then
  say "ITEM=stray_process status=warn count=$n_ps hint=hung_elevated_powershell"
else
  say "ITEM=stray_process status=pass count=${n_ps:-0}"
fi

if [ "$MODE" = "clean" ]; then
  if [ "$mounted" = "1" ]; then
    dirs=$(run_ps "
(Get-ChildItem 'H:\brwork' -Directory -Force -ErrorAction SilentlyContinue | Where-Object { \$_.Name -match 'mnt\$' }) | ForEach-Object { 'DIR=' + \$_.FullName }
'__END__'
")
    for d in $(printf '%s' "$dirs" | grep -oE 'DIR=[^ ]*' | cut -d= -f2-); do
      run_ps "& dism.exe /English /Unmount-Image /MountDir:$d /Discard 2>&1 | Out-Null; '__END__'" >/dev/null
      say "ACTION=unmount_wim dir=$d"
    done
  fi
  if [ -n "${ghost:-}" ]; then
    for letter in $(printf '%s' "$ghost" | tr ',' ' '); do
      run_ps "& reg.exe delete 'HKLM\SYSTEM\MountedDevices' /v '\DosDevices\\${letter}:' /f 2>&1 | Out-Null; '__END__'" >/dev/null
      say "ACTION=reg_delete ghost_letter=$letter"
    done
  fi
  # 复核：清理类动作必须**写完再读一遍**，不能只看动作执行成功
  if [ -f "$0" ]; then
    after=$(probe_state) || after=''
    g2=$(printf '%s' "$after" | grep -oE 'GHOST=[^ ]*' | head -1 | cut -d= -f2-)
    m2=$(printf '%s' "$after" | grep -oE 'MOUNTED=[^ ]*' | head -1 | cut -d= -f2-)
    if [ -z "${g2:-}" ] && [ "$mounted" = "1" ]; then
      say "ITEM=clean_verify status=error reason=recheck_failed"
      say "POSTFLIGHT=error"
      exit 2
    fi
    say "ITEM=clean_verify status=$([ -z "${g2:-}" ] && [ "${m2:-1}" = "0" ] && echo pass || echo fail) ghost_after=${g2:-none} mounted_after=${m2:-?}"
  fi
fi

if [ "$left" -gt 0 ]; then
  say "POSTFLIGHT=leftovers"
  say "  >> 文件/目录类残留**不代删**（删文件风险不对等）：请按上面 detail 人工确认后删。"
  exit 1
fi
say "POSTFLIGHT=clean"
exit 0
