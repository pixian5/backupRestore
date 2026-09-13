$log = 'C:\Users\Public\brv15\status-run.log'
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class VUI2 {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lp);
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
"@
$sb = New-Object System.Text.StringBuilder
$proc = [System.Diagnostics.Process]::GetProcessesByName('BackupRestore') | Select-Object -First 1
$pidTarget = if ($proc) { $proc.Id } else { 0 }
[System.IO.File]::AppendAllText($log, "TARGET-PID=$pidTarget" + [Environment]::NewLine)
$found = @()
$cb = [VUI2+EnumWindowsProc]{
    param($h, $lp)
    $t = New-Object System.Text.StringBuilder 512
    [VUI2]::GetWindowTextW($h, $t, 512) | Out-Null
    $title = $t.ToString()
    if ($title) {
        $p = 0
        [VUI2]::GetWindowThreadProcessId($h, [ref]$p) | Out-Null
        if ($p -eq $pidTarget) {
            $script:found += ("PID=$p TITLE=[$title] VIS=" + [VUI2]::IsWindowVisible($h))
        }
    }
    return $true
}
[VUI2]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
$script:found | ForEach-Object { [System.IO.File]::AppendAllText($log, $_ + [Environment]::NewLine) }
[System.IO.File]::AppendAllText($log, "ENUM-DONE" + [Environment]::NewLine)
