$log = 'C:\Users\x\Desktop\BackupRestore\_deploy-pe.log'
Set-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] deploy start" -Encoding UTF8
$cmd = "cd /d C:\Users\x\Desktop\BackupRestore && powershell -NoProfile -ExecutionPolicy Bypass -File _deploy-pe.ps1"
Start-Process cmd.exe -ArgumentList '/c', "`"$cmd`"" -Wait -Verb RunAs
Add-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] deploy finished" -Encoding UTF8
Write-Output 'DONE'
