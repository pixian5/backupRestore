# Elevated (via helper). Select, in combo box <Id> of the single visible top-level window of class <Class> owned by
# process <Proc>, the item whose text starts with <Text>. The selection reaches the program the same way a user's
# choice does: CB_SETCURSEL on the combo, then WM_COMMAND(CBN_SELCHANGE) to the parent window. Refuses unless exactly
# one window matches and exactly one item starts with <Text>. Nothing else is touched.
# "MSGBOX" means "#32770". Prints one SELECT=... line, then SELECTED=<text> for humans (decode before comparing).
# NOTE: keep this file ASCII-only (PowerShell 5 reads BOM-less files as ANSI).
param([string]$Class='', [int]$Id=0, [string]$Text='', [string]$Proc='BackupRestore')
$ErrorActionPreference='Continue'
[Console]::OutputEncoding=[Text.Encoding]::UTF8
Add-Type -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices;
public class UICB {
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, StringBuilder l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageTimeoutW(IntPtr h, uint m, IntPtr w, IntPtr l, uint f, uint t, out IntPtr r);
}
'@
[void][UICB]::SetProcessDPIAware()
$pids = @(Get-Process -Name $Proc -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
$script:win = [IntPtr]::Zero
$script:count = 0
$cb = [UICB+Cb]{ param([IntPtr]$h,[IntPtr]$l) $c = New-Object System.Text.StringBuilder 256; [void][UICB]::GetClassNameW($h, $c, 256); if ($c.ToString() -eq $Class -and [UICB]::IsWindowVisible($h)) { $wp=0; [void][UICB]::GetWindowThreadProcessId($h, [ref]$wp); if ($pids -contains [int]$wp) { $script:count++; $script:win = $h } }; return $true }
[void][UICB]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:count -ne 1) { "SELECT=WINDOW_COUNT_$($script:count) class=$Class id=$Id"; '__END__'; exit 2 }
$win = $script:win
$ctl = [UICB]::GetDlgItem($win, $Id)
if ($ctl -eq [IntPtr]::Zero) { "SELECT=NO_CONTROL id=$Id"; '__END__'; exit 1 }
if (-not [UICB]::IsWindowVisible($ctl)) { "SELECT=HIDDEN id=$Id"; '__END__'; exit 2 }
$n = [int][UICB]::SendMessageW($ctl, 0x0146, [IntPtr]::Zero, [IntPtr]::Zero)      # CB_GETCOUNT
$hit = -1; $hits = 0
for ($i = 0; $i -lt $n; $i++) {
  $sb = New-Object System.Text.StringBuilder 1024
  [void][UICB]::SendMessageW($ctl, 0x0148, [IntPtr]$i, $sb)                      # CB_GETLBTEXT
  if ($sb.ToString().StartsWith($Text, [StringComparison]::OrdinalIgnoreCase)) { $hits++; $hit = $i }
}
if ($hits -ne 1) { "SELECT=ITEM_COUNT_$hits id=$Id text=$Text count=$n"; '__END__'; exit 3 }
[void][UICB]::SendMessageW($ctl, 0x014E, [IntPtr]$hit, [IntPtr]::Zero)           # CB_SETCURSEL
# WM_COMMAND to the parent: wParam = control id | (CBN_SELCHANGE << 16), lParam = the combo's handle.
# Bounded on purpose: a hung program must not hang the resident helper.
$r = [IntPtr]::Zero
$sent = [int][UICB]::SendMessageTimeoutW($win, 0x0111, [IntPtr]($Id -bor 0x10000), $ctl, 2, 5000, [ref]$r)
$sel = [int][UICB]::SendMessageW($ctl, 0x0147, [IntPtr]::Zero, [IntPtr]::Zero)   # CB_GETCURSEL
$sb2 = New-Object System.Text.StringBuilder 1024
if ($sel -ge 0) { [void][UICB]::SendMessageW($ctl, 0x0148, [IntPtr]$sel, $sb2) }
Start-Sleep -Milliseconds 500
if ($sel -eq $hit -and $sent -ne 0) { "SELECT=OK id=$Id index=$hit text=$Text notified=1" } else { "SELECT=MISMATCH id=$Id wanted=$hit selected=$sel notified=$sent" }
"SELECTED=$sb2"
'__END__'
