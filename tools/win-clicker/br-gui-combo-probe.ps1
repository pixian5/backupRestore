# br-gui-combo-probe.ps1 - read the real compression ComboBox state via Win32 messages.
# Must run ELEVATED (High IL, Session 1) or UIPI/desktop checks fail:
#   powershell -File X:\tools\win-clicker\br-gui-exec.ps1 -Script br-gui-combo-probe.ps1 -Log combo.txt
# ASCII-only on purpose. Read-only except for switching the operation radio + language combo.
$ErrorActionPreference = 'Continue'

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrCombo {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr SendMessageStr(IntPtr h, uint msg, IntPtr wp, StringBuilder lp);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L; public int T; public int R; public int B; }
  public const uint CB_GETCOUNT=0x0146, CB_GETCURSEL=0x0147, CB_GETLBTEXT=0x0148, CB_GETLBTEXTLEN=0x0149, CB_SETCURSEL=0x014E;
  public const uint WM_COMMAND=0x0111, BM_GETCHECK=0x00F0, BM_SETCHECK=0x00F1;
  public static string Text(IntPtr h) {
    StringBuilder sb = new StringBuilder(512);
    GetWindowTextW(h, sb, 512);
    return sb.ToString();
  }
  public static string Cls(IntPtr h) {
    StringBuilder sb = new StringBuilder(128);
    GetClassNameW(h, sb, 128);
    return sb.ToString();
  }
  public static IntPtr FindByTitle(string needle) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      string t = Text(h);
      if (t != null && t.IndexOf(needle, StringComparison.OrdinalIgnoreCase) >= 0) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static string ComboDump(IntPtr cb) {
    if (cb == IntPtr.Zero) return "NO_CONTROL";
    int n = (int)SendMessageW(cb, CB_GETCOUNT, IntPtr.Zero, IntPtr.Zero);
    int cur = (int)SendMessageW(cb, CB_GETCURSEL, IntPtr.Zero, IntPtr.Zero);
    StringBuilder sb = new StringBuilder();
    sb.Append("count=" + n + " cursel=" + cur + " selectedText=[" + Text(cb) + "] items=[");
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
  public static string EnumChildCombo(IntPtr parent) {
    StringBuilder sb = new StringBuilder();
    EnumChildWindows(parent, delegate(IntPtr h, IntPtr l) {
      if (Cls(h) == "ComboBox") {
        sb.Append("  CHILD_COMBO id=" + GetDlgCtrlID(h) + " visible=" + IsWindowVisible(h) + " " + ComboDump(h) + "\r\n");
      }
      return true;
    }, IntPtr.Zero);
    return sb.ToString();
  }
}
"@

$ids = @{ 1100 = 'OP_PROBE'; 1101 = 'OP_BACKUP'; 1102 = 'OP_RESTORE'; 1103 = 'OP_SECONDARY'; 1104 = 'OP_PE'; 1005 = 'LANGUAGE'; 1202 = 'SOURCE'; 1203 = 'IMAGE'; 1204 = 'TARGET'; 1206 = 'INDEX'; 1207 = 'MENU'; 1208 = 'COMPRESS' }

$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }

$hwnd = [BrCombo]::FindByTitle('BackupRestore - Rust GUI')
W ("HWND=" + $hwnd + " title=[" + [BrCombo]::Text($hwnd) + "]")
W ("procSessionId=" + (Get-Process -Id $PID).SessionId + " whoami=" + (whoami))
W ("foregroundBefore=[" + [BrCombo]::Text([BrCombo]::GetForegroundWindow()) + "]")
[void][BrCombo]::ShowWindow($hwnd, 9)   # SW_RESTORE
[void][BrCombo]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 400
W ("foregroundAfter=[" + [BrCombo]::Text([BrCombo]::GetForegroundWindow()) + "]")

W ""
W "== RADIO / CONTROL STATE (before switch) =="
foreach ($id in ($ids.Keys | Sort-Object)) {
  $h = [BrCombo]::GetDlgItem($hwnd, $id)
  $chk = [BrCombo]::SendMessageW($h, [BrCombo]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero)
  W ("  id=" + $id + " (" + $ids[$id] + ") hwnd=" + $h + " class=" + [BrCombo]::Cls($h) + " visible=" + [BrCombo]::IsWindowVisible($h) + " check=" + $chk)
}

function Switch-Op($hwnd, $id) {
  $h = [BrCombo]::GetDlgItem($hwnd, $id)
  [void][BrCombo]::SendMessageW($hwnd, [BrCombo]::WM_COMMAND, [IntPtr]$id, $h)
  Start-Sleep -Milliseconds 500
}
function Set-Lang($hwnd, $idx) {
  $h = [BrCombo]::GetDlgItem($hwnd, 1005)
  [void][BrCombo]::SendMessageW($h, [BrCombo]::CB_SETCURSEL, [IntPtr]$idx, [IntPtr]::Zero)
  $wp = [IntPtr](1005 -bor (1 -shl 16))   # CBN_SELCHANGE
  [void][BrCombo]::SendMessageW($hwnd, [BrCombo]::WM_COMMAND, $wp, $h)
  Start-Sleep -Milliseconds 700
}
function Dump-Compress($hwnd, $tag) {
  W ""
  W ("== COMPRESS COMBO (" + $tag + ") ==")
  $cb = [BrCombo]::GetDlgItem($hwnd, 1208)
  W ("  hwnd=" + $cb + " class=" + [BrCombo]::Cls($cb) + " visible=" + [BrCombo]::IsWindowVisible($cb))
  W ("  " + [BrCombo]::ComboDump($cb))
  $rect = [BrCombo+RECT]::new()
  if ($cb -ne [IntPtr]::Zero) {
    [void][BrCombo]::GetWindowRect($cb, [ref]$rect)
    W ("  rect_screen=" + $rect.L + "," + $rect.T + "," + $rect.R + "," + $rect.B)
  }
  W ("  CHILD COMBOS OF MAIN WINDOW:")
  W ([BrCombo]::EnumChildCombo($hwnd))
}
function Dump-Coords($hwnd, $tag) {
  W ("-- click coords (" + $tag + ") --")
  foreach ($id in @(1100, 1101, 1102, 1103, 1104, 1208)) {
    $h = [BrCombo]::GetDlgItem($hwnd, $id)
    if ($h -eq [IntPtr]::Zero) { continue }
    $rect = [BrCombo+RECT]::new()
    [void][BrCombo]::GetWindowRect($h, [ref]$rect)
    W ("  id=" + $id + " visible=" + [BrCombo]::IsWindowVisible($h) + " rect=" + $rect.L + "," + $rect.T + "," + $rect.R + "," + $rect.B + " center=" + [int](($rect.L + $rect.R) / 2) + "," + [int](($rect.T + $rect.B) / 2))
  }
}

Set-Lang $hwnd 0
Switch-Op $hwnd 1101
Dump-Compress $hwnd "zh / backup page"
Dump-Coords $hwnd "zh / backup page"

Set-Lang $hwnd 1
Dump-Compress $hwnd "en / backup page"

Set-Lang $hwnd 0
Dump-Compress $hwnd "zh again / backup page"

W ""
W "DONE"
$out = "C:\Users\Public\pkg\combo-probe.txt"
$lines -join "`r`n" | Set-Content -Path $out -Encoding ASCII
Write-Output ("PROBE_DONE -> " + $out)