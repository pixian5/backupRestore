# BackupRestore UI overlap verification script (Session 0 channel)
# Locate main window by EnumWindows, then check WS_VISIBLE style bit and
# rect of each key control; report pairwise overlap of actually visible ones.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32C {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr hWnd, int nIndex);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$GWL_STYLE = -16
$WS_VISIBLE = 0x10000000

$found = [IntPtr]::Zero
$cb = [Win32C+EnumProc]{
    param($h, $l)
    $sb = New-Object System.Text.StringBuilder 512
    [Win32C]::GetWindowText($h, $sb, 512) | Out-Null
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[Win32C]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }
Write-Output ("window hwnd=" + $found)

$ids = [ordered]@{
    1001 = "refresh"; 1002 = "read_image"; 1003 = "create_task"; 1004 = "refresh_task"
    1006 = "browse_image"
    1410 = "pe_reboot"; 1411 = "shortcut"; 1207 = "menu_edit"; 2008 = "menu_label"
    1412 = "mode_ram"; 1413 = "mode_disk"; 1415 = "pe_dir_edit"; 1417 = "pe_name_edit"
    1206 = "index_list"
}
$rects = @{}
foreach ($k in $ids.Keys) {
    $ctl = [Win32C]::GetDlgItem($found, [int]$k)
    if ($ctl -eq [IntPtr]::Zero) {
        Write-Output ("id={0,-6} {1,-14} : NULL" -f $k, $ids[$k])
        continue
    }
    $r = New-Object Win32C+RECT
    [Win32C]::GetWindowRect($ctl, [ref]$r) | Out-Null
    $style = [Win32C]::GetWindowLongPtr($ctl, $GWL_STYLE).ToInt64()
    $wsvis = (($style -band $WS_VISIBLE) -ne 0)
    Write-Output ("id={0,-6} {1,-14} ws_vis={2,-5} rect=({3},{4})-({5},{6})" -f $k, $ids[$k], $wsvis, $r.Left, $r.Top, $r.Right, $r.Bottom)
    if ($wsvis) { $rects[$k] = $r }
}

Write-Output "---- overlap check (WS_VISIBLE controls) ----"
$keys = @($rects.Keys)
$found2 = $false
for ($i = 0; $i -lt $keys.Count; $i++) {
    for ($j = $i + 1; $j -lt $keys.Count; $j++) {
        $a = $rects[$keys[$i]]; $b = $rects[$keys[$j]]
        $ix = [Math]::Max(0, [Math]::Min($a.Right, $b.Right) - [Math]::Max($a.Left, $b.Left))
        $iy = [Math]::Max(0, [Math]::Min($a.Bottom, $b.Bottom) - [Math]::Max($a.Top, $b.Top))
        $overlap = $ix * $iy
        if ($overlap -gt 200) {
            Write-Output ("OVERLAP id={0}({1}) x id={2}({3}) area={4}" -f $keys[$i], $ids[$keys[$i]], $keys[$j], $ids[$keys[$j]], $overlap)
            $found2 = $true
        }
    }
}
if (-not $found2) { Write-Output "NO SIGNIFICANT OVERLAP" }
