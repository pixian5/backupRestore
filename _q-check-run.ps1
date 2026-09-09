$log = 'C:\Users\x\Desktop\BackupRestore\_q-check.log'
Set-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] qcheck start" -Encoding UTF8
$cmd = "cd /d C:\Users\x\Desktop\BackupRestore && powershell -NoProfile -ExecutionPolicy Bypass -File _q-check.ps1"
Start-Process cmd.exe -ArgumentList '/c', "`"$cmd`"" -Wait -Verb RunAs
Add-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] qcheck finished" -Encoding UTF8
Write-Output 'DONE'
