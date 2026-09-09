$out = 'C:\Users\x\Desktop\BackupRestore\_adk-font-check.txt'
$lines = @()
$root = 'C:\Program Files (x86)\Windows Kits\10\Assessment and Deployment Kit\Windows Preinstallation Environment\arm64'
$lines += "root exists: $(Test-Path $root)"
$lines += '--- WinPE_OCS recurse Font ---'
$lines += (Get-ChildItem (Join-Path $root 'WinPE_OCS') -Recurse -Filter '*Font*' -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })
$lines += '--- WinPE_OCS zh-cn ---'
$lines += (Get-ChildItem (Join-Path $root 'WinPE_OCS\zh-cn') -ErrorAction SilentlyContinue | ForEach-Object { $_.Name })
$lines += '--- also check amd64 for reference ---'
$lines += (Get-ChildItem (Join-Path $root 'WinPE_OCS\en-us') -ErrorAction SilentlyContinue | ForEach-Object { $_.Name })
Set-Content -LiteralPath $out -Value $lines -Encoding UTF8
Write-Output 'CHECK_WRITTEN'
