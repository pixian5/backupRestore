# br-settings-apply2.ps1 -- drive the Settings dialog the way a user would:
# set the threshold text, pick a radio, press Save.
#
# ASCII ONLY (prlctl exec / scheduled-task launch must survive GBK decoding).
#
# Two output channels on purpose:
#   C:\Users\Public\apply-trace.txt                        -- local disk, always writable,
#                                                             written from the very first line
#                                                             so "did the script even start"
#                                                             is never a mystery again.
#   \\Mac\backupRestore\.test-artifacts\...\e2e2-settings-apply.txt -- the readable log.
#
# Target values come from C:\Users\Public\settings-target.txt (line 1 threshold, line 2 action).
# Controls are addressed by the ids the GUI created them with:
#   3100 threshold edit, 3101 auto radio, 3102 pause radio, 3103 save, 3104 cancel
$ErrorActionPreference = "Continue"
$trace = "C:\Users\Public\apply-trace.txt"
function T($s) {
    ((Get-Date -Format 'HH:mm:ss') + " " + $s) | Out-File -FilePath $trace -Append -Encoding UTF8
}
T "=== apply2 start ==="
T ("pid=" + $PID + " user=" + [Security.Principal.WindowsIdentity]::GetCurrent().Name +
   " elevated=" + (New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))

$out = "\\Mac\backupRestore\.test-artifacts\e2e2-settings-apply.txt"
$target = "C:\Users\Public\settings-target.txt"
T ("out=" + $out + " target=" + $target)
try {
    Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class BrApply {
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool SetWindowTextW(IntPtr h, string s);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern int IsDlgButtonChecked(IntPtr h, int id);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Text(IntPtr h) { var sb = new StringBuilder(1024); GetWindowTextW(h, sb, 1024); return sb.ToString(); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  // NOTE: FindWindowExW with a PowerShell $null title marshals as "" not NULL, so it
  // never matched the dialog (its title is the localized "程序设置"). Enumerate instead.
  public static IntPtr Find(string cls) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) { if (Cls(h) == cls) { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
}
"@
    T "add-type ok"
} catch {
    T ("add-type FAILED: " + $_.Exception.Message)
    exit 11
}
try {
    "=== settings apply2 probe ===" | Out-File -FilePath $out -Encoding UTF8
    T "wrote header to out"
} catch {
    T ("write out FAILED: " + $_.Exception.Message)
}
function W($s) { try { $s | Out-File -FilePath $out -Append -Encoding UTF8 } catch {} ; T $s }

$lines = [System.IO.File]::ReadAllLines($target)
$wantThreshold = $lines[0].Trim()
$wantAction = $lines[1].Trim()
W "target threshold=$wantThreshold action=$wantAction"
$dlg = [BrApply]::Find("BackupRestoreSettingsWindow")
W "dialog_hwnd=$dlg is_window=$([BrApply]::IsWindow($dlg))"
if ($dlg -eq [IntPtr]::Zero) { W "DIALOG_NOT_OPEN"; exit 2 }
$edit = [BrApply]::GetDlgItem($dlg, 3100)
W "edit_hwnd=$edit text_before=[$([BrApply]::Text($edit))]"
W "radio_before auto=$([BrApply]::IsDlgButtonChecked($dlg,3101)) pause=$([BrApply]::IsDlgButtonChecked($dlg,3102))"
$setOk = [BrApply]::SetWindowTextW($edit, $wantThreshold)
W "settext_ok=$setOk"
Start-Sleep -Milliseconds 300
W "edit text_after=[$([BrApply]::Text($edit))]"
# Pick the radio the way a click does: BM_CLICK on the wanted one; the dialog
# manager unchecks the other member of the group.
$radioId = 3101
if ($wantAction -eq "pause") { $radioId = 3102 }
$radio = [BrApply]::GetDlgItem($dlg, $radioId)
W "clicking radio id=$radioId hwnd=$radio"
[BrApply]::SendMessageW($radio, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
Start-Sleep -Milliseconds 500
W "radio_after auto=$([BrApply]::IsDlgButtonChecked($dlg,3101)) pause=$([BrApply]::IsDlgButtonChecked($dlg,3102))"
# Save = WM_COMMAND with control id 3103 (ID_SETTINGS_SAVE).
$posted = [BrApply]::PostMessageW($dlg, 0x111, [IntPtr]3103, [IntPtr]::Zero)
W "post_save=$posted"
Start-Sleep -Milliseconds 2000
W "dialog_still_open_after_save=$([BrApply]::IsWindow($dlg))"
W "DONE"
T "=== apply2 end ==="
