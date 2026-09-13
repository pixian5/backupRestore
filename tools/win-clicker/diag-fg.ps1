# diag-fg.ps1 - report foreground window + focus info (run in Session 1 via run-in-session.ps1)
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class FW {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern IntPtr GetFocus();
    [DllImport("user32.dll")] public static extern IntPtr GetActiveWindow();
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
}
"@
$h = [FW]::GetForegroundWindow()
$sb = New-Object System.Text.StringBuilder 256
[FW]::GetWindowText($h, $sb, 256) | Out-Null
$cls = New-Object System.Text.StringBuilder 256
[FW]::GetClassName($h, $cls, 256) | Out-Null
$pid2 = 0
[FW]::GetWindowThreadProcessId($h, [ref]$pid2) | Out-Null
$f = [FW]::GetFocus()
$sb2 = New-Object System.Text.StringBuilder 256
[FW]::GetWindowText($f, $sb2, 256) | Out-Null
$cls2 = New-Object System.Text.StringBuilder 256
[FW]::GetClassName($f, $cls2, 256) | Out-Null
"FG hwnd=$h title='$($sb.ToString())' class='$($cls.ToString())' pid=$pid2"
"FOCUS hwnd=$f title='$($sb2.ToString())' class='$($cls2.ToString())'"
$p = Get-Process -Id $pid2 -ErrorAction SilentlyContinue
if ($p) { "FGPROC name=$($p.ProcessName) main='$($p.MainWindowTitle)'" }
