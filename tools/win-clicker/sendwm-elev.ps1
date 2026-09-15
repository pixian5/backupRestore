# sendwm-elev.ps1 — re-run sendwm-diag.ps1 elevated (high integrity) so it can post to the elevated GUI
Start-Process -FilePath "powershell.exe" -Verb RunAs -Wait -ArgumentList @(
    '-NoProfile','-ExecutionPolicy','Bypass',
    '-File','C:\Users\Public\backupRestore-package\sendwm-diag.ps1'
)
$exists = Test-Path "C:\Users\Public\backupRestore-package\sendwm-diag.txt"
Add-Content -Path "C:\Users\Public\backupRestore-package\sendwm-elev.txt" -Value (($exists) ? "ran-elevated diag-written" : "ran-elevated diag-missing")