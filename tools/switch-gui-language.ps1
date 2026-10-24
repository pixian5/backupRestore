# switch-gui-language.ps1 - ASCII only.
#
# Switch the running GUI's language combo and prove it took effect by reading
# back a button caption before/after.
#
# PROVEN END-TO-END 2026-10-25 (v2.6.41):
#      target=1 -> BEFORE=[<zh caption>] AFTER=[Refresh environment]
#      target=0 -> BEFORE=[Refresh environment] AFTER=[<zh caption>]
#    gui.log: "GUI action: control=ID_LANGUAGE id=1005 notification=0x1"
#             "GUI action completed: language changed"
#
# MUST run ELEVATED IN SESSION 1. Two independent requirements:
#   1) Session: the GUI lives in session 1. `prlctl exec --current-user` already
#      lands there (verified: SessionId=1, user x), so no token dance is needed
#      just for the session.
#   2) Elevation: the GUI runs elevated (measured GUI_elevated=True) while
#      `--current-user` gives a STANDARD token. UIPI then silently blocks every
#      SendMessage -- measured CB_GETCOUNT -> GetLastError=5 ERROR_ACCESS_DENIED,
#      and the combo reads COUNT=0 / empty text even though the UI clearly shows
#      its Chinese caption. That fake "empty combo" is what made earlier
#      attempts look like a product bug. It is not: an elevated caller reads
#      COUNT=2 normally.
#   => launch through run-elevated-session1.ps1 (borrows the GUI's own elevated
#      token) instead of calling this file directly.
#
# Output goes to a FILE, never stdout: a process created via CreateProcessAsUser
# has no inherited stdout, so printed output vanishes (older attempts reported
# launch_result=True with "no output").
#
# Controls (native_gui.rs): ID_REFRESH=1001 (proof target), ID_LANGUAGE=1005.
# Messages: CB_GETCOUNT=0x146 CB_GETCURSEL=0x147 CB_GETLBTEXT=0x148
#           CB_SETCURSEL=0x14E WM_COMMAND=0x111 CBN_SELCHANGE=1
# Note: CB_SETCURSEL changes the selection but sends NO notification, so the
# CBN_SELCHANGE must be posted explicitly -- that is what triggers apply_language.
$out = 'H:\brwork\lang-switch-result.txt'
function W([string]$s) { Add-Content -Path $out -Value $s -Encoding UTF8 }

# Target combo index: 0 = Chinese, 1 = English. Default 1 (to English); write a
# number into lang-switch-target.txt to test the other direction.
$target = 1
if (Test-Path 'H:\brwork\lang-switch-target.txt') {
  $raw = (Get-Content 'H:\brwork\lang-switch-target.txt' -Raw).Trim()
  if ($raw -match '^\d+$') { $target = [int]$raw }
}

Set-Content -Path $out -Value ("RUN_START " + (Get-Date -Format o) + " target=" + $target) -Encoding UTF8
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
W ("SELF_ELEVATED=" + $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))
W ("SELF_SESSION=" + (Get-Process -Id $PID).SessionId)
W ("SELF_USER=" + [Environment]::UserName)

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class LS {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr Send(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendStr(IntPtr h, uint m, IntPtr w, StringBuilder l);
  [DllImport("user32.dll", EntryPoint="PostMessageW")] public static extern bool Post(IntPtr h, uint m, IntPtr w, IntPtr l);
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
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(1024); GetWindowText(h, sb, 1024); return sb.ToString(); }
  public static long Count(IntPtr h) { return (long)Send(h, 0x0146, IntPtr.Zero, IntPtr.Zero); }
  public static long CurSel(IntPtr h) { return (long)Send(h, 0x0147, IntPtr.Zero, IntPtr.Zero); }
  public static long SetCurSel(IntPtr h, int i) { return (long)Send(h, 0x014E, (IntPtr)i, IntPtr.Zero); }
  public static string Item(IntPtr h, int i) {
    StringBuilder sb = new StringBuilder(512);
    IntPtr r = SendStr(h, 0x0148, (IntPtr)i, sb);
    return (r == (IntPtr)(-1)) ? "<ERR>" : sb.ToString();
  }
}
'@

$main = [LS]::FindMain()
if ($main -eq [IntPtr]::Zero) { W "MAIN_NOT_FOUND"; exit 1 }
W ("MAIN=" + $main)

$btn = [LS]::GetDlgItem($main, 1001)
$combo = [LS]::GetDlgItem($main, 1005)
W ("BTN=" + $btn + " COMBO=" + $combo)
W ("BEFORE=[" + [LS]::Text($btn) + "]")
$count = [LS]::Count($combo)
W ("COUNT=" + $count + " CURSEL=" + [LS]::CurSel($combo))
for ($i = 0; $i -lt $count; $i++) { W ("ITEM[" + $i + "]=[" + [LS]::Item($combo, $i) + "]") }

if ($count -lt 2) { W "ABORT_COMBO_TOO_SHORT (COUNT<2: if this ran WITHOUT elevation it is UIPI, not a product bug)"; exit 2 }
if ($target -ge $count) { W ("ABORT_TARGET_OUT_OF_RANGE target=" + $target + " count=" + $count); exit 2 }

$prev = [LS]::SetCurSel($combo, $target)
W ("SETCURSEL target=" + $target + " prev=" + $prev + " now=" + [LS]::CurSel($combo))
# WM_COMMAND: LOWORD = control id, HIWORD = notification code (CBN_SELCHANGE = 1)
[void][LS]::Post($main, 0x0111, [IntPtr]((1 -shl 16) -bor 1005), $combo)
Start-Sleep -Seconds 2
W ("AFTER=[" + [LS]::Text($btn) + "]")
W ("AFTER_CURSEL=" + [LS]::CurSel($combo))
W "RUN_END"
