# br-pe-payload.ps1 - read-only: mount a WinPE WIM and hash any BackupRestore /
# Recovery binaries inside it, so the PE payload entry point can be version-checked.
# ASCII only. Output: C:\Users\Public\pkg\pe-payload.txt (UTF-8 no BOM)
param([string]$Wim = "T:\petest\boot.wim", [string]$Tag = "")
$ErrorActionPreference = 'Continue'
$l = New-Object System.Collections.Generic.List[string]
function W($s) { [void]$l.Add([string]$s) }
$outName = "pe-payload.txt"
if ($Tag -ne "") { $outName = "pe-payload-$Tag.txt" }
function Flush { [System.IO.File]::WriteAllText("C:\Users\Public\pkg\$outName", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false))) }

$mnt = "C:\br-pe"
W ("TIME=" + (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))
W ("WIM=" + $Wim)
if (-not (Test-Path -LiteralPath $Wim)) { W "WIM_ABSENT"; Flush; Write-Output "PE_PAYLOAD_DONE"; exit }

try { $fi = Get-Item -LiteralPath $Wim -Force; W ("WIMFILE size=" + $fi.Length + " mtime=" + $fi.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss")) } catch {}
try { W ("WIMSHA=" + (Get-FileHash -Algorithm SHA256 -LiteralPath $Wim -ErrorAction Stop).Hash) } catch {}

$wi = & dism.exe /English /Get-WimInfo /WimFile:$Wim 2>&1 | ForEach-Object { $_.ToString() }
W "== Get-WimInfo =="
foreach ($x in $wi) { W ("  " + $x) }
W ""

if (Test-Path -LiteralPath $mnt) { & dism.exe /English /Unmount-Image /MountDir:$mnt /Discard 2>&1 | Out-Null }
New-Item -ItemType Directory -Path $mnt -Force | Out-Null
$mo = & dism.exe /English /Mount-Image /ImageFile:$Wim /Index:1 /MountDir:$mnt /ReadOnly 2>&1 | ForEach-Object { $_.ToString() }
W "== Mount-Image =="
foreach ($x in $mo) { if ($x -notmatch '^\s*\[' -and $x.Trim() -ne '') { W ("  " + $x) } }
W ""

if (Test-Path -LiteralPath $mnt) {
  W "== BackupRestore / Recovery binaries inside PE =="
  $found = $false
  Get-ChildItem -LiteralPath $mnt -Recurse -Force -File -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^(BackupRestore|Recovery)\.exe$' } |
    ForEach-Object {
      $found = $true
      $h = "HASH_ERR"
      try { $h = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName -ErrorAction Stop).Hash } catch {}
      W ("  " + $_.FullName.Replace($mnt, "") + " | " + $_.Length + " | " + $h + " | mtime=" + $_.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss"))
    }
  if (-not $found) { W "  NONE_FOUND" }
  W ""
  W "== PE root (top level) =="
  Get-ChildItem -LiteralPath $mnt -Force -ErrorAction SilentlyContinue | ForEach-Object {
    $k = "FILE"; if ($_.PSIsContainer) { $k = "DIR " }
    W ("  " + $k + " " + $_.Name)
  }
  $uo = & dism.exe /English /Unmount-Image /MountDir:$mnt /Discard 2>&1 | ForEach-Object { $_.ToString() }
  W ""
  W "== Unmount (discard) =="
  foreach ($x in $uo) { if ($x -notmatch '^\s*\[' -and $x.Trim() -ne '') { W ("  " + $x) } }
} else { W "MOUNT_DIR_MISSING" }

W ""
W "== END =="
Flush
Write-Output "PE_PAYLOAD_DONE"