# Move BackupRestore window to a visible position and show it.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32M {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr hWnd, int x, int y, int w, int h, bool repaint);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern long GetWindowLongPtr(IntPtr hWnd, int idx);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$found = [IntPtr]::Zero
$cb = [Win32M+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32M]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32M]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }
$r = New-Object Win32M+RECT
[void][Win32M]::GetWindowRect($found, [ref]$r)
Write-Output ("window=" + $found + " rect=(" + $r.Left + "," + $r.Top + ")-(" + $r.Right + "," + $r.Bottom + ")")
$style = [Win32M]::GetWindowLongPtr($found, -16)
Write-Output ("style=0x" + $style.ToString("X") + " visible=" + [Win32M]::IsWindowVisible($found) + " iconic=" + [Win32M]::IsIconic($found))
# force move to visible area and show
[void][Win32M]::MoveWindow($found, 50, 50, 1020, 760, $true)
[void][Win32M]::ShowWindow($found, 9)
[void][Win32M]::SetForegroundWindow($found)
Start-Sleep -Seconds 1
[void][Win32M]::GetWindowRect($found, [ref]$r)
Write-Output ("after move rect=(" + $r.Left + "," + $r.Top + ")-(" + $r.Right + "," + $r.Bottom + ")")
