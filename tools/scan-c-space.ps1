# Scan C: top-level dir sizes + root big files (ASCII out)
$out = 'C:\Users\Public\backupRestore-package\c-space.log'
$lines = New-Object System.Collections.ArrayList

[void]$lines.Add('=== TOP-LEVEL DIRS (MB) ===')
Get-ChildItem C:\ -Directory -Force -ErrorAction SilentlyContinue | ForEach-Object {
    $s = (Get-ChildItem $_.FullName -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
    $mb = if ($s) { [math]::Round($s / 1MB, 0) } else { 0 }
    [void]$lines.Add(('{0,10}  {1}' -f $mb, $_.Name))
}

[void]$lines.Add('')
[void]$lines.Add('=== ROOT FILES (MB) ===')
Get-ChildItem C:\ -File -Force -ErrorAction SilentlyContinue | ForEach-Object {
    $mb = [math]::Round($_.Length / 1MB, 0)
    if ($mb -ge 1) { [void]$lines.Add(('{0,10}  {1}' -f $mb, $_.Name)) }
}

$lines | Set-Content -Path $out -Encoding ascii
Write-Output 'DONE'
