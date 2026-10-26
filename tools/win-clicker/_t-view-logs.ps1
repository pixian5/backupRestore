# _t-view-logs.ps1 - ASCII only. Runs ELEVATED in session 1 (via run-gui-token.ps1).
#
# Acceptance for P1-a (GUI "View logs..." entry):
#   1. PostMessage(BM_CLICK) the top-bar button ID_VIEW_LOGS (1010).
#   2. Look for the BackupRestoreLogsWindow top-level window.
#   3. Read the combo item count (CB_GETCOUNT) and the hint static text
#      (WM_GETTEXT) - both prove the dialog really listed the log files.
#   4. Read the log folder static text to prove it points at the shared root.
#   5. Close the dialog again (ESC -> WM_CLOSE) so the GUI is left usable.
#
# Results go to a FILE: the CreateProcessAsUser child has no inherited stdout.
$out = 'H:\brwork\logs-viewer-check.txt'
function Say($s) { $s | Add-Content -Path $out -Encoding UTF8 }
Set-Content -Path $out -Value ("CHECK_START " + (Get-Date -Format o)) -Encoding UTF8

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class VL {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, StringBuilder l);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint WM_CLOSE = 0x0010;
  public const uint BM_CLICK = 0x00F5;
  public const uint CB_GETCOUNT = 0x0146;
  public const uint WM_GETTEXT = 0x000D;
  public const uint WM_GETTEXTLENGTH = 0x000E;
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static string Txt(IntPtr h) {
    int len = (int)SendMessageW(h, WM_GETTEXTLENGTH, IntPtr.Zero, IntPtr.Zero);
    if (len <= 0) return "";
    StringBuilder sb = new StringBuilder(len + 8);
    SendMessageW(h, WM_GETTEXT, (IntPtr)(len + 8), sb);
    return sb.ToString();
  }
}
'@

function Find-Win($cls) {
  $script:found = [IntPtr]::Zero
  $cb = [VL+EnumCb]{ param($h,$l) if ([VL]::Cls($h) -eq $cls) { $script:found = $h; return $false }; return $true }
  [void][VL]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

$gui = Find-Win 'BackupRestoreNativeGui'
if ($gui -eq [IntPtr]::Zero) { Say "GUI_NOT_FOUND"; exit 1 }
Say ("gui_hwnd=" + $gui.ToInt64())

$btn = [VL]::GetDlgItem($gui, 1010)
if ($btn -eq [IntPtr]::Zero) { Say "ID_VIEW_LOGS_CONTROL_NULL"; exit 2 }
Say ("ID_VIEW_LOGS_button_text=[" + [VL]::Txt($btn) + "]")
[void][VL]::PostMessageW($btn, [VL]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 1500

$dlg = Find-Win 'BackupRestoreLogsWindow'
if ($dlg -eq [IntPtr]::Zero) { Say "LOGS_WINDOW_NOT_FOUND"; exit 3 }
Say ("LOGS_WINDOW_FOUND hwnd=" + $dlg.ToInt64())

$combo = [VL]::GetDlgItem($dlg, 3300)
$count = [VL]::SendMessageW($combo, [VL]::CB_GETCOUNT, [IntPtr]::Zero, [IntPtr]::Zero)
Say ("combo_count=" + $count.ToInt64())

$hint = [VL]::GetDlgItem($dlg, 3306)
Say ("hint_text=[" + [VL]::Txt($hint) + "]")

$detail = [VL]::GetDlgItem($dlg, 3301)
Say ("detail_text=[" + [VL]::Txt($detail) + "]")

# Leave the GUI usable: close the dialog.
[void][VL]::PostMessageW($dlg, [VL]::WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 500
$after = Find-Win 'BackupRestoreLogsWindow'
Say ("closed=" + [int]($after -eq [IntPtr]::Zero))
Say "CHECK_END"
