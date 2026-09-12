# Delete backupRestore-src* dirs using \\?\ long-path prefix (handles reserved names like nul)
$log = 'C:\Users\Public\backupRestore-package-v12\cleanup-src.log'
$result = New-Object System.Collections.ArrayList
$names = @('backupRestore-src') + (2..11 | ForEach-Object { "backupRestore-src-v$_" })
foreach ($n in $names) {
    $p = "C:\Users\Public\$n"
    if (Test-Path $p) {
        try {
            [System.IO.Directory]::Delete("\\?\\$p", $true)
            if (Test-Path $p) { [void]$result.Add("FAILED: $n") } else { [void]$result.Add("removed: $n") }
        } catch {
            [void]$result.Add("ERROR: $n : $($_.Exception.Message)")
        }
    }
}
$result | Set-Content -Path $log -Encoding ascii
Write-Output "DONE"
