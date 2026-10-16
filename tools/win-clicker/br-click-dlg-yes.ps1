# br-click-dlg-yes.ps1 - click the "是(&Y)" (id=6) button of the BackupRestore native
# destructive-confirm dialog (class #32770). Elevated, Session 1.
param([int]$ButtonId = 6, [string]$Title = "")
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrCY {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(2048); GetWindowTextW(h, sb, 2048); return sb.ToString(); }
}
"@
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $p) { Write-Output "NO_PROC"; exit 1 }
$script:dlg = [IntPtr]::Zero
$cb = [BrCY+EnumCb]{ param($h,$l)
  $pid2 = [uint32]0
  [void][BrCY]::GetWindowThreadProcessId($h, [ref]$pid2)
  if ($pid2 -eq $p.Id -and [BrCY]::Cls($h) -eq "#32770" -and [BrCY]::IsWindowVisible($h)) {
    $script:dlg = $h; return $false
  }
  return $true
}
[void][BrCY]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:dlg -eq [IntPtr]::Zero) { Write-Output "NO_DIALOG"; exit 2 }
[void][BrCY]::SetForegroundWindow($script:dlg)
Start-Sleep -Milliseconds 300
$b = [BrCY]::GetDlgItem($script:dlg, $ButtonId)
if ($b -eq [IntPtr]::Zero) { Write-Output ("NO_BUTTON id=" + $ButtonId); exit 3 }
Write-Output ("DIALOG=" + $script:dlg.ToInt64() + " body=[" + ([BrCY]::Text($script:dlg) -replace "`r`n"," / ") + "]")
[void][BrCY]::SendMessageW($b, [BrCY]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output ("CLICKED_YES id=" + $ButtonId + " btn=" + $b.ToInt64())