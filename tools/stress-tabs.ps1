# Stress test on fresh window: many tab + mode switches, then check status text,
# control rects, and child count for overlap/duplication.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32S {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    public delegate bool EnumChildProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hParent, EnumChildProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr hWnd);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$WM_COMMAND = 0x0111
$BM_CLICK = 0x00F5

$found = [IntPtr]::Zero
$cb = [Win32S+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32S]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32S]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }
Write-Output ("window hwnd=" + $found)

function Click-Tab($id) { [void][Win32S]::SendMessageW($found, $WM_COMMAND, [IntPtr][int64]$id, [IntPtr]::Zero) }
function Click-Radio($id) {
    $ctl = [Win32S]::GetDlgItem($found, $id)
    [void][Win32S]::SendMessageW($ctl, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
}
function Count-Children {
    $global:n = 0
    $c = [Win32S+EnumChildProc]{ param($w,$l) $global:n++; return $true }
    [void][Win32S]::EnumChildWindows($found, $c, [IntPtr]::Zero)
    return $global:n
}
function Dump-Status {
    $s = [Win32S]::GetDlgItem($found, 1300)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32S]::GetWindowText($s, $sb, 512)
    $txt = $sb.ToString()
    $head = if ($txt.Length -gt 20) { $txt.Substring(0,20) } else { $txt }
    Write-Output ("  status text: " + $head + " (len=" + $txt.Length + ")")
}

Write-Output ("initial child_count=" + (Count-Children))
# stress: 20 rounds of switches with mode toggles
for ($i = 0; $i -lt 20; $i++) {
    Click-Tab 1101
    Click-Tab 1103
    Click-Tab 1104
    Click-Radio 1412
    Click-Radio 1413
    Click-Tab 1102
    Click-Tab 1104
    Click-Radio 1413
}
Start-Sleep -Seconds 2
Write-Output ("after stress child_count=" + (Count-Children))
Click-Tab 1104
Start-Sleep -Seconds 1
Write-Output "---- PE tab after stress ----"
Dump-Status
# rect dump
foreach ($id in @(1300, 1203, 2004, 1410, 1411)) {
    $ctl = [Win32S]::GetDlgItem($found, $id)
    if ($ctl -eq [IntPtr]::Zero) { continue }
    $r = New-Object Win32S+RECT
    [void][Win32S]::GetWindowRect($ctl, [ref]$r)
    Write-Output ("  id=" + $id + " rect=(" + $r.Left + "," + $r.Top + ")-(" + $r.Right + "," + $r.Bottom + ")")
}
