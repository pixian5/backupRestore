# br-gui-state.ps1 -- read-only dump of the live BackupRestore GUI state.
# Must run ELEVATED in Session 1 (use tools/win-clicker/br-s1.sh).
# ASCII ONLY on purpose (PowerShell -File decodes scripts as system ANSI/GBK).
# Writes UTF-8 (no BOM) to C:\Users\Public\pkg\guistate.txt

param([string]$Note = "")

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrState {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr SendMessageStr(IntPtr h, uint msg, IntPtr wp, StringBuilder lp);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint tid, ref GUITHREADINFO gi);
  [DllImport("user32.dll", EntryPoint="GetWindowLongW")] public static extern int GetWindowLong(IntPtr h, int idx);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO {
    public int cbSize; public int flags; public IntPtr hwndActive; public IntPtr hwndFocus;
    public IntPtr hwndCapture; public IntPtr hwndMenuOwner; public IntPtr hwndMoveSize; public IntPtr hwndCaret;
    public int left; public int top; public int right; public int bottom;
  }
  public static string FocusOf(IntPtr root) {
    uint tid = GetWindowThreadProcessId(root, IntPtr.Zero);
    GUITHREADINFO gi = new GUITHREADINFO();
    gi.cbSize = Marshal.SizeOf(typeof(GUITHREADINFO));
    if (!GetGUIThreadInfo(tid, ref gi)) return "GUIThreadInfo_FAILED tid=" + tid;
    return "tid=" + tid + " active=" + gi.hwndActive + " focus=" + gi.hwndFocus
         + " focusId=" + GetDlgCtrlID(gi.hwndFocus) + " focusClass=" + Cls(gi.hwndFocus);
  }
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L; public int T; public int R; public int B; }
  public const uint CB_GETCOUNT=0x0146, CB_GETCURSEL=0x0147, CB_GETLBTEXT=0x0148, CB_GETLBTEXTLEN=0x0149, CB_SHOWDROPDOWN=0x014F;
  public const uint WM_GETTEXT=0x000D, WM_GETTEXTLENGTH=0x000E, BM_GETCHECK=0x00F0;
  public static string Text(IntPtr h) {
    StringBuilder sb = new StringBuilder(2048);
    GetWindowTextW(h, sb, 2048);
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
  public static string ComboInfo(IntPtr cb) {
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
  public static string EditText(IntPtr e) {
    if (e == IntPtr.Zero) return "NO_CONTROL";
    return Text(e);
  }
}
"@

$l = New-Object System.Collections.Generic.List[string]
function W($s) { [void]$l.Add($s) }

$hwnd = [BrState]::FindByTitle('BackupRestore - Rust GUI')
W ("NOTE=" + $Note)
W ("HWND=" + $hwnd + " title=[" + [BrState]::Text($hwnd) + "] session=" + (Get-Process -Id $PID).SessionId + " whoami=" + (whoami))
W ("foreground=[" + [BrState]::Text([BrState]::GetForegroundWindow()) + "]")
W ("focus=" + [BrState]::FocusOf($hwnd))

$ops = @(1100, 1101, 1102, 1103, 1104)
$opnames = @('PROBE', 'BACKUP', 'RESTORE', 'SECONDARY', 'PE')
W ""
W "== OPERATION RADIOS =="
for ($i = 0; $i -lt $ops.Count; $i++) {
  $h = [BrState]::GetDlgItem($hwnd, $ops[$i])
  $chk = [BrState]::SendMessageW($h, [BrState]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero)
  $r = [BrState+RECT]::new()
  [void][BrState]::GetWindowRect($h, [ref]$r)
  W ("  id=" + $ops[$i] + " (" + $opnames[$i] + ") visible=" + [BrState]::IsWindowVisible($h) + " check=" + $chk + " center=" + [int](($r.L+$r.R)/2) + "," + [int](($r.T+$r.B)/2))
}

$combos = @{ 1005='LANGUAGE'; 1202='SOURCE'; 1204='TARGET'; 1206='INDEX'; 1208='COMPRESS' }
W ""
W "== COMBOBOXES =="
foreach ($id in ($combos.Keys | Sort-Object)) {
  $h = [BrState]::GetDlgItem($hwnd, $id)
  $r = [BrState+RECT]::new()
  [void][BrState]::GetWindowRect($h, [ref]$r)
  W ("  id=" + $id + " (" + $combos[$id] + ") visible=" + [BrState]::IsWindowVisible($h) + " rect=" + $r.L + "," + $r.T + "," + $r.R + "," + $r.B + " center=" + [int](($r.L+$r.R)/2) + "," + [int](($r.T+$r.B)/2))
  W ("      " + [BrState]::ComboInfo($h))
}

$edits = @{ 1203='IMAGE_PATH'; 1207='SECONDARY_NAME'; 1209='INDEX_NAME'; 1210='KEEP_N'; 1300='STATUS' }
W ""
W "== EDITS / STATIC =="
foreach ($id in ($edits.Keys | Sort-Object)) {
  $h = [BrState]::GetDlgItem($hwnd, $id)
  $r = [BrState+RECT]::new()
  [void][BrState]::GetWindowRect($h, [ref]$r)
  W ("  id=" + $id + " (" + $edits[$id] + ") visible=" + [BrState]::IsWindowVisible($h) + " style=0x" + ([BrState]::GetWindowLong($h, -16)).ToString("X8") + " rect=" + $r.L + "," + $r.T + "," + $r.R + "," + $r.B + " center=" + [int](($r.L+$r.R)/2) + "," + [int](($r.T+$r.B)/2))
  W ("      text=[" + [BrState]::EditText($h) + "]")
}

$btns = @(1001, 1002, 1003, 1004, 1006)
W ""
W "== BUTTONS =="
foreach ($id in $btns) {
  $h = [BrState]::GetDlgItem($hwnd, $id)
  $r = [BrState+RECT]::new()
  [void][BrState]::GetWindowRect($h, [ref]$r)
  W ("  id=" + $id + " text=[" + [BrState]::Text($h) + "] visible=" + [BrState]::IsWindowVisible($h) + " rect=" + $r.L + "," + $r.T + "," + $r.R + "," + $r.B + " center=" + [int](($r.L+$r.R)/2) + "," + [int](($r.T+$r.B)/2))
}

[System.IO.File]::WriteAllText("C:\Users\Public\pkg\guistate.txt", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output "GUISTATE_DONE"