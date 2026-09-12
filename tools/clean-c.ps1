# Clean C: dev residue: BackupRestoreBuild, BRTest, brsrc, empty test dirs (ASCII log)
$log = 'C:\Users\Public\backupRestore-package\clean-c.log'
$res = New-Object System.Collections.ArrayList

$targets = @(
    'C:\BackupRestoreBuild',
    'C:\BRTest',
    'C:\brsrc'
)
foreach ($t in $targets) {
    if (Test-Path $t) {
        try {
            [System.IO.Directory]::Delete("\\?\\$t", $true)
            if (Test-Path $t) { [void]$res.Add("FAILED: $t") } else { [void]$res.Add("removed: $t") }
        } catch { [void]$res.Add("ERROR: $t : $($_.Exception.Message)") }
    }
}

# empty dirs (no recursive content)
$empties = @('C:\Quick Scan C','C:\ESPRead','C:\PEMount','C:\pewimmount','C:\PEVerify','C:\DiskGenius_WinPE','C:\WinRE-PoC','C:\Temp')
foreach ($t in $empties) {
    if (Test-Path $t) {
        $c = (Get-ChildItem $t -Recurse -Force -File -ErrorAction SilentlyContinue | Measure-Object).Count
        if ($c -eq 0) { Remove-Item $t -Recurse -Force -ErrorAction SilentlyContinue; [void]$res.Add("empty-dir removed: $t") }
    }
}

$res | Set-Content -Path $log -Encoding ascii
Write-Output 'DONE'
