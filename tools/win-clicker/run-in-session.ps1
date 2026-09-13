# run-in-session.ps1 —— 在指定交互会话（默认 Session 1 = 已登录桌面）启动命令
#
# 背景：prlctl exec 以 SYSTEM 身份跑在 Session 0（无桌面）。GUI 自动化
# 必须注入到用户可见的交互桌面（Session 1，console 会话，用户 x 已登录）。
# 本工具用 WTSQueryUserToken 取交互会话的用户令牌，DuplicateTokenEx 提为
# 主令牌，再 CreateProcessAsUser 在 winsta0\default 桌面启动目标命令——
# 全程零下载，只用 Windows 自带 API（SYSTEM 拥有 SeTcbPrivilege，可调用）。
#
# 用法（VM 内，SYSTEM 上下文）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File run-in-session.ps1 ^
#       -SessionId 1 -Command "powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\...\clicker.ps1 -X 100 -Y 100 -Click"

param(
    [int]$SessionId = 1,
    [string]$Command = ""
)

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
