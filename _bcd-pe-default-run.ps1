$log = 'C:\Users\x\Desktop\BackupRestore\_bcd-pe-default.log'
Set-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] bcd switch start" -Encoding UTF8
$cmd = "cd /d C:\Users\x\Desktop\BackupRestore && powershell -NoProfile -ExecutionPolicy Bypass -File _bcd-pe-default.ps1"
Start-Process cmd.exe -ArgumentList '/c', "`"$cmd`"" -Wait -Verb RunAs
Add-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] bcd switch finished" -Encoding UTF8
Write-Output 'DONE'
