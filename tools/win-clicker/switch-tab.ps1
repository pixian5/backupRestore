param([int]$Tab = 1101)
$log = 'C:\Users\Public\brv15\tab-switch.log'
$out = 'C:\Users\Public\brv15\status-read.txt'
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class VM5 {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lp);
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
"@
[System.IO.File]::AppendAllText($log, "SWITCH TAB=$Tab " + (Get-Date) + [Environment]::NewLine)
# 不依赖 PID 匹配：枚举所有可见窗口，标题含 BackupRestore 即为主窗口。
$h = [IntPtr]::Zero
$cb = [VM5+EnumWindowsProc]{ param($hw,$lp)
    $t = New-Object System.Text.StringBuilder 512
    [VM5]::GetWindowTextW($hw,$t,512) | Out-Null
    if ($t.ToString() -like '*BackupRestore*' -and [VM5]::IsWindowVisible($hw)) { $script:h = $hw; return $false }
    return $true
}
[VM5]::EnumWindows($cb,[IntPtr]::Zero) | Out-Null
[System.IO.File]::AppendAllText($log, "MAIN=$h" + [Environment]::NewLine)
if ($h -ne [IntPtr]::Zero) {
    $ok = [VM5]::PostMessageW($h, 273, [IntPtr]$Tab, [IntPtr]::Zero)
    [System.IO.File]::AppendAllText($log, "POST-OK=$ok" + [Environment]::NewLine)
} else {
    [System.IO.File]::AppendAllText($log, 'WINDOW-NOT-FOUND' + [Environment]::NewLine)
    exit 1
}
Start-Sleep -Milliseconds 800
# 读 status 文本框（ID 1300）：复用已定位的主窗口句柄 $h
$sb = New-Object System.Text.StringBuilder 16384
$n = [VM5]::GetWindowTextW([VM5]::GetDlgItem($h,1300),$sb,16384)
[System.IO.File]::WriteAllText($out, $sb.ToString())
[System.IO.File]::AppendAllText($log, "N=$n" + [Environment]::NewLine)
