# _t-probe-entry.ps1 - ASCII only. Runs ELEVATED in session 1 (via run-gui-token.ps1).
#
# Acceptance for P2 (GUI "Inspect" entry, 6th tab ID_OPERATION_PROBE=1107):
#   1. PostMessage(BM_CLICK) the tab; read BM_GETCHECK back (1 = really selected).
#      2026-10-26 fix: this file used 0x00F2, which is BM_GETSTATE, not BM_GETCHECK
#      (0x00F0). BM_GETSTATE returns check|pushed|focus = 1|8 = 9, so the assertion
#      "probe_checked=1" could never pass and the bug looked like it was in the GUI.
#      The product constant (native_gui.rs: BM_GETCHECK = 0x00F0) was right; only
#      this instrument was wrong. Older scripts (br-check-read.ps1, _t-gstate.ps1)
#      always had 0x00F0 -- do not "fix" them.
#   2. The Inspect page must hide the image/target/source rows (they belong to
#      real operations) - ASCII visibility flags are the real verdict here,
#      never window text (the prlctl channel mangles non-ASCII).
#   3. Click the backup tab again to leave the GUI as it was.
$out = 'H:\brwork\probe-entry-check.txt'
function Say($s) { $s | Add-Content -Path $out -Encoding UTF8 }
Set-Content -Path $out -Value ("CHECK_START " + (Get-Date -Format o)) -Encoding UTF8

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class PE {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public const uint BM_GETCHECK = 0x00F0;
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
'@

function Find-Win($cls) {
  $script:found = [IntPtr]::Zero
  $cb = [PE+EnumCb]{ param($h,$l) if ([PE]::Cls($h) -eq $cls) { $script:found = $h; return $false }; return $true }
  [void][PE]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

function Vis($parent, $id) {
  $h = [PE]::GetDlgItem($parent, $id)
  if ($h -eq [IntPtr]::Zero) { return "null" }
  if ([PE]::IsWindowVisible($h)) { return "1" } else { return "0" }
}

$gui = Find-Win 'BackupRestoreNativeGui'
if ($gui -eq [IntPtr]::Zero) { Say "GUI_NOT_FOUND"; exit 1 }

# Baseline: backup tab selected.
$backup = [PE]::GetDlgItem($gui, 1101)
$probe  = [PE]::GetDlgItem($gui, 1107)
if ($probe -eq [IntPtr]::Zero) { Say "ID_OPERATION_PROBE_CONTROL_NULL"; exit 2 }
Say ("before: backup_checked=" + [PE]::SendMessageW($backup, [PE]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero).ToInt64())
Say ("before: image_vis=" + (Vis $gui 1203) + " target_vis=" + (Vis $gui 1204) + " source_vis=" + (Vis $gui 1202))

[void][PE]::PostMessageW($probe, [PE]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 800
Say ("after: probe_checked=" + [PE]::SendMessageW($probe, [PE]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero).ToInt64())
Say ("after: backup_checked=" + [PE]::SendMessageW($backup, [PE]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero).ToInt64())
Say ("after: image_vis=" + (Vis $gui 1203) + " target_vis=" + (Vis $gui 1204) + " source_vis=" + (Vis $gui 1202))
Say ("after: create_task_vis=" + (Vis $gui 1003))

[void][PE]::PostMessageW($backup, [PE]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 600
Say ("restored: backup_checked=" + [PE]::SendMessageW($backup, [PE]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero).ToInt64())
Say ("restored: image_vis=" + (Vis $gui 1203) + " source_vis=" + (Vis $gui 1202))
Say "CHECK_END"
