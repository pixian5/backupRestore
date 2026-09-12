# BackupRestore control dump: text + rect + WS_VISIBLE for key controls
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32D {
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
$cb = [Win32D+EnumProc]{
    param($h, $l)
    $sb = New-Object System.Text.StringBuilder 512
    [Win32D]::GetWindowText($h, $sb, 512) | Out-Null
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[Win32D]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }
Write-Output ("window hwnd=" + $found)

$ids = [ordered]@{
    1001 = "btn_refresh"; 1002 = "btn_read_image"; 1003 = "btn_create_task"; 1004 = "btn_refresh_task"
    1006 = "btn_browse"
    1410 = "btn_pe_reboot"; 1411 = "btn_shortcut"; 1207 = "menu_edit"; 2008 = "menu_label"
    1412 = "mode_ram"; 1413 = "mode_disk"; 1414 = "pe_dir_label"; 1415 = "pe_dir_edit"
    1416 = "pe_name_label"; 1417 = "pe_name_edit"; 2004 = "image_label"
    1206 = "index_list"
}
foreach ($k in $ids.Keys) {
    $ctl = [Win32D]::GetDlgItem($found, [int]$k)
    if ($ctl -eq [IntPtr]::Zero) {
        Write-Output ("id={0,-6} {1,-15} : NULL" -f $k, $ids[$k])
        continue
    }
    $r = New-Object Win32D+RECT
    [Win32D]::GetWindowRect($ctl, [ref]$r) | Out-Null
    $style = [Win32D]::GetWindowLongPtr($ctl, $GWL_STYLE).ToInt64()
    $wsvis = (($style -band $WS_VISIBLE) -ne 0)
    $sb = New-Object System.Text.StringBuilder 512
    [Win32D]::GetWindowText($ctl, $sb, 512) | Out-Null
    $txt = $sb.ToString().Replace("`r","").Replace("`n","|")
    if ($txt.Length -gt 60) { $txt = $txt.Substring(0,60) + "..." }
    Write-Output ("id={0,-6} {1,-15} vis={2,-5} rect=({3},{4})-({5},{6}) text='{7}'" -f $k, $ids[$k], $wsvis, $r.Left, $r.Top, $r.Right, $r.Bottom, $txt)
}
