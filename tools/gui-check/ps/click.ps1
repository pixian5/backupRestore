# NOTE: keep this file ASCII-only.
# Click the centre of a control by (window class, control id).
# Real mouse click (not BM_CLICK) so it exercises the same path a user does.
# Usage: -Class BackupRestoreNativeGui -Id 1104
param(
  [string]$Class = 'BackupRestoreNativeGui',
  [int]$Id = 0
)
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class GCLK {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr e);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@
[void][GCLK]::SetProcessDPIAware()
$script:win = [IntPtr]::Zero
$cb = [GCLK+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $c = New-Object System.Text.StringBuilder 256
  [void][GCLK]::GetClassNameW($h, $c, 256)
  if ($c.ToString() -eq $Class) { $script:win = $h; return $false }
  return $true
}
[void][GCLK]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:win -eq [IntPtr]::Zero) { "CLICK=NO_WINDOW class=$Class"; '__END__'; exit 1 }
$ctl = [GCLK]::GetDlgItem($script:win, $Id)
if ($ctl -eq [IntPtr]::Zero) { "CLICK=NO_CONTROL id=$Id"; '__END__'; exit 1 }
if (-not [GCLK]::IsWindowVisible($ctl)) { "CLICK=HIDDEN id=$Id"; '__END__'; exit 2 }
$r = New-Object GCLK+RECT
[void][GCLK]::GetWindowRect($ctl, [ref]$r)
$x = [int](($r.L + $r.R) / 2); $y = [int](($r.T + $r.B) / 2)
# The coordinates MUST be sanity-checked. 2026-10-30: the type name in
# New-Object was not renamed along with the class (RECT still resolved to
# System.GC), so $r failed to construct and all four fields were 0 -- the click
# landed at screen (0,0) while the script still reported CLICK=OK, and not a
# single tab actually switched. Without this check the verdict keeps lying.
# (Also: this file must stay ASCII-only -- PowerShell 5 reads BOM-less files as
# ANSI, and non-ASCII bytes in a script can break parsing outright.)
if ($r.R -le $r.L -or $r.B -le $r.T) {
  "CLICK=BAD_RECT id=$Id rect=$($r.L),$($r.T),$($r.R),$($r.B)"
  '__END__'
  exit 3
}
[void][GCLK]::SetForegroundWindow($script:win)
Start-Sleep -Milliseconds 400
[void][GCLK]::SetCursorPos($x, $y)
Start-Sleep -Milliseconds 200
[GCLK]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 80
[GCLK]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 700
"CLICK=OK id=$Id at=$x,$y"
'__END__'
