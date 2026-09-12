Write-Output "START"
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32F {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    public delegate bool EnumChildProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hParent, EnumChildProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
}
"@
$found = [IntPtr]::Zero
$cb = [Win32F+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32F]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32F]::EnumWindows($cb, [IntPtr]::Zero)
Write-Output ("found=" + $found)
if ($found -eq [IntPtr]::Zero) { exit 1 }

$global:rows = New-Object 'System.Collections.Generic.List[string]'
$cc = [Win32F+EnumChildProc]{ param($w,$l)
    $c = New-Object System.Text.StringBuilder 128
    [void][Win32F]::GetClassName($w, $c, 128)
    $id = [Win32F]::GetDlgCtrlID($w)
    $t = New-Object System.Text.StringBuilder 128
    [void][Win32F]::GetWindowText($w, $t, 128)
    $global:rows.Add(("{0}|id={1}|{2}" -f $c.ToString(), $id, $t.ToString()))
    return $true
}
[void][Win32F]::EnumChildWindows($found, $cc, [IntPtr]::Zero)
Write-Output ("child_count=" + $global:rows.Count)
$global:rows | Group-Object { ($_ -split '\|')[0] } | ForEach-Object { Write-Output ("  class=" + $_.Name + " count=" + $_.Count) }
Write-Output "---- after switch ----"
[void][Win32F]::PostMessageW($found, 0x0111, [IntPtr][int64]1103, [IntPtr]::Zero)
Start-Sleep -Seconds 2
$global:rows.Clear()
[void][Win32F]::EnumChildWindows($found, $cc, [IntPtr]::Zero)
Write-Output ("child_count_after_switch=" + $global:rows.Count)
$global:rows | Group-Object { ($_ -split '\|')[0] } | ForEach-Object { Write-Output ("  class=" + $_.Name + " count=" + $_.Count) }
Write-Output "DONE"
