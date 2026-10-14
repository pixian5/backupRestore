# Remove finished BackupRestore task residue. ASCII-only body (GBK-safe).
# Targets: RE payload staging dirs, historical task dirs, abandoned task dirs.
# User images under H:\BRImages are NEVER touched.
$ErrorActionPreference = 'SilentlyContinue'
$targets = @(
  'H:\BackupRestoreRE',
  'H:\brwork\tasks',
  'H:\brwork\tasks-abandoned'
)
$total = 0
foreach ($root in $targets) {
  if (-not (Test-Path -LiteralPath $root)) { Write-Output "SKIP (absent): $root"; continue }
  $dirs = Get-ChildItem -LiteralPath $root -Directory
  Write-Output "=== $root : $($dirs.Count) dirs ==="
  foreach ($d in $dirs) {
    $sz = (Get-ChildItem -LiteralPath $d.FullName -Recurse -File | Measure-Object -Property Length -Sum).Sum
    $mb = [math]::Round($sz / 1MB, 1)
    Remove-Item -LiteralPath $d.FullName -Recurse -Force
    if (Test-Path -LiteralPath $d.FullName) { Write-Output "  FAIL  $($d.Name) (${mb}MB)" }
    else { Write-Output "  DEL   $($d.Name) (${mb}MB)"; $total += $sz }
  }
}
Write-Output ('TOTAL_RECLAIMED_MB=' + [math]::Round($total / 1MB, 1))
Write-Output '--- verify ---'
foreach ($root in $targets) {
  if (Test-Path -LiteralPath $root) {
    $n = (Get-ChildItem -LiteralPath $root -Directory).Count
    Write-Output ("$root remaining_dirs=$n")
  } else { Write-Output "$root (absent)" }
}
