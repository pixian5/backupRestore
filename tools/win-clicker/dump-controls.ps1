$out = 'C:\Users\Public\brv15\status-read.txt'
$log = 'C:\Users\Public\brv15\status-run.log'
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class VD {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lp);
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool GetWindowRect(IntPtr h, out R r);
  public struct R { public int l,t,r,b; }
}
"@
$mainH = [IntPtr]::Zero
$cb = [VD+EnumWindowsProc]{ param($hw,$lp)
    $t = New-Object System.Text.StringBuilder 512
    [VD]::GetWindowTextW($hw,$t,512) | Out-Null
    if ($t.ToString() -like '*BackupRestore*' -and [VD]::IsWindowVisible($hw)) { $script:mainH = $hw; return $false }
    return $true
}
[VD]::EnumWindows($cb,[IntPtr]::Zero) | Out-Null
[System.IO.File]::WriteAllText($out, "MAIN=" + $mainH + [Environment]::NewLine)
# 枚举主窗口下所有子控件，打印 id、窗口类、文本、位置
foreach ($id in 1200..1400) {
    $c = [VD]::GetDlgItem($mainH, $id)
    if ($c -eq [IntPtr]::Zero) { continue }
    $sb = New-Object System.Text.StringBuilder 512
    [VD]::GetWindowTextW($c,$sb,512) | Out-Null
    $cls = New-Object System.Text.StringBuilder 64
    [VD]::GetClassNameW($c,$cls,64) | Out-Null
    $r = New-Object VD+R
    [VD]::GetWindowRect($c,[ref]$r) | Out-Null
    [System.IO.File]::AppendAllText($out, "ID=$id CLASS=$($cls.ToString()) XY=$($r.l),$($r.t) TEXT=[$($sb.ToString())]" + [Environment]::NewLine)
}