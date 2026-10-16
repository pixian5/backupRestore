# br-combo-set.ps1 - select an item in a Win32 combo by index, reliably.
# Why not CB_SETCURSEL alone: 0105 measured that CB_SETCURSEL does NOT fire
# CBN_SELCHANGE, so the app never learns about the new selection. Fix: set the
# selection AND post the WM_COMMAND/CBN_SELCHANGE notification to the parent,
# which is exactly what a real user selection produces.
# ASCII-only body (GBK-safe for prlctl exec).
param(
    [int]$ComboId = 1206,
    [int]$Index = 1
)
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrCombo {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint CB_SETCURSEL = 0x014E;
  public const uint CB_GETCURSEL = 0x0147;
  public const uint CB_GETCOUNT  = 0x0146;
  public const uint WM_COMMAND  = 0x0111;
  public const uint CBN_SELCHANGE = 1;
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
}
"@
$h = [IntPtr]::Zero
$cb = [BrCombo+EnumCb]{ param($hw,$lp)
    $t = New-Object System.Text.StringBuilder 512
    [BrCombo]::GetWindowTextW($hw,$t,512) | Out-Null
    if ($t.ToString() -like '*BackupRestore*') { $script:h = $hw; return $false }
    return $true
}
[void][BrCombo]::EnumWindows($cb, [IntPtr]::Zero)
if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW'; exit 1 }
$c = [BrCombo]::GetDlgItem($h, $ComboId)
if ($c -eq [IntPtr]::Zero) { Write-Output ('COMBO_NULL id=' + $ComboId); exit 2 }
$count = [BrCombo]::SendMessageW($c, [BrCombo]::CB_GETCOUNT, [IntPtr]::Zero, [IntPtr]::Zero).ToInt32()
$before = [BrCombo]::SendMessageW($c, [BrCombo]::CB_GETCURSEL, [IntPtr]::Zero, [IntPtr]::Zero).ToInt32()
[void][BrCombo]::SendMessageW($c, [BrCombo]::CB_SETCURSEL, [IntPtr]$Index, [IntPtr]::Zero)
# notify parent: wParam = MAKEWPARAM(comboId, CBN_SELCHANGE), lParam = combo hwnd
$wp = [IntPtr]([int64]$ComboId -bor ([int64]$CBN_SELCHANGE -shl 16))
[void][BrCombo]::SendMessageW($h, [BrCombo]::WM_COMMAND, $wp, $c)
Start-Sleep -Milliseconds 400
$after = [BrCombo]::SendMessageW($c, [BrCombo]::CB_GETCURSEL, [IntPtr]::Zero, [IntPtr]::Zero).ToInt32()
Write-Output ('combo id=' + $ComboId + ' count=' + $count + ' before=' + $before + ' after=' + $after)
