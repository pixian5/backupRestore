# br-gui-selftest.ps1 —— 输入注入通道自检（提权运行）
#
# 判定「点击到底有没有被系统接收」的硬指标是 GetLastInputInfo 的 tick：
# SendInput 注入的鼠标事件同样会刷新它。若点击前后 tick 不变，说明注入没生效
# （通常是完整性级别不足被 UIPI 拦掉，或跑在 Session 0），光看脚本退出码是骗人的。
#
# 自检动作刻意做成无害的：把光标挪到任务栏空白区（y = 屏幕高 - 12）并左键单击，
# 那里没有任何可触发的功能。
#
# 输出：C:\Users\Public\pkg\_selftest.txt（ASCII）

param(
    [int]$X = -1,
    [int]$Y = -1
)

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class BrSelf {
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] p, int cb);
    [DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LASTINPUTINFO li);
    [DllImport("kernel32.dll")] public static extern uint GetTickCount();
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int n);
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [StructLayout(LayoutKind.Sequential)] public struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public InputU U; }
    [StructLayout(LayoutKind.Explicit)] public struct InputU { [FieldOffset(0)] public MOUSEINPUT mi; }
    [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr extra; }
    public static uint LastTick() {
        LASTINPUTINFO li = new LASTINPUTINFO();
        li.cbSize = (uint)Marshal.SizeOf(typeof(LASTINPUTINFO));
        GetLastInputInfo(ref li);
        return li.dwTime;
    }
    public static void Click(int x, int y) {
        SetCursorPos(x, y);
        System.Threading.Thread.Sleep(120);
        INPUT[] inp = new INPUT[2];
        inp[0].type = 0; inp[0].U.mi.dwFlags = 0x0002; inp[0].U.mi.dx = x; inp[0].U.mi.dy = y;
        inp[1].type = 0; inp[1].U.mi.dwFlags = 0x0004; inp[1].U.mi.dx = x; inp[1].U.mi.dy = y;
        SendInput(2, inp, Marshal.SizeOf(typeof(INPUT)));
    }
    public static string Cur() { POINT p; GetCursorPos(out p); return p.X + "," + p.Y; }
    public static IntPtr WinAt(int x, int y) { POINT p; p.X = x; p.Y = y; return WindowFromPoint(p); }
}
"@

$sw = [BrSelf]::GetSystemMetrics(0)
$sh = [BrSelf]::GetSystemMetrics(1)
if ($X -lt 0) { $X = [int]($sw / 2) }
if ($Y -lt 0) { $Y = $sh - 12 }

$tickBefore = [BrSelf]::LastTick()
$curBefore  = [BrSelf]::Cur()
[BrSelf]::Click($X, $Y)
Start-Sleep -Milliseconds 400
$tickAfter = [BrSelf]::LastTick()
$curAfter  = [BrSelf]::Cur()

$lines = @()
$lines += "screen=" + $sw + "x" + $sh
$lines += "target=" + $X + "," + $Y
$lines += "cursor=" + $curBefore + " -> " + $curAfter
$lines += "lastInputTick=" + $tickBefore + " -> " + $tickAfter
$lines += "hwndAtPoint=" + [BrSelf]::WinAt($X, $Y)
if ($tickAfter -ne $tickBefore) { $lines += "VERDICT=INPUT_DELIVERED" } else { $lines += "VERDICT=NO_INPUT" }
$lines -join "`r`n" | Set-Content "C:\Users\Public\pkg\_selftest.txt" -Encoding ASCII
Write-Output "SELFTEST_DONE"
