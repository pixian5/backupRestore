# probe-window2.ps1 - ASCII only. Collect windows of the GUI process, print after enum.
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
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
}
'@
$target = 3008
$found = New-Object System.Collections.ArrayList
$proc = [EW+EnumProc]{
    param($h, $l)
    $wp = 0
    [void][EW]::GetWindowThreadProcessId($h, [ref]$wp)
    if ($wp -eq $script:target) {
        $t = New-Object System.Text.StringBuilder 256
        [void][EW]::GetWindowText($h, $t, 256)
        $c = New-Object System.Text.StringBuilder 256
        [void][EW]::GetClassName($h, $c, 256)
        $vis = [EW]::IsWindowVisible($h)
        $owner = [EW]::GetWindow($h, 4)
        [void]$script:found.Add(("hwnd=" + $h + " vis=" + $vis + " owner=" + $owner + " class=[" + $c.ToString() + "] title=[" + $t.ToString() + "]"))
    }
    return $true
}
[void][EW]::EnumWindows($proc, [IntPtr]::Zero)
Write-Output ("found=" + $found.Count)
foreach ($line in $found) { Write-Output $line }
