# br-enum.ps1 - dump all descendant controls of the BackupRestore window.
# Read-only. ASCII only (PowerShell -File decodes as system ANSI/GBK).
# Output: C:\Users\Public\pkg\enum.txt
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public class BrEnum {
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
  public static IntPtr Root = IntPtr.Zero;
  public static List<string> Dump() {
    var res = new List<string>();
    EnumChildWindows(Root, delegate(IntPtr h, IntPtr l) {
      RECT r; GetWindowRect(h, out r);
      string t = Text(h);
      if (t.Length > 150) t = t.Substring(0, 150) + "...";
      res.Add("hwnd=" + h.ToInt64() + " parent=" + GetParent(h).ToInt64() + " id=" + GetDlgCtrlID(h)
        + " class=" + Cls(h) + " vis=" + (IsWindowVisible(h) ? 1 : 0)
        + " rect=" + r.L + "," + r.T + "," + r.R + "," + r.B
        + " text=[" + t + "]");
      return true;
    }, IntPtr.Zero);
    return res;
  }
}
"@
$found = [IntPtr]::Zero
$cb = [BrEnum+EnumCb]{ param($h,$l)
  $t = [BrEnum]::Text($h)
  if ($t -ne $null -and $t.StartsWith("BackupRestore - Rust GUI")) { $script:found = $h; return $false }
  return $true
}
[BrEnum]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
$lines = New-Object System.Collections.ArrayList
[void]$lines.Add("ROOT=" + $found + " title=[" + [BrEnum]::Text($found) + "]")
[BrEnum]::Root = $found
foreach ($l in [BrEnum]::Dump()) { [void]$lines.Add($l) }
$out = "C:\Users\Public\pkg\enum.txt"
$lines -join "`r`n" | Set-Content -Path $out -Encoding ASCII
Write-Output ("ENUM_DONE count=" + ($lines.Count - 1) + " -> " + $out)