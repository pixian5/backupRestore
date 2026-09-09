$out = 'C:\Users\x\Desktop\BackupRestore\_q-check.txt'
$lines = @()
$lines += '--- Q volume ---'
$lines += (Get-Volume -DriveLetter Q -ErrorAction SilentlyContinue | Select-Object DriveLetter,FileSystemLabel,FileSystem,Size,SizeRemaining | Format-List | Out-String)
$lines += '--- Q root (elevated) ---'
$lines += (Get-ChildItem Q:\ -Force -ErrorAction SilentlyContinue | ForEach-Object { $_.Name })
$lines += '--- C:\BackupRestorePE\media\media ---'
$lines += (Get-ChildItem 'C:\BackupRestorePE\media\media' -ErrorAction SilentlyContinue | ForEach-Object { $_.Name })
$lines += '--- media\media\sources ---'
$lines += (Get-ChildItem 'C:\BackupRestorePE\media\media\sources' -ErrorAction SilentlyContinue | ForEach-Object { "$($_.Name) $($_.Length)" })
Set-Content -LiteralPath $out -Value $lines -Encoding UTF8
Write-Output 'QCHECK_WRITTEN'
