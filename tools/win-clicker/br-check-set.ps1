# br-check-set.ps1 - set/clear a checkbox by control id in the main GUI window.
# EnumWindows-based lookup (FindWindowW proved unreliable in the elevated
# br-s1.sh context). ASCII-only body (GBK-safe for prlctl exec).
# Usage: br-check-set.ps1 -Id 1214 -Checked false
param(
    [int]$Id = 1214,
    [string]$Checked = "true",
    [string]$WindowClass = "BackupRestoreNativeGui"
)
$want = $true
if ($Checked -eq "false" -or $Checked -eq "0" -or $Checked -eq "no") { $want = $false }
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrCS {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_GETCHECK = 0x00F0;
  public const uint BM_SETCHECK = 0x00F2;
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
"@
$h = [IntPtr]::Zero
$cb = [BrCS+EnumCb]{ param($hw,$lp)
    if ([BrCS]::Cls($hw) -eq $WindowClass) { $script:h = $hw; return $false }
    return $true
}
[void][BrCS]::EnumWindows($cb, [IntPtr]::Zero)
if ($h -eq [IntPtr]::Zero) { Write-Output ('NO_WINDOW class=' + $WindowClass); exit 1 }
$c = [BrCS]::GetDlgItem($h, $Id)
if ($c -eq [IntPtr]::Zero) { Write-Output ('CONTROL_NULL id=' + $Id); exit 2 }
$before = [int][BrCS]::SendMessageW($c, [BrCS]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero)
$val = [IntPtr]::Zero
if ($want) { $val = [IntPtr]1 }
[void][BrCS]::SendMessageW($c, [BrCS]::BM_SETCHECK, $val, [IntPtr]::Zero)
$after = [int][BrCS]::SendMessageW($c, [BrCS]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output ('CHECKBOX id=' + $Id + ' before=' + $before + ' after=' + $after + ' want=' + $want)
