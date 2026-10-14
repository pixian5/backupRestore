# List every task dir under H:\brwork\tasks with its status/progress from status.json (ASCII-safe output).
$ErrorActionPreference = 'SilentlyContinue'
$root = 'H:\brwork\tasks'
foreach ($d in Get-ChildItem -LiteralPath $root -Directory) {
  $st = 'NO_STATUS'
  $op = '-'
  $p = Join-Path $d.FullName 'status.json'
  if (Test-Path -LiteralPath $p) {
    $raw = [IO.File]::ReadAllText($p)
    if ($raw -match '"status"\s*:\s*"([^"]+)"') { $st = $Matches[1] }
    if ($raw -match '"operation"\s*:\s*"([^"]+)"') { $op = $Matches[1] }
    elseif ($raw -match '"operation"\s*:\s*(\d+)') { $op = 'op' + $Matches[1] }
  }
  $sz = [math]::Round((Get-ChildItem -LiteralPath $d.FullName -Recurse -File | Measure-Object -Property Length -Sum).Sum / 1MB, 1)
  Write-Output ('{0}  status={1,-10} op={2,-18} sizeMB={3}' -f $d.Name, $st, $op, $sz)
}
Write-Output '--- abandoned ---'
$ab = 'H:\brwork\tasks-abandoned'
if (Test-Path -LiteralPath $ab) {
  foreach ($d in Get-ChildItem -LiteralPath $ab -Directory) {
    $sz = [math]::Round((Get-ChildItem -LiteralPath $d.FullName -Recurse -File | Measure-Object -Property Length -Sum).Sum / 1MB, 1)
    Write-Output ('{0}  sizeMB={1}' -f $d.Name, $sz)
  }
}
