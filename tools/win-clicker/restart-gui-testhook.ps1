# restart-gui-testhook.ps1 - kill old GUI, relaunch BackupRestore.exe --test-hook elevated
$ErrorActionPreference = 'Continue'
# 1. Kill any existing GUI
taskkill /F /IM BackupRestore.exe 2>$null | Out-Null
Start-Sleep -Seconds 2
# 2. Relaunch elevated via scheduled task (user x, interactive, highest)
$action = New-ScheduledTaskAction -Execute 'C:\Users\Public\backupRestore-package\BackupRestore.exe' `
    -Argument '--test-hook'
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 60) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRGUIT' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRGUIT'
Start-Sleep -Seconds 5
$info = Get-ScheduledTaskInfo -TaskName 'BRGUIT'
Add-Content -Path 'C:\Users\Public\backupRestore-package\restart-gui.txt' -Value ("started at " + (Get-Date -Format 'HH:mm:ss') + " lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRGUIT' -Confirm:$false
Write-Output "gui relaunch initiated"
