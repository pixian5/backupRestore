# br-probe-1203.ps1 - read the image edit (id 1203) with three different APIs
# to find out which view of the control text is empty.
param([string]$Path = "H:\BRImages\c-full-20261014.wim")
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrP3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool SetDlgItemTextW(IntPtr hDlg, int id, string text);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetDlgItemTextW(IntPtr hDlg, int id, StringBuilder sb, int max);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint WM_GETTEXTLENGTH = 0x000E;
  public const uint WM_GETTEXT = 0x000D;
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
"@
$h = [IntPtr]::Zero
$cb = [BrP3+EnumCb]{ param($hw,$lp)
    if ([BrP3]::Cls($hw) -eq 'BackupRestoreNativeGui') { $script:h = $hw; return $false }
    return $true
}
[void][BrP3]::EnumWindows($cb, [IntPtr]::Zero)
if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW'; exit 1 }
$c = [BrP3]::GetDlgItem($h, 1203)
Write-Output ("ctrl hwnd=" + $c.ToInt64())
function ReadAll($ctl, $parent) {
    $sb1 = New-Object System.Text.StringBuilder 512
    $n1 = [BrP3]::GetDlgItemTextW($parent, 1203, $sb1, 512)
    $len = [BrP3]::SendMessageW($ctl, [BrP3]::WM_GETTEXTLENGTH, [IntPtr]::Zero, [IntPtr]::Zero).ToInt64()
    $buf = New-Object System.Text.StringBuilder 1024
    $n2 = [BrP3]::SendMessageW($ctl, [BrP3]::WM_GETTEXT, [IntPtr]1024, $buf).ToInt64()
    Write-Output ("  GetDlgItemTextW len=" + $n1 + " text=[" + $sb1.ToString() + "]")
    Write-Output ("  WM_GETTEXTLENGTH=" + $len)
    Write-Output ("  WM_GETTEXT n=" + $n2 + " text=[" + $buf.ToString() + "]")
}
Write-Output "BEFORE_SET:"
ReadAll $c $h
$ok = [BrP3]::SetDlgItemTextW($h, 1203, $Path)
Write-Output ("SetDlgItemTextW ok=" + $ok)
Write-Output "AFTER_SET:"
ReadAll $c $h
Write-Output "PROBE_DONE"
