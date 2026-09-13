# send-wm.ps1 —— 向 BackupRestore 主窗口或前台窗口 PostMessage（供自动化验收）
param(
    [uint32]$Msg = 273,
    [int]$WParam = 1003
)
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class WM2 {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
}
"@
foreach ($title in @("BackupRestore - Rust GUI v1.5.10", "BackupRestore - Rust GUI")) {
    $h = [WM2]::FindWindowW([NullString]::Value, $title)
    if ($h -ne [IntPtr]::Zero) {
        $sb = New-Object System.Text.StringBuilder 256
        [WM2]::GetClassName($h, $sb, 256) | Out-Null
        $pid2 = 0
        [WM2]::GetWindowThreadProcessId($h, [ref]$pid2) | Out-Null
        Write-Output ("title='{0}' hwnd={1} class='{2}' pid={3} isWindow={4}" -f $title, $h, $sb.ToString(), $pid2, [WM2]::IsWindow($h))
        if ($Msg -gt 0) {
            $ok = [WM2]::PostMessageW($h, $Msg, [IntPtr]$WParam, [IntPtr]::Zero)
            Write-Output ("  -> PostMessageW({0},{1}) = {2}" -f $Msg, $WParam, $ok)
        }
        exit 0
    }
}
Write-Output "not found"