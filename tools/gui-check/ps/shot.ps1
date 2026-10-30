# NOTE: keep this file ASCII-only.
# Screenshot the app window (screen copy, not PrintWindow -- PrintWindow is
# unreliable for a window that has never been composited on this desktop).
# Saved to H:\brwork\shot.png; the CALLER copies it to the mac share, because an
# elevated process has a split token and typically cannot see the UNC share
# credentials of the interactive logon.
param([string]$Out = 'H:\brwork\shot.png')
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class SH {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int n);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@
[void][SH]::SetProcessDPIAware()
Add-Type -AssemblyName System.Drawing
$script:win = [IntPtr]::Zero
$cb = [SH+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $c = New-Object System.Text.StringBuilder 256
  [void][SH]::GetClassNameW($h, $c, 256)
  if ($c.ToString() -eq 'BackupRestoreNativeGui') { $script:win = $h; return $false }
  return $true
}
[void][SH]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:win -eq [IntPtr]::Zero) { 'SHOT=NO_WINDOW'; '__END__'; exit 1 }
[void][SH]::ShowWindow($script:win, 9)   # SW_RESTORE (in case it is minimised)
[void][SH]::SetForegroundWindow($script:win)
Start-Sleep -Milliseconds 1200
$r = New-Object SH+RECT
[void][SH]::GetWindowRect($script:win, [ref]$r)
$sw = [SH]::GetSystemMetrics(0)
$sh = [SH]::GetSystemMetrics(1)
# capture the WHOLE primary screen: the point is to see what a human sees,
# including any modal window sitting on top of the app.
$x = 0; $y = 0
$w = $sw
$h2 = $sh
"WIN_RECT=$($r.L),$($r.T),$($r.R),$($r.B)" | Out-Null
if ($w -le 0 -or $h2 -le 0) { "SHOT=BAD_RECT $($r.L),$($r.T),$($r.R),$($r.B)"; '__END__'; exit 1 }
$bmp = New-Object System.Drawing.Bitmap $w, $h2
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
"SHOT=OK rect=$x,$y,$w,$h2 out=$Out"
'__END__'
