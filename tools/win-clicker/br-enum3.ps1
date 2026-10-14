# enum3.ps1 - dump child controls of BackupRestore window (probe-pattern, no pipe, no early return).
# Read-only. ASCII-only body. Output: C:\Users\Public\pkg\enum3.txt
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public class BrEnum3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(2048); GetWindowTextW(h, sb, 2048); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
"@
$script:tops = New-Object System.Collections.ArrayList
$cbTop = [BrEnum3+EnumCb]{
  param($h, $l)
  [void]$script:tops.Add($h)
  return $true
}
$r1 = [BrEnum3]::EnumWindows($cbTop, [IntPtr]::Zero)
$root = [IntPtr]::Zero
foreach ($h in $script:tops) {
  if ([BrEnum3]::Cls($h) -eq "BackupRestoreNativeGui") { $root = $h }
}
Write-Output ("tops=" + $script:tops.Count + " root=" + $root)
if ($root -eq [IntPtr]::Zero) { Write-Output "ROOT_NOT_FOUND"; return }
$script:kids = New-Object System.Collections.ArrayList
$cbKid = [BrEnum3+EnumCb]{
  param($h, $l)
  $rect = New-Object BrEnum3+RECT
  [BrEnum3]::GetWindowRect($h, [ref]$rect)
  $t = [BrEnum3]::Text($h)
  if ($t.Length -gt 120) { $t = $t.Substring(0, 120) + "..." }
  $vis = 0
  if ([BrEnum3]::IsWindowVisible($h)) { $vis = 1 }
  [void]$script:kids.Add(("hwnd=" + $h.ToInt64() + " parent=" + [BrEnum3]::GetParent($h).ToInt64() + " id=" + [BrEnum3]::GetDlgCtrlID($h) + " class=" + [BrEnum3]::Cls($h) + " vis=" + $vis + " rect=" + $rect.L + "," + $rect.T + "," + $rect.R + "," + $rect.B + " text=[" + $t + "]"))
  return $true
}
$r2 = [BrEnum3]::EnumChildWindows($root, $cbKid, [IntPtr]::Zero)
$out = "C:\Users\Public\pkg\enum3.txt"
($script:kids -join "`r`n") | Set-Content -Path $out -Encoding ASCII
Write-Output ("ENUM3_DONE kids=" + $script:kids.Count + " -> " + $out)
