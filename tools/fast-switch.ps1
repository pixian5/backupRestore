# Fast tab switching via WM_COMMAND using ArrayList-based EnumWindows (proven working in Session 1).
Add-Type -ReferencedAssemblies System.Drawing.dll @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Drawing;
using System.Drawing.Imaging;
public class Win32FS {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr hWnd);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$log = "C:\Users\Public\backupRestore-package\fast-switch.log"
function Log($msg) { Add-Content -Path $log -Value $msg -Encoding ascii }
$global:glist = New-Object System.Collections.ArrayList
$cb = [Win32FS+EnumProc]{
    param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32FS]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) {
        [void]$global:glist.Add($h)
    }
    return $true
}
[void][Win32FS]::EnumWindows($cb, [IntPtr]::Zero)
Log ("enum count=" + $global:glist.Count)
if ($global:glist.Count -eq 0) { "WINDOW NOT FOUND" | Out-File $log -Encoding ascii; exit 1 }
$found = $global:glist[0]
Log ("window=" + $found)
function ReadText($id) {
    $h = [Win32FS]::GetDlgItem($found, $id)
    $sb = New-Object System.Text.StringBuilder 2048
    $n = [Win32FS]::GetWindowTextLength($h)
    if ($n -gt 0) { [void][Win32FS]::GetWindowText($h, $sb, 2048) }
    return $sb.ToString()
}
# fast switch 30 rounds, alternating 1103/1104, 20ms apart
for ($i = 1; $i -le 30; $i++) {
    $tab = if ($i % 2 -eq 0) { 1103 } else { 1104 }
    # WM_COMMAND: wParam low 16 bits = control id, high 16 bits = notification(0)
    [void][Win32FS]::SendMessage($found, 0x0111, [IntPtr]$tab, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 20
}
# end on PE tab
[void][Win32FS]::SendMessage($found, 0x0111, [IntPtr]1104, [IntPtr]::Zero)
Start-Sleep -Milliseconds 600
$t2004 = ReadText 2004
$t1300 = ReadText 1300
Log ("final tab=pe 2004len=" + $t2004.Length + " text=[" + $t2004 + "]")
Log ("status len=" + $t1300.Length + " head=[" + $t1300.Substring(0, [Math]::Min(40, $t1300.Length)) + "]")
# screenshot window region for visual check
$wr = New-Object Win32FS+RECT
[void][Win32FS]::GetWindowRect($found, [ref]$wr)
$w = $wr.Right - $wr.Left; $h = $wr.Bottom - $wr.Top
$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($wr.Left, $wr.Top, 0, 0, $bmp.Size)
$bmp.Save("C:\Users\Public\backupRestore-package\fast-switch-final.png", [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Log "saved fast-switch-final.png"
Log "DONE"
