# br-dismiss.ps1 - close any modal #32770 dialog owned by the BackupRestore process.
# The GUI shows a modal MessageBox after every online backup/restore; while it is
# open the main window is DISABLED, so real mouse clicks are swallowed. Call this
# between GUI operations.
# ASCII-only source. Output: C:\Users\Public\pkg\dismiss.txt (UTF-8, no BOM)
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public class BrDis {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(2048); GetWindowTextW(h, sb, 2048); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static List<IntPtr> Dlg(uint want) {
    var res = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pid == want && Cls(h) == "#32770" && IsWindowVisible(h)) res.Add(h);
      return true;
    }, IntPtr.Zero);
    return res;
  }
}
"@
$p = (Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1)
$lines = New-Object System.Collections.ArrayList
if (-not $p) { [void]$lines.Add("NO_PROC") } else {
  $dlgs = [BrDis]::Dlg([uint32]$p.Id)
  [void]$lines.Add("OPEN_DIALOGS=" + $dlgs.Count)
  foreach ($h in $dlgs) {
    [void]$lines.Add("closing hwnd=" + $h + " title=[" + [BrDis]::Text($h) + "]")
    [void][BrDis]::SendMessageW($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)   # WM_CLOSE
    Start-Sleep -Milliseconds 400
  }
  $left = [BrDis]::Dlg([uint32]$p.Id)
  [void]$lines.Add("REMAINING_DIALOGS=" + $left.Count)
}
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\dismiss.txt", ($lines -join "`r`n"), $utf8)
Write-Output ("DISMISS_DONE -> " + $lines[0])