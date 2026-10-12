# real-click.ps1 — 在会话 1 里对前台窗口的指定控件做真实鼠标点击
# 用法：-Id 6（控件 ID）[-TitleMatch 'xxx'（先找并前置该标题窗口）]
param(
  [Parameter(Mandatory=$true)][int]$Id,
  [string]$TitleMatch = '',
  [string]$ClassMatch = '',
  [string]$Tag = 'click'
)
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class RC {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  public delegate bool CB(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(CB cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public struct R { public int l, t, r, b; }
  public static IntPtr Found = IntPtr.Zero;
}
"@
[RC]::SetProcessDPIAware() | Out-Null
$target = [IntPtr]::Zero
if ($ClassMatch -ne '') {
  # 按窗口类找（ASCII 类名，如引导菜单编辑器的 BackupRestoreBootMenuClass）
  $cbc = [RC+CB]{ param($h,$l)
    $c = New-Object System.Text.StringBuilder 64
    [RC]::GetClassNameW($h,$c,64) | Out-Null
    if ($c.ToString() -eq $ClassMatch -and [RC]::IsWindowVisible($h)) { [RC]::Found = $h; return $false }
    return $true }
  [RC]::EnumWindows($cbc,[IntPtr]::Zero) | Out-Null
  $target = [RC]::Found
  if ($target -ne [IntPtr]::Zero) { [RC]::SetForegroundWindow($target) | Out-Null; Start-Sleep -Milliseconds 500 }
}
if ($target -eq [IntPtr]::Zero -and $TitleMatch -ne '') {
  $cb = [RC+CB]{ param($h,$l)
    $t = New-Object System.Text.StringBuilder 256
    [RC]::GetWindowTextW($h,$t,256) | Out-Null
    if ($t.ToString().Contains($TitleMatch) -and [RC]::IsWindowVisible($h)) { [RC]::Found = $h; return $false }
    return $true }
  [RC]::EnumWindows($cb,[IntPtr]::Zero) | Out-Null
  $target = [RC]::Found
  if ($target -ne [IntPtr]::Zero) { [RC]::SetForegroundWindow($target) | Out-Null; Start-Sleep -Milliseconds 600 }
}
if ($target -eq [IntPtr]::Zero) {
  # 优先找“含该控件 ID 的可见 #32770 对话框”（MessageBox 按钮），避免被
  # 计划任务自己的控制台抢占前台后点错窗口。
  $cbk = [RC+CB]{ param($h,$l)
    $c = New-Object System.Text.StringBuilder 64
    [RC]::GetClassNameW($h,$c,64) | Out-Null
    if ($c.ToString() -ne '#32770' -or -not [RC]::IsWindowVisible($h)) { return $true }
    if ([RC]::GetDlgItem($h, $Id) -ne [IntPtr]::Zero) { [RC]::Found = $h; return $false }
    return $true }
  [RC]::Found = [IntPtr]::Zero
  [RC]::EnumWindows($cbk,[IntPtr]::Zero) | Out-Null
  $target = [RC]::Found
  if ($target -ne [IntPtr]::Zero) { [RC]::SetForegroundWindow($target) | Out-Null; Start-Sleep -Milliseconds 500 }
}
if ($target -eq [IntPtr]::Zero) { $target = [RC]::GetForegroundWindow() }
$t2 = New-Object System.Text.StringBuilder 256
[RC]::GetWindowTextW($target,$t2,256) | Out-Null
$c = [RC]::GetDlgItem($target, $Id)
if ($c -eq [IntPtr]::Zero) { Write-Output ("MISSING id=" + $Id + " on [" + $t2.ToString() + "]"); exit 1 }
$r = New-Object RC+R
[RC]::GetWindowRect($c, [ref]$r) | Out-Null
$x = [int](($r.l + $r.r) / 2); $y = [int](($r.t + $r.b) / 2)
[RC]::SetCursorPos($x, $y) | Out-Null
Start-Sleep -Milliseconds 150
[RC]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
Start-Sleep -Milliseconds 70
[RC]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
Write-Output ("CLICK " + $Tag + " id=" + $Id + " at " + $x + "," + $y + " on [" + $t2.ToString() + "]")
