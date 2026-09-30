#!/bin/bash
set -euo pipefail

VM="Windows 11"

echo ">> 1. 写入 C:\\br-test.json"
prlctl exec "$VM" cmd /d /c 'echo {"tab":"backup","source_volume":"C","image":"F:\\6.wim","system_drive_choice":2,"auto_install":true} > C:\br-test.json'
prlctl exec "$VM" cmd /d /c "type C:\\br-test.json"

echo ">> 2. 通过任务计划程序以交互高权限启动 BackupRestore.exe --test-hook"
prlctl exec "$VM" powershell -NoProfile -ExecutionPolicy Bypass -Command '
$action = New-ScheduledTaskAction -Execute "C:\Users\Public\backupRestore-package\BackupRestore.exe" -Argument "--test-hook"
$principal = New-ScheduledTaskPrincipal -UserId "x" -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 60) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName "BRGUIT" -Action $action -Principal $principal -Settings $settings -Force | Out-Null
taskkill /F /IM BackupRestore.exe 2>$null | Out-Null
Start-Sleep -Seconds 1
Start-ScheduledTask -TaskName "BRGUIT"
Start-Sleep -Seconds 3
Unregister-ScheduledTask -TaskName "BRGUIT" -Confirm:$false
Write-Output ">> 任务启动成功"
'

echo ">> 启动完成，GUI 正在执行备份准备并即将重启进入 WinRE..."
