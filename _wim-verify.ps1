$out = 'C:\Users\x\Desktop\BackupRestore\_wim-verify.txt'
$lines = @()
$lines += '--- Q:\sources\boot.wim ---'
try { $lines += (Get-Item Q:\sources\boot.wim | Select-Object Length,LastWriteTime | Out-String).Trim() } catch { $lines += "err: $($_.Exception.Message)" }
try { $lines += "hash=" + (Get-FileHash Q:\sources\boot.wim -Algorithm SHA256).Hash } catch { $lines += "hash err: $($_.Exception.Message)" }
$lines += '--- C:\BackupRestorePE\media\media\sources\boot.wim ---'
try { $lines += (Get-Item 'C:\BackupRestorePE\media\media\sources\boot.wim' | Select-Object Length,LastWriteTime | Out-String).Trim() } catch { $lines += "err: $($_.Exception.Message)" }
try { $lines += "hash=" + (Get-FileHash 'C:\BackupRestorePE\media\media\sources\boot.wim' -Algorithm SHA256).Hash } catch { $lines += "hash err: $($_.Exception.Message)" }
$lines += '--- BackupRestorePE.wim ---'
try { $lines += "hash=" + (Get-FileHash 'C:\BackupRestorePE\BackupRestorePE.wim' -Algorithm SHA256).Hash } catch { $lines += "hash err: $($_.Exception.Message)" }
$lines += '--- PE entry verbose ---'
$lines += (bcdedit /enum {179ca179-ac73-11f1-8753-f4d0933357ef} /v 2>&1 | ForEach-Object { $_ })
$lines += '--- ramdiskoptions verbose ---'
$lines += (bcdedit /enum {ramdiskoptions} /v 2>&1 | ForEach-Object { $_ })
Set-Content -LiteralPath $out -Value $lines -Encoding UTF8
Write-Output 'WIMV_WRITTEN'
