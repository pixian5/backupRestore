# Elevated. Read-only: list the items of combo box <Id> (and the current selection) in the main GUI window.
param([int]$Id=1202)
$ErrorActionPreference='Continue'
[Console]::OutputEncoding=[Text.Encoding]::UTF8
Add-Type -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices;
public class CBX {
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, StringBuilder l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
}
'@
[void][CBX]::SetProcessDPIAware()
$pids = @(Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
$script:win = [IntPtr]::Zero
$cb = [CBX+Cb]{ param([IntPtr]$h,[IntPtr]$l) $c = New-Object System.Text.StringBuilder 256; [void][CBX]::GetClassNameW($h, $c, 256); if ($c.ToString() -eq 'BackupRestoreNativeGui') { $wp=0; [void][CBX]::GetWindowThreadProcessId($h, [ref]$wp); if ($pids -contains [int]$wp -and [CBX]::IsWindowVisible($h)) { $script:win = $h; return $false } }; return $true }
[void][CBX]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:win -eq [IntPtr]::Zero) { "COMBO=NO_WINDOW id=$Id"; '__END__'; exit 1 }
$ctl = [CBX]::GetDlgItem($script:win, $Id)
if ($ctl -eq [IntPtr]::Zero) { "COMBO=NO_CONTROL id=$Id"; '__END__'; exit 1 }
$n = [int][CBX]::SendMessage($ctl, 0x0146, [IntPtr]::Zero, [IntPtr]::Zero)   # CB_GETCOUNT
$sel = [int][CBX]::SendMessage($ctl, 0x0147, [IntPtr]::Zero, [IntPtr]::Zero) # CB_GETCURSEL
"COMBO id=$Id count=$n selected=$sel visible=$([CBX]::IsWindowVisible($ctl))"
for ($i = 0; $i -lt $n; $i++) {
  $sb = New-Object System.Text.StringBuilder 1024
  [void][CBX]::SendMessageW($ctl, 0x0148, [IntPtr]$i, $sb)                   # CB_GETLBTEXT
  "  ITEM[$i]=$sb"
}
'__END__'
