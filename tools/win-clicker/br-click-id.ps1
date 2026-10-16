# br-click-id.ps1 - click a child control (by dialog item id) of the main GUI window.
# Why by id: 0105 recorded stable control ids (1002=read image, 1003=create task);
# text matching fails when the log path mangles GBK and button captions carry
# mnemonics/trailing spaces. ASCII-only body (GBK-safe for prlctl exec).
param(
    [int]$Id = 1002,
    [string]$WindowClass = "BackupRestoreNativeGui",
    [int]$WaitMs = 600
)
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrClickI {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll")] public static extern IntPtr PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
"@
$h = [IntPtr]::Zero
$cb = [BrClickI+EnumCb]{ param($hw,$lp)
    if ([BrClickI]::Cls($hw) -eq $WindowClass) { $script:h = $hw; return $false }
    return $true
}
[void][BrClickI]::EnumWindows($cb, [IntPtr]::Zero)
if ($h -eq [IntPtr]::Zero) { Write-Output ('NO_WINDOW class=' + $WindowClass); exit 1 }
[void][BrClickI]::SetForegroundWindow($h)
Start-Sleep -Milliseconds 300
$b = [BrClickI]::GetDlgItem($h, $Id)
if ($b -eq [IntPtr]::Zero) { Write-Output ('CONTROL_NULL id=' + $Id); exit 2 }
[void][BrClickI]::PostMessageW($b, [BrClickI]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds $WaitMs
$len = [BrClickI]::Text($b).Length
Write-Output ('CLICKED id=' + $Id + ' hwnd=' + $b.ToInt64() + ' textlen=' + $len)
