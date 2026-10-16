# br-click-create.ps1 - click the create-task button of the BackupRestore GUI by handle.
# Runs via br-gui-exec.ps1 (elevated, visible session) so UIPI allows input to the GUI.
# Why by handle: the agent screenshot is cropped and keyboard focus may sit on a tab
# radio, so neither pixel clicks nor Enter reliably reach the default button.
param([string]$Button = "创建任务")
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrClick {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
"@
$script:tops = New-Object System.Collections.ArrayList
$cb = [BrClick+EnumCb]{ param($h,$l) [void]$script:tops.Add($h); return $true }
[void][BrClick]::EnumWindows($cb, [IntPtr]::Zero)
$gui = [IntPtr]::Zero
foreach ($h in $script:tops) {
  if ([BrClick]::Cls($h) -eq 'BackupRestoreNativeGui') { $gui = $h }
}
if ($gui -eq [IntPtr]::Zero) { Write-Output 'NO_GUI'; exit 1 }
[void][BrClick]::SetForegroundWindow($gui)
Start-Sleep -Milliseconds 400
$script:kids = New-Object System.Collections.ArrayList
$cb2 = [BrClick+EnumCb]{ param($h,$l) [void]$script:kids.Add($h); return $true }
[void][BrClick]::EnumChildWindows($gui, $cb2, [IntPtr]::Zero)
$hit = [IntPtr]::Zero
foreach ($k in $script:kids) {
  if ([BrClick]::Text($k) -eq $Button) { $hit = $k }
}
if ($hit -eq [IntPtr]::Zero) {
  Write-Output ('BUTTON_NOT_FOUND ' + $Button)
  foreach ($k in $script:kids) { $t = [BrClick]::Text($k); if ($t) { Write-Output ('  child: ' + $t) } }
  exit 2
}
[void][BrClick]::PostMessageW($hit, [BrClick]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output ('CLICKED ' + $hit.ToInt64())
