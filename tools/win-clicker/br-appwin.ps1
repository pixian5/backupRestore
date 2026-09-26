# br-appwin.ps1 - list EVERY top-level window owned by the BackupRestore process
# (visible or hidden) plus their Edit descendants, to find the control that holds
# the image path string actually used by the app.
# ASCII only. Output: C:\Users\Public\pkg\appwin.txt
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public class BrAW {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(2048); GetWindowTextW(h, sb, 2048); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static List<IntPtr> Tops(uint want) {
    var res = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pid == want) res.Add(h);
      return true;
    }, IntPtr.Zero);
    return res;
  }
  public static List<string> Kids(IntPtr root) {
    var res = new List<string>();
    EnumChildWindows(root, delegate(IntPtr h, IntPtr l) {
      res.Add("    child hwnd=" + h.ToInt64() + " id=" + GetDlgCtrlID(h) + " class=" + Cls(h)
        + " vis=" + (IsWindowVisible(h) ? 1 : 0) + " text=[" + Text(h) + "]");
      return true;
    }, IntPtr.Zero);
    return res;
  }
}
"@
$pid_ = (Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1).Id
$lines = New-Object System.Collections.ArrayList
[void]$lines.Add("PID=" + $pid_)
if (-not $pid_) { $lines -join "`r`n" | Set-Content "C:\Users\Public\pkg\appwin.txt" -Encoding ASCII; Write-Output "NO_PID"; exit 1 }
$tops = [BrAW]::Tops([uint32]$pid_)
[void]$lines.Add("TOPLEVEL_COUNT=" + $tops.Count)
foreach ($h in $tops) {
  $vis = 0
  if ([BrAW]::IsWindowVisible($h)) { $vis = 1 }
  [void]$lines.Add("top hwnd=" + $h + " class=" + [BrAW]::Cls($h) + " vis=" + $vis + " title=[" + [BrAW]::Text($h) + "]")
  foreach ($c in [BrAW]::Kids($h)) {
    if ($c -match '\.wim' -or $c -match 'Edit') { [void]$lines.Add($c) }
  }
}
$lines -join "`r`n" | Set-Content "C:\Users\Public\pkg\appwin.txt" -Encoding ASCII
Write-Output ("APPWIN_DONE tops=" + $tops.Count)