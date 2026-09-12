# List size of each dir under C:\Users\Public (ASCII output)
$out = "C:\Users\Public\backupRestore-package\pubdirs.log"
$lines = New-Object System.Collections.ArrayList
Get-ChildItem C:\Users\Public -Directory -ErrorAction SilentlyContinue | ForEach-Object {
    $s = (Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
    $mb = if ($s) { [math]::Round($s / 1MB, 1) } else { 0 }
    [void]$lines.Add("$($_.Name)`t$mb MB")
}
$lines | Set-Content -Path $out -Encoding ascii
Write-Output "WROTE"
