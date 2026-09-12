# Enumerate ALL child controls with hwnd, ctrl id, class; detect duplicate ids.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32D {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    public delegate bool EnumChildProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hParent, EnumChildProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$found = [IntPtr]::Zero
$cb = [Win32D+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32D]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32D]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }
Write-Output ("window hwnd=" + $found)

$global:grows = New-Object System.Collections.ArrayList
$cc = [Win32D+EnumChildProc]{ param($w,$l)
    $c = New-Object System.Text.StringBuilder 128
    [void][Win32D]::GetClassName($w, $c, 128)
    $id = [Win32D]::GetDlgCtrlID($w)
    $t = New-Object System.Text.StringBuilder 200
    [void][Win32D]::GetWindowText($w, $t, 200)
    $r = New-Object Win32D+RECT
    [void][Win32D]::GetWindowRect($w, [ref]$r)
    [void]$global:grows.Add(("{0}|0x{1:X}|{2}|({3},{4})-({5},{6})|{7}" -f $c.ToString(), $id, $w, $r.Left, $r.Top, $r.Right, $r.Bottom, $t.ToString()))
    return $true
}
[void][Win32D]::EnumChildWindows($found, $cc, [IntPtr]::Zero)
Write-Output ("total_children=" + $global:grows.Count)
Write-Output "---- duplicate ctrl ids ----"
$global:grows | ForEach-Object { ($_ -split '\|')[1] } | Group-Object | Where-Object { $_.Count -gt 1 } | ForEach-Object {
    Write-Output ("  id=" + $_.Name + " count=" + $_.Count)
}
Write-Output "---- id=1300 controls ----"
$global:grows | Where-Object { ($_ -split '\|')[1] -eq '0x514' } | ForEach-Object { Write-Output ("  " + $_) }
Write-Output "---- id=1203 controls ----"
$global:grows | Where-Object { ($_ -split '\|')[1] -eq '0x4B3' } | ForEach-Object { Write-Output ("  " + $_) }
Write-Output "DONE"
