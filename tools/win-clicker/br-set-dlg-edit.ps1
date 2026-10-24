# br-set-dlg-edit.ps1 - set text of a child EDIT control inside a dialog window
# (used by the P1-c backup-file-management acceptance: set "keep latest N").
# Why a helper: br-click-id.ps1 only clicks; the keep count lives in an EDIT.
# ASCII-only body (GBK-safe for prlctl exec).
param(
    [int]$Id = 3202,
    [string]$Text = "1",
    [string]$WindowClass = "BackupRestoreImageAdminWindow"
)
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrSetEdit {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, string l);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint WM_SETTEXT = 0x000C;
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
"@
$h = [IntPtr]::Zero
$cb = [BrSetEdit+EnumCb]{ param($hw,$lp)
    if ([BrSetEdit]::Cls($hw) -eq $WindowClass) { $script:h = $hw; return $false }
    return $true
}
[void][BrSetEdit]::EnumWindows($cb, [IntPtr]::Zero)
if ($h -eq [IntPtr]::Zero) { Write-Output ("NO_WINDOW class=" + $WindowClass); exit 1 }
$edit = [BrSetEdit]::GetDlgItem($h, $Id)
if ($edit -eq [IntPtr]::Zero) { Write-Output ("NO_CONTROL id=" + $Id); exit 1 }
$r = [BrSetEdit]::SendMessageW($edit, [BrSetEdit]::WM_SETTEXT, [IntPtr]::Zero, $Text)
Write-Output ("SET id=" + $Id + " text=" + $Text + " result=" + $r)
