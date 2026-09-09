$ErrorActionPreference = 'Continue'
$p = Get-ChildItem P:\ -Recurse -File -Force -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch 'System Volume Information' }
$q = Get-ChildItem Q:\ -Recurse -File -Force -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch 'System Volume Information' }
Write-Output ("P files: " + $p.Count + "  Q files: " + $q.Count)
$relP = @{}; $relQ = @{}
foreach ($f in $p) { $relP[$f.FullName.Substring(3).ToLowerInvariant()] = $f.Length }
foreach ($f in $q) { $relQ[$f.FullName.Substring(3).ToLowerInvariant()] = $f.Length }
$common = 0; $sizeMatch = 0; $missingInQ = @(); $sizeMismatch = @()
foreach ($k in $relP.Keys) {
  if ($relQ.ContainsKey($k)) {
    $common++
    if ($relP[$k] -eq $relQ[$k]) { $sizeMatch++ } else { $sizeMismatch += $k }
  } else { $missingInQ += $k }
}
$extraInQ = @($relQ.Keys | Where-Object { -not $relP.ContainsKey($_) })
Write-Output ("common: " + $common + "  size-match: " + $sizeMatch + "  size-mismatch: " + $sizeMismatch.Count)
Write-Output ("missing-in-Q: " + $missingInQ.Count + "  extra-in-Q: " + $extraInQ.Count)
if ($sizeMismatch.Count -gt 0) { Write-Output "--- size mismatches ---"; $sizeMismatch | Select-Object -First 8 }
if ($missingInQ.Count -gt 0 -and $missingInQ.Count -le 12) { Write-Output "--- missing in Q ---"; $missingInQ }
Write-Output ("DIRS P: " + (Get-ChildItem P:\ -Directory -Recurse -Force -ErrorAction SilentlyContinue).Count + "  DIRS Q: " + (Get-ChildItem Q:\ -Directory -Recurse -Force -ErrorAction SilentlyContinue).Count)
