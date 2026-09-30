# re-gui-run.ps1 - drive the real GUI through the "system volume must go offline" path:
#   operation=backup, source=%SystemDrive%, then the offline-environment dialog ->
#   "Enter Windows RE" -> confirm Yes -> prepare -> real reboot into the task RE.
# Everything goes through real window messages (WM_COMMAND / BM_CLICK / WM_CHAR), never
# a --test-hook, so the dialog chain itself is under test.
# THIS FILE MUST STAY PURE ASCII: powershell -File decodes as GBK and non-ASCII breaks parsing.
param(
  [string]$Exe       = "H:\brwork\BackupRestore.exe",
  [string]$ImagePath = "F:\6.wim",
  [string]$IndexName = "c-backup-6",
  [string]$SourceLetter = "C",
  [int]$StopBefore   = 0,          # 1 = stop right before the confirm dialog (no reboot)
  [string]$Log       = "H:\brwork\logs\re-gui-run.log"
)

$ErrorActionPreference = "Continue"
New-Item -ItemType Directory -Force -Path (Split-Path $Log) | Out-Null
function L($s) {
  $line = ((Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ") + "  " + $s)
  Add-Content -LiteralPath $Log -Value $line
  Write-Output $line
}

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrUi {
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="GetWindowTextW")] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="GetClassNameW")] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr h, out int pid);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Text(IntPtr h) { if (h == IntPtr.Zero) return ""; StringBuilder sb = new StringBuilder(4096); GetWindowTextW(h, sb, 4096); return sb.ToString(); }
  public static string Cls(IntPtr h) { if (h == IntPtr.Zero) return ""; StringBuilder sb = new StringBuilder(512); GetClassNameW(h, sb, 512); return sb.ToString(); }
  public static System.Collections.Generic.List<IntPtr> All() {
    var r = new System.Collections.Generic.List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr l) { r.Add(h); return true; }, IntPtr.Zero);
    return r;
  }
}
"@

$WM_COMMAND = 0x0111
$WM_CHAR    = 0x0102
$BM_CLICK    = 0x00F5
$EM_SETSEL   = 0x00B1
$CB_GETCOUNT = 0x0146
$CB_SETCURSEL= 0x014E
$CB_GETCURSEL= 0x0147
$WM_GETTEXTLENGTH = 0x000E

$ID_CREATE_TASK      = 1003
$ID_OPERATION_BACKUP = 1101
$ID_SOURCE           = 1202
$ID_SOURCE_DETAILS   = 2012
$ID_IMAGE            = 1203
$ID_KEEP_EDIT        = 1210
$ID_CHOICE_RE        = 2002
$IDYES               = 6

function FindWindowByClass([string]$cls, [int]$timeoutMs) {
  $deadline = (Get-Date).AddMilliseconds($timeoutMs)
  while ((Get-Date) -lt $deadline) {
    foreach ($h in [BrUi]::All()) {
      if ([BrUi]::Cls($h) -eq $cls -and [BrUi]::IsWindowVisible($h)) { return $h }
    }
    Start-Sleep -Milliseconds 200
  }
  return [IntPtr]::Zero
}

# A MessageBoxW popup is class #32770. The caption is localized, so match structurally
# instead: same owning process as the GUI and an actual IDYES button present. That
# cannot pick up an unrelated dialog and keeps this file pure ASCII.
function FindConfirmDialog([int]$ownerPid, [int]$timeoutMs) {
  $deadline = (Get-Date).AddMilliseconds($timeoutMs)
  while ((Get-Date) -lt $deadline) {
    foreach ($h in [BrUi]::All()) {
      if ([BrUi]::Cls($h) -ne "#32770") { continue }
      if (-not [BrUi]::IsWindowVisible($h)) { continue }
      $owner = 0
      [void][BrUi]::GetWindowThreadProcessId($h, [ref]$owner)
      if ($owner -ne $ownerPid) { continue }
      if ([BrUi]::GetDlgItem($h, 6) -eq [IntPtr]::Zero) { continue }
      return $h
    }
    Start-Sleep -Milliseconds 200
  }
  return [IntPtr]::Zero
}

L "=== re-gui-run start ==="
L ("whoami=" + (whoami) + " exe=" + $Exe + " image=" + $ImagePath + " stopBefore=" + $StopBefore)

taskkill /f /im BackupRestore.exe 2>$null | Out-Null
Start-Sleep -Milliseconds 800

$proc = Start-Process -FilePath $Exe -PassThru
L ("GUI started pid=" + $proc.Id)

$gui = FindWindowByClass "BackupRestoreNativeGui" 30000
if ($gui -eq [IntPtr]::Zero) { L "FAIL: main window never appeared"; exit 10 }
L ("main window hwnd=" + $gui + " title=[" + [BrUi]::Text($gui) + "]")
[void][BrUi]::SetForegroundWindow($gui)
Start-Sleep -Milliseconds 600

# 1) operation = backup
[void][BrUi]::PostMessageW($gui, $WM_COMMAND, [IntPtr]$ID_OPERATION_BACKUP, [IntPtr]::Zero)
Start-Sleep -Milliseconds 1200
L "operation=backup posted"

# 2) source volume. A drop-down-list ComboBox owned by another process has no internal
# caption, so GetWindowTextW returns "" across the process boundary and the item text
# cannot be read from here (CB_GETLBTEXT would have to marshal a buffer). CB_SETCURSEL
# carries only an integer and does cross, and the GUI maps the selected index straight
# into its own drives vector -- the very list "list-volumes" prints. So resolve the
# index from that JSON and then verify the pick through the details pane, which is a
# Static and therefore does read back cross-process.
$srcCombo = [BrUi]::GetDlgItem($gui, $ID_SOURCE)
if ($srcCombo -eq [IntPtr]::Zero) { L "FAIL: source combo not found"; exit 11 }
$count = [int][BrUi]::SendMessageW($srcCombo, $CB_GETCOUNT, [IntPtr]::Zero, [IntPtr]::Zero)
# BackupRestore.exe is a /SUBSYSTEM:WINDOWS binary, so its stdout goes nowhere when a
# non-console parent invokes it directly (verified: the pipeline came back empty). Route
# it through cmd with a file redirection so the child inherits a real file handle.
$volFile = Join-Path $env:TEMP "br-list-volumes.json"
Remove-Item -LiteralPath $volFile -ErrorAction SilentlyContinue
& cmd.exe /d /c "`"$Exe`" list-volumes > `"$volFile`"" | Out-Null
$volJson = ""
if (Test-Path -LiteralPath $volFile) { $volJson = (Get-Content -Raw -LiteralPath $volFile) }
L ("list-volumes bytes=" + $volJson.Length)
$letters = @()
foreach ($v in ($volJson | ConvertFrom-Json)) { $letters += $v.letter }
L ("source combo items=" + $count + " list-volumes order=" + ($letters -join ","))
if ($letters.Count -ne $count) { L ("WARN: combo item count " + $count + " differs from list-volumes count " + $letters.Count) }
$picked = [Array]::IndexOf($letters, $SourceLetter)
if ($picked -lt 0) { L ("FAIL: list-volumes has no " + $SourceLetter); exit 12 }
[void][BrUi]::SendMessageW($srcCombo, $CB_SETCURSEL, [IntPtr]$picked, [IntPtr]::Zero)
# Tell the parent the selection changed (HIWORD=CBN_SELCHANGE=1) so the details pane
# and the GUI's own bookkeeping follow along, exactly as a real click would.
$wp = [IntPtr](([int]1 -shl 16) -bor $ID_SOURCE)
[void][BrUi]::SendMessageW($gui, $WM_COMMAND, $wp, $srcCombo)
Start-Sleep -Milliseconds 1000
$cur = [int][BrUi]::SendMessageW($srcCombo, $CB_GETCURSEL, [IntPtr]::Zero, [IntPtr]::Zero)
$details = [BrUi]::Text([BrUi]::GetDlgItem($gui, $ID_SOURCE_DETAILS))
L ("source selected index=" + $picked + " cur=" + $cur + " details=[" + ($details -replace "`r`n", " / ") + "]")
if ($cur -ne $picked) { L "FAIL: CB_SETCURSEL did not stick"; exit 12 }
if ($details.Length -gt 0 -and $details.IndexOf($SourceLetter + ":", [StringComparison]::OrdinalIgnoreCase) -lt 0) {
  L ("FAIL: details pane does not mention " + $SourceLetter + ":")
  exit 12
}

# 3) image path + index name + keep: WM_CHAR one char at a time (cross-process
# WM_SETTEXT does not take on this GUI; EM_SETSEL first so typing replaces the value).
# WM_GETTEXTLENGTH returns an integer, so unlike WM_GETTEXT it is usable across the
# process boundary: it proves every character we typed landed even when the control
# text itself cannot be read back from here.
function TypeInto([int]$id, [string]$value) {
  $ctl = [BrUi]::GetDlgItem($gui, $id)
  if ($ctl -eq [IntPtr]::Zero) { L ("FAIL: control " + $id + " not found"); return $false }
  [void][BrUi]::SendMessageW($ctl, $EM_SETSEL, [IntPtr]0, [IntPtr](-1))
  foreach ($ch in $value.ToCharArray()) {
    [void][BrUi]::SendMessageW($ctl, $WM_CHAR, [IntPtr][int]$ch, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 8
  }
  $len = [int][BrUi]::SendMessageW($ctl, $WM_GETTEXTLENGTH, [IntPtr]::Zero, [IntPtr]::Zero)
  $back = [BrUi]::Text($ctl)
  L ("  typed id=" + $id + " len=" + $len + " want=" + $value.Length + " readback=[" + $back + "]")
  return ($len -eq $value.Length)
}
if (-not (TypeInto $ID_IMAGE $ImagePath)) { L "FAIL: image path length mismatch"; exit 13 }
# keep=0 disables index pruning, so index 1 (the earlier full C: capture) survives
# and the new capture is appended as a second index.
if (-not (TypeInto $ID_KEEP_EDIT "0")) { L "WARN: keep length mismatch" }

if ($StopBefore -eq 1) { L "STOP_BEFORE_CREATE requested; leaving GUI up"; exit 0 }

# 4) create task -> the offline-environment dialog must appear
[void][BrUi]::PostMessageW($gui, $WM_COMMAND, [IntPtr]$ID_CREATE_TASK, [IntPtr]::Zero)
L "create-task posted"

$choice = FindWindowByClass "BackupRestoreSystemDriveChoice" 30000
if ($choice -eq [IntPtr]::Zero) { L "FAIL: offline-environment dialog never appeared"; exit 14 }
L ("choice dialog hwnd=" + $choice + " title=[" + [BrUi]::Text($choice) + "]")
$reBtn = [BrUi]::GetDlgItem($choice, $ID_CHOICE_RE)
L ("RE button hwnd=" + $reBtn + " text=[" + [BrUi]::Text($reBtn) + "]")
Start-Sleep -Milliseconds 800
# BM_CLICK on the real button, the same message a mouse click produces.
[void][BrUi]::SendMessageW($reBtn, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
L "RE button clicked"

# 5) confirm dialog -> Yes. After this the machine really reboots.
$confirm = FindConfirmDialog $proc.Id 20000
if ($confirm -eq [IntPtr]::Zero) { L "FAIL: confirm dialog never appeared"; exit 15 }
$yes = [BrUi]::GetDlgItem($confirm, $IDYES)
L ("confirm dialog hwnd=" + $confirm + " title=[" + [BrUi]::Text($confirm) + "] yesBtn=" + $yes + " yesText=[" + [BrUi]::Text($yes) + "]")
Start-Sleep -Milliseconds 800
[void][BrUi]::PostMessageW($confirm, $WM_COMMAND, [IntPtr]$IDYES, [IntPtr]::Zero)
L "confirm YES posted -> prepare chain running, reboot expected"

# 6) watch prepare for as long as we live; the reboot will cut this short.
for ($i = 0; $i -lt 120; $i++) {
  Start-Sleep -Seconds 5
  $alive = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
  $wins = @()
  foreach ($h in [BrUi]::All()) {
    if (-not [BrUi]::IsWindowVisible($h)) { continue }
    $t = [BrUi]::Text($h)
    if ($t.Length -eq 0) { continue }
    $c = [BrUi]::Cls($h)
    if ($c -like "BackupRestore*" -or $c -eq "#32770" -or $t -like "*repare*") { $wins += ($c + "|" + $t) }
  }
  L ("watch " + $i + " guiAlive=" + [bool]$alive + " windows=" + ($wins -join " ;; "))
}
L "=== re-gui-run end ==="
