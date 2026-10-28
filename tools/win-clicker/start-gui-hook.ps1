# start-gui-hook.ps1 - ASCII only (staged as GBK by br-s1.sh; keep it ASCII).
# Writes C:\br-test.json, restarts the GUI with --test-hook so it auto-fills the
# "Install PE" tab and (auto_install=true) triggers the install after the window shows.
$json = '{"tab":"pe","mode":"ram","pe_dir":"C:\\BackupRestorePE","pe_name":"BR-PE-E2E","pe_image":"H:\\BRImages\\pe-boot.wim","auto_install":true}'
[IO.File]::WriteAllText('C:\br-test.json', $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Output ("JSON_WRITTEN=" + $json.Length)
Get-Process BackupRestore -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2
Start-Process -FilePath 'H:\brwork\BackupRestore.exe' -ArgumentList '--test-hook' -WorkingDirectory 'H:\brwork'
Start-Sleep -Seconds 8
$p = Get-Process BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) {
    Write-Output ("GUI_PID=" + $p.Id + " SESSION=" + $p.SessionId)
} else {
    Write-Output "GUI_NOT_RUNNING"
}
