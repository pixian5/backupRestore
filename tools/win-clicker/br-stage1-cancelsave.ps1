# br-stage1-cancelsave.ps1 -- close the modal save dialog without touching the GUI thread synchronously
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrCancel3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, IntPtr dwExtraInfo);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint WM_COMMAND = 0x0111;
  public static string Text(IntPtr h) { var sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  static List<IntPtr> tops = new List<IntPtr>();
  static bool TopCb(IntPtr h, IntPtr l) { tops.Add(h); return true; }
  public static IntPtr FindByTitle(string contains) {
    tops.Clear();
    EnumWindows(TopCb, IntPtr.Zero);
    foreach (var h in tops) if (Text(h).Contains(contains)) return h;
    return IntPtr.Zero;
  }
}
"@
$dlg = [BrCancel3]::FindByTitle('选择备份镜像保存位置')
if ($dlg -eq [IntPtr]::Zero) { Write-Output 'SAVE_DIALOG_NOT_FOUND'; exit 0 }
Write-Output ("save_dialog=" + $dlg.ToInt64())
# 方式1：WM_COMMAND / IDCANCEL(2)
[void][BrCancel3]::PostMessageW($dlg, [BrCancel3]::WM_COMMAND, [IntPtr]2, [IntPtr]::Zero)
Start-Sleep -Milliseconds 900
if ([BrCancel3]::IsWindow($dlg)) {
  Write-Output 'still_open_after_wm_command_cancel'
  # 方式2：置前台 + 硬件级 Escape
  [void][BrCancel3]::SetForegroundWindow($dlg)
  Start-Sleep -Milliseconds 400
  [BrCancel3]::keybd_event(0x1B, 0, 0, [IntPtr]::Zero)
  [BrCancel3]::keybd_event(0x1B, 0, 2, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 900
}
if ([BrCancel3]::IsWindow($dlg)) {
  Write-Output 'SAVE_DIALOG_STILL_OPEN'
} else {
  Write-Output 'SAVE_DIALOG_CLOSED'
}
