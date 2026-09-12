# Force full-window redraw and check whether the ghost text disappears.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32D {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool InvalidateRect(IntPtr hWnd, IntPtr rect, bool erase);
    [DllImport("user32.dll")] public static extern bool UpdateWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool RedrawWindow(IntPtr hWnd, IntPtr rect, IntPtr hrgn, uint flags);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
}
"@
$log = "C:\Users\Public\backupRestore-package\redraw-test.log"
function Log($msg) { Add-Content -Path $log -Value $msg -Encoding ascii }
$found = [IntPtr]::Zero
$cb = [Win32D+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32D]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32D]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { "WINDOW NOT FOUND" | Out-File $log -Encoding ascii; exit 1 }
function Read2004 {
    $h = [Win32D]::GetDlgItem($found, 2004)
    $sb = New-Object System.Text.StringBuilder 512
    $n = [Win32D]::GetWindowTextLength($h)
    if ($n -gt 0) { [void][Win32D]::GetWindowText($h, $sb, 512) }
    return ("len=" + $sb.ToString().Length)
}
Log ("window=" + $found + " before_redraw " + (Read2004))
# force full redraw
[void][Win32D]::RedrawWindow($found, [IntPtr]::Zero, [IntPtr]::Zero, 0x0085)  # RDW_INVALIDATE|RDW_ERASE|RDW_ALLCHILDREN|RDW_FRAME
[void][Win32D]::UpdateWindow($found)
Start-Sleep -Milliseconds 800
Log ("after_redraw " + (Read2004))
# also nudge window pos to force DWM re-composite
[void][Win32D]::SetWindowPos($found, [IntPtr]::Zero, 0, 0, 0, 0, 0x0001 -bor 0x0004 -bor 0x0010)  # NOSIZE|NOMOVE|SHOWWINDOW
Start-Sleep -Milliseconds 800
Log ("after_setwindowpos " + (Read2004))
Log "DONE"
