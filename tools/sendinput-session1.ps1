# Simulate REAL mouse clicks on tab radios with SendInput, then dump 2004/1300 text.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32S2 {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, UIntPtr dwExtraInfo);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$log = "C:\Users\Public\backupRestore-package-v12\sendinput-session1.log"
function Log($msg) { Add-Content -Path $log -Value $msg -Encoding ascii }
$found = [IntPtr]::Zero
$cb = [Win32S2+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32S2]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32S2]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { "WINDOW NOT FOUND" | Out-File $log -Encoding ascii; exit 1 }
Log ("window=" + $found)
function Click-Center($id) {
    $ctl = [Win32S2]::GetDlgItem($found, $id)
    if ($ctl -eq [IntPtr]::Zero) { return }
    $r = New-Object Win32S2+RECT
    [void][Win32S2]::GetWindowRect($ctl, [ref]$r)
    $x = [int](($r.Left + $r.Right) / 2)
    $y = [int](($r.Top + $r.Bottom) / 2)
    [void][Win32S2]::SetCursorPos($x, $y)
    Start-Sleep -Milliseconds 50
    [Win32S2]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)  # left down
    [Win32S2]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)  # left up
}
function Dump {
    $h2004 = [Win32S2]::GetDlgItem($found, 2004)
    $t2004 = New-Object System.Text.StringBuilder 512
    if ($h2004 -ne [IntPtr]::Zero) { $n = [Win32S2]::GetWindowTextLength($h2004); if ($n -gt 0) { [void][Win32S2]::GetWindowText($h2004, $t2004, 512) } }
    $h1300 = [Win32S2]::GetDlgItem($found, 1300)
    $t1300 = New-Object System.Text.StringBuilder 512
    if ($h1300 -ne [IntPtr]::Zero) { $n = [Win32S2]::GetWindowTextLength($h1300); if ($n -gt 0) { [void][Win32S2]::GetWindowText($h1300, $t1300, 512) } }
    Log ("2004len=" + $t2004.ToString().Length + " 1300len=" + $t1300.ToString().Length)
}
# click secondary tab (real click)
Click-Center 1103
Start-Sleep -Milliseconds 400
Log ("after real click -> secondary")
Dump
for ($i = 1; $i -le 10; $i++) {
    Click-Center 1104
    Start-Sleep -Milliseconds 200
    Log ("round " + $i + " -> pe")
    Dump
    Click-Center 1103
    Start-Sleep -Milliseconds 200
}
Click-Center 1104
Start-Sleep -Milliseconds 400
Log ("final -> pe")
Dump
Log "DONE"
