$out = 'C:\Users\x\Desktop\BackupRestore\_rc2.txt'
$lines = [System.Collections.ArrayList]::new()
$null = $lines.Add('--- C:\WinPE_arm64 ---')
try { Get-ChildItem 'C:\WinPE_arm64' -ErrorAction Stop | ForEach-Object { $null = $lines.Add($_.Name) } } catch { $null = $lines.Add('ERR: ' + $_.Exception.Message) }
$null = $lines.Add('--- C:\WinPE_arm64\media\sources ---')
try { Get-ChildItem 'C:\WinPE_arm64\media\sources' -ErrorAction Stop | ForEach-Object { $null = $lines.Add("$($_.Name) $($_.Length)") } } catch { $null = $lines.Add('ERR: ' + $_.Exception.Message) }
$null = $lines.Add("--- BackupRestorePE exists: $(Test-Path 'C:\BackupRestorePE')")
$null = $lines.Add('--- Q root ---')
try { Get-ChildItem Q:\ -Force -ErrorAction Stop | ForEach-Object { $null = $lines.Add($_.Name) } } catch { $null = $lines.Add('ERR: ' + $_.Exception.Message) }
$null = $lines.Add('--- Q sources ---')
try { Get-ChildItem Q:\sources -Force -ErrorAction Stop | ForEach-Object { $null = $lines.Add("$($_.Name) $($_.Length)") } } catch { $null = $lines.Add('ERR: ' + $_.Exception.Message) }
Set-Content -LiteralPath $out -Value $lines -Encoding UTF8
Write-Output 'RC2_WRITTEN'
