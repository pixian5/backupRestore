# br-dlgclick.ps1 - click a button (by control id) in a modal #32770 dialog owned by
# the BackupRestore process. Used for the destructive-confirmation prompt, which a
# plain WM_CLOSE would cancel.
# ASCII only. Output: C:\Users\Public\pkg\dlgclick.txt
param([int]$Id = 6, [string]$TitleMatch = "")
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrDC {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(2048); GetWindowTextW(h, sb, 2048); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static List<IntPtr> Dlgs(uint want, string match) {
    var res = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pid != want) return true;
      if (Cls(h) != "#32770") return true;
      if (!IsWindowVisible(h)) return true;
      string t = Text(h);
      if (match.Length > 0 && t.IndexOf(match, StringComparison.OrdinalIgnoreCase) < 0) return true;
      res.Add(h);
      return true;
    }, IntPtr.Zero);
    return res;
  }
}
"@
$p = (Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1)
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }
if (-not $p) { W "NO_PROC" } else {
  $dlgs = [BrDC]::Dlgs([uint32]$p.Id, $TitleMatch)
  W ("DIALOGS=" + $dlgs.Count + " wantId=" + $Id)
  foreach ($h in $dlgs) {
    W ("dlg hwnd=" + $h + " title=[" + [BrDC]::Text($h) + "]")
    $b = [BrDC]::GetDlgItem($h, $Id)
    if ($b -eq [IntPtr]::Zero) { W ("  button " + $Id + " NOT FOUND") } else {
      W ("  clicking button id=" + $Id + " text=[" + [BrDC]::Text($b) + "]")
      [void][BrDC]::SendMessageW($b, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)   # BM_CLICK
      Start-Sleep -Milliseconds 600
    }
  }
  $left = [BrDC]::Dlgs([uint32]$p.Id, "")
  W ("REMAINING=" + $left.Count)
}
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\dlgclick.txt", ($lines -join "`r`n"), $utf8)
Write-Output "DLGCLICK_DONE"