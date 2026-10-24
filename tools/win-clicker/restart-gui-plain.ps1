# restart-gui-plain.ps1 - kill old GUI, relaunch BackupRestore.exe WITHOUT --test-hook
# Purpose: real-UI acceptance runs (destructive confirmations must be clicked for real).
$ErrorActionPreference = 'Continue'
# 1. Kill any existing GUI
taskkill /F /IM BackupRestore.exe 2>$null | Out-Null
Start-Sleep -Seconds 2
# 2. Relaunch elevated via scheduled task (user x, interactive, highest), NO --test-hook
$action = New-ScheduledTaskAction -Execute 'H:\brwork\BackupRestore.exe'
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 60) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRGUIPlain' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRGUIPlain'
Start-Sleep -Seconds 6
$info = Get-ScheduledTaskInfo -TaskName 'BRGUIPlain'
Add-Content -Path 'H:\brwork\restart-gui-plain.txt' -Value ("plain started at " + (Get-Date -Format 'HH:mm:ss') + " lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRGUIPlain' -Confirm:$false
# 3. Report resulting process
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue
if ($p) { Write-Output ("GUI_UP pid=" + $p.Id) } else { Write-Output "GUI_DOWN" }
