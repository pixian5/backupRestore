$env:TEMP = "C:\Users\Public"
$env:TMP  = "C:\Users\Public"
try { Start-Transcript -LiteralPath "H:\brwork\_rects.log" -Force | Out-Null } catch { }
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class W {
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
    public static IntPtr FindByPrefix(string p) {
        IntPtr f = IntPtr.Zero;
        EnumWindows((h,l) => {
            if (!IsWindowVisible(h)) return true;
            StringBuilder sb = new StringBuilder(512);
            GetWindowTextW(h, sb, 512);
            if (sb.ToString().StartsWith(p)) { f = h; return false; }
            return true;
        }, IntPtr.Zero);
        return f;
    }
    public static string Class(IntPtr h) {
        StringBuilder sb = new StringBuilder(128);
        GetClassNameW(h, sb, 128);
        return sb.ToString();
    }
}
'@
$main = [W]::FindByPrefix("BackupRestore - Rust GUI")
if ($main -eq [IntPtr]::Zero) { Write-Output "WINDOW_NOT_FOUND" } else {
    Write-Output ("HWND=" + $main)
    foreach ($id in @(1001, 1003, 1006, 1202, 1203, 1204, 1208, 1410, 1411)) {
        $c = [W]::GetDlgItem($main, $id)
        if ($c -eq [IntPtr]::Zero) { Write-Output ("ID=" + $id + " NULL"); continue }
        $r = New-Object W+RECT
        [void][W]::GetWindowRect($c, [ref]$r)
        $cx = [int](($r.Left + $r.Right) / 2)
        $cy = [int](($r.Top + $r.Bottom) / 2)
        Write-Output ("ID=" + $id + " class=" + [W]::Class($c) + " rect=(" + $r.Left + "," + $r.Top + ")-(" + $r.Right + "," + $r.Bottom + ") center=" + $cx + "," + $cy)
    }
}
try { Stop-Transcript | Out-Null } catch { }
