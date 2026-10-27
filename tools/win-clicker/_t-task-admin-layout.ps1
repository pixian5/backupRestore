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
  [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendMsg(IntPtr h, uint m, IntPtr w, IntPtr l);
  // 文本与字体度量：WM_GETTEXT 是系统会封送的消息，但缓冲区必须**在我们自己进程里**
  // 且用 AllocHGlobal——踩过的坑是用 StringBuilder 封送只读回 3 个字符。
  [DllImport("user32.dll", EntryPoint="SendMessageTimeoutW")] public static extern IntPtr SendMsgT(IntPtr h, uint m, IntPtr w, IntPtr l, uint f, uint t, out IntPtr res);
  [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
  [DllImport("gdi32.dll")] public static extern IntPtr SelectObject(IntPtr dc, IntPtr o);
  [DllImport("gdi32.dll", CharSet=CharSet.Unicode)] public static extern bool GetTextExtentPoint32W(IntPtr dc, string s, int c, out SIZE sz);
  [DllImport("gdi32.dll")] public static extern bool GetTextMetricsW(IntPtr dc, out TM tm);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int idx);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  // ★ 跨进程改控件文字必须用 SetWindowTextW(本进程字符串由系统封送)。
  //   2026-10-27 实测：自己拼 SendMessageW(WM_SETTEXT, ptr) 从另一个进程发**不生效**
  //   （回读长度仍是旧值 7），于是「量尺寸」量的是旧文字——仪器自欺的典型。
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool SetWindowTextW(IntPtr h, string s);
  [StructLayout(LayoutKind.Sequential)] public struct SIZE { public int cx, cy; }
  [StructLayout(LayoutKind.Sequential)] public struct TM {
    public int tmHeight, tmAscent, tmDescent, tmInternalLeading, tmExternalLeading;
    public int tmAveCharWidth, tmMaxCharWidth, tmWeight, tmOverhang, tmDigitizedAspectX, tmDigitizedAspectY;
    public short tmFirstChar, tmLastChar, tmDefaultChar, tmBreakChar;
    public byte tmItalic, tmUnderlined, tmStruckOut, tmPitchAndFamily, tmCharSet;
  }
  public static string TextOf(IntPtr h) {
    int len = 16384;
    IntPtr buf = Marshal.AllocHGlobal(len * 2);
    IntPtr res;
    SendMsgT(h, 0x000D, (IntPtr)len, buf, 0x0002, 800, out res);
    string s = Marshal.PtrToStringUni(buf);
    Marshal.FreeHGlobal(buf);
    return s == null ? "" : s;
  }
  public static IntPtr FontOf(IntPtr h) { IntPtr res; SendMsgT(h, 0x0031, IntPtr.Zero, IntPtr.Zero, 0x0002, 300, out res); return res; }
  public static int LineCountOf(IntPtr h) { IntPtr res; SendMsgT(h, 0x00BA, IntPtr.Zero, IntPtr.Zero, 0x0002, 300, out res); return (int)res; }
  // 行距（tmHeight + 外部行距）：多行 EDIT 按**行距**排行，不是按字高。
  public static int LinePitch(IntPtr h) {
    IntPtr hfont = FontOf(h); IntPtr dc = GetDC(h); IntPtr old = IntPtr.Zero;
    if (hfont != IntPtr.Zero) old = SelectObject(dc, hfont);
    TM tm = new TM(); GetTextMetricsW(dc, out tm);
    if (old != IntPtr.Zero) SelectObject(dc, old);
    ReleaseDC(h, dc);
    return tm.tmHeight + tm.tmExternalLeading;
  }
  // 最宽一行（用控件自己的字体量）。
  public static int WidestLine(IntPtr h) {
    IntPtr hfont = FontOf(h); IntPtr dc = GetDC(h); IntPtr old = IntPtr.Zero;
    if (hfont != IntPtr.Zero) old = SelectObject(dc, hfont);
    int widest = 0;
    string txt = TextOf(h);
    string[] parts = txt.Replace("\r\n", "\n").Split('\n');
    foreach (string ln in parts) {
      SIZE sz = new SIZE();
      GetTextExtentPoint32W(dc, ln, ln.Length, out sz);
      if (sz.cx > widest) widest = sz.cx;
    }
    if (old != IntPtr.Zero) SelectObject(dc, old);
    ReleaseDC(h, dc);
    return widest;
  }  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, ref RECT r);
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
  Say ("ADDTYPE_ERROR_RS3=" + ($_.ToString() -replace "[\r\n]+", " | "))
  if ($_.Exception.InnerException) { Say ("ADDTYPE_INNER=" + ($_.Exception.InnerException.Message -replace "[\r\n]+", " | ")) }
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
# ── 确认框（#32770）与按钮点击 ────────────────────────────────────────────
# 「清理工作树」会弹 MB_YESNO，等于在**模态窗口里再开一个模态框**；不处理就会把
# 整个程序（和后续所有用例）锁死。所以每次点完都要轮询它是否消失，最后用
# WM_CLOSE 兜底（MessageBox 收到关闭请求 = 取消），绝不留一个框在屏幕上。
# 回调里引用外层变量必须走 $script:（委托从原生回调时跑在新作用域，本文件 §1 已栽过）。
$script:guiPid = 0
function Find-Dialog {
  $script:dlgHit = [IntPtr]::Zero
  if ($script:guiPid -eq 0) { return [IntPtr]::Zero }
  $cb = [RS3+EnumCb]{ param($h,$l)
    if ([RS3]::Cls($h) -eq '#32770' -and [RS3]::IsWindowVisible($h)) {
      $pp = [uint32]0
      [void][RS3]::GetWindowThreadProcessId($h, [ref]$pp)
      if ($pp -eq [uint32]$script:guiPid) { $script:dlgHit = $h; return $false }
    }
    return $true
  }
  [void][RS3]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:dlgHit
}
# 返回：1=点完自然消失；2=只在兜底关窗后消失；3=仍在；0=根本没有框
function Click-DialogButton($id) {
  $dlg = Find-Dialog
  if ($dlg -eq [IntPtr]::Zero) { return 0 }
  $b = [RS3]::GetDlgItem($dlg, $id)
  if ($b -eq [IntPtr]::Zero) { Say ("  DIALOG_NO_BUTTON id=" + $id); return 0 }
  [void][RS3]::PostMessageW($b, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)   # BM_CLICK
  for ($i = 0; $i -lt 30; $i++) {
    Start-Sleep -Milliseconds 100
    if ((Find-Dialog) -eq [IntPtr]::Zero) { return 1 }
  }
  [void][RS3]::PostMessageW($dlg, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)  # WM_CLOSE = 取消
  for ($i = 0; $i -lt 20; $i++) {
    Start-Sleep -Milliseconds 100
    if ((Find-Dialog) -eq [IntPtr]::Zero) { return 2 }
  }
  return 3
}

$gui = Find-Win 'BackupRestoreNativeGui'if ($gui -eq [IntPtr]::Zero) {
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
$script:guiPid = (Get-Process BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1).Id
$btn = [RS3]::GetDlgItem($gui, 1004)
if ($btn -eq [IntPtr]::Zero) { Say "REFRESH_BUTTON_NULL"; Say "DONE"; exit 2 }

# ── 夹具：一个失败任务 + 一个坏记录（2026-10-27 补）──────────────────────
# 起因：本轮第一次跑 T2 得到 `T2_NO_FAILED_TASK=1` —— 50 个历史任务都没有错误码了，
# 于是「详情能显示码」这条判据**因为没有可测对象而恒绿**（空集通过，项目最忌讳）。
# 自带夹具后 T2/T3 每次都有对象可测；跑完删掉，被试的是产品代码而不是现场运气。
# 夹具形状从**磁盘上已有任务**复制再改字段，避免手写 serde 必填字段
# （`Task` 的 created/bootOnce/status/bootPlan.mode 少一个就 `missing field`）。
# ★ 任务目录名必须是 **UUID**：`cleanup_task_artifacts` 走 `task_dir()`，
#   那里会校验 `[BR-CORE-035] task_id is not a UUID`（第一版用
#   「9999-fixture-failed」这种可读名字，界面能显示、清理却直接报错）。
#   于是每次运行生成新 GUID，并用一个标记文件做「上次崩在中途」的自愈。
$FIX_MARK = '.br-fixture-marker'
$FIX_FAIL = [guid]::NewGuid().ToString()
$FIX_BAD  = [guid]::NewGuid().ToString()
$fixtureMade = @()
$exePath = (Get-Process BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1).Path
$tasksRoot = ''
if ($exePath) { $tasksRoot = Join-Path (Split-Path -Parent $exePath) 'tasks' }
function Remove-Fixtures {
  if (-not $tasksRoot) { return }
  foreach ($n in @($FIX_FAIL, $FIX_BAD)) {
    $p = Join-Path $tasksRoot $n
    if (Test-Path -LiteralPath $p) { Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction SilentlyContinue }
  }
  # 上次运行如果在建完夹具后崩了，GUID 名字已经对不上，靠标记文件扫干净。
  foreach ($d in (Get-ChildItem -LiteralPath $tasksRoot -Directory -ErrorAction SilentlyContinue)) {
    if (Test-Path -LiteralPath (Join-Path $d.FullName $FIX_MARK)) {
      Remove-Item -LiteralPath $d.FullName -Recurse -Force -ErrorAction SilentlyContinue
    }
  }
}
if ($tasksRoot -and (Test-Path -LiteralPath $tasksRoot)) {
  Remove-Fixtures   # 先清上一次可能残留的同名夹具，保证计数可预期
  # ★ PS 5.1 的 `Set-Content -Encoding UTF8` **会写 BOM**（EF BB BF），而 serde_json
  #   对 BOM 直接报 `expected value at line 1 column 1` —— 夹具于是被判成「记录损坏」，
  #   而判据显示的却是「详情里没有任务号」，真因被埋了两层。写 JSON 一律走
  #   `[IO.File]::WriteAllText` + `UTF8Encoding($false)`（无 BOM）。
  $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
  function Write-Json($path, $obj) {
    [System.IO.File]::WriteAllText($path, ($obj | ConvertTo-Json -Depth 20), $utf8NoBom)
  }
  $srcDir = $null
  foreach ($d in (Get-ChildItem -LiteralPath $tasksRoot -Directory -ErrorAction SilentlyContinue)) {
    if ($d.Name -eq $FIX_FAIL -or $d.Name -eq $FIX_BAD) { continue }
    if ((Test-Path (Join-Path $d.FullName 'task.json')) -and (Test-Path (Join-Path $d.FullName 'status.json'))) { $srcDir = $d; break }
  }
  if (-not $srcDir) {
    Say "FIXTURE_SKIP=no_source_task"
  } else {
    $now = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    foreach ($spec in @(@{ N = $FIX_FAIL; Bad = $false }, @{ N = $FIX_BAD; Bad = $true })) {
      try {
        $dst = Join-Path $tasksRoot $spec.N
        New-Item -ItemType Directory -Path $dst -Force | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $dst $FIX_MARK), 'created by _t-task-admin-layout.ps1')
        $task = Get-Content -LiteralPath (Join-Path $srcDir.FullName 'task.json') -Raw -Encoding UTF8 | ConvertFrom-Json
        $task.taskId = $spec.N
        # ★ core 的 cleanup_task_artifacts 有三道守卫：未完成禁止清理、
        #   `status.stage` 必须等于 `task.status`、`mount` 必须为空。
        #   夹具从真任务复制出来的 `task.status` 是 success，只改 status.json 的话
        #   清理会报 BR-CORE-059——所以两边必须一起改。
        if (-not $spec.Bad) { $task.status = 'failed' }
        Write-Json (Join-Path $dst 'task.json') $task
        if ($spec.Bad) {
          # 坏记录：状态文件根本不是 JSON。产品应把它标成「记录损坏」而不是整窗打不开。
          Set-Content -LiteralPath (Join-Path $dst 'status.json') -Value ('{ "taskId": "' + $spec.N + '", "stage": ') -Encoding UTF8
        } else {
          $st = Get-Content -LiteralPath (Join-Path $srcDir.FullName 'status.json') -Raw -Encoding UTF8 | ConvertFrom-Json
          $st.taskId = $spec.N
          $st.stage = 'failed'
          $st.progress = 0
          # ★ 源记录里**未必有这些字段**（正常任务 status.json 没有 error/errorCodeText），
          #   而 PSCustomObject 不能靠赋值新增属性 → 必须 Add-Member，否则报
          #   `errorCodeText:errorCodeText` 这种看不出所以然的错（本轮踩过一次）。
          foreach ($kv in @(@{ K = 'error'; V = 'fixture: 镜像文件不存在' }, @{ K = 'errorCodeText'; V = 'BR-PE-039' })) {
            if ($st.PSObject.Properties.Name -contains $kv.K) {
              $st.($kv.K) = $kv.V
            } else {
              $st | Add-Member -NotePropertyName $kv.K -NotePropertyValue $kv.V
            }
          }
          $st.updated = $now       # 最新 → 排序落在第一项，选中位置可预期
          Write-Json (Join-Path $dst 'status.json') $st
        }
        if (-not $spec.Bad) {
          # 工作树：清理白名单是 mount/stage/original/payload，且 **mount 非空会报
          # BR-CORE-060（还有挂载残留）**，所以只堆 stage 与 payload，mount 干脆不建。
          foreach ($wt in @('stage', 'payload')) {
            $wd = Join-Path $dst $wt
            New-Item -ItemType Directory -Path $wd -Force | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $wd 'leftover.bin'), 'x')
          }
        }
        $fixtureMade += $spec.N
      } catch {
        Say ("FIXTURE_ERROR=" + $spec.N + " " + $_.Exception.Message)
      }
    }
    Say ("FIXTURES=" + ($fixtureMade -join ','))
  }
}

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
      Hwnd = $h; Id = [RS3]::GetDlgCtrlID($h); Cls = [RS3]::Cls($h); X = $pt.X; Y = $pt.Y
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

# ── 文本层：截断与溢出（2026-10-27 补）────────────────────────────────────
# 前三条只看「框在哪」；文字比框宽、或行数比框高，框本身完全合规却看不见内容。
# 判据与主窗口一致：
#   BUTTON/STATIC  → 最宽一行的像素宽 > 客户区宽 = 截断
#   ES_MULTILINE EDIT → 行数 × **行距**（tmHeight + externalLeading）> 可用高 = 溢出
#                       （按字高算会把「9 行 108px 装得下」判成合格，而屏幕上最后一行被切半截）
$clipN = 0; $ovfN = 0
foreach ($r in $script:rects) {
  $cr = New-Object RS3+RECT
  [void][RS3]::GetClientRect([IntPtr]$r.Hwnd, [ref]$cr)
  $cw = $cr.Right - $cr.Left
  $chh = $cr.Bottom - $cr.Top
  if ($r.Cls -eq 'Edit') {
    $style = [RS3]::GetWindowLong([IntPtr]$r.Hwnd, -16)
    if (($style -band 0x0004) -ne 0) {   # ES_MULTILINE
      $lines = [RS3]::LineCountOf([IntPtr]$r.Hwnd)
      $pitch = [RS3]::LinePitch([IntPtr]$r.Hwnd)
      $availH = $chh - 4               # 单像素边框上下各 1，留 2 缓冲
      if ($pitch -gt 0 -and $lines -gt 0 -and ($lines * $pitch) -gt $availH) {
        $ovfN++
        Say ("  OVERFLOW id=" + $r.Id + " lines=" + $lines + " pitch=" + $pitch + " need=" + ($lines * $pitch) + " availH=" + $availH)
      }
    }
  } elseif ($r.Cls -eq 'Button' -or $r.Cls -eq 'Static') {
    $tw = [RS3]::WidestLine([IntPtr]$r.Hwnd)
    if ($tw -gt $cw) {
      $clipN++
      Say ("  CLIP id=" + $r.Id + " clientW=" + $cw + " textW=" + $tw + " text=" + [RS3]::TextOf([IntPtr]$r.Hwnd))
    }
  }
}
Say ("TEXT_CLIPPED=" + $clipN)
Say ("TEXT_OVERFLOW=" + $ovfN)

# ── 潜在状态探测：零任务态的标签文字放不放得下 ──────────────────────────────
# 这坑已经踩过一次：v2.6.67 的零态文案是「任务：0 个（尚无任务记录）」，
# 几何判据实测 **156 px** 而框只有 **80 px** → 装不下，而当时的 50 条任务让这个
# 分支永远跑不到（判据在「有任务」的状态下全绿）。2.6.68 把说明挪到提示条，
# 标签只剩「任务：N 个」。
# 这里做两件事：
#   ① 量零态文案（与 native_gui.rs 的零态文本保持一致；改文案要同步这里）；
#   ② 钉住格式不变量：标签只能是「任务：N 个」/「Tasks: N」，不许再塞长句。
$lbl = [RS3]::GetDlgItem($win, 1400)
if ($lbl -ne [IntPtr]::Zero) {
  $orig = [RS3]::TextOf($lbl)
  $zero = "任务：0 个"
  [void][RS3]::SetWindowTextW($lbl, $zero)
  Start-Sleep -Milliseconds 120
  # 回读核对：写没写进去必须先确认，否则就是「拿旧文字量尺寸」还理直气壮。
  $nowLen = ([RS3]::TextOf($lbl)).Length
  $zcr = New-Object RS3+RECT
  [void][RS3]::GetClientRect($lbl, [ref]$zcr)
  $zcw = $zcr.Right - $zcr.Left
  $ztw = [RS3]::WidestLine($lbl)
  [void][RS3]::SetWindowTextW($lbl, $orig)   # 还原，别改坏界面
  Say ("EMPTY_LABEL_SETTEXT_LEN=" + $nowLen + " (want 6)")
  Say ("EMPTY_LABEL_CLIENT_W=" + $zcw + " EMPTY_LABEL_TEXT_W=" + $ztw)
  Say ("EMPTY_LABEL_FITS=" + [int]($ztw -le $zcw))
  # 格式不变量：现在的标签必须已经是短格式（长句只能出现在提示条）。
  Say ("LABEL_FORMAT_SHORT=" + [int](($orig -match '^任务：\d+ 个$') -or ($orig -match '^Tasks: \d+$')))
}

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

# ── 行为 T3：一个坏记录不能让整窗打不开/让列表少项 ─────────────────────────
# 判据 = 「坏记录确实在盘上」且「列表项数仍等于目录数」。只验前者太弱（可能整窗没开），
# 只验后者也弱（可能坏目录被静默跳过）——必须两条同时成立。
$badPresent = 0
if ($tasksRoot) { $badPresent = [int](Test-Path -LiteralPath (Join-Path $tasksRoot $FIX_BAD)) }
Say ("T3_BAD_FIXTURE_PRESENT=" + $badPresent)
Say ("T3_LIST_COMPLETE_WITH_BAD=" + [int]($badPresent -eq 1 -and $items -ge 0 -and $items -eq $dirCount))

# ── 行为 T2：选中一个失败任务，详情面板必须给出它的错误码与任务号 ──────────
# 为什么不用「逐项选中再读文案」：50 项 × 每次 200ms，慢且容易抽风。
# 这里按磁盘事实算出「失败任务在列表里的预期位置」，选中后**用详情里的任务号反证**
# ——若任务号对不上，说明排序假设错了（报 SELECT_ID_MATCH=0），不能算 T2 通过。
$T2 = 0; $selMatch = -1; $wantId = ''; $wantCode = ''
if ($exe -and $dirCount -gt 0) {
  $tasksDir = Join-Path (Split-Path -Parent $exe) 'tasks'
  $all = @()
  foreach ($d in (Get-ChildItem -LiteralPath $tasksDir -Directory -ErrorAction SilentlyContinue)) {
    $sp = Join-Path $d.FullName 'status.json'
    if (-not (Test-Path -LiteralPath $sp)) { continue }
    try { $st = Get-Content -LiteralPath $sp -Raw -Encoding UTF8 | ConvertFrom-Json } catch { continue }
    $code = ''
    if ($st.errorCodeText) { $code = [string]$st.errorCodeText }
    if ($code -notlike 'BR-*') { $code = '' }
    if (-not $code -and $st.error) {
      $m = [regex]::Match([string]$st.error, 'BR-[A-Z]+-\d{3}')
      if ($m.Success) { $code = $m.Value }
    }
    if (-not $code) { continue }
    $all += [pscustomobject]@{ Id = $d.Name; Code = $code; Updated = [string]$st.updated }
  }
  if ($all.Count -gt 0) {
    # 与 core 的 sort_rows 同规则：更新时间倒序，同一时刻按 id 升序。
    $sorted = @($all | Sort-Object @{Expression={[string]$_.Updated}; Descending=$true}, @{Expression={[string]$_.Id}; Ascending=$true})
    $pick = $sorted[0]
    $wantId = $pick.Id; $wantCode = $pick.Code
    # ★ 不按「算出来的位置」去选：第一次这么干，选 index 0 读到的却是别的任务
    #   （详情 283 字符、就是不含夹具 id）。位置依产品内部排序，靠猜只会得到
    #   「判据红但不知为什么」。改成**逐个选中、按 id 找**，并把落在第几项打出来
    #   ——既定位又自证。找到后用同一份详情验错误码。
    $foundAt = -1; $detail = ''
    $limit = [Math]::Min($items, 60)
    for ($i = 0; $i -lt $limit; $i++) {
      [void][RS3]::PostMessageW($combo, 0x014E, [IntPtr]$i, [IntPtr]::Zero)   # CB_SETCURSEL
      # ★ 关键（2026-10-27 实测踩到）：**`CB_SETCURSEL` 不发送 `CBN_SELCHANGE`**
      #   （Win32 明文规定），而详情面板是靠这条通知刷新的。只改选中 →
      #   详情一直是旧的（长度恒为 283），看起来像「详情不显示任务号」。
      #   用户点鼠标时通知由控件自己发；自动化要**补发等价通知**：
      #   WM_COMMAND, wParam = MAKEWPARAM(1401, CBN_SELCHANGE=1), lParam = combo。
      $wp = [IntPtr](1401 -bor (1 -shl 16))
      [void][RS3]::PostMessageW($win, 0x0111, $wp, $combo)
      # 轮询到详情里出现本项 id 为止（最多 5×150ms）：通知是异步的，固定等 200ms
      # 偶发读到「上一项」的详情——那会同时污染 T2 的 id 与码两个判据。
      $detail = ''
      for ($w = 0; $w -lt 5; $w++) {
        Start-Sleep -Milliseconds 150
        $detail = [RS3]::TextOf([IntPtr]([RS3]::GetDlgItem($win, 1402)))
        if ($detail -match [regex]::Escape($wantId)) { break }
      }
      # 前几轮把「选中项回读」也打出来：`CB_SETCURSEL` 发了没生效时，
      # 症状是「详情一直是同一个」，与「详情不显示任务号」长得一样。
      if ($i -lt 3) {
        $cs = [int][RS3]::SendMsg($combo, 0x0147, [IntPtr]::Zero, [IntPtr]::Zero)  # CB_GETCURSEL
        Say ("  SCAN i=" + $i + " cursel=" + $cs + " detailLen=" + $detail.Length)
      }
      if ($detail -match [regex]::Escape($wantId)) { $foundAt = $i; break }
    }
    Say ("T2_WANT_ID=" + $wantId + " T2_WANT_CODE=" + $wantCode + " T2_FOUND_AT=" + $foundAt)
    Say ("T2_SCANNED=" + $i + " T2_DETAIL_LEN=" + $detail.Length)
    if ($foundAt -ge 0) {
      $selMatch = 1
      $hasCode = [int]($detail -match [regex]::Escape($wantCode))
      # 诊断：wantCode 的码点 + 用「纯 ASCII 正则」再判一次。
      # 两者不一致 = 别名字符（全角连字符之类）在作怪；都 0 = 详情真的没有码。
      Say ("T2_WANT_CODE_CODES=" + (($wantCode.ToCharArray() | ForEach-Object { [int]$_ }) -join ','))
      Say ("T2_PLAIN_ASCII_MATCH=" + [int]($detail -match 'BR-PE-0[0-9][0-9]'))
      # 详情原文（非 ASCII 会被 run-body 的 ASCII 过滤掉，留下的正是 serde 英文错误文本）
      Say ("T2_DETAIL=" + ($detail -replace "[\r\n]+", " | "))
      Say ("T2_ID_MATCH=1 T2_DETAIL_HAS_CODE=" + $hasCode)
      # 两个条件同时成立才算通过：选中对了 + 详情里真有码。
      $T2 = [int]($hasCode -eq 1)
    } else {
      Say "T2_ID_MATCH=0 T2_DETAIL_HAS_CODE=0"
    }
  } else {
    Say "T2_NO_FAILED_TASK=1"
  }
}
Say ("T2_FAILED_DETAIL_SHOWS_CODE=" + $T2)

# ── 行为 T4：清理工作树的**确认框是真闸门**，且证据必须留着 ────────────────
# 两轮：
#   ① 点「否」→ 工作树一个字节都不许少（证明确认框不是装饰）；
#   ② 点「是」→ stage/payload 被删，但 task.json / status.json **必须还在**
#      （「证据是产品承诺」是文档里写明的一条，这里把它变成判据）。
# 说明：`cleanup_task_artifacts` 有三道守卫——未完成禁止清理、`status.stage` 必须
# 等于 `task.status`、`mount` 非空报 BR-CORE-060。夹具已按此构造。
$T4Cancel = 0; $T4Yes = 0; $T4Hint = 0
if ($foundAt -ge 0 -and $tasksRoot) {
  $fxDir = Join-Path $tasksRoot $wantId
  $btnClean = [RS3]::GetDlgItem($win, 1404)
  $enabled = [int]([RS3]::IsWindowEnabled($btnClean))
  Say ("T4_CLEANUP_ENABLED=" + $enabled)
  if ($enabled -eq 1) {
    # ---- 第一轮：点「否」----
    [void][RS3]::PostMessageW($btnClean, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)   # BM_CLICK
    Start-Sleep -Milliseconds 600
    $r1 = Click-DialogButton 7   # IDNO
    Start-Sleep -Milliseconds 300
    $stageStill = [int](Test-Path -LiteralPath (Join-Path $fxDir 'stage'))
    Say ("T4_CANCEL_DIALOG_RESULT=" + $r1 + " T4_CANCEL_STAGE_KEPT=" + $stageStill)
    $T4Cancel = [int]($r1 -ge 1 -and $stageStill -eq 1)
    # ---- 第二轮：点「是」----
    [void][RS3]::PostMessageW($btnClean, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 600
    $r2 = Click-DialogButton 6   # IDYES
    Start-Sleep -Milliseconds 800
    $stageGone = [int](-not (Test-Path -LiteralPath (Join-Path $fxDir 'stage')))
    $payloadGone = [int](-not (Test-Path -LiteralPath (Join-Path $fxDir 'payload')))
    $taskKept = [int](Test-Path -LiteralPath (Join-Path $fxDir 'task.json'))
    $statusKept = [int](Test-Path -LiteralPath (Join-Path $fxDir 'status.json'))
    $hint = [RS3]::TextOf([IntPtr]([RS3]::GetDlgItem($win, 1407)))
    Say ("T4_YES_DIALOG_RESULT=" + $r2 + " STAGE_GONE=" + $stageGone + " PAYLOAD_GONE=" + $payloadGone)
    Say ("T4_TASKJSON_KEPT=" + $taskKept + " T4_STATUSJSON_KEPT=" + $statusKept + " T4_HINT_LEN=" + $hint.Length)
    Say ("T4_HINT=" + ($hint -replace "[\r\n]+", " | "))
    $T4Hint = [int]($hint.Length -gt 0)
    $T4Yes = [int]($stageGone -eq 1 -and $payloadGone -eq 1 -and $taskKept -eq 1 -and $statusKept -eq 1)
  }
} else {
  Say "T4_SKIP=no_fixture"
}
Say ("T4_CANCEL_GUARDS=" + $T4Cancel)
Say ("T4_CLEANUP_KEEPS_EVIDENCE=" + $T4Yes)
Say ("T4_HINT_SHOWN=" + $T4Hint)
# 兜底：万一确认框还挂在屏幕上，先把程序救回来（模态框留着等于程序卡死）。
# ★ 先取到变量再判断：在方法参数位置上直接写函数调用，一旦函数返回的不是
#   「单个 IntPtr」，绑定失败会让这一句整条跳过（本轮实测 DIALOG_LEFT 那行凭空消失，
#   而后面的行照常输出——最难查的那种「少一行」）。
$dlgLeft = Find-Dialog
if ($dlgLeft -ne [IntPtr]::Zero) {
  [void][RS3]::PostMessageW($dlgLeft, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 300
  $dlgLeft = Find-Dialog
}
Say ("DIALOG_LEFT=" + [int]($dlgLeft -ne [IntPtr]::Zero))
# Close it: a modal window left up blocks the whole app for the user.
[void][RS3]::PostMessageW($win, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)   # WM_CLOSE
for ($i = 0; $i -lt 40; $i++) {
  Start-Sleep -Milliseconds 100
  if ((Find-Win 'BackupRestoreTaskAdmin') -eq [IntPtr]::Zero) { break }
}
Say ("CLOSED=" + [int]((Find-Win 'BackupRestoreTaskAdmin') -eq [IntPtr]::Zero))
# ── 行为 T5：关掉子窗口后主窗口必须还活着 ──────────────────────────────────
# 子窗口与主窗口同线程；若子窗口过程里误调 DefWindowProc/PostQuitMessage，
# 子窗口关掉会把整个程序的消息循环打掉——对用户就是「点了关闭，程序消失了」。
Say ("MAIN_ALIVE=" + [int]((Find-Win 'BackupRestoreNativeGui') -ne [IntPtr]::Zero))
# 夹具必须自己收走：留在 tasks 里会污染用户的真实任务列表。
Remove-Fixtures
$left = 0
if ($tasksRoot) { $left = @(@($FIX_FAIL, $FIX_BAD) | Where-Object { Test-Path -LiteralPath (Join-Path $tasksRoot $_) }).Count }
Say ("FIXTURES_REMOVED=" + [int]($left -eq 0))
Say "DONE"
