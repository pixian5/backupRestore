# run-in-session.ps1 -- start a command inside an interactive session
# (default Session 1 = the logged-on desktop).
#
# Why: prlctl exec runs as SYSTEM in Session 0 (no desktop). GUI automation must
# be injected into the user-visible interactive desktop (Session 1, console
# session). This tool takes the interactive session's user token via
# WTSQueryUserToken, promotes it with DuplicateTokenEx, then CreateProcessAsUser
# on winsta0\default -- no downloads, only built-in Windows APIs (SYSTEM holds
# SeTcbPrivilege so it may call them).
#
# ASCII ONLY. The first version of this file carried Chinese comments and no BOM;
# PowerShell 5.1 then decoded the UTF-8 bytes with the system ANSI codepage and
# the tokenizer broke -- it reported "MissingExpressionAfterToken" on the param
# block and the tool could never run. Keep every byte ASCII.
#
# Usage (inside the VM, SYSTEM context):
#   powershell -NoProfile -ExecutionPolicy Bypass -File run-in-session.ps1 ^
#       -SessionId 1 -Command "powershell.exe -NoProfile -File C:\...\clicker.ps1 -X 100 -Y 100 -Click"
#
# prlctl exec strips double quotes, so a -Command containing spaces cannot be
# passed directly; point -CommandFile at a plain text file holding the full
# in-session command line instead.

param(
    [int]$SessionId = 1,
    [string]$Command = "",
    [string]$CommandFile = ""
)

if ($CommandFile -ne "") {
    if (-not (Test-Path -LiteralPath $CommandFile)) {
        Write-Output "MISSING_CMDFILE $CommandFile"; exit 4
    }
    $Command = ([System.IO.File]::ReadAllText($CommandFile)).Trim()
}
if ($Command -eq "") { Write-Output "EMPTY_COMMAND"; exit 5 }

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class SessionLauncher {
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool CloseHandle(IntPtr h);
    [DllImport("wtsapi32.dll", SetLastError=true)] static extern bool WTSQueryUserToken(uint sessionId, out IntPtr phToken);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool DuplicateTokenEx(IntPtr hExistingToken, uint dwDesiredAccess, IntPtr lpTokenAttributes, int ImpersonationLevel, int TokenType, out IntPtr phNewToken);
    [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern bool CreateProcessAsUser(IntPtr hToken, string lpApplicationName, StringBuilder lpCommandLine, IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles, uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory, ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);
    [DllImport("userenv.dll", SetLastError=true)] static extern bool CreateEnvironmentBlock(out IntPtr lpEnvironment, IntPtr hToken, bool bInherit);
    [DllImport("userenv.dll", SetLastError=true)] static extern bool DestroyEnvironmentBlock(IntPtr lpEnvironment);
    [StructLayout(LayoutKind.Sequential)] public struct STARTUPINFO { public int cb; public string lpReserved; public string lpDesktop; public string lpTitle; public int dwX; public int dwY; public int dwXSize; public int dwYSize; public int dwXCountChars; public int dwYCountChars; public int dwFillAttribute; public int dwFlags; public short wShowWindow; public short cbReserved2; public IntPtr lpReserved2; public IntPtr hStdInput; public IntPtr hStdOutput; public IntPtr hStdError; }
    [StructLayout(LayoutKind.Sequential)] public struct PROCESS_INFORMATION { public IntPtr hProcess; public IntPtr hThread; public int dwProcessId; public int dwThreadId; }
    const uint TOKEN_DUPLICATE = 0x0002;
    const uint TOKEN_ASSIGN_PRIMARY = 0x0001;
    const uint TOKEN_QUERY = 0x0008;
    const int SecurityImpersonation = 2;
    const int TokenPrimary = 1;
    const uint CREATE_NO_WINDOW = 0x08000000;  // console apps get no window, never steal focus
    const uint CREATE_UNICODE_ENVIRONMENT = 0x00000400;
    [DllImport("kernel32.dll", SetLastError=true)] static extern uint WaitForSingleObject(IntPtr h, uint ms);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetExitCodeProcess(IntPtr h, out uint code);
    public static int Launch(uint sessionId, string commandLine) {
        IntPtr token;
        if (!WTSQueryUserToken(sessionId, out token)) return -100 - Marshal.GetLastWin32Error();
        IntPtr primary;
        if (!DuplicateTokenEx(token, TOKEN_ASSIGN_PRIMARY|TOKEN_DUPLICATE|TOKEN_QUERY, IntPtr.Zero, SecurityImpersonation, TokenPrimary, out primary)) {
            CloseHandle(token);
            return -200 - Marshal.GetLastWin32Error();
        }
        STARTUPINFO si = new STARTUPINFO();
        si.cb = Marshal.SizeOf(typeof(STARTUPINFO));
        si.lpDesktop = null;  // let the token decide its default desktop
        PROCESS_INFORMATION pi;
        StringBuilder cmd = new StringBuilder(commandLine);
        // User env block + user dir: without them the child exits 0xC0000142.
        IntPtr env;
        CreateEnvironmentBlock(out env, primary, false);
        bool ok = CreateProcessAsUser(primary, null, cmd, IntPtr.Zero, IntPtr.Zero, false, CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT, env, "C:\\Users\\x", ref si, out pi);
        int err = ok ? 0 : Marshal.GetLastWin32Error();
        if (env != IntPtr.Zero) { DestroyEnvironmentBlock(env); }
        if (ok) {
            Console.WriteLine("pid=" + pi.dwProcessId);
            // wait 500ms: if process died early, report its exit code.
            if (WaitForSingleObject(pi.hProcess, 500) == 0) {
                uint exitCode;
                GetExitCodeProcess(pi.hProcess, out exitCode);
                Console.WriteLine("exited_early=" + exitCode);
            } else {
                Console.WriteLine("alive=1");
            }
            CloseHandle(pi.hProcess);
            CloseHandle(pi.hThread);
        } else {
            Console.WriteLine("create_failed=" + err);
        }
        CloseHandle(primary);
        CloseHandle(token);
        return err;
    }
}
"@
$ErrorActionPreference = 'Stop'
$code = [SessionLauncher]::Launch([uint32]$SessionId, $Command)
Write-Output "launch code=$code (0=OK)"
exit $code
