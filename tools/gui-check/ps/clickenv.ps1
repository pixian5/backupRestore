# NOTE: keep this file ASCII-only.
# One-shot diagnostic for "a click does nothing": who am I, can I see the window,
# can I see the control, and what does PostMessage return?
# Run it twice -- directly (medium) and via runas.ps1 (elevated) -- to tell a UIPI
# rejection apart from "window not found".
# Output: IDENT=.. ADMIN=.. INTEGRITY=.. MAIN_FOUND=.. BTN_FOUND=.. POST_OK=..
#         __END__
param([int]$Id = 1007)
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class CE {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(Cb cb, IntPtr l);
  public delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
}
'@
[void][CE]::SetProcessDPIAware()
$id = [Security.Principal.WindowsIdentity]::GetCurrent()
$adm = ([Security.Principal.WindowsPrincipal]$id).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$lvl = 'unknown'
foreach ($line in (& whoami /groups 2>&1)) {
  if ($line -match 'Mandatory Label') {
    if ($line -match 'System') { $lvl = 'System' }
    elseif ($line -match 'High') { $lvl = 'High' }
    elseif ($line -match 'Medium') { $lvl = 'Medium' }
    elseif ($line -match 'Low') { $lvl = 'Low' }
  }
}
$script:main = [IntPtr]::Zero
$cb = [CE+Cb]{
  param([IntPtr]$h,[IntPtr]$l)
  $sb = New-Object System.Text.StringBuilder 256
  [void][CE]::GetClassNameW($h, $sb, 256)
  if ($sb.ToString() -eq 'BackupRestoreNativeGui') { $script:main = $h }
  return $true
}
[void][CE]::EnumWindows($cb, [IntPtr]::Zero)
$mf = ($script:main -ne [IntPtr]::Zero)
$bf = $false
$post = 'n/a'
if ($mf) {
  $b = [CE]::GetDlgItem($script:main, $Id)
  $bf = ($b -ne [IntPtr]::Zero)
  if ($bf) { $post = [CE]::PostMessageW($b, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero) }
}
"IDENT=$($id.Name) ADMIN=$adm INTEGRITY=$lvl MAIN_FOUND=$mf BTN_FOUND=$bf POST_OK=$post"
'__END__'
