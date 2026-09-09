$out = 'C:\Users\x\Desktop\BackupRestore\_verify-deploy2.txt'
$lines = @()
$lines += '--- Q sources ---'
$lines += (Get-ChildItem Q:\sources -ErrorAction SilentlyContinue | ForEach-Object { "$($_.Name) $($_.Length)" })
try {
    $lines += '--- Q boot.wim hash ---'
    $lines += (Get-FileHash Q:\sources\boot.wim -Algorithm SHA256).Hash
} catch { $lines += "boot.wim hash error: $($_.Exception.Message)" }
try {
    $lines += '--- Q boot.wim.stock hash ---'
    $lines += (Get-FileHash Q:\sources\boot.wim.stock -Algorithm SHA256).Hash
} catch { $lines += "stock hash error: $($_.Exception.Message)" }
$lines += '--- BCD full ---'
$lines += (bcdedit /enum 2>&1 | ForEach-Object { $_ })
Set-Content -LiteralPath $out -Value $lines -Encoding UTF8
Write-Output 'V2_WRITTEN'
