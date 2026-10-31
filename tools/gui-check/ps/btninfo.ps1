# NOTE: keep this file ASCII-only.
# Report one child control of the main window: class, text, rect, enabled, visible.
# Used when a click "does nothing" -- a DISABLED button silently ignores BM_CLICK,
# and a zero-sized / off-screen rect makes a synthetic mouse click land nowhere.
# Output: BTN id=.. cls=.. text=[..] rect=.. en=.. vis=.. found=1|0
#         __END__
param([int]$Id = 1007, [string]$Class = 'BackupRestoreNativeGui')
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class BI {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@
[void][BI]::SetProcessDPIAware()
$script:main = [IntPtr]::Zero
$cb = [BI+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $sb = New-Object System.Text.StringBuilder 256
  [void][BI]::GetClassNameW($h, $sb, 256)
  if ($sb.ToString() -eq $Class) { $script:main = $h }   # -Class: also targets the auxiliary windows
  return $true
}
[void][BI]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:main -eq [IntPtr]::Zero) { ('BTN found=0 reason=no_window class=' + $Class); '__END__'; exit 1 }
$c = [BI]::GetDlgItem($script:main, $Id)
if ($c -eq [IntPtr]::Zero) { "BTN id=$Id found=0"; '__END__'; exit 1 }
$sb = New-Object System.Text.StringBuilder 256
[void][BI]::GetClassNameW($c, $sb, 256)
$cls = $sb.ToString()
$tb = New-Object System.Text.StringBuilder 256
[void][BI]::GetWindowTextW($c, $tb, 256)
$r = New-Object BI+RECT
[void][BI]::GetWindowRect($c, [ref]$r)
"BTN id=$Id found=1 cls=$cls text=[$($tb.ToString())] rect=$($r.L),$($r.T),$($r.R),$($r.B) en=$([BI]::IsWindowEnabled($c)) vis=$([BI]::IsWindowVisible($c)) parent_en=$([BI]::IsWindowEnabled($script:main))"
'__END__'
