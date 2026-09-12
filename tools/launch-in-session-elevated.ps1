# Launch a PowerShell script in session 1 with the user's ELEVATED token.
param([string]$ScriptPath, [string]$LogPath)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32E {
    [DllImport("kernel32.dll")] public static extern uint WTSGetActiveConsoleSessionId();
    [DllImport("wtsapi32.dll")] public static extern bool WTSQueryUserToken(uint sessionId, out IntPtr phToken);
    [DllImport("advapi32.dll", SetLastError = true)] public static extern bool DuplicateTokenEx(
        IntPtr hExistingToken, uint dwDesiredAccess, IntPtr lpTokenAttributes,
        int ImpersonationLevel, int TokenType, out IntPtr phNewToken);
    [DllImport("advapi32.dll", SetLastError = true)] public static extern bool GetTokenInformation(
        IntPtr tokenHandle, int tokenInformationClass, IntPtr tokenInformation,
        uint tokenInformationLength, out uint returnLength);
    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)] public static extern bool CreateProcessAsUser(
        IntPtr hToken, string lpApplicationName, string lpCommandLine, IntPtr lpProcessAttributes,
        IntPtr lpThreadAttributes, bool bInheritHandles, uint dwCreationFlags, IntPtr lpEnvironment,
        string lpCurrentDirectory, ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr hObject);
    [DllImport("kernel32.dll")] public static extern IntPtr LocalFree(IntPtr hMem);
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
function Fail($msg) { $msg | Out-File $LogPath -Encoding ascii; exit 1 }
$sid = [Win32E]::WTSGetActiveConsoleSessionId()
$hToken = [IntPtr]::Zero
if (-not [Win32E]::WTSQueryUserToken($sid, [ref]$hToken)) { Fail "WTSQueryUserToken failed" }
# TokenLinkedToken = 18
$len = 0
[void][Win32E]::GetTokenInformation($hToken, 18, [IntPtr]::Zero, 0, [ref]$len)
$buf = [System.Runtime.InteropServices.Marshal]::AllocHGlobal([int]$len)
$ok = [Win32E]::GetTokenInformation($hToken, 18, $buf, $len, [ref]$len)
if (-not $ok) { Fail "GetTokenInformation(TokenLinkedToken) failed" }
$hLinked = [System.Runtime.InteropServices.Marshal]::ReadIntPtr($buf)
[void][Win32E]::LocalFree($buf)
Write-Output ("linked token=" + $hLinked)
$si = New-Object Win32E+STARTUPINFO
$si.cb = [System.Runtime.InteropServices.Marshal]::SizeOf($si)
$si.lpDesktop = "winsta0\default"
$pi = New-Object Win32E+PROCESS_INFORMATION
$cmd = "powershell -ExecutionPolicy Bypass -NoProfile -File `"$ScriptPath`""
$ok3 = [Win32E]::CreateProcessAsUser($hLinked, "C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe", $cmd, [IntPtr]::Zero, [IntPtr]::Zero, $false, 0x00000010, [IntPtr]::Zero, "C:\Users\Public\backupRestore-package-v12", [ref]$si, [ref]$pi)
"launch_result=$ok3 pid=$($pi.dwProcessId)" | Out-File $LogPath -Encoding ascii
[void][Win32E]::CloseHandle($hLinked)
[void][Win32E]::CloseHandle($hToken)
