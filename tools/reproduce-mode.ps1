# Reproduce: real click on mode radio (BM_CLICK) + tab switches like the user did.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32R2 {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int IsDlgButtonChecked(IntPtr hDlg, int id);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$BM_CLICK = 0x00F5
$WM_COMMAND = 0x0111

$found = [IntPtr]::Zero
$cb = [Win32R2+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32R2]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32R2]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }
Write-Output ("window hwnd=" + $found)

function Dump($label) {
    $out = @()
    foreach ($id in @(1300, 1203, 2004, 1415, 1417)) {
        $ctl = [Win32R2]::GetDlgItem($found, $id)
        if ($ctl -eq [IntPtr]::Zero) { continue }
        $r = New-Object Win32R2+RECT
        [void][Win32R2]::GetWindowRect($ctl, [ref]$r)
        $sb = New-Object System.Text.StringBuilder 512
        [void][Win32R2]::GetWindowText($ctl, $sb, 512)
        $txt = $sb.ToString().Replace("`r"," ").Replace("`n","|")
        if ($txt.Length -gt 25) { $txt = $txt.Substring(0,25) + ".." }
        $out += ("id={0}({1},{2})-({3},{4})[{5}]" -f $id, $r.Left, $r.Top, $r.Right, $r.Bottom, $txt)
    }
    Write-Output ("{0}: {1}" -f $label, ($out -join "  "))
}

function Click-Radio($id) {
    $ctl = [Win32R2]::GetDlgItem($found, $id)
    [void][Win32R2]::SendMessageW($ctl, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
}
function Click-Tab($id) {
    [void][Win32R2]::SendMessageW($found, $WM_COMMAND, [IntPtr][int64]$id, [IntPtr]::Zero)
}

# go to pe first
Click-Tab 1104
Start-Sleep -Milliseconds 500
Dump "pe ram"
# user's sequence: click DISK mode (1413), switch to secondary, back to pe
Click-Radio 1413
Start-Sleep -Milliseconds 500
Click-Tab 1103
Start-Sleep -Milliseconds 500
Click-Tab 1104
Start-Sleep -Milliseconds 800
Dump "after user seq 1 (disk)"
# repeat a few times
for ($i = 0; $i -lt 3; $i++) {
    Click-Radio 1412
    Start-Sleep -Milliseconds 300
    Click-Radio 1413
    Start-Sleep -Milliseconds 300
    Click-Tab 1101
    Start-Sleep -Milliseconds 300
    Click-Tab 1103
    Start-Sleep -Milliseconds 300
    Click-Tab 1104
    Start-Sleep -Milliseconds 500
}
Dump ("after loop")
# check radio states
Write-Output ("ram_checked=" + [Win32R2]::IsDlgButtonChecked($found, 1412) + " disk_checked=" + [Win32R2]::IsDlgButtonChecked($found, 1413))
