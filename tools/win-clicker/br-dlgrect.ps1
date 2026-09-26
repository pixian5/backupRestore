# br-dlgrect.ps1 - dump #32770 dialogs of BackupRestore with screen rects + centres
# and a responsiveness probe, so a real mouse click can be aimed at the buttons.
# ASCII only. Output: C:\Users\Public\pkg\dlgrect.txt
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrDR {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, IntPtr l, uint f, uint t, out IntPtr res);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(2048); GetWindowTextW(h, sb, 2048); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static string Rect(IntPtr h) { RECT r; GetWindowRect(h, out r); return r.L + "," + r.T + "," + r.R + "," + r.B + " c=" + ((r.L + r.R) / 2) + "," + ((r.T + r.B) / 2); }
  public static string Responsive(IntPtr h) { IntPtr res; IntPtr ok = SendMessageTimeout(h, 0, IntPtr.Zero, IntPtr.Zero, 2, 1500, out res); return ok == IntPtr.Zero ? "HUNG" : ("OK code=" + res.ToInt64()); }
  public static List<IntPtr> Tops(uint want) {
    var res = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pid == want && Cls(h) == "#32770") res.Add(h);
      return true;
    }, IntPtr.Zero);
    return res;
  }
  public static List<string> Kids(IntPtr root) {
    var res = new List<string>();
    EnumChildWindows(root, delegate(IntPtr h, IntPtr l) {
      res.Add("  child id=" + GetDlgCtrlID(h) + " class=" + Cls(h) + " vis=" + (IsWindowVisible(h) ? 1 : 0) + " enabled=" + (IsWindowEnabled(h) ? 1 : 0) + " rect=" + Rect(h) + " text=[" + Text(h) + "]");
      return true;
    }, IntPtr.Zero);
    return res;
  }
}
"@
$p = (Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1)
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }
$fg = [BrDR]::GetForegroundWindow()
W ("FRG hwnd=" + $fg + " class=" + [BrDR]::Cls($fg) + " text=[" + [BrDR]::Text($fg) + "]")
if (-not $p) { W "NO_PROC" } else {
  $tops = [BrDR]::Tops([uint32]$p.Id)
  W ("DIALOG_COUNT=" + $tops.Count)
  foreach ($h in $tops) {
    W ("dlg hwnd=" + $h + " vis=" + (IsWindowVisible($h) ? 1 : 0) + " enabled=" + (IsWindowEnabled($h) ? 1 : 0) + " rect=" + [BrDR]::Rect($h) + " resp=" + [BrDR]::Responsive($h) + " title=[" + [BrDR]::Text($h) + "]")
    foreach ($c in [BrDR]::Kids($h)) { W $c }
  }
}
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\dlgrect.txt", ($lines -join "`r`n"), $utf8)
Write-Output "DLGRECT_DONE"