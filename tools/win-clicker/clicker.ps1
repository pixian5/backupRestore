# clicker.ps1 —— VM 内鼠标/键盘注入执行器（零下载，用 Windows 自带 PowerShell + C#）
#
# 背景：macOS 宿主合成鼠标事件不被 Parallels 转发给 Guest（CGEvent/SmartMouse
# 限制），但"虚拟机内部程序自己点击"完全可行——点击发生在 Windows 系统内部，
# 不经过宿主。本脚本用 PowerShell Add-Type 编译 C#，调用 user32 的
# SetCursorPos + mouse_event / keybd_event，实现点击、双击、移动、按键。
#
# 用法（在 VM 内运行，需配合 schtasks /it 放到交互式桌面会话）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File clicker.ps1 -X 960 -Y 1440 -Click
#   powershell -NoProfile -ExecutionPolicy Bypass -File clicker.ps1 -Key 0x0D
# 坐标是虚拟机内部分辨率像素（VM 帧缓冲坐标，与 prlctl capture 截图一致）。
#
# 注意：prlctl exec 默认跑在 Session 0（无桌面），必须用
#   schtasks /create /ru x /rp 1 /it ... && schtasks /run ...
# 才能在用户交互会话（可见桌面）里注入。

param(
    [int]$X = 0,
    [int]$Y = 0,
    [switch]$Click,
    [switch]$Dbl,
    [switch]$Move,
    [string]$Key = "",
    [string]$Text = "",
    [string]$Chord = ""
)

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class BrInject {
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, UIntPtr dwExtraInfo);
    [DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
    [DllImport("user32.dll")] public static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public InputUnion U; }
    [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Explicit)] public struct InputUnion { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
    [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    public const uint LEFTDOWN = 0x0002;
    public const uint LEFTUP = 0x0004;
    public const uint KEYUP = 0x0002;
    public const uint KEYEVENTF_UNICODE = 0x0004;
    public const uint MOUSEEVENTF_LEFTDOWN = 0x0002;
    public const uint MOUSEEVENTF_LEFTUP = 0x0004;
    public static void Click(int x, int y) {
        SetCursorPos(x, y);
        System.Threading.Thread.Sleep(100);
        INPUT[] inp = new INPUT[2];
        inp[0].type = 0; // INPUT_MOUSE
        inp[0].U.mi.dx = 0; inp[0].U.mi.dy = 0;
        inp[0].U.mi.mouseData = 0; inp[0].U.mi.dwFlags = MOUSEEVENTF_LEFTDOWN;
        inp[0].U.mi.time = 0; inp[0].U.mi.dwExtraInfo = IntPtr.Zero;
        inp[1] = inp[0];
        inp[1].U.mi.dwFlags = MOUSEEVENTF_LEFTUP;
        SendInput(2, inp, Marshal.SizeOf(typeof(INPUT)));
    }
    public static void DblClick(int x, int y) {
        Click(x, y);
        System.Threading.Thread.Sleep(100);
        Click(x, y);
    }
    public static void Move(int x, int y) {
        SetCursorPos(x, y);
    }
    public static void Key(string name) {
        byte vk = Convert.ToByte(name.Replace("0x", ""), 16);
        keybd_event(vk, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(30);
        keybd_event(vk, 0, KEYUP, UIntPtr.Zero);
    }
    public static void Text(string s) {
        foreach (char c in s) {
            INPUT[] inp = new INPUT[2];
            inp[0].type = 1; // INPUT_KEYBOARD
            inp[0].U.ki.wVk = 0;
            inp[0].U.ki.wScan = (ushort)c;
            inp[0].U.ki.dwFlags = KEYEVENTF_UNICODE;
            inp[0].U.ki.time = 0;
            inp[0].U.ki.dwExtraInfo = IntPtr.Zero;
            inp[1] = inp[0];
            inp[1].U.ki.dwFlags = KEYEVENTF_UNICODE | KEYUP;
            SendInput(2, inp, Marshal.SizeOf(typeof(INPUT)));
            System.Threading.Thread.Sleep(10);
        }
    }
    public static void Chord(string s) {
        // chord syntax: "ctrl,p" / "alt,f4" / "shift,a". last part = main key.
        string[] parts = s.Split(',');
        if (parts.Length < 2) { Key(parts[0]); return; }
        byte[] vks = new byte[parts.Length];
        for (int i = 0; i < parts.Length; i++) {
            string p = parts[i].Trim().ToLower();
            if (p == "ctrl") { vks[i] = 0x11; }
            else if (p == "alt") { vks[i] = 0x12; }
            else if (p == "shift") { vks[i] = 0x10; }
            else if (p == "win") { vks[i] = 0x5B; }
            else if (p.Length == 1 && p[0] >= 'a' && p[0] <= 'z') { vks[i] = (byte)(p[0] - 32); } // letter -> VK (a=0x41)
            else { vks[i] = Convert.ToByte(p.Replace("0x", ""), 16); }
        }
        for (int i = 0; i < vks.Length - 1; i++) {
            keybd_event(vks[i], 0, 0, UIntPtr.Zero);  // press modifiers
            System.Threading.Thread.Sleep(30);
        }
        byte main = vks[vks.Length - 1];
        keybd_event(main, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(40);
        keybd_event(main, 0, KEYUP, UIntPtr.Zero);
        for (int i = vks.Length - 2; i >= 0; i--) {
            keybd_event(vks[i], 0, KEYUP, UIntPtr.Zero);  // release modifiers
            System.Threading.Thread.Sleep(20);
        }
    }
}
"@
$ErrorActionPreference = 'Stop'
$LogPath = 'C:\Users\Public\backupRestore-package\clicker-log.txt'
function Write-Log($msg) {
    try { Add-Content -Path $LogPath -Value ("{0} {1}" -f (Get-Date -Format 'HH:mm:ss.fff'), $msg) -ErrorAction SilentlyContinue } catch {}
}
Write-Log "start X=$X Y=$Y Click=$Click Dbl=$Dbl Move=$Move Key=$Key Text=$Text Chord=$Chord"
if ($Click) { [BrInject]::Click($X, $Y); Write-Log "clicked $X,$Y" }
elseif ($Dbl) { [BrInject]::DblClick($X, $Y); Write-Log "dblclicked $X,$Y" }
elseif ($Key -ne "") { [BrInject]::Key($Key); Write-Log "key $Key" }
elseif ($Text -ne "") { [BrInject]::Text($Text); Write-Log "text '$Text'" }
elseif ($Chord -ne "") { [BrInject]::Chord($Chord); Write-Log "chord $Chord" }
elseif ($Move) { [BrInject]::Move($X, $Y); Write-Log "moved $X,$Y" }
else { Write-Log "no-op" }
Write-Log "done"
