# br-stage1-fgprobe.ps1 -- foreground window + save dialog rect
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrFg3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public struct R { public int l, t, r, b; }
  public static string Text(IntPtr h) { var sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  static List<IntPtr> tops = new List<IntPtr>();
  static bool TopCb(IntPtr h, IntPtr l) { tops.Add(h); return true; }
  public static List<IntPtr> Tops() { tops.Clear(); EnumWindows(TopCb, IntPtr.Zero); return new List<IntPtr>(tops); }
}
"@
$fg = [BrFg3]::GetForegroundWindow()
Write-Output ("foreground=" + $fg.ToInt64() + " cls=" + [BrFg3]::Cls($fg) + " title=[" + [BrFg3]::Text($fg) + "]")
$screen = New-Object BrFg3+R
[void][BrFg3]::GetWindowRect($fg, [ref]$screen)
Write-Output ("screen_rect=" + $screen.l + "," + $screen.t + "," + $screen.r + "," + $screen.b)
foreach ($h in [BrFg3]::Tops()) {
  $t = [BrFg3]::Text($h)
  if ($t -like '*镜像*' -or [BrFg3]::Cls($h) -eq '#32770') {
    $r = New-Object BrFg3+R
    [void][BrFg3]::GetWindowRect($h, [ref]$r)
    $procId = 0
    [void][BrFg3]::GetWindowThreadProcessId($h, [ref]$procId)
    Write-Output ("DLG " + $h.ToInt64() + " pid=" + $procId + " rect=" + $r.l + "," + $r.t + "," + $r.r + "," + $r.b + " title=[" + $t + "]")
  }
}
Write-Output 'FG_PROBE_DONE'
