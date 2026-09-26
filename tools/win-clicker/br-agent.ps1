# br-agent.ps1 —— VM 内常驻的 GUI 自动化执行器（提权运行，用共享目录收命令）
#
# 设计要点：
#   * 常驻循环，Add-Type 的 C# 只编译一次 → 省掉每次调用的编译开销（这是延迟大头）
#   * 提权(High IL)运行 → 能向提权前台窗口注入，不被 UIPI 拦
#   * 命令/结果走 Parallels 共享目录的 UNC 路径（\\Mac\backupRestore\...），
#     不用网络端口：宿主直接写 macOS 本地文件，agent 轮询读，写回结果文件。
#     【关键发现】盘符 X: 在提权进程里不可见，但 UNC 路径 \\Mac\backupRestore 可以，
#     所以这里必须用 UNC，不能用 X:。
#   * 空闲超过 IdleTimeoutSec 自动退出，避免遗留常驻进程。
#
# 命令协议（每行一条）：
#   click <x> <y>        dbl <x> <y>        move <x> <y>
#   key <vk hex>         例：key 0x0D
#   chord <mod,key>      例：chord ctrl,p
#   text <字符串>        支持中文（SendInput UNICODE）
#   windows              列出窗口
#   cursor               返回当前光标位置
#   shot <相对文件名>    截图保存到共享目录
#   quit                 退出
#
# 【ASCII 约束】Add-Type 的 C# 代码块内必须全 ASCII：PowerShell -File 按 GBK 解码
# 脚本，UTF-8 中文注释会变乱码并让 C# 编译失败。

param(
    [string]$Share = "\\Mac\backupRestore\tools\win-clicker\_agent",
    [int]$PollMs = 60,
    [int]$IdleTimeoutSec = 900
)

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public class BrAgt {
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] p, int cb);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte sc, uint f, UIntPtr e);
    [DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LASTINPUTINFO li);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int n);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", EntryPoint = "GetWindowTextW", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", EntryPoint = "GetWindowTextLengthW", CharSet = CharSet.Unicode)] public static extern int GetWindowTextLength(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    public delegate bool EnumCb(IntPtr h, IntPtr l);
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] public struct LASTINPUTINFO { public uint cbSize, dwTime; }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public InputU U; }
    [StructLayout(LayoutKind.Explicit)] public struct InputU { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
    [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk, wScan; public uint dwFlags, time; public IntPtr extra; }
    const uint MOUSE_LEFTDOWN = 0x0002, MOUSE_LEFTUP = 0x0004;
    const uint KEYEVENTF_KEYUP = 0x0002, KEYEVENTF_UNICODE = 0x0004;
    public static void Down(int x, int y) { INPUT i = new INPUT(); i.type = 0; i.U.mi.dx = x; i.U.mi.dy = y; i.U.mi.dwFlags = MOUSE_LEFTDOWN; SendInput(1, new INPUT[] { i }, Marshal.SizeOf(typeof(INPUT))); }
    public static void Up(int x, int y) { INPUT i = new INPUT(); i.type = 0; i.U.mi.dx = x; i.U.mi.dy = y; i.U.mi.dwFlags = MOUSE_LEFTUP; SendInput(1, new INPUT[] { i }, Marshal.SizeOf(typeof(INPUT))); }
    public static void Click(int x, int y) { SetCursorPos(x, y); System.Threading.Thread.Sleep(30); Down(x, y); Up(x, y); }
    public static void Dbl(int x, int y) { Click(x, y); System.Threading.Thread.Sleep(60); Click(x, y); }
    public static void Vk(byte vk) { keybd_event(vk, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(20); keybd_event(vk, 0, KEYEVENTF_KEYUP, UIntPtr.Zero); }
    public static void ModDown(byte vk) { keybd_event(vk, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(25); }
    public static void ModUp(byte vk) { keybd_event(vk, 0, KEYEVENTF_KEYUP, UIntPtr.Zero); System.Threading.Thread.Sleep(20); }
    public static void Uni(string s) {
        foreach (char c in s) {
            INPUT[] a = new INPUT[2];
            a[0].type = 1; a[0].U.ki.wScan = (ushort)c; a[0].U.ki.dwFlags = KEYEVENTF_UNICODE;
            a[1].type = 1; a[1].U.ki.wScan = (ushort)c; a[1].U.ki.dwFlags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP;
            SendInput(2, a, Marshal.SizeOf(typeof(INPUT)));
            System.Threading.Thread.Sleep(8);
        }
    }
    public static uint LastTick() { LASTINPUTINFO li = new LASTINPUTINFO(); li.cbSize = (uint)Marshal.SizeOf(typeof(LASTINPUTINFO)); GetLastInputInfo(ref li); return li.dwTime; }
    public static string Cur() { POINT p; GetCursorPos(out p); return p.X + "," + p.Y; }
    public static List<string> Wins() {
        var res = new List<string>();
        IntPtr fg = GetForegroundWindow();
        EnumWindows(delegate(IntPtr h, IntPtr l) {
            if (!IsWindowVisible(h)) return true;
            int len = GetWindowTextLength(h);
            if (len <= 0) return true;
            StringBuilder sb = new StringBuilder(len + 2);
            GetWindowText(h, sb, len + 2);
            RECT r; GetWindowRect(h, out r);
            uint pid; GetWindowThreadProcessId(h, out pid);
            res.Add("hwnd=" + h.ToInt64() + " pid=" + pid + " rect=" + r.L + "," + r.T + "," + r.R + "," + r.B + " fg=" + (h == fg ? 1 : 0) + " title=[" + sb.ToString() + "]");
            return true;
        }, IntPtr.Zero);
        return res;
    }
}
"@

$beatFile = Join-Path $Share "heartbeat.txt"
if (-not (Test-Path $Share)) { New-Item -ItemType Directory -Path $Share -Force | Out-Null }

# 每条命令用唯一文件名（cmd.<id>.txt / res.<id>.txt）。
# 别用同一个文件名反复覆盖：SMB/共享目录客户端缓存会把旧内容返回好几秒，
# 实测同名覆盖的往返延迟超过 12 秒；换成新文件名后目录项是新的，立刻可见。
function Write-Res($id, $lines) {
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.AppendLine("id=$id")
    foreach ($l in $lines) { [void]$sb.AppendLine($l) }
    [void]$sb.AppendLine("END")
    $f = Join-Path $Share ("res." + $id + ".txt")
    [System.IO.File]::WriteAllText($f, $sb.ToString(), (New-Object System.Text.UTF8Encoding($false)))
}

Set-Content -Path $beatFile -Value "launch $(Get-Date -Format 'HH:mm:ss')" -Encoding ASCII
$shareOk = Test-Path $Share
if (-not $shareOk) { New-Item -ItemType Directory -Path $Share -Force | Out-Null }
Set-Content -Path $beatFile -Value @(
    "started $(Get-Date -Format 'HH:mm:ss')",
    "share=$Share",
    "shareExists=$shareOk",
    "elevated=" + ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator),
    "session=" + (Get-Process -Id $PID).SessionId
) -Encoding ASCII
$script:guiLoaded = $false
$done = @{}
$lastCmd = Get-Date
Write-Res "boot" @("agent=ready", "screen=" + [BrAgt]::GetSystemMetrics(0) + "x" + [BrAgt]::GetSystemMetrics(1))

while ($true) {
    $pending = @(Get-ChildItem -Path $Share -Filter "cmd.*.txt" -File -ErrorAction SilentlyContinue |
                 Where-Object { -not $done.ContainsKey($_.Name) } | Sort-Object Name)
    foreach ($cf in $pending) {
        $done[$cf.Name] = $true
        $id = $cf.Name -replace '^cmd\.', '' -replace '\.txt$', ''
        $raw = ""
        try { $raw = [System.IO.File]::ReadAllText($cf.FullName, (New-Object System.Text.UTF8Encoding($false))) } catch { }
        Remove-Item $cf.FullName -ErrorAction SilentlyContinue
        $lines = $raw -split "`r?`n" | Where-Object { $_.Trim().Length -gt 0 }
        if ($lines.Count -gt 0) {
                $lastCmd = Get-Date
                $out = @()
                foreach ($ln in $lines) {
                    $ln = $ln.Trim()
                    $op = ($ln -split '\s+', 2)[0]
                    $rest = ""
                    if ($ln.Length -gt $op.Length) { $rest = $ln.Substring($op.Length).Trim() }
                    $parts = $ln -split '\s+', 3
                    try {
                        switch -CaseSensitive ($op) {
                            'click'  { [BrAgt]::Click([int]$parts[1], [int]$parts[2]); $out += "clicked $($parts[1]),$($parts[2])" }
                            'dbl'    { [BrAgt]::Dbl([int]$parts[1], [int]$parts[2]); $out += "dblclicked" }
                            'move'   { [void][BrAgt]::SetCursorPos([int]$parts[1], [int]$parts[2]); $c = [BrAgt]::Cur(); $out += "moved cursor=$c" }
                            'key'    { $vk = [Convert]::ToByte($parts[1].Replace('0x',''), 16); [BrAgt]::Vk($vk); $out += "key $($parts[1])" }
                            'chord'  {
                                $ps = $parts[1] -split ','
                                $vks = @()
                                foreach ($p in $ps) {
                                    $m = $p.Trim().ToLower()
                                    if ($m -eq 'ctrl') { $vks += 0x11 }
                                    elseif ($m -eq 'alt') { $vks += 0x12 }
                                    elseif ($m -eq 'shift') { $vks += 0x10 }
                                    elseif ($m -eq 'win') { $vks += 0x5B }
                                    elseif ($m.Length -eq 1 -and $m -match '[a-z]') { $vks += ([byte][char]($m.ToUpper()[0])) }
                                    else { $vks += [Convert]::ToByte($m.Replace('0x',''), 16) }
                                }
                                for ($i = 0; $i -lt $vks.Count - 1; $i++) { [BrAgt]::ModDown([byte]$vks[$i]) }
                                [BrAgt]::Vk([byte]$vks[$vks.Count - 1])
                                for ($i = $vks.Count - 2; $i -ge 0; $i--) { [BrAgt]::ModUp([byte]$vks[$i]) }
                                $out += "chord $($parts[1])"
                            }
                            'text'   { [BrAgt]::Uni($rest); $n = $rest.Length; $out += "typed len=$n" }
                            'windows'{ $out += [BrAgt]::Wins() }
                            'cursor' { $c = [BrAgt]::Cur(); $out += "cursor=$c" }
                            'tick'   { $t = [BrAgt]::LastTick(); $out += "lastInputTick=$t" }
                            'shot'   {
                                if (-not $script:guiLoaded) {
                                    Add-Type -AssemblyName System.Drawing
                                    Add-Type -AssemblyName System.Windows.Forms
                                    $script:guiLoaded = $true
                                }
                                $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
                                $bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
                                $g = [System.Drawing.Graphics]::FromImage($bmp)
                                $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
                                $f = Join-Path $Share ($parts[1])
                                $bmp.Save($f, [System.Drawing.Imaging.ImageFormat]::Png)
                                $g.Dispose(); $bmp.Dispose()
                                $nm = $parts[1]; $szw = $b.Width; $szh = $b.Height; $out += "shot=$nm size=$szwx$szh"
                            }
                            'quit'   { Write-Res $id @("agent=bye"); exit 0 }
                            default  { $out += "ERR unknown op: $op" }
                        }
                    } catch { $out += "ERR " + $_.Exception.Message }
                }
                Write-Res $id $out
        }
    }
    $elapsed = ((Get-Date) - $lastCmd).TotalSeconds
    if ($elapsed -gt $IdleTimeoutSec) {
        Set-Content -Path $beatFile -Value "idle-exit $(Get-Date -Format 'HH:mm:ss')" -Encoding ASCII
        exit 0
    }
    Start-Sleep -Milliseconds $PollMs
}
