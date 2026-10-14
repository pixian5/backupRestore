# restart-gui-h.ps1 - relaunch BackupRestore.exe from H:\brwork (NOT the C: package copy).
# WHY: A-entry (restore system to C:) is blocked with
#   "program workspace volume matches restore target"
# when the running binary lives on the very volume being restored. Launching from
# H:\brwork puts the program workspace on a different volume than the C: target.
# ASCII-only body (GBK-safe for prlctl exec).
$ErrorActionPreference = 'Continue'
taskkill /F /IM BackupRestore.exe 2>$null | Out-Null
Start-Sleep -Seconds 2
$action = New-ScheduledTaskAction -Execute 'H:\brwork\BackupRestore.exe'
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 120) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRGUIH' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRGUIH'
Start-Sleep -Seconds 6
$info = Get-ScheduledTaskInfo -TaskName 'BRGUIH'
Add-Content -Path 'C:\Users\Public\restart-gui-h.txt' -Value ("H launched at " + (Get-Date -Format 'HH:mm:ss') + " lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRGUIH' -Confirm:$false
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue
if ($p) { Write-Output ("GUI_UP pid=" + $p.Id) } else { Write-Output "GUI_DOWN" }
