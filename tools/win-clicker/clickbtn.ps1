# clickbtn.ps1 - click a control by dialog ID at its true screen center (Session 1, DPI-aware)
param([int]$ButtonId = 1003)
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class Cb {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int left; public int top; public int right; public int bottom; }
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, UIntPtr dwExtraInfo);
    public const uint LEFTDOWN = 0x0002;
    public const uint LEFTUP = 0x0004;
    public static string Click(int x, int y) {
        SetCursorPos(x, y);
        System.Threading.Thread.Sleep(150);
        mouse_event(LEFTDOWN, 0, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(80);
        mouse_event(LEFTUP, 0, 0, 0, UIntPtr.Zero);
        return "clicked " + x + "," + y;
    }
    public static string Run(int id) {
        SetProcessDPIAware();
        IntPtr fg = GetForegroundWindow();
        IntPtr c = GetDlgItem(fg, id);
        RECT r;
        bool ok = GetWindowRect(c, out r);
        int sw = GetSystemMetrics(0); // SM_CXSCREEN
        int sh = GetSystemMetrics(1); // SM_CYSCREEN
        if (!ok) return "getrect fail fg=" + fg;
        int cx = (r.left + r.right) / 2;
        int cy = (r.top + r.bottom) / 2;
        Click(cx, cy);
        return "fg=" + fg + " id=" + id + " rect=" + r.left + "," + r.top + "," + r.right + "," + r.bottom + " center=" + cx + "," + cy + " screen=" + sw + "x" + sh;
    }
}
"@
$result = [Cb]::Run($ButtonId)
$result | Out-File C:\Users\Public\backupRestore-package\clickbtn.txt -Encoding utf8
Write-Output $result
