# br-gui-probe.ps1 —— 探测 prlctl exec 上下文能否直接做 GUI 注入
#
# 目的：判断「prlctl exec --current-user 跑出来的进程」落在哪个 Windows 会话。
# 只有落在已登录用户的交互桌面（winsta0\default，通常是 Session 1）时，
# SetCursorPos / SendInput 才会真的动到用户能看见的那个光标。
# 若落在 Session 0（服务会话，无桌面），注入会静默无效——这是本通道最大的坑。
#
# 判定依据（三条互相印证）：
#   1. 进程 SessionId 是否等于当前活动控制台会话 ID
#   2. 能不能枚举到用户桌面的可见窗口（Session 0 枚举不到）
#   3. SetCursorPos 之后 GetCursorPos 是否真的跟着变
#
# 【重要】下面 Add-Type 的 C# 代码块内必须全 ASCII！
#  PowerShell 用 -File 执行时按系统 ANSI(GBK) 解码脚本文件，UTF-8 中文注释
#  会被解成乱码并喂给 C# 编译器，直接报 "Method must have a return type" 之类的
#  莫名其妙的编译错误。PS 段注释乱码无所谓（不影响执行），C# 段不行。

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrProbe {
    [DllImport("kernel32.dll")] public static extern uint WTSGetActiveConsoleSessionId();
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern IntPtr GetShellWindow();
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    public delegate bool EnumCb(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    public static string Title(IntPtr h) {
        StringBuilder sb = new StringBuilder(256);
        GetWindowText(h, sb, 256);
        return sb.ToString();
    }
    public static int CountVisibleWindows() {
        int n = 0;
        EnumWindows(delegate(IntPtr h, IntPtr l) { if (IsWindowVisible(h) && Title(h).Length > 0) n++; return true; }, IntPtr.Zero);
        return n;
    }
    // move cursor then read it back, all inside C# to avoid PS struct field issues
    public static string MoveAndReadBack(int x, int y) {
        POINT a; GetCursorPos(out a);
        bool ok = SetCursorPos(x, y);
        System.Threading.Thread.Sleep(200);
        POINT b; GetCursorPos(out b);
        return a.X + "," + a.Y + " -> " + b.X + "," + b.Y + " setOk=" + ok;
    }
}
"@

$out = "C:\Users\Public\pkg\_guiprobe.txt"
$lines = @()
$lines += "whoami=" + (whoami)
$lines += "procSessionId=" + (Get-Process -Id $PID).SessionId
$lines += "consoleSessionId=" + [BrProbe]::WTSGetActiveConsoleSessionId()
$lines += "visibleWindows=" + [BrProbe]::CountVisibleWindows()
$fg = [BrProbe]::GetForegroundWindow()
$lines += "foreground=[" + [BrProbe]::Title($fg) + "]"
$sh = [BrProbe]::GetShellWindow()
$lines += "shell=[" + [BrProbe]::Title($sh) + "]"
$lines += "cursorMove=" + [BrProbe]::MoveAndReadBack(1234, 567)

$lines -join "`r`n" | Set-Content -Path $out -Encoding ASCII
Write-Output "PROBE_DONE"
