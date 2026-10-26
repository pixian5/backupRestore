# _t-image-admin.ps1 - ASCII only. Runs ELEVATED in session 1 (via run-gui-token.ps1).
#
# Acceptance for the 2026-10-26 split: the image-admin window moved out of
# native_gui.rs into native_gui/image_admin_window.rs as a CHILD module
# (use super::*). Compiling proves symbol resolution; this proves the runtime
# wiring still holds:
#   1. PostMessage(BM_CLICK) the top-bar button ID_IMAGE_ADMIN (1009).
#   2. Wait for window class BackupRestoreImageAdminWindow. The list is filled by a
#      WORKER THREAD that runs `list-images` over H:\BRImages, so this has to poll.
#   3. combo_count (3200) must equal the number of .wim files in that directory --
#      that is the judge that matters: the worker result travels through
#      IMAGE_ADMIN_LIST_RESULT, and the fill runs inside the moved module. If a
#      static had been duplicated instead of moved, this comes back 0.
#   4. hint (3207) must be non-empty: proves WM_CREATE still runs the moved code.
#   5. Select row 0 + post CBN_SELCHANGE -> detail (3201) must become non-empty:
#      proves the "show detail for the selected image" path still reaches the
#      moved combo/statics.
#   6. Click CLOSE (3206) and assert the window is gone.
#
# ZERO SIDE EFFECTS: no delete, no rename, no write. Only reads and one selection.
# Results go to a FILE: the CreateProcessAsUser child has no inherited stdout.
$out = 'H:\brwork\image-admin-check.txt'
function Say($s) { $s | Add-Content -Path $out -Encoding UTF8 }
Set-Content -Path $out -Value ("CHECK_START " + (Get-Date -Format o)) -Encoding UTF8

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class IA {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, StringBuilder l);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public const uint WM_GETTEXT = 0x000D;
  public const uint WM_GETTEXTLENGTH = 0x000E;
  public const uint CB_GETCOUNT = 0x0146;
  public const uint CB_SETCURSEL = 0x014E;
  public const uint WM_COMMAND = 0x0111;
  public const int CBN_SELCHANGE = 1;
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
  $cb = [IA+EnumCb]{ param($h,$l) if ([IA]::Cls($h) -eq $cls) { $script:found = $h; return $false }; return $true }
  [void][IA]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

$gui = Find-Win 'BackupRestoreNativeGui'
if ($gui -eq [IntPtr]::Zero) { Say "GUI_WINDOW_NOT_FOUND"; exit 1 }
Say ("gui_hwnd=" + $gui.ToInt64())

$btn = [IA]::GetDlgItem($gui, 1009)
if ($btn -eq [IntPtr]::Zero) { Say "ID_IMAGE_ADMIN_CONTROL_NULL"; exit 2 }
Say ("ID_IMAGE_ADMIN_button_text=[" + [IA]::Txt($btn) + "]")
[void][IA]::PostMessageW($btn, [IA]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)

# The window appears immediately; the combo fills when the worker posts its
# completion message. Poll for both.
$dlg = [IntPtr]::Zero
for ($i = 0; $i -lt 40; $i++) {
  Start-Sleep -Milliseconds 500
  $dlg = Find-Win 'BackupRestoreImageAdminWindow'
  if ($dlg -ne [IntPtr]::Zero) {
    $c = [IA]::SendMessageW([IA]::GetDlgItem($dlg, 3200), [IA]::CB_GETCOUNT, [IntPtr]::Zero, [IntPtr]::Zero).ToInt64()
    if ($c -gt 0) { break }
  }
}
if ($dlg -eq [IntPtr]::Zero) { Say "IMAGE_ADMIN_WINDOW_NOT_FOUND"; exit 3 }
Say ("IMAGE_ADMIN_WINDOW_FOUND hwnd=" + $dlg.ToInt64())
Say ("waited_rounds=" + $i)

$dir = 'H:\BRImages'
$wims = @(Get-ChildItem -LiteralPath $dir -Filter '*.wim' -File -ErrorAction SilentlyContinue)
Say ("wim_files_on_disk=" + $wims.Count)

$combo = [IA]::GetDlgItem($dlg, 3200)
$count = [IA]::SendMessageW($combo, [IA]::CB_GETCOUNT, [IntPtr]::Zero, [IntPtr]::Zero).ToInt64()
Say ("combo_count=" + $count)
Say ("combo_matches_disk=" + [int]($count -eq $wims.Count))

$hint = [IA]::Txt([IA]::GetDlgItem($dlg, 3207))
Say ("hint_len=" + $hint.Length)
Say ("hint_nonempty=" + [int]($hint.Length -gt 0))

# Select the first row and publish CBN_SELCHANGE: detail must be filled from the
# moved IMAGE_ADMIN_LIST_RESULT snapshot.
$beforeDetail = [IA]::Txt([IA]::GetDlgItem($dlg, 3201)).Length
if ($count -gt 0) {
  [void][IA]::SendMessageW($combo, [IA]::CB_SETCURSEL, [IntPtr]::Zero, [IntPtr]::Zero)
  [void][IA]::PostMessageW($dlg, [IA]::WM_COMMAND, [IntPtr](([IA]::CBN_SELCHANGE -shl 16) -bor 3200), $combo)
  Start-Sleep -Milliseconds 900
}
$afterDetail = [IA]::Txt([IA]::GetDlgItem($dlg, 3201)).Length
Say ("detail_len_before=" + $beforeDetail)
Say ("detail_len_after=" + $afterDetail)
Say ("detail_filled=" + [int]($afterDetail -gt 0))

$close = [IA]::GetDlgItem($dlg, 3206)
[void][IA]::PostMessageW($close, [IA]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 1500
$still = Find-Win 'BackupRestoreImageAdminWindow'
Say ("closed=" + [int]($still -eq [IntPtr]::Zero))

Say "CHECK_END"
