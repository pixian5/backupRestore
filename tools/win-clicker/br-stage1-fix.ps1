# br-stage1-fix.ps1 -- 修复阶段1a驱动：取消「没有输入镜像路径」模态框，回备份页，正确写入镜像路径
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrBackup2 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern bool SetWindowTextW(IntPtr h, string s);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public static string Text(IntPtr h) { var sb = new StringBuilder(1024); GetWindowTextW(h, sb, 1024); return sb.ToString(); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  static List<IntPtr> tops = new List<IntPtr>();
  static bool TopCb(IntPtr h, IntPtr l) { tops.Add(h); return true; }
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
  public static IntPtr ClickByText(IntPtr parent, string text) {
    foreach (var k in Kids(parent)) {
      if (Text(k) == text) {
        PostMessageW(k, BM_CLICK, IntPtr.Zero, IntPtr.Zero);
        return k;
      }
    }
    return IntPtr.Zero;
  }
}
"@

# 1) 取消「没有输入镜像路径」模态框
$dlg = [BrBackup2]::FindByClass('BackupRestoreInvalidPathChoice')
if ($dlg -ne [IntPtr]::Zero) {
  Write-Output ("dlg_hwnd=" + $dlg.ToInt64())
  foreach ($k in [BrBackup2]::Kids($dlg)) {
    $t = [BrBackup2]::Text($k)
    if ($t) { Write-Output ("  dlgchild: " + $k.ToInt64() + " [" + [BrBackup2]::Cls($k) + "] " + $t) }
  }
  $cancel = [BrBackup2]::ClickByText($dlg, '取消')
  Write-Output ("cancel_clicked=" + ($cancel -ne [IntPtr]::Zero))
  Start-Sleep -Milliseconds 800
  Write-Output ("dlg_still_open=" + [BrBackup2]::IsWindow($dlg))
} else {
  Write-Output 'no_invalid_path_dialog'
}

# 2) 回备份页
$gui = [BrBackup2]::FindByClass('BackupRestoreNativeGui')
if ($gui -eq [IntPtr]::Zero) { Write-Output 'NO_GUI'; exit 1 }
$tab = [BrBackup2]::GetDlgItem($gui, 1101)
[void][BrBackup2]::PostMessageW($tab, [BrBackup2]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 1500

# 3) 读取并正确写入镜像路径（Edit 1203）
$want = 'H:\BRImages\BR-sys-20261018.wim'
$edit = [BrBackup2]::GetDlgItem($gui, 1203)
Write-Output ("edit1203_before=[" + [BrBackup2]::Text($edit) + "]")
[void][BrBackup2]::SetWindowTextW($edit, $want)
Start-Sleep -Milliseconds 400
$after = [BrBackup2]::Text($edit)
Write-Output ("edit1203_after=[" + $after + "]")
Write-Output ("path_ok=" + ($after -eq $want))

# 4) 核对索引名（1209）与保留数（1210）
$idx = [BrBackup2]::GetDlgItem($gui, 1209)
$keep = [BrBackup2]::GetDlgItem($gui, 1210)
Write-Output ("index_name=[" + [BrBackup2]::Text($idx) + "]")
Write-Output ("keep_n=[" + [BrBackup2]::Text($keep) + "]")
Write-Output 'STAGE1_FIX_DONE'
