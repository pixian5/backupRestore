# br-macdisk-diag.ps1 - read-only diagnosis of the T:\Mac disk file that blocks
# dism /Apply-Image with ERROR_SHARING_VIOLATION (0x80070020).
# Does NOT modify, rename or delete anything. ASCII only.
# Output: C:\Users\Public\pkg\macdisk-diag.txt (UTF-8 no BOM)
$ErrorActionPreference = 'Continue'
$l = New-Object System.Collections.Generic.List[string]
function W($s) { [void]$l.Add($s) }
$p = "T:\Mac disk"
W ("TIME=" + (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))
W ("PATH=" + $p)
if (-not (Test-Path -LiteralPath $p)) { W "ABSENT"; } else {
  $i = Get-Item -LiteralPath $p -Force
  W ("Exists=True Length=" + $i.Length + " Attributes=" + $i.Attributes)
  W ("CreationTime=" + $i.CreationTime.ToString("yyyy-MM-dd HH:mm:ss") + " LastWriteTime=" + $i.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss"))
  W ("FullName=[" + $i.FullName + "]")
  W ""
  W "== open tests =="
  foreach ($mode in @('ReadWrite','Write','Read')) {
    foreach ($share in @('None','ReadWrite')) {
      try {
        $fs = [System.IO.File]::Open($p, [System.IO.FileMode]::Open, [System.IO.FileAccess]::$mode, [System.IO.FileShare]::$share)
        $fs.Close()
        W ("  access=" + $mode + " share=" + $share + " -> OPEN_OK")
      } catch {
        W ("  access=" + $mode + " share=" + $share + " -> FAIL " + $_.Exception.GetType().Name + " 0x" + ("{0:X8}" -f $_.Exception.HResult) + " " + $_.Exception.Message)
      }
    }
  }
  W ""
  W "== reparse point =="
  $rp = & fsutil.exe reparsepoint query "$p" 2>&1 | ForEach-Object { $_.ToString() }
  foreach ($line in $rp) { W ("  " + $line) }
}
W ""
W "== Parallels processes =="
Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -like 'prl*' } | ForEach-Object {
  W ("  " + $_.ProcessName + " pid=" + $_.Id + " session=" + $_.SessionId)
}
W ""
W "== services =="
Get-Service -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'prl*' } | ForEach-Object {
  W ("  " + $_.Name + " status=" + $_.Status + " display=[" + $_.DisplayName + "]")
}
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\macdisk-diag.txt", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output "MACDISK_DIAG_DONE"