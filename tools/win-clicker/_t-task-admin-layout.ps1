# _t-task-admin-layout.ps1 - open the "Tasks & evidence" window, run the SAME five
# geometry predicates against it, then close it.
#
# Why a driver: the geometry script (/_t-layout-rects.ps1) inspects a window that is
# already on screen. This one clicks ID_REFRESH_TASK (1004) to raise the modal window,
# hands over, and closes it afterwards - a modal window left open looks like a hang.
$out = 'H:\brwork\task-admin-layout-driver.txt'
function Say($s) { $s | Add-Content -Path $out -Encoding UTF8 }
Set-Content -Path $out -Value ("STARTED " + (Get-Date -Format o)) -Encoding UTF8

# ── 诊断前言（2026-10-27 05:50 补）──────────────────────────────────────────
# 上一版在 GUI_NOT_FOUND 时只写一个词，于是「通道不对」与「窗口真没有」分不开，
# 只能记成「疑窗口站差异」——这违反本项目判据纪律（读不到 ≠ 判据不满足）。
# 现在失败时必须先自证：我在哪个会话/窗口站/桌面、系统里有几个顶层窗口、
# GUI 进程在不在。判读：
#   SESSION≠1 或 WINSTA≠WinSta0 或 DESKTOP≠Default → 通道问题（仪器）
#   TOP_LEVEL=0 但 GUI 进程在                → 环境问题（仪器）
#   TOP_LEVEL>0 且 GUI 进程在、类名不命中      → 真问题（产品/类名变了）
# ★ Add-Type 失败在 PS 5.1 是**非终止错误**：类型没定义、脚本继续跑，症状是
#   「枚举永远返回 0」，与「窗口不存在」长得一模一样。必须 try/catch 落文件。
try { Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Ctx3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetProcessWindowStation();
  [DllImport("user32.dll")] public static extern IntPtr GetThreadDesktop(uint tid);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)]
  public static extern bool GetUserObjectInformationW(IntPtr h, int idx, StringBuilder buf, int len, out int need);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string ObjName(IntPtr h) {
    StringBuilder sb = new StringBuilder(256); int need;
    if (!GetUserObjectInformationW(h, 2, sb, 512, out need)) return "ERR";
    return sb.ToString();
  }
}
'@ } catch { Say ("ADDTYPE_ERROR_Ctx3=" + $_.Exception.Message) }
function Say-Ctx {
  Say ("PID=" + $PID + " PROC_SESSION=" + (Get-Process -Id $PID).SessionId)
  Say ("WINSTA=" + [Ctx3]::ObjName([Ctx3]::GetProcessWindowStation()) + " DESKTOP=" + [Ctx3]::ObjName([Ctx3]::GetThreadDesktop([Ctx3]::GetCurrentThreadId())))
  $ps = Get-Process BackupRestore -ErrorAction SilentlyContinue
  Say ("PROC_COUNT=" + @($ps).Count)
  foreach ($p in $ps) { Say ("PROC pid=" + $p.Id + " session=" + $p.SessionId + " mainwnd=" + $p.MainWindowHandle) }
  $script:tl = 0
  $cb0 = [Ctx3+EnumCb]{ param($h,$l) $script:tl++; return $true }
  [void][Ctx3]::EnumWindows($cb0, [IntPtr]::Zero)
  Say ("TOP_LEVEL=" + $script:tl)
}

try { Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class RS3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  // CB_GETCOUNT 不会触发 CBN_SELCHANGE 之类的通知，故这里用同步 SendMessage 是安全的
  // （踩过的坑是 CB_SETCURSEL 会触发通知回调，那才会重入锁）。
  [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendMsg(IntPtr h, uint m, IntPtr w, IntPtr l);  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, ref RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, ref RECT r);
  [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr h, ref POINT p);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
'@ } catch {
  # 类型没建起来 → 后面每一条判据都是假的。这是「仪器坏了」，退出码 4 与
  # 「窗口不存在（1/3）」分开，避免把仪器故障训练成红判据。
  Say ("ADDTYPE_ERROR_RS3=" + $_.Exception.Message)
  Say "DONE"
  exit 4
}
function Find-Win($cls) {
  # ★ 2026-10-27 05:52 真因：委托回调从原生 EnumWindows 返回时跑在**新作用域**，
  #   外层函数的参数 $cls 在那里是 $null → 比较永远为假 → 稳定 GUI_NOT_FOUND。
  #   上一版把这个假象记成「疑窗口站/桌面差异」，白查两小时。
  #   解法：目标类名走 $script:（同作用域下 $script:hit 在回调里已被证实可见）。
  $script:want = $cls
  $script:hit = [IntPtr]::Zero
  $cb = [RS3+EnumCb]{ param($h,$l) if ([RS3]::Cls($h) -eq $script:want) { $script:hit = $h; return $false }; return $true }
  [void][RS3]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:hit
}
$gui = Find-Win 'BackupRestoreNativeGui'
if ($gui -eq [IntPtr]::Zero) {
  # 先自证读数侧，再谈「找不到」。
  Say-Ctx
  $sess = (Get-Process -Id $PID).SessionId
  $ws = [Ctx3]::ObjName([Ctx3]::GetProcessWindowStation())
  $dt = [Ctx3]::ObjName([Ctx3]::GetThreadDesktop([Ctx3]::GetCurrentThreadId()))
  $verdict = 'GUI_WINDOW_MISSING'
  if ($sess -ne 1 -or $ws -ne 'WinSta0' -or $dt -ne 'Default' -or $script:tl -eq 0) { $verdict = 'GUI_ENV_BAD' }
  Say ("VERDICT=" + $verdict)
  Say "DONE"; exit 1
}
$btn = [RS3]::GetDlgItem($gui, 1004)
if ($btn -eq [IntPtr]::Zero) { Say "REFRESH_BUTTON_NULL"; Say "DONE"; exit 2 }
[void][RS3]::PostMessageW($btn, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)   # BM_CLICK
Start-Sleep -Milliseconds 2000
$win = Find-Win 'BackupRestoreTaskAdmin'
Say ("WINDOW=" + [int]($win -ne [IntPtr]::Zero))
if ($win -eq [IntPtr]::Zero) { Say "DONE"; exit 3 }
# 内联几何判据（OVERLAP / OUTSIDE / ZERO_SIZE）。不跨脚本调用：那条路试过一次
# 没有任何输出，而这几条判据本身只有二十行，自己读自己算最可靠。
$script:rects = New-Object System.Collections.ArrayList
$cbChild = [RS3+EnumCb]{ param($h,$l)
  if ([RS3]::GetParent($h) -eq $win -and ([RS3]::IsWindowVisible($h))) {
    $r = New-Object RS3+RECT
    [void][RS3]::GetWindowRect($h, [ref]$r)
    $pt = New-Object RS3+POINT
    $pt.X = $r.Left; $pt.Y = $r.Top
    [void][RS3]::ScreenToClient($win, [ref]$pt)
    $pt2 = New-Object RS3+POINT
    $pt2.X = $r.Right; $pt2.Y = $r.Bottom
    [void][RS3]::ScreenToClient($win, [ref]$pt2)
    [void]$script:rects.Add([pscustomobject]@{
      Hwnd = $h; Id = [RS3]::GetDlgCtrlID($h); X = $pt.X; Y = $pt.Y
      W = ($pt2.X - $pt.X); H = ($pt2.Y - $pt.Y)
    })
  }
  return $true }
[void][RS3]::EnumChildWindows($win, $cbChild, [IntPtr]::Zero)
$cw = 0; $ch = 0
$cr = New-Object RS3+RECT
[void][RS3]::GetClientRect($win, [ref]$cr)
$cw = $cr.Right; $ch = $cr.Bottom
Say ("CLIENT_W=" + $cw + " CLIENT_H=" + $ch + " CHILDREN=" + $script:rects.Count)
foreach ($r in $script:rects) { Say ("  RECT id=" + $r.Id + " x=" + $r.X + " y=" + $r.Y + " w=" + $r.W + " h=" + $r.H) }
$overlap = 0; $outside = 0; $zero = 0
for ($i = 0; $i -lt $script:rects.Count; $i++) {
  $a = $script:rects[$i]
  if ($a.W -le 0 -or $a.H -le 0) { $zero++ }
  if ($a.X -lt 0 -or $a.Y -lt 0 -or ($a.X + $a.W) -gt $cw -or ($a.Y + $a.H) -gt $ch) {
    $outside++
    Say ("  OUTSIDE id=" + $a.Id + " x=" + $a.X + " y=" + $a.Y + " w=" + $a.W + " h=" + $a.H)
  }
  for ($j = $i + 1; $j -lt $script:rects.Count; $j++) {
    $b = $script:rects[$j]
    $ox = [Math]::Min($a.X + $a.W, $b.X + $b.W) - [Math]::Max($a.X, $b.X)
    $oy = [Math]::Min($a.Y + $a.H, $b.Y + $b.H) - [Math]::Max($a.Y, $b.Y)
    if ($ox -gt 0 -and $oy -gt 0) { $overlap++; Say ("  OVERLAP id=" + $a.Id + " x id=" + $b.Id + " = " + $ox + "x" + $oy) }
  }
}
Say ("OVERLAP_COUNT=" + $overlap)
Say ("OUTSIDE_COUNT=" + $outside)
Say ("ZERO_SIZE=" + $zero)

# ── 存在性判据：与签入的基线比，两个方向都验 ──────────────────────────────
# 只验几何是不够的：控件**根本没创建**时，重叠/越界/塌缩三条全是 0（没东西可
# 重叠），于是假绿。真值用签入的 control-baseline-taskadmin.txt，不解析源码。
$basePath = '\\Mac\backupRestore\tools\win-clicker\control-baseline-taskadmin.txt'
if (-not (Test-Path -LiteralPath $basePath)) {
  Say "BASELINE_UNREADABLE=1"
  Say ("BASELINE_PATH=" + $basePath)
} else {
  $want = @{}
  foreach ($line in (Get-Content -LiteralPath $basePath -Encoding UTF8)) {
    $t = $line.Trim()
    if ($t -eq '' -or $t.StartsWith('#')) { continue }
    $parts = $t.Split('|')
    if ($parts.Count -ge 2) { $want[$parts[0].Trim()] = $parts[1].Trim() }
  }
  $have = @{}
  foreach ($r in $script:rects) { $have[[string]$r.Id] = [RS3]::Cls([IntPtr]$r.Hwnd) }
  $missing = 0; $stale = 0; $classBad = 0
  foreach ($id in $want.Keys) {
    if (-not $have.ContainsKey($id)) { $missing++; Say ("  MISSING id=" + $id + " want=" + $want[$id]) }
    elseif ($want[$id] -ne '' -and $have[$id].ToUpper() -ne $want[$id].ToUpper()) {
      # 控件还在、类型变了（比如按钮退化成静态文本）——这类回归只看 id 是抓不到的。
      $classBad++
      Say ("  CLASS_MISMATCH id=" + $id + " want=" + $want[$id] + " got=" + $have[$id])
    }
  }
  foreach ($id in $have.Keys) {
    if (-not $want.ContainsKey($id)) { $stale++; Say ("  STALE id=" + $id + " class=" + $have[$id]) }
  }
  Say ("BASELINE_WANT=" + $want.Count + " BASELINE_HAVE=" + $have.Count)
  Say ("MISSING_CONTROLS=" + $missing)
  Say ("BASELINE_STALE=" + $stale)
  Say ("CLASS_MISMATCH=" + $classBad)
}

# ── 行为判据 T1：下拉项数必须等于磁盘上的任务目录数 ────────────────────────
# 「窗口出现了」是弱判据；项数对上才证明它真读到了全部任务。
$combo = [RS3]::GetDlgItem($win, 1401)
$items = -1
if ($combo -ne [IntPtr]::Zero) { $items = [int][RS3]::SendMsg($combo, 0x0146, [IntPtr]::Zero, [IntPtr]::Zero) }
$exe = (Get-Process BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1).Path
$dirCount = -1
if ($exe) {
  $tasksDir = Join-Path (Split-Path -Parent $exe) 'tasks'
  if (Test-Path -LiteralPath $tasksDir) { $dirCount = @(Get-ChildItem -LiteralPath $tasksDir -Directory -ErrorAction SilentlyContinue).Count }
}
Say ("COMBO_ITEMS=" + $items + " TASK_DIRS=" + $dirCount)
Say ("T1_ITEMS_MATCH_DIRS=" + [int]($items -ge 0 -and $items -eq $dirCount))
# Close it: a modal window left up blocks the whole app for the user.
[void][RS3]::PostMessageW($win, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)   # WM_CLOSE
for ($i = 0; $i -lt 40; $i++) {
  Start-Sleep -Milliseconds 100
  if ((Find-Win 'BackupRestoreTaskAdmin') -eq [IntPtr]::Zero) { break }
}
Say ("CLOSED=" + [int]((Find-Win 'BackupRestoreTaskAdmin') -eq [IntPtr]::Zero))
Say "DONE"
