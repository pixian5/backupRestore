$log = 'C:\Users\x\Desktop\BackupRestore\_wim-verify.log'
Set-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] wim verify start" -Encoding UTF8
$cmd = "cd /d C:\Users\x\Desktop\BackupRestore && powershell -NoProfile -ExecutionPolicy Bypass -File _wim-verify.ps1"
Start-Process cmd.exe -ArgumentList '/c', "`"$cmd`"" -Wait -Verb RunAs
Add-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] wim verify finished" -Encoding UTF8
Write-Output 'DONE'
