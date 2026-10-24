# start-gui.ps1 - ASCII only. Launch the deployed GUI (H:\brwork) elevated on the
# session-1 desktop, then report the process.
# Run via: ./tools/win-clicker/br-s1.sh start-gui.ps1
Start-Process -FilePath 'H:\brwork\BackupRestore.exe' -WorkingDirectory 'H:\brwork'
Start-Sleep -Seconds 4
$p = Get-Process BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) {
    Write-Output ("GUI_PID=" + $p.Id + " SESSION=" + $p.SessionId + " TITLE=" + $p.MainWindowTitle)
} else {
    Write-Output "GUI_NOT_RUNNING"
}
