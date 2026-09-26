# br-svc-probe.ps1 - read-only: identify the service whose image is
# prl_tools_service.exe, plus the state of the Parallels VSS provider.
# ASCII only. Output: C:\Users\Public\pkg\svc-probe.txt (UTF-8 no BOM)
$ErrorActionPreference = 'Continue'
$l = New-Object System.Collections.Generic.List[string]
function W($s) { [void]$l.Add([string]$s) }

W ("TIME=" + (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))
W ""
W "== services whose path mentions prl_tools_service =="
Get-CimInstance Win32_Service -ErrorAction SilentlyContinue |
  Where-Object { $_.PathName -match 'prl_tools_service' } |
  ForEach-Object { W ("  Name=[" + $_.Name + "] Display=[" + $_.DisplayName + "] State=" + $_.State + " StartMode=" + $_.StartMode + " Pid=" + $_.ProcessId + " Path=" + $_.PathName) }

W ""
W "== all services whose Name starts with Prl or DisplayName mentions Parallels =="
Get-CimInstance Win32_Service -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -like 'Prl*' -or $_.DisplayName -like '*Parallels*' } |
  ForEach-Object { W ("  Name=[" + $_.Name + "] Display=[" + $_.DisplayName + "] State=" + $_.State + " StartMode=" + $_.StartMode + " Pid=" + $_.ProcessId + " Path=" + $_.PathName) }

W ""
W "== process 4116 =="
try {
  $p = Get-Process -Id 4116 -ErrorAction Stop
  W ("  name=" + $p.ProcessName + " path=" + $p.Path + " session=" + $p.SessionId + " start=" + $p.StartTime.ToString("yyyy-MM-dd HH:mm:ss"))
} catch { W ("  pid 4116 not found: " + $_.Exception.Message) }

W ""
W "== all prl* processes (pid/path/session) =="
Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -like 'prl*' } | ForEach-Object {
  $pp = "?"
  try { $pp = $_.Path } catch {}
  $st = "?"
  try { $st = $_.StartTime.ToString("yyyy-MM-dd HH:mm:ss") } catch {}
  W ("  " + $_.ProcessName + " pid=" + $_.Id + " session=" + $_.SessionId + " start=" + $st + " path=" + $pp)
}

W ""
W "== surviving handles: who has T:\ root open (drive-letter session, read-only) =="
try {
  $out = & fsutil.exe volume queryinfo T: 2>&1 | ForEach-Object { $_.ToString() }
  foreach ($o in $out) { W ("  " + $o) }
} catch { W ("  fsutil err " + $_.Exception.Message) }

[System.IO.File]::WriteAllText("C:\Users\Public\pkg\svc-probe.txt", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output "SVC_PROBE_DONE"