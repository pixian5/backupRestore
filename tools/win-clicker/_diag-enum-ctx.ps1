# _diag-enum-ctx.ps1 - "why can't EnumWindows see the GUI?" 纯诊断，不做任何判据。
#
# 背景（2026-10-27 02:57）：_t-task-admin-layout.ps1 首跑 WINDOW=1，之后稳定
# GUI_NOT_FOUND，而 GUI 进程与主窗口都在。当时的结论只能写「疑窗口站/桌面差异」——
# 因为仪器**没有输出自己所在的环境**，所以「通道不对」与「窗口真没有」分不开。
# 这份探针就是补这一刀：把 PID / 会话 / 窗口站 / 桌面 / 候选窗口全打出来。
#
# 判读规则：
#   - SESSION=1 且 WINSTA=WinSta0 且 DESKTOP=Default → 通道是对的，找不到就是真没有；
#   - SESSION=0 或 WINSTA=Service-0x0-... → 说明令牌/桌面串错了，问题在 run-gui-token.ps1；
#   - TOP_LEVEL=0 而 GUI 进程在 → 环境不对（不是产品问题）。
$out = 'H:\brwork\diag-enum-ctx.txt'
Set-Content -Path $out -Value ("STARTED " + (Get-Date -Format o)) -Encoding UTF8
function Say($s) { $s | Add-Content -Path $out -Encoding UTF8 }

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Ctx {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetProcessWindowStation();
  [DllImport("user32.dll")] public static extern IntPtr GetThreadDesktop(uint tid);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)]
  public static extern bool GetUserObjectInformationW(IntPtr h, int idx, StringBuilder buf, int len, out int need);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static string Txt(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetWindowTextW(h, sb, 256); return sb.ToString(); }
  public static string ObjName(IntPtr h) {
    StringBuilder sb = new StringBuilder(256); int need;
    if (!GetUserObjectInformationW(h, 2, sb, 512, out need)) return "ERR";
    return sb.ToString();
  }
}
'@

# ---------- 1. 我到底是谁 ----------
Say ("PID=" + $PID)
Say ("PROC_SESSION=" + (Get-Process -Id $PID).SessionId)
Say ("WINSTA=" + [Ctx]::ObjName([Ctx]::GetProcessWindowStation()))
Say ("DESKTOP=" + [Ctx]::ObjName([Ctx]::GetThreadDesktop([Ctx]::GetCurrentThreadId())))
Say ("USER=" + [System.Security.Principal.WindowsIdentity]::GetCurrent().Name)
Say ("INTERACTIVE=" + [Environment]::UserInteractive)

# ---------- 2. GUI 进程在哪个会话 ----------
$procs = Get-Process BackupRestore -ErrorAction SilentlyContinue
if (-not $procs) { Say "PROC_COUNT=0" } else {
  Say ("PROC_COUNT=" + @($procs).Count)
  foreach ($p in $procs) { Say ("PROC pid=" + $p.Id + " session=" + $p.SessionId + " mainwnd=" + $p.MainWindowHandle) }
}

# ---------- 3. 顶层窗口清单（这是判据的读数侧）----------
$script:all = New-Object System.Collections.ArrayList
$cb = [Ctx+EnumCb]{ param($h,$l)
  [void]$script:all.Add([pscustomobject]@{ H=$h; Cls=[Ctx]::Cls($h); Txt=[Ctx]::Txt($h); Vis=[Ctx]::IsWindowVisible($h) })
  return $true }
[void][Ctx]::EnumWindows($cb, [IntPtr]::Zero)
Say ("TOP_LEVEL=" + $script:all.Count)
$found = 0
foreach ($w in $script:all) {
  if ($w.Cls -eq 'BackupRestoreNativeGui') { $found++; Say ("  MATCH_GUI h=" + $w.H + " vis=" + $w.Vis + " txt=" + $w.Txt) }
}
Say ("GUI_CLASS_MATCH=" + $found)
# 候选：名字里带 BackupRestore 的（类名或标题），防止类名登记变了却报「没有窗口」
Say "-- candidates --"
foreach ($w in $script:all) {
  if ($w.Cls -like '*Backup*' -or $w.Txt -like '*Backup*' -or $w.Cls -like '*Restore*') {
    Say ("  CAND h=" + $w.H + " cls=" + $w.Cls + " vis=" + $w.Vis + " txt=" + $w.Txt)
  }
}
Say "DONE"
