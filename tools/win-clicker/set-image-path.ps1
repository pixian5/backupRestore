# set-image-path.ps1 —— 在 Session 1 把 BackupRestore 的"镜像绝对路径"(ID 1203) 设为给定值
param(
    [string]$Path = "E:\br-cdrive-v1.wim",
    [string]$Log = "H:\brwork\set-image-path.log"
)
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class SIP2 {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool SetDlgItemTextW(IntPtr hDlg, int id, string text);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetDlgItemTextW(IntPtr hDlg, int id, StringBuilder sb, int max);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
}
"@
$f = "H:\brwork\set-image-path.log"
function Log($m) { Add-Content -Path $f -Value (Get-Date -Format "HH:mm:ss") + " " + $m }
$found = [IntPtr]::Zero
$cb = [SIP2+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [SIP2]::GetWindowText($h, $sb, 512) | Out-Null
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[SIP2]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($found -eq [IntPtr]::Zero) { Log "WINDOW NOT FOUND"; Write-Output "WINDOW NOT FOUND"; exit 1 }
$id = 1203
$ctl = [SIP2]::GetDlgItem($found, $id)
if ($ctl -eq [IntPtr]::Zero) { Log "CONTROL $id NULL"; Write-Output "CONTROL $id NULL"; exit 1 }
$ok = [SIP2]::SetDlgItemTextW($found, $id, $Path)
# SetDlgItemTextW does NOT notify the app: without EN_CHANGE the GUI's internal
# state stays empty and "read WIM metadata" logs image= (2026-10-16 V1 hit this).
# Send WM_COMMAND/EN_CHANGE (0x0300) to the parent with the edit as lParam.
$notify = [IntPtr]([int64]$id -bor (0x300 -shl 16))
[void][SIP2]::SendMessageW($found, 0x0111, $notify, $ctl)
Start-Sleep -Milliseconds 300
$sb = New-Object System.Text.StringBuilder 512
$n = [SIP2]::GetDlgItemTextW($found, $id, $sb, 512)
Log "set=$ok len=$n text='$($sb.ToString())' path='$Path' en_change_sent=true"
Write-Output ("set=$ok len=$n text='$($sb.ToString())'")