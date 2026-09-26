# br-wim-evidence.ps1 - collect authoritative evidence for a WIM file:
#   size, SHA-256, /Get-WimInfo summary, and the matching DISM log command line.
# ASCII only. Output: C:\Users\Public\pkg\wim-evidence.txt (UTF-8 no BOM)
param(
  [string]$Wim = "C:\Users\Public\br-test\v174-20260926\test-none.wim",
  [string]$Note = ""
)
$ErrorActionPreference = 'Continue'
$l = New-Object System.Collections.Generic.List[string]
function W($s) { [void]$l.Add($s) }
W ("NOTE=" + $Note)
W ("TIME=" + (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))
W ("WIM=" + $Wim)
if (-not (Test-Path -LiteralPath $Wim)) {
  W "WIM_ABSENT"
} else {
  $f = Get-Item -LiteralPath $Wim
  W ("size=" + $f.Length + " bytes")
  W ("mtime=" + $f.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss"))
  W ("sha256=" + (Get-FileHash -LiteralPath $Wim -Algorithm SHA256).Hash)
  W ""
  W "== dism /Get-WimInfo =="
  $info = & dism.exe /Get-WimInfo /WimFile:$Wim 2>&1 | ForEach-Object { $_.ToString() }
  foreach ($line in $info) { W ("  " + $line) }
}
W ""
W "== DISM log command lines (last 6 mentioning this wim) =="
$log = "C:\Windows\Logs\DISM\dism.log"
if (Test-Path $log) {
  $hits = Select-String -Path $log -Pattern 'dism\.exe' -SimpleMatch:$false
  $n = 0
  for ($i = $hits.Count - 1; $i -ge 0 -and $n -lt 6; $i--) {
    $line = $hits[$i].Line
    if ($line -like "*" + (Split-Path $Wim -Leaf) + "*") {
      W ("  " + $line)
      $n++
    }
  }
  if ($n -eq 0) { W "  (no dism.exe line found for this file)" }
}
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\wim-evidence.txt", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output "WIM_EVIDENCE_DONE"