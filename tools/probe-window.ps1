# probe-window.ps1 - ASCII only
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class EW {
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern IntPtr GetShellWindow();
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
}
'@
$count = 0
$brCount = 0
$proc = [EW+EnumProc]{
    param($h, $l)
    $script:count = $script:count + 1
    $wp = 0
    [void][EW]::GetWindowThreadProcessId($h, [ref]$wp)
    if ($wp -eq 3008) {
        $script:brCount = $script:brCount + 1
        $t = New-Object System.Text.StringBuilder 256
        [void][EW]::GetWindowText($h, $t, 256)
        $c = New-Object System.Text.StringBuilder 256
        [void][EW]::GetClassName($h, $c, 256)
        Write-Output ("br-window hwnd=" + $h + " class=[" + $c.ToString() + "] title=[" + $t.ToString() + "]")
    }
    return $true
}
[void][EW]::EnumWindows($proc, [IntPtr]::Zero)
Write-Output ("total-windows=" + $count + " br-windows=" + $brCount)
Write-Output ("shell-window=" + [EW]::GetShellWindow())
Write-Output ("find-main=[" + [EW]::FindWindow("BackupRestoreNativeGui", $null) + "]")
