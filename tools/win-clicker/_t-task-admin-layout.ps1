# _t-task-admin-layout.ps1 - open the "Tasks & evidence" window, run the SAME five
# geometry predicates against it, then close it.
#
# Why a driver: the geometry script (/_t-layout-rects.ps1) inspects a window that is
# already on screen. This one clicks ID_REFRESH_TASK (1004) to raise the modal window,
# hands over, and closes it afterwards - a modal window left open looks like a hang.
$out = 'H:\brwork\task-admin-layout-driver.txt'
function Say($s) { $s | Add-Content -Path $out -Encoding UTF8 }
Set-Content -Path $out -Value ("STARTED " + (Get-Date -Format o)) -Encoding UTF8
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class RS3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, [ref] RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, [ref] RECT r);
  [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr h, [ref] POINT p);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
'@
function Find-Win($cls) {
  $script:hit = [IntPtr]::Zero
  $cb = [RS3+EnumCb]{ param($h,$l) if ([RS3]::Cls($h) -eq $cls) { $script:hit = $h; return $false }; return $true }
  [void][RS3]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:hit
}
$gui = Find-Win 'BackupRestoreNativeGui'
if ($gui -eq [IntPtr]::Zero) { Say "GUI_NOT_FOUND"; Say "DONE"; exit 1 }
$btn = [RS3]::GetDlgItem($gui, 1004)
if ($btn -eq [IntPtr]::Zero) { Say "REFRESH_BUTTON_NULL"; Say "DONE"; exit 2 }
[void][RS3]::PostMessageW($btn, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)   # BM_CLICK
Start-Sleep -Milliseconds 2000
$win = Find-Win 'BackupRestoreTaskAdmin'
Say ("WINDOW=" + [int]($win -ne [IntPtr]::Zero))
if ($win -eq [IntPtr]::Zero) { Say "DONE"; exit 3 }
# 内联几何判据（OVERLAP / OUTSIDE / ZERO_SIZE）。不跨脚本调用：那条路试过一次
# 没有任何输出，而这几条判据本身只有二十行，自己读自己算最可靠。
$script:rects = New-Object System.Collections.ArrayList
$cbChild = [RS3+EnumCb]{ param($h,$l)
  if ([RS3]::GetParent($h) -eq $win -and ([RS3]::IsWindowVisible($h))) {
    $r = New-Object RS3+RECT
    [void][RS3]::GetWindowRect($h, [ref]$r)
    $pt = New-Object RS3+POINT
    $pt.X = $r.Left; $pt.Y = $r.Top
    [void][RS3]::ScreenToClient($win, [ref]$pt)
    $pt2 = New-Object RS3+POINT
    $pt2.X = $r.Right; $pt2.Y = $r.Bottom
    [void][RS3]::ScreenToClient($win, [ref]$pt2)
    [void]$script:rects.Add([pscustomobject]@{
      Id = [RS3]::GetDlgCtrlID($h); X = $pt.X; Y = $pt.Y
      W = ($pt2.X - $pt.X); H = ($pt2.Y - $pt.Y)
    })
  }
  return $true }
[void][RS3]::EnumChildWindows($win, $cbChild, [IntPtr]::Zero)
$cw = 0; $ch = 0
$cr = New-Object RS3+RECT
[void][RS3]::GetClientRect($win, [ref]$cr)
$cw = $cr.Right; $ch = $cr.Bottom
Say ("CLIENT_W=" + $cw + " CLIENT_H=" + $ch + " CHILDREN=" + $script:rects.Count)
foreach ($r in $script:rects) { Say ("  RECT id=" + $r.Id + " x=" + $r.X + " y=" + $r.Y + " w=" + $r.W + " h=" + $r.H) }
$overlap = 0; $outside = 0; $zero = 0
for ($i = 0; $i -lt $script:rects.Count; $i++) {
  $a = $script:rects[$i]
  if ($a.W -le 0 -or $a.H -le 0) { $zero++ }
  if ($a.X -lt 0 -or $a.Y -lt 0 -or ($a.X + $a.W) -gt $cw -or ($a.Y + $a.H) -gt $ch) {
    $outside++
    Say ("  OUTSIDE id=" + $a.Id + " x=" + $a.X + " y=" + $a.Y + " w=" + $a.W + " h=" + $a.H)
  }
  for ($j = $i + 1; $j -lt $script:rects.Count; $j++) {
    $b = $script:rects[$j]
    $ox = [Math]::Min($a.X + $a.W, $b.X + $b.W) - [Math]::Max($a.X, $b.X)
    $oy = [Math]::Min($a.Y + $a.H, $b.Y + $b.H) - [Math]::Max($a.Y, $b.Y)
    if ($ox -gt 0 -and $oy -gt 0) { $overlap++; Say ("  OVERLAP id=" + $a.Id + " x id=" + $b.Id + " = " + $ox + "x" + $oy) }
  }
}
Say ("OVERLAP_COUNT=" + $overlap)
Say ("OUTSIDE_COUNT=" + $outside)
Say ("ZERO_SIZE=" + $zero)
# Close it: a modal window left up blocks the whole app for the user.
[void][RS3]::PostMessageW($win, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)   # WM_CLOSE
for ($i = 0; $i -lt 40; $i++) {
  Start-Sleep -Milliseconds 100
  if ((Find-Win 'BackupRestoreTaskAdmin') -eq [IntPtr]::Zero) { break }
}
Say ("CLOSED=" + [int]((Find-Win 'BackupRestoreTaskAdmin') -eq [IntPtr]::Zero))
Say "DONE"
