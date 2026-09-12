# Check SVI size + vssadmin output via SYSTEM task (ASCII)
$out = 'C:\Users\Public\backupRestore-package\vss-check.log'
$lines = New-Object System.Collections.ArrayList
$svi = (Get-ChildItem 'C:\System Volume Information' -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
[void]$lines.Add(('SVI_MB={0}' -f [math]::Round($svi/1MB,0)))
$lines | Set-Content -Path $out -Encoding ascii
Write-Output 'DONE'
