# NOTE: keep this file ASCII-only.
# Generic overlap judge for the main window: enumerate every VISIBLE child control,
# then report
#   - how many controls were actually checked (not a vacuous pass)
#   - every pair whose rectangles intersect
#   - the smallest vertical gap between vertically adjacent controls
# Usage: -Class BackupRestoreNativeGui
param([string]$Class = 'BackupRestoreNativeGui')
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class OV {
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
[void][OV]::SetProcessDPIAware()
$main = [IntPtr]::Zero
$cb = [OV+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $sb = New-Object System.Text.StringBuilder 256
  [void][OV]::GetClassNameW($h, $sb, 256)
  if ($sb.ToString() -eq $Class) { $script:main = $h }
  return $true
}
[void][OV]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:main -eq [IntPtr]::Zero) { "WINDOW_NOT_FOUND class=$Class"; exit 1 }
$items = New-Object System.Collections.ArrayList
$cb2 = [OV+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  if (-not [OV]::IsWindowVisible($h)) { return $true }
  $r = New-Object OV+RECT
  [void][OV]::GetWindowRect($h, [ref]$r)
  if (($r.R - $r.L) -le 0 -or ($r.B - $r.T) -le 0) { return $true }
  $sb = New-Object System.Text.StringBuilder 256
  [void][OV]::GetClassNameW($h, $sb, 256)
  [void]$items.Add([pscustomobject]@{
    id = [OV]::GetDlgCtrlID($h); cls = $sb.ToString()
    l = $r.L; t = $r.T; r = $r.R; b = $r.B
  })
  return $true
}
[void][OV]::EnumChildWindows($script:main, $cb2, [IntPtr]::Zero)

$n = $items.Count
"CONTROLS=$n"
if ($n -lt 10) { 'TOO_FEW_CONTROLS_REFUSE_TO_PASS'; exit 2 }

$bad = 0
for ($i = 0; $i -lt $n; $i++) {
  for ($j = $i + 1; $j -lt $n; $j++) {
    $a = $items[$i]; $b = $items[$j]
    $ox = [Math]::Min($a.r, $b.r) - [Math]::Max($a.l, $b.l)
    $oy = [Math]::Min($a.b, $b.b) - [Math]::Max($a.t, $b.t)
    if ($ox -gt 0 -and $oy -gt 0) {
      $bad++
      "OVERLAP id=$($a.id)($($a.cls)) y=$($a.t)-$($a.b) x=$($a.l)-$($a.r)  vs  id=$($b.id)($($b.cls)) y=$($b.t)-$($b.b) x=$($b.l)-$($b.r)  overlap=${ox}x${oy}"
    }
  }
}
# smallest vertical gap between two rects that share horizontal space
$minGap = 999999; $minWho = ''
for ($i = 0; $i -lt $n; $i++) {
  for ($j = 0; $j -lt $n; $j++) {
    if ($i -eq $j) { continue }
    $a = $items[$i]; $b = $items[$j]
    if ($b.t -lt $a.b) { continue }
    $shareX = ([Math]::Min($a.r, $b.r) - [Math]::Max($a.l, $b.l)) -gt 0
    if (-not $shareX) { continue }
    $g = $b.t - $a.b
    if ($g -lt $minGap) { $minGap = $g; $minWho = "id=$($a.id) bottom=$($a.b) -> id=$($b.id) top=$($b.t)" }
  }
}
"OVERLAPS=$bad MIN_GAP=$minGap  ($minWho)"
'__END__'
