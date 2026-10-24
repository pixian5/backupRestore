# switch-gui-language.ps1 - ASCII only.
# Switch the running GUI's language combo to English and report a button's text
# before/after, so we can prove the UI really re-renders in English.
#
# Self-contained (defines its own P/Invoke) so it can be run straight off the
# host share without dot-sourcing helper scripts that have side effects.
#
# Controls (native_gui.rs): ID_REFRESH = 1001, ID_LANGUAGE = 1005
#   ui_text(Chinese, "refresh") = "刷新环境"
#   ui_text(English, "refresh") = "Refresh environment"
# CBN_SELCHANGE = 1, CB_SETCURSEL = 0x014E, WM_COMMAND = 0x0111
#
# ⚠ STATUS: NOT YET PROVEN END-TO-END (2026-10-24). Two blockers were hit and
#   are recorded here so nobody re-derives them:
#   ① Session isolation: run from the prlctl SYSTEM channel (session 0) this
#      script reports MAIN_NOT_FOUND -- EnumWindows only sees windows in the
#      *caller's* session, and the GUI lives in session 1. It must be launched
#      through a session-1 helper (`launch-as-gui-token.ps1` /
#      `launch-in-session-elevated.ps1`).
#   ② ExecutionPolicy=Restricted in the VM: `& script.ps1` / `-File` throw
#      PSSecurityException (UnauthorizedAccess). Only in-memory execution works:
#      `Get-Content -Raw -Encoding UTF8 '<path>'; iex $c`. For scripts that take
#      param(), strip the `param(...)` line first and set the variables outside.
#   A session-1 run did return True but produced no BEFORE/AFTER output, and a
#   screenshot afterwards still showed Chinese ("语言" combo = "中文", button
#   "刷新环境"), so the switch did NOT take effect. Unresolved.

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class LangSwitch {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);

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
    public static string Text(IntPtr h) {
        StringBuilder sb = new StringBuilder(512);
        GetWindowText(h, sb, 512);
        return sb.ToString();
    }
}
'@

$main = [LangSwitch]::FindMain()
if ($main -eq [IntPtr]::Zero) { Write-Output "MAIN_NOT_FOUND"; exit 1 }

$btn = [LangSwitch]::GetDlgItem($main, 1001)
if ($btn -eq [IntPtr]::Zero) { Write-Output "BTN_NOT_FOUND"; exit 1 }
Write-Output ("BEFORE=[" + [LangSwitch]::Text($btn) + "]")

$combo = [LangSwitch]::GetDlgItem($main, 1005)
if ($combo -eq [IntPtr]::Zero) { Write-Output "COMBO_NOT_FOUND"; exit 1 }
[void][LangSwitch]::SendMessage($combo, 0x014E, [IntPtr]1, [IntPtr]::Zero)
[void][LangSwitch]::PostMessage($main, 0x0111, [IntPtr]((1 -shl 16) -bor 1005), $combo)
Start-Sleep -Seconds 2
Write-Output ("AFTER=[" + [LangSwitch]::Text($btn) + "]")
