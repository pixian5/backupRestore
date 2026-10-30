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
public class GC {
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
[void][GC]::SetProcessDPIAware()
$script:win = [IntPtr]::Zero
$cb = [GC+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $c = New-Object System.Text.StringBuilder 256
  [void][GC]::GetClassNameW($h, $c, 256)
  if ($c.ToString() -eq $Class) { $script:win = $h; return $false }
  return $true
}
[void][GC]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:win -eq [IntPtr]::Zero) { "CLICK=NO_WINDOW class=$Class"; '__END__'; exit 1 }
$ctl = [GC]::GetDlgItem($script:win, $Id)
if ($ctl -eq [IntPtr]::Zero) { "CLICK=NO_CONTROL id=$Id"; '__END__'; exit 1 }
if (-not [GC]::IsWindowVisible($ctl)) { "CLICK=HIDDEN id=$Id"; '__END__'; exit 2 }
$r = New-Object GC+RECT
[void][GC]::GetWindowRect($ctl, [ref]$r)
$x = [int](($r.L + $r.R) / 2); $y = [int](($r.T + $r.B) / 2)
[void][GC]::SetForegroundWindow($script:win)
Start-Sleep -Milliseconds 400
[void][GC]::SetCursorPos($x, $y)
Start-Sleep -Milliseconds 200
[GC]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 80
[GC]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 700
"CLICK=OK id=$Id at=$x,$y"
'__END__'
