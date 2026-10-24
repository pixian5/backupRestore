# run-elevated-session1.ps1 - ASCII only.
#
# Launch a PowerShell script ELEVATED in the active console session (session 1),
# so UIPI stops blocking SendMessage to the GUI's controls.
#
# Run this through the SYSTEM channel:
#   prlctl exec "Windows 11" powershell -NoProfile -Command "<iex this file>"
#
# Two lessons baked in (2026-10-25):
#   ① The child has NO inherited stdout. Anything it prints is lost forever --
#      that is why older attempts reported launch_result=True but no output.
#      The body script therefore writes its findings to a FILE.
#   ② Older helpers passed lpCurrentDirectory = C:\Users\Public\backupRestore-package,
#      which no longer exists (v2.6.34 stopped deploying to C:). A missing cwd
#      makes CreateProcessAsUser fail with ERROR_DIRECTORY(267). Use C:\Windows\Temp.
#
# Also note: prlctl exec --current-user already lands in session 1 but with a
# STANDARD token, which is exactly what UIPI blocks. Elevation is the part that
# actually matters.
$BodyPath   = '\\Mac\Home\code\backupRestore\tools\switch-gui-language.ps1'
$LaunchLog  = 'H:\brwork\lang-switch-launch.txt'

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
'@

function Say($s) { $s | Add-Content -Path $LaunchLog -Encoding UTF8 }
Set-Content -Path $LaunchLog -Value ("LAUNCH_START " + (Get-Date -Format o)) -Encoding UTF8

$sid = [WTS]::WTSGetActiveConsoleSessionId()
Say ("active_session=" + $sid)

# Borrow the running GUI's own elevated token. This is more robust than
# WTSQueryUserToken + TokenLinkedToken, which returned ERROR_INVALID_HANDLE(6)
# here (2026-10-25). SYSTEM has SeDebugPrivilege, so OpenProcess succeeds.
$gui = Get-Process BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $gui) { Say "GUI_PROCESS_NOT_FOUND"; exit 1 }
Say ("gui_pid=" + $gui.Id + " gui_session=" + $gui.SessionId)

$hProc = [WTS]::OpenProcess(0x0400, $false, [uint32]$gui.Id)   # QUERY_INFORMATION
if ($hProc -eq [IntPtr]::Zero) {
  Say ("OpenProcess failed err=" + [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()); exit 1
}
$hToken = [IntPtr]::Zero
# TOKEN_ASSIGN_PRIMARY(0x0001) | TOKEN_DUPLICATE(0x0002) | TOKEN_QUERY(0x0008)
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
# Single-quoted PS string: '' escapes one literal quote.
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
