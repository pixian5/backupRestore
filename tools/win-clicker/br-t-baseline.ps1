# br-t-baseline.ps1 -- T: test-volume baseline / damage / verification helper.
# ASCII ONLY on purpose (PowerShell -File decodes scripts as system ANSI/GBK).
#
# Phases:
#   init      write T:\BRTEST174-20260926-before.txt (fixed string + timestamp), print its SHA-256
#   snapshot  enumerate T:\ with SHA-256 of every file (used for before / after-restore)
#   damage    mutate marker, rename data4.bin, delete data3.bin, add after.txt
#   reset     remove ONLY this test's own leftovers (after.txt, data4.bin.renamed)
#             so the next backup starts from an undamaged baseline. Never touches
#             user files, petest, Mac disk or the recycle bin.
# Output: C:\Users\Public\pkg\t-<Phase>.txt (UTF-8 no BOM)

param(
    [string]$Phase = "snapshot",
    [string]$Out = ""
)

$pkg    = "C:\Users\Public\pkg"
$marker = "T:\BRTEST174-20260926-before.txt"
$after  = "T:\BRTEST174-20260926-after.txt"
if ($Out -eq "") { $Out = "$pkg\t-$Phase.txt" }

function Sha([string]$p) {
    try { return (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash }
    catch { return ("HASH_ERR:" + $_.Exception.Message) }
}

$lines = New-Object System.Collections.Generic.List[string]
$lines.Add("PHASE=" + $Phase)
$lines.Add("TIME=" + (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))

$vol = Get-Volume -DriveLetter T -ErrorAction SilentlyContinue
if ($vol -ne $null) {
    $lines.Add("VOLUME label=" + $vol.FileSystemLabel + " fs=" + $vol.FileSystem + " size=" + $vol.Size + " free=" + $vol.SizeRemaining)
} else {
    $lines.Add("VOLUME ABSENT")
}

if ($Phase -eq "init") {
    $stamp = (Get-Date -Format "yyyy-MM-dd HH:mm:ss")
    $text = "BRTEST174 baseline marker`r`nFIXED-STRING=BRTEST174-20260926-KEEP-ME`r`nTIMESTAMP=$stamp`r`n"
    [System.IO.File]::WriteAllText($marker, $text, (New-Object System.Text.UTF8Encoding($false)))
    $lines.Add("MARKER_WRITTEN " + $marker + " bytes=" + (Get-Item -LiteralPath $marker).Length + " sha256=" + (Sha $marker))
}

if ($Phase -eq "damage") {
    [System.IO.File]::WriteAllText($marker, "DAMAGED-BY-TEST-2026-09-27`r`nFIXED-STRING=MUTATED`r`n", (New-Object System.Text.UTF8Encoding($false)))
    $lines.Add("DAMAGE marker rewritten sha256=" + (Sha $marker))

    if (Test-Path -LiteralPath "T:\data4.bin") {
        Move-Item -LiteralPath "T:\data4.bin" -Destination "T:\data4.bin.renamed" -Force
        $lines.Add("DAMAGE renamed T:\data4.bin -> T:\data4.bin.renamed")
    } else { $lines.Add("DAMAGE SKIP data4.bin absent") }

    if (Test-Path -LiteralPath "T:\data3.bin") {
        Remove-Item -LiteralPath "T:\data3.bin" -Force
        $lines.Add("DAMAGE deleted T:\data3.bin")
    } else { $lines.Add("DAMAGE SKIP data3.bin absent") }

    [System.IO.File]::WriteAllText($after, "SHOULD-NOT-SURVIVE-RESTORE`r`n", (New-Object System.Text.UTF8Encoding($false)))
    $lines.Add("DAMAGE added " + $after)
}

if ($Phase -eq "reset") {
    foreach ($p in @($after, "T:\data4.bin.renamed")) {
        if (Test-Path -LiteralPath $p) {
            Remove-Item -LiteralPath $p -Force
            $lines.Add("RESET removed " + $p)
        } else { $lines.Add("RESET SKIP absent " + $p) }
    }
}

$items = @(Get-ChildItem -LiteralPath "T:\" -Force -Recurse -ErrorAction SilentlyContinue)
foreach ($f in $items) {
    if ($f.PSIsContainer) {
        $lines.Add("DIR  " + $f.FullName)
    } else {
        $len = "?"
        try { $len = [string]$f.Length } catch { }
        $lines.Add("FILE " + $f.FullName + " | " + $len + " | " + (Sha $f.FullName))
    }
}
$lines.Add("TOTAL_ITEMS=" + $items.Count)

[System.IO.File]::WriteAllText($Out, ($lines -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output ("SNAPSHOT_DONE phase=" + $Phase + " file=" + $Out + " items=" + $items.Count)