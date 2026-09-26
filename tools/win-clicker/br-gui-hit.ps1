# br-gui-hit.ps1 -- hit-test probe: what control is under the current cursor?
# ASCII ONLY on purpose (PowerShell -File decodes scripts as system ANSI/GBK).
# Writes C:\Users\Public\pkg\hit.txt

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrHit {
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetWindowText(h, sb, 256); return sb.ToString(); }
    public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassName(h, sb, 256); return sb.ToString(); }
    public static string Rect(IntPtr h) { RECT r; if (!GetWindowRect(h, out r)) return "n/a"; return r.L + "," + r.T + "," + r.R + "," + r.B; }
}
"@

$out = "C:\Users\Public\pkg\hit.txt"
$l = New-Object System.Collections.Generic.List[string]
$l.Add("SM_CXSCREEN=" + [BrHit]::GetSystemMetrics(0) + " SM_CYSCREEN=" + [BrHit]::GetSystemMetrics(1))
$p = [BrHit+POINT]::new()
[void][BrHit]::GetCursorPos([ref]$p)
$l.Add("cursor=" + $p.X + "," + $p.Y)

$h = [BrHit]::WindowFromPoint($p)
$l.Add("hit hwnd=" + $h + " id=" + [BrHit]::GetDlgCtrlID($h) + " class=" + [BrHit]::Cls($h) + " text=[" + [BrHit]::Text($h) + "] rect=" + [BrHit]::Rect($h))

$root = [BrHit]::GetAncestor($h, 2)   # GA_ROOT
$l.Add("root hwnd=" + $root + " class=" + [BrHit]::Cls($root) + " text=[" + [BrHit]::Text($root) + "] rect=" + [BrHit]::Rect($root))

$fg = [BrHit]::GetForegroundWindow()
$l.Add("fg hwnd=" + $fg + " text=[" + [BrHit]::Text($fg) + "] rect=" + [BrHit]::Rect($fg))

[System.IO.File]::WriteAllText($out, ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output "HIT_DONE"