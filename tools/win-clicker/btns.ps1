# btns.ps1 - report screen rect of main action buttons (run in Session 1)
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Btns {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int left; public int top; public int right; public int bottom; }
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    public static string Rect(IntPtr root, int id) {
        IntPtr c = GetDlgItem(root, id);
        RECT r; bool ok = GetWindowRect(c, out r);
        return ok ? ("id=" + id + " hwnd=" + c + " rect=" + r.left + "," + r.top + "," + r.right + "," + r.bottom) : ("id=" + id + " fail");
    }
}
"@
$fg = [Btns]::GetForegroundWindow()
$out = @()
$out += "fg=$fg"
$out += [Btns]::Rect($fg, 1003)   # create_task
$out += [Btns]::Rect($fg, 1001)   # refresh env
$out += [Btns]::Rect($fg, 1002)   # read image
$out += [Btns]::Rect($fg, 1004)   # refresh task
$out | Out-File C:\Users\Public\backupRestore-package\btns.txt -Encoding utf8
