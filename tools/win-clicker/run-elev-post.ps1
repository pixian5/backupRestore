# run-elev-post.ps1 — register a highest-privilege interactive task (user x, session 1) that runs sendwm-diag.ps1
$ErrorActionPreference = 'Stop'
$action = New-ScheduledTaskAction -Execute 'powershell.exe' `
    -Argument '-NoProfile -ExecutionPolicy Bypass -File "C:\Users\Public\backupRestore-package\sendwm-diag.ps1"'
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 5) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRPOST' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRPOST'
Start-Sleep -Seconds 3
$info = Get-ScheduledTaskInfo -TaskName 'BRPOST'
Add-Content -Path 'C:\Users\Public\backupRestore-package\elev-post.txt' -Value ("lastRun=" + $info.LastRunTime + " lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRPOST' -Confirm:$false