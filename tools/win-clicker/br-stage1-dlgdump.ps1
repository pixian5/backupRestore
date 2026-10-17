# br-stage1-dlgdump.ps1 -- dump the modal save dialog (class #32770) children
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrDlg3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Text(IntPtr h) { var sb = new StringBuilder(1024); GetWindowTextW(h, sb, 1024); return sb.ToString(); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  static List<IntPtr> tops = new List<IntPtr>();
  static bool TopCb(IntPtr h, IntPtr l) { tops.Add(h); return true; }
  public static List<IntPtr> Tops() {
    tops.Clear();
    EnumWindows(TopCb, IntPtr.Zero);
    return new List<IntPtr>(tops);
  }
}
"@
foreach ($h in [BrDlg3]::Tops()) {
  $cls = [BrDlg3]::Cls($h)
  $txt = [BrDlg3]::Text($h)
  if ($cls -eq '#32770' -or $txt -like '*镜像*' -or $txt -like '*保存*') {
    Write-Output ("DLG " + $h.ToInt64() + " cls=" + $cls + " title=[" + $txt + "]")
    $cb = [BrDlg3+EnumCb]{ param($k,$l)
      $vis = if ([BrDlg3]::IsWindowVisible($k)) { "1" } else { "0" }
      $id = -1
      try { $id = [BrDlg3]::GetDlgCtrlID($k) } catch {}
      $t = [BrDlg3]::Text($k)
      if ($vis -eq "1" -or $id -ge 0) {
        Write-Output ("  CHILD " + $k.ToInt64() + " id=" + $id + " cls=" + [BrDlg3]::Cls($k) + " vis=" + $vis + " text=[" + $t + "]")
      }
      return $true
    }
    [void][BrDlg3]::EnumChildWindows($h, $cb, [IntPtr]::Zero)
  }
}
Write-Output 'DLG_DUMP_DONE'
