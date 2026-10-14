# enum-dlg.ps1 - enumerate the modal dialog (class #32770) controls of BackupRestore.
# Read-only. Output: C:\Users\Public\pkg\enumdlg.txt
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Dlg1 {
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
$cb = [Dlg1+EnumCb]{ param($h,$l) [void]$script:tops.Add($h); return $true }
$r = [Dlg1]::EnumWindows($cb, [IntPtr]::Zero)
$lines = New-Object System.Collections.ArrayList
foreach ($h in $script:tops) {
  $cls = [Dlg1]::Cls($h)
  $txt = [Dlg1]::Text($h)
  if ($cls -eq "#32770" -or $txt -match "BackupRestore" -or $cls -match "BackupRestore") {
    $rect = New-Object Dlg1+RECT
    [Dlg1]::GetWindowRect($h, [ref]$rect)
    [void]$lines.Add(("TOP hwnd=" + $h.ToInt64() + " class=" + $cls + " vis=1 rect=" + $rect.L + "," + $rect.T + "," + $rect.R + "," + $rect.B + " text=[" + $txt + "]"))
    $kids = New-Object System.Collections.ArrayList
    $cbk = [Dlg1+EnumCb]{
      param($kh, $kl)
      $kr = New-Object Dlg1+RECT
      [Dlg1]::GetWindowRect($kh, [ref]$kr)
      $vis = 0
      if ([Dlg1]::IsWindowVisible($kh)) { $vis = 1 }
      $kt = [Dlg1]::Text($kh)
      if ($kt.Length -gt 100) { $kt = $kt.Substring(0, 100) + "..." }
      [void]$kids.Add(("  KID hwnd=" + $kh.ToInt64() + " id=" + [Dlg1]::GetDlgCtrlID($kh) + " class=" + [Dlg1]::Cls($kh) + " vis=" + $vis + " rect=" + $kr.L + "," + $kr.T + "," + $kr.R + "," + $kr.B + " text=[" + $kt + "]"))
      return $true
    }
    $r2 = [Dlg1]::EnumChildWindows($h, $cbk, [IntPtr]::Zero)
    foreach ($k in $kids) { [void]$lines.Add($k) }
  }
}
$out = "C:\Users\Public\pkg\enumdlg.txt"
($lines -join "`r`n") | Set-Content -Path $out -Encoding UTF8
Write-Output ("DLG_ENUM_DONE lines=" + $lines.Count)
