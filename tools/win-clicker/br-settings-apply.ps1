# br-settings-apply.ps1 -- drive the Settings dialog the way a user would:
# set the threshold text, pick a radio, press Save. Reads the target values from
# a two-line text file on the shared drive (threshold, then action).
# Must run ELEVATED (same integrity level as the GUI) or UIPI blocks it.
# Controls are addressed by the ids the GUI created them with:
#   3100 threshold edit, 3101 auto radio, 3102 pause radio, 3103 save, 3104 cancel
$ErrorActionPreference = "Stop"
$out = "\\Mac\backupRestore\.test-artifacts\e2e2-settings-apply.txt"
$target = "\\Mac\backupRestore\.test-artifacts\settings-target.txt"
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class BrApply {
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool SetWindowTextW(IntPtr h, string s);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr p, IntPtr c, string cls, string title);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern int IsDlgButtonChecked(IntPtr h, int id);
  public static string Text(IntPtr h) { var sb = new StringBuilder(1024); GetWindowTextW(h, sb, 1024); return sb.ToString(); }
}
"@
function W($s) { $s | Out-File -FilePath $out -Append -Encoding UTF8 }
"=== settings apply probe ===" | Out-File -FilePath $out -Encoding UTF8
$lines = [System.IO.File]::ReadAllLines($target)
$wantThreshold = $lines[0].Trim()
$wantAction = $lines[1].Trim()
W "target threshold=$wantThreshold action=$wantAction"
$dlg = [BrApply]::FindWindowExW([IntPtr]::Zero, [IntPtr]::Zero, "BackupRestoreSettingsWindow", $null)
W "dialog_hwnd=$dlg is_window=$([BrApply]::IsWindow($dlg))"
if ($dlg -eq [IntPtr]::Zero) { W "DIALOG_NOT_OPEN"; exit 2 }
$edit = [BrApply]::GetDlgItem($dlg, 3100)
W "edit_hwnd=$edit text_before=[$([BrApply]::Text($edit))]"
W "radio_before auto=$([BrApply]::IsDlgButtonChecked($dlg,3101)) pause=$([BrApply]::IsDlgButtonChecked($dlg,3102))"
[BrApply]::SetWindowTextW($edit, $wantThreshold) | Out-Null
W "edit text_after=[$([BrApply]::Text($edit))]"
# Pick the radio the way a click does: BM_CLICK on the wanted one; the dialog
# manager unchecks the other member of the group.
$radioId = if ($wantAction -eq "pause") { 3102 } else { 3101 }
$radio = [BrApply]::GetDlgItem($dlg, $radioId)
W "clicking radio id=$radioId hwnd=$radio"
[BrApply]::SendMessageW($radio, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
Start-Sleep -Milliseconds 400
W "radio_after auto=$([BrApply]::IsDlgButtonChecked($dlg,3101)) pause=$([BrApply]::IsDlgButtonChecked($dlg,3102))"
# Save = WM_COMMAND with control id 3103 (ID_SETTINGS_SAVE).
$posted = [BrApply]::PostMessageW($dlg, 0x111, [IntPtr]3103, [IntPtr]::Zero)
W "post_save=$posted"
Start-Sleep -Milliseconds 1800
W "dialog_still_open_after_save=$([BrApply]::IsWindow($dlg))"
W "DONE"
