# NOTE: keep this file ASCII-only.
# Horizontal fit judge for the settings dialog.
# For every SINGLE-LINE static control, compare its rect width with the width of
# its text measured by an independently rebuilt font (Microsoft YaHei, weight 400,
# height = -FontSize, charset 134 -- same recipe as the app's gui_font()).
# Also reports the client width and the right-most child edge.
param([int]$FontSize = 14, [string]$Class = 'BackupRestoreSettingsWindow')
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class DF {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, Cb cb, IntPtr l);
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
  [DllImport("gdi32.dll")] public static extern IntPtr SelectObject(IntPtr dc, IntPtr o);
  [DllImport("gdi32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr CreateFontW(int h, int w, int e, int o, int weight, uint it, uint ul, uint so, uint cs, uint op, uint cp, uint q, uint pf, string face);
  [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr o);
  [DllImport("gdi32.dll")] public static extern int GetDeviceCaps(IntPtr dc, int index);
  [DllImport("gdi32.dll", CharSet=CharSet.Unicode)] public static extern bool GetTextExtentPoint32W(IntPtr dc, string s, int n, out SIZE z);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int DrawTextW(IntPtr dc, string s, int n, ref RECT r, uint f);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct SIZE { public int cx, cy; }
}
'@
[void][DF]::SetProcessDPIAware()
$dlg = [IntPtr]::Zero
$cb = [DF+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $sb = New-Object System.Text.StringBuilder 256
  [void][DF]::GetClassNameW($h, $sb, 256)
  if ($sb.ToString() -eq $Class) { $script:dlg = $h }
  return $true
}
[void][DF]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:dlg -eq [IntPtr]::Zero) { "WINDOW_NOT_FOUND class=$Class"; exit 1 }
$dr = New-Object DF+RECT
[void][DF]::GetWindowRect($script:dlg, [ref]$dr)
$cr = New-Object DF+RECT
[void][DF]::GetClientRect($script:dlg, [ref]$cr)
"DLG rect=$($dr.L),$($dr.T) w=$($dr.R-$dr.L) h=$($dr.B-$dr.T) clientW=$($cr.R) clientH=$($cr.B)"

$screen = [DF]::GetDC([IntPtr]::Zero)
$dpi = [DF]::GetDeviceCaps($screen, 90)
$scale = $dpi / 96.0
$px = [int][math]::Round($FontSize * $scale)
$font = [DF]::CreateFontW(-$px, 0, 0, 0, 400, 0, 0, 0, 134, 0, 0, 0, 0, 'Microsoft YaHei')
$old = [DF]::SelectObject($screen, $font)
$linePx = [int][math]::Round(($FontSize + [math]::Floor(($FontSize + 5) / 6)) * $scale)
"DPI=$dpi SCALE=$scale FONT_PX=$px LINE_PX=$linePx"

$script:worst = -999
$bad = 0
$rows = New-Object System.Collections.ArrayList
$cb2 = [DF+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $sb = New-Object System.Text.StringBuilder 256
  [void][DF]::GetClassNameW($h, $sb, 256)
  # 2.6.86: also judge BUTTON labels -- a button whose text does not fit is the
  # same defect; before this only Static was checked.
  $cls = $sb.ToString()
  if ($cls -ne 'Static' -and $cls -ne 'Button') { return $true }
  $tb = New-Object System.Text.StringBuilder 1024
  [void][DF]::GetWindowTextW($h, $tb, 1024)
  $text = $tb.ToString()
  if ($text.Length -eq 0) { return $true }
  $r = New-Object DF+RECT
  [void][DF]::GetWindowRect($h, [ref]$r)
  $w = $r.R - $r.L
  $hgt = $r.B - $r.T
  if ($hgt -gt [int]($linePx * 1.6)) {
    # Multi-line Static: judge the HEIGHT instead of the width (this is the 2.6.83
    # defect class -- a wrapped block whose control is too short and eats the last
    # line). Buttons are never wrapped, so they always take the width path.
    if ($cls -ne 'Static') {
      $sz2 = New-Object DF+SIZE
      [void][DF]::GetTextExtentPoint32W($screen, $text, $text.Length, [ref]$sz2)
      $slack2 = $w - $sz2.cx
      if ($slack2 -lt $script:worst) { $script:worst = $slack2 }
      $v2 = 'FITS'; if ($slack2 -lt 2) { $v2 = 'CLIPPED'; $script:bad++ }
      [void]$rows.Add(("w={0,4} text={1,4} slack={2,4} {3} relX={4,5} h={5,3} cls={7} text=[{6}]" -f $w, $sz2.cx, $slack2, $v2, ($r.L - $dr.L), $hgt, $text, $cls))
      return $true
    }
    $need = New-Object DF+RECT
    $need.L = 0; $need.T = 0; $need.R = $w; $need.B = 0
    [void][DF]::DrawTextW($screen, $text, -1, [ref]$need, 0x0400 -bor 0x0010 -bor 0x0800)
    $needH = $need.B - $need.T
    $slackV = $hgt - $needH
    if ($slackV -lt $script:worst) { $script:worst = $slackV }
    $v3 = 'FITS'; if ($slackV -lt 0) { $v3 = 'CUT'; $script:bad++ }
    [void]$rows.Add(("MULTILINE h={0,4} need={1,4} slack={2,4} {3} relX={4,5} w={5,4} cls={7} text=[{6}]" -f $hgt, $needH, $slackV, $v3, ($r.L - $dr.L), $w, $text, $cls))
    return $true
  }
  $sz = New-Object DF+SIZE
  [void][DF]::GetTextExtentPoint32W($screen, $text, $text.Length, [ref]$sz)
  $slack = $w - $sz.cx
  if ($slack -lt $script:worst) { $script:worst = $slack }
  $verdict = 'FITS'
  if ($slack -lt 2) { $verdict = 'CLIPPED'; $script:bad++ }
  [void]$rows.Add(("w={0,4} text={1,4} slack={2,4} {3} relX={4,5} h={5,3} cls={7} text=[{6}]" -f $w, $sz.cx, $slack, $verdict, ($r.L - $dr.L), $hgt, $text, $cls))
  return $true
}
[void][DF]::EnumChildWindows($script:dlg, $cb2, [IntPtr]::Zero)
$rows | ForEach-Object { $_ }
[void][DF]::SelectObject($screen, $old)
[void][DF]::DeleteObject($font)
[void][DF]::ReleaseDC([IntPtr]::Zero, $screen)
"SINGLE_LINE_CLIPPED=$bad WORST_SLACK=$script:worst"
'__END__'
