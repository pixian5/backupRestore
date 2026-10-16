# br-pull-v2logs.ps1 - pull V2-era logs from VM to host share via -File (reliable chain)
$ErrorActionPreference = 'Continue'
$dst = "\\Mac\backupRestore\tools\win-clicker\_s1"
$stamp = Get-Date -Format "HHmmss"

function Pull($src, $name) {
    if (Test-Path -LiteralPath $src) {
        Copy-Item -LiteralPath $src -Destination (Join-Path $dst $name) -Force
        Write-Output ("pulled " + $src + " -> " + $name)
    } else { Write-Output ("MISSING " + $src) }
}

Pull "H:\brwork\logs\prepare-bootstrap.log" "prepare-bootstrap.log"
Pull "H:\brwork\logs\gui.log" "gui.log"
Pull "H:\brwork\logs\v2-baseline.txt" "v2-baseline.txt"

# task dirs: list + pull every file
$taskRoot = "H:\brwork\tasks"
if (Test-Path -LiteralPath $taskRoot) {
    Get-ChildItem -LiteralPath $taskRoot -Directory | ForEach-Object {
        $n = $_.Name
        Get-ChildItem -LiteralPath $_.FullName -Recurse -File | ForEach-Object {
            Pull $_.FullName ("task-" + $n + "-" + $_.Name)
        }
    }
}
Write-Output "PULL_DONE"
