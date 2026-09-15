# run-prep.ps1 — register+run highest-privilege interactive task to execute prepare backup, capture output to a file
$ErrorActionPreference = 'Stop'
$cmd = 'cmd /c ""C:\Users\Public\backupRestore-package\BackupRestore.exe" prepare --operation backup --source-drive C --target-drive C --boot-menu-name "Windows Backup" --image-path E:\br-cdrive-v1.wim --wim-index 1 --compress fast --image-name cdrive-verify > E:\prep-elev.log 2>&1"'
$action = New-ScheduledTaskAction -Execute 'cmd.exe' -Argument ('/c "' + $cmd + '"')
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 30) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRPREP' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRPREP'
Start-Sleep -Seconds 4
$info = Get-ScheduledTaskInfo -TaskName 'BRPREP'
Add-Content -Path 'C:\Users\Public\backupRestore-package\prep-run.txt' -Value ("started; status=" + $info.LastTaskResult)