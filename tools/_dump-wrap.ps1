$env:TEMP = "C:\Users\Public"
$env:TMP  = "C:\Users\Public"
try { Start-Transcript -LiteralPath "H:\brwork\_dump.log" -Force | Out-Null } catch { }
& "H:\brwork\dump-controls.ps1"
try { Stop-Transcript | Out-Null } catch { }
