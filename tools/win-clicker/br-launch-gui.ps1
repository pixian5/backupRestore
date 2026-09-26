# br-launch-gui.ps1 - relaunch the v1.7.4 GUI in Session 1 (High IL) with the image
# path preset through the app's own supported BACKUPRESTORE_OPEN_IMAGE variable.
# This avoids writing to the image EDIT from outside the process, which the running
# app does not honour (its live value only tracks real in-process input).
# ASCII only. Output: C:\Users\Public\pkg\launchgui.txt
param(
  [string]$Image = "C:\Users\Public\br-test\v174-20260926\test-fast.wim",
  [string]$Tab = "1"
)
$ErrorActionPreference = 'Continue'
$exe = 'C:\Users\Public\backupRestore-package\BackupRestore.exe'
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }
Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object {
  W ("killing old pid=" + $_.Id)
  Stop-Process -Id $_.Id -Force
}
Start-Sleep -Milliseconds 1200
# only the image path is preset; no auto-install / no BCD / no WinRE flags are touched
$env:BACKUPRESTORE_OPEN_IMAGE = $Image
Start-Process -FilePath $exe -ArgumentList @('--tab', $Tab)
Start-Sleep -Milliseconds 2500
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) {
  W ("new pid=" + $p.Id + " session=" + $p.SessionId + " path=" + $p.Path)
} else {
  W "NO_PROCESS"
}
W ("envImage=" + $Image + " tab=" + $Tab)
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\launchgui.txt", ($lines -join "`r`n"), $utf8)
Write-Output "LAUNCHGUI_DONE"