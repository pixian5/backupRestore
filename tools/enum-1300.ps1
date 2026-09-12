# After switching to PE tab, enumerate ALL controls with id 1300.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32V {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    public delegate bool EnumChildProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hParent, EnumChildProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern long GetWindowLongPtr(IntPtr hWnd, int idx);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$found = [IntPtr]::Zero
$cb = [Win32V+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32V]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32V]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }
Write-Output ("window hwnd=" + $found)

# switch to PE tab first
[void][Win32V]::PostMessageW($found, 0x0111, [IntPtr][int64]1104, [IntPtr]::Zero)
Start-Sleep -Seconds 2

$global:glist = New-Object System.Collections.ArrayList
$cc = [Win32V+EnumChildProc]{ param($w,$l)
    $id = [Win32V]::GetDlgCtrlID($w)
    if ($id -eq 1300) {
        $t = New-Object System.Text.StringBuilder 512
        [void][Win32V]::GetWindowText($w, $t, 512)
        $r = New-Object Win32V+RECT
        [void][Win32V]::GetWindowRect($w, [ref]$r)
        $style = [Win32V]::GetWindowLongPtr($w, -16)
        $vis = ($style -band 0x10000000) -ne 0
        [void]$global:glist.Add(("hwnd={0} rect=({1},{2})-({3},{4}) vis={5} text={6}" -f $w, $r.Left, $r.Top, $r.Right, $r.Bottom, $vis, $t.ToString()))
    }
    return $true
}
[void][Win32V]::EnumChildWindows($found, $cc, [IntPtr]::Zero)
Write-Output ("--- id=1300 controls: " + $global:glist.Count + " ---")
$global:glist | ForEach-Object { Write-Output ("  " + $_) }
