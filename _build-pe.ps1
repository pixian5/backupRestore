$log = 'C:\Users\x\Desktop\BackupRestore\_build-pe.log'
Set-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] build start" -Encoding UTF8
$cmd = "cd /d C:\Users\x\Desktop\BackupRestore && powershell -NoProfile -ExecutionPolicy Bypass -File windows\build-windows.ps1 -Architecture arm64 -CargoTargetDir C:\BackupRestoreBuild\target -OutputRoot C:\BackupRestoreBuild\package >> `"$log`" 2>&1"
Start-Process cmd.exe -ArgumentList '/c', "`"$cmd`"" -Wait -Verb RunAs
Add-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] build finished" -Encoding UTF8
Write-Output 'DONE'
