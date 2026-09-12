# Inspect control 0x600a4 (state.controls.status) vs GetDlgItem(1300).
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32H {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    public delegate bool EnumChildProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hParent, EnumChildProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern long GetWindowLongPtr(IntPtr hWnd, int idx);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hWnd);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$found = [IntPtr]::Zero
$cb = [Win32H+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32H]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32H]::EnumWindows($cb, [IntPtr]::Zero)
Write-Output ("main window=" + $found)
$h1 = [IntPtr]393380  # 0x600a4
Write-Output ("iswindow(0x600a4)=" + [Win32H]::IsWindow($h1))
if ([Win32H]::IsWindow($h1)) {
    $c = New-Object System.Text.StringBuilder 64
    [void][Win32H]::GetClassName($h1, $c, 64)
    $id = [Win32H]::GetDlgCtrlID($h1)
    $p = [Win32H]::GetParent($h1)
    $sb = New-Object System.Text.StringBuilder 256
    [void][Win32H]::GetWindowText($h1, $sb, 256)
    $r = New-Object Win32H+RECT
    [void][Win32H]::GetWindowRect($h1, [ref]$r)
    $style = [Win32H]::GetWindowLongPtr($h1, -16)
    Write-Output ("0x600a4: class=" + $c.ToString() + " id=" + $id + " parent=" + $p + " style=" + $style + " rect=(" + $r.Left + "," + $r.Top + ")-(" + $r.Right + "," + $r.Bottom + ") text=" + $sb.ToString())
}
$h2 = [Win32H]::GetDlgItem($found, 1300)
Write-Output ("GetDlgItem(1300)=" + $h2)
if ($h2 -ne [IntPtr]::Zero) {
    $c = New-Object System.Text.StringBuilder 64
    [void][Win32H]::GetClassName($h2, $c, 64)
    $id = [Win32H]::GetDlgCtrlID($h2)
    $p = [Win32H]::GetParent($h2)
    $sb = New-Object System.Text.StringBuilder 256
    [void][Win32H]::GetWindowText($h2, $sb, 256)
    $r = New-Object Win32H+RECT
    [void][Win32H]::GetWindowRect($h2, [ref]$r)
    $style = [Win32H]::GetWindowLongPtr($h2, -16)
    Write-Output ("1300ctrl: class=" + $c.ToString() + " id=" + $id + " parent=" + $p + " style=" + $style + " rect=(" + $r.Left + "," + $r.Top + ")-(" + $r.Right + "," + $r.Bottom + ") text=" + $sb.ToString())
}
