# br-click-text.ps1 - click a button (by text) inside any top-level window of a given class.
# Generalization of br-click-create.ps1: default targets the main GUI window; pass
# -WindowClass BackupRestoreSystemDriveChoice for the self-drawn PE/RE choice dialog.
# Runs via br-gui-exec.ps1 (elevated, visible session) so UIPI allows input to the GUI.
# ASCII-only body (GBK-safe for prlctl exec).
param(
    [string]$WindowClass = "BackupRestoreNativeGui",
    [string]$ButtonText = "\u521b\u5efa\u4efb\u52a1",
    [int]$WaitMs = 600
)
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrClickT {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
"@
$script:tops = New-Object System.Collections.ArrayList
$cb = [BrClickT+EnumCb]{ param($h,$l) [void]$script:tops.Add($h); return $true }
[void][BrClickT]::EnumWindows($cb, [IntPtr]::Zero)
$gui = [IntPtr]::Zero
foreach ($h in $script:tops) {
    if ([BrClickT]::Cls($h) -eq $WindowClass) { $gui = $h }
}
if ($gui -eq [IntPtr]::Zero) { Write-Output ('NO_WINDOW class=' + $WindowClass); exit 1 }
[void][BrClickT]::SetForegroundWindow($gui)
Start-Sleep -Milliseconds 400
$script:kids = New-Object System.Collections.ArrayList
$cb2 = [BrClickT+EnumCb]{ param($h,$l) [void]$script:kids.Add($h); return $true }
[void][BrClickT]::EnumChildWindows($gui, $cb2, [IntPtr]::Zero)
$hit = [IntPtr]::Zero
foreach ($k in $script:kids) {
    if ([BrClickT]::Text($k) -eq $ButtonText) { $hit = $k }
}
if ($hit -eq [IntPtr]::Zero) {
    Write-Output ('BUTTON_NOT_FOUND ' + $ButtonText)
    foreach ($k in $script:kids) { $t = [BrClickT]::Text($k); if ($t) { Write-Output ('  child: ' + $t) } }
    exit 2
}
[void][BrClickT]::PostMessageW($hit, [BrClickT]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds $WaitMs
Write-Output ('CLICKED ' + $hit.ToInt64() + ' text=' + $ButtonText)
