# br-dlg.ps1 - dump every #32770 dialog owned by the BackupRestore process,
# with its child controls (class/id/text), written as UTF-8 so Chinese is readable.
# ASCII-only source. Output: C:\Users\Public\pkg\dlg.txt (UTF-8, no BOM)
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public class BrDlg {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(2048); GetWindowTextW(h, sb, 2048); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
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
      res.Add("  child hwnd=" + h.ToInt64() + " id=" + GetDlgCtrlID(h) + " class=" + Cls(h) + " vis=" + (IsWindowVisible(h) ? "1" : "0") + " text=[" + Text(h) + "]");
      return true;
    }, IntPtr.Zero);
    return res;
  }
}
"@
$p = (Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1)
$lines = New-Object System.Collections.ArrayList
if (-not $p) { [void]$lines.Add("NO_PROC") } else {
  [void]$lines.Add("PID=" + $p.Id)
  $tops = [BrDlg]::Tops([uint32]$p.Id)
  [void]$lines.Add("DIALOG_COUNT=" + $tops.Count)
  foreach ($h in $tops) {
    [void]$lines.Add("dlg hwnd=" + $h + " vis=" + (IsWindowVisible($h) ? "1" : "0") + " title=[" + [BrDlg]::Text($h) + "]")
    foreach ($c in [BrDlg]::Kids($h)) { [void]$lines.Add($c) }
  }
}
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\dlg.txt", ($lines -join "`r`n"), $utf8)
Write-Output ("DLG_DONE -> " + "C:\Users\Public\pkg\dlg.txt")