# rename-editor-entry.ps1 — 真实鼠标聚焦引导菜单编辑器的名称框，键入后缀并保存
$ErrorActionPreference='Continue'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class RE {
  public delegate bool CB(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(CB cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public struct R { public int l, t, r, b; }
  public static IntPtr Found = IntPtr.Zero;
}
"@
$m=[IntPtr]::Zero
$cb=[RE+CB]{ param($h,$l)
  $c=New-Object System.Text.StringBuilder 64
  [RE]::GetClassNameW($h,$c,64)|Out-Null
  if($c.ToString() -eq 'BackupRestoreBootMenuClass' -and [RE]::IsWindowVisible($h)){ $script:m=$h; return $false }
  return $true }
[RE]::EnumWindows($cb,[IntPtr]::Zero)|Out-Null
if($m -eq [IntPtr]::Zero){ 'NO_EDITOR'; exit 1 }
[RE]::SetForegroundWindow($m)|Out-Null
Start-Sleep -Milliseconds 500
# 真实鼠标点击名称编辑框（ID 202）
$edit=[RE]::GetDlgItem($m,202)
$r=New-Object RE+R
[RE]::GetWindowRect($edit,[ref]$r)|Out-Null
$x=[int](($r.l+$r.r)/2); $y=[int](($r.t+$r.b)/2)
[RE]::SetCursorPos($x,$y)|Out-Null
Start-Sleep -Milliseconds 150
[RE]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
Start-Sleep -Milliseconds 70
[RE]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
Start-Sleep -Milliseconds 400
# 全选并键入新名（Ctrl+A 后输入）
[System.Windows.Forms.SendKeys]::SendWait('^a')
Start-Sleep -Milliseconds 200
[System.Windows.Forms.SendKeys]::SendWait('Mouse Edited Entry')
Start-Sleep -Milliseconds 400
# 真实鼠标点保存（ID 208）
$save=[RE]::GetDlgItem($m,208)
[RE]::GetWindowRect($save,[ref]$r)|Out-Null
$x=[int](($r.l+$r.r)/2); $y=[int](($r.t+$r.b)/2)
[RE]::SetCursorPos($x,$y)|Out-Null
Start-Sleep -Milliseconds 150
[RE]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
Start-Sleep -Milliseconds 70
[RE]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
'RENAMED_AND_SAVED'
