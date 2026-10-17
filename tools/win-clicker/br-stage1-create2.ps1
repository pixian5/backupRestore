# br-stage1-create2.ps1 -- 恢复按钮文本为「创建任务」并点击，等待 PE/RE 选择框
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrCr3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern bool SetDlgItemTextW(IntPtr hDlg, int id, string text);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetDlgItemTextW(IntPtr hDlg, int id, StringBuilder sb, int max);
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
  public static List<string> BrTops() {
    var res = new List<string>();
    tops.Clear();
    EnumWindows(TopCb, IntPtr.Zero);
    foreach (var h in tops) {
      string c = Cls(h);
      if (c.StartsWith("BackupRestore")) res.Add(h.ToInt64().ToString() + "|" + c + "|" + Text(h));
    }
    return res;
  }
}
"@
$gui = [BrCr3]::FindGui()
if ($gui -eq [IntPtr]::Zero) { Write-Output 'NO_GUI'; exit 1 }

# 1) 恢复「创建任务」按钮文本（= 浏览成功后的状态，GUI 自身第 4600 行同样逻辑）
$btn = [BrCr3]::GetDlgItem($gui, 1003)
Write-Output ("btn_before=[" + [BrCr3]::Text($btn) + "]")
[void][BrCr3]::SetDlgItemTextW($gui, 1003, '创建任务')
Start-Sleep -Milliseconds 300
Write-Output ("btn_after=[" + [BrCr3]::Text($btn) + "]")

# 2) 点「创建任务」
[void][BrCr3]::PostMessageW($btn, [BrCr3]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 3500

# 3) 枚举 BackupRestore 系窗口（抓 PE/RE 选择框）
Write-Output '--- br tops ---'
foreach ($line in [BrCr3]::BrTops()) { Write-Output ("TOP " + $line) }
Write-Output 'STAGE1_CREATE2_DONE'
