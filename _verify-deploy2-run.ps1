$log = 'C:\Users\x\Desktop\BackupRestore\_verify-deploy2.log'
Set-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] verify2 start" -Encoding UTF8
$cmd = "cd /d C:\Users\x\Desktop\BackupRestore && powershell -NoProfile -ExecutionPolicy Bypass -File _verify-deploy2.ps1"
Start-Process cmd.exe -ArgumentList '/c', "`"$cmd`"" -Wait -Verb RunAs
Add-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] verify2 finished" -Encoding UTF8
Write-Output 'DONE'
