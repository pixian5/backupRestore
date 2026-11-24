# Elevated, desktop session. Diagnose + set text with per-step timeouts; progress is written to a file as it goes.
param([int]$Id=1203, [string]$Text='')
$ErrorActionPreference='Continue'
[Console]::OutputEncoding=[Text.Encoding]::UTF8
$prog = 'H:\brwork\uitext-progress.txt'
function Note([string]$m) { ("{0} {1}" -f (Get-Date -Format 'HH:mm:ss.fff'), $m) | Out-File -Append -Encoding utf8 $prog }
"" | Out-File -Encoding utf8 $prog
Note "start pid=$PID"
Add-Type -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices;
public class UTX {
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, string l, uint f, uint t, out IntPtr r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true, EntryPoint="SendMessageTimeout")] public static extern IntPtr SendMessageTimeoutSb(IntPtr h, uint m, IntPtr w, StringBuilder l, uint f, uint t, out IntPtr r);
}
'@
Note "addtype done"
[void][UTX]::SetProcessDPIAware()
$pids = @(Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
$script:win = [IntPtr]::Zero
$cb = [UTX+Cb]{ param([IntPtr]$h,[IntPtr]$l) $c = New-Object System.Text.StringBuilder 256; [void][UTX]::GetClassNameW($h, $c, 256); if ($c.ToString() -eq 'BackupRestoreNativeGui') { $wp=0; [void][UTX]::GetWindowThreadProcessId($h, [ref]$wp); if ($pids -contains [int]$wp -and [UTX]::IsWindowVisible($h)) { $script:win = $h; return $false } }; return $true }
[void][UTX]::EnumWindows($cb, [IntPtr]::Zero)
Note "enum done win=$($script:win) pids=$($pids -join ',')"
if ($script:win -eq [IntPtr]::Zero) { "RESULT=NO_WINDOW"; '__END__'; exit 1 }
$ctl = [UTX]::GetDlgItem($script:win, $Id)
Note "ctl=$ctl"
$r = [IntPtr]::Zero
$p = [UTX]::SendMessageTimeout($ctl, 0x0000, [IntPtr]::Zero, [NullString]::Value, 2, 3000, [ref]$r); Note "ping(WM_NULL) ret=$p err=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
$r = [IntPtr]::Zero
$p = [UTX]::SendMessageTimeout($ctl, 0x000E, [IntPtr]::Zero, [NullString]::Value, 2, 3000, [ref]$r); Note "textlen ret=$p len=$r"
$r = [IntPtr]::Zero
$p = [UTX]::SendMessageTimeout($ctl, 0x000C, [IntPtr]::Zero, $Text, 2, 5000, [ref]$r); Note "settext ret=$p res=$r err=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
$sb = New-Object System.Text.StringBuilder 1024
$r = [IntPtr]::Zero
$p = [UTX]::SendMessageTimeoutSb($ctl, 0x000D, [IntPtr]1024, $sb, 2, 3000, [ref]$r); Note "gettext ret=$p"
"SETTEXT id=$Id ret=$p text=$sb match=$($sb.ToString() -eq $Text)"
Get-Content $prog
'__END__'
