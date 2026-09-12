# Click tabs with SendInput and capture screen region after each click to catch ghost text.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Drawing;
using System.Drawing.Imaging;
public class Win32G {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, UIntPtr dwExtraInfo);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$log = "C:\Users\Public\backupRestore-package-v12\frames\shot.log"
New-Item -ItemType Directory -Force -Path "C:\Users\Public\backupRestore-package-v12\frames" | Out-Null
function Log($msg) { Add-Content -Path $log -Value $msg -Encoding ascii }
$found = [IntPtr]::Zero
$cb = [Win32G+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32G]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32G]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { "WINDOW NOT FOUND" | Out-File $log -Encoding ascii; exit 1 }
Log ("window=" + $found)
[void][Win32G]::SetForegroundWindow($found)
Start-Sleep -Milliseconds 500

function Click-Center($id) {
    $ctl = [Win32G]::GetDlgItem($found, $id)
    if ($ctl -eq [IntPtr]::Zero) { return }
    $r = New-Object Win32G+RECT
    [void][Win32G]::GetWindowRect($ctl, [ref]$r)
    $x = [int](($r.Left + $r.Right) / 2)
    $y = [int](($r.Top + $r.Bottom) / 2)
    [void][Win32G]::SetCursorPos($x, $y)
    Start-Sleep -Milliseconds 100
    [Win32G]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    [Win32G]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
}
function Snap($name) {
    $wr = New-Object Win32G+RECT
    [void][Win32G]::GetWindowRect($found, [ref]$wr)
    $w = $wr.Right - $wr.Left; $h = $wr.Bottom - $wr.Top
    if ($w -le 0 -or $h -le 0) { return }
    $bmp = New-Object System.Drawing.Bitmap($w, $h)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($wr.Left, $wr.Top, 0, 0, $bmp.Size)
    $path = "C:\Users\Public\backupRestore-package-v12\frames\" + $name + ".png"
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    Log ("saved " + $name + " w=" + $w + " h=" + $h)
}

# go to PE first
Click-Center 1104
Start-Sleep -Milliseconds 600
Snap "00_pe"
# click secondary then PE rapidly, snapshot after each
for ($i = 1; $i -le 8; $i++) {
    Click-Center 1103
    Start-Sleep -Milliseconds 120
    Snap ("01_sec_" + $i)
    Click-Center 1104
    Start-Sleep -Milliseconds 120
    Snap ("02_pe_" + $i)
}
Log "DONE"
