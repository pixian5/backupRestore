# br-stage1-backup-sys.ps1 -- 阶段1a：GUI 备份 C: 系统到 H:\BRImages\BR-sys-20261018.wim
# 步骤：确认备份页 -> 填镜像绝对路径 -> 核对索引名 -> 点「创建任务」-> 枚举新弹窗
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrBackup1 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetWindowTextW(IntPtr h, string s);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
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
  public static List<string> Tops() {
    var res = new List<string>();
    tops.Clear();
    EnumWindows(TopCb, IntPtr.Zero);
    foreach (var h in tops) {
      string t = Text(h);
      if (!string.IsNullOrEmpty(t)) res.Add(h.ToInt64().ToString() + "|" + Cls(h) + "|" + t);
    }
    return res;
  }
}
"@
$gui = [BrBackup1]::FindGui()
if ($gui -eq [IntPtr]::Zero) { Write-Output 'NO_GUI'; exit 1 }
Write-Output ("gui_hwnd=" + $gui.ToInt64())

# 1) 确认停在备份页（tab 1101）
$tab = [BrBackup1]::GetDlgItem($gui, 1101)
Write-Output ("tab1101=" + ($tab -ne [IntPtr]::Zero))
[void][BrBackup1]::PostMessageW($tab, [BrBackup1]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 900

# 2) 填镜像绝对路径（Edit 1203）
$want = 'H:\BRImages\BR-sys-20261018.wim'
$edit = [BrBackup1]::GetDlgItem($gui, 1203)
Write-Output ("edit1203_before=[" + [BrBackup1]::Text($edit) + "]")
[void][BrBackup1]::SetWindowTextW($edit, $want)
Start-Sleep -Milliseconds 300
Write-Output ("edit1203_after=[" + [BrBackup1]::Text($edit) + "]")

# 3) 核对索引名（Edit 1209）与源卷 combo（1202 选中项）
$idx = [BrBackup1]::GetDlgItem($gui, 1209)
Write-Output ("index_name=[" + [BrBackup1]::Text($idx) + "]")
$keep = [BrBackup1]::GetDlgItem($gui, 1210)
Write-Output ("keep_n=[" + [BrBackup1]::Text($keep) + "]")

# 4) 点「创建任务」（1003）
$create = [BrBackup1]::GetDlgItem($gui, 1003)
Write-Output ("create_btn=" + ($create -ne [IntPtr]::Zero))
[void][BrBackup1]::PostMessageW($create, [BrBackup1]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 3000

# 5) 枚举顶层窗（抓确认框）
Write-Output '--- top windows after create ---'
foreach ($line in [BrBackup1]::Tops()) { Write-Output ("TOP " + $line) }
Write-Output 'STAGE1A_DRIVE_DONE'
