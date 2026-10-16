# br-extract-reagent.ps1 - extract reagentc-related lines from Recovery.reagent.log
# and the product-side V2 failure evidence, writing both to the host share for reading.

$ErrorActionPreference = 'Continue'
$dst = "\\Mac\backupRestore\tools\win-clicker\_s1"

# 1) reagentc log timeline (keyword filter, keep line numbers for traceability)
$src = "H:\BRImages\Recovery.reagent.log"
if (Test-Path -LiteralPath $src) {
    $lines = Get-Content -LiteralPath $src
    $hits = @()
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match "reagentc|WinReGetWimVersion|initialize DISM|/info|/enable|/disable|setreimage|Error:|Exit|exit code") {
            $hits += ("{0,6}: {1}" -f ($i + 1), $lines[$i])
        }
    }
    [System.IO.File]::WriteAllLines((Join-Path $dst "reagent-timeline.txt"), $hits, [System.Text.Encoding]::UTF8)
    Write-Output ("reagent-timeline lines=" + $hits.Count)
} else { Write-Output "MISSING $src" }

# 2) product log: last 400 lines of Recovery.log (covers V1 restore + V2 failure)
$src2 = "H:\BRImages\Recovery.log"
if (Test-Path -LiteralPath $src2) {
    $l2 = Get-Content -LiteralPath $src2 -Tail 400
    [System.IO.File]::WriteAllLines((Join-Path $dst "recovery-tail.txt"), $l2, [System.Text.Encoding]::UTF8)
    Write-Output "recovery-tail lines=" + $l2.Count
} else { Write-Output "MISSING $src2" }

# 3) locate launcher-errors.log and any prepare-failure logs under H:\brwork
$found = Get-ChildItem -Path "H:\brwork" -Recurse -Filter "launcher-errors.log" -ErrorAction SilentlyContinue
foreach ($f in $found) {
    $body = Get-Content -LiteralPath $f.FullName -Tail 60
    [System.IO.File]::WriteAllLines((Join-Path $dst ("launcher-errors-" + $f.Directory.Name + ".txt")), $body, [System.Text.Encoding]::UTF8)
    Write-Output ("launcher-errors: " + $f.FullName)
}
Write-Output "EXTRACT_DONE"
