# br-diag-elog.ps1 -- 读原 C 系统（E:）卷上的事件日志
$ErrorActionPreference = 'Continue'
$p = "E:\Windows\System32\winevt\Log\System.evtx"
"exists=" + (Test-Path $p)
"mtime=" + (Get-Item $p).LastWriteTime.ToString("HH:mm:ss")
"=== E: System.evtx events after 05:35 ==="
try {
  Get-WinEvent -Path $p -ErrorAction Stop | Where-Object { $_.TimeCreated -gt (Get-Date "2026-10-18 05:35:00") } |
    Sort-Object TimeCreated | Select-Object -First 40 | ForEach-Object {
      $_.TimeCreated.ToString("HH:mm:ss") + " id=" + $_.Id + " prov=" + $_.ProviderName + " | " + (($_.Message -replace "`r?`n"," ")).Substring(0, [Math]::Min(70, ($_.Message -replace "`r?`n"," ").Length))
    }
} catch {
  "ERR: " + $_.Exception.Message
}
