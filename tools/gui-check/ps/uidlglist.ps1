# Elevated (via helper). Read-only: list the controls (id | class | text) of every visible top-level
# window of class <Class> owned by process <Proc>. "MSGBOX" means "#32770".
param([string]$Class='', [string]$Proc='BackupRestore')
$ErrorActionPreference='Continue'
[Console]::OutputEncoding=[Text.Encoding]::UTF8
if ($Class -eq 'MSGBOX') { $Class = '#32770' }
$any = ($Class -eq 'ANY')
Add-Type -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices;
public class UIL {
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, Cb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr h, int idx);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeoutW(IntPtr h, uint msg, IntPtr wp, IntPtr lp, uint flags, uint timeout, out IntPtr res);
}
'@
function TextOf([IntPtr]$h) { $t = New-Object System.Text.StringBuilder 2048; [void][UIL]::GetWindowTextW($h, $t, 2048); return $t.ToString() }
function LiveTextOf([IntPtr]$h, [string]$cls) {
  if ($cls -ne 'Edit') { return (TextOf $h) }
  $r = [IntPtr]::Zero
  if ([UIL]::SendMessageTimeoutW($h, 0x000E, [IntPtr]::Zero, [IntPtr]::Zero, 0x0002, 2000, [ref]$r) -eq [IntPtr]::Zero) { return (TextOf $h) }
  $len = [int]$r.ToInt64()
  if ($len -le 0) { return '' }
  if ($len -gt 60000) { $len = 60000 }
  $buf = [System.Runtime.InteropServices.Marshal]::AllocHGlobal(($len + 2) * 2)
  try {
    $r2 = [IntPtr]::Zero
    if ([UIL]::SendMessageTimeoutW($h, 0x000D, [IntPtr]::new($len + 1), $buf, 0x0002, 2000, [ref]$r2) -eq [IntPtr]::Zero) { return (TextOf $h) }
    return [System.Runtime.InteropServices.Marshal]::PtrToStringUni($buf)
  } finally {
    [System.Runtime.InteropServices.Marshal]::FreeHGlobal($buf)
  }
}
function ClassOf([IntPtr]$h) { $c = New-Object System.Text.StringBuilder 256; [void][UIL]::GetClassNameW($h, $c, 256); return $c.ToString() }
function IsDefaultBtn([IntPtr]$c, [string]$cls) { if ($cls -ne 'Button') { return '' }; $isDef = (([UIL]::GetWindowLongW($c, -16) -band 0xF) -eq 1); return " def=$isDef" }
[void][UIL]::SetProcessDPIAware()
$pids = @(Get-Process -Name $Proc -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
$script:found = New-Object System.Collections.ArrayList
$cb = [UIL+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  if (((ClassOf $h) -eq $Class -or $any) -and [UIL]::IsWindowVisible($h)) {
    $wp = 0; [void][UIL]::GetWindowThreadProcessId($h, [ref]$wp)
    if ($pids -contains [int]$wp) { [void]$script:found.Add($h) }
  }
  return $true
}
[void][UIL]::EnumWindows($cb, [IntPtr]::Zero)
"WINDOWS=$($script:found.Count) class=$Class"
foreach ($h in $script:found) {
  "WIN hwnd=$h class=$(ClassOf $h) title=$(TextOf $h)"
  $script:items = New-Object System.Collections.ArrayList
  $ccb = [UIL+Cb]{ param([IntPtr]$c,[IntPtr]$l) [void]$script:items.Add($c); return $true }
  [void][UIL]::EnumChildWindows($h, $ccb, [IntPtr]::Zero)
  foreach ($c in $script:items) { $cls = ClassOf $c; "  id=$([UIL]::GetDlgCtrlID($c)) class=$cls visible=$([UIL]::IsWindowVisible($c))$(IsDefaultBtn $c $cls) text=$(LiveTextOf $c $cls)" }
}
'__END__'
