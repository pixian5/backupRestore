$out = 'C:\Users\x\Desktop\BackupRestore\_rollback-check.txt'
$lines = @()
$lines += '--- C:\WinPE_arm64 (stock copype) ---'
$lines += (Get-ChildItem 'C:\WinPE_arm64' -ErrorAction SilentlyContinue | ForEach-Object { $_.Name })
$lines += '--- C:\WinPE_arm64\media\sources ---'
$lines += (Get-ChildItem 'C:\WinPE_arm64\media\sources' -ErrorAction SilentlyContinue | ForEach-Object { "$($_.Name) $($_.Length)" })
$lines += '--- C:\BackupRestorePE exists? ---'
$lines += "exists: $(Test-Path 'C:\BackupRestorePE')"
$lines += '--- Q root ---'
$lines += (Get-ChildItem Q:\ -Force -ErrorAction SilentlyContinue | ForEach-Object { $_.Name })
$lines += '--- Q sources ---'
$lines += (Get-ChildItem Q:\sources -Force -ErrorAction SilentlyContinue | ForEach-Object { "$($_.Name) $($_.Length)" })
$lines += '--- BCD bootmgr ---'
$lines += (bcdedit /enum "{bootmgr}" 2>&1 | ForEach-Object { $_ })
$lines += '--- BCD all entries (identifiers) ---'
$lines += (bcdedit /enum 2>&1 | Select-String -Pattern '标识符' | ForEach-Object { $_.Line })
Set-Content -LiteralPath $out -Value $lines -Encoding UTF8
Write-Output 'RC_WRITTEN'
