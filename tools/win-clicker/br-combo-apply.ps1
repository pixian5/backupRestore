# br-combo-apply.ps1 - set BackupRestore GUI ComboBox selections by control id using
# real Win32 ComboBox messages (CB_SETCURSEL + WM_COMMAND/CBN_SELCHANGE) from the SAME
# interactive session at High IL, then read the control's live count/cursel/items back.
# The app itself reads the compression choice through live CB_GETCURSEL, so the control
# state set here is exactly what the app will use for the task.
#
# Optional -BiCheck 1 switches the language combo zh<->en and re-reads the compress combo
# to prove both the item list and the index mapping survive a language change.
#
# ASCII ONLY on purpose (PowerShell -File decodes sources as system ANSI/GBK).
# Output: C:\Users\Public\pkg\combo-apply.txt  (UTF-8, no BOM)

param(
  [int]$Op = 0,        # 1100..1104 to switch operation radio, 0 = leave alone
  [int]$Source = -1,   # 1202 drive combo index
  [int]$Target = -1,   # 1204 drive combo index
  [int]$Index = -1,    # 1206 wim index combo
  [int]$Compress = -1, # 1208 compression combo index (0=compress/fast, 1=no compression/none)
  [int]$Lang = -1,     # 1005 language combo index
  [int]$BiCheck = 0,   # 1 = run the zh/en mapping check
  [string]$Note = ""
)

$ErrorActionPreference = 'Continue'

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrCA {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr SendMessageStr(IntPtr h, uint msg, IntPtr wp, StringBuilder lp);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint CB_GETCOUNT=0x0146, CB_GETCURSEL=0x0147, CB_GETLBTEXT=0x0148, CB_GETLBTEXTLEN=0x0149, CB_SETCURSEL=0x014E;
  public const uint WM_COMMAND=0x0111, BM_GETCHECK=0x00F0;
  public static string Text(IntPtr h) {
    if (h == IntPtr.Zero) return "";
    StringBuilder sb = new StringBuilder(2048);
    GetWindowTextW(h, sb, 2048);
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
}
"@

$pkg = "C:\Users\Public\pkg"
$l = New-Object System.Collections.Generic.List[string]
function W($s) { [void]$l.Add($s) }

$hwnd = [BrCA]::FindByTitle('BackupRestore - Rust GUI')
W ("NOTE=" + $Note)
W ("TIME=" + (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))
W ("HWND=" + $hwnd + " title=[" + [BrCA]::Text($hwnd) + "] session=" + (Get-Process -Id $PID).SessionId + " whoami=" + (whoami))
if ($hwnd -eq [IntPtr]::Zero) {
  [System.IO.File]::WriteAllText("$pkg\combo-apply.txt", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
  Write-Output "NO_WINDOW"; exit 1
}

function SetCombo([int]$id, [int]$idx) {
  $h = [BrCA]::GetDlgItem($hwnd, $id)
  if ($h -eq [IntPtr]::Zero) { W ("  SET id=" + $id + " NO_CONTROL"); return }
  [void][BrCA]::SendMessageW($h, [BrCA]::CB_SETCURSEL, [IntPtr]$idx, [IntPtr]::Zero)
  $wp = [IntPtr]($id -bor (1 -shl 16))   # CBN_SELCHANGE
  [void][BrCA]::SendMessageW($hwnd, [BrCA]::WM_COMMAND, $wp, $h)
  Start-Sleep -Milliseconds 600
  W ("  SET id=" + $id + " -> " + [BrCA]::ComboDump($h))
}
function SwitchOp([int]$id) {
  $h = [BrCA]::GetDlgItem($hwnd, $id)
  [void][BrCA]::SendMessageW($hwnd, [BrCA]::WM_COMMAND, [IntPtr]$id, $h)
  Start-Sleep -Milliseconds 700
  W ("  OP switch -> id=" + $id + " check=" + [BrCA]::SendMessageW($h, [BrCA]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero))
}

$names = @{ 1202='SOURCE'; 1204='TARGET'; 1206='INDEX'; 1208='COMPRESS'; 1005='LANGUAGE' }

W ""
W "== BEFORE =="
foreach ($id in @(1005, 1202, 1204, 1206, 1208)) {
  $h = [BrCA]::GetDlgItem($hwnd, $id)
  W ("  id=" + $id + " (" + $names[$id] + ") visible=" + [BrCA]::IsWindowVisible($h) + " " + [BrCA]::ComboDump($h))
}

W ""
W "== APPLY =="
if ($Op -ne 0)            { SwitchOp $Op }
if ($Lang -ge 0)          { SetCombo 1005 $Lang }
if ($Source -ge 0)        { SetCombo 1202 $Source }
if ($Target -ge 0)        { SetCombo 1204 $Target }
if ($Index -ge 0)         { SetCombo 1206 $Index }
if ($Compress -ge 0)      { SetCombo 1208 $Compress }

W ""
W "== AFTER =="
foreach ($id in @(1005, 1202, 1204, 1206, 1208)) {
  $h = [BrCA]::GetDlgItem($hwnd, $id)
  W ("  id=" + $id + " (" + $names[$id] + ") visible=" + [BrCA]::IsWindowVisible($h) + " " + [BrCA]::ComboDump($h))
}

if ($BiCheck -eq 1) {
  W ""
  W "== BILINGUAL CHECK (compress combo) =="
  $cur = [int]([BrCA]::SendMessageW([BrCA]::GetDlgItem($hwnd, 1208), [BrCA]::CB_GETCURSEL, [IntPtr]::Zero, [IntPtr]::Zero))
  W ("  keep compress cursel=" + $cur)
  W ("  [zh] lang=" + [BrCA]::ComboDump([BrCA]::GetDlgItem($hwnd, 1005)))
  W ("  [zh] compress=" + [BrCA]::ComboDump([BrCA]::GetDlgItem($hwnd, 1208)))
  SetCombo 1005 1
  W ("  [en] lang=" + [BrCA]::ComboDump([BrCA]::GetDlgItem($hwnd, 1005)))
  W ("  [en] compress=" + [BrCA]::ComboDump([BrCA]::GetDlgItem($hwnd, 1208)))
  SetCombo 1005 0
  W ("  [zh2] lang=" + [BrCA]::ComboDump([BrCA]::GetDlgItem($hwnd, 1005)))
  W ("  [zh2] compress=" + [BrCA]::ComboDump([BrCA]::GetDlgItem($hwnd, 1208)))
  SetCombo 1208 $cur
  W ("  restored compress cursel=" + $cur + " -> " + [BrCA]::ComboDump([BrCA]::GetDlgItem($hwnd, 1208)))
}

W ""
W "DONE"
[System.IO.File]::WriteAllText("$pkg\combo-apply.txt", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output "COMBO_APPLY_DONE"