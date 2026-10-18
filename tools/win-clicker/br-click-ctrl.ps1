# br-click-ctrl.ps1 - click a control by ID via PostMessage(BM_CLICK).
# Finds the top-level window that owns the control (main GUI class
# BackupRestoreNativeGui or a modal #32770), then posts BM_CLICK.
# MUST use PostMessage: synchronous SendMessage waits for the modal loop to
# finish and hangs the agent (10-17/18 lesson).
# Usage: br-click-ctrl.ps1 <controlId>
param([int]$CtrlId = 0)
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Btn3 {
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(2048); GetWindowTextW(h, sb, 2048); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
"@
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }
$BM_CLICK = 0x00F5
$tops = New-Object System.Collections.ArrayList
$cb = [Btn3+EnumCb]{ param($h,$l) [void]$tops.Add($h); return $true }
[void][Btn3]::EnumWindows($cb, [IntPtr]::Zero)
$target = [IntPtr]::Zero
$owner = [IntPtr]::Zero
foreach ($h in $tops) {
  $c = [Btn3]::GetDlgItem($h, $CtrlId)
  if ($c -ne [IntPtr]::Zero -and [Btn3]::IsWindow($c)) {
    $target = $c
    $owner = $h
    W ("owner hwnd=" + $h.ToInt64() + " cls=" + [Btn3]::Cls($h) + " text=[" + [Btn3]::Text($h) + "]")
    W ("ctrl id=" + $CtrlId + " hwnd=" + $c.ToInt64() + " text=[" + [Btn3]::Text($c) + "]")
    break
  }
}
if ($target -eq [IntPtr]::Zero) {
  W ("CTRL_NOT_FOUND id=" + $CtrlId)
} else {
  $ok = [Btn3]::PostMessageW($target, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
  W ("CLICKED id=" + $CtrlId + " post=" + $ok)
}
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\clickctrl.txt", ($lines -join "`r`n"), $utf8)
Write-Output "DONE"
