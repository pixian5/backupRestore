# restart-gui-h-testhook.ps1 - relaunch H:\brwork\BackupRestore.exe --test-hook elevated.
# Same contract as restart-gui-h.ps1 (program workspace on H:, NOT the C: package copy),
# plus the --test-hook flag so C:\br-test.json is consumed at startup.
# ASCII-only body (GBK-safe for prlctl exec).
$ErrorActionPreference = 'Continue'
taskkill /F /IM BackupRestore.exe 2>$null | Out-Null
Start-Sleep -Seconds 2
$action = New-ScheduledTaskAction -Execute 'H:\brwork\BackupRestore.exe' -Argument '--test-hook'
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 120) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRGUIHT' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRGUIHT'
Start-Sleep -Seconds 6
$info = Get-ScheduledTaskInfo -TaskName 'BRGUIHT'
Add-Content -Path 'C:\Users\Public\restart-gui-h.txt' -Value ("H+testhook launched at " + (Get-Date -Format 'HH:mm:ss') + " lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRGUIHT' -Confirm:$false
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue
if ($p) { Write-Output ("GUI_UP pid=" + $p.Id) } else { Write-Output "GUI_DOWN" }
