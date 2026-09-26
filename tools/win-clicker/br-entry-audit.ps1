# br-entry-audit.ps1 - read-only inventory of every real BackupRestore entry point.
# Run ELEVATED through br-gui-exec.ps1 so High-IL process/path/hash reads succeed:
#   powershell -File X:\tools\win-clicker\br-gui-exec.ps1 -Script br-entry-audit.ps1 -Log audit.txt
# ASCII-only on purpose (GBK console safety). Every section is isolated in try/catch.
$ErrorActionPreference = 'SilentlyContinue'

function Sec($t) { Write-Output ""; Write-Output ("### " + $t) }
function HashOf($p) {
  try { $h = (Get-FileHash -Algorithm SHA256 -LiteralPath $p -ErrorAction Stop).Hash; return $h }
  catch { return ("HASH_ERR:" + $_.Exception.GetType().Name) }
}
function ShowFile($p) {
  try {
    $i = Get-Item -LiteralPath $p -Force -ErrorAction Stop
  } catch { Write-Output ("MISSING_OR_DENIED " + $p); return }
  $len = $i.Length
  $ver = "?"
  try { $ver = $i.VersionInfo.FileVersion } catch {}
  $mt = $i.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss")
  $h = HashOf $p
  Write-Output ($p + " | size=" + $len + " | sha256=" + $h + " | ver=" + $ver + " | mtime=" + $mt)
}
function TreeFiles($root, $depth) {
  try {
    Get-ChildItem -LiteralPath $root -Recurse -Force -File -Depth $depth -ErrorAction SilentlyContinue |
      Where-Object { $_.Name -eq 'BackupRestore.exe' -or $_.Name -eq 'Recovery.exe' } |
      ForEach-Object { $_.FullName }
  } catch {}
}

Sec "A. RUNNING PROCESSES"
try {
  $procs = Get-Process -ErrorAction SilentlyContinue
  foreach ($pr in $procs) {
    if ($pr.ProcessName -notmatch 'BackupRestore|Recovery') { continue }
    $path = "?"
    try { $path = $pr.Path } catch {}
    $title = "?"
    try { $title = $pr.MainWindowTitle } catch {}
    $ver = "?"
    try { $ver = $pr.MainModule.FileVersionInfo.FileVersion } catch {}
    $h = "?"
    try { $h = HashOf $path } catch {}
    Write-Output ("PROC name=" + $pr.ProcessName + " pid=" + $pr.Id + " title=[" + $title + "] ver=" + $ver + " path=" + $path + " sha256=" + $h)
  }
} catch { Write-Output ("SECTION_A_ERR " + $_.Exception.Message) }

Sec "B. KNOWN PACKAGE DIRS"
foreach ($d in @('C:\Users\Public\backupRestore-package', 'C:\Users\Public\backupRestore-package-v12', 'C:\Users\Public\pkg')) {
  if (-not (Test-Path -LiteralPath $d)) { Write-Output ("DIR_ABSENT " + $d); continue }
  Write-Output ("DIR_PRESENT " + $d)
  try {
    Get-ChildItem -LiteralPath $d -Force -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '\.(exe|dll|ini|wim)$' } | ForEach-Object { ShowFile $_.FullName }
  } catch { Write-Output ("B_ERR " + $_.Exception.Message) }
}

Sec "C. ALL EXE COPIES UNDER CANDIDATE ROOTS"
$roots = @('C:\Recovery', 'C:\Windows\System32\Recovery', 'C:\Windows\Boot', 'C:\ProgramData', 'C:\Users\Public', 'C:\BackupRestoreBuild', 'C:\Users\x\Desktop')
foreach ($r in $roots) {
  foreach ($f in (TreeFiles $r 5)) { ShowFile $f }
}

Sec "D1. SHORTCUTS (Desktop + Startup)"
foreach ($d in @("$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Startup", 'C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup', 'C:\Users\x\Desktop', 'C:\Users\Public\Desktop')) {
  if (-not (Test-Path -LiteralPath $d)) { continue }
  Get-ChildItem -LiteralPath $d -Force -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'BackupRestore|Recovery|\.lnk$' } | ForEach-Object {
    $tgt = ''
    try { $tgt = (New-Object -ComObject WScript.Shell).CreateShortcut($_.FullName).TargetPath } catch {}
    Write-Output ($_.FullName + " -> [" + $tgt + "]")
  }
}

Sec "D2. RUN KEYS"
foreach ($k in @('HKCU:\Software\Microsoft\Windows\CurrentVersion\Run', 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Run')) {
  try {
    $props = Get-ItemProperty -Path $k -ErrorAction Stop
    foreach ($n in $props.PSObject.Properties.Name) {
      if ($n -like 'PS*') { continue }
      Write-Output ($k + " :: " + $n + " = " + $props.$n)
    }
  } catch {}
}

Sec "D3. SCHEDULED TASKS REFERENCING BR (name | exec | args | state)"
try {
  foreach ($t in (Get-ScheduledTask -ErrorAction SilentlyContinue)) {
    foreach ($a in $t.Actions) {
      $blob = "$($a.Execute) $($a.Arguments)"
      if ($blob -match 'BackupRestore|Recovery\.exe|br-agent|clicker') {
        Write-Output ($t.TaskName + " | " + $a.Execute + " | " + $a.Arguments + " | state=" + $t.State)
      }
    }
  }
} catch { Write-Output ("D3_ERR " + $_.Exception.Message) }

Sec "E. TASK ROOTS"
$taskRoots = @('C:\Users\Public\backupRestore-package\tasks', 'C:\Users\Public\pkg\tasks', 'T:\tasks', 'C:\tasks', 'C:\Users\Public\br-test')
foreach ($tr in $taskRoots) {
  if (Test-Path -LiteralPath $tr) {
    Write-Output ("TASKROOT_PRESENT " + $tr)
    Get-ChildItem -LiteralPath $tr -Force -ErrorAction SilentlyContinue | Select-Object -First 20 | ForEach-Object {
      Write-Output ("   " + $_.Name + " | dir=" + $_.PSIsContainer + " | mtime=" + $_.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss"))
    }
  } else { Write-Output ("TASKROOT_ABSENT " + $tr) }
}

Sec "F. WORKSPACE/TASK ARTIFACT SEARCH (task.json / task.env)"
foreach ($r in @('C:\Users\Public\backupRestore-package', 'C:\Users\Public\pkg', 'C:\BackupRestoreBuild', 'T:\')) {
  try {
    Get-ChildItem -LiteralPath $r -Recurse -Force -File -Include 'task.json', '*.env', 'status.json' -Depth 4 -ErrorAction SilentlyContinue |
      Select-Object -First 25 | ForEach-Object { Write-Output ($_.FullName + " | " + $_.Length + " | " + $_.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss")) }
  } catch {}
}

Sec "G. VOLUMES + T: BASELINE"
try {
  Get-Volume -ErrorAction SilentlyContinue | Where-Object { $_.DriveLetter -in @('C', 'T') } | ForEach-Object {
    Write-Output ("VOL " + $_.DriveLetter + ": label=" + $_.FileSystemLabel + " fs=" + $_.FileSystem + " size=" + $_.Size + " free=" + $_.SizeRemaining)
  }
} catch {}
if (Test-Path -LiteralPath 'T:\') {
  Get-ChildItem -LiteralPath 'T:\' -Force -ErrorAction SilentlyContinue | ForEach-Object {
    $kind = 'FILE'
    if ($_.PSIsContainer) { $kind = 'DIR' }
    Write-Output ("T-ENTRY " + $kind + " " + $_.Name + " | size=" + $_.Length + " | mtime=" + $_.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss"))
  }
} else { Write-Output "T: NOT PRESENT" }

Sec "H. WINRE / RECOVERY STATE (read-only)"
try { $r = & reagentc /info 2>&1; foreach ($l in $r) { Write-Output ("REAGENTC " + $l) } } catch { Write-Output ("H_REAGENTC_ERR " + $_.Exception.Message) }
foreach ($p in @('C:\Windows\System32\Recovery\Winre.wim', 'C:\Recovery\WindowsRE\Winre.wim')) {
  if (Test-Path -LiteralPath $p) { ShowFile $p } else { Write-Output ("WINRE_ABSENT " + $p) }
}

Sec "I. TASKS DIR CONTENT"
$tr = 'C:\Users\Public\backupRestore-package\tasks'
if (Test-Path -LiteralPath $tr) {
  $kids = Get-ChildItem -LiteralPath $tr -Force -Recurse -Depth 2 -ErrorAction SilentlyContinue
  if (-not $kids) { Write-Output "TASKS_EMPTY" }
  foreach ($k in $kids) { Write-Output ($k.FullName + " | dir=" + $k.PSIsContainer + " | size=" + $k.Length) }
} else { Write-Output "TASKS_DIR_ABSENT" }

Sec "DONE"