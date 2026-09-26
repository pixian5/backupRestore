# br-win-move.ps1 - restore the BackupRestore window from maximized and move it to an
# explicit position/size so dropdown popups are not clipped by the screen edge.
# Coordinates are in the same frame that GetWindowRect / the click agent use.
# ASCII only. Output: C:\Users\Public\pkg\winmove.txt (UTF-8 no BOM)
param(
  [int]$X = -4,
  [int]$Y = -200,
  [int]$W = 585,
  [int]$H = 430
)
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrWM {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L; public int T; public int R; public int B; }
  public static string Text(IntPtr h) {
    if (h == IntPtr.Zero) return "";
    StringBuilder sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString();
  }
  public static IntPtr FindByTitle(string needle) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      string t = Text(h);
      if (t != null && t.IndexOf(needle, StringComparison.OrdinalIgnoreCase) >= 0) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@
$l = New-Object System.Collections.ArrayList
function W($s) { [void]$l.Add($s) }
$hwnd = [BrWM]::FindByTitle('BackupRestore - Rust GUI')
W ("HWND=" + $hwnd + " title=[" + [BrWM]::Text($hwnd) + "]")
$r0 = [BrWM+RECT]::new(); [void][BrWM]::GetWindowRect($hwnd, [ref]$r0)
W ("before rect=" + $r0.L + "," + $r0.T + "," + $r0.R + "," + $r0.B)
[void][BrWM]::ShowWindow($hwnd, 9)      # SW_RESTORE
Start-Sleep -Milliseconds 600
# SWP_NOZORDER=0x0004 | SWP_NOACTIVATE=0x0010
[void][BrWM]::SetWindowPos($hwnd, [IntPtr]::Zero, $X, $Y, $W, $H, 0x0014)
Start-Sleep -Milliseconds 600
[void][BrWM]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 400
$r1 = [BrWM+RECT]::new(); [void][BrWM]::GetWindowRect($hwnd, [ref]$r1)
W ("after  rect=" + $r1.L + "," + $r1.T + "," + $r1.R + "," + $r1.B)
W ("requested x=" + $X + " y=" + $Y + " w=" + $W + " h=" + $H)
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\winmove.txt", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output "WINMOVE_DONE"