# focus-gui.ps1 - force BackupRestore GUI to foreground and verify.
# Prints FOCUS_OK <hwnd> when foreground readback matches, else FOCUS_FAIL.
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Fg1 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
}
"@
$script:tops = New-Object System.Collections.ArrayList
$cb = [Fg1+EnumCb]{ param($h,$l) [void]$script:tops.Add($h); return $true }
$r = [Fg1]::EnumWindows($cb, [IntPtr]::Zero)
$gui = [IntPtr]::Zero
foreach ($h in $script:tops) {
  $sb = New-Object Text.StringBuilder 256
  [void][Fg1]::GetClassNameW($h, $sb, 256)
  if ($sb.ToString() -eq "BackupRestoreNativeGui") { $gui = $h }
}
if ($gui -eq [IntPtr]::Zero) { Write-Output "FOCUS_FAIL no-gui-window"; return }
if ([Fg1]::IsIconic($gui)) { [void][Fg1]::ShowWindow($gui, 3) }  # SW_MAXIMIZE only if minimized; do NOT SW_RESTORE
[void][Fg1]::SetForegroundWindow($gui)
Start-Sleep -Milliseconds 300
$fg = [Fg1]::GetForegroundWindow()
if ($fg -eq $gui) { Write-Output ("FOCUS_OK hwnd=" + $gui.ToInt64()) } else { Write-Output ("FOCUS_FAIL fg=" + $fg.ToInt64() + " gui=" + $gui.ToInt64()) }
