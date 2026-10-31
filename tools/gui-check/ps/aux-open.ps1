# NOTE: keep this file ASCII-only.
# Open a MODAL auxiliary window and wait until it really exists.
#
# Why this exists: every auxiliary window (settings / image admin / log viewer /
# task admin) is modal -- the handler calls EnableWindow(main, 0), pumps its own
# message loop, and only returns when the dialog closes. So the main window is
# DISABLED while one is open: any BM_CLICK posted to a main-window button is
# silently dropped (a disabled parent disables its children). Consequence: you
# must close the previous dialog before opening the next one, and you cannot
# "just click a tab" to measure the main window in between.
#
# Output:
#   MAIN_ENABLED=True|False        state before we touch anything
#   STALE_CLOSED=<class>           a leftover dialog we closed first
#   OPENED=1|0 class=... title=[...] clientW=... clientH=...
#   __END__
param(
  [int]$Id = 1007,
  [string]$Class = 'BackupRestoreSettingsWindow',
  [int]$WaitMs = 12000
)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class AX {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@
[void][AX]::SetProcessDPIAware()

function Find-Class([string]$want) {
  $script:found = [IntPtr]::Zero
  $cb = [AX+Cb]{
    param([IntPtr]$h,[IntPtr]$l)
    $sb = New-Object System.Text.StringBuilder 256
    [void][AX]::GetClassNameW($h, $sb, 256)
    if ($sb.ToString() -eq $want -and [AX]::IsWindowVisible($h)) { $script:found = $h }
    return $true
  }
  [void][AX]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

# Close every stale modal first (a previous run may have died while one was open).
foreach ($c in @('BackupRestoreSettingsWindow','BackupRestoreImageAdminWindow','BackupRestoreLogsWindow','BackupRestoreTaskAdmin')) {
  $h = Find-Class $c
  if ($h -ne [IntPtr]::Zero) {
    [void][AX]::PostMessageW($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)  # WM_CLOSE
    "STALE_CLOSED=$c"
    $deadline = [DateTime]::Now.AddMilliseconds(6000)
    while ([DateTime]::Now -lt $deadline -and [AX]::IsWindow($h) -ne 0) { Start-Sleep -Milliseconds 150 }
  }
}

$main = Find-Class 'BackupRestoreNativeGui'
if ($main -eq [IntPtr]::Zero) { 'OPENED=0 reason=no_main_window'; '__END__'; exit 1 }
# ★ Report the integrity level. A non-elevated run cannot post a click into the
#   elevated app (UIPI drops it), and the ONLY symptom is "the dialog never
#   appears" -- indistinguishable from a product defect. Measured 2026-10-31:
#   same script, Medium -> POST_OK=False, High -> POST_OK=True.
$lvl = 'unknown'
foreach ($line in (& whoami /groups 2>&1)) {
  if ($line -match 'Mandatory Label') {
    if ($line -match 'System') { $lvl = 'System' }
    elseif ($line -match 'High') { $lvl = 'High' }
    elseif ($line -match 'Medium') { $lvl = 'Medium' }
    elseif ($line -match 'Low') { $lvl = 'Low' }
  }
}
"INTEGRITY=$lvl ELEVATED=$([bool]($lvl -eq 'High' -or $lvl -eq 'System'))"
"MAIN_ENABLED=$([AX]::IsWindowEnabled($main))"

$btn = [AX]::GetDlgItem($main, $Id)
if ($btn -eq [IntPtr]::Zero) { "OPENED=0 reason=no_button id=$Id"; '__END__'; exit 1 }

# PostMessage (not SendMessage): the click starts a NESTED modal loop inside the
# app; SendMessage would block us for the whole lifetime of the dialog.
$posted = [AX]::PostMessageW($btn, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)   # BM_CLICK
"POST_OK=$posted"
if (-not $posted) { "OPENED=0 class=$Class reason=post_rejected"; '__END__'; exit 1 }

$deadline = [DateTime]::Now.AddMilliseconds($WaitMs)
$dlg = [IntPtr]::Zero
while ([DateTime]::Now -lt $deadline) {
  $dlg = Find-Class $Class
  if ($dlg -ne [IntPtr]::Zero) { break }
  Start-Sleep -Milliseconds 200
}
if ($dlg -eq [IntPtr]::Zero) {
  "OPENED=0 class=$Class reason=timeout main_enabled=$([AX]::IsWindowEnabled($main))"
  '__END__'
  exit 1
}
$tb = New-Object System.Text.StringBuilder 256
[void][AX]::GetWindowTextW($dlg, $tb, 256)
$cr = New-Object AX+RECT
[void][AX]::GetClientRect($dlg, [ref]$cr)
"OPENED=1 class=$Class title=[$($tb.ToString())] clientW=$($cr.R) clientH=$($cr.B) main_enabled=$([AX]::IsWindowEnabled($main))"
'__END__'
