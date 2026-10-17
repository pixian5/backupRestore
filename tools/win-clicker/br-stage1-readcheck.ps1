# br-stage1-readcheck.ps1 -- 读当前 1203 编辑框内容 + 取消引导对话框
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrRc3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetDlgItemTextW(IntPtr hDlg, int id, StringBuilder sb, int max);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public static string Text(IntPtr h) { var sb = new StringBuilder(1024); GetWindowTextW(h, sb, 1024); return sb.ToString(); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  static List<IntPtr> tops = new List<IntPtr>();
  static bool TopCb(IntPtr h, IntPtr l) { tops.Add(h); return true; }
  public static IntPtr FindGui() {
    tops.Clear();
    EnumWindows(TopCb, IntPtr.Zero);
    foreach (var h in tops) if (Cls(h) == "BackupRestoreNativeGui") return h;
    return IntPtr.Zero;
  }
  public static IntPtr FindByClass(string cls) {
    tops.Clear();
    EnumWindows(TopCb, IntPtr.Zero);
    foreach (var h in tops) if (Cls(h) == cls) return h;
    return IntPtr.Zero;
  }
  static List<IntPtr> kids = new List<IntPtr>();
  static bool KidCb(IntPtr h, IntPtr l) { kids.Add(h); return true; }
  public static List<IntPtr> Kids(IntPtr p) {
    kids.Clear();
    EnumChildWindows(p, KidCb, IntPtr.Zero);
    return new List<IntPtr>(kids);
  }
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
}
"@
$gui = [BrRc3]::FindGui()
if ($gui -eq [IntPtr]::Zero) { Write-Output 'NO_GUI'; exit 1 }
$sb = New-Object System.Text.StringBuilder 1024
$n = [BrRc3]::GetDlgItemTextW($gui, 1203, $sb, 1024)
Write-Output ("edit1203_now_len=" + $n + " content=[" + $sb.ToString() + "]")
$btnSb = New-Object System.Text.StringBuilder 256
[void][BrRc3]::GetDlgItemTextW($gui, 1003, $btnSb, 256)
Write-Output ("btn1003=[" + $btnSb.ToString() + "]")
# 取消当前引导对话框（若有）
$dlg = [BrRc3]::FindByClass('BackupRestoreInvalidPathChoice')
if ($dlg -ne [IntPtr]::Zero) {
  foreach ($k in [BrRc3]::Kids($dlg)) {
    if ([BrRc3]::Text($k) -eq '取消') {
      [void][BrRc3]::PostMessageW($k, [BrRc3]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
      Write-Output 'guidance_cancelled'
    }
  }
  Start-Sleep -Milliseconds 700
  Write-Output ("guidance_still_open=" + [BrRc3]::IsWindow($dlg))
} else {
  Write-Output 'no_guidance_dialog'
}
Write-Output 'READCHECK_DONE'
