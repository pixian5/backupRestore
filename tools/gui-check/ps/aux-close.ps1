# NOTE: keep this file ASCII-only.
# Close a modal auxiliary window by class and wait until it is really gone.
# Closing matters twice over: (1) the next dialog cannot open while the main
# window is disabled, (2) a leftover modal makes every later click look dead.
# Output:
#   CLOSED=1|0 class=... main_enabled=...
#   __END__
param([string]$Class = 'BackupRestoreSettingsWindow', [int]$WaitMs = 8000)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class AC {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
}
'@
[void][AC]::SetProcessDPIAware()

function Find-Class([string]$want) {
  $script:found = [IntPtr]::Zero
  $cb = [AC+Cb]{
    param([IntPtr]$h,[IntPtr]$l)
    $sb = New-Object System.Text.StringBuilder 256
    [void][AC]::GetClassNameW($h, $sb, 256)
    if ($sb.ToString() -eq $want -and [AC]::IsWindowVisible($h)) { $script:found = $h }
    return $true
  }
  [void][AC]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

$h = Find-Class $Class
if ($h -eq [IntPtr]::Zero) { "CLOSED=1 class=$Class reason=not_open"; '__END__'; exit 0 }
# ★ Two attempts: WM_CLOSE crosses the UIPI boundary, so a non-elevated run is
#   silently dropped. And a dialog that is mid-repaint can miss the first one.
#   Report the integrity level so "could not close" is never mistaken for a
#   product bug.
$lvl = 'unknown'
foreach ($line in (& whoami /groups 2>&1)) {
  if ($line -match 'Mandatory Label') {
    if ($line -match 'System') { $lvl = 'System' }
    elseif ($line -match 'High') { $lvl = 'High' }
    elseif ($line -match 'Medium') { $lvl = 'Medium' }
    elseif ($line -match 'Low') { $lvl = 'Low' }
  }
}
"INTEGRITY=$lvl"
for ($attempt = 1; $attempt -le 2; $attempt++) {
  [void][AC]::PostMessageW($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)   # WM_CLOSE
  $deadline = [DateTime]::Now.AddMilliseconds($WaitMs)
  while ([DateTime]::Now -lt $deadline -and [AC]::IsWindow($h) -ne 0) { Start-Sleep -Milliseconds 150 }
  if ([AC]::IsWindow($h) -eq 0) { break }
}
$still = [AC]::IsWindow($h)
$main = Find-Class 'BackupRestoreNativeGui'
$men = 'n/a'
if ($main -ne [IntPtr]::Zero) { $men = [AC]::IsWindowEnabled($main) }
if ($still -ne 0) { "CLOSED=0 class=$Class reason=still_open main_enabled=$men"; '__END__'; exit 1 }
"CLOSED=1 class=$Class main_enabled=$men"
'__END__'
