$log = "C:\click-pe-log.txt"
"START2 $(Get-Date -Format HH:mm:ss)" | Out-File $log
Add-Type -AssemblyName UIAutomationClient
Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class W { [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l); }'
$root = [System.Windows.Automation.AutomationElement]::RootElement
$allWins = $root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
$win = $null
foreach ($w in $allWins) { if ($w.Current.Name -like "*BackupRestore*") { $win = $w; break } }
if (-not $win) { "WIN_NOT_FOUND" | Out-File $log -Append; exit 1 }
$anyCond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)
$btns = $win.FindAll([System.Windows.Automation.TreeScope]::Descendants, $anyCond)
"BTNS=" + $btns.Count | Out-File $log -Append
foreach ($b in $btns) {
  "B name=" + $b.Current.Name + " ct=" + $b.Current.ControlType.ProgrammaticName | Out-File $log -Append
}
$target = $null
foreach ($b in $btns) { if ($b.Current.Name -like "*硬盘*") { $target = $b; break } }
if ($target) {
  $hwnd = [IntPtr]$target.Current.NativeWindowHandle
  "HWND=" + $hwnd | Out-File $log -Append
  [W]::SendMessage($hwnd, 0x00F5, [IntPtr]1, [IntPtr]0) | Out-Null  # BM_CLICK
  "BM_CLICKED" | Out-File $log -Append
} else {
  "NO_DISK_BTN" | Out-File $log -Append
}
"DONE2" | Out-File $log -Append
