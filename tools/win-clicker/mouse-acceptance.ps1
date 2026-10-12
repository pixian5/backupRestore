# mouse-acceptance.ps1 — 真实鼠标全链路验收（会话 1 前台运行）
# 前置：C:\br-test.json 已配置还原字段但无 auto_install；GUI 已由计划任务启动。
# 流程：真实鼠标点「创建任务」→ 破坏性确认「是」→ 元数据确认「是」→
#       引导菜单编辑器「上移」「保存」→ 完成弹窗「确定」。全程截图取证。
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class MA {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  public struct R { public int l, t, r, b; }
  public const uint DOWN = 2, UP = 4;
  public static string ClickId(IntPtr dlg, int id) {
    SetProcessDPIAware();
    IntPtr c = GetDlgItem(dlg, id);
    if (c == IntPtr.Zero) return "MISSING id=" + id;
    R r; GetWindowRect(c, out r);
    int x = (r.l + r.r) / 2, y = (r.t + r.b) / 2;
    SetCursorPos(x, y);
    System.Threading.Thread.Sleep(120);
    mouse_event(DOWN, 0, 0, 0, UIntPtr.Zero);
    System.Threading.Thread.Sleep(60);
    mouse_event(UP, 0, 0, 0, UIntPtr.Zero);
    return "CLICK id=" + id + " at " + x + "," + y;
  }
  public static string Title(IntPtr h) {
    var s = new System.Text.StringBuilder(256);
    GetWindowTextW(h, s, 256);
    return s.ToString();
  }
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  public static IntPtr FindWindowByText(string needle) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      var s = new System.Text.StringBuilder(256);
      GetWindowTextW(h, s, 256);
      if (s.ToString().Contains(needle) && IsWindowVisible(h)) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@
$shot = { param($name)
  Add-Type -AssemblyName System.Windows.Forms,System.Drawing
  $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
  $bmp.Save("C:\br-verify\mouse-$name.png"); $g.Dispose(); $bmp.Dispose()
}
$log = 'C:\br-verify\mouse-acceptance.log'
function W([string]$m) { Add-Content $log $m }
Set-Content $log ("START " + (Get-Date))
& $shot '1-initial'
$fg = [MA]::GetForegroundWindow()
W ("foreground1=" + [MA]::Title($fg))
# 1) 主窗口「创建任务」按钮（ID 1003）
W ([MA]::ClickId($fg, 1003))
Start-Sleep 2
& $shot '2-destructive-dlg'
# 2) 破坏性确认「是」（MessageBox IDYES=6）
$fg = [MA]::GetForegroundWindow()
W ("fg2=" + [MA]::Title($fg))
W ([MA]::ClickId($fg, 6))
Start-Sleep 2
& $shot '3-metadata-dlg'
# 3) 元数据确认「是」
$fg = [MA]::GetForegroundWindow()
W ("fg3=" + [MA]::Title($fg))
W ([MA]::ClickId($fg, 6))
& $shot '4-online-started'
# 4) 等引导菜单编辑器窗口出现（阶段 2），最长 120 秒
$editor = [IntPtr]::Zero
for ($i = 0; $i -lt 60; $i++) {
  Start-Sleep 2
  if (-not (Get-Process BackupRestore -ErrorAction SilentlyContinue)) { W 'GUI exited early'; break }
  $editor = [MA]::FindWindowByText('引导修复')
  if ($editor -ne [IntPtr]::Zero) { break }
}
if ($editor -ne [IntPtr]::Zero) {
  W ("editor found hwnd=" + $editor)
  [MA]::SetForegroundWindow($editor) | Out-Null
  Start-Sleep 1
  & $shot '5-editor'
  # 编辑器按钮：上移 206 / 保存 208（控件 ID 见 boot_menu_window.rs）
  W ([MA]::ClickId($editor, 206))
  Start-Sleep 1
  W ([MA]::ClickId($editor, 208))
  Start-Sleep 1
  & $shot '6-editor-saved'
} else {
  W 'editor not found in 120s'
}
# 5) 等完成弹窗（最长 20 分钟），真实鼠标点「确定」
$done = $false
for ($i = 0; $i -lt 120; $i++) {
  Start-Sleep 10
  $fg = [MA]::GetForegroundWindow()
  $t = [MA]::Title($fg)
  if ($t -eq '执行完成' -or $t -eq '执行失败') {
    W ("final dlg=" + $t)
    & $shot '7-final'
    W ([MA]::ClickId($fg, 1))
    $done = $true
    break
  }
  if (-not (Get-Process BackupRestore -ErrorAction SilentlyContinue)) { W 'GUI exited without final dlg'; break }
}
if (-not $done) { W 'no final dialog in 20min' }
W ("END " + (Get-Date))
Get-Content $log
