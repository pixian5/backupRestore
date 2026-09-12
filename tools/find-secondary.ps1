# Find which control/window actually renders "新增第二系统" text on the PE tab.
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
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool GetWindowTextLength(IntPtr hWnd);
    [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr hWnd, int nIndex);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$log = "C:\Users\Public\backupRestore-package\find-secondary.log"
function Log($msg) { Add-Content -Path $log -Value $msg -Encoding ascii }
Log ("=== scan " + (Get-Date -Format "HH:mm:ss") + " ===")

$found = [IntPtr]::Zero
$cb = [Win32F+EnumProc]{ param($h,$l)
    $cls = New-Object System.Text.StringBuilder 128
    [void][Win32F]::GetClassName($h, $cls, 128)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32F]::GetWindowText($h, $sb, 512)
    $c = $cls.ToString(); $t = $sb.ToString()
    if ($t.StartsWith("BackupRestore - Rust GUI")) { $script:found = $h; Log ("MAIN window=" + $h) }
    if ($c -like "tooltips*") {
        $r = New-Object Win32F+RECT
        [void][Win32F]::GetWindowRect($h, [ref]$r)
        $style = [Win32F]::GetWindowLongPtr($h, -16).ToInt64()
        $vis = ($style -band 0x10000000) -ne 0
        Log ("TOOLTIP hwnd=" + $h + " class=" + $c + " rect=(" + $r.Left + "," + $r.Top + ")-(" + $r.Right + "," + $r.Bottom + ") vis=" + $vis + " text=[" + $t + "]")
    }
    return $true
}
[void][Win32F]::EnumWindows($cb, [IntPtr]::Zero)

if ($found -ne [IntPtr]::Zero) {
    $cc = [Win32F+EnumChildProc]{ param($w,$l)
        $id = [Win32F]::GetDlgCtrlID($w)
        $cls = New-Object System.Text.StringBuilder 64
        [void][Win32F]::GetClassName($w, $cls, 64)
        $len = [Win32F]::GetWindowTextLength($w)
        $t = New-Object System.Text.StringBuilder 512
        if ($len -gt 0) { [void][Win32F]::GetWindowText($w, $t, 512) }
        $r = New-Object Win32F+RECT
        [void][Win32F]::GetWindowRect($w, [ref]$r)
        $style = [Win32F]::GetWindowLongPtr($w, -16).ToInt64()
        $vis = ($style -band 0x10000000) -ne 0
        $txt = $t.ToString()
        $flag = ""
        if ($txt.IndexOf([char]0x65B0) -ge 0 -and $txt.IndexOf([char]0x6DFB) -ge 0) { $flag = "  <<< CONTAINS_XINZENG" }
        Log ("child id=" + $id + " class=" + $cls.ToString() + " rect=(" + $r.Left + "," + $r.Top + ")-(" + $r.Right + "," + $r.Bottom + ") vis=" + $vis + " len=" + $len + " text=[" + $txt + "]" + $flag)
        return $true
    }
    Log "--- children ---"
    [void][Win32F]::EnumChildWindows($found, $cc, [IntPtr]::Zero)
}
Log "DONE"
