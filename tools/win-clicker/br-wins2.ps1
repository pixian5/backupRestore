# wins2.ps1 - list all visible top-level windows with rect (find custom dialogs).
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class W2 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
"@
$script:lines = New-Object System.Collections.ArrayList
$cb = [W2+EnumCb]{
  param($h, $l)
  if (![W2]::IsWindowVisible($h)) { return $true }
  $sb = New-Object Text.StringBuilder 512
  [void][W2]::GetWindowTextW($h, $sb, 512)
  if ($sb.Length -le 0) { return $true }
  $cn = New-Object Text.StringBuilder 256
  [void][W2]::GetClassNameW($h, $cn, 256)
  $rect = New-Object W2+RECT
  [W2]::GetWindowRect($h, [ref]$rect)
  $wpid = 0
  [void][W2]::GetWindowThreadProcessId($h, [ref]$wpid)
  [void]$script:lines.Add(("hwnd=" + $h.ToInt64() + " pid=" + $wpid + " class=[" + $cn.ToString() + "] rect=" + $rect.L + "," + $rect.T + "," + $rect.R + "," + $rect.B + " title=[" + $sb.ToString() + "]"))
  return $true
}
$r = [W2]::EnumWindows($cb, [IntPtr]::Zero)
$out = "C:\Users\Public\pkg\wins2.txt"
($script:lines -join "`r`n") | Set-Content -Path $out -Encoding UTF8
Write-Output ("WINS2_DONE n=" + $script:lines.Count)
