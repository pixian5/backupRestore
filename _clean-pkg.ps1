$out = 'C:\Users\x\Desktop\BackupRestore\_clean-pkg.txt'
$lines = @()
Get-Process BackupRestore,Recovery -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
try {
    Remove-Item -LiteralPath 'C:\BackupRestoreBuild\package\BackupRestore-windows-arm64-v1.3.3' -Recurse -Force -ErrorAction Stop
    $lines += 'package removed'
} catch {
    $lines += "remove failed: $($_.Exception.Message)"
}
$lines += '--- package dir now ---'
$lines += (Get-ChildItem 'C:\BackupRestoreBuild\package' -ErrorAction SilentlyContinue | ForEach-Object { $_.Name })
Set-Content -LiteralPath $out -Value $lines -Encoding UTF8
Write-Output 'CLEAN_DONE'
