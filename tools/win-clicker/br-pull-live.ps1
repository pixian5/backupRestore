# br-pull-live.ps1 - pull the LIVE launcher log (H:\brwork\logs)
# and DISM dir metadata (creation time, ACL)
$ErrorActionPreference = 'Continue'
$dst = "\\Mac\backupRestore\tools\win-clicker\_s1"

$live = "H:\brwork\logs\launcher-errors.log"
if (Test-Path -LiteralPath $live) {
    Copy-Item -LiteralPath $live -Destination (Join-Path $dst "launcher-errors-LIVE.log") -Force
    Write-Output ("pulled " + $live + " size=" + (Get-Item $live).Length)
} else { Write-Output ("MISSING " + $live) }

# also the package logs dir listing
Get-ChildItem "H:\brwork\logs" -ErrorAction SilentlyContinue |
    ForEach-Object { Write-Output ("pkglog " + $_.Name + " " + $_.Length + " " + $_.LastWriteTime) }

# DISM dir metadata
$d = "C:\Windows\Logs\DISM"
if (Test-Path -LiteralPath $d) {
    $i = Get-Item -LiteralPath $d
    Write-Output ("DISMdir created=" + $i.CreationTime.ToString("yyyy-MM-dd HH:mm:ss") + " modified=" + $i.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss"))
}
$l = "C:\Windows\Logs"
if (Test-Path -LiteralPath $l) {
    $i2 = Get-Item -LiteralPath $l
    Write-Output ("Logsdir created=" + $i2.CreationTime.ToString("yyyy-MM-dd HH:mm:ss"))
}
# CBS log existence/time (servicing activity after restore)
$cbs = "C:\Windows\Logs\CBS\CBS.log"
if (Test-Path -LiteralPath $cbs) {
    $i3 = Get-Item -LiteralPath $cbs
    Write-Output ("CBS.log created=" + $i3.CreationTime.ToString("yyyy-MM-dd HH:mm:ss") + " modified=" + $i3.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss") + " size=" + $i3.Length)
} else { Write-Output "CBS.log MISSING" }
# test hook config still present?
$tj = "C:\br-test.json"
if (Test-Path -LiteralPath $tj) {
    Write-Output ("br-test.json: " + (Get-Content -LiteralPath $tj -Raw))
} else { Write-Output "br-test.json MISSING" }
Write-Output "PULLLIVE_DONE"
