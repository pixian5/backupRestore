# sendwm-diag.ps1 — find BackupRestore main window, post WM_COMMAND 1003, write result to file
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class DW {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWin cb, IntPtr lp);
    public delegate bool EnumWin(IntPtr h, IntPtr lp);
}
"@
$out = @()
foreach ($title in @("BackupRestore - Rust GUI v1.5.10", "BackupRestore - Rust GUI", "BackupRestore")) {
    $h = [DW]::FindWindowW([NullString]::Value, $title)
    $out += ("find '{0}' -> hwnd={1}" -f $title, $h)
    if ($h -ne [IntPtr]::Zero) {
        $pid2 = 0
        [DW]::GetWindowThreadProcessId($h, [ref]$pid2) | Out-Null
        $ok = [DW]::PostMessageW($h, 273, [IntPtr]1003, [IntPtr]::Zero)
        $out += ("  post WM_COMMAND(273,1003) = {0} pid={1} isWindow={2}" -f $ok, $pid2, [DW]::IsWindow($h))
        break
    }
}
if (-not ($out -join '').Contains('post')) {
    # fallback: enumerate windows to see what titles exist
    $found = @()
    $cb = [DW+EnumWin]{ param($hw,$lp)
        $t = New-Object System.Text.StringBuilder 256
        [DW]::GetWindowText($hw, $t, 256) | Out-Null
        if ($t.Length -gt 0) { $script:winList += $t.ToString() }
        return $true
    }
    $script:winList = New-Object System.Collections.ArrayList
    [DW]::EnumWindows($cb,[IntPtr]::Zero) | Out-Null
    $out += "NO-MATCH; visible titles sample:"
    $out += ($script:winList | Select-Object -First 40)
}
Set-Content -Path "C:\Users\Public\backupRestore-package\sendwm-diag.txt" -Value $out