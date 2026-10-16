# launch-as-gui-token.ps1 - ASCII only.
# Launch a script ELEVATED in session 1 on winsta0\default by borrowing the
# already-elevated GUI process's own token (BackupRestore.exe). This gives the
# child the same High integrity level as the GUI, so UIPI no longer drops
# BM_CLICK sent to the GUI's controls. Run this via the SYSTEM channel
# (prlctl exec "Windows 11" without --current-user) so OpenProcess/OpenProcessToken
# succeed with SeDebugPrivilege.
param([string]$ScriptPath, [string]$LogPath)

Add-Type @'
using System;
using System.Runtime.InteropServices;
public class Win32G {
  [DllImport("kernel32.dll")] public static extern IntPtr OpenProcess(uint dwDesiredAccess, bool bInheritHandle, uint dwProcessId);
  [DllImport("advapi32.dll", SetLastError=true)] public static extern bool OpenProcessToken(IntPtr hProcess, uint dwDesiredAccess, out IntPtr phToken);
  [DllImport("advapi32.dll", SetLastError=true)] public static extern bool DuplicateTokenEx(IntPtr hExistingToken, uint dwDesiredAccess, IntPtr lpTokenAttributes, int ImpersonationLevel, int TokenType, out IntPtr phNewToken);
  [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool CreateProcessAsUser(IntPtr hToken, string lpApplicationName, string lpCommandLine, IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles, uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory, ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);
  [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr hObject);
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct STARTUPINFO {
    public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
    public int dwX; public int dwY; public int dwXSize; public int dwYSize;
    public int dwXCountChars; public int dwYCountChars; public int dwFillAttribute; public int dwFlags;
    public short wShowWindow; public short cbReserved2; public IntPtr lpReserved2; public IntPtr hStdInput;
    public IntPtr hStdOutput; public IntPtr hStdError;
  }
  [StructLayout(LayoutKind.Sequential)] public struct PROCESS_INFORMATION {
    public IntPtr hProcess; public IntPtr hThread; public uint dwProcessId; public uint dwThreadId;
  }
}
'@

function Fail($m) { $m | Out-File $LogPath -Encoding ascii; exit 1 }

$proc = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Fail "BackupRestore.exe not running" }
$pidv = [uint32]$proc.Id
"gui-pid=$pidv" | Out-File $LogPath -Encoding ascii

$hProc = [Win32G]::OpenProcess(0x0400, $false, $pidv)
if ($hProc -eq [IntPtr]::Zero) { Fail ("OpenProcess failed err=" + [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()) }

$hTok = [IntPtr]::Zero
$ok = [Win32G]::OpenProcessToken($hProc, 0x0002 -bor 0x0008, [ref]$hTok)
if (-not $ok) { Fail ("OpenProcessToken failed err=" + [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()) }

$hPrimary = [IntPtr]::Zero
$okDup = [Win32G]::DuplicateTokenEx($hTok, 0x02000000, [IntPtr]::Zero, 2, 1, [ref]$hPrimary)
if (-not $okDup) { Fail ("DuplicateTokenEx failed err=" + [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()) }

$si = New-Object Win32G+STARTUPINFO
$si.cb = [System.Runtime.InteropServices.Marshal]::SizeOf($si)
$si.lpDesktop = "winsta0\default"
$pi = New-Object Win32G+PROCESS_INFORMATION
$cmd = "powershell -ExecutionPolicy Bypass -NoProfile -File `"$ScriptPath`""
$ok3 = [Win32G]::CreateProcessAsUser($hPrimary, "C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe", $cmd, [IntPtr]::Zero, [IntPtr]::Zero, $false, 0x00000010, [IntPtr]::Zero, "C:\Users\Public\backupRestore-package", [ref]$si, [ref]$pi)
if (-not $ok3) {
  "launch_result=False err=" + [System.Runtime.InteropServices.Marshal]::GetLastWin32Error() + " pid=" + $pi.dwProcessId | Out-File -Append $LogPath -Encoding ascii
} else {
  "launch_result=True pid=" + $pi.dwProcessId | Out-File -Append $LogPath -Encoding ascii
}
[Win32G]::CloseHandle($hPrimary)
[Win32G]::CloseHandle($hTok)
[Win32G]::CloseHandle($hProc)
