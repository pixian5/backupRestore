$log = 'C:\Users\x\Desktop\BackupRestore\_rollback-check.log'
Set-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] rc start" -Encoding UTF8
$cmd = "cd /d C:\Users\x\Desktop\BackupRestore && powershell -NoProfile -ExecutionPolicy Bypass -File _rollback-check.ps1"
Start-Process cmd.exe -ArgumentList '/c', "`"$cmd`"" -Wait -Verb RunAs
Add-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] rc finished" -Encoding UTF8
Write-Output 'DONE'
