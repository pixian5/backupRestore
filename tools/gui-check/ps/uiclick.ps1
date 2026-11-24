# Elevated (via helper). Click control <Id> of the single visible top-level window of class <Class>
# owned by process <Proc>, with a REAL mouse click. Refuses to click unless exactly one window matches.
# Class name "MSGBOX" means "#32770" (kept out of the command line because of the hash sign).
param([string]$Class='', [int]$Id=0, [string]$Proc='BackupRestore')
$ErrorActionPreference='Continue'
[Console]::OutputEncoding=[Text.Encoding]::UTF8
if ($Class -eq 'MSGBOX') { $Class = '#32770' }
Add-Type -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices;
public class UIC {
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr e);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@
[void][UIC]::SetProcessDPIAware()
$pids = @(Get-Process -Name $Proc -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
$script:found = New-Object System.Collections.ArrayList
$cb = [UIC+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $c = New-Object System.Text.StringBuilder 256
  [void][UIC]::GetClassNameW($h, $c, 256)
  if ($c.ToString() -eq $Class -and [UIC]::IsWindowVisible($h)) {
    $wp = 0; [void][UIC]::GetWindowThreadProcessId($h, [ref]$wp)
    if ($pids -contains [int]$wp) { [void]$script:found.Add($h) }
  }
  return $true
}
[void][UIC]::EnumWindows($cb, [IntPtr]::Zero)
"WINDOWS=$($script:found.Count) class=$Class proc=$Proc pids=$($pids -join ',')"
foreach ($h in $script:found) { $t = New-Object System.Text.StringBuilder 512; [void][UIC]::GetWindowTextW($h, $t, 512); "WIN hwnd=$h title=$t" }
if ($script:found.Count -ne 1) { "CLICK=WINDOW_COUNT_$($script:found.Count)"; '__END__'; exit 2 }
$win = $script:found[0]
if ($Id -eq 0) { "CLICK=NONE_REQUESTED"; '__END__'; exit 0 }
$ctl = [UIC]::GetDlgItem($win, $Id)
if ($ctl -eq [IntPtr]::Zero) { "CLICK=NO_CONTROL id=$Id"; '__END__'; exit 1 }
if (-not [UIC]::IsWindowVisible($ctl)) { "CLICK=HIDDEN id=$Id"; '__END__'; exit 2 }
$r = New-Object UIC+RECT
[void][UIC]::GetWindowRect($ctl, [ref]$r)
if ($r.R -le $r.L -or $r.B -le $r.T -or $r.R -gt 20000 -or $r.B -gt 20000) { "CLICK=BAD_RECT id=$Id rect=$($r.L),$($r.T),$($r.R),$($r.B)"; '__END__'; exit 3 }
$x = [int](($r.L + $r.R) / 2); $y = [int](($r.T + $r.B) / 2)
[void][UIC]::SetForegroundWindow($win)
Start-Sleep -Milliseconds 500
[void][UIC]::SetCursorPos($x, $y)
Start-Sleep -Milliseconds 250
[UIC]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 90
[UIC]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 900
"CLICK=OK id=$Id at=$x,$y rect=$($r.L),$($r.T),$($r.R),$($r.B)"
'__END__'
