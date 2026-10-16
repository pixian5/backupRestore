# br-settings-open.ps1 -- open the new Settings dialog (WM_COMMAND id 1007) in the
# interactive session and dump its child controls, so layout/clipping can be
# verified from evidence instead of eyeballing a screenshot.
$ErrorActionPreference = "Stop"
$out = "\\Mac\backupRestore\.test-artifacts\e2e2-settings-dump.txt"
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class BrSet {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static string Text(IntPtr h) { var sb = new StringBuilder(1024); GetWindowTextW(h, sb, 1024); return sb.ToString(); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static IntPtr Find(string cls) { IntPtr f = IntPtr.Zero; EnumWindows(delegate(IntPtr h, IntPtr l) { if (Cls(h) == cls) { f = h; return false; } return true; }, IntPtr.Zero); return f; }
}
"@
function W($s) { $s | Out-File -FilePath $out -Append -Encoding UTF8 }
"=== settings dialog probe ===" | Out-File -FilePath $out -Encoding UTF8
$main = [BrSet]::Find("BackupRestoreNativeGui")
W "main_hwnd=$main"
if ($main -eq [IntPtr]::Zero) { W "MAIN_NOT_FOUND"; exit 1 }
$mpid = 0
[BrSet]::GetWindowThreadProcessId($main, [ref]$mpid) | Out-Null
W "main_pid=$mpid self_pid=$PID"
$posted = [BrSet]::PostMessageW($main, 0x111, [IntPtr]1007, [IntPtr]::Zero)
W "post_wm_command_1007=$posted err=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
W "self_elevated=$((New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))"
W "self_user=$([Security.Principal.WindowsIdentity]::GetCurrent().Name)"
Start-Sleep -Milliseconds 1800
$dlg = [BrSet]::Find("BackupRestoreSettingsWindow")
W "dialog_hwnd=$dlg"
W "--- all top level windows ---"
$cbAll = [BrSet+EnumCb]{
  param($h, $l)
  $r = New-Object BrSet+RECT
  [BrSet]::GetWindowRect($h, [ref]$r) | Out-Null
  $p = 0
  [BrSet]::GetWindowThreadProcessId($h, [ref]$p) | Out-Null
  ("top pid={0} cls={1} vis={2} rect=({3},{4})-({5},{6}) text=[{7}]" -f $p, [BrSet]::Cls($h), [BrSet]::IsWindowVisible($h), $r.L, $r.T, $r.R, $r.B, [BrSet]::Text($h)) | Out-File -FilePath $out -Append -Encoding UTF8
  return $true
}
[BrSet]::EnumWindows($cbAll, [IntPtr]::Zero) | Out-Null
if ($dlg -eq [IntPtr]::Zero) { W "DIALOG_NOT_FOUND"; exit 2 }
$dr = New-Object BrSet+RECT
[BrSet]::GetWindowRect($dlg, [ref]$dr) | Out-Null
W ("dialog rect=({0},{1})-({2},{3}) size={4}x{5}" -f $dr.L, $dr.T, $dr.R, $dr.B, ($dr.R - $dr.L), ($dr.B - $dr.T))
$cb = [BrSet+EnumCb]{
  param($h, $l)
  $r = New-Object BrSet+RECT
  [BrSet]::GetWindowRect($h, [ref]$r) | Out-Null
  $line = "child id={0} cls={1} vis={2} rel=({3},{4}) size={5}x{6} text=[{7}]" -f [BrSet]::GetDlgCtrlID($h), [BrSet]::Cls($h), [BrSet]::IsWindowVisible($h), ($r.L - $dr.L), ($r.T - $dr.T), ($r.R - $r.L), ($r.B - $r.T), [BrSet]::Text($h)
  $line | Out-File -FilePath $out -Append -Encoding UTF8
  return $true
}
[BrSet]::EnumChildWindows($dlg, $cb, [IntPtr]::Zero) | Out-Null
W "DONE"
