# Synchronous switch to PE then read status text immediately.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32Y {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
}
"@
$found = [IntPtr]::Zero
$cb = [Win32Y+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32Y]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32Y]::EnumWindows($cb, [IntPtr]::Zero)
Write-Output ("window=" + $found)

# switch to secondary then pe synchronously
[void][Win32Y]::SendMessageW($found, 0x0111, [IntPtr][int64]1103, [IntPtr]::Zero)
[void][Win32Y]::SendMessageW($found, 0x0111, [IntPtr][int64]1104, [IntPtr]::Zero)
Start-Sleep -Seconds 1
$s = [Win32Y]::GetDlgItem($found, 1300)
$sb = New-Object System.Text.StringBuilder 512
[void][Win32Y]::GetWindowText($s, $sb, 512)
Write-Output ("status text after sync PE switch: " + $sb.ToString())
Write-Output ("status hwnd=" + $s)
