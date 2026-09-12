# Enumerate top-level windows in Session 0 to find BackupRestore main window
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class WinEnum {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
    [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr hWnd, uint cmd);
}
"@
$result = New-Object System.Collections.ArrayList
$cb = [WinEnum+EnumProc]{
    param($h, $l)
    $sb = New-Object System.Text.StringBuilder 512
    [WinEnum]::GetWindowText($h, $sb, 512) | Out-Null
    $p = [uint32]0
    [WinEnum]::GetWindowThreadProcessId($h, [ref]$p) | Out-Null
    if ($sb.Length -gt 0) {
        [void]$script:result.Add(("hwnd={0} pid={1} vis={2} len={3} title='{4}'" -f $h, $p, [WinEnum]::IsWindowVisible($h), $sb.Length, $sb.ToString()))
    }
    return $true
}
[WinEnum]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
$result | Select-Object -First 40
