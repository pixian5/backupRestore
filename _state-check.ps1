$out = 'C:\Users\x\Desktop\BackupRestore\_state-check.txt'
$lines = @()
$lines += '--- volumes ---'
$lines += (Get-PSDrive -PSProvider FileSystem | Where-Object { $_.Name -match '^[A-Z]$' } | ForEach-Object { "$($_.Name): $($_.Root)" })
$lines += '--- Q root ---'
$lines += (Get-ChildItem Q:\ -ErrorAction SilentlyContinue | ForEach-Object { $_.Name })
$lines += '--- Q sources ---'
$lines += (Get-ChildItem Q:\sources -ErrorAction SilentlyContinue | ForEach-Object { "$($_.Name) $($_.Length)" })
$lines += '--- BCD bootmgr ---'
$lines += (bcdedit /enum {bootmgr} 2>&1 | ForEach-Object { $_ })
$lines += '--- BCD PE entry ---'
$lines += (bcdedit /enum 2>&1 | Select-String -Pattern 'Windows PE Test|ramdisk|winload' | ForEach-Object { $_.Line })
Set-Content -LiteralPath $out -Value $lines -Encoding UTF8
Write-Output 'STATE_WRITTEN'
