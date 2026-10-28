# br-choose-pe-env.ps1 -- 在「需要进入恢复环境处理」弹窗里点「进入 PE（推荐）」。
# 按窗口类名 BackupRestoreSystemDriveChoice 定位弹窗，再按按钮文字里含 "PE" 这个 ASCII 特征选第一个按钮
# （不写死 hwnd，也不依赖中文匹配——中文只为日志留证）。
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrChoice {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public static string Text(IntPtr h) { var sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  static List<IntPtr> tops = new List<IntPtr>();
  static bool TopCb(IntPtr h, IntPtr l) { tops.Add(h); return true; }
  static List<IntPtr> kids = new List<IntPtr>();
  static bool KidCb(IntPtr h, IntPtr l) { kids.Add(h); return true; }
  public static string ClickPe() {
    var log = new StringBuilder();
    tops.Clear(); EnumWindows(TopCb, IntPtr.Zero);
    foreach (var h in tops) {
      if (Cls(h) != "BackupRestoreSystemDriveChoice") continue;
      log.AppendLine("dialog hwnd=" + h.ToInt64() + " title=[" + Text(h) + "] visible=" + IsWindowVisible(h));
      kids.Clear(); EnumChildWindows(h, KidCb, IntPtr.Zero);
      foreach (var k in kids) {
        if (Cls(k) != "Button") continue;
        string kt = Text(k);
        log.AppendLine("  btn=[" + kt + "]");
        if (kt.Contains("PE")) {
          bool ok = PostMessageW(k, BM_CLICK, IntPtr.Zero, IntPtr.Zero);
          log.AppendLine("  CLICKED [" + kt + "] post=" + ok);
          return log.ToString();
        }
      }
    }
    log.AppendLine("NO_DIALOG_OR_BUTTON");
    return log.ToString();
  }
}
"@
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\choose-pe-env.txt", [BrChoice]::ClickPe(), $utf8)
Write-Output "CHOOSE_DONE"
