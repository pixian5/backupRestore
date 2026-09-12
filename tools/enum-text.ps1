# Find which control holds pe_hint text and check its parent/visibility.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32P {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    public delegate bool EnumChildProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hParent, EnumChildProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern long GetWindowLongPtr(IntPtr hWnd, int idx);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
}
"@
$found = [IntPtr]::Zero
$cb = [Win32P+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32P]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32P]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND"; exit 1 }
Write-Output ("window hwnd=" + $found + " style=" + [Win32P]::GetWindowLongPtr($found, -16))

$global:ghits = New-Object System.Collections.ArrayList
$cc = [Win32P+EnumChildProc]{ param($w,$l)
    $t = New-Object System.Text.StringBuilder 512
    [void][Win32P]::GetWindowText($w, $t, 512)
    $txt = $t.ToString()
    $c = New-Object System.Text.StringBuilder 64
    [void][Win32P]::GetClassName($w, $c, 64)
    $id = [Win32P]::GetDlgCtrlID($w)
    $parent = [Win32P]::GetParent($w)
    $style = [Win32P]::GetWindowLongPtr($w, -16)
    if ($txt.Length -gt 0) {
        $short = $txt
        if ($short.Length -gt 45) { $short = $short.Substring(0,45) + ".." }
        [void]$global:ghits.Add(("hwnd={0} id={1} class={2} parent={3} vis={4} text={5}" -f $w, $id, $c.ToString(), $parent, ($style -band 0x10000000), $short))
    }
    return $true
}
[void][Win32P]::EnumChildWindows($found, $cc, [IntPtr]::Zero)
Write-Output ("--- controls with text (" + $global:ghits.Count + ") ---")
$global:ghits | ForEach-Object { Write-Output ("  " + $_) }
$status = [Win32P]::GetDlgItem($found, 1300)
Write-Output ("GetDlgItem(1300) = " + $status + " parent=" + [Win32P]::GetParent($status))
