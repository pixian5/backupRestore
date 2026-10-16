# br-settings-post.ps1 -- post WM_COMMAND id 1007 (Settings...) to the GUI window.
# Used when the caller already runs at a higher integrity level (SYSTEM via
# prlctl exec), because a non-elevated session-1 process is blocked by UIPI.
$ErrorActionPreference = "Stop"
$out = "\\Mac\backupRestore\.test-artifacts\e2e2-settings-post.txt"
$hwndFile = "\\Mac\backupRestore\.test-artifacts\gui-hwnd.txt"
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class BrPost {
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr FindWindowExW(IntPtr p, IntPtr c, string cls, string title);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static string Text(IntPtr h) { var sb = new StringBuilder(1024); GetWindowTextW(h, sb, 1024); return sb.ToString(); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
"@
function W($s) { $s | Out-File -FilePath $out -Append -Encoding UTF8 }
"=== settings post probe ===" | Out-File -FilePath $out -Encoding UTF8
$raw = ([System.IO.File]::ReadAllText($hwndFile)).Trim()
W "hwnd_file_raw=$raw"
$h = [IntPtr][int]$raw
W "hwnd=$h is_window=$([BrPost]::IsWindow($h))"
$r = New-Object BrPost+RECT
[BrPost]::GetWindowRect($h, [ref]$r) | Out-Null
W ("target rect=({0},{1})-({2},{3}) cls={4} text=[{5}]" -f $r.L, $r.T, $r.R, $r.B, [BrPost]::Cls($h), [BrPost]::Text($h))
$ok = [BrPost]::PostMessageW($h, 0x111, [IntPtr]1007, [IntPtr]::Zero)
W "post_wm_command_1007=$ok err=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
Start-Sleep -Milliseconds 1500
# The dialog is a top-level popup owned by the GUI; FindWindowEx over the
# desktop finds it even when the caller lives in another session.
$desk = [IntPtr]::Zero
$dlg = [BrPost]::FindWindowExW($desk, [IntPtr]::Zero, "BackupRestoreSettingsWindow", $null)
W "dialog_hwnd=$dlg"
if ($dlg -ne [IntPtr]::Zero) {
  $dr = New-Object BrPost+RECT
  [BrPost]::GetWindowRect($dlg, [ref]$dr) | Out-Null
  W ("dialog rect=({0},{1})-({2},{3}) size={4}x{5} text=[{6}]" -f $dr.L, $dr.T, $dr.R, $dr.B, ($dr.R - $dr.L), ($dr.B - $dr.T), [BrPost]::Text($dlg))
  $child = [IntPtr]::Zero
  $i = 0
  while ($true) {
    $child = [BrPost]::FindWindowExW($dlg, $child, $null, $null)
    if ($child -eq [IntPtr]::Zero) { break }
    $cr = New-Object BrPost+RECT
    [BrPost]::GetWindowRect($child, [ref]$cr) | Out-Null
    W ("child[{0}] cls={1} rel=({2},{3}) size={4}x{5} text=[{6}]" -f $i, [BrPost]::Cls($child), ($cr.L - $dr.L), ($cr.T - $dr.T), ($cr.R - $cr.L), ($cr.B - $cr.T), [BrPost]::Text($child))
    $i = $i + 1
  }
  W "child_count=$i"
}
W "DONE"
