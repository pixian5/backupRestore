# br-check-toggle.ps1 - click a checkbox to toggle it, then report state.
# BM_SETCHECK proved ineffective on this control; a real click (BM_CLICK)
# toggles the state and lets the app observe it.
param(
    [int]$Id = 1214,
    [int]$Times = 1,
    [string]$WindowClass = "BackupRestoreNativeGui"
)
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrCT {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_GETCHECK = 0x00F0;
  public const uint BM_CLICK = 0x00F5;
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
"@
$h = [IntPtr]::Zero
$cb = [BrCT+EnumCb]{ param($hw,$lp)
    if ([BrCT]::Cls($hw) -eq $WindowClass) { $script:h = $hw; return $false }
    return $true
}
[void][BrCT]::EnumWindows($cb, [IntPtr]::Zero)
if ($h -eq [IntPtr]::Zero) { Write-Output ('NO_WINDOW class=' + $WindowClass); exit 1 }
$c = [BrCT]::GetDlgItem($h, $Id)
if ($c -eq [IntPtr]::Zero) { Write-Output ('CONTROL_NULL id=' + $Id); exit 2 }
$before = [int][BrCT]::SendMessageW($c, [BrCT]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero)
for ($i = 0; $i -lt $Times; $i++) {
    [void][BrCT]::PostMessageW($c, [BrCT]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 400
}
$after = [int][BrCT]::SendMessageW($c, [BrCT]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output ('CHECKBOX id=' + $Id + ' before=' + $before + ' after=' + $after + ' toggles=' + $Times)
