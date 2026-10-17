# br-stage1a-backup.ps1 -- 阶段1a：GUI 备份 C: 系统到 H:\BRImages\BR-sys-20261018.wim
# 沿用 set-image-path.ps1 的成熟模式：SetDlgItemTextW + 补发 WM_COMMAND/EN_CHANGE
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrBk3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern bool SetDlgItemTextW(IntPtr hDlg, int id, string text);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetDlgItemTextW(IntPtr hDlg, int id, StringBuilder sb, int max);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public const uint WM_COMMAND = 0x0111;
  public const ushort EN_CHANGE = 0x0300;
  public static string Text(IntPtr h) { var sb = new StringBuilder(1024); GetWindowTextW(h, sb, 1024); return sb.ToString(); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static string ItemText(IntPtr parent, int id) { var sb = new StringBuilder(1024); GetDlgItemTextW(parent, id, sb, 1024); return sb.ToString(); }
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
      string c = Cls(h);
      if (c.StartsWith("BackupRestore")) res.Add(h.ToInt64().ToString() + "|" + c + "|" + Text(h));
    }
    return res;
  }
  static List<IntPtr> kids = new List<IntPtr>();
  static bool KidCb(IntPtr h, IntPtr l) { kids.Add(h); return true; }
  public static List<IntPtr> Kids(IntPtr p) {
    kids.Clear();
    EnumChildWindows(p, KidCb, IntPtr.Zero);
    return new List<IntPtr>(kids);
  }
}
"@
$gui = [BrBk3]::FindGui()
if ($gui -eq [IntPtr]::Zero) { Write-Output 'NO_GUI'; exit 1 }
Write-Output ("gui_hwnd=" + $gui.ToInt64())

# 1) 备份页（tab 1101）
[void][BrBk3]::PostMessageW([BrBk3]::GetDlgItem($gui, 1101), [BrBk3]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 1200

# 2) 写镜像路径 + 补发 EN_CHANGE（关键：否则 GUI 内部状态不知道）
$want = 'H:\BRImages\BR-sys-20261018.wim'
$ok = [BrBk3]::SetDlgItemTextW($gui, 1203, $want)
$ctl = [BrBk3]::GetDlgItem($gui, 1203)
$notify = [IntPtr]([int64]1203 -bor ([int64][BrBk3]::EN_CHANGE -shl 16))
[void][BrBk3]::SendMessageW($gui, [BrBk3]::WM_COMMAND, $notify, $ctl)
Start-Sleep -Milliseconds 500
$after = [BrBk3]::ItemText($gui, 1203)
Write-Output ("path_set_ok=" + $ok + " readback=[" + $after + "] match=" + ($after -eq $want))

# 3) 索引名/保留数（可能是 cue banner，读出来为空也正常）
Write-Output ("index_name=[" + [BrBk3]::ItemText($gui, 1209) + "]")
Write-Output ("keep_n=[" + [BrBk3]::ItemText($gui, 1210) + "]")

# 4) 点「创建任务」（1003）
[void][BrBk3]::PostMessageW([BrBk3]::GetDlgItem($gui, 1003), [BrBk3]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 3500

# 5) 枚举 BackupRestore 相关顶层窗（抓确认框）
Write-Output '--- tops after create ---'
foreach ($line in [BrBk3]::Tops()) { Write-Output ("TOP " + $line) }
Write-Output 'STAGE1A_DRIVE2_DONE'
