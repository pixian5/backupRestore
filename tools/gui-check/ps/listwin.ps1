# NOTE: keep this file ASCII-only.
# Diagnostic: list every top-level window whose class starts with "BackupRestore",
# with its rect and enabled state. Used to prove WHICH window a judge measured.
# Output: WINDOW class=... rect=L,T,R,B vis=.. en=.. title=[..]
#         __END__
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class LW {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@
[void][LW]::SetProcessDPIAware()
# ★ Inside a scriptblock invoked as a native delegate, a bare string is NOT
#   written to the pipeline -- it is silently discarded. Collect first, print
#   after EnumWindows returns. (First version printed straight from the callback
#   and reported "no windows at all" while the app was plainly alive.)
$rows = New-Object System.Collections.ArrayList
$cb = [LW+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $sb = New-Object System.Text.StringBuilder 256
  [void][LW]::GetClassNameW($h, $sb, 256)
  $c = $sb.ToString()
  if ($c -like 'BackupRestore*') {
    $tb = New-Object System.Text.StringBuilder 256
    [void][LW]::GetWindowTextW($h, $tb, 256)
    $r = New-Object LW+RECT
    [void][LW]::GetWindowRect($h, [ref]$r)
    [void]$rows.Add("WINDOW class=$c rect=$($r.L),$($r.T),$($r.R),$($r.B) vis=$([LW]::IsWindowVisible($h)) en=$([LW]::IsWindowEnabled($h)) title=[$($tb.ToString())]")
  }
  return $true
}
[void][LW]::EnumWindows($cb, [IntPtr]::Zero)
"WINDOW_COUNT=$($rows.Count)"
$rows | ForEach-Object { $_ }
'__END__'
