# Switch tabs many times, dump status(1300) rect+text and image label after each PE return.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32T {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr hWnd, int x, int y, int w, int h, bool repaint);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$found = [IntPtr]::Zero
$cb = [Win32T+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32T]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32T]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }
Write-Output ("window hwnd=" + $found)

function Dump-Key($label) {
    foreach ($id in @(1300, 2004, 1417, 1410)) {
        $ctl = [Win32T]::GetDlgItem($found, $id)
        if ($ctl -eq [IntPtr]::Zero) { continue }
        $r = New-Object Win32T+RECT
        [void][Win32T]::GetWindowRect($ctl, [ref]$r)
        $sb = New-Object System.Text.StringBuilder 512
        [void][Win32T]::GetWindowText($ctl, $sb, 512)
        $txt = $sb.ToString().Replace("`r","").Replace("`n","|")
        if ($txt.Length -gt 30) { $txt = $txt.Substring(0,30) + "..." }
        Write-Output ("  {0} id={1} rect=({2},{3})-({4},{5}) text='{6}'" -f $label, $id, $r.Left, $r.Top, $r.Right, $r.Bottom, $txt)
    }
}

Dump-Key "initial"
$seq = @(1103, 1104, 1101, 1104, 1102, 1104, 1103, 1104, 1101, 1104)
for ($i = 0; $i -lt $seq.Count; $i++) {
    [void][Win32T]::PostMessageW($found, 0x0111, [IntPtr][int64]$seq[$i], [IntPtr]::Zero)
    Start-Sleep -Milliseconds 600
    if ($seq[$i] -eq 1104) {
        Dump-Key ("after PE switch #" + (($i / 1) + 1))
    }
}
