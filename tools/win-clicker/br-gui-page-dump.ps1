# br-gui-page-dump.ps1 - dump every child control of the BackupRestore main window
# after switching to a given operation page. Elevated required.
#   powershell -File X:\tools\win-clicker\br-gui-exec.ps1 -Script br-gui-page-dump.ps1 -B64 <b64("-Op 1101")> -Log pagedump.txt
# Log is written as UTF-8 (no ASCII loss) to C:\Users\Public\pkg\pagedump.txt
param([int]$Op = 1101, [int]$Lang = 0)
$ErrorActionPreference = 'Continue'
$Op = $Op
$Lang = $Lang

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrPage {
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr SendMessageStr(IntPtr h, uint msg, IntPtr wp, StringBuilder lp);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L; public int T; public int R; public int B; }
  public const uint CB_GETCOUNT=0x0146, CB_GETCURSEL=0x0147, CB_GETLBTEXT=0x0148, CB_GETLBTEXTLEN=0x0149, CB_SETCURSEL=0x014E;
  public const uint WM_COMMAND=0x0111, BM_GETCHECK=0x00F0;
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(128); GetClassNameW(h, sb, 128); return sb.ToString(); }
  public static IntPtr FindByTitle(string needle) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) { string t = Text(h); if (t != null && t.IndexOf(needle, StringComparison.OrdinalIgnoreCase) >= 0) { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  public static string Combo(IntPtr cb) {
    if (cb == IntPtr.Zero) return "NO_CONTROL";
    int n = (int)SendMessageW(cb, CB_GETCOUNT, IntPtr.Zero, IntPtr.Zero);
    int cur = (int)SendMessageW(cb, CB_GETCURSEL, IntPtr.Zero, IntPtr.Zero);
    StringBuilder sb = new StringBuilder();
    sb.Append("count=" + n + " cursel=" + cur + " items=[");
    for (int i = 0; i < n; i++) {
      int len = (int)SendMessageW(cb, CB_GETLBTEXTLEN, (IntPtr)i, IntPtr.Zero);
      StringBuilder t = new StringBuilder(len + 4);
      SendMessageStr(cb, CB_GETLBTEXT, (IntPtr)i, t);
      if (i > 0) sb.Append(" | ");
      sb.Append(t.ToString());
    }
    sb.Append("]");
    return sb.ToString();
  }
  public static string DumpAll(IntPtr parent) {
    StringBuilder sb = new StringBuilder();
    EnumChildWindows(parent, delegate(IntPtr h, IntPtr l) {
      RECT r; GetWindowRect(h, out r);
      string cls = Cls(h);
      string extra = "";
      if (cls == "ComboBox") extra = " " + Combo(h);
      else if (cls == "Button") extra = " check=" + SendMessageW(h, BM_GETCHECK, IntPtr.Zero, IntPtr.Zero);
      sb.Append("  id=" + GetDlgCtrlID(h) + " class=" + cls + " vis=" + IsWindowVisible(h) + " rect=" + r.L + "," + r.T + "," + r.R + "," + r.B + " text=[" + Text(h) + "]" + extra + "\r\n");
      return true;
    }, IntPtr.Zero);
    return sb.ToString();
  }
}
"@

$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add([string]$s) }

$hwnd = [BrPage]::FindByTitle('BackupRestore - Rust GUI')
W ("HWND=" + $hwnd + " title=[" + [BrPage]::Text($hwnd) + "]")
[void][BrPage]::ShowWindow($hwnd, 9)
[void][BrPage]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 300

# language
$langCb = [BrPage]::GetDlgItem($hwnd, 1005)
[void][BrPage]::SendMessageW($langCb, [BrPage]::CB_SETCURSEL, [IntPtr]$Lang, [IntPtr]::Zero)
[void][BrPage]::SendMessageW($hwnd, [BrPage]::WM_COMMAND, [IntPtr](1005 -bor (1 -shl 16)), $langCb)
Start-Sleep -Milliseconds 700

# operation page
$opBtn = [BrPage]::GetDlgItem($hwnd, $Op)
[void][BrPage]::SendMessageW($hwnd, [BrPage]::WM_COMMAND, [IntPtr]$Op, $opBtn)
Start-Sleep -Milliseconds 700

W ("lang_cursel=" + [BrPage]::SendMessageW($langCb, [BrPage]::CB_GETCURSEL, [IntPtr]::Zero, [IntPtr]::Zero))
W ("=== ALL CHILD CONTROLS (op=" + $Op + " lang=" + $Lang + ") ===")
W ([BrPage]::DumpAll($hwnd))
W "DONE"

$out = "C:\Users\Public\pkg\pagedump.txt"
[System.IO.File]::WriteAllText($out, ($lines -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output ("PAGEDUMP_DONE -> " + $out)