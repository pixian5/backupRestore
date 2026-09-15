# run-prep2.ps1 — run prepare backup via highest-privilege interactive task, with --test-efi-drive S override
$ErrorActionPreference = 'Stop'
$cmd = 'cmd /c ""C:\Users\Public\backupRestore-package\BackupRestore.exe" prepare --operation backup --source-drive C --target-drive C --boot-menu-name "Windows Backup" --image-path E:\br-cdrive-v1.wim --wim-index 1 --compress fast --image-name cdrive-verify --test-efi-drive S > E:\prep-elev2.log 2>&1"'
$action = New-ScheduledTaskAction -Execute 'cmd.exe' -Argument ('/c "' + $cmd + '"')
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 30) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRPREP2' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRPREP2'
Start-Sleep -Seconds 4
$info = Get-ScheduledTaskInfo -TaskName 'BRPREP2'
Add-Content -Path 'C:\Users\Public\backupRestore-package\prep-run2.txt' -Value ("started; status=" + $info.LastTaskResult)