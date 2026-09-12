# Run inside interactive session: switch tabs many times and dump control state.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32Q {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    public delegate bool EnumChildProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hParent, EnumChildProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr hWnd);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$log = "C:\Users\Public\backupRestore-package-v12\tab-stress-session1.log"
$WM_COMMAND = 0x0111
$BM_CLICK = 0x00F5

$found = [IntPtr]::Zero
$cb = [Win32Q+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32Q]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32Q]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { "WINDOW NOT FOUND" | Out-File $log -Encoding ascii; exit 1 }
function Log($msg) { Add-Content -Path $log -Value $msg -Encoding ascii }
Log ("window hwnd=" + $found)

function Click-Tab($id) { [void][Win32Q]::SendMessageW($found, $WM_COMMAND, [IntPtr][int64]$id, [IntPtr]::Zero) }
function Click-Radio($id) {
    $ctl = [Win32Q]::GetDlgItem($found, $id)
    if ($ctl -ne [IntPtr]::Zero) { [void][Win32Q]::SendMessageW($ctl, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero) }
}
function Count-Children {
    $global:n = 0
    $c = [Win32Q+EnumChildProc]{ param($w,$l) $global:n++; return $true }
    [void][Win32Q]::EnumChildWindows($found, $c, [IntPtr]::Zero)
    return $global:n
}
function Dump-Key {
    $line = ""
    foreach ($id in @(1300, 1203, 2004, 1410, 1411, 1415, 1417, 1206, 1207)) {
        $ctl = [Win32Q]::GetDlgItem($found, $id)
        if ($ctl -eq [IntPtr]::Zero) { continue }
        $r = New-Object Win32Q+RECT
        [void][Win32Q]::GetWindowRect($ctl, [ref]$r)
        $sb = New-Object System.Text.StringBuilder 80
        [void][Win32Q]::GetWindowText($ctl, $sb, 80)
        $line += (" id=" + $id + "(" + $r.Left + "," + $r.Top + "," + $r.Right + "," + $r.Bottom + ")" + $sb.ToString())
    }
    Log $line
}

Log ("initial children=" + (Count-Children))
Log ("tab 1")
Click-Tab 1101
Dump-Key
Log ("tab 4 PE")
Click-Tab 1104
Dump-Key
Log ("tab 3 secondary")
Click-Tab 1103
Dump-Key
Log ("tab 4 PE again")
Click-Tab 1104
Dump-Key
# stress loop
for ($i = 0; $i -lt 15; $i++) {
    Click-Tab 1103
    Click-Tab 1104
    Click-Radio 1412
    Click-Radio 1413
    Click-Tab 1101
    Click-Tab 1104
}
Log ("after stress children=" + (Count-Children))
Click-Tab 1104
Start-Sleep -Seconds 1
Log ("final PE dump")
Dump-Key
Log ("DONE")
