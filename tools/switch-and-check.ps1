# Switch operation tabs via WM_COMMAND and verify key control layout per tab.
# Usage: powershell -ExecutionPolicy Bypass -File ...\switch-and-check.ps1
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32S {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr hWnd, int nIndex);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$GWL_STYLE = -16
$WS_VISIBLE = 0x10000000
$WM_COMMAND = 0x0111

$found = [IntPtr]::Zero
$cb = [Win32S+EnumProc]{
    param($h, $l)
    $sb = New-Object System.Text.StringBuilder 512
    [Win32S]::GetWindowText($h, $sb, 512) | Out-Null
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[Win32S]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }

$tabs = @{1101="backup"; 1102="restore"; 1103="secondary"; 1104="pe"}
foreach ($tabId in $tabs.Keys) {
    [Win32S]::PostMessageW($found, $WM_COMMAND, [IntPtr][int64]$tabId, [IntPtr]::Zero) | Out-Null
    Start-Sleep -Seconds 3
    Write-Output ("===== TAB {0} ({1}) =====" -f $tabId, $tabs[$tabId])
    $ids = [ordered]@{
        1001 = "btn_refresh"; 1002 = "btn_read_image"; 1003 = "btn_create_task"; 1004 = "btn_refresh_task"
        1410 = "btn_pe_reboot"; 1411 = "btn_shortcut"; 1207 = "menu_edit"; 2008 = "menu_label"
        1206 = "index_list"; 2004 = "image_label"; 2003 = "source_label"; 2005 = "target_label"
    }
    $rects = @{}
    foreach ($k in $ids.Keys) {
        $ctl = [Win32S]::GetDlgItem($found, [int]$k)
        if ($ctl -eq [IntPtr]::Zero) { continue }
        $r = New-Object Win32S+RECT
        [Win32S]::GetWindowRect($ctl, [ref]$r) | Out-Null
        $style = [Win32S]::GetWindowLongPtr($ctl, $GWL_STYLE).ToInt64()
        $wsvis = (($style -band $WS_VISIBLE) -ne 0)
        if ($wsvis) {
            $rects[$k] = $r
            Write-Output ("  {0,-15} rect=({1},{2})-({3},{4})" -f $ids[$k], $r.Left, $r.Top, $r.Right, $r.Bottom)
        }
    }
    $keys = @($rects.Keys)
    $bad = $false
    for ($i = 0; $i -lt $keys.Count; $i++) {
        for ($j = $i + 1; $j -lt $keys.Count; $j++) {
            $a = $rects[$keys[$i]]; $b = $rects[$keys[$j]]
            $ix = [Math]::Max(0, [Math]::Min($a.Right, $b.Right) - [Math]::Max($a.Left, $b.Left))
            $iy = [Math]::Max(0, [Math]::Min($a.Bottom, $b.Bottom) - [Math]::Max($a.Top, $b.Top))
            $overlap = $ix * $iy
            if ($overlap -gt 200) {
                Write-Output ("  OVERLAP {0} x {1} area={2}" -f $ids[$keys[$i]], $ids[$keys[$j]], $overlap)
                $bad = $true
            }
        }
    }
    if (-not $bad) { Write-Output "  OK: no significant overlap" }
}
