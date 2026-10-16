# br-v2-forensics.ps1 - pull V2 failure evidence after returning to Windows.
$ErrorActionPreference = 'Continue'
$dst = "\\Mac\backupRestore\tools\win-clicker\_s1"

function Pull($src, $name) {
    if (Test-Path -LiteralPath $src) {
        Copy-Item -LiteralPath $src -Destination (Join-Path $dst $name) -Force -ErrorAction SilentlyContinue
        Write-Output ("pulled " + $src + " -> " + $name + " (" + (Get-Item -LiteralPath $src).Length + " B)")
    } else { Write-Output ("MISSING " + $src) }
}

# 1) RE persistent early log (written to the real C: partition from WinRE)
Pull "C:\BackupRestore-Recovery-early.log" "v2-early.log"
Pull "C:\WinRE-PoC\BackupRestore-Recovery-early.log" "v2-early-poc.log"

# 2) main recovery log on the image volume
$recovery = Get-ChildItem "H:\" -Recurse -Filter "Recovery.log" -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 3
foreach ($r in $recovery) {
    $body = Get-Content -LiteralPath $r.FullName -Tail 250 -ErrorAction SilentlyContinue
    [System.IO.File]::WriteAllLines((Join-Path $dst ("v2-recovery-" + ($r.FullName -replace '[\\/:]','_'))), $body, [System.Text.Encoding]::UTF8)
    Write-Output ("recovery tail: " + $r.FullName + " mtime=" + $r.LastWriteTime)
}

# 3) the V2 task dir (newest) contents
$taskRoot = "H:\brwork\tasks"
$newest = Get-ChildItem -LiteralPath $taskRoot -Directory | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($newest) {
    Write-Output ("newest task dir: " + $newest.Name)
    Get-ChildItem -LiteralPath $newest.FullName -Recurse -File | ForEach-Object {
        $safe = ($_.FullName -replace '[\\/:]','_')
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $dst ("vtask-" + $safe)) -Force -ErrorAction SilentlyContinue
        Write-Output ("  taskfile " + $_.Name + " " + $_.Length + " " + $_.LastWriteTime)
    }
}

# 4) does C: still hold the sentinel? (overlay apply must have kept it)
Get-ChildItem "C:\" -Filter "BR-*.txt" -Force -ErrorAction SilentlyContinue |
    ForEach-Object { Write-Output ("SENTINEL PRESENT: " + $_.Name + " " + $_.Length + "B " + $_.LastWriteTime) }

# 5) C: volume serial (must be unchanged: no format happened)
& cmd.exe /c "vol C:" 2>&1 | ForEach-Object { Write-Output ("  V| " + $_) }

Write-Output "FORENSICS_DONE"