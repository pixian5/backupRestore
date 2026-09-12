# Restore BackupRestore window to foreground for visual verification.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32Z {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
}
"@
$found = [IntPtr]::Zero
$cb = [Win32Z+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32Z]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32Z]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }
Write-Output ("window=" + $found + " iconic=" + [Win32Z]::IsIconic($found))
[void][Win32Z]::ShowWindow($found, 9)  # SW_RESTORE
[void][Win32Z]::SetForegroundWindow($found)
Start-Sleep -Milliseconds 500
Write-Output ("restored, iconic=" + [Win32Z]::IsIconic($found))
