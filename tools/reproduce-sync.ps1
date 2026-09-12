# Simulate fast real mouse clicks with synchronous SendMessage WM_COMMAND,
# which re-enters the window proc. Check if status/image controls drift.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32Q {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$found = [IntPtr]::Zero
$cb = [Win32Q+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32Q]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32Q]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }
Write-Output ("window hwnd=" + $found)

function Dump($label) {
    $out = @()
    foreach ($id in @(1300, 2004, 1410, 1411, 1207)) {
        $ctl = [Win32Q]::GetDlgItem($found, $id)
        if ($ctl -eq [IntPtr]::Zero) { continue }
        $r = New-Object Win32Q+RECT
        [void][Win32Q]::GetWindowRect($ctl, [ref]$r)
        $out += ("id={0}({1},{2})-({3},{4})" -f $id, $r.Left, $r.Top, $r.Right, $r.Bottom)
    }
    Write-Output ("{0}: {1}" -f $label, ($out -join "  "))
}

Dump "initial"
# Fast synchronous switching: pe <-> secondary x 12, no sleep (re-entry)
$seq = @(1104, 1103, 1104, 1101, 1104, 1102, 1104, 1103, 1104, 1101, 1104, 1102, 1104, 1103, 1104)
foreach ($tab in $seq) {
    [void][Win32Q]::SendMessageW($found, 0x0111, [IntPtr][int64]$tab, [IntPtr]::Zero)
}
Start-Sleep -Seconds 1
Dump "after fast sync switching"
# switch to pe and dump again
[void][Win32Q]::SendMessageW($found, 0x0111, [IntPtr][int64]1104, [IntPtr]::Zero)
Start-Sleep -Seconds 1
Dump "final pe"
