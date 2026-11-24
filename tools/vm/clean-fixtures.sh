#!/bin/bash
# tools/vm/clean-fixtures.sh —— 清掉 tools/vm 下各个用例留下的夹具与临时文件。只删这些用例自己造的东西，
# 每一项先列出来、删完再回读确认。不动任何用户文件、不动任务目录（那由产品自己的保留策略在下次准备时清理）。
#
# 清理清单：
#   F:\br-vm-tools-pad.bin                         pause-backup / pause-restore / orphan-race 的 20 GB 占位文件
#   H:\BRImages\br-vm-tools-test.wim(+副档)        它们产出 / 消费的测试镜像
#   F:\BR-SENTINEL-*.txt                            哨兵法还原放的哨兵文件
#   H:\brwork\bcd-pre-*-v*.bak                     这些用例做的启动配置文件级备份（名字里带「v 版本号」的才是）
#       ★ 第一次写成「版本号后面还要有时间戳」，漏掉了加时间戳之前留下的四份——待删清单里没有它们，
#         回读那一项却是「0 个、通过」（空集通过）。两种名字现在都认：以版本号结尾的、版本号后带时间戳的。
#       ★ 回读不能只用删除时的那一个匹配写法（同一台仪器证明的只是仪器自洽）：STEP=4 另有一条不同代码路径的交叉核对。
#   H:\brwork\test-*.log、build-vm-tools.log        虚拟机里的测试与构建日志（宿主证据目录里有原样副本）
#   共享盘 X: 的映射                               这些用例映射的（不跨重启；留着会让下次登录弹「无法访问 X:」）
# ★ 删启动配置备份之前先做一次健康核对：当前读数必须等于最近一次用例结束时的读数；一次性启动项必须为 0。
# 前提：没有任务在跑（都回到终态、虚拟机在 Windows 桌面）。
# 用法：tools/vm/clean-fixtures.sh          退出码：0 = 全部清掉并回读确认；非 0 = 否。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/lib/common.sh"
run_name "clean-fixtures"
fail=0
ok() { if [ "$2" = "$3" ]; then say "  OK   $1 = $3"; else say "  FAIL $1 expected=[$2] got=[$3]"; fail=1; fi; }

say "STEP=1 boot configuration health check before deleting its backups"
bcd_readings | tee "$EV/$NAME-bcd.txt"
last=$(ls -t "$EV"/*-bcd-after.txt 2>/dev/null | head -1)
if [ -n "$last" ]; then
  say "comparing with the readings when the last case ended: $(basename "$last")"
  SAME=$(bcd_same "$last" "$EV/$NAME-bcd.txt")
else
  SAME="BCD_READINGS_EQUAL=<no previous readings to compare with>"
fi
SEQ=$(sed -n 's/^BOOTSEQUENCE_LINES=\([0-9]*\)$/\1/p' "$EV/$NAME-bcd.txt")
printf '%s\n' "$SAME" | tee -a "$EV/$NAME-bcd.txt"
[ "$SEQ" = "0" ] || { say "ABORT: a one-shot boot sequence is present; clean up that task first"; exit 3; }
case "$SAME" in *'=True'|*'no previous'*) ;; *) say "ABORT: the boot configuration differs from the last case's final readings; not deleting the backups"; exit 3 ;; esac
n=$(vrun "tasklist /fi \"IMAGENAME eq BackupRestore.exe\" /fo csv /nh" 3 | tr -cd '\11\12\40-\176' | grep -a -c 'BackupRestore.exe')
inflight=$(vrun "tasklist /fi \"IMAGENAME eq dism.exe\" /fo csv /nh" 3 | tr -cd '\11\12\40-\176' | grep -a -c 'dism.exe')
say "BACKUPRESTORE_PROCESSES=$n DISM_PROCESSES=$inflight"

say "STEP=2 what is going to be deleted (listed first)"
vrun "dir /b $PAD $IMAGE $IMAGE.index-1.metadata.json F:\\BR-SENTINEL-*.txt H:\\brwork\\bcd-pre-*-v*.bak H:\\brwork\\test-*.log H:\\brwork\\build-vm-tools.log" 2 | tr -cd '\11\12\40-\176\r' | tr -d '\r' | grep -a -v 'VM_DONE' | grep -a -E '\.(bin|wim|json|txt|bak|log)$' | sed 's/^/  BEFORE /'

say "STEP=3 delete"
vrun "del /f /q $PAD" 2 | grep -a 'VM_DONE' | sed 's/^/  PAD /'
vrun "del /f /q $IMAGE $IMAGE.index-1.metadata.json" 2 | grep -a 'VM_DONE' | sed 's/^/  IMAGE /'
vrun "del /f /q F:\\BR-SENTINEL-*.txt" 2 | grep -a 'VM_DONE' | sed 's/^/  SENTINELS /'
vrun "del /f /q H:\\brwork\\bcd-pre-*-v*.bak" 2 | grep -a 'VM_DONE' | sed 's/^/  BCD_BACKUPS /'
vrun "del /f /q H:\\brwork\\test-*.log H:\\brwork\\build-vm-tools.log" 2 | grep -a 'VM_DONE' | sed 's/^/  LOGS /'

say "STEP=4 read back"
left() { vrun "dir /b $1" 2 | tr -cd '\11\12\40-\176\r' | tr -d '\r' | grep -a -v 'VM_DONE' | grep -a -E '\.(bin|wim|json|txt|bak|log)$' | grep -a -c -E "$2"; }
ok "pad file" 0 "$(left "$PAD" 'pad\.bin')"
ok "test image + sidecar" 0 "$(left "$IMAGE*" 'br-vm-tools-test')"
ok "sentinel files" 0 "$(left 'F:\BR-SENTINEL-*.txt' 'SENTINEL')"
ok "boot configuration backups of these cases" 0 "$(left 'H:\brwork\bcd-pre-*-v*.bak' '\.bak')"
ok "test / build logs" 0 "$(left 'H:\brwork\test-*.log H:\brwork\build-vm-tools.log' '\.log')"
# 交叉核对（不同代码路径）：上面几项的回读和删除用的是同一个通配写法，匹配漏了就会空集通过（2026-11-25 实测：
# 漏了四份旧命名的启动配置备份）。这里改用 PowerShell 的文件枚举再数一遍：启动配置备份 / 哨兵 / 测试镜像 / 占位文件。
cross=$(vrun "powershell -NoProfile -Command \"'CROSS=' + @(Get-ChildItem H:\\brwork -File | Where-Object { \$_.Name -like 'bcd-pre-*v*.bak' }).Count + ',' + @(Get-ChildItem F:\\ -Filter 'BR-SENTINEL-*.txt' -File).Count + ',' + @(Get-ChildItem H:\\BRImages -Filter 'br-vm-tools-test*' -File).Count + ',' + @(Get-ChildItem F:\\ -Filter 'br-vm-tools-pad.bin' -File).Count; '__END__'\"" 3 | tr -cd '\11\12\40-\176\r' | tr -d '\r' | grep -a -o 'CROSS=[0-9,]*' | tail -1)
ok "cross-check by PowerShell enumeration (bcd backups / sentinels / test image files / pad)" "CROSS=0,0,0,0" "${cross:-CROSS=<unreadable>}"
kept=$(vrun "dir /b H:\\BRImages\\*.wim" 2 | tr -cd '\11\12\40-\176\r' | tr -d '\r' | grep -a -c '\.wim')
say "  INFO baseline images kept in H:\\BRImages: $kept"
vrun "powershell -NoProfile -Command \"Get-PSDrive $SOURCE_DRIVE,H | ForEach-Object { 'DRIVE=' + \$_.Name + ' FreeGiB=' + [math]::Round(\$_.Free/1GB,1) }\"" 3 | tr -cd '\11\12\40-\176' | grep -a 'DRIVE=' | sed 's/^/  FREE /'

say "STEP=5 remove the share mapping made by these cases"
"$PRL" exec "$VM" --current-user cmd /d /c "net use X: /delete /y" >/dev/null 2>&1
map=$("$PRL" exec "$VM" --current-user cmd /d /c "net use" 2>/dev/null | tr -cd '\11\12\40-\176' | grep -a -c 'X:')
ok "X: mapping" 0 "$map"

if [ $fail -eq 0 ]; then say "CLEAN_FIXTURES=PASS"; else say "CLEAN_FIXTURES=FAIL"; fi
exit $fail
