# Deep scan of suspicious/known dirs: BackupRestoreBuild, BRTest, BuildTools, PF, PF(x86), Users, ProgramData, BackupRestorePE
$out = 'C:\Users\Public\backupRestore-package\c-space-deep.log'
$lines = New-Object System.Collections.ArrayList

$roots = @(
    'C:\BackupRestoreBuild',
    'C:\BRTest',
    'C:\BuildTools',
    'C:\BackupRestorePE',
    'C:\Program Files',
    'C:\Program Files (x86)',
    'C:\Users',
    'C:\ProgramData',
    'C:\brsrc'
)
foreach ($r in $roots) {
    if (-not (Test-Path $r)) { continue }
    [void]$lines.Add('')
    [void]$lines.Add("=== $r ===")
    Get-ChildItem $r -Directory -Force -ErrorAction SilentlyContinue | ForEach-Object {
        $s = (Get-ChildItem $_.FullName -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
        $mb = if ($s) { [math]::Round($s / 1MB, 0) } else { 0 }
        if ($mb -ge 50) { [void]$lines.Add(('{0,10}  {1}' -f $mb, $_.Name)) }
    }
}

$lines | Set-Content -Path $out -Encoding ascii
Write-Output 'DONE'
