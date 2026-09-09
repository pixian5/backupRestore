$out = 'C:\Users\x\Desktop\BackupRestore\_bcd-pe-default.txt'
$lines = @()
$lines += '--- before ---'
$lines += (bcdedit /enum "{bootmgr}" 2>&1 | ForEach-Object { $_ })
bcdedit /set "{bootmgr}" default "{179ca179-ac73-11f1-8753-f4d0933357ef}"
$lines += "set default exit=$LASTEXITCODE"
bcdedit /timeout 5
$lines += "set timeout exit=$LASTEXITCODE"
$lines += '--- after ---'
$lines += (bcdedit /enum "{bootmgr}" 2>&1 | ForEach-Object { $_ })
Set-Content -LiteralPath $out -Value $lines -Encoding UTF8
Write-Output 'BCD_DONE'
