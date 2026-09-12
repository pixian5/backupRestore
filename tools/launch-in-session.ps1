# Launch a PowerShell script in the interactive console session.
param([string]$ScriptPath, [string]$LogPath)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32L {
    [DllImport("kernel32.dll")] public static extern uint WTSGetActiveConsoleSessionId();
    [DllImport("wtsapi32.dll")] public static extern bool WTSQueryUserToken(uint sessionId, out IntPtr phToken);
    [DllImport("advapi32.dll", SetLastError = true)] public static extern bool DuplicateTokenEx(
        IntPtr hExistingToken, uint dwDesiredAccess, IntPtr lpTokenAttributes,
        int ImpersonationLevel, int TokenType, out IntPtr phNewToken);
    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)] public static extern bool CreateProcessAsUser(
        IntPtr hToken, string lpApplicationName, string lpCommandLine, IntPtr lpProcessAttributes,
        IntPtr lpThreadAttributes, bool bInheritHandles, uint dwCreationFlags, IntPtr lpEnvironment,
        string lpCurrentDirectory, ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr hObject);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] public struct STARTUPINFO {
        public int cb; public string lpReserved; public string lpDesktop;
        public string lpTitle; public int dwX; public int dwY; public int dwXSize; public int dwYSize;
        public int dwXCountChars; public int dwYCountChars; public int dwFillAttribute; public int dwFlags;
        public short wShowWindow; public short cbReserved2; public IntPtr lpReserved2; public IntPtr hStdInput;
        public IntPtr hStdOutput; public IntPtr hStdError;
    }
    [StructLayout(LayoutKind.Sequential)] public struct PROCESS_INFORMATION {
        public IntPtr hProcess; public IntPtr hThread; public uint dwProcessId; public uint dwThreadId;
    }
}
"@
$sid = [Win32L]::WTSGetActiveConsoleSessionId()
$hToken = [IntPtr]::Zero
$ok = [Win32L]::WTSQueryUserToken($sid, [ref]$hToken)
if (-not $ok) { "WTSQueryUserToken failed" | Out-File $LogPath -Encoding ascii; exit 1 }
$hDup = [IntPtr]::Zero
$ok2 = [Win32L]::DuplicateTokenEx($hToken, 0x02000000, [IntPtr]::Zero, 2, 1, [ref]$hDup)
if (-not $ok2) { "DuplicateTokenEx failed" | Out-File $LogPath -Encoding ascii; exit 1 }
$si = New-Object Win32L+STARTUPINFO
$si.cb = [System.Runtime.InteropServices.Marshal]::SizeOf($si)
$si.lpDesktop = "winsta0\default"
$pi = New-Object Win32L+PROCESS_INFORMATION
$cmd = "powershell -ExecutionPolicy Bypass -NoProfile -File `"$ScriptPath`""
$ok3 = [Win32L]::CreateProcessAsUser($hDup, "C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe", $cmd, [IntPtr]::Zero, [IntPtr]::Zero, $false, 0x00000010, [IntPtr]::Zero, "C:\Users\Public\backupRestore-package-v12", [ref]$si, [ref]$pi)
"launch_result=$ok3 pid=$($pi.dwProcessId)" | Out-File $LogPath -Encoding ascii
[void][Win32L]::CloseHandle($hDup)
[void][Win32L]::CloseHandle($hToken)
