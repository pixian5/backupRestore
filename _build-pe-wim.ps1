$log = 'C:\Users\x\Desktop\BackupRestore\_build-pe-wim.log'
Set-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] PE WIM build start" -Encoding UTF8
$cmd = "cd /d C:\Users\x\Desktop\BackupRestore && powershell -NoProfile -ExecutionPolicy Bypass -File poc\build-backuprestore-pe.ps1 -Root C:\BackupRestorePE -SkipIso >> `"$log`" 2>&1"
Start-Process cmd.exe -ArgumentList '/c', "`"$cmd`"" -Wait -Verb RunAs
Add-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] PE WIM build finished" -Encoding UTF8
Write-Output 'DONE'
