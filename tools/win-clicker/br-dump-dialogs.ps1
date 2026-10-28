# br-dump-dialogs.ps1 -- 只读探测：列出当前所有顶层窗口及其按钮子控件的文字（不点任何东西）。
# 用途：在无法用肉眼点击的场景下，先看清弹窗里有哪些按钮、标题是什么，再决定点哪一个。
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrDlg {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Text(IntPtr h) { var sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  static List<IntPtr> tops = new List<IntPtr>();
  static bool TopCb(IntPtr h, IntPtr l) { tops.Add(h); return true; }
  static List<IntPtr> kids = new List<IntPtr>();
  static bool KidCb(IntPtr h, IntPtr l) { kids.Add(h); return true; }
  public static List<string> Dump() {
    var res = new List<string>();
    tops.Clear(); EnumWindows(TopCb, IntPtr.Zero);
    foreach (var h in tops) {
      if (!IsWindowVisible(h)) continue;
      string t = Text(h);
      if (t.Length == 0) continue;
      res.Add("TOP hwnd=" + h.ToInt64() + " class=" + Cls(h) + " title=[" + t + "]");
      kids.Clear(); EnumChildWindows(h, KidCb, IntPtr.Zero);
      foreach (var k in kids) {
        string kc = Cls(k);
        string kt = Text(k);
        if (kc == "Button" && kt.Length > 0) {
          res.Add("    BTN hwnd=" + k.ToInt64() + " text=[" + kt + "]");
        }
      }
    }
    return res;
  }
}
"@
$utf8 = New-Object System.Text.UTF8Encoding($false)
$out = [BrDlg]::Dump() -join "`r`n"
[System.IO.File]::WriteAllText("H:\brwork\dlg-dump.txt", $out, $utf8)
Write-Output "DUMP_DONE"
