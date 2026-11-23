# List every task dir under H:\brwork\tasks with its stage/progress from status.json and its
# operation from task.json (ASCII-safe output; this file must stay ASCII-only for PowerShell 5.1).
#
# 2026-11-24 fixes (found during the baseline run):
#   - status.json records the stage under "stage" (kebab-case), not "status"; the old regex matched
#     nothing and every task was reported as NO_STATUS.
#   - the operation lives in task.json, not status.json.
#   - size needs -Force: hidden work-tree subdirectories (mount/stage/original/payload) were skipped,
#     which once reported 5.3 GB as 18.5 MB.
$ErrorActionPreference = 'SilentlyContinue'
function DirSizeMB([string]$path) {
  return [math]::Round((Get-ChildItem -LiteralPath $path -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum / 1MB, 1)
}
$root = 'H:\brwork\tasks'
foreach ($d in Get-ChildItem -LiteralPath $root -Directory) {
  $st = 'NO_STATUS'; $pg = '-'; $code = '-'; $op = '-'
  $p = Join-Path $d.FullName 'status.json'
  if (Test-Path -LiteralPath $p) {
    $raw = [IO.File]::ReadAllText($p)
    if ($raw -match '"stage"\s*:\s*"([^"]+)"') { $st = $Matches[1] }
    elseif ($raw -match '"status"\s*:\s*"([^"]+)"') { $st = $Matches[1] }
    if ($raw -match '"progress"\s*:\s*([0-9]+)') { $pg = $Matches[1] }
    if ($raw -match '"errorCodeText"\s*:\s*"([^"]+)"') { $code = $Matches[1] }
  }
  $t = Join-Path $d.FullName 'task.json'
  if (Test-Path -LiteralPath $t) {
    $traw = [IO.File]::ReadAllText($t)
    if ($traw -match '"operation"\s*:\s*"([^"]+)"') { $op = $Matches[1] }
    elseif ($traw -match '"operation"\s*:\s*(\d+)') { $op = 'op' + $Matches[1] }
  }
  Write-Output ('{0}  stage={1,-10} progress={2,-4} op={3,-18} code={4,-12} sizeMB={5}' -f $d.Name, $st, $pg, $op, $code, (DirSizeMB $d.FullName))
}
Write-Output '--- abandoned ---'
$ab = 'H:\brwork\tasks-abandoned'
if (Test-Path -LiteralPath $ab) {
  foreach ($d in Get-ChildItem -LiteralPath $ab -Directory) {
    Write-Output ('{0}  sizeMB={1}' -f $d.Name, (DirSizeMB $d.FullName))
  }
}
