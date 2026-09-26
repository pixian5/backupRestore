# br-agent-tcp.ps1 —— VM 内常驻 GUI 自动化执行器（提权 + TCP 监听版）
#
# 与 br-agent.ps1 的区别：命令不走共享目录，直接开 TCP 端口收命令。
# 宿主用局域网 IP 直连（默认 10.211.55.x，端口 9124），往返 <5ms。
#
# 设计要点：
#   * 常驻监听，Add-Type 的 C# 只编译一次 → 省掉每次调用的编译开销（延迟大头）
#   * 提权(High IL)运行 → 能向提权前台窗口注入，不被 UIPI 拦
#   * 同一个连接内可连续发多条命令，每条以 END 行结束
#   * 空闲超过 IdleTimeoutSec 无连接自动退出，避免遗留常驻进程
#
# 命令协议（每行一条，UTF-8）：
#   click <x> <y>        dbl <x> <y>        move <x> <y>
#   key <vk hex>         例：key 0x0D
#   chord <mod,key>      例：chord ctrl,p
#   text <字符串>        支持中文（SendInput UNICODE）
#   windows              列出窗口
#   cursor               返回当前光标位置
#   tick                 返回 GetLastInputInfo tick（判断输入是否被接收）
#   shot                 截图，回传 base64（宿主侧存盘）
#   quit                 断开并退出
#
# 【ASCII 约束】Add-Type 的 C# 代码块内必须全 ASCII：PowerShell -File 按 GBK 解码
# 脚本，UTF-8 中文注释会变乱码并让 C# 编译失败。

param(
    [int]$Port = 9124,
    [int]$IdleTimeoutSec = 900,
    [int]$PollMs = 20
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

$script:guiLoaded = $false
$utf8 = New-Object System.Text.UTF8Encoding($false)

# 执行一行命令，返回字符串数组
function Invoke-Line($ln) {
    $ln = $ln.Trim()
    if ($ln.Length -eq 0) { return @() }
    $op = ($ln -split '\s+', 2)[0]
    $rest = ""
    if ($ln.Length -gt $op.Length) { $rest = $ln.Substring($op.Length).Trim() }
    $parts = $ln -split '\s+', 3
    $out = @()
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
            'screen' { $out += "screen=" + [BrAgt]::GetSystemMetrics(0) + "x" + [BrAgt]::GetSystemMetrics(1) }
            'exec'   {
                # exec <base64(utf8) PowerShell>
                # Runs arbitrary PowerShell in THIS already-elevated session and returns
                # stdout/stderr. Needed because `prlctl exec` (the Parallels Tools channel)
                # goes away when prl_tools_service is stopped, while this agent does not.
                # Output lines are prefixed with O| so they can never collide with END.
                # Keep callers' scripts SHORT: the host-side socket has a read timeout.
                $code = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($rest))
                $sb = [ScriptBlock]::Create($code)
                $res = @(& $sb 2>&1)
                $n = 0
                foreach ($r in $res) {
                    foreach ($sub in ([string]$r -split "`r?`n")) {
                        if ($n -lt 500) { $out += ("O|" + $sub) }
                        $n++
                    }
                }
                $out += ("O|__EXEC_LINES=" + $n + "__")
            }
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
                $ms = New-Object System.IO.MemoryStream
                $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
                $g.Dispose(); $bmp.Dispose()
                $b64 = [Convert]::ToBase64String($ms.ToArray())
                $ms.Dispose()
                $w = $b.Width; $h = $b.Height
                $out += "shot size=$w x $h b64len=" + $b64.Length
                $out += "B64:" + $b64
            }
            default  { $out += "ERR unknown op: $op" }
        }
    } catch { $out += "ERR " + $_.Exception.Message }
    return $out
}

$listener = New-Object System.Net.Sockets.TcpListener([System.Net.IPAddress]::Any, $Port)
$listener.Start()
Write-Output ("BRAGENT_LISTENING port=" + $Port + " elevated=" + ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) + " session=" + (Get-Process -Id $PID).SessionId)
$lastCmd = Get-Date

while ($true) {
    if (-not $listener.Pending()) {
        if (((Get-Date) - $lastCmd).TotalSeconds -gt $IdleTimeoutSec) {
            Write-Output "IDLE_EXIT"
            $listener.Stop()
            exit 0
        }
        Start-Sleep -Milliseconds $PollMs
        continue
    }

    $client = $listener.AcceptTcpClient()
    $lastCmd = Get-Date
    $stream = $client.GetStream()
    $stream.ReadTimeout = 300000
    $reader = New-Object System.IO.StreamReader($stream, $utf8, $false)
    $writer = New-Object System.IO.StreamWriter($stream, $utf8)
    $writer.AutoFlush = $true
    $writer.WriteLine("BRAGENT READY")

    $alive = $true
    while ($alive) {
        $ln = $null
        try { $ln = $reader.ReadLine() } catch { break }
        if ($null -eq $ln) { break }
        if ($ln.Trim().Length -eq 0) { continue }
        if ($ln.Trim().ToLower() -eq 'quit') { $writer.WriteLine("BYE"); $writer.WriteLine("END"); $alive = $false; break }
        $lastCmd = Get-Date
        $res = Invoke-Line $ln
        foreach ($r in $res) { $writer.WriteLine($r) }
        $writer.WriteLine("END")
    }
    try { $writer.Flush() } catch { }
    $client.Close()
    # 'quit' must actually terminate the agent: previously it only dropped the
    # client, so the outer accept loop kept the listener alive until the 900s idle
    # timeout and host-side `br-agent-tcp.sh stop` reported success while the
    # port stayed open (observed 2026-09-27). Close the listener and exit here.
    if (-not $alive) {
        try { $listener.Stop() } catch { }
        exit 0
    }
}
