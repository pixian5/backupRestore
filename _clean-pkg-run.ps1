$log = 'C:\Users\x\Desktop\BackupRestore\_clean-pkg.log'
Set-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] clean start" -Encoding UTF8
$cmd = "cd /d C:\Users\x\Desktop\BackupRestore && powershell -NoProfile -ExecutionPolicy Bypass -File _clean-pkg.ps1"
Start-Process cmd.exe -ArgumentList '/c', "`"$cmd`"" -Wait -Verb RunAs
Add-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] clean finished" -Encoding UTF8
Write-Output 'DONE'
