# NOTE: keep this file ASCII-only (PowerShell 5 reads BOM-less files as ANSI).
# G4: is the error code actually VISIBLE somewhere the user can read it?
#
# ★ What the "refresh task status" button really does (read from the source,
#   then confirmed live): it refreshes the MAIN status box AND immediately opens
#   the "Tasks & evidence" window (class BackupRestoreTaskAdmin). That window is
#   hand-rolled modal: it calls EnableWindow(main, 0) and runs its own
#   PeekMessage loop until it is closed. Consequences for any judgement script:
#     - after one click the main window stays DISABLED (measured MAIN_ENABLED=
#       False), so later mouse clicks land nowhere;
#     - the code the manual tells the user to read lives in that window's detail
#       box (id 1402), not only in the main status box (id 1300).
#   So: close any leftover modal first, click once, then read BOTH boxes.
#
# ★ Why BM_CLICK is POSTed, not sent: SendMessage blocks until the app's GUI
#   thread pumps it; when that thread sits in one of those nested modal loops
#   the call hangs for tens of seconds and the verdict degenerates into silence.
#   PostMessage queues it and we poll the log for the resulting record, so a
#   busy app delays the verdict instead of destroying it.
#
# ★★ And why this reads with WM_GETTEXT instead of GetWindowTextW: see
#   GetTextLive below. A cross-process GetWindowTextW returns the text the
#   control was CREATED with, not what is on screen -- using it here produced
#   three rounds of "the UI hides the error code" that were pure artifact.
#
# Output (always ends with __END__):
#   VERDICT=VISIBLE_IN_BOTH / VISIBLE_IN_STATUS_ONLY /
#           VISIBLE_IN_TASK_WINDOW_ONLY / MISSING
#   VERDICT=NA reason=...            nothing was actually measured
param([int]$RefreshId = 1004, [int]$StatusId = 1300, [int]$DetailId = 1402)
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class EC {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeoutW(IntPtr h, uint m, IntPtr w, IntPtr l, uint f, uint t, out IntPtr r);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@
[void][EC]::SetProcessDPIAware()
$log = 'H:\brwork\logs\gui.log'
function FindWin([string]$cls) {
  $script:hit = [IntPtr]::Zero
  $script:cnt = 0
  $cb = [EC+Cb]{
    param([IntPtr]$h,[IntPtr]$l)
    $c = New-Object System.Text.StringBuilder 256
    [void][EC]::GetClassNameW($h, $c, 256)
    if ($c.ToString() -eq $cls) { $script:cnt++; if ($script:hit -eq [IntPtr]::Zero) { $script:hit = $h } }
    return $true
  }
  [void][EC]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:hit
}
function GetText($h) {
  # CACHED text (what GetWindowTextW gives for a window owned by another process).
  # Kept only to expose the trap; never used as a verdict.
  if ($h -eq [IntPtr]::Zero) { return '' }
  $sb = New-Object System.Text.StringBuilder 16384
  [void][EC]::GetWindowTextW($h, $sb, 16384)
  return $sb.ToString()
}
function GetTextLive($h) {
  # ★★ The single most important function in this file.
  # GetWindowTextW does NOT send WM_GETTEXT to a window owned by ANOTHER process:
  # it returns the window manager's cached copy, which is refreshed only when
  # DefWindowProc handles WM_SETTEXT. EDIT controls handle WM_SETTEXT themselves
  # and never touch that cache, so a cross-process GetWindowTextW on an EDIT
  # returns the TEXT IT WAS CREATED WITH -- forever. Measured here: the status
  # box visibly shows 8 lines ending in "Error code: BR-PE-039" (proved by a
  # screenshot), while GetWindowTextW still returned the 31-char creation-time
  # placeholder. Every "the UI hides the error code" verdict before this fix was
  # that artifact. So: send WM_GETTEXT explicitly and read the real buffer.
  if ($h -eq [IntPtr]::Zero) { return '' }
  $r = [IntPtr]::Zero
  if ([EC]::SendMessageTimeoutW($h, 0x000E, [IntPtr]::Zero, [IntPtr]::Zero, 0x0002, 2000, [ref]$r) -eq [IntPtr]::Zero) { return '' }
  $len = [int]$r.ToInt64()
  if ($len -le 0) { return '' }
  if ($len -gt 60000) { $len = 60000 }
  $buf = [System.Runtime.InteropServices.Marshal]::AllocHGlobal(($len + 2) * 2)
  try {
    $r2 = [IntPtr]::Zero
    if ([EC]::SendMessageTimeoutW($h, 0x000D, [IntPtr]::new($len + 1), $buf, 0x0002, 2000, [ref]$r2) -eq [IntPtr]::Zero) { return '' }
    return [System.Runtime.InteropServices.Marshal]::PtrToStringUni($buf)
  } finally {
    [System.Runtime.InteropServices.Marshal]::FreeHGlobal($buf)
  }
}
function CodeIn([string]$t) {
  if ($t -match 'BR-[A-Z]+-\d+') { return $matches[0] }
  return 'none'
}
# 0) close leftover modal windows so the main window is interactive again
$closed = 0
for ($i = 0; $i -lt 6; $i++) {
  $m = FindWin 'BackupRestoreTaskAdmin'
  if ($m -eq [IntPtr]::Zero) { break }
  [void][EC]::PostMessageW($m, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
  $closed++
  Start-Sleep -Milliseconds 700
}
"CLOSED_LEFTOVER_MODALS=$closed"

$win = FindWin 'BackupRestoreNativeGui'
if ($win -eq [IntPtr]::Zero) { 'VERDICT=NA reason=no_window'; '__END__'; exit 1 }
"MAIN_WINDOW_COUNT=$script:cnt"
"MAIN_ENABLED=$([EC]::IsWindowEnabled($win))"
$btn = [EC]::GetDlgItem($win, $RefreshId)
if ($btn -eq [IntPtr]::Zero) { 'VERDICT=NA reason=no_refresh_button'; '__END__'; exit 1 }
$br = New-Object EC+RECT
[void][EC]::GetWindowRect($btn, [ref]$br)
if ($br.R -le $br.L -or $br.B -le $br.T) { 'VERDICT=NA reason=bad_rect'; '__END__'; exit 3 }

# 1) baseline: how many refresh records exist right now
$before = 0
if (Test-Path -LiteralPath $log) {
  $before = @(Get-Content -LiteralPath $log -Encoding UTF8 |
              Where-Object { $_ -match 'refresh latest task status; ' }).Count
}
"REFRESH_RECORDS_BEFORE=$before"

# 2) click once (queued, not blocking) and wait for the app to react
[void][EC]::PostMessageW($btn, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)
$arrived = 0
for ($i = 0; $i -lt 40; $i++) {
  Start-Sleep -Milliseconds 500
  if (Test-Path -LiteralPath $log) {
    $n = @(Get-Content -LiteralPath $log -Encoding UTF8 |
           Where-Object { $_ -match 'refresh latest task status; ' }).Count
    if ($n -gt $before) { $arrived = 1; break }
  }
}
"REFRESH_TRIGGERED=$arrived"
if ($arrived -eq 0) { 'VERDICT=NA reason=refresh_not_triggered'; '__END__'; exit 2 }

# What the app itself says the record contains. Needed to tell two very
# different situations apart: "a failed task exists but the UI hides its code"
# (a defect) vs "there is simply no failed task to look at" (nothing to judge).
$rec = Get-Content -LiteralPath $log -Encoding UTF8 |
       Where-Object { $_ -match 'refresh latest task status; ' } | Select-Object -Last 1
$recordCode = 'none'
if ($rec -and $rec -match 'code=([A-Za-z0-9-]+)') { $recordCode = $matches[1] }
"RECORD_CODE=$recordCode"

# 3) wait for the "Tasks & evidence" window the click opens
$admin = [IntPtr]::Zero
for ($i = 0; $i -lt 30; $i++) {
  $admin = FindWin 'BackupRestoreTaskAdmin'
  if ($admin -ne [IntPtr]::Zero) { break }
  Start-Sleep -Milliseconds 500
}
if ($admin -eq [IntPtr]::Zero) { 'ADMIN_WINDOW=no' } else { 'ADMIN_WINDOW=yes' }
Start-Sleep -Milliseconds 800

# 4) read both places
$status = GetTextLive ([EC]::GetDlgItem($win, $StatusId))
"STATUS_CACHED_len=$((GetText ([EC]::GetDlgItem($win, $StatusId))).Length)"
$detail = ''
if ($admin -ne [IntPtr]::Zero) {
  $d = [EC]::GetDlgItem($admin, $DetailId)
  "ADMIN_DETAIL_HWND=$($d -ne [IntPtr]::Zero)"
  $detail = GetTextLive $d
  # Diagnostics: an empty detail box is only a defect if the combo actually has
  # a selected row; "no rows at all" is a different (fixture) problem.
  $combo = [EC]::GetDlgItem($admin, 1401)
  $r1 = [IntPtr]::Zero; $r2 = [IntPtr]::Zero
  $n1 = [EC]::SendMessageTimeoutW($combo, 0x0146, [IntPtr]::Zero, [IntPtr]::Zero, 0x0002, 2000, [ref]$r1)
  $n2 = [EC]::SendMessageTimeoutW($combo, 0x0147, [IntPtr]::Zero, [IntPtr]::Zero, 0x0002, 2000, [ref]$r2)
  "ADMIN_COMBO_count_ok=$n1 count=$r1 sel_ok=$n2 sel=$r2"
  $lab = GetText ([EC]::GetDlgItem($admin, 1400))
  "ADMIN_LABEL_len=$($lab.Length)"
  $hint = GetText ([EC]::GetDlgItem($admin, 1407))
  "ADMIN_HINT_len=$($hint.Length)"
}
$sc = CodeIn $status
$dc = CodeIn $detail
"STATUS_len=$($status.Length) code=$sc"
"DETAIL_len=$($detail.Length) code=$dc"

# The detail box shows whichever row the combo points at (row 0 on open), which
# may be a perfectly healthy task. Walk every row like a user would.
$rowsWithCode = 0
$rowsTotal = 0
if ($admin -ne [IntPtr]::Zero) {
  $combo = [EC]::GetDlgItem($admin, 1401)
  $rc = [IntPtr]::Zero
  if ([EC]::SendMessageTimeoutW($combo, 0x0146, [IntPtr]::Zero, [IntPtr]::Zero, 0x0002, 2000, [ref]$rc) -ne [IntPtr]::Zero) {
    $rowsTotal = [int]$rc.ToInt64()
    for ($i = 0; $i -lt $rowsTotal; $i++) {
      # CB_SETCURSEL alone does NOT notify the parent, so the app would never
      # redraw the detail box; post the CBN_SELCHANGE it would have received.
      [void][EC]::SendMessageTimeoutW($combo, 0x014E, [IntPtr]::new($i), [IntPtr]::Zero, 0x0002, 2000, [ref]$rc)
      [void][EC]::PostMessageW($admin, 0x0111, [IntPtr]::new((1 -shl 16) + 1401), $combo)
      Start-Sleep -Milliseconds 400
      $t = GetTextLive ([EC]::GetDlgItem($admin, $DetailId))
      $c = CodeIn $t
      "ADMIN_ROW $i len=$($t.Length) code=$c"
      if ($c -ne 'none') { $rowsWithCode++ }
    }
  }
}
"ADMIN_ROWS_WITH_CODE=$rowsWithCode of $rowsTotal"

# 5) leave the fixture as we found it: close the modal again
if ($admin -ne [IntPtr]::Zero) {
  [void][EC]::PostMessageW($admin, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 700
}
if ($sc -ne 'none' -and $rowsWithCode -gt 0) { 'VERDICT=VISIBLE_IN_BOTH' }
elseif ($sc -ne 'none') { 'VERDICT=VISIBLE_IN_STATUS_ONLY' }
elseif ($rowsWithCode -gt 0) { 'VERDICT=VISIBLE_IN_TASK_WINDOW_ONLY' }
else { 'VERDICT=MISSING' }
'__END__'
