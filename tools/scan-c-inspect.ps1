# Inspect contents of BackupRestoreBuild package/target, BRTest tasks, C:\0, Users\x (ASCII)
$out = 'C:\Users\Public\backupRestore-package\c-inspect.log'
$lines = New-Object System.Collections.ArrayList

[void]$lines.Add('=== BackupRestoreBuild\package (top) ===')
Get-ChildItem 'C:\BackupRestoreBuild\package' -Force -ErrorAction SilentlyContinue | ForEach-Object { [void]$lines.Add($_.Name) } | Select-Object -First 30

[void]$lines.Add('')
[void]$lines.Add('=== BackupRestoreBuild\package\target (latest build? check) ===')
if (Test-Path 'C:\BackupRestoreBuild\package\BackupRestore.exe') { [void]$lines.Add('HAS BackupRestore.exe') }
if (Test-Path 'C:\BackupRestoreBuild\package\target') {
    Get-ChildItem 'C:\BackupRestoreBuild\package\target' -Force -ErrorAction SilentlyContinue | Select-Object -First 10 | ForEach-Object { [void]$lines.Add($_.Name) }
}

[void]$lines.Add('')
[void]$lines.Add('=== BRTest\tasks (top) ===')
Get-ChildItem 'C:\BRTest\tasks' -Force -ErrorAction SilentlyContinue | Select-Object -First 20 | ForEach-Object { [void]$lines.Add("$($_.Name)  $([math]::Round($_.Length/1MB,0))MB") }

[void]$lines.Add('')
[void]$lines.Add('=== C:\0 ===')
Get-ChildItem 'C:\0' -Force -Recurse -ErrorAction SilentlyContinue | Select-Object -First 15 | ForEach-Object { [void]$lines.Add($_.FullName) }

[void]$lines.Add('')
[void]$lines.Add('=== C:\Users\x (top, MB) ===')
Get-ChildItem 'C:\Users\x' -Directory -Force -ErrorAction SilentlyContinue | ForEach-Object {
    $s = (Get-ChildItem $_.FullName -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
    $mb = if ($s) { [math]::Round($s / 1MB, 0) } else { 0 }
    if ($mb -ge 30) { [void]$lines.Add(('{0,10}  {1}' -f $mb, $_.Name)) }
}

$lines | Set-Content -Path $out -Encoding ascii
Write-Output 'DONE'
