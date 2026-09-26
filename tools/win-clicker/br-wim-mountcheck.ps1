# br-wim-mountcheck.ps1 - read-only inspection of a WIM: mount, list root entries
# with size + SHA-256 (files <= 60MB), then unmount/discard. No T: writes.
# ASCII only. Output: C:\Users\Public\pkg\wim-mount-<Tag>.txt (UTF-8 no BOM)
param([string]$Wim, [string]$Tag = "wim")
$ErrorActionPreference = 'Continue'
$l = New-Object System.Collections.Generic.List[string]
function W($s) { [void]$l.Add([string]$s) }
function Flush { [System.IO.File]::WriteAllText("C:\Users\Public\pkg\wim-mount-$Tag.txt", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false))) }

$mnt = "C:\br-mnt"
W ("TIME=" + (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))
W ("WIM=" + $Wim)
W ("MNT=" + $mnt)
W ""

if (-not (Test-Path -LiteralPath $Wim)) { W "WIM_ABSENT"; Flush; Write-Output "WIM_MOUNT_DONE"; exit }

$wi = & dism.exe /English /Get-WimInfo /WimFile:$Wim 2>&1 | ForEach-Object { $_.ToString() }
W "== Get-WimInfo =="
foreach ($x in $wi) { W ("  " + $x) }
W ""

Get-ChildItem -LiteralPath $Wim -Force | ForEach-Object { W ("WIMFILE size=" + $_.Length + " mtime=" + $_.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss")) }
try { W ("WIMSHA=" + (Get-FileHash -Algorithm SHA256 -LiteralPath $Wim -ErrorAction Stop).Hash) } catch { W "WIMSHA_ERR" }
W ""

if (Test-Path -LiteralPath $mnt) { & dism.exe /English /Unmount-Image /MountDir:$mnt /Discard 2>&1 | Out-Null }
New-Item -ItemType Directory -Path $mnt -Force | Out-Null

$mo = & dism.exe /English /Mount-Image /ImageFile:$Wim /Index:1 /MountDir:$mnt /ReadOnly 2>&1 | ForEach-Object { $_.ToString() }
W "== Mount-Image =="
foreach ($x in $mo) { W ("  " + $x) }
W ""

if (Test-Path -LiteralPath $mnt) {
  W "== WIM ROOT CONTENT =="
  Get-ChildItem -LiteralPath $mnt -Force -ErrorAction SilentlyContinue | ForEach-Object {
    $kind = "FILE"
    if ($_.PSIsContainer) { $kind = "DIR " }
    $h = ""
    if (-not $_.PSIsContainer -and $_.Length -le 62914560) {
      try { $h = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName -ErrorAction Stop).Hash } catch { $h = "HASH_ERR" }
    }
    W ("  " + $kind + " " + $_.Name + " | " + $_.Length + " | " + $h)
  }
  W ""
  W "== EXTRA CHECKS =="
  foreach ($n in @("BRTEST174-20260926-before.txt", "BRTEST174-20260926-after.txt", "data4.bin.renamed", "keep-marker.txt", "Mac disk")) {
    $p = Join-Path $mnt $n
    if (Test-Path -LiteralPath $p) { W ("  PRESENT " + $n + " size=" + (Get-Item -LiteralPath $p -Force).Length) }
    else { W ("  ABSENT  " + $n) }
  }
  $uo = & dism.exe /English /Unmount-Image /MountDir:$mnt /Discard 2>&1 | ForEach-Object { $_.ToString() }
  W ""
  W "== Unmount (discard) =="
  foreach ($x in $uo) { W ("  " + $x) }
} else {
  W "MOUNT_DIR_MISSING"
}

W ""
W "== END =="
Flush
Write-Output "WIM_MOUNT_DONE"