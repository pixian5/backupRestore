# br-click-cancel.ps1 - click the Cancel button of the unfinished-task resume dialog.
# Standard MessageBox (#32770): button IDs IDYES=6 IDNO=7 IDCANCEL=2.
# MUST use PostMessage(BM_CLICK): synchronous SendMessage waits for the modal
# loop to finish and hangs the agent (10-17/18 lesson).
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Btn2 {
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
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
$cb = [Btn2+EnumCb]{ param($h,$l) [void]$tops.Add($h); return $true }
[void][Btn2]::EnumWindows($cb, [IntPtr]::Zero)
$dlg = [IntPtr]::Zero
foreach ($h in $tops) {
  $cls = [Btn2]::Cls($h)
  $txt = [Btn2]::Text($h)
  W ("top hwnd=" + $h.ToInt64() + " cls=" + $cls + " text=[" + $txt + "]")
  if ($cls -eq "#32770" -and $txt -match "Unfinished") { $dlg = $h }
}
if ($dlg -eq [IntPtr]::Zero) {
  W "DLG_NOT_FOUND"
} else {
  W ("dlg hwnd=" + $dlg.ToInt64())
  foreach ($pair in @(@(6,"YES"), @(7,"NO"), @(2,"CANCEL"))) {
    $id = $pair[0]; $name = $pair[1]
    $bh = [Btn2]::GetDlgItem($dlg, $id)
    if ($bh -ne [IntPtr]::Zero -and [Btn2]::IsWindow($bh)) {
      $r = New-Object Btn2+RECT
      [void][Btn2]::GetWindowRect($bh, [ref]$r)
      W ("btn " + $name + " id=" + $id + " hwnd=" + $bh.ToInt64() + " rect=" + $r.L + "," + $r.T + "," + $r.R + "," + $r.B + " text=[" + [Btn2]::Text($bh) + "]")
    } else {
      W ("btn " + $name + " id=" + $id + " NOT_FOUND")
    }
  }
  # click CANCEL (id=2)
  $cancel = [Btn2]::GetDlgItem($dlg, 2)
  if ($cancel -ne [IntPtr]::Zero -and [Btn2]::IsWindow($cancel)) {
    $ok = [Btn2]::PostMessageW($cancel, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
    W ("CANCEL_CLICKED post=" + $ok)
  } else {
    W "CANCEL_BTN_MISSING"
  }
}
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\clickcancel.txt", ($lines -join "`r`n"), $utf8)
Write-Output "DONE"
