# _t-layout-rects.ps1 - ASCII only. Geometric acceptance for the main window.
#
# Read-only except for a click on each of the five operation tabs (which only
# changes the selected page; it creates no task and touches no disk).
#
# Why this exists (2026-10-26): the user found the top toolbar buttons drawn on
# top of the operation-mode buttons. Every acceptance case up to then asked
# "does control X exist / does it answer?" and none asked "do two controls own
# the same pixels?". The defect therefore survived several rounds of "all green".
#
# Three lessons baked in:
#   1. Enumerate ALL children instead of a hand-picked list. The 2.5.2 defect
#      (bottom button row covering the "verify hash" checkbox) was the same class
#      of bug and a hand-picked list missed it too.
#   2. Check every page, not just the one that happens to be open: each entry
#      has its own visible field set, and hidden rows move other rows around.
#   3. PostMessage, never SendMessage, for BM_CLICK. A synchronous send from this
#      helper deadlocks against the GUI's modal handling; the first version of
#      this script used SendMessage and silently died before writing anything,
#      which made a stale result file look like a pass. Hence the incremental
#      flush after every page: a hang is now visible in the file itself.
#
# Results go to a FILE: the CreateProcessAsUser child has no inherited stdout.
param(
    [string]$Out = 'H:\brwork\gui-layout-rects.txt'
)
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$OutputEncoding = [Text.Encoding]::UTF8

# Written first so that "file exists but is nearly empty" still tells us the
# script started; an absent file means the launcher or Add-Type failed.
Set-Content -LiteralPath $Out -Value 'STARTED' -Encoding ASCII
function Emit([string]$line) { Add-Content -LiteralPath $Out -Value $line -Encoding ASCII }

Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Lay {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
'@

$BM_CLICK = 0x00F5

$NAMES = @{
    1101='tab_backup'; 1102='tab_restore_c'; 1105='tab_restore_nonC';
    1106='tab_secondary'; 1104='tab_install_pe';
    1005='combo_language'; 2009='lbl_language'; 1007='btn_settings';
    1008='btn_cancel_pending'; 1009='btn_image_admin'; 1010='btn_view_logs';
    2001='lbl_operation'
}

$script:main = [IntPtr]::Zero
$cbMain = [Lay+EnumCb]{ param($h,$l) if ([Lay]::Cls($h) -eq 'BackupRestoreNativeGui') { $script:main = $h; return $false }; return $true }
[void][Lay]::EnumWindows($cbMain, [IntPtr]::Zero)
$main = $script:main
if ($main -eq [IntPtr]::Zero) {
    Set-Content -LiteralPath $Out -Value 'MAIN_WINDOW_NOT_FOUND' -Encoding ASCII
    exit 0
}

function Find-ChildById([IntPtr]$parent, [int]$id) {
    $script:hit = [IntPtr]::Zero
    $cb = [Lay+EnumCb]{ param($hh,$ll)
        if ([Lay]::GetParent($hh) -eq $parent -and [Lay]::GetDlgCtrlID($hh) -eq $id) { $script:hit = $hh; return $false }
        return $true }
    [void][Lay]::EnumChildWindows($parent, $cb, [IntPtr]::Zero)
    return $script:hit
}

function Get-ChildRects([IntPtr]$parent) {
    $cr = New-Object Lay+RECT
    [void][Lay]::GetClientRect($parent, [ref]$cr)
    $origin = New-Object Lay+POINT
    $origin.X = 0; $origin.Y = 0
    [void][Lay]::ClientToScreen($parent, [ref]$origin)

    $list = New-Object System.Collections.ArrayList
    $cb = [Lay+EnumCb]{
        param($h,$l)
        if ([Lay]::GetParent($h) -ne $parent) { return $true }    # direct children only
        if (-not [Lay]::IsWindowVisible($h)) { return $true }
        $r = New-Object Lay+RECT
        if (-not [Lay]::GetWindowRect($h, [ref]$r)) { return $true }
        $w = [int]($r.Right - $r.Left)
        $hh = [int]($r.Bottom - $r.Top)
        if ($w -le 0 -or $hh -le 0) { return $true }
        $id = [int][Lay]::GetDlgCtrlID($h)
        $name = $NAMES[$id]
        if (-not $name) { $name = ("{0}:{1}" -f [Lay]::Cls($h).ToLower(), $id) }
        [void]$list.Add([pscustomobject]@{
            Name=$name; X=[int]($r.Left - $origin.X); Y=[int]($r.Top - $origin.Y); W=$w; H=$hh
        })
        return $true
    }
    [void][Lay]::EnumChildWindows($parent, $cb, [IntPtr]::Zero)
    return @{ Rects = $list; W = $cr.Right; H = $cr.Bottom }
}

# 五个入口（2026-10-26 用户要求移除「探测」，第六个 tab 已删）
$tabs = @(
    @(1101,'backup'), @(1102,'restore_c'), @(1105,'restore_nonC'),
    @(1106,'secondary'), @(1104,'install_pe')
)

$totalOverlaps = 0
$pagesDone = 0
$first = $true
try {
    foreach ($t in $tabs) {
        $tabId = [int]$t[0]
        $tabName = [string]$t[1]
        $tabHwnd = Find-ChildById $main $tabId
        if ($tabHwnd -eq [IntPtr]::Zero) {
            Emit "PAGE $tabName TAB_NOT_FOUND"
            continue
        }
        [void][Lay]::PostMessageW($tabHwnd, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 500

        $snap = Get-ChildRects $main
        if ($first) { Emit "CLIENT_W=$($snap.W) CLIENT_H=$($snap.H)"; $first = $false }
        Emit "PAGE $tabName visible_children=$($snap.Rects.Count)"

        foreach ($rc in $snap.Rects) {
            Emit ("  RECT {0} x={1} y={2} w={3} h={4}" -f $rc.Name, $rc.X, $rc.Y, $rc.W, $rc.H)
        }

        $n = 0
        for ($i = 0; $i -lt $snap.Rects.Count; $i++) {
            for ($j = $i + 1; $j -lt $snap.Rects.Count; $j++) {
                $a = $snap.Rects[$i]; $b = $snap.Rects[$j]
                $ax = [int]$a.X; $ay = [int]$a.Y; $aw = [int]$a.W; $ah = [int]$a.H
                $bx = [int]$b.X; $by = [int]$b.Y; $bw = [int]$b.W; $bh = [int]$b.H
                $ox = [Math]::Min($ax + $aw, $bx + $bw) - [Math]::Max($ax, $bx)
                $oy = [Math]::Min($ay + $ah, $by + $bh) - [Math]::Max($ay, $by)
                if ($ox -gt 0 -and $oy -gt 0) {
                    $n++
                    Emit ("  OVERLAP {0} x {1} = {2}x{3} px" -f $a.Name, $b.Name, $ox, $oy)
                }
            }
        }
        Emit "  page_overlaps=$n"
        $totalOverlaps += $n
        $pagesDone++
    }
} catch {
    Emit ("SCRIPT_ERROR={0}" -f $_.Exception.Message)
}

# Leave the GUI on its default page again.
$back = Find-ChildById $main 1101
if ($back -ne [IntPtr]::Zero) {
    [void][Lay]::PostMessageW($back, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
}

Emit "OVERLAP_COUNT=$totalOverlaps"
Emit "PAGES_CHECKED=$pagesDone"
