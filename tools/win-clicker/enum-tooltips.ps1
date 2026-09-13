# enum-tooltips.ps1 - 枚举进程中窗口并报告 tooltip 相关类，用于验证 tooltip 是否注册
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class TT1 {
    [DllImport("user32.dll")] public static extern bool EnumWindows(TTEnum cb, IntPtr lp);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    public delegate bool TTEnum(IntPtr h, IntPtr lp);
    public struct RECT { public int l,t,r,b; }
}
"@
$cs = New-Object System.Text.StringBuilder 256
$ts = New-Object System.Text.StringBuilder 256
$n = 0
$cb = [TT1+TTEnum]{ param($hw,$lp)
    [TT1]::GetWindowTextW($hw,$ts,256) | Out-Null
    [TT1]::GetClassNameW($hw,$cs,256) | Out-Null
    $pid2 = 0
    [TT1]::GetWindowThreadProcessId($hw,[ref]$pid2) | Out-Null
    $vis = [TT1]::IsWindowVisible($hw)
    $r = New-Object TT1+RECT
    [TT1]::GetWindowRect($hw,[ref]$r) | Out-Null
    Write-Output ("hwnd={0} class='{1}' text='{2}' pid={3} vis={4} rect=({5},{6},{7},{8})" -f $hw,$cs.ToString(),$ts.ToString(),$pid2,$vis,$r.l,$r.t,$r.r,$r.b)
    $script:n++
    return $true
}
[TT1]::EnumWindows($cb,[IntPtr]::Zero) | Out-Null
Write-Output ("TOTAL={0}" -f $script:n)