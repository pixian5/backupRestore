# br-check-read.ps1 - READ-ONLY: report state of GUI controls by id (no toggling).
param([int[]]$Ids = @(1214, 1215))
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrCR {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string t);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_GETCHECK = 0x00F0;
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(1024); GetWindowTextW(h, sb, 1024); return sb.ToString(); }
}
"@
$script:gui = [IntPtr]::Zero
$cb = [BrCR+EnumCb]{ param($h,$l) if ([BrCR]::Cls($h) -eq "BackupRestoreNativeGui") { $script:gui = $h; return $false } return $true }
[void][BrCR]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:gui -eq [IntPtr]::Zero) { Write-Output "NO_GUI"; exit 1 }
Write-Output ("GUI=" + $script:gui.ToInt64())
foreach ($id in $Ids) {
    $h = [BrCR]::GetDlgItem($script:gui, $id)
    if ($h -eq [IntPtr]::Zero) { Write-Output ("id=" + $id + " (null)") ; continue }
    $chk = [int][BrCR]::SendMessageW($h, [BrCR]::BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero)
    Write-Output ("id=" + $id + " class=" + [BrCR]::Cls($h) + " visible=" + [BrCR]::IsWindowVisible($h) + " check=" + $chk + " text=[" + [BrCR]::Text($h) + "]")
}
Write-Output "CHECKREAD_DONE"