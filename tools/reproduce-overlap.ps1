# Reproduce tab-switch overlap: switch tabs N times, count child windows
# and dump status/image control rects after each cycle.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32R {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    public delegate bool EnumChildProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hParent, EnumChildProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$WM_COMMAND = 0x0111

$found = [IntPtr]::Zero
$cb = [Win32R+EnumProc]{
    param($h, $l)
    $sb = New-Object System.Text.StringBuilder 512
    [Win32R]::GetWindowText($h, $sb, 512) | Out-Null
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[Win32R]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }

function Count-Children($h) {
    $n = 0
    $c = [Win32R+EnumChildProc]{
        param($w, $l)
        $script:n++
        return $true
    }
    [Win32R]::EnumChildWindows($h, $c, [IntPtr]::Zero) | Out-Null
    return $script:n
}

function Dump-Status($h) {
    # status edit id: 1205? try list of candidate ids
    foreach ($id in @(1205, 1208, 1005, 1008, 1009, 1010, 1209, 1210)) {
        $ctl = [Win32R]::GetDlgItem($h, $id)
        if ($ctl -ne [IntPtr]::Zero) {
            $r = New-Object Win32R+RECT
            [Win32R]::GetWindowRect($ctl, [ref]$r) | Out-Null
            $sb = New-Object System.Text.StringBuilder 512
            [Win32R]::GetWindowText($ctl, $sb, 512) | Out-Null
            $txt = $sb.ToString().Replace("`r","").Replace("`n","|")
            if ($txt.Length -gt 40) { $txt = $txt.Substring(0,40) + "..." }
            Write-Output ("  id={0} rect=({1},{2})-({3},{4}) text='{5}'" -f $id, $r.Left, $r.Top, $r.Right, $r.Bottom, $txt)
        }
    }
}

Write-Output ("window hwnd=" + $found)
Write-Output ("initial child count = " + (Count-Children $found))

# Switch sequence: pe -> secondary -> pe -> restore -> pe -> backup -> pe x several
$seq = @(1104, 1103, 1104, 1102, 1104, 1101, 1104, 1103, 1104, 1102, 1104)
$round = 0
foreach ($tab in $seq) {
    [Win32R]::PostMessageW($found, $WM_COMMAND, [IntPtr][int64]$tab, [IntPtr]::Zero) | Out-Null
    Start-Sleep -Milliseconds 800
    $round++
    if ($round % 3 -eq 0 -or $round -eq $seq.Count) {
        $cnt = Count-Children $found
        Write-Output ("round {0} tab={1} child_count={2}" -f $round, $tab, $cnt)
    }
}
Write-Output "---- final dump (pe tab) ----"
[Win32R]::PostMessageW($found, $WM_COMMAND, [IntPtr][int64]1104, [IntPtr]::Zero) | Out-Null
Start-Sleep -Seconds 2
Write-Output ("final child count = " + (Count-Children $found))
Dump-Status $found
