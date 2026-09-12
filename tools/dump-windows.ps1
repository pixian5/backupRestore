# Dump all top-level windows (title/class/pid/visible) + BackupRestore process info.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32W {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr hWnd, uint cmd);
}
"@
$log = "C:\Users\Public\backupRestore-package-v12\dump-windows.log"
function Log($msg) { Add-Content -Path $log -Value $msg -Encoding ascii }
$list = New-Object System.Collections.ArrayList
$cb = [Win32W+EnumProc]{
    param($h,$l)
    $sb = New-Object System.Text.StringBuilder 256
    $cl = New-Object System.Text.StringBuilder 128
    [void][Win32W]::GetWindowText($h, $sb, 256)
    [void][Win32W]::GetClassName($h, $cl, 128)
    $pid2 = 0
    [void][Win32W]::GetWindowThreadProcessId($h, [ref]$pid2)
    $vis = [Win32W]::IsWindowVisible($h)
    $t = $sb.ToString()
    if ($t.Length -gt 0 -or $vis) {
        [void]$list.Add("hwnd=$h class=[$($cl.ToString())] pid=$pid2 vis=$vis title=[$t]")
    }
    return $true
}
[void][Win32W]::EnumWindows($cb, [IntPtr]::Zero)
Log ("== top-level windows: " + $list.Count)
foreach ($s in $list) { Log $s }
Log "== backup process"
Get-Process BackupRestore -ErrorAction SilentlyContinue | ForEach-Object {
    Log ("pid=" + $_.Id + " hwnd=" + $_.MainWindowHandle + " session=" + $_.SessionId + " title=[" + $_.MainWindowTitle + "]")
}
Log "DONE"
