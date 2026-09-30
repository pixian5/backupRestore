#!/bin/bash
set -euo pipefail

VM="Windows 11"

echo ">> 1. 清理前次测试日志（保留 F:\6.wim 镜像！）"
prlctl exec "$VM" cmd /d /c "del /f /q C:\\br-test.json F:\\Recovery.log 2>nul & taskkill /f /im BackupRestore.exe 2>nul & exit /b 0"

echo ">> 2. 在 Session 1 以提权启动 H:\\brwork\\BackupRestore.exe"
prlctl exec "$VM" powershell -NoProfile -ExecutionPolicy Bypass -Command '
$action = New-ScheduledTaskAction -Execute "H:\brwork\BackupRestore.exe"
$principal = New-ScheduledTaskPrincipal -UserId "x" -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 60) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName "BRLAUNCH" -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName "BRLAUNCH"
Start-Sleep -Seconds 3
Unregister-ScheduledTask -TaskName "BRLAUNCH" -Confirm:$false
'

echo ">> 3. 等待 GUI 窗口启动"
for i in {1..20}; do
  sleep 1
  if (cd tools/win-clicker && ./br-agent-tcp.sh windows) | grep -q "BackupRestore - Rust GUI"; then
    echo ">> 窗口已出现"
    break
  fi
done

echo ">> 4. 执行真实窗口消息驱动还原流程 (drive-restore-flow.ps1)"
cd tools/win-clicker
./br-agent-tcp.sh exec ../drive-restore-flow.ps1

echo ">> 5. 驱动完成，开始监控并截图观察 WinRE 还原进度！"
cd ../..
./tools/watch-re-restore.sh
