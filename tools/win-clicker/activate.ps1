# activate.ps1 - bring a window to foreground by title fragment (run in Session 1)
param([string]$Title = "BackupRestore")
$ws = New-Object -ComObject WScript.Shell
$ok = $ws.AppActivate($Title)
"activate='$Title' => $ok"
Start-Sleep -Milliseconds 500
