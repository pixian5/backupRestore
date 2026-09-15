# post-probe.ps1 — report PostMessageW last error and integrity level
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class PP {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("kernel32.dll")] public static extern int GetLastError();
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
}
"@
$h = [PP]::FindWindowW([NullString]::Value, "BackupRestore - Rust GUI v1.5.10")
if ($h -eq [IntPtr]::Zero) { Set-Content "C:\Users\Public\backupRestore-package\post-probe.txt" "NO-WINDOW"; exit }
$pid2 = 0
[PP]::GetWindowThreadProcessId($h, [ref]$pid2) | Out-Null
$ok = [PP]::PostMessageW($h, 273, [IntPtr]1003, [IntPtr]::Zero)
$err = [PP]::GetLastError()
$who = whoami
$lines = @()
$lines += "hwnd=$h pid=$pid2 post=$ok lastError=$err isWindow=$([PP]::IsWindow($h)) whoami=$who"
# integrity level via process
$proc = Get-Process -Id $pid2
$lines += ("guiStartTime=" + $proc.StartTime.ToString("HH:mm:ss"))
# args via CIM
try { $lines += ("guiCmd=" + (Get-CimInstance Win32_Process -Filter "ProcessId=$pid2").CommandLine) } catch { $lines += "cim-err" }
Set-Content "C:\Users\Public\backupRestore-package\post-probe.txt" -Value $lines