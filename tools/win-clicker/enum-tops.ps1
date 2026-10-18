# enum-tops.ps1 - dump all top-level windows: hwnd, visibility, class, text.
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class W6 {
  public delegate bool ECb(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(ECb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
}
"@
$tops = New-Object System.Collections.ArrayList
$cb = [W6+ECb]{ param($h,$l) [void]$tops.Add($h); return $true }
[void][W6]::EnumWindows($cb, [IntPtr]::Zero)
foreach ($h in $tops) {
  $s = New-Object Text.StringBuilder 256
  [void][W6]::GetWindowTextW($h, $s, 256)
  $c = New-Object Text.StringBuilder 256
  [void][W6]::GetClassNameW($h, $c, 256)
  $v = [W6]::IsWindowVisible($h)
  $c1 = [W6]::GetDlgItem($h, 1)
  $c1410 = [W6]::GetDlgItem($h, 1410)
  $extra = ""
  if ($c1 -ne [IntPtr]::Zero) { $extra += " HAS_ID1" }
  if ($c1410 -ne [IntPtr]::Zero) { $extra += " HAS_ID1410" }
  Write-Output ($h.ToInt64().ToString() + " vis=" + $v + " cls=" + $c.ToString() + " txt=[" + $s.ToString() + "]" + $extra)
}
Write-Output "ENUM_DONE"
