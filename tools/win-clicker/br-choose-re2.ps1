# br-choose-re2.ps1 - click "进入 Windows RE" (id=2002) on the BackupRestoreSystemDriveChoice
# dialog. Uses EnumWindows (FindWindowW is unreliable in the elevated context).
param([int]$ButtonId = 2002, [string]$WindowClass = "BackupRestoreSystemDriveChoice")
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrCRE {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(1024); GetWindowTextW(h, sb, 1024); return sb.ToString(); }
}
"@
$script:dlg = [IntPtr]::Zero
$cb = [BrCRE+EnumCb]{ param($h,$l)
  if ([BrCRE]::Cls($h) -eq $WindowClass) { $script:dlg = $h; return $false }
  return $true
}
[void][BrCRE]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:dlg -eq [IntPtr]::Zero) { Write-Output ("NO_DIALOG class=" + $WindowClass); exit 1 }
Write-Output ("DIALOG=" + $script:dlg.ToInt64() + " title=[" + [BrCRE]::Text($script:dlg) + "] vis=" + [BrCRE]::IsWindowVisible($script:dlg))
[void][BrCRE]::SetForegroundWindow($script:dlg)
Start-Sleep -Milliseconds 300
$b = [BrCRE]::GetDlgItem($script:dlg, $ButtonId)
if ($b -eq [IntPtr]::Zero) { Write-Output ("NO_BUTTON id=" + $ButtonId); exit 2 }
Write-Output ("BUTTON id=" + $ButtonId + " text=[" + [BrCRE]::Text($b) + "] enabled=" + [BrCRE]::IsWindowEnabled($b))
[void][BrCRE]::SendMessageW($b, [BrCRE]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output ("CLICKED id=" + $ButtonId + " handle=" + $b.ToInt64())