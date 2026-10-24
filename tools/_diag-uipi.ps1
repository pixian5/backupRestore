# _diag-uipi.ps1 - ASCII only. Is the GUI process elevated, and does a plain
# SendMessage from this (standard) process get blocked by UIPI?
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$OutputEncoding = [Text.Encoding]::UTF8

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Uipi {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentProcessId();
  [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
  [DllImport("advapi32.dll", SetLastError=true)] public static extern bool OpenProcessToken(IntPtr proc, uint access, out IntPtr token);
  [DllImport("advapi32.dll", SetLastError=true)] public static extern bool GetTokenInformation(IntPtr token, int cls, out uint info, uint len, out uint ret);
  [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
  public static IntPtr FindMain() {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => {
      if (!IsWindowVisible(h)) return true;
      StringBuilder sb = new StringBuilder(512);
      GetWindowText(h, sb, 512);
      if (sb.ToString().StartsWith("BackupRestore - Rust GUI")) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static string Elevation(uint pid) {
    IntPtr p = OpenProcess(0x1000, false, pid);   // QUERY_LIMITED_INFORMATION
    if (p == IntPtr.Zero) return "OPEN_FAILED err=" + Marshal.GetLastWin32Error();
    IntPtr t;
    if (!OpenProcessToken(p, 0x0008, out t)) { CloseHandle(p); return "TOKEN_FAILED err=" + Marshal.GetLastWin32Error(); }
    uint info, ret;
    bool ok = GetTokenInformation(t, 20 /*TokenElevation*/, out info, 4, out ret);
    CloseHandle(t); CloseHandle(p);
    if (!ok) return "INFO_FAILED err=" + Marshal.GetLastWin32Error();
    return "elevated=" + (info != 0);
  }
  public static string Probe(IntPtr hwnd, uint msg) {
    IntPtr result;
    IntPtr ok = SendMessageTimeout(hwnd, msg, IntPtr.Zero, IntPtr.Zero, 0x0002 /*SMTO_ABORTIFHUNG*/, 2000, out result);
    return "ok=" + ok + " result=" + result + " err=" + Marshal.GetLastWin32Error();
  }
}
'@

$main = [Uipi]::FindMain()
if ($main -eq [IntPtr]::Zero) { Write-Output "MAIN_NOT_FOUND"; exit 1 }
$procId = [uint32]0
Add-Type -MemberDefinition '[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);' -Name W2 -Namespace N2
[void][N2.W2]::GetWindowThreadProcessId($main, [ref]$procId)
Write-Output ("MAIN_PID=" + $procId)
Write-Output ("GUI_" + [Uipi]::Elevation($procId))
Write-Output ("SELF_" + [Uipi]::Elevation([Uipi]::GetCurrentProcessId()))
$combo = [Uipi]::GetDlgItem($main, 1005)
Write-Output ("COMBO=" + $combo)
Write-Output ("CB_GETCOUNT " + [Uipi]::Probe($combo, 0x0146))
Write-Output ("CB_GETCURSEL " + [Uipi]::Probe($combo, 0x0147))
