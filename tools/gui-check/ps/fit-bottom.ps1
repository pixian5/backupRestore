# NOTE: keep this file ASCII-only.
# Are any visible controls pushed below the main window's bottom edge?
# Reports the window rect, the lowest visible child, and the remaining margin.
param([string]$Class = 'BackupRestoreNativeGui')
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class FB {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, Cb cb, IntPtr l);
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@
[void][FB]::SetProcessDPIAware()
$script:main = [IntPtr]::Zero
$cb = [FB+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $sb = New-Object System.Text.StringBuilder 256
  [void][FB]::GetClassNameW($h, $sb, 256)
  if ($sb.ToString() -eq $Class) { $script:main = $h }
  return $true
}
[void][FB]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:main -eq [IntPtr]::Zero) { "WINDOW_NOT_FOUND class=$Class"; exit 1 }
$wr = New-Object FB+RECT
[void][FB]::GetWindowRect($script:main, [ref]$wr)
$maxB = 0; $maxId = 0
$cb2 = [FB+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  if (-not [FB]::IsWindowVisible($h)) { return $true }
  $r = New-Object FB+RECT
  [void][FB]::GetWindowRect($h, [ref]$r)
  if (($r.R - $r.L) -le 0 -or ($r.B - $r.T) -le 0) { return $true }
  if ($r.B -gt $script:maxB) { $script:maxB = $r.B; $script:maxId = [FB]::GetDlgCtrlID($h) }
  return $true
}
[void][FB]::EnumChildWindows($script:main, $cb2, [IntPtr]::Zero)
$margin = $wr.B - $maxB
$verdict = if ($margin -ge 0) { 'FITS' } else { 'BELOW_WINDOW' }
"WIN bottom=$($wr.B) | LOWEST id=$($script:maxId) bottom=$($script:maxB) | MARGIN=$margin $verdict"
'__END__'
