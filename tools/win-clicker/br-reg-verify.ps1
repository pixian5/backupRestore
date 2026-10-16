# br-reg-verify.ps1 -- run br-settings-verify.ps1 ELEVATED on the interactive desktop
# via a one-shot scheduled task (same proven path as restart-gui-h.ps1). ASCII only.
$ErrorActionPreference = 'Continue'
$action = New-ScheduledTaskAction -Execute 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe' -Argument '-NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\br-settings-verify.ps1'
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 5) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRVerifySettings' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRVerifySettings'
Start-Sleep -Seconds 14
$info = Get-ScheduledTaskInfo -TaskName 'BRVerifySettings'
Write-Output ("BRVerifySettings lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRVerifySettings' -Confirm:$false
Write-Output "unregistered"
