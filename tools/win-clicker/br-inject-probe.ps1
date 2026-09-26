# br-inject-probe.ps1 - check whether GUI input injection works in current context.
# ASCII ONLY on purpose: PowerShell -File decodes scripts as system ANSI (GBK on
# this VM), so any UTF-8 Chinese text (even in comments) becomes mojibake and,
# inside an Add-Type C# block, breaks the compiler.
# Output: C:\Users\Public\pkg\_inj.txt

param([string]$Tag = "run")

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class BrInj {
    [DllImport("kernel32.dll")] public static extern uint WTSGetActiveConsoleSessionId();
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern uint GetLastError();
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    public static string MoveAndReadBack(int x, int y) {
        POINT a; GetCursorPos(out a);
        bool ok = SetCursorPos(x, y);
        uint err = GetLastError();
        System.Threading.Thread.Sleep(200);
        POINT b; GetCursorPos(out b);
        return "before=" + a.X + "," + a.Y + " setOk=" + ok + " lastErr=" + err + " after=" + b.X + "," + b.Y;
    }
}
"@

$out = "C:\Users\Public\pkg\_inj.txt"
$lines = @()
$lines += "tag=" + $Tag
$lines += "whoami=" + (whoami)
$id = [Security.Principal.WindowsIdentity]::GetCurrent()
$pr = New-Object Security.Principal.WindowsPrincipal($id)
$lines += "elevated=" + $pr.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$lines += "sessionId=" + (Get-Process -Id $PID).SessionId
$lines += "consoleSession=" + [BrInj]::WTSGetActiveConsoleSessionId()
$lines += "foregroundHwnd=" + [BrInj]::GetForegroundWindow()
$lines += "cursor=" + [BrInj]::MoveAndReadBack(1234, 567)
$lines -join "`r`n" | Set-Content -Path $out -Encoding ASCII
Write-Output "INJ_DONE $Tag"
