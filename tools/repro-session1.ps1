# Reproduce: rapid real clicks between secondary and PE tabs; dump 2004/1300 text each round.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32R {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr hWnd);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$log = "C:\Users\Public\backupRestore-package-v12\repro-session1.log"
$BM_CLICK = 0x00F5
function Log($msg) { Add-Content -Path $log -Value $msg -Encoding ascii }
$found = [IntPtr]::Zero
$cb = [Win32R+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32R]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32R]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { "WINDOW NOT FOUND" | Out-File $log -Encoding ascii; exit 1 }
Log ("window=" + $found)
function Click-Tab($id) { $ctl = [Win32R]::GetDlgItem($found, $id); if ($ctl -ne [IntPtr]::Zero) { [void][Win32R]::SendMessageW($ctl, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero) } }
function Dump {
    $t2004 = New-Object System.Text.StringBuilder 256
    $h2004 = [Win32R]::GetDlgItem($found, 2004)
    if ($h2004 -ne [IntPtr]::Zero) { $n = [Win32R]::GetWindowTextLength($h2004); if ($n -gt 0) { [void][Win32R]::GetWindowText($h2004, $t2004, 256) } }
    $t1300 = New-Object System.Text.StringBuilder 256
    $h1300 = [Win32R]::GetDlgItem($found, 1300)
    if ($h1300 -ne [IntPtr]::Zero) { $n = [Win32R]::GetWindowTextLength($h1300); if ($n -gt 0) { [void][Win32R]::GetWindowText($h1300, $t1300, 256) } }
    $s2004 = $t2004.ToString(); $s1300 = $t1300.ToString()
    $flag = ""
    if ($s2004 -match "XinZeng" -or ($s2004.Length -gt 6 -and $s2004 -notmatch "^[\x00-\x7F]*$")) { $flag = " NONASCII(" + $s2004.Length + ")" }
    Log ("2004len=" + $s2004.Length + " 1300len=" + $s1300.Length + $flag)
}
# go to secondary first
Click-Tab 1103
Start-Sleep -Milliseconds 300
Log ("after -> secondary")
Dump
for ($i = 1; $i -le 12; $i++) {
    Click-Tab 1104
    Start-Sleep -Milliseconds 150
    Log ("round " + $i + " after -> pe")
    Dump
    Click-Tab 1103
    Start-Sleep -Milliseconds 150
}
Log ("final -> pe")
Click-Tab 1104
Start-Sleep -Milliseconds 300
Dump
Log "DONE"
