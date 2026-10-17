# br-diag-elog2.ps1 -- 读原 C 系统（E:）卷上的事件日志（正确路径 Logs）
$ErrorActionPreference = 'Continue'
$p = "E:\Windows\System32\winevt\Logs\System.evtx"
"exists=" + (Test-Path $p)
"mtime=" + (Get-Item $p).LastWriteTime.ToString("MM-dd HH:mm:ss")
"=== E: System.evtx events after 05:30 ==="
try {
  Get-WinEvent -Path $p -ErrorAction Stop | Where-Object { $_.TimeCreated -gt (Get-Date "2026-10-18 05:30:00") } |
    Sort-Object TimeCreated | Select-Object -First 45 | ForEach-Object {
      $_.TimeCreated.ToString("HH:mm:ss") + " id=" + $_.Id + " prov=" + $_.ProviderName
    }
} catch {
  "ERR: " + $_.Exception.Message
}
