# br-reg-apply.ps1 -- run br-settings-apply2.ps1 ELEVATED inside the interactive
# session, using the same proven path restart-gui-h.ps1 uses: a one-shot scheduled
# task with -RunLevel Highest -LogonType Interactive.
#
# WHY NOT Start-Process -Verb runas: that worked for opening the dialog but the
# child never produced a log, and there is no way to observe it. A scheduled task
# gives LastTaskResult, which is observable. ASCII only.
$ErrorActionPreference = 'Continue'
$action = New-ScheduledTaskAction -Execute 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe' -Argument '-NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\br-settings-apply2.ps1'
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 5) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRApplySettings' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRApplySettings'
Start-Sleep -Seconds 9
$info = Get-ScheduledTaskInfo -TaskName 'BRApplySettings'
Write-Output ("BRApplySettings lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRApplySettings' -Confirm:$false
Write-Output "unregistered"
