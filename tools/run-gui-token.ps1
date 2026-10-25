# run-gui-token.ps1 - ASCII only. PARAMETERIZED version of run-elevated-session1.ps1.
#
# Launch a PowerShell body script ELEVATED in the active console session, borrowing
# the running GUI's own token, so UIPI stops blocking SendMessage/PostMessage to
# the GUI's controls (2026-10-25: a --current-user standard token is silently
# rejected with ERROR_ACCESS_DENIED(5)).
#
# Run through the SYSTEM channel:
#   prlctl exec "Windows 11" powershell -NoProfile -Command "iex (Get-Content -Raw -Encoding UTF8 '<this file>'); ..."
# Simplest: give the two paths as literals by editing below, or call it with -File
# and edit $BodyPath/$LaunchLog. prlctl strips quotes, so no fancy quoting here.
#
# Two lessons baked in (2026-10-25):
#   1. The child has NO inherited stdout -> the body MUST write results to a file.
#   2. lpCurrentDirectory must be an existing dir (C:\Windows\Temp), else
#      CreateProcessAsUser fails with ERROR_DIRECTORY(267).
param(
    [string]$BodyPath  = '\\Mac\Home\code\backupRestore\tools\win-clicker\_t-view-logs.ps1',
    [string]$LaunchLog = 'H:\brwork\gui-token-launch.txt'
)

# Override hook: drop the body script's UNC path into H:\brwork\gui-token-body.txt
# (one line) before launching. prlctl mangles nested quoting, so passing the path
# as a command-line argument is not reliable; a file is.
$bodyHook = 'H:\brwork\gui-token-body.txt'
if (Test-Path -LiteralPath $bodyHook) {
    $hooked = (Get-Content -Raw -Encoding UTF8 $bodyHook).Trim()
    if ($hooked -ne '') { $BodyPath = $hooked }
}

Add-Type @'
using System;
using System.Runtime.InteropServices;
public class WTS {
  [DllImport("kernel32.dll")] public static extern uint WTSGetActiveConsoleSessionId();
  [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr OpenProcess(uint dwDesiredAccess, bool bInheritHandle, uint dwProcessId);
  [DllImport("advapi32.dll", SetLastError=true)] public static extern bool OpenProcessToken(IntPtr hProcess, uint dwDesiredAccess, out IntPtr phToken);
  [DllImport("advapi32.dll", SetLastError=true)] public static extern bool DuplicateTokenEx(
      IntPtr hExistingToken, uint dwDesiredAccess, IntPtr lpTokenAttributes,
      int ImpersonationLevel, int TokenType, out IntPtr phNewToken);
  [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool CreateProcessAsUser(
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
'@

function Say($s) { $s | Add-Content -Path $LaunchLog -Encoding UTF8 }
Set-Content -Path $LaunchLog -Value ("LAUNCH_START " + (Get-Date -Format o)) -Encoding UTF8

$sid = [WTS]::WTSGetActiveConsoleSessionId()
Say ("active_session=" + $sid)

$gui = Get-Process BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $gui) { Say "GUI_PROCESS_NOT_FOUND"; exit 1 }
Say ("gui_pid=" + $gui.Id + " gui_session=" + $gui.SessionId)

$hProc = [WTS]::OpenProcess(0x0400, $false, [uint32]$gui.Id)   # QUERY_INFORMATION
if ($hProc -eq [IntPtr]::Zero) {
  Say ("OpenProcess failed err=" + [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()); exit 1
}
$hToken = [IntPtr]::Zero
if (-not [WTS]::OpenProcessToken($hProc, 0x000B, [ref]$hToken)) {
  Say ("OpenProcessToken failed err=" + [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()); exit 1
}
$hPrimary = [IntPtr]::Zero
if (-not [WTS]::DuplicateTokenEx($hToken, 0x02000000, [IntPtr]::Zero, 2, 1, [ref]$hPrimary)) {
  Say ("DuplicateTokenEx failed err=" + [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()); exit 1
}
Say "primary token ok"

$si = New-Object WTS+STARTUPINFO
$si.cb = [System.Runtime.InteropServices.Marshal]::SizeOf($si)
$si.lpDesktop = "winsta0\default"
$pi = New-Object WTS+PROCESS_INFORMATION
$cmd = 'powershell -NoProfile -Command "iex (Get-Content -Raw -Encoding UTF8 ''' + $BodyPath + ''')"'
Say ("cmd=" + $cmd)
$ok = [WTS]::CreateProcessAsUser(
  $hPrimary,
  'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe',
  $cmd,
  [IntPtr]::Zero, [IntPtr]::Zero, $false,
  0x00000010,  # CREATE_NEW_CONSOLE
  [IntPtr]::Zero,
  'C:\Windows\Temp',
  [ref]$si, [ref]$pi)
if (-not $ok) {
  Say ("CreateProcessAsUser=False err=" + [System.Runtime.InteropServices.Marshal]::GetLastWin32Error())
} else {
  Say ("CreateProcessAsUser=True pid=" + $pi.dwProcessId)
}
[WTS]::CloseHandle($hPrimary)
[WTS]::CloseHandle($hToken)
Say "LAUNCH_END"
