# 探测主窗口下 ID 1100..1215 的控件，定位「源盘」下拉框
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class P {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
}
"@
$found = [IntPtr]::Zero
$cb = [P+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [P]::GetWindowText($h, $sb, 512) | Out-Null
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[P]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW_NOT_FOUND"; exit 1 }
Write-Output ("hwnd=" + $found)
for ($id = 1100; $id -le 1215; $id++) {
    $ctl = [P]::GetDlgItem($found, $id)
    if ($ctl -eq [IntPtr]::Zero) { continue }
    $t = New-Object System.Text.StringBuilder 512
    [P]::GetWindowText($ctl, $t, 512) | Out-Null
    $c = New-Object System.Text.StringBuilder 128
    [P]::GetClassName($ctl, $c, 128) | Out-Null
    $txt = $t.ToString().Replace("`r","").Replace("`n","|")
    if ($txt.Length -gt 40) { $txt = $txt.Substring(0,40) + "..." }
    Write-Output ("id={0} class={1} text='{2}'" -f $id, $c.ToString(), $txt)
}
