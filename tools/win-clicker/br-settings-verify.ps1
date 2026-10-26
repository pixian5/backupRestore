# br-settings-verify.ps1 -- end-to-end check of the Settings dialog:
#   A) LOAD : open the dialog and read back what it pre-filled
#   B) SAVE : change the radio + the threshold text, press Save, then read the ini
#
# Must run ELEVATED on the interactive desktop (scheduled task, RunLevel Highest),
# otherwise UIPI silently drops the cross-process messages.
#
# ASCII ONLY.
$ErrorActionPreference = 'Continue'
$out = "\\Mac\backupRestore\.test-artifacts\e2e3\verify.txt"
function W($s) { $s | Out-File -FilePath $out -Append -Encoding UTF8 }
"=== settings verify ===" | Out-File -FilePath $out -Encoding UTF8

Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class BrV {
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern int IsDlgButtonChecked(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static IntPtr Find(string cls) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) { if (Cls(h) == cls && IsWindowVisible(h)) { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  // WM_GETTEXT is marshalled across processes (unlike GetWindowText, which returns
  // the cached caption for a foreign control). Always read EDIT text this way.
  public static string TextViaMessage(IntPtr h) {
    int len = (int)SendMessageW(h, 0x000E, IntPtr.Zero, IntPtr.Zero);
    int cap = Math.Max(len + 1, 64);
    IntPtr buf = Marshal.AllocHGlobal(cap * 2);
    try {
      SendMessageW(h, 0x000D, (IntPtr)cap, buf);
      return Marshal.PtrToStringUni(buf) ?? "";
    } finally { Marshal.FreeHGlobal(buf); }
  }
  public static void SetTextViaMessage(IntPtr h, string s) {
    IntPtr p = Marshal.StringToHGlobalUni(s);
    try { SendMessageW(h, 0x000C, IntPtr.Zero, p); } finally { Marshal.FreeHGlobal(p); }
  }
  public static string DumpClasses() {
    StringBuilder sb = new StringBuilder();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      if (IsWindowVisible(h)) sb.Append("[").Append(Cls(h)).Append("]");
      return true;
    }, IntPtr.Zero);
    return sb.ToString();
  }
}
"@

$elev = (New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
W ("elevated=" + $elev + " user=" + [Security.Principal.WindowsIdentity]::GetCurrent().Name)

function Show($tag) {
    $d = [BrV]::Find("BackupRestoreSettingsWindow")
    if ($d -eq [IntPtr]::Zero) { W ($tag + " dialog=NONE"); return }
    $e = [BrV]::GetDlgItem($d, 3100)
    W ($tag + " dialog=" + $d + " threshold_text=[" + [BrV]::TextViaMessage($e) + "] auto=" + [BrV]::IsDlgButtonChecked($d, 3101) + " pause=" + [BrV]::IsDlgButtonChecked($d, 3102))
}
function IniLine() {
    $p = "H:\brwork\BackupRestore.settings.ini"
    if (Test-Path -LiteralPath $p) {
        W "INI:"
        # -Encoding UTF8 mandatory: the default ANSI decode merges lines when a
        # UTF-8 char ending in a GBK-lead-looking byte precedes a newline
        # (2026-10-26: full-width `）` swallowed the following \n).
        Get-Content -LiteralPath $p -Encoding UTF8 | ForEach-Object { W ("  " + $_) }
    } else { W "INI: MISSING" }
}

$main = [BrV]::Find("BackupRestoreNativeGui")
W ("main=" + $main)
if ($main -eq [IntPtr]::Zero) { W "MAIN_NOT_FOUND"; exit 1 }

W "--- step A: open dialog, expect pre-filled 7 / pause ---"
IniLine
W ("open_posted=" + [BrV]::PostMessageW($main, 0x111, [IntPtr]1007, [IntPtr]::Zero))
Start-Sleep -Seconds 2
Show "A"

W "--- step B: set threshold 9, pick auto-escape, save ---"
$d = [BrV]::Find("BackupRestoreSettingsWindow")
if ($d -eq [IntPtr]::Zero) { W "DIALOG_GONE_BEFORE_B"; exit 2 }
$e = [BrV]::GetDlgItem($d, 3100)
[BrV]::SetTextViaMessage($e, "9")
Start-Sleep -Milliseconds 300
Show "B1(after settext)"
[BrV]::SendMessageW([BrV]::GetDlgItem($d, 3101), 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
Start-Sleep -Milliseconds 500
Show "B2(after radio)"
W ("save_posted=" + [BrV]::PostMessageW($d, 0x111, [IntPtr]3103, [IntPtr]::Zero))
Start-Sleep -Seconds 3
Show "B3(after save)"
W ("visible_classes=" + [BrV]::DumpClasses())
W "--- step C: read the ini back ---"
IniLine
W "DONE"
