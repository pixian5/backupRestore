$out = 'C:\Users\Public\brv15\status-read.txt'
$log = 'C:\Users\Public\brv15\status-run.log'
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class VUI3 {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lp);
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
"@
$proc = [System.Diagnostics.Process]::GetProcessesByName('BackupRestore') | Select-Object -First 1
$pidTarget = if ($proc) { $proc.Id } else { 0 }
$mainH = [IntPtr]::Zero
$cb = [VUI3+EnumWindowsProc]{
    param($h, $lp)
    $t = New-Object System.Text.StringBuilder 512
    [VUI3]::GetWindowTextW($h, $t, 512) | Out-Null
    $p = 0
    [VUI3]::GetWindowThreadProcessId($h, [ref]$p) | Out-Null
    if ($p -eq $pidTarget -and $t.ToString() -like '*BackupRestore*' -and [VUI3]::IsWindowVisible($h)) {
        $script:mainH = $h
        return $false
    }
    return $true
}
[VUI3]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
[System.IO.File]::AppendAllText($log, "MAIN-HWND=" + $mainH + [Environment]::NewLine)
if ($mainH -eq [IntPtr]::Zero) { [System.IO.File]::WriteAllText($out, 'NO-WINDOW'); exit 1 }
$sb = New-Object System.Text.StringBuilder 16384
$n = [VUI3]::GetWindowTextW([VUI3]::GetDlgItem($mainH, 1300), $sb, 16384)
[System.IO.File]::AppendAllText($log, "STATUS-N=" + $n + [Environment]::NewLine)
[System.IO.File]::WriteAllText($out, $sb.ToString())
[System.IO.File]::AppendAllText($log, 'DONE-OK' + [Environment]::NewLine)
