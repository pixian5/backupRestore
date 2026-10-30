# NOTE: keep this file ASCII-only (PowerShell 5 reads BOM-less files as ANSI).
# G4: is the error code actually VISIBLE in the GUI status area?
#
# Why this exists: the manual promises "read the error code" (section 6), and the
# "Tasks & evidence" window is documented as showing the code on failed tasks.
# But nobody had ever measured whether the MAIN status area really renders it.
# This script clicks "Refresh task status" and reads the status control (id 1300).
#
# Output (always ends with __END__):
#   HAS_CODE=1 CODE=BR-PE-039     a failed task's code is visible  -> verdict FITS
#   HAS_CODE=0                    a failed task exists but no code -> verdict FAIL
#   HAS_CODE=NA                   no failed task to look at        -> cannot judge
param([int]$RefreshId = 1004, [int]$StatusId = 1300)
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class EC {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr e);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@
[void][EC]::SetProcessDPIAware()
$script:win = [IntPtr]::Zero
$cb = [EC+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $c = New-Object System.Text.StringBuilder 256
  [void][EC]::GetClassNameW($h, $c, 256)
  if ($c.ToString() -eq 'BackupRestoreNativeGui') { $script:win = $h; return $false }
  return $true
}
[void][EC]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:win -eq [IntPtr]::Zero) { 'HAS_CODE=NA reason=no_window'; '__END__'; exit 1 }
# click "Refresh task status" so the status area is repopulated
$btn = [EC]::GetDlgItem($script:win, $RefreshId)
if ($btn -ne [IntPtr]::Zero) {
  $r = New-Object EC+RECT
  [void][EC]::GetWindowRect($btn, [ref]$r)
  [void][EC]::SetForegroundWindow($script:win)
  Start-Sleep -Milliseconds 300
  [void][EC]::SetCursorPos([int](($r.L + $r.R) / 2), [int](($r.T + $r.B) / 2))
  Start-Sleep -Milliseconds 150
  [EC]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 80
  [EC]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 2500
}
$ctl = [EC]::GetDlgItem($script:win, $StatusId)
if ($ctl -eq [IntPtr]::Zero) { 'HAS_CODE=NA reason=no_status_control'; '__END__'; exit 1 }
$sb = New-Object System.Text.StringBuilder 4096
[void][EC]::GetWindowTextW($ctl, $sb, 4096)
$text = $sb.ToString()
if ($text.Length -eq 0) { 'HAS_CODE=NA reason=empty_status'; '__END__'; exit 0 }
# Is there a failed task at all? If the status area says success, there is nothing
# to judge -- report NA rather than a fake pass.
$failed = ($text -match 'failed') -or ($text -match '失败')
if ($text -match 'BR-[A-Z]+-\d+') {
  "HAS_CODE=1 CODE=$($matches[0]) LEN=$($text.Length)"
} elseif ($failed) {
  "HAS_CODE=0 LEN=$($text.Length)"
} else {
  "HAS_CODE=NA reason=no_failed_task LEN=$($text.Length)"
}
'__END__'
